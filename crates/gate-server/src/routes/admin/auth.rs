//! 后台登录、退出、改密码、会话检查（旧版 admin.py 的 auth 部分）。

use super::BODY_LIMIT;
use crate::body::read_json_body;
use crate::error::{ApiError, HttpError, ok_json};
use crate::session::{
    ADMIN_COOKIE, ADMIN_SESSION_SECS, AdminSession, admin_password_ok, delete_cookie_header,
    hash_password, hit_login, make_admin_cookie, origin_ok, set_cookie_header,
};
use crate::state::AppState;
use axum::Router;
use axum::extract::{Request, State};
use axum::response::Response;
use axum::routing::{get, post, put};
use gate_core::pyval::py_str;
use serde_json::{Value, json};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/admin/api/login", post(login))
        .route("/admin/api/password", put(change_password))
        .route("/admin/api/logout", post(logout))
        .route("/admin/api/me", get(me))
}

fn field(body: &serde_json::Map<String, Value>, name: &str) -> String {
    body.get(name).map(py_str).unwrap_or_default()
}

async fn login(State(state): State<AppState>, req: Request) -> Result<Response, ApiError> {
    let (parts, body) = req.into_parts();
    if !origin_ok(&state, &parts.headers) {
        return Err(HttpError::new(403, "来源校验失败").into());
    }
    let ip = crate::auth::client_ip(&parts, &state.env.forwarded_allow_ips);
    if !hit_login(&state, &ip) {
        return Err(HttpError::new(429, "登录尝试过于频繁，请稍后再试").into());
    }
    let body = read_json_body(&parts.headers, body, BODY_LIMIT).await?;
    let password = field(&body, "password");
    let has_hash = state
        .admin_pw_hash
        .read()
        .expect("pw")
        .as_deref()
        .is_some_and(|s| !s.is_empty());
    if !has_hash {
        if state.env.admin_password.is_empty() {
            return Err(
                HttpError::new(503, "尚未设置 ADMIN_PASSWORD 环境变量，管理端已锁定").into(),
            );
        }
        if state.env.admin_password == "changeme-please" {
            return Err(HttpError::new(
                503,
                "ADMIN_PASSWORD 仍是示例值 changeme-please，请先在 .env 中改成强密码",
            )
            .into());
        }
    }
    if !admin_password_ok(&state, &password) {
        return Err(HttpError::new(401, "密码错误").into());
    }
    let mut resp = ok_json(&json!({"ok": true}));
    resp.headers_mut().append(
        "set-cookie",
        set_cookie_header(
            ADMIN_COOKIE,
            &make_admin_cookie(&state),
            ADMIN_SESSION_SECS,
            state.env.admin_cookie_secure,
            "strict",
        ),
    );
    Ok(resp)
}

/// 需要当前密码；成功后所有已登录会话（包括当前）立即失效，需用新密码重新登录。
async fn change_password(
    State(state): State<AppState>,
    _s: AdminSession,
    req: Request,
) -> Result<Response, ApiError> {
    let (parts, body) = req.into_parts();
    let ip = crate::auth::client_ip(&parts, &state.env.forwarded_allow_ips);
    // 同样受登录限流保护，防止被劫持会话后猜当前密码
    if !hit_login(&state, &ip) {
        return Err(HttpError::new(429, "尝试过于频繁，请稍后再试").into());
    }
    let body = read_json_body(&parts.headers, body, BODY_LIMIT).await?;
    let (current, new) = (field(&body, "current"), field(&body, "new"));
    if !admin_password_ok(&state, &current) {
        return Err(HttpError::new(401, "当前密码不正确").into());
    }
    if new.chars().count() < 12 || new == "changeme-please" || new == current {
        return Err(
            HttpError::new(422, "新密码至少 12 个字符，且不能与当前密码或示例密码相同").into(),
        );
    }
    let stored = hash_password(&new);
    state.db.set_setting("admin_password_hash", &stored).await?;
    *state.admin_pw_hash.write().expect("pw") = Some(stored);
    let mut resp = ok_json(&json!({"ok": true, "relogin": true}));
    resp.headers_mut().append(
        "set-cookie",
        delete_cookie_header(ADMIN_COOKIE, state.env.admin_cookie_secure, "strict", true),
    );
    Ok(resp)
}

async fn logout(State(state): State<AppState>) -> Response {
    let mut resp = ok_json(&json!({"ok": true}));
    resp.headers_mut().append(
        "set-cookie",
        delete_cookie_header(ADMIN_COOKIE, state.env.admin_cookie_secure, "strict", true),
    );
    resp
}

async fn me(_s: AdminSession) -> Response {
    ok_json(&json!({"ok": true}))
}
