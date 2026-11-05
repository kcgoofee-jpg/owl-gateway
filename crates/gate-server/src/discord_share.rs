//! Per-member Discord channel authorization. Only encrypted webhook credentials are persisted.

use crate::error::{ApiError, ApiResult, GateError, HttpError, InternalError, ok_json};
use crate::session::{self, LoginMode, STATE_COOKIE};
use crate::state::AppState;
use axum::Router;
use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::get;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use ring::aead::{self, Aad, LessSafeKey, Nonce, UnboundKey};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::time::Duration;

const PREFIX: &str = "discord_share_destination:";

// No Debug implementation: this struct contains a bearer credential.
#[derive(Serialize, Deserialize)]
pub struct Destination {
    pub id: String,
    pub channel_id: String,
    pub guild_id: String,
    pub token: String,
}

pub fn router() -> Router<AppState> {
    Router::new().route(
        "/public/me/share-destination",
        get(status).post(connect).delete(disconnect),
    )
}

fn private(response: Response) -> Response {
    let mut response = response;
    response
        .headers_mut()
        .insert("cache-control", HeaderValue::from_static("no-store"));
    response
        .headers_mut()
        .insert("referrer-policy", HeaderValue::from_static("no-referrer"));
    response
}

async fn subject(state: &AppState, headers: &HeaderMap, write: bool) -> Result<String, ApiError> {
    if write && !session::origin_ok(state, headers) {
        return Err(HttpError::new(403, "来源校验失败").into());
    }
    let Some((id, key)) = session::member_state(state, headers).await? else {
        return Err(HttpError::new(401, "请先登录，再连接个人频道。").into());
    };
    if !key.is_some_and(|k| k.enabled()) {
        return Err(HttpError::new(403, "你还没有可用的成员凭据。").into());
    }
    Ok(id)
}

fn encryption_key(state: &AppState) -> Result<LessSafeKey, ApiError> {
    let key = Sha256::digest(format!("{}:discord-share-encryption-v1", state.secret_key));
    UnboundKey::new(&aead::AES_256_GCM, &key)
        .map(LessSafeKey::new)
        .map_err(|_| InternalError("个人频道凭据无法读取".into()).into())
}

fn seal(state: &AppState, subject: &str, dest: &Destination) -> Result<String, ApiError> {
    let nonce: [u8; 12] = rand::random();
    let mut bytes =
        serde_json::to_vec(dest).map_err(|_| InternalError("个人频道凭据无法保存".into()))?;
    encryption_key(state)?
        .seal_in_place_append_tag(
            Nonce::assume_unique_for_key(nonce),
            Aad::from(subject),
            &mut bytes,
        )
        .map_err(|_| InternalError("个人频道凭据无法保存".into()))?;
    Ok(format!(
        "v1.{}.{}",
        URL_SAFE_NO_PAD.encode(nonce),
        URL_SAFE_NO_PAD.encode(bytes)
    ))
}

pub async fn destination(state: &AppState, subject: &str) -> Result<Option<Destination>, ApiError> {
    let Some(raw) = state
        .db
        .get_setting(&format!("{PREFIX}{subject}"))
        .await?
        .filter(|s| !s.is_empty())
    else {
        return Ok(None);
    };
    let invalid = || InternalError("个人频道凭据无法读取，请重新连接。".into());
    let parts: Vec<_> = raw.split('.').collect();
    let ["v1", nonce, data] = parts.as_slice() else {
        return Err(invalid().into());
    };
    let nonce: [u8; 12] = URL_SAFE_NO_PAD
        .decode(nonce)
        .map_err(|_| invalid())?
        .try_into()
        .map_err(|_| invalid())?;
    let mut bytes = URL_SAFE_NO_PAD.decode(data).map_err(|_| invalid())?;
    let opened = encryption_key(state)?
        .open_in_place(
            Nonce::assume_unique_for_key(nonce),
            Aad::from(subject),
            &mut bytes,
        )
        .map_err(|_| invalid())?;
    let dest: Destination = serde_json::from_slice(opened).map_err(|_| invalid())?;
    if !valid_destination(&dest) {
        return Err(invalid().into());
    }
    Ok(Some(dest))
}

fn valid_id(s: &str) -> bool {
    !s.is_empty() && s.len() <= 20 && s.bytes().all(|b| b.is_ascii_digit())
}
fn valid_destination(d: &Destination) -> bool {
    valid_id(&d.id)
        && valid_id(&d.channel_id)
        && valid_id(&d.guild_id)
        && !d.token.is_empty()
        && d.token.len() <= 256
        && d.token
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

async fn status(State(state): State<AppState>, headers: HeaderMap) -> ApiResult {
    let id = subject(&state, &headers, false).await?;
    let data = match destination(&state, &id).await? {
        Some(d) => {
            json!({"connected": true, "binding_id": d.id, "channel_url": format!("https://discord.com/channels/{}/{}",d.guild_id,d.channel_id)})
        }
        None => json!({"connected": false}),
    };
    Ok(private(ok_json(&data)))
}

async fn connect(State(state): State<AppState>, headers: HeaderMap) -> ApiResult {
    let id = subject(&state, &headers, true).await?;
    let Some(cfg) = state.env.registration.as_ref() else {
        return Err(HttpError::new(503, "Discord 授权尚未配置。").into());
    };
    if !state
        .explore_uploads
        .check_then_push(&format!("connect:{id}"), 3, 60.0, state.now())
    {
        return Err(HttpError::new(429, "连接操作太频繁，请一分钟后重试。").into());
    }
    let nonce = URL_SAFE_NO_PAD.encode(rand::random::<[u8; 24]>());
    let signed = session::sign_login_state(
        &state.secret_key,
        LoginMode::Webhook,
        &nonce,
        &id,
        state.now() as i64,
    );
    let redirect = format!("{}login/callback", cfg.site_url);
    let query = crate::registration::urlencode(&[
        ("client_id", &cfg.client_id),
        ("redirect_uri", &redirect),
        ("response_type", "code"),
        ("scope", "identify webhook.incoming"),
        ("state", &signed),
    ]);
    let mut response = private(ok_json(
        &json!({"url":format!("https://discord.com/oauth2/authorize?{query}")}),
    ));
    response.headers_mut().append(
        "set-cookie",
        session::set_cookie_header(STATE_COOKIE, &signed, 600, true, "lax"),
    );
    Ok(response)
}

pub fn client() -> &'static reqwest::Client {
    static CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(gate_core::discord_ua::DISCORD_USER_AGENT)
            .build()
            .expect("fixed Discord client config")
    })
}

fn webhook_url(state: &AppState, d: &Destination) -> String {
    format!(
        "{}/webhooks/{}/{}",
        state.env.discord_api_base.trim_end_matches('/'),
        d.id,
        d.token
    )
}

pub fn result_page(ok: bool) -> Response {
    let nonce = URL_SAFE_NO_PAD.encode(rand::random::<[u8; 18]>());
    let message = if ok {
        "个人频道已连接。可以关闭此页，返回原工作台分享。"
    } else {
        "个人频道连接未完成，原有设置保持不变。请关闭此页，返回工作台重试。"
    };
    let script = if ok {
        format!("<script nonce=\"{nonce}\">window.close();</script>")
    } else {
        String::new()
    };
    let mut response = private(Html(format!("<!doctype html><html lang=zh-CN><meta charset=utf-8><meta name=viewport content=\"width=device-width,initial-scale=1\"><title>个人频道连接</title><body style=\"background:#111110;color:#eeeeec;font:16px/1.7 system-ui;padding:32px\"><h1>个人频道连接</h1><p>{message}</p>{script}</body></html>")).into_response());
    response.headers_mut().insert("content-security-policy", HeaderValue::from_str(&format!("default-src 'none'; style-src 'unsafe-inline'; script-src 'nonce-{nonce}'; base-uri 'none'; frame-ancestors 'none'")).expect("safe nonce"));
    response.headers_mut().append(
        "set-cookie",
        session::delete_cookie_header(STATE_COOKIE, true, "lax", true),
    );
    response
}

pub async fn callback(
    state: &AppState,
    headers: &HeaderMap,
    q: &HashMap<String, String>,
    bound: &str,
) -> ApiResult {
    let id = match subject(state, headers, false).await {
        Ok(id) if id == bound => id,
        _ => return Ok(result_page(false)),
    };
    let Some(cfg) = state.env.registration.as_ref() else {
        return Ok(result_page(false));
    };
    let redirect = format!("{}login/callback", cfg.site_url);
    let response = client()
        .post(format!(
            "{}/oauth2/token",
            state.env.discord_api_base.trim_end_matches('/')
        ))
        .form(&[
            ("client_id", cfg.client_id.as_str()),
            ("client_secret", cfg.client_secret.as_str()),
            ("grant_type", "authorization_code"),
            ("code", q.get("code").map_or("", String::as_str)),
            ("redirect_uri", redirect.as_str()),
        ])
        .send()
        .await;
    let Ok(response) = response else {
        return Ok(result_page(false));
    };
    if !response.status().is_success() {
        return Ok(result_page(false));
    }
    let Ok(data) = response.json::<Value>().await else {
        return Ok(result_page(false));
    };
    let Some(token) = data["access_token"].as_str().filter(|t| !t.is_empty()) else {
        return Ok(result_page(false));
    };
    // identify binds the newly authorized Discord account to the member who initiated the flow.
    let identity = client()
        .get(format!(
            "{}/users/@me",
            state.env.discord_api_base.trim_end_matches('/')
        ))
        .bearer_auth(token)
        .send()
        .await;
    let Ok(identity) = identity else {
        return Ok(result_page(false));
    };
    if !identity.status().is_success() {
        return Ok(result_page(false));
    }
    let Ok(identity) = identity.json::<Value>().await else {
        return Ok(result_page(false));
    };
    if identity["id"].as_str() != Some(id.as_str()) {
        return Ok(result_page(false));
    }
    let w = &data["webhook"];
    let dest = Destination {
        id: w["id"].as_str().unwrap_or_default().into(),
        token: w["token"].as_str().unwrap_or_default().into(),
        channel_id: w["channel_id"].as_str().unwrap_or_default().into(),
        guild_id: w["guild_id"].as_str().unwrap_or_default().into(),
    };
    if !valid_destination(&dest) {
        return Ok(result_page(false));
    }
    // Deliberately ignore webhook.url; build all requests from the configured Discord API origin.
    let old = destination(state, &id).await.ok().flatten();
    state
        .db
        .set_setting(&format!("{PREFIX}{id}"), &seal(state, &id, &dest)?)
        .await?;
    if let Some(old) = old.filter(|old| old.id != dest.id) {
        // Best effort: the new binding is already committed and must stay usable.
        let _ = client()
            .delete(webhook_url(state, &old))
            .timeout(Duration::from_secs(5))
            .send()
            .await;
    }
    Ok(result_page(true))
}

async fn disconnect(State(state): State<AppState>, headers: HeaderMap) -> ApiResult {
    let id = subject(&state, &headers, true).await?;
    let revoked = match destination(&state, &id).await? {
        Some(d) => client()
            .delete(webhook_url(&state, &d))
            .send()
            .await
            .is_ok_and(|r| r.status().is_success() || r.status().as_u16() == 404),
        None => true,
    };
    state.db.set_setting(&format!("{PREFIX}{id}"), "").await?;
    let mut response = private(ok_json(&json!({"ok":true,"remote_revoked":revoked})));
    response.headers_mut().append(
        "set-cookie",
        session::delete_cookie_header(STATE_COOKIE, true, "lax", true),
    );
    Ok(response)
}

pub async fn check_cooldown(state: &AppState, d: &Destination) -> Result<(), ApiError> {
    for key in [
        "discord_share_cooldown:global".to_string(),
        format!("discord_share_cooldown:{}", d.id),
    ] {
        if let Some(until) = state
            .db
            .get_setting(&key)
            .await?
            .and_then(|v| v.parse::<f64>().ok())
            .filter(|v| v.is_finite())
        {
            let wait = (until - state.now()).ceil().clamp(0.0, 3600.0) as u64;
            if wait > 0 {
                return Err(GateError::with_code(
                    429,
                    "Discord 频道正在冷却，请稍后再试。",
                    "discord_share_rate",
                )
                .retry_after(wait)
                .into());
            }
        }
    }
    Ok(())
}

pub async fn execute(
    state: &AppState,
    d: &Destination,
    form: reqwest::multipart::Form,
) -> Result<String, ApiError> {
    check_cooldown(state, d).await?;
    let response = client()
        .post(format!("{}?wait=true", webhook_url(state, d)))
        .multipart(form)
        .send()
        .await
        .map_err(|_| {
            HttpError::new(
                502,
                "Discord 投递结果未确认，请先核对个人频道，避免重复发送。",
            )
        })?;
    let status = response.status();
    let retry = response
        .headers()
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<f64>().ok());
    let data = response.json::<Value>().await.unwrap_or(Value::Null);
    if status.as_u16() == 429 {
        let wait = retry
            .or_else(|| data["retry_after"].as_f64())
            .filter(|n| n.is_finite())
            .unwrap_or(30.0)
            .ceil()
            .clamp(1.0, 3600.0) as u64;
        let key = if data["global"].as_bool() == Some(true) {
            "discord_share_cooldown:global".to_string()
        } else {
            format!("discord_share_cooldown:{}", d.id)
        };
        state
            .db
            .set_setting(&key, &(state.now() + wait as f64).to_string())
            .await?;
        return Err(GateError::with_code(
            429,
            "Discord 暂时限流，请稍后再试。",
            "discord_share_rate",
        )
        .retry_after(wait)
        .into());
    }
    if matches!(status.as_u16(), 401 | 403 | 404) {
        return Err(HttpError::new(
            409,
            "个人频道连接已失效或没有发送权限，请重新连接。本次不会转发到 OWL 公共频道。",
        )
        .into());
    }
    if !status.is_success() {
        return Err(HttpError::new(
            502,
            "Discord 未确认投递结果，请先核对个人频道，再决定是否重试。",
        )
        .into());
    }
    data["id"]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .ok_or_else(|| HttpError::new(502, "Discord 未返回消息确认，请先核对个人频道。").into())
}
