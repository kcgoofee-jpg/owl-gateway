//! Explicit owner-only reads of account metadata. Only cooldown timestamps survive the request.
use super::upstream_information::authorize_information_request;
use crate::error::{ApiResult, HttpError, ok_json};
use crate::session::AdminSession;
use crate::state::AppState;
use axum::{
    Router,
    extract::{Path, Request, State},
    response::IntoResponse,
    routing::post,
};
use gate_upstream::client::Token;
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};

const COOLDOWN: f64 = 60.0;
const MAX_BYTES: usize = 64 * 1024;

pub(super) fn router() -> Router<AppState> {
    Router::new().route("/admin/api/upstream/{token_id}/details", post(details))
}

// Keep unknown metadata, but never forward credentials, keystores or client settings.
fn redact(value: &mut Value, tokens: &[Arc<Token>]) {
    match value {
        Value::String(text) => {
            for token in tokens {
                *text = text.replace(&token.token, "[已隐藏]");
            }
        }
        Value::Array(items) => {
            for item in items {
                redact(item, tokens);
            }
        }
        Value::Object(object) => {
            let fields = std::mem::take(object);
            for (name, mut item) in fields {
                let normalized = name
                    .chars()
                    .filter(char::is_ascii_alphanumeric)
                    .flat_map(char::to_lowercase)
                    .collect::<String>();
                if [
                    "token",
                    "accesstoken",
                    "persistenttoken",
                    "apikey",
                    "privatekey",
                    "secretkey",
                    "refreshtoken",
                    "authorization",
                    "password",
                    "secret",
                    "accesskey",
                    "keystore",
                    "settings",
                ]
                .contains(&normalized.as_str())
                {
                    item = json!("[已隐藏]");
                } else {
                    redact(&mut item, tokens);
                }
                let mut safe_name = name;
                for token in tokens {
                    safe_name = safe_name.replace(&token.token, "[已隐藏]");
                }
                object.insert(safe_name, item);
            }
        }
        _ => {}
    }
}

async fn read_endpoint(
    state: &AppState,
    token: &Token,
    endpoint: &str,
    tokens: &[Arc<Token>],
) -> Value {
    let result = tokio::time::timeout(
        Duration::from_secs(8),
        state.nai.get_json_with(
            &token.token,
            token.profile_index(),
            &format!("{}/user/{endpoint}", state.nai.image_host()),
            Duration::from_secs(8),
            MAX_BYTES,
            false,
        ),
    )
    .await;
    match result {
        Ok(Ok((200, _, body))) => match serde_json::from_slice::<Value>(&body) {
            Ok(mut data) if data.is_object() => {
                redact(&mut data, tokens);
                json!({"status":200,"data":data,"error":null})
            }
            _ => json!({"status":200,"data":null,"error":"官方信息格式无效"}),
        },
        Ok(Ok((status, _, _))) => {
            if status == 401 {
                state.nai.isolate_unauthorized(token).await;
            }
            json!({"status":status,"data":null,"error":format!("官方接口返回 HTTP {status}")})
        }
        Ok(Err(_)) | Err(_) => json!({"status":null,"data":null,"error":"官方接口暂时无法连接"}),
    }
}

async fn details(
    State(state): State<AppState>,
    _session: AdminSession,
    Path(token_id): Path<String>,
    req: Request,
) -> ApiResult {
    authorize_information_request(&state, req).await?;
    let tokens = state.nai.tokens().await;
    let token = tokens
        .iter()
        .find(|token| token.token_id == token_id)
        .ok_or_else(|| HttpError::new(404, "上游 Token 不存在"))?;
    let now = state.now();
    {
        let mut attempts = state.ops.account_details_attempts.lock().await;
        attempts.retain(|id, _| tokens.iter().any(|token| token.token_id == *id));
        if let Some(last) = attempts.get(&token_id) {
            let remaining = (last + COOLDOWN - now).ceil() as i64;
            if remaining > 0 {
                let mut response = HttpError::new(429, "账号信息查询冷却中").into_response();
                response
                    .headers_mut()
                    .insert("retry-after", remaining.into());
                return Ok(response);
            }
        }
        attempts.insert(token_id.clone(), now);
    }
    let (information, subscription, priority) = tokio::join!(
        read_endpoint(&state, token, "information", &tokens),
        read_endpoint(&state, token, "subscription", &tokens),
        read_endpoint(&state, token, "priority", &tokens),
    );
    if !state
        .nai
        .tokens()
        .await
        .iter()
        .any(|token| token.token_id == token_id)
    {
        return Err(HttpError::new(409, "账号已变更，请重新打开详情").into());
    }
    let mut response = ok_json(
        &json!({"token_id":token_id,"queried_at":now,"retry_after":60,"information":information,"subscription":subscription,"priority":priority}),
    );
    response
        .headers_mut()
        .insert("cache-control", "no-store".parse().expect("constant"));
    Ok(response)
}
