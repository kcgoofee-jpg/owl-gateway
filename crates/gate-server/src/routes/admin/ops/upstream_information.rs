//! Admin-only, manually refreshed operational projection from NovelAI `/user/information`.
//! Only an explicit allowlist is stored; the raw upstream response is never persisted or returned.

use crate::body::read_json_body;
use crate::error::{ApiResult, HttpError, ok_json};
use crate::session::{ADMIN_COOKIE, AdminSession, cookie};
use crate::state::AppState;
use axum::Router;
use axum::extract::{Request, State};
use axum::http::{HeaderMap, header};
use axum::response::IntoResponse;
use axum::routing::get;
use gate_upstream::client::Token;
use serde_json::{Map, Value, json};
use std::time::Duration;

const CACHE_KEY: &str = "upstream_account_information_projection";
const COOLDOWN_SECONDS: f64 = 300.0;
const STALE_SECONDS: f64 = 24.0 * 60.0 * 60.0;
const BODY_LIMIT: usize = 64 * 1024;

pub(super) fn router() -> Router<AppState> {
    Router::new().route("/admin/api/upstream-information", get(status).post(refresh))
}

pub(in crate::routes::admin) fn scoped_csrf(
    state: &AppState,
    scope: &str,
    session: &str,
) -> String {
    let key = crate::session::admin_key_from(
        &state.secret_key,
        &crate::session::admin_pw_material(state),
    );
    crate::session::hmac_hex(&key, &format!("{scope}:{session}"))
}

pub(in crate::routes::admin) fn origin_ok(headers: &HeaderMap) -> bool {
    let Some(origin) = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok()) else {
        return false;
    };
    let Some(host) = headers.get(header::HOST).and_then(|v| v.to_str().ok()) else {
        return false;
    };
    let Some((scheme, authority)) = origin.split_once("://") else {
        return false;
    };
    matches!(scheme, "http" | "https")
        && !authority.is_empty()
        && !authority.contains('/')
        && authority.eq_ignore_ascii_case(host)
}

fn empty_cache() -> Value {
    json!({"last_attempt_at": null, "accounts": {}})
}

async fn read_cache(state: &AppState) -> Result<Value, crate::error::ApiError> {
    let raw = state.db.get_setting(CACHE_KEY).await?;
    Ok(raw
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .filter(Value::is_object)
        .unwrap_or_else(empty_cache))
}

async fn write_cache(state: &AppState, cache: &Value) -> Result<(), crate::error::ApiError> {
    state.db.set_setting(CACHE_KEY, &cache.to_string()).await?;
    Ok(())
}

fn project(cache: &Value, tokens: &[std::sync::Arc<Token>], now: f64) -> Value {
    let last_attempt_at = cache.get("last_attempt_at").and_then(Value::as_f64);
    let retry_after = last_attempt_at
        .map(|t| (t + COOLDOWN_SECONDS - now).ceil().max(0.0) as i64)
        .unwrap_or(0);
    let cached = cache.get("accounts").and_then(Value::as_object);
    let accounts: Vec<Value> = tokens
        .iter()
        .enumerate()
        .map(|(index, token)| {
            let row = cached.and_then(|m| m.get(&token.token_id));
            let last_success = row
                .and_then(|r| r.get("last_success"))
                .and_then(Value::as_f64);
            let mut output = Map::new();
            output.insert("account".into(), json!(index + 1));
            for key in [
                "ban_status",
                "ban_message",
                "email_verified",
                "account_created_at",
                "trial_activated",
                "trial_actions_left",
                "trial_images_left",
                "last_success",
                "last_error",
                "last_error_at",
            ] {
                output.insert(
                    key.into(),
                    row.and_then(|r| r.get(key)).cloned().unwrap_or(Value::Null),
                );
            }
            output.insert(
                "stale".into(),
                json!(last_success.is_none_or(|t| now - t > STALE_SECONDS)),
            );
            Value::Object(output)
        })
        .collect();
    json!({
        "last_attempt_at": last_attempt_at,
        "accounts": accounts,
        "cooldown_seconds": COOLDOWN_SECONDS as i64,
        "retry_after": retry_after,
        "csrf_token": Value::Null,
    })
}

async fn status(State(state): State<AppState>, _session: AdminSession, req: Request) -> ApiResult {
    let (parts, _) = req.into_parts();
    let cache = read_cache(&state).await?;
    let tokens = state.nai.tokens().await;
    let mut out = project(&cache, &tokens, state.now());
    out["csrf_token"] = json!(scoped_csrf(
        &state,
        "upstream-information",
        &cookie(&parts.headers, ADMIN_COOKIE).unwrap_or_default()
    ));
    Ok(ok_json(&out))
}

fn information_error(status: Option<u16>, parse_error: bool) -> &'static str {
    if parse_error {
        "上游账号信息格式无法识别"
    } else {
        match status {
            None => "无法连接上游账号信息服务",
            Some(401) => "上游凭据被拒绝",
            Some(403) => "上游账号信息访问被拒绝",
            Some(429) => "上游账号信息查询受限",
            Some(200) => "上游账号信息读取失败",
            Some(_) => "上游账号信息暂时不可用",
        }
    }
}

fn project_upstream(body: &[u8]) -> Option<Map<String, Value>> {
    let value: Value = serde_json::from_slice(body).ok()?;
    let object = value.as_object()?;
    let mut out = Map::new();
    let text = |key: &str, max: usize| -> Option<Value> {
        let value = object.get(key)?;
        if value.is_null() {
            return Some(Value::Null);
        }
        let s = value.as_str()?;
        Some(Value::String(s.chars().take(max).collect()))
    };
    let bool_field = |key: &str| object.get(key).and_then(Value::as_bool).map(Value::Bool);
    let int_field = |key: &str| object.get(key).and_then(Value::as_i64).map(Value::from);
    for (key, value) in [
        ("ban_status", text("banStatus", 80)?),
        (
            "ban_message",
            text("banMessage", 1000).unwrap_or(Value::Null),
        ),
        (
            "email_verified",
            bool_field("emailVerified").unwrap_or(Value::Null),
        ),
        (
            "account_created_at",
            int_field("accountCreatedAt").unwrap_or(Value::Null),
        ),
        (
            "trial_activated",
            bool_field("trialActivated").unwrap_or(Value::Null),
        ),
        (
            "trial_actions_left",
            int_field("trialActionsLeft").unwrap_or(Value::Null),
        ),
        (
            "trial_images_left",
            int_field("trialImagesLeft").unwrap_or(Value::Null),
        ),
    ] {
        out.insert(key.into(), value);
    }
    Some(out)
}

async fn refresh(State(state): State<AppState>, _session: AdminSession, req: Request) -> ApiResult {
    let (parts, body) = req.into_parts();
    if !origin_ok(&parts.headers) {
        return Err(HttpError::new(403, "请求来源校验失败").into());
    }
    let session_cookie = cookie(&parts.headers, ADMIN_COOKIE).unwrap_or_default();
    let expected = scoped_csrf(&state, "upstream-information", &session_cookie);
    let given = crate::auth::header_str(&parts.headers, "x-nai-admin-csrf");
    if !crate::session::ct_eq(given.as_bytes(), expected.as_bytes()) {
        return Err(HttpError::new(403, "会话校验失败，请刷新后台后重试").into());
    }
    let ct = crate::auth::header_str(&parts.headers, "content-type");
    if ct.split(';').next().unwrap_or("").trim() != "application/json" {
        return Err(HttpError::new(415, "请使用 JSON 请求").into());
    }
    if !read_json_body(&parts.headers, body, 1024).await?.is_empty() {
        return Err(HttpError::new(400, "本操作不接受自定义账号或地址").into());
    }

    let _guard = state.ops.account_information.lock().await;
    let now = state.now();
    let mut cache = read_cache(&state).await?;
    if let Some(last) = cache.get("last_attempt_at").and_then(Value::as_f64) {
        let remaining = (last + COOLDOWN_SECONDS - now).ceil() as i64;
        if remaining > 0 {
            let mut response =
                HttpError::new(429, "上游账号信息查询冷却中，请稍后再试").into_response();
            if let Ok(v) = remaining.to_string().parse() {
                response.headers_mut().insert("retry-after", v);
            }
            return Ok(response);
        }
    }
    cache["last_attempt_at"] = json!(now);
    write_cache(&state, &cache).await?;

    let tokens = state.nai.tokens().await;
    let mut account_map = cache
        .get("accounts")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    for token in &tokens {
        let url = format!("{}/user/information", state.nai.image_host());
        let response = tokio::time::timeout(
            Duration::from_secs(8),
            state.nai.get_json_with(
                &token.token,
                token.profile_index(),
                &url,
                Duration::from_secs(8),
                BODY_LIMIT,
                false,
            ),
        )
        .await;
        let parsed = match response {
            Ok(Ok((401, _, _))) => {
                state.nai.isolate_unauthorized(token).await;
                let error = information_error(Some(401), false);
                let row = account_map
                    .entry(token.token_id.clone())
                    .or_insert_with(|| json!({}));
                row["last_error"] = json!(error);
                row["last_error_at"] = json!(now);
                continue;
            }
            Ok(Ok((200, _, body))) => project_upstream(&body),
            Ok(Ok((status, _, _))) => {
                let error = information_error(Some(status), false);
                let row = account_map
                    .entry(token.token_id.clone())
                    .or_insert_with(|| json!({}));
                row["last_error"] = json!(error);
                row["last_error_at"] = json!(now);
                continue;
            }
            Ok(Err(_)) | Err(_) => {
                let error = information_error(None, false);
                let row = account_map
                    .entry(token.token_id.clone())
                    .or_insert_with(|| json!({}));
                row["last_error"] = json!(error);
                row["last_error_at"] = json!(now);
                continue;
            }
        };
        match parsed {
            Some(mut projection) => {
                projection.insert("last_success".into(), json!(now));
                projection.insert("last_error".into(), Value::Null);
                projection.insert("last_error_at".into(), Value::Null);
                account_map.insert(token.token_id.clone(), Value::Object(projection));
            }
            None => {
                let row = account_map
                    .entry(token.token_id.clone())
                    .or_insert_with(|| json!({}));
                row["last_error"] = json!(information_error(Some(200), true));
                row["last_error_at"] = json!(now);
            }
        }
    }
    cache["accounts"] = Value::Object(account_map);
    write_cache(&state, &cache).await?;
    let mut out = project(&cache, &tokens, now);
    out["csrf_token"] = json!(expected);
    Ok(ok_json(&out))
}
