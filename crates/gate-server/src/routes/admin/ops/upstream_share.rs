//! 拼车网关上游：独立 CRUD，不进官方「添加 pst-」对话框。
//! 添加时向对方 Host 查 `/user/subscription`，只要 `active === true`。完整 Key 不进响应和日志。

use super::upstream::{self, Edit};
use crate::body::read_json_body;
use crate::error::{ApiError, ApiResult, HttpError, ok_json};
use crate::session::AdminSession;
use crate::state::AppState;
use axum::Router;
use axum::extract::{Path, Request, State};
use axum::http::request::Parts;
use axum::routing::{get, put};
use gate_core::{primitives, validation, values};
use gate_upstream::{pool::Token, token_store};
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use std::time::Duration;

const TOKEN_BODY_LIMIT: usize = 4096;
const NOT_FOUND: &str = "拼车网关凭据不存在";

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/admin/api/share-upstreams", get(list).post(add))
        .route(
            "/admin/api/share-upstreams/{token_id}/settings",
            put(settings),
        )
        .route(
            "/admin/api/share-upstreams/{token_id}",
            put(replace).delete(remove),
        )
}

async fn list(State(state): State<AppState>, _s: AdminSession) -> ApiResult {
    let now = state.now();
    let day = state.day();
    let all = state.nai.tokens().await;
    let pool_size = all.len();
    // 捐赠 = 拼车 ∩ 绑定到启用赞助席的号；其余拼车才是朋友网关。来源标签据此区分（不影响选号）。
    let donated = state.db.donated_token_ids().await?;
    let shares: Vec<_> = all.into_iter().filter(|t| t.is_share()).collect();
    let stats_pool: Vec<Value> = shares
        .iter()
        .map(|token| json!({ "token_id": token.token_id }))
        .collect();
    let stats = super::account_stats::account_rows(&state, &stats_pool, now).await?;
    let stats_by_id: HashMap<String, Value> = stats
        .into_iter()
        .filter_map(|row| {
            let id = row["token_id"].as_str()?.to_string();
            Some((id, row))
        })
        .collect();
    let mut accounts = Vec::with_capacity(shares.len());
    for token in shares {
        let sub = fetch_subscription(&state, &token).await;
        let fields = token.fields();
        let used = state.db.get_upstream_counter(&token.token_id, &day).await?;
        let extra = stats_by_id.get(&token.token_id);
        accounts.push(json!({
            "token_id": token.token_id,
            "position": fields.kind_index,
            "token": gate_core::policy::mask_token(&token.token, 6),
            "image_host": token.image_host,
            "donated": donated.contains(&token.token_id),
            "usable": token.usable(now),
            "admin_enabled": fields.admin_enabled,
            "disabled": fields.disabled,
            "fails": fields.fails,
            "allow_anlas": fields.allow_anlas,
            "v5_daily_limit": fields.v5_daily_limit,
            "daily_anlas": fields.share_limits.as_ref().map_or(0, |s| s.daily_anlas),
            "rpm": fields.share_limits.as_ref().map_or(0, |s| s.rpm),
            "concurrency": token.image_slots.limit(),
            "post_wait_min": fields.share_limits.as_ref().map_or_else(|| state.guard.post_jitter().0 as i64, |s| s.post_wait_min),
            "post_wait_max": fields.share_limits.as_ref().map_or_else(|| state.guard.post_jitter().1 as i64, |s| s.post_wait_max),
            "anlas_used_today": state.db.upstream_anlas_used(&token.token_id, &day).await?,
            "v5_used": used.v5,
            "images_today": used.images,
            "w1h": extra.map(|row| row["w1h"].clone()).unwrap_or(Value::Null),
            "w24h": extra.map(|row| row["w24h"].clone()).unwrap_or(Value::Null),
            "active": sub.get("active").cloned().unwrap_or(Value::Null),
            "usage_percent": sub.get("usage_percent").cloned().unwrap_or(Value::Null),
            "anlas": sub.get("anlas").cloned().unwrap_or(Value::Null),
            "error": sub.get("error").cloned().unwrap_or(Value::Null),
        }));
    }
    Ok(ok_json(&json!({
        "ok": true,
        "pool_configured": state.nai.configured().await,
        "pool_size": pool_size,
        "fields": {
            "v5_daily_limit": {
                "kind": "int",
                "min": 0,
                "max": gate_core::member_policy::DAILY_MAX,
            }
        },
        "accounts": accounts,
    })))
}

async fn settings(
    State(state): State<AppState>,
    _s: AdminSession,
    Path(id): Path<String>,
    req: Request,
) -> ApiResult {
    let (parts, body) = req.into_parts();
    let body = read_json_body(&parts.headers, body, TOKEN_BODY_LIMIT).await?;
    share_token(&state, &id).await?;
    let Some(allow_anlas) = body.get("allow_anlas").and_then(Value::as_bool) else {
        return Err(HttpError::new(422, "allow_anlas 必须是布尔值").into());
    };
    let mut raw = body;
    raw.remove("allow_anlas");
    // Require every numeric setting; no silent defaults on malformed/partial requests.
    for key in [
        "daily_anlas",
        "rpm",
        "concurrency",
        "post_wait_min",
        "post_wait_max",
    ] {
        if !raw.contains_key(key) {
            return Err(HttpError::new(422, format!("缺少 {key}")).into());
        }
    }
    let limits: token_store::ShareLimits = serde_json::from_value(Value::Object(raw))
        .map_err(|_| HttpError::new(422, "拼车设置必须是有效整数，且不含未知字段"))?;
    if !limits.valid() {
        return Err(HttpError::new(422, "拼车设置超出范围，或等待时间上限小于下限").into());
    }
    upstream::edit_pool(
        &state,
        Edit::ShareSettings {
            id,
            allow_anlas,
            limits,
        },
    )
    .await?;
    Ok(ok_json(&json!({"ok": true})))
}

async fn add(State(state): State<AppState>, _s: AdminSession, req: Request) -> ApiResult {
    let (parts, body) = req.into_parts();
    let (token, host, allow_anlas) = share_payload(&parts, body, true).await?;
    let host = host.ok_or_else(|| {
        HttpError::new(
            422,
            "拼车网关地址不对：需要 https:// 开头的主机名，不要带路径",
        )
    })?;
    verify_share(&state, &token, &host, None).await?;
    upstream::edit_pool(
        &state,
        Edit::Add {
            token,
            allow_anlas,
            image_host: Some(host),
        },
    )
    .await?;
    upstream::alert_owner(
        &state,
        "后台添加了一条拼车网关凭据。如果这不是你操作的，请立刻更换后台密码。",
    );
    Ok(ok_json(&json!({"ok": true})))
}

async fn replace(
    State(state): State<AppState>,
    _s: AdminSession,
    Path(old_id): Path<String>,
    req: Request,
) -> ApiResult {
    let (parts, body) = req.into_parts();
    let (token, maybe_host, _) = share_payload(&parts, body, false).await?;
    let existing = share_token(&state, &old_id).await?;
    let host = match maybe_host {
        Some(h) => h,
        None => existing
            .image_host
            .clone()
            .ok_or_else(|| HttpError::new(404, NOT_FOUND))?,
    };
    verify_share(&state, &token, &host, Some(&old_id)).await?;
    upstream::edit_pool(
        &state,
        Edit::Replace {
            id: old_id,
            token,
            image_host: Some(host),
        },
    )
    .await?;
    upstream::alert_owner(
        &state,
        "后台替换了一条拼车网关凭据。如果这不是你操作的，请立刻更换后台密码。",
    );
    Ok(ok_json(&json!({"ok": true})))
}

async fn remove(
    State(state): State<AppState>,
    _s: AdminSession,
    Path(id): Path<String>,
) -> ApiResult {
    let _ = share_token(&state, &id).await?;
    upstream::edit_pool(&state, Edit::Remove { id }).await?;
    upstream::alert_owner(&state, "后台删除了一条拼车网关凭据。");
    Ok(ok_json(&json!({"ok": true})))
}

async fn share_payload(
    parts: &Parts,
    body: axum::body::Body,
    require_host: bool,
) -> Result<(String, Option<String>, bool), ApiError> {
    let body = read_json_body(&parts.headers, body, TOKEN_BODY_LIMIT).await?;
    let token = body
        .get("token")
        .map(|v| primitives::strip(&validation::text(v)).to_string())
        .unwrap_or_default();
    if !token_store::valid_share_token(&token) {
        return Err(
            HttpError::new(422, "Key 格式不对：拼车网关的 Key 应以 NAI- 或 pk- 开头").into(),
        );
    }
    let host = match body.get("image_host") {
        None | Some(Value::Null) if !require_host => None,
        other => {
            let raw = other
                .map(|v| primitives::strip(&validation::text(v)).to_string())
                .unwrap_or_default();
            let Some(host) = token_store::normalize_share_host(&raw) else {
                return Err(HttpError::new(
                    422,
                    "拼车网关地址不对：需要 https:// 开头的主机名，不要带路径",
                )
                .into());
            };
            Some(host)
        }
    };
    if require_host && host.is_none() {
        return Err(HttpError::new(
            422,
            "拼车网关地址不对：需要 https:// 开头的主机名，不要带路径",
        )
        .into());
    }
    let allow_anlas = body.get("allow_anlas").is_some_and(values::enabled);
    Ok((token, host, allow_anlas))
}

async fn share_token(state: &AppState, id: &str) -> Result<std::sync::Arc<Token>, ApiError> {
    state
        .nai
        .tokens()
        .await
        .into_iter()
        .find(|t| t.token_id == id && t.is_share())
        .ok_or_else(|| HttpError::new(404, NOT_FOUND).into())
}

async fn verify_share(
    state: &AppState,
    token: &str,
    host: &str,
    replacing: Option<&str>,
) -> Result<i64, ApiError> {
    state
        .nai
        .verify_token(token, replacing, Some(host))
        .await
        .map_err(|msg| {
            let mapped = if msg.contains("NovelAI 拒绝") {
                "拼车网关拒绝了这把 Key（无效或已失效）".into()
            } else if msg.contains("没有有效订阅") {
                "拼车网关说这份份额未生效".into()
            } else if msg.contains("无法连接 NovelAI") {
                "无法连接拼车网关验证这把 Key，请稍后再试".into()
            } else if let Some(rest) = msg.strip_prefix("NovelAI 返回 ") {
                format!("拼车网关返回 {rest}")
            } else {
                msg
            };
            HttpError::new(422, mapped).into()
        })
}

// ------------------------------------------------------------------ 赞助席自助

/// 赞助席自助启停自己捐赠的拼车 Key。与后台开关同一条路径；
/// 停用时会等这把 Key 手上在途的图跑完才返回。
pub(crate) async fn patron_set_enabled(
    state: &AppState,
    id: &str,
    enabled: bool,
) -> Result<(), ApiError> {
    share_token(state, id).await?;
    let _serial = state.token_edits.lock().await;
    state.db.set_upstream_token_enabled(id, enabled).await?;
    if !state.nai.set_admin_enabled(id, enabled).await {
        return Err(HttpError::new(404, NOT_FOUND).into());
    }
    upstream::alert_owner(
        state,
        "赞助席自助改动了一条拼车网关凭据的启停。如果这不是你授意的，请检查席位设置。",
    );
    Ok(())
}

/// 赞助席自助更换捐赠的拼车 Key：沿用原网关地址（不接受新地址），先验活再替换。
/// 返回新 token_id；席位重绑到新指纹由调用方完成（替换会解绑旧指纹）。
pub(crate) async fn patron_replace_token(
    state: &AppState,
    old_id: &str,
    token: &str,
) -> Result<String, ApiError> {
    let existing = share_token(state, old_id).await?;
    let Some(host) = existing.image_host.clone() else {
        return Err(HttpError::new(404, NOT_FOUND).into());
    };
    verify_share(state, token, &host, Some(old_id)).await?;
    upstream::edit_pool(
        state,
        Edit::Replace {
            id: old_id.to_string(),
            token: token.to_string(),
            image_host: Some(host),
        },
    )
    .await?;
    upstream::alert_owner(
        state,
        "赞助席自助替换了一条拼车网关凭据。如果这不是你授意的，请检查席位设置。",
    );
    Ok(gate_upstream::headers::token_id(token))
}

fn remaining(sub: &Value) -> Map<String, Value> {
    let mut out = Map::new();
    out.insert(
        "active".into(),
        json!(sub.get("active").and_then(Value::as_bool)),
    );
    let percent = sub
        .get("usage")
        .and_then(|u| u.get("percent"))
        .and_then(|v| v.as_i64().or_else(|| v.as_f64().map(|f| f.round() as i64)));
    out.insert("usage_percent".into(), json!(percent));
    let anlas = sub.get("trainingStepsLeft").map(|t| {
        let fixed = t
            .get("fixedTrainingStepsLeft")
            .and_then(Value::as_i64)
            .unwrap_or(0);
        let purchased = t
            .get("purchasedTrainingSteps")
            .and_then(Value::as_i64)
            .unwrap_or(0);
        fixed.saturating_add(purchased)
    });
    out.insert("anlas".into(), json!(anlas));
    out
}

async fn fetch_subscription(state: &AppState, token: &Token) -> Map<String, Value> {
    let Some(host) = token.image_host.as_deref() else {
        let mut err = Map::new();
        err.insert("error".into(), json!("缺少拼车网关地址"));
        return err;
    };
    let url = format!("{}/user/subscription", host.trim_end_matches('/'));
    let fetched = tokio::time::timeout(
        Duration::from_secs(8),
        state.nai.get_json_with(
            &token.token,
            token.profile_index(),
            &url,
            Duration::from_secs(8),
            64 * 1024,
            false,
        ),
    )
    .await;
    match fetched {
        Ok(Ok((200, _, body))) => match serde_json::from_slice::<Value>(&body) {
            Ok(sub) if sub.is_object() => remaining(&sub),
            _ => {
                let mut err = Map::new();
                err.insert("error".into(), json!("拼车网关返回了无法识别的份额信息"));
                err
            }
        },
        Ok(Ok((status, _, _))) => {
            if status == 401 {
                state.nai.isolate_unauthorized(token).await;
            }
            let mut err = Map::new();
            err.insert("error".into(), json!(format!("拼车网关返回 HTTP {status}")));
            err
        }
        _ => {
            let mut err = Map::new();
            err.insert("error".into(), json!("无法连接拼车网关"));
            err
        }
    }
}
