//! 后台「上游」页的全部接口。
//!
//! 读数只有一个聚合接口 [`page`]（`GET /admin/api/upstream`）：每个上游账号一行，合并
//! 令牌池状态、官方读数（`/user/subscription` 的 V5 余量缓存、`/user/information` 的账号状态缓存）、
//! 本站观测（请求窗口、今日张数）和 Anlas 核对推算，外加核对历史、未知预留和两把会话 CSRF。
//! 页面一次取全，手动刷新官方信息（`POST /admin/api/upstream/refresh-info`）和核对（`POST /admin/api/reconciliation`）是仅有的两个会碰上游的动作。
//!
//! 上游 Token 的增删改都是敏感操作：完整 Token 永远不出现在响应、日志和操作记录里（响应里只有打码版本），
//! 每次变更都会私信站长（「如果这不是你操作的，请立刻更换后台密码」）。

use super::super::BODY_LIMIT;
use crate::body::read_json_body;
use crate::error::{ApiError, ApiResult, HttpError, InternalError, ok_json};
use crate::reconciliation::{self, RunError};
use crate::session::{ADMIN_COOKIE, AdminSession, cookie};
use crate::state::AppState;
use axum::Router;
use axum::body::Body;
use axum::extract::{Path, Request, State};
use axum::http::request::Parts;
use axum::routing::{get, post, put};
use gate_core::{primitives, validation, values};
use gate_upstream::{headers::token_id, token_store};
use serde_json::{Map, Value, json};

/// 上游 Token 请求体的上限：一个 Token 绰绰有余。
const TOKEN_BODY_LIMIT: usize = 4096;
const NOT_FOUND: &str = "上游 Token 不存在";

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/admin/api/upstream", get(page))
        .route("/admin/api/upstream-tokens", post(add_token))
        .route(
            "/admin/api/upstream-tokens/{token_id}",
            put(replace_token).delete(remove_token),
        )
        .route(
            "/admin/api/upstream-tokens/{token_id}/v5-limit",
            put(set_v5_limit),
        )
        .route(
            "/admin/api/upstream-tokens/{token_id}/enabled",
            put(set_enabled),
        )
        .route("/admin/api/reconciliation", post(reconcile))
        .route(
            "/admin/api/request-settlements/{request_id}/release",
            post(release_request_settlement),
        )
}

/// 「上游」页的一次性读数：账号行 + 核对 + 未知预留 + 冷却 + 会话 CSRF。
/// 除 `refresh-info` / `reconciliation` 两个动作外，页面不需要再发第二个请求。
async fn page(State(state): State<AppState>, _s: AdminSession, req: Request) -> ApiResult {
    let (parts, _) = req.into_parts();
    let now = state.now();
    let pool = pool_status(&state).await?;
    let stats = super::account_stats::account_rows(&state, &pool, now).await?;
    let ids: Vec<String> = pool
        .iter()
        .map(|p| p["token_id"].as_str().unwrap_or_default().to_string())
        .collect();
    let allowance = state.allowance.snapshot(&ids).await?;
    let allowance_rows = allowance["accounts"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let info = super::upstream_information::cached(&state).await?;
    let info_refresh = json!({
        "last_attempt_at": info["last_attempt_at"],
        "cooldown_seconds": info["cooldown_seconds"],
        "retry_after": info["retry_after"],
    });
    let info_rows = info["accounts"].as_array().cloned().unwrap_or_default();
    let info_by_id: Map<String, Value> = info_rows
        .iter()
        .filter_map(|r| r["token_id"].as_str().map(|id| (id.to_string(), r.clone())))
        .collect();
    let stats_by_id: Map<String, Value> = stats
        .iter()
        .filter_map(|r| r["token_id"].as_str().map(|id| (id.to_string(), r.clone())))
        .collect();

    let mut accounts = Vec::with_capacity(pool.len());
    for (i, p) in pool.iter().enumerate() {
        let mut row = p.clone();
        if let Some(id) = p["token_id"].as_str() {
            if let Some(st) = stats_by_id.get(id) {
                for key in ["w1h", "w24h", "load_share_24h", "balance_share", "balance"] {
                    row[key] = st[key].clone();
                }
            }
            // V5 官方余量（/user/subscription 的缓存），行序与 pool 一致
            row["v5"] = allowance_rows.get(i).cloned().unwrap_or(Value::Null);
            // 账号状态（/user/information 的缓存）
            row["info"] = info_by_id.get(id).cloned().unwrap_or(Value::Null);
        }
        accounts.push(row);
    }

    let recon = match reconciliation::status(&state).await {
        Ok(Value::Object(m)) => m,
        Ok(_) => Map::new(),
        Err(e) => return run_error(e),
    };
    let session = session_cookie(&parts);
    Ok(ok_json(&json!({
        "generated_at": values::float(now),
        "pool_configured": state.nai.configured().await,
        "accounts": accounts,
        "threshold": allowance["threshold"],
        "reconciliation": {
            "history": recon.get("history").cloned().unwrap_or(json!([])),
            "running": recon.get("running").cloned().unwrap_or(json!(false)),
            "retry_after": recon.get("retry_after").cloned().unwrap_or(json!(0)),
        },
        "settlements": state
            .db
            .list_unknown_request_settlements()
            .await?
            .into_iter()
            .map(|r| json!({
                "request_id": r.request_id, "rid": r.rid, "key_id": r.key_id, "status": r.status,
                "correlation_id": r.correlation_id,
                "service_day": r.day, "service_month": r.month, "reserved_anlas": r.anlas,
                "reserved_v5": r.v5, "reserved_legacy": r.legacy, "is_vibe": r.is_vibe
            }))
            .collect::<Vec<_>>(),
        "info_refresh": info_refresh,
        // 会话绑定：核对与释放预留用同一把；手动查官方信息是另一把
        "csrf_token": csrf_token(&state, &session),
        "csrf_info": super::upstream_information::scoped_csrf_public(&state, &session),
    })))
}

async fn release_request_settlement(
    State(state): State<AppState>,
    _s: AdminSession,
    Path(request_id): Path<String>,
    req: Request,
) -> ApiResult {
    let (parts, body) = req.into_parts();
    let expected = csrf_token(&state, &session_cookie(&parts));
    let given = crate::auth::header_str(&parts.headers, "x-nai-admin-csrf");
    if !crate::session::ct_eq(given.as_bytes(), expected.as_bytes()) {
        return Err(HttpError::new(403, "会话校验失败，请刷新后台后重试").into());
    }
    let value = read_json_body(&parts.headers, body, 1024).await?;
    let reason = value
        .get("reason")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    if value.len() != 2
        || value.get("confirmed_not_executed").and_then(Value::as_bool) != Some(true)
        || reason.chars().count() < 12
        || reason.chars().count() > 500
    {
        return Err(HttpError::new(
            400,
            "仅在人工确认该请求未执行后提交确认，并提供 12 至 500 字的核对说明",
        )
        .into());
    }
    let released = state
        .db
        .manually_release_unknown_request(&request_id, "站长", reason, state.now())
        .await?;
    if !released {
        return Err(HttpError::new(409, "请求不处于待核对状态").into());
    }
    crate::admission::release_reservations(&state, &request_id).await;
    Ok(ok_json(
        &json!({"ok":true,"status":"released_manual","csrf_token":expected}),
    ))
}

// ------------------------------------------------------------------ 令牌池状态

/// 令牌池的当前状态：Token 打码，另附当天用量。
pub(super) async fn pool_status(state: &AppState) -> Result<Vec<Value>, ApiError> {
    let now = state.now();
    let mut out = Vec::new();
    for (t, f) in state.nai.status_rows().await {
        let c = state
            .db
            .get_upstream_counter(&t.token_id, &state.day())
            .await?;
        // 今天出满（单号每日上限）：账号本身可用，但今天不会再接图
        let full = state
            .guard
            .with(|g| g.at_daily_cap(&t.token_id, c.images, now));
        out.push(json!({
            "token_id": t.token_id,
            "position": f.position,
            "token": gate_core::policy::mask_token(&t.token, 6),
            "usable": t.usable(now),
            "full": full,
            "admin_enabled": f.admin_enabled,
            "disabled": f.disabled,
            "fails": f.fails,
            "blocked_for": ((f.blocked_until - now) as i64).max(0),
            "last_ok": values::float(f.last_ok),
            "allow_anlas": f.allow_anlas,
            "image_concurrency": t.image_slots.limit(),
            "image_active": t.image_slots.active(),
            "v5_daily_limit": f.v5_daily_limit,
            "v5_used": c.v5,
            "images_today": c.images,
        }));
    }
    Ok(out)
}

/// 把令牌池写回 `upstream_tokens.json`（权限 600，不进数据库）。
async fn persist_pool(state: &AppState) -> Result<(), ApiError> {
    let entries: Vec<token_store::Entry> = state
        .nai
        .tokens()
        .await
        .iter()
        .map(|t| token_store::Entry {
            token: t.token.clone(),
            allow_anlas: t.fields().allow_anlas,
        })
        .collect();
    token_store::save(&state.env.upstream_tokens_path(), &entries)
        .map_err(|e| InternalError(format!("上游 Token 文件写入失败：{e}")))?;
    Ok(())
}

async fn exists(state: &AppState, id: &str) -> bool {
    state.nai.tokens().await.iter().any(|t| t.token_id == id)
}

/// 上游 Token 被改动：私信站长（告警，不是公告）。
fn alert_owner(state: &AppState, text: &str) {
    state.alerter.notify("upstream_token_changed", text, 0.0);
}

/// 请求体里的 Token（去掉首尾空白）：必须是 `pst-` 开头的持久 API Token。
async fn token_payload(
    parts: &Parts,
    body: Body,
) -> Result<(String, Map<String, Value>), ApiError> {
    let body = read_json_body(&parts.headers, body, TOKEN_BODY_LIMIT).await?;
    let token = body
        .get("token")
        .map(|v| primitives::strip(&validation::text(v)).to_string())
        .unwrap_or_default();
    if !token_store::valid_token(&token) {
        return Err(HttpError::new(
            422,
            "Token 格式不对：应以 pst- 开头（在 NovelAI 的 账号设置 → Get Persistent API Token 里获取）",
        )
        .into());
    }
    Ok((token, body))
}

/// 向 NovelAI 查订阅信息确认 Token 有效（只读，不消耗额度）。返回订阅等级。
/// `replacing`：替换时是旧 token_id（用那个位置的浏览器指纹），新增时为 None。
async fn verify(state: &AppState, token: &str, replacing: Option<&str>) -> Result<i64, ApiError> {
    state
        .nai
        .verify_token(token, replacing)
        .await
        .map_err(|msg| HttpError::new(422, msg).into())
}

// ------------------------------------------------------------------ 增删改

/// 添加一把上游 Token：先向 NovelAI 验证，通过后加入令牌池。
async fn add_token(State(state): State<AppState>, _s: AdminSession, req: Request) -> ApiResult {
    let (parts, body) = req.into_parts();
    let (token, body) = token_payload(&parts, body).await?;
    let tier = verify(&state, &token, None).await?;
    let id = token_id(&token);
    let saved_v5 = state
        .db
        .get_upstream_token_limits()
        .await?
        .into_iter()
        .find(|(k, _)| *k == id)
        .map(|(_, v)| v);
    let saved_enabled = state
        .db
        .get_upstream_token_enabled()
        .await?
        .into_iter()
        .find(|(k, _)| *k == id)
        .map(|(_, v)| v);
    let allow_anlas = body.get("allow_anlas").is_some_and(values::enabled);
    state
        .nai
        .add_token(token, allow_anlas, saved_v5, saved_enabled)
        .await
        .map_err(|msg| HttpError::new(409, msg))?;
    state
        .nai
        .clear_unauthorized(&id)
        .await
        .map_err(|e| InternalError(format!("验证后的上游凭据隔离状态清除失败：{e}")))?;
    persist_pool(&state).await?;
    alert_owner(
        &state,
        &format!(
            "后台添加了一把上游 Token（订阅等级 {tier}）。如果这不是你操作的，请立刻更换后台密码。"
        ),
    );
    Ok(ok_json(
        &json!({"ok": true, "tier": tier, "pool": pool_status(&state).await?}),
    ))
}

/// 替换某个槽位的 Token（例如在 NovelAI 重置了 Token 后粘贴新的）；V5 日限、启停、并发和当天计数延续。
async fn replace_token(
    State(state): State<AppState>,
    _s: AdminSession,
    Path(old_id): Path<String>,
    req: Request,
) -> ApiResult {
    let (parts, body) = req.into_parts();
    let (token, _) = token_payload(&parts, body).await?;
    let tier = verify(&state, &token, Some(&old_id)).await?;
    match state.nai.replace_token(&old_id, token).await {
        Ok((new, previous)) => {
            state
                .db
                .move_upstream_token(&previous, &new.token_id)
                .await?;
            state
                .nai
                .clear_unauthorized(&previous)
                .await
                .map_err(|e| InternalError(format!("验证后的上游凭据隔离状态清除失败：{e}")))?;
            state
                .nai
                .clear_unauthorized(&new.token_id)
                .await
                .map_err(|e| InternalError(format!("验证后的新上游凭据隔离状态清除失败：{e}")))?;
            persist_pool(&state).await?;
        }
        Err(msg) if exists(&state, &old_id).await => return Err(HttpError::new(409, msg).into()),
        Err(_) => return Err(HttpError::new(404, NOT_FOUND).into()),
    }
    alert_owner(
        &state,
        "后台替换了一把上游 Token。如果这不是你操作的，请立刻更换后台密码。",
    );
    Ok(ok_json(
        &json!({"ok": true, "tier": tier, "pool": pool_status(&state).await?}),
    ))
}

async fn remove_token(
    State(state): State<AppState>,
    _s: AdminSession,
    Path(id): Path<String>,
) -> ApiResult {
    match state.nai.remove_token(&id).await {
        Err(msg) => return Err(HttpError::new(409, msg).into()),
        Ok(false) => return Err(HttpError::new(404, NOT_FOUND).into()),
        Ok(true) => {}
    }
    persist_pool(&state).await?;
    alert_owner(&state, "后台删除了一把上游 Token。");
    Ok(ok_json(
        &json!({"ok": true, "pool": pool_status(&state).await?}),
    ))
}

async fn set_v5_limit(
    State(state): State<AppState>,
    _s: AdminSession,
    Path(id): Path<String>,
    req: Request,
) -> ApiResult {
    let (parts, body) = req.into_parts();
    let body = read_json_body(&parts.headers, body, BODY_LIMIT).await?;
    let limit = match body.get("v5_daily_limit").and_then(values::strict_int) {
        // 10-27 规范化：0 = 停用这把 Token 的免费 V5，上限最大 9999（存量 0 已迁移成 9999）
        Some(n) if (0..=9999).contains(&n) => n,
        _ => {
            return Err(HttpError::new(
                422,
                "上游 V5 日限额必须是 0～9999 的整数（0 = 停用免费 V5）",
            )
            .into());
        }
    };
    if !exists(&state, &id).await {
        return Err(HttpError::new(404, NOT_FOUND).into());
    }
    state.db.set_upstream_token_limit(&id, limit).await?;
    if !state.nai.set_v5_daily_limit(&id, limit).await {
        return Err(HttpError::new(404, NOT_FOUND).into());
    }
    Ok(ok_json(&json!({"ok": true, "v5_daily_limit": limit})))
}

async fn set_enabled(
    State(state): State<AppState>,
    _s: AdminSession,
    Path(id): Path<String>,
    req: Request,
) -> ApiResult {
    let (parts, body) = req.into_parts();
    let body = read_json_body(&parts.headers, body, BODY_LIMIT).await?;
    let Some(enabled) = body.get("enabled").and_then(Value::as_bool) else {
        return Err(HttpError::new(422, "enabled 必须是布尔值").into());
    };
    if !exists(&state, &id).await {
        return Err(HttpError::new(404, NOT_FOUND).into());
    }
    state.db.set_upstream_token_enabled(&id, enabled).await?;
    // 停用时会等这把 Token 手上的图跑完才返回：之后发出的请求一定能看到新状态
    if !state.nai.set_admin_enabled(&id, enabled).await {
        return Err(HttpError::new(404, NOT_FOUND).into());
    }
    Ok(ok_json(&json!({"ok": true, "enabled": enabled})))
}

// ------------------------------------------------------------------ Anlas 核对

/// 会话绑定的 CSRF 令牌：用管理员会话密钥对「reconciliation:<会话 Cookie>」做 HMAC。
/// 密钥与后台会话相同（`session::admin_key_from`），
/// 所以改密码 / 换会话都会让旧令牌失效。
fn csrf_for(secret_key: &str, password_material: &str, session_cookie: &str) -> String {
    let key = crate::session::admin_key_from(secret_key, password_material);
    crate::session::hmac_hex(&key, &format!("reconciliation:{session_cookie}"))
}

fn csrf_token(state: &AppState, session_cookie: &str) -> String {
    csrf_for(
        &state.secret_key,
        &crate::session::admin_pw_material(state),
        session_cookie,
    )
}

fn session_cookie(parts: &Parts) -> String {
    cookie(&parts.headers, ADMIN_COOKIE).unwrap_or_default()
}

fn run_error(e: RunError) -> ApiResult {
    match e {
        RunError::Internal(e) => Err(e),
        RunError::Refused(r) => {
            use axum::response::IntoResponse;
            let mut resp = HttpError::new(r.status, r.message).into_response();
            if r.retry_after != 0
                && let Ok(v) = r.retry_after.to_string().parse()
            {
                resp.headers_mut().insert("retry-after", v);
            }
            Ok(resp)
        }
    }
}

async fn reconcile(State(state): State<AppState>, _s: AdminSession, req: Request) -> ApiResult {
    let (parts, body) = req.into_parts();
    let expected = csrf_token(&state, &session_cookie(&parts));
    // 会话绑定的 CSRF：反向代理 / TLS 终止后也成立，且隔离同级站点
    let given = crate::auth::header_str(&parts.headers, "x-nai-admin-csrf");
    if !crate::session::ct_eq(given.as_bytes(), expected.as_bytes()) {
        return Err(HttpError::new(403, "会话校验失败，请刷新后台后重试").into());
    }
    let ct = crate::auth::header_str(&parts.headers, "content-type");
    if primitives::strip(ct.split(';').next().unwrap_or("")).to_lowercase() != "application/json" {
        return Err(HttpError::new(415, "请使用 JSON 请求").into());
    }
    if !read_json_body(&parts.headers, body, 1024).await?.is_empty() {
        return Err(HttpError::new(400, "本操作不接受自定义账号或地址").into());
    }
    let mut out = match reconciliation::run(&state).await {
        Ok(Value::Object(m)) => m,
        Ok(_) => Map::new(),
        Err(e) => return run_error(e),
    };
    out.insert("csrf_token".into(), Value::String(expected));
    Ok(ok_json(&Value::Object(out)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csrf_matches_python() {
        // python: hmac.new((SECRET + ":" + sha256(ADMIN_PASSWORD).hexdigest()).encode(),
        //                  b"reconciliation:1900000000.abc", sha256).hexdigest()
        assert_eq!(
            csrf_for(
                "test-secret-key-test-secret-key-0123456789",
                "dev-admin-password-123",
                "1900000000.abc"
            ),
            "c3b4dff9435fcebb29ab289c86573f7fa42746d3fae0a2df9f3ce485e376df58"
        );
    }
}
