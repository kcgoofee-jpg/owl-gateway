//! 参考图缓存：官网生图页会把同一张图第二次以后只发 `*_cache_secret_key`（数据不再重发）。
//! 上游会按这个 key 找自己缓存过的图，本站没有上游缓存，所以在进程内按 (Key, cache_secret_key)
//! 临时存一份原始 base64：第一次带 data 时写入，后续缺 data 时补回去。
//!
//! 只存有上限、有 TTL 的内存副本，不落库、不进日志。

use serde_json::Value;
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

const TTL: Duration = Duration::from_secs(30 * 60);
const MAX_ENTRIES: usize = 256;
const MAX_BYTES: usize = 64 * 1024 * 1024;

struct Entry {
    data: String,
    stored: Instant,
}

static CACHE: LazyLock<Mutex<HashMap<String, Entry>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn slot(key_id: i64, cache_key: &str) -> String {
    format!("{key_id}:{cache_key}")
}

fn prune(map: &mut HashMap<String, Entry>) {
    let now = Instant::now();
    map.retain(|_, e| now.duration_since(e.stored) < TTL);
    if map.len() <= MAX_ENTRIES {
        return;
    }
    let mut items: Vec<(String, Instant)> =
        map.iter().map(|(k, e)| (k.clone(), e.stored)).collect();
    items.sort_by_key(|(_, t)| *t);
    for (k, _) in items.into_iter().take(map.len() - MAX_ENTRIES) {
        map.remove(&k);
    }
}

/// 把请求里自带 data 的参考图按 cache_secret_key 记下来。
pub fn absorb(body: &Value, key_id: i64) {
    let Some(params) = body.get("parameters") else {
        return;
    };
    let mut pairs: Vec<(String, String)> = Vec::new();
    for field in [
        "reference_image_multiple_cached",
        "director_reference_images_cached",
    ] {
        if let Some(items) = params.get(field).and_then(Value::as_array) {
            for item in items {
                let key = item.get("cache_secret_key").and_then(Value::as_str);
                let data = item.get("data").and_then(Value::as_str);
                if let (Some(k), Some(d)) = (key, data)
                    && !d.is_empty()
                {
                    pairs.push((k.to_string(), d.to_string()));
                }
            }
        }
    }
    for (field, key_field) in [
        ("image", "image_cache_secret_key"),
        ("mask", "mask_cache_secret_key"),
        ("reference_image", "reference_image_cache_secret_key"),
    ] {
        if let (Some(k), Some(d)) = (
            params.get(key_field).and_then(Value::as_str),
            params.get(field).and_then(Value::as_str),
        ) && !d.is_empty()
        {
            pairs.push((k.to_string(), d.to_string()));
        }
    }
    if pairs.is_empty() {
        return;
    }
    let Ok(mut map) = CACHE.lock() else {
        return;
    };
    let now = Instant::now();
    for (k, data) in pairs {
        map.insert(slot(key_id, &k), Entry { data, stored: now });
    }
    prune(&mut map);
}

/// 请求里只有 cache_secret_key、没有 data 时，用缓存补回 data（补不到就原样返回，让校验报错）。
pub fn expand(body: &mut Value, key_id: i64) {
    let Ok(map) = CACHE.lock() else {
        return;
    };
    let now = Instant::now();
    let lookup = |k: &str| -> Option<String> {
        map.get(&slot(key_id, k))
            .filter(|e| now.duration_since(e.stored) < TTL)
            .map(|e| e.data.clone())
    };
    let Some(params) = body.get_mut("parameters").and_then(Value::as_object_mut) else {
        return;
    };
    for field in [
        "reference_image_multiple_cached",
        "director_reference_images_cached",
    ] {
        if let Some(items) = params.get_mut(field).and_then(Value::as_array_mut) {
            for item in items {
                let Some(obj) = item.as_object_mut() else {
                    continue;
                };
                let key = obj
                    .get("cache_secret_key")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                let has_data = obj
                    .get("data")
                    .and_then(Value::as_str)
                    .is_some_and(|d| !d.is_empty());
                if let (Some(k), false) = (key, has_data)
                    && let Some(data) = lookup(&k)
                {
                    obj.insert("data".into(), Value::String(data));
                }
            }
        }
    }
    for (field, key_field) in [
        ("image", "image_cache_secret_key"),
        ("mask", "mask_cache_secret_key"),
        ("reference_image", "reference_image_cache_secret_key"),
    ] {
        let key = params
            .get(key_field)
            .and_then(Value::as_str)
            .map(str::to_owned);
        let has_data = params
            .get(field)
            .and_then(Value::as_str)
            .is_some_and(|d| !d.is_empty());
        if let (Some(k), false) = (key, has_data)
            && let Some(data) = lookup(&k)
        {
            params.insert(field.into(), Value::String(data));
        }
    }
}

/// 当前缓存条目数（后台/测试观测用）。
pub fn size() -> usize {
    CACHE.lock().map(|m| m.len()).unwrap_or(0)
}

/// 单条上限检查：超过 MAX_BYTES 的图不缓存。
pub fn too_large(data: &str) -> bool {
    data.len() > MAX_BYTES
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn stores_and_refills_cached_references() {
        let key = "a".repeat(64);
        let body = json!({
            "parameters": {
                "director_reference_images_cached": [
                    {"cache_secret_key": key, "data": "QUJD"}
                ]
            }
        });
        absorb(&body, 7);

        // 同一条 Key 再来一次，只带 key 不带 data
        let mut second = json!({
            "parameters": {
                "director_reference_images_cached": [{"cache_secret_key": key}]
            }
        });
        expand(&mut second, 7);
        assert_eq!(
            second["parameters"]["director_reference_images_cached"][0]["data"],
            json!("QUJD")
        );

        // 别的 Key 不能蹭到同一份缓存
        let mut other = json!({
            "parameters": {
                "director_reference_images_cached": [{"cache_secret_key": key}]
            }
        });
        expand(&mut other, 8);
        assert!(
            other["parameters"]["director_reference_images_cached"][0]
                .get("data")
                .is_none()
        );
    }

    #[test]
    fn refills_scalar_image_cache() {
        let key = "b".repeat(64);
        absorb(
            &json!({"parameters": {"image": "QUJD", "image_cache_secret_key": key}}),
            3,
        );
        let mut body = json!({"parameters": {"image_cache_secret_key": key}});
        expand(&mut body, 3);
        assert_eq!(body["parameters"]["image"], json!("QUJD"));
    }
}
