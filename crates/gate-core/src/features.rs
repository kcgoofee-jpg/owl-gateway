//! 功能开关（对应 `app/features.py`）：站长全局开关每项上游功能，并按 Key 授权。
//!
//! 不在这里的（服务层负责）：
//! - `global_flags(db)`：`db.get_setting("features_global", None)` 读 `site_settings` 的 `features_global`（JSON 文本），
//!   交给 [`parse_global_flags`]；
//! - `check(db, key, name, settings)` 里的 `site_flags.get(...)`：只有 `name == "vibe"` 且前面没放行时才读
//!   `global_daily_anlas`（int）这个键，算法见 [`needs_vibe_site_flags`]；取到的值交给 [`check_decision`]。
//!   判定本身（[`check_decision`]）是纯函数。2.x 还读 `global_daily_anlas_until`（开放截止时间戳），
//!   后台一直没有写入口，3.0 删除。
//!
//! 已知边界：`features_global` 里若有 Python 才认的裸 `NaN` / `Infinity`，serde_json 会当成坏 JSON（→ 全部开启）。

use crate::{pyext, pyval};
use serde_json::Value;
use std::collections::BTreeMap;

/// 功能名 → 中文说明（顺序有意义：`normalize` / `dump` 按它排序）。
pub const FEATURES: [(&str, &str); 5] = [
    ("image", "文生图"),
    ("upscale", "放大"),
    ("augment", "导演工具（线稿、上色等）"),
    ("vibe", "Vibe 编码"),
    ("tags", "标签补全"),
];
pub const GLOBAL_KEY: &str = "features_global";
/// 需要消耗 Anlas 的功能：本站只提供免费出图，拒绝时说明原因，免得成员以为是 Key 坏了。
pub const ANLAS_FEATURES: [(&str, &str); 3] = [
    ("vibe", "每次编码参考图约 2 Anlas"),
    ("upscale", "按图片大小扣 Anlas"),
    ("augment", "按图片大小扣 Anlas"),
];

/// 用量日志 `usage_log.kind` → 功能。后台按功能统计与筛选日志时使用。
pub const KIND_FEATURE: [(&str, &str); 6] = [
    ("image", "image"),
    ("image_stream", "image"),
    ("upscale", "upscale"),
    ("augment-image", "augment"),
    ("vibe_encode", "vibe"),
    ("tags", "tags"),
];

/// 全局开关：功能名 → 是否开启。
pub type Flags = BTreeMap<&'static str, bool>;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FeaturesError {
    /// `features_global` 是合法 JSON 但不是对象（库里存了坏值才会出现；文案带 `AttributeError`
    /// 类型名前缀，调用方原样转成 500 上报）
    #[error("AttributeError: {0}")]
    Attribute(String),
    /// 未登记的功能名走到 `FEATURES[name]`
    #[error("KeyError: {0}")]
    Key(String),
    /// `name == "vibe"` 走到 Anlas 分支却没有给 [`VibeAnlas`]（调用方 bug）
    #[error("缺少 vibe 的全站 Anlas 设置")]
    MissingVibe,
}

fn label(name: &str) -> Result<&'static str, FeaturesError> {
    FEATURES
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, l)| *l)
        .ok_or_else(|| FeaturesError::Key(name.to_string()))
}

fn anlas_note(name: &str) -> Option<&'static str> {
    ANLAS_FEATURES
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, l)| *l)
}

/// `kinds_for(feature)`：该功能对应的所有 usage_log.kind（按 `KIND_FEATURE` 的顺序）。
pub fn kinds_for(feature: &str) -> Vec<&'static str> {
    KIND_FEATURE
        .iter()
        .filter(|(_, f)| *f == feature)
        .map(|(k, _)| *k)
        .collect()
}

/// `parse_list(value)`：把 Key 上的 features 字段解析为列表；None 表示沿用旧行为（全局开启的都可用）。
/// 保留重复项与出现顺序；空串得到空列表（不是 None）。
pub fn parse_list(value: &Value) -> Option<Vec<String>> {
    if value.is_null() {
        return None;
    }
    Some(
        pyext::py_str(value)
            .split(',')
            .filter(|n| FEATURES.iter().any(|(f, _)| f == n))
            .map(str::to_string)
            .collect(),
    )
}

/// `normalize(names)`：去重并按 `FEATURES` 的顺序排列，未登记的名字丢弃。
pub fn normalize<I, S>(names: I) -> Vec<&'static str>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let set: Vec<String> = names.into_iter().map(|s| s.as_ref().to_string()).collect();
    FEATURES
        .iter()
        .filter(|(n, _)| set.iter().any(|s| s == n))
        .map(|(n, _)| *n)
        .collect()
}

/// `dump(names)`：写回 `api_keys.features`；None 原样写 NULL。
pub fn dump<I, S>(names: Option<I>) -> Option<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    names.map(|n| normalize(n).join(","))
}

/// `key_features(key)`：`key` 是 `api_keys` 的一行（JSON 对象）；没有 `features` 列等同 NULL。
pub fn key_features(key: &Value) -> Option<Vec<String>> {
    pyval::get(key, "features").and_then(parse_list)
}

/// `global_flags(db)` 的解析部分：`raw` 是 `features_global` 的取值（没有这一行 = None）。
/// 缺省全部开启；坏 JSON 当作没存；只认布尔值。
pub fn parse_global_flags(raw: Option<&str>) -> Result<Flags, FeaturesError> {
    let mut flags: Flags = FEATURES.iter().map(|(n, _)| (*n, true)).collect();
    let saved = match raw {
        Some(text) if !text.is_empty() => match serde_json::from_str::<Value>(text) {
            Ok(v) => v,
            Err(_) => Value::Object(Default::default()),
        },
        _ => Value::Object(Default::default()),
    };
    let Value::Object(saved) = saved else {
        return Err(FeaturesError::Attribute(format!(
            "'{}' object has no attribute 'get'",
            pyext::type_name(&saved)
        )));
    };
    for (name, _) in FEATURES {
        if let Some(Value::Bool(b)) = saved.get(name) {
            flags.insert(name, *b);
        }
    }
    Ok(flags)
}

/// `site_flags.get(...)` 取到的 Vibe 全站 Anlas 设置。
#[derive(Debug, Clone, Copy)]
pub struct VibeAnlas {
    /// `global_daily_anlas`：全站每日 Anlas 额度（0 = 不开放 PVP）
    pub daily_cap: i64,
}

fn has_anlas(key: &Value) -> bool {
    pyval::get(key, "allow_anlas").is_some_and(pyval::truthy)
}

/// 走到 `check` 里读全站 Anlas 设置的那个分支吗？服务层用它决定要不要去读 `site_settings`
/// （惰性读取：只有真走到这里才读库）。`global_raw` 是 `features_global` 的取值。
/// 开关坏了（[`parse_global_flags`] 报错）时返回 false，随后 [`check_decision`] 会返回同一个错误。
pub fn needs_vibe_site_flags(global_raw: Option<&str>, key: &Value, name: &str) -> bool {
    let Ok(flags) = parse_global_flags(global_raw) else {
        return false;
    };
    if !flags.get(name).copied().unwrap_or(true) {
        return false;
    }
    if anlas_note(name).is_some() && has_anlas(key) {
        return false;
    }
    name == "vibe"
}

/// `check(db, key, name, settings)` 的判定部分：允许则 `Ok(None)`，否则 `Ok(Some(面向用户的拒绝原因))`。
/// `global_raw` 是 `features_global` 的取值（没有这一行 = None）。`vibe` 只有 [`needs_vibe_site_flags`] 为真时才会被用到。
pub fn check_decision(
    global_raw: Option<&str>,
    key: &Value,
    name: &str,
    vibe: Option<VibeAnlas>,
) -> Result<Option<String>, FeaturesError> {
    let flags = parse_global_flags(global_raw)?;
    if !flags.get(name).copied().unwrap_or(true) {
        return Ok(Some(format!(
            "本站未开放此功能：{}，未扣费。请在客户端中关闭该功能。",
            label(name)?
        )));
    }
    let allowed = key_features(key);
    if anlas_note(name).is_some() && has_anlas(key) {
        // 2026-10-10 放开 Anlas：有 Anlas 权限（含自动分配）的成员可以用付费功能，费用按每日 Anlas 上限扣
        return Ok(None);
    }
    if name == "vibe" {
        let v = vibe.ok_or(FeaturesError::MissingVibe)?;
        if v.daily_cap > 0 {
            // 针对 Vibe 开放全站每日 Anlas PVP 抢额度（由 quota_image_check 统一校验全站池）
            return Ok(None);
        }
    }
    if let Some(allowed) = allowed
        && !allowed.iter().any(|a| a == name)
    {
        if let Some(note) = anlas_note(name) {
            return Ok(Some(format!(
                "{}需要消耗 Anlas：{}。本站仅提供免费出图，未扣费。请在客户端中关闭该功能。",
                label(name)?,
                note
            )));
        }
        return Ok(Some(format!(
            "你的 Key 无权使用：{}，未扣费。如需开通，请联系站长。",
            label(name)?
        )));
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_and_dump() {
        assert_eq!(parse_list(&Value::Null), None);
        assert_eq!(
            parse_list(&json!("vibe,bogus,image")),
            Some(vec!["vibe".to_string(), "image".to_string()])
        );
        assert_eq!(
            dump(Some(["vibe", "text", "image", "zzz"])),
            Some("image,vibe".into())
        );
        assert_eq!(dump::<[&str; 0], &str>(None), None);
        assert_eq!(kinds_for("image"), ["image", "image_stream"]);
    }

    #[test]
    fn decision() {
        let flags = Some(r#"{"upscale": false, "tags": "no"}"#);
        let key = json!({"features": "image", "allow_anlas": 0});
        assert_eq!(
            check_decision(flags, &key, "upscale", None).unwrap(),
            Some("本站未开放此功能：放大，未扣费。请在客户端中关闭该功能。".into())
        );
        assert_eq!(
            check_decision(flags, &key, "tags", None).unwrap(),
            Some("你的 Key 无权使用：标签补全，未扣费。如需开通，请联系站长。".into())
        );
        assert!(parse_global_flags(Some("[1]")).is_err());
    }
}
