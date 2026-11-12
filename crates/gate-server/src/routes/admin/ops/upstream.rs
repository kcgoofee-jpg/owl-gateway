//! 后台「上游」页的全部接口。
//!
//! 读数只有一个聚合接口 [`page`]（`GET /admin/api/upstream`）：每个上游账号一行，合并
//! 令牌池状态、官方读数（`/user/subscription` 的 V5 余量缓存、`/user/information` 的账号状态缓存）、
//! 本站观测（请求窗口、今日张数）和 Anlas 核对推算，外加核对历史、未知预留和两把会话 CSRF。
//! 页面一次取全，手动刷新官方信息（`POST /admin/api/upstream/refresh-info`）是唯一会碰上游的动作。
//!
//! 上游 Token 的增删改都是敏感操作：完整 Token 永远不出现在响应、日志和操作记录里（响应里只有打码版本），
//! 每次变更都会私信站长（「如果这不是你操作的，请立刻更换后台密码」）。

use super::super::BODY_LIMIT;
use crate::body::read_json_body;
use crate::error::{ApiError, ApiResult, HttpError, InternalError, ok_json};
use crate::session::{ADMIN_COOKIE, AdminSession, cookie};
use crate::state::AppState;
use axum::Router;
use axum::body::Body;
use axum::extract::{Path, Request, State};
use axum::http::request::Parts;
use axum::routing::{get, post, put};
use gate_core::{primitives, quota, validation, values};
use gate_upstream::{headers::token_id, pool::Token, token_store};
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

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
        .route(
            "/admin/api/request-settlements/{request_id}/release",
            post(release_request_settlement),
        )
}

/// 「上游」页的一次性读数：账号行 + 核对 + 未知预留 + 冷却 + 会话 CSRF。
/// 除 `refresh-info` 外，页面不需要再发第二个请求。每行另带运行读数（昨天出图、排队、
/// 最近限流、出图间隔、并发锁、闲时实测恢复、临时上限、站付 V5），供账号卡片直接显示。
async fn page(State(state): State<AppState>, _s: AdminSession, req: Request) -> ApiResult {
    let (parts, _) = req.into_parts();
    let now = state.now();
    let all = pool_status(&state).await?;
    let tokens: HashMap<String, Arc<Token>> = state
        .nai
        .status_rows()
        .await
        .into_iter()
        .map(|(token, _)| (token.token_id.clone(), token))
        .collect();
    let pool_size = all.len();
    let pool: Vec<Value> = all
        .into_iter()
        .filter(|p| {
            p["token_id"]
                .as_str()
                .and_then(|id| tokens.get(id))
                .is_none_or(|t| !t.is_share())
        })
        .collect();
    let queue_timeout = state.limits().queue_timeout;
    let yesterday = state.clock.day(now - 86400.0);
    let caps = state.guard.with(|g| g.token_daily_caps.clone());
    let raw_paid = state.db.get_setting(gate_core::paid_v5::KEY).await?;
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
            attach_runtime(
                &state,
                &mut row,
                RuntimeInput {
                    token: tokens.get(id),
                    id,
                    now,
                    queue_timeout,
                    yesterday: &yesterday,
                    caps: &caps,
                    raw_paid: raw_paid.as_deref(),
                },
            )
            .await?;
        }
        accounts.push(row);
    }

    let session = session_cookie(&parts);
    Ok(ok_json(&json!({
        "generated_at": values::float(now),
        "pool_configured": state.nai.configured().await,
        "pool_size": pool_size,
        "accounts": accounts,
        "threshold": allowance["threshold"],
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
        "fields": {
            "v5_daily_limit": {"kind": "int", "min": 0, "max": gate_core::member_policy::DAILY_MAX},
        },
    })))
}

struct RuntimeInput<'a> {
    token: Option<&'a Arc<Token>>,
    id: &'a str,
    now: f64,
    queue_timeout: f64,
    yesterday: &'a str,
    caps: &'a BTreeMap<String, (i64, f64)>,
    raw_paid: Option<&'a str>,
}

/// 账号卡片用的运行读数。电量百分比、Anlas 和今天张数已经在别的字段里，这里不重复。
async fn attach_runtime(
    state: &AppState,
    row: &mut Value,
    input: RuntimeInput<'_>,
) -> Result<(), ApiError> {
    let y = state
        .db
        .get_upstream_counter(input.id, input.yesterday)
        .await?;
    let quiet = quota::quiet_recovery(&state.db.quota_quiet_inputs_for(input.id, input.now).await?)
        .map_or(Value::Null, |q| q.to_value());
    let manual = input
        .caps
        .get(input.id)
        .filter(|(_, until)| input.now < *until)
        .map(|(cap, until)| json!({"cap": cap, "until": values::float(*until)}));
    let paid = if let Some(g) = gate_core::paid_v5::grant(input.raw_paid, input.id, input.now) {
        let spent = state
            .db
            .site_paid_spent(input.id, gate_core::paid_v5::MARKER)
            .await?;
        Some(json!({
            "max_anlas": values::float(g.max_anlas),
            "spent": values::float(spent),
            "until": values::float(g.until),
        }))
    } else {
        None
    };
    let pacing = input.token.map(|token| state.nai.pacing(token));
    row["yesterday"] = json!({"images": y.images, "v5": y.v5});
    super::merge_json(
        row,
        &super::pacing_fields(pacing.as_ref(), input.queue_timeout),
    );
    row["quiet_rate"] = quiet;
    row["manual_cap"] = manual.unwrap_or(Value::Null);
    row["paid_v5"] = paid.unwrap_or(Value::Null);
    Ok(())
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
    let exclusive = state.nai.exclusive_tokens().await;
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
            "position": f.kind_index,
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
            "exclusive": exclusive.contains(&t.token_id),
        }));
    }
    Ok(out)
}

pub(super) enum Edit {
    Add {
        token: String,
        allow_anlas: bool,
        image_host: Option<String>,
    },
    Replace {
        id: String,
        token: String,
        image_host: Option<String>,
    },
    Remove {
        id: String,
    },
}

pub(super) async fn edit_pool(state: &AppState, edit: Edit) -> Result<(), ApiError> {
    // Disconnecting the administrator must not cancel publication after the DB commit.
    let worker = state.clone();
    state
        .tasks
        .spawn(async move { commit_edit(&worker, edit).await })
        .await
        .map_err(|_| InternalError("上游凭据变更任务中断".into()))?
}

async fn commit_edit(state: &AppState, edit: Edit) -> Result<(), ApiError> {
    let _serial = state.token_edits.lock().await;
    let previous: Vec<_> = state
        .nai
        .tokens()
        .await
        .iter()
        .map(|t| token_store::Entry {
            token: t.token.clone(),
            allow_anlas: t.fields().allow_anlas,
            image_host: t.image_host.clone(),
        })
        .collect();
    let mut next = previous.clone();
    let position = |id: &str| previous.iter().position(|e| token_id(&e.token) == id);
    let mut replaced = None;
    let mut verified = Vec::new();
    let mut saved_v5 = None;
    let mut saved_enabled = None;
    match &edit {
        Edit::Add {
            token,
            allow_anlas,
            image_host,
        } => {
            let id = token_id(token);
            if position(&id).is_some() {
                return Err(HttpError::new(409, "这把 Token 已经在令牌池里").into());
            }
            saved_v5 = state
                .db
                .get_upstream_token_limits()
                .await?
                .into_iter()
                .find(|(k, _)| *k == id)
                .map(|(_, v)| v);
            saved_enabled = state
                .db
                .get_upstream_token_enabled()
                .await?
                .into_iter()
                .find(|(k, _)| *k == id)
                .map(|(_, v)| v);
            verified.push(id);
            next.push(token_store::Entry {
                token: token.clone(),
                allow_anlas: *allow_anlas,
                image_host: image_host.clone(),
            });
        }
        Edit::Replace {
            id,
            token,
            image_host,
        } => {
            let i = position(id).ok_or_else(|| HttpError::new(404, NOT_FOUND))?;
            let new_id = token_id(token);
            if position(&new_id).is_some_and(|other| other != i) {
                return Err(HttpError::new(409, "这把 Token 已经在令牌池的另一个位置").into());
            }
            verified.extend([id.clone(), new_id.clone()]);
            replaced = Some((id.clone(), new_id));
            next[i].token = token.clone();
            if let Some(host) = image_host {
                next[i].image_host = Some(host.clone());
            }
        }
        Edit::Remove { id } => {
            if previous.len() <= 1 {
                return Err(HttpError::new(409, "至少需要保留一把上游 Token").into());
            }
            next.remove(position(id).ok_or_else(|| HttpError::new(404, NOT_FOUND))?);
        }
    }
    let _quarantine = state.invalid_token_save.lock().await;
    let mut quarantine = None;
    if !verified.is_empty() {
        let raw = state
            .db
            .get_setting(crate::hooks::INVALID_TOKENS_KEY)
            .await?;
        let mut ids = crate::hooks::decode_invalid_tokens(raw.as_deref()).map_err(InternalError)?;
        let before = ids.len();
        for id in &verified {
            ids.remove(id);
        }
        if ids.len() != before {
            let mut sorted: Vec<_> = ids.into_iter().collect();
            sorted.sort();
            quarantine = Some(serde_json::to_string(&sorted).expect("string array"));
        }
    }
    let path = state.env.upstream_tokens_path();
    let revision = hex::encode(rand::random::<[u8; 16]>());
    let had_file = path
        .try_exists()
        .map_err(|e| InternalError(e.to_string()))?;
    let prior = if had_file { previous.as_slice() } else { &[] };
    token_store::stage(&path, prior, &next, &revision)
        .map_err(|e| InternalError(format!("上游 Token 文件写入失败：{e}")))?;
    if let Err(err) = state
        .db
        .commit_token_pool(revision, replaced, quarantine)
        .await
    {
        // An interrupted cleanup also recovers the old pool by revision on restart.
        let cleanup = if had_file {
            token_store::save(&path, prior)
        } else {
            std::fs::remove_file(&path)
        };
        if let Err(e) = cleanup {
            tracing::warn!("清理未提交的上游凭据失败：{e}");
        }
        return Err(err.into());
    }
    // Membership and per-account settings are serialized by token_edits. The
    // operations below have no I/O and their preconditions were checked above.
    match edit {
        Edit::Add {
            token,
            allow_anlas,
            image_host,
        } => {
            state
                .nai
                .add_token(token, allow_anlas, saved_v5, saved_enabled, image_host)
                .await
                .map_err(InternalError)?;
        }
        Edit::Replace {
            id,
            token,
            image_host,
        } => {
            state.db.unbind_patron_token(&id).await?;
            state
                .nai
                .replace_token(&id, token, image_host)
                .await
                .map_err(InternalError)?;
            state.refresh_exclusive_tokens().await;
        }
        Edit::Remove { id } => {
            state.db.unbind_patron_token(&id).await?;
            state.nai.remove_token(&id).await.map_err(InternalError)?;
            state.refresh_exclusive_tokens().await;
        }
    }
    // The committed candidate is already restart-safe. Remove old credentials
    // and restore the legacy on-disk format once publication is complete.
    if let Err(e) = token_store::save(&path, &next) {
        tracing::warn!("已提交上游 Token 变更；清理旧凭据失败：{e}");
    }
    Ok(())
}

async fn exists(state: &AppState, id: &str) -> bool {
    state.nai.tokens().await.iter().any(|t| t.token_id == id)
}

/// 上游 Token 被改动：记一条警告日志（告警渠道已下线）。
pub(super) fn alert_owner(_state: &AppState, text: &str) {
    tracing::warn!(target: "upstream", "上游 Token 被改动：{text}");
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
        .verify_token(token, replacing, None)
        .await
        .map_err(|msg| HttpError::new(422, msg).into())
}

async fn require_official(state: &AppState, id: &str) -> Result<(), ApiError> {
    let tokens = state.nai.tokens().await;
    let Some(token) = tokens.iter().find(|t| t.token_id == id) else {
        return Err(HttpError::new(404, NOT_FOUND).into());
    };
    if token.is_share() {
        return Err(HttpError::new(404, NOT_FOUND).into());
    }
    Ok(())
}

// ------------------------------------------------------------------ 增删改

/// 添加一把上游 Token：先向 NovelAI 验证，通过后加入令牌池。
async fn add_token(State(state): State<AppState>, _s: AdminSession, req: Request) -> ApiResult {
    let (parts, body) = req.into_parts();
    let (token, body) = token_payload(&parts, body).await?;
    let tier = verify(&state, &token, None).await?;
    let allow_anlas = body.get("allow_anlas").is_some_and(values::enabled);
    edit_pool(
        &state,
        Edit::Add {
            token,
            allow_anlas,
            image_host: None,
        },
    )
    .await?;
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
    require_official(&state, &old_id).await?;
    let tier = verify(&state, &token, Some(&old_id)).await?;
    edit_pool(
        &state,
        Edit::Replace {
            id: old_id,
            token,
            image_host: None,
        },
    )
    .await?;
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
    edit_pool(&state, Edit::Remove { id }).await?;
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
    let max = i64::from(gate_core::member_policy::DAILY_MAX);
    let limit = match body.get("v5_daily_limit").and_then(values::strict_int) {
        // 0 = 停用这把 Token 的免费 V5，上限与全站每日额度相同。
        Some(n) if (0..=max).contains(&n) => n,
        _ => {
            return Err(HttpError::new(
                422,
                format!("上游 V5 日限额必须是 0～{max} 的整数（0 = 停用免费 V5）"),
            )
            .into());
        }
    };
    let _serial = state.token_edits.lock().await;
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
    let _serial = state.token_edits.lock().await;
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
