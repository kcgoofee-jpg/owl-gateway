//! NovelAI 客户端兼容接口和 tokenizer 资源。
use crate::auth::authenticate;
use crate::error::{ApiResult, InternalError, ok_json};
use crate::key::Key;
use crate::state::AppState;
use axum::Router;
use axum::body::Body;
use axum::extract::{Query, Request, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use futures::StreamExt;
use serde::Deserialize;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::io::Read;
use tokio::sync::OnceCell;
use tokio::time::Duration;

pub fn router() -> Router<AppState> {
    let mut r = Router::new().route("/public/tokenizer/{model}", get(public_tokenizer));
    for p in ["/user/subscription", "/ai/user/subscription"] {
        r = r.route(p, get(user_subscription));
    }
    for p in ["/user/data", "/ai/user/data"] {
        r = r.route(p, get(user_data));
    }
    for p in ["/user/priority", "/ai/user/priority"] {
        r = r.route(p, get(user_priority));
    }
    for p in ["/user/keystore", "/ai/user/keystore"] {
        r = r.route(p, get(user_keystore).put(user_keystore_put));
    }
    for p in ["/user/inventory", "/ai/user/inventory"] {
        r = r.route(p, get(user_inventory));
    }
    for p in ["/user/login", "/ai/user/login"] {
        r = r.route(p, post(user_login));
    }
    for p in ["/user/clientsettings", "/ai/user/clientsettings"] {
        r = r.route(p, get(user_settings).put(user_settings_put));
    }
    r = r.route("/ai/trial-status", get(trial_status));
    // 账单 / 礼包：本站没有付费和礼包体系，官网前端仍会探这两个接口，回空数组/空对象避免 404。
    r = r.route("/user/giftkeys", get(empty_list));
    for p in ["/user/cb/get-customer", "/ai/user/cb/get-customer"] {
        r = r.route(p, get(empty_object));
    }
    // 官网镜像页会探这些账号/对象接口：本站没有对应能力，回空结构而不是 404（404 会在页面里报错）
    r = r.route("/user/self", get(empty_object));
    for p in ["/usage/link", "/usage/event"] {
        r = r.route(p, post(empty_object)).route(p, get(empty_object));
    }
    for p in ["/user/objects", "/user/objects/{kind}"] {
        r = r.route(p, get(empty_objects));
    }
    for p in ["/user/information", "/ai/user/information"] {
        r = r.route(p, get(user_information));
    }
    r
}

// The URL pins NovelAI's v2 tokenizer assets; hashes below intentionally keep one stable model per release.
static QWEN_TOKENIZER: OnceCell<Vec<u8>> = OnceCell::const_new();
static T5_TOKENIZER: OnceCell<Vec<u8>> = OnceCell::const_new();
static CLIP_TOKENIZER: OnceCell<Vec<u8>> = OnceCell::const_new();
const TOKENIZER_COMPRESSED_LIMIT: usize = 8 * 1024 * 1024;
const TOKENIZER_JSON_LIMIT: usize = 32 * 1024 * 1024;

fn tokenizer_sha256(model: &str) -> Option<&'static str> {
    match model {
        "qwen" => Some("f4040d875827d2f9edc30dd4d736bc4a853a19ceeb0d04f6e3648f17796d1667"),
        "t5" => Some("3cbf698ff2c19efb5c75165d781c1d1b9c035baf119c3e71f1a380cbbfb9dfcb"),
        "clip" => Some("05852800e8ca59374d5b6d6d3993d9ccea63b7613dc827d12abff64f601a1744"),
        _ => None,
    }
}

fn valid_tokenizer_data(model: &str, compressed: &[u8]) -> bool {
    let Some(expected) = tokenizer_sha256(model) else {
        return false;
    };
    let digest = Sha256::digest(compressed);
    let actual: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    if actual != expected {
        return false;
    }
    let decoder = flate2::read::DeflateDecoder::new(compressed);
    let mut json_bytes = Vec::new();
    if decoder
        .take((TOKENIZER_JSON_LIMIT + 1) as u64)
        .read_to_end(&mut json_bytes)
        .is_err()
        || json_bytes.len() > TOKENIZER_JSON_LIMIT
    {
        return false;
    }
    let Ok(data) = serde_json::from_slice::<Value>(&json_bytes) else {
        return false;
    };
    match model {
        "qwen" => {
            data.get("config").is_some_and(Value::is_object)
                && data.get("vocab").is_some_and(Value::is_object)
                && data.get("merges").is_some_and(Value::is_array)
                && data.get("specialTokens").is_some_and(Value::is_array)
        }
        "t5" => {
            data.get("model")
                .and_then(|value| value.get("type"))
                .and_then(Value::as_str)
                == Some("Unigram")
                && data.get("normalizer").is_some()
                && data.get("pre_tokenizer").is_some()
        }
        "clip" => data
            .get("text")
            .and_then(Value::as_str)
            .is_some_and(|text| text.starts_with("bpe_simple_vocab_16e6.txt")),
        _ => false,
    }
}

#[derive(Deserialize)]
struct TokenizerQuery {
    sha256: String,
}

async fn public_tokenizer(
    axum::extract::Path(model): axum::extract::Path<String>,
    Query(query): Query<TokenizerQuery>,
) -> Response {
    let Some(expected_hash) = tokenizer_sha256(&model) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if query.sha256 != expected_hash {
        let mut response = StatusCode::CONFLICT.into_response();
        response
            .headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
        return response;
    }
    let (filename, cache) = match model.as_str() {
        "qwen" => ("qwen35_tokenizer.def", &QWEN_TOKENIZER),
        "t5" => ("t5_tokenizer.def", &T5_TOKENIZER),
        "clip" => ("clip_tokenizer.def", &CLIP_TOKENIZER),
        _ => return StatusCode::NOT_FOUND.into_response(),
    };
    let bytes = cache
        .get_or_try_init(|| async move {
            let client = reqwest::Client::builder()
                .timeout(Duration::from_secs(20))
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .map_err(|_| StatusCode::BAD_GATEWAY)?;
            let response = client
                .get(format!(
                    "https://novelai.net/tokenizer/compressed/{filename}?v=2&static=true"
                ))
                .send()
                .await
                .map_err(|_| StatusCode::BAD_GATEWAY)?;
            if !response.status().is_success() {
                return Err(StatusCode::BAD_GATEWAY);
            }
            if response
                .content_length()
                .is_some_and(|length| length > TOKENIZER_COMPRESSED_LIMIT as u64)
            {
                return Err(StatusCode::BAD_GATEWAY);
            }
            let mut stream = response.bytes_stream();
            let mut bytes = Vec::new();
            while let Some(chunk) = stream.next().await {
                let chunk = chunk.map_err(|_| StatusCode::BAD_GATEWAY)?;
                if bytes.len().saturating_add(chunk.len()) > TOKENIZER_COMPRESSED_LIMIT {
                    return Err(StatusCode::BAD_GATEWAY);
                }
                bytes.extend_from_slice(&chunk);
            }
            // Check the version-pinned static asset before caching it for this process lifetime.
            if !valid_tokenizer_data(model.as_str(), &bytes) {
                return Err(StatusCode::BAD_GATEWAY);
            }
            Ok::<_, StatusCode>(bytes)
        })
        .await;
    let Ok(bytes) = bytes else {
        return StatusCode::BAD_GATEWAY.into_response();
    };
    let mut response = Response::new(Body::from(bytes.clone()));
    *response.status_mut() = StatusCode::OK;
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=31536000, immutable"),
    );
    headers.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    response
}
// ------------------------------------------------------------------ 客户端兼容视图（/user/*）

/// 虚拟订阅视图：数字由 gate_core 算（纯函数），这里只负责把库里的数查好。
async fn subscription(state: &AppState, key: &Key) -> Result<Value, InternalError> {
    let (day, month) = (state.day(), state.month());
    let shared = state.db.shared_budgets().await?;
    let day_counter = state.db.get_counter(key.id(), &day).await?;
    let data = gate_core::client_views::SubscriptionData {
        month_anlas_used: state.db.month_anlas(key.id(), &month).await?,
        daily_anlas_used: day_counter.anlas,
        global_anlas_limit: f64::from(shared.daily_anlas),
        global_anlas_total: state.db.day_anlas_total(&day).await?,
        daily_v5_used: day_counter.v5,
        global_v5_limit: shared.daily_v5,
        global_v5_total: state.db.day_v5_total(&day).await?,
        v5_paused: crate::ops::v5_paused(&state.db).await?,
        daily_legacy_used: day_counter.legacy_free_images,
    };
    let mut view = gate_core::client_views::subscription_payload(
        &key.policy,
        key.expires_at(),
        state.now(),
        &data,
    );
    view["naiGate"]["ownerImageFull"] = json!(key.owner_image_full);
    view["naiGate"]["remaining"] = json!({
        "v45Images": (i64::from(key.policy.daily_images) - day_counter.legacy_free_images).max(0),
        "v5Images": view["naiGate"]["v5LeftToday"],
        "anlas": view["naiGate"]["anlasLeft"]
    });
    Ok(view)
}

/// 官方 /user/priority 的形状（2026-10-13 实测 image.novelai.net 返回 200）：
/// {maxPriorityActions, nextRefillAt, taskPriority}。本站没有「买优先队列」这回事，
/// 如实回 0 次优先操作、下个整点刷新，别编造余额。
fn virtual_priority(now: f64) -> Value {
    let now = now as i64;
    json!({"maxPriorityActions": 0, "nextRefillAt": now + (3600 - now % 3600), "taskPriority": 10})
}

/// 给原生 NAI 客户端的虚拟订阅视图，不暴露真实上游账户余额。
async fn user_subscription(State(state): State<AppState>, req: Request) -> ApiResult {
    let (parts, _) = req.into_parts();
    let key = authenticate(&state, &parts, false).await?;
    Ok(ok_json(&subscription(&state, &key).await?))
}

async fn user_data(State(state): State<AppState>, req: Request) -> ApiResult {
    let (parts, _) = req.into_parts();
    let key = authenticate(&state, &parts, false).await?;
    let sub = subscription(&state, &key).await?;
    let paused = crate::ops::v5_paused(&state.db).await?;
    let public_sub: Map<String, Value> = sub
        .as_object()
        .map(|m| {
            m.iter()
                .filter(|(k, _)| k.as_str() != "naiGate")
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect()
        })
        .unwrap_or_default();
    let steps = sub["trainingStepsLeft"].clone();
    // 官网生图页登录后必须拿到 keystore 才能进登录态（客户端自己加密，服务端只存密文）。
    let keystore = state
        .db
        .get_key_keystore(key.id())
        .await?
        .map(|(blob, change_index)| json!({"keystore": blob, "changeIndex": change_index}));
    // 官网页面按 settings.defaultModel 选默认模型；本站默认用 V4.5（V5 可能没给这把 Key 开）。
    // 界面语言默认中文：官网自带 zh-CN 翻译，uiLanguage 是客户端设置，改这里不用动前端。
    // 本地留图默认开启（persistImageGenHistory）：让刷新后的原生历史恢复开箱可用，只补缺失字段，
    // 不覆盖成员自己选过的任何设置，也不写库——默认值是读时补的，不需要数据迁移。
    let settings = state
        .db
        .get_key_settings(key.id())
        .await?
        .map(|raw| with_default_client_settings(&raw))
        .or_else(|| Some(DEFAULT_CLIENT_SETTINGS.to_string()));
    Ok(ok_json(&json!({
        "subscription": public_sub,
        "priority": virtual_priority(state.now()),
        "trainingStepsLeft": steps,
        "anlas": steps["fixedTrainingStepsLeft"],
        "v5Access": {"allowed": key.policy.allows("v5"), "paused": paused},
        "keystore": keystore,
        "settings": settings,
    })))
}

/// 新成员的首屏客户端设置：默认模型 V4.5 + 中文界面 + 本地留图默认开启。
pub const DEFAULT_CLIENT_SETTINGS: &str = r#"{"defaultModel":"nai-diffusion-4-5-curated","uiLanguage":"zh-CN","persistImageGenHistory":true}"#;

/// 客户端设置的站点默认值：`uiLanguage` 缺了补 `zh-CN`，`persistImageGenHistory` 缺了补 `true`。
/// 只补缺失字段；成员显式选过的值（包括 `false`）原样保留。解析不了就原样返回，一个字都不改。
/// 只在读响应时补，不写库：老成员下次拿到设置就是补过默认值的，前端 PUT 回来会自然固化。
fn with_default_client_settings(raw: &str) -> String {
    let Ok(mut value) = serde_json::from_str::<Value>(raw) else {
        return raw.to_string();
    };
    fill_default_client_settings(&mut value);
    value.to_string()
}

/// 对已解析的设置对象就地补缺省值；非对象原样跳过。
fn fill_default_client_settings(value: &mut Value) {
    let Some(obj) = value.as_object_mut() else {
        return;
    };
    if !obj.contains_key("uiLanguage") {
        obj.insert("uiLanguage".into(), json!("zh-CN"));
    }
    if !obj.contains_key("persistImageGenHistory") {
        obj.insert("persistImageGenHistory".into(), json!(true));
    }
}

/// 原生客户端会探这个端点；404 会被当成「渠道不支持」。官方是 200，我们也回同样的形状。
async fn user_priority(State(state): State<AppState>, req: Request) -> ApiResult {
    let (parts, _) = req.into_parts();
    authenticate(&state, &parts, false).await?;
    Ok(ok_json(&virtual_priority(state.now())))
}

/// 官网生图页的密钥库：客户端用本地钥匙加密后存这里，服务端只做不透明存取。
async fn user_keystore(State(state): State<AppState>, req: Request) -> ApiResult {
    let (parts, _) = req.into_parts();
    let key = authenticate(&state, &parts, false).await?;
    let stored = state.db.get_key_keystore(key.id()).await?;
    Ok(ok_json(&match stored {
        Some((blob, change_index)) => json!({"keystore": blob, "changeIndex": change_index}),
        None => json!({"keystore": null, "changeIndex": 0}),
    }))
}

#[derive(Deserialize)]
struct KeystoreBody {
    #[serde(default)]
    keystore: Option<String>,
    #[serde(default, rename = "changeIndex")]
    change_index: Option<i64>,
}

async fn user_keystore_put(State(state): State<AppState>, req: Request) -> ApiResult {
    let (parts, body) = req.into_parts();
    let key = authenticate(&state, &parts, false).await?;
    let bytes = axum::body::to_bytes(body, 1 << 20).await.map_err(|_| {
        crate::error::GateError::with_code(413, "密钥库太大", gate_core::reasons::PARAM)
    })?;
    let parsed: KeystoreBody = serde_json::from_slice(&bytes).map_err(|_| {
        crate::error::GateError::with_code(400, "密钥库格式无效", gate_core::reasons::PARAM)
    })?;
    let blob = parsed.keystore.unwrap_or_default();
    state
        .db
        .set_key_keystore(
            key.id(),
            &blob,
            parsed.change_index.unwrap_or(0),
            state.now(),
        )
        .await?;
    Ok(ok_json(&json!({"ok": true})))
}

/// 官网客户端设置：主题、默认模型、许可标记等整块 JSON。
/// 和 /user/data 一样补站点默认值：前端把这里读到的对象直接当设置用，
/// 缺了 persistImageGenHistory 会让本地留图退回官网默认（关）。
async fn user_settings(State(state): State<AppState>, req: Request) -> ApiResult {
    let (parts, _) = req.into_parts();
    let key = authenticate(&state, &parts, false).await?;
    let stored = state.db.get_key_settings(key.id()).await?;
    let mut value = match stored {
        Some(json) => serde_json::from_str::<Value>(&json)
            .unwrap_or_else(|_| serde_json::from_str(DEFAULT_CLIENT_SETTINGS).unwrap()),
        None => serde_json::from_str(DEFAULT_CLIENT_SETTINGS).unwrap(),
    };
    fill_default_client_settings(&mut value);
    Ok(ok_json(&value))
}

async fn user_settings_put(State(state): State<AppState>, req: Request) -> ApiResult {
    let (parts, body) = req.into_parts();
    let key = authenticate(&state, &parts, false).await?;
    let bytes = axum::body::to_bytes(body, 1 << 20).await.map_err(|_| {
        crate::error::GateError::with_code(413, "设置太大", gate_core::reasons::PARAM)
    })?;
    let text = String::from_utf8_lossy(&bytes).to_string();
    if serde_json::from_str::<Value>(&text).is_err() {
        return Err(crate::error::GateError::with_code(
            400,
            "设置格式无效",
            gate_core::reasons::PARAM,
        )
        .into());
    }
    state
        .db
        .set_key_settings(key.id(), &text, state.now())
        .await?;
    Ok(ok_json(&json!({"ok": true})))
}

/// 空对象：官网镜像页会探测、但本站没有对应能力的接口（/user/self、/usage/link）。
async fn empty_object() -> ApiResult {
    Ok(ok_json(&json!({})))
}

/// 官网前端把礼包当成数组直接 `.json()` 用，回 `[]` 比 `{}` 安全。
async fn empty_list() -> ApiResult {
    Ok(ok_json(&json!([])))
}

/// 官网对象库（提示词宏、预设、脚本…）：本站不存远端对象，回空列表。
async fn empty_objects(State(state): State<AppState>, req: Request) -> ApiResult {
    let (parts, _) = req.into_parts();
    authenticate(&state, &parts, false).await?;
    Ok(ok_json(&json!({"objects": []})))
}

/// 官网文本后端的试用额度：本站没有试用，回 0（前端不再提示试用次数）。
async fn trial_status(State(state): State<AppState>, req: Request) -> ApiResult {
    let (parts, _) = req.into_parts();
    authenticate(&state, &parts, false).await?;
    Ok(ok_json(&json!({
        "remaining_image_actions": 0,
        "remaining_text_actions": 0,
    })))
}

/// 官网背包：本站没有道具系统，回空列表（前端按空背包渲染）。
async fn user_inventory(State(state): State<AppState>, req: Request) -> ApiResult {
    let (parts, _) = req.into_parts();
    authenticate(&state, &parts, false).await?;
    Ok(ok_json(&json!({"items": []})))
}

#[derive(Deserialize)]
struct LoginBody {
    #[serde(default)]
    key: Option<String>,
    #[serde(default)]
    access_key: Option<String>,
}

/// 官网登录入口：body `{key, recaptcha}`，成功返回 `{accessToken}`；本站的 Key 本身就是令牌。
async fn user_login(State(state): State<AppState>, req: Request) -> ApiResult {
    let (_parts, body) = req.into_parts();
    let bytes = axum::body::to_bytes(body, 64 * 1024).await.map_err(|_| {
        crate::error::GateError::with_code(413, "登录请求太大", gate_core::reasons::PARAM)
    })?;
    let parsed: LoginBody = serde_json::from_slice(&bytes).map_err(|_| {
        crate::error::GateError::with_code(400, "登录请求格式无效", gate_core::reasons::PARAM)
    })?;
    let token = parsed
        .key
        .or(parsed.access_key)
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .ok_or_else(|| {
            crate::error::GateError::with_code(400, "缺少 Key", gate_core::reasons::PARAM)
        })?;
    let Some(row) = state.db.get_key_by_token(&token).await? else {
        return Err(crate::error::GateError::with_code(
            401,
            "API Key 无效。请检查客户端中的 Key 是否完整，或在 Discord 用 /resetkey 重新获取。",
            gate_core::reasons::KEY_INVALID,
        )
        .into());
    };
    if row["enabled"].as_i64().unwrap_or(1) != 1 {
        return Err(crate::error::GateError::with_code(
            403,
            "这个 Key 已停用。",
            gate_core::reasons::KEY_INVALID,
        )
        .into());
    }
    Ok(ok_json(&json!({"accessToken": token})))
}

async fn user_information(State(state): State<AppState>, req: Request) -> ApiResult {
    let (parts, _) = req.into_parts();
    let key = authenticate(&state, &parts, false).await?;
    let now = state.now();
    // int(key["expires_at"] or time.time() + 3650 * 86400)
    let expires = key
        .expires_at()
        .filter(|e| *e != 0.0)
        .map_or((now + 3650.0 * 86400.0) as i64, |e| e as i64);
    Ok(ok_json(&json!({
        "tier": 3, "active": true, "username": key.name(), "expiresAt": expires,
    })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenizer_asset_names_are_whitelisted_and_version_pinned() {
        assert!(tokenizer_sha256("qwen").is_some());
        assert!(tokenizer_sha256("t5").is_some());
        assert!(tokenizer_sha256("clip").is_some());
        assert!(tokenizer_sha256("https://example.com/file").is_none());
    }
}
#[cfg(test)]
mod client_settings_tests {
    use super::{
        DEFAULT_CLIENT_SETTINGS, fill_default_client_settings, with_default_client_settings,
    };

    #[test]
    fn adds_chinese_language_when_missing() {
        let out = with_default_client_settings(r#"{"defaultModel":"nai-diffusion-4-5-curated"}"#);
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["uiLanguage"], "zh-CN");
        assert_eq!(v["defaultModel"], "nai-diffusion-4-5-curated");
    }

    #[test]
    fn enables_local_image_history_by_default() {
        // 老成员的存量设置没有这个字段：读到响应时补 true（本地留图默认开启）。
        let out = with_default_client_settings(r#"{"uiLanguage":"zh-CN"}"#);
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["persistImageGenHistory"], true);
    }

    #[test]
    fn keeps_explicit_choices() {
        // 显式选过的值一律保留：语言、以及刻意关掉的本地留图。
        let raw = r#"{"uiLanguage":"en-US","persistImageGenHistory":false}"#;
        let v: serde_json::Value =
            serde_json::from_str(&with_default_client_settings(raw)).unwrap();
        assert_eq!(v["uiLanguage"], "en-US");
        assert_eq!(v["persistImageGenHistory"], false);
    }

    #[test]
    fn default_settings_carry_both_defaults() {
        let v: serde_json::Value = serde_json::from_str(DEFAULT_CLIENT_SETTINGS).unwrap();
        assert_eq!(v["defaultModel"], "nai-diffusion-4-5-curated");
        assert_eq!(v["uiLanguage"], "zh-CN");
        assert_eq!(v["persistImageGenHistory"], true);
    }

    #[test]
    fn leaves_broken_json_alone() {
        assert_eq!(with_default_client_settings("not json"), "not json");
        assert_eq!(with_default_client_settings("[1,2]"), "[1,2]");
    }

    #[test]
    fn fill_skips_non_objects() {
        let mut value = serde_json::json!([1, 2, 3]);
        fill_default_client_settings(&mut value);
        assert_eq!(value, serde_json::json!([1, 2, 3]));
    }
}
