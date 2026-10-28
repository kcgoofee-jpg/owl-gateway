//! 运行保护：共享预算、V5 暂停与降档、图片排队与冷却、上游账号保护。

use super::super::BODY_LIMIT;
use crate::auth::site_flag;
use crate::body::read_json_body;
use crate::error::{ApiError, ApiResult, HttpError, InternalError, ok_json};
use crate::guard::GuardIoError;
use crate::session::AdminSession;
use crate::state::{AppState, RUNTIME_LIMIT_BOUNDS};
use axum::Router;
use axum::extract::{Request, State};
use axum::routing::get;
use gate_core::site_flags::{V5_MEDIUM_MODE, V5_PAUSED};
use gate_core::validation::ValidationError;
use gate_core::{primitives, values};
use serde_json::{Value, json};

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/admin/api/settings", get(get_settings).put(put_settings))
        .route(
            "/admin/api/runtime-limits",
            get(get_runtime_limits).put(put_runtime_limits),
        )
        .route("/admin/api/guard", get(guard_get).put(guard_put))
}

// ------------------------------------------------------------------ 预算

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RuntimeSettings {
    budgets: gate_core::member_policy::SharedBudgets,
    v5_paused: bool,
    v5_medium_mode: String,
}

async fn runtime_settings(state: &AppState) -> Result<RuntimeSettings, ApiError> {
    Ok(RuntimeSettings {
        budgets: state.db.shared_budgets().await?,
        v5_paused: crate::ops::v5_paused(&state.db).await?,
        v5_medium_mode: site_flag(state, &V5_MEDIUM_MODE)
            .await?
            .as_str()
            .ok_or_else(|| InternalError("V5 降档设置无效".into()))?
            .to_owned(),
    })
}

async fn get_settings(State(state): State<AppState>, _session: AdminSession) -> ApiResult {
    Ok(ok_json(&json!(runtime_settings(&state).await?)))
}

async fn put_settings(
    State(state): State<AppState>,
    _session: AdminSession,
    req: Request,
) -> ApiResult {
    let (parts, body) = req.into_parts();
    let body = read_json_body(&parts.headers, body, BODY_LIMIT).await?;
    let settings: RuntimeSettings = serde_json::from_value(Value::Object(body))
        .map_err(|_| HttpError::new(422, "运行保护设置格式无效"))?;
    settings
        .budgets
        .validate()
        .map_err(|message| HttpError::new(422, message))?;
    if !["off", "on"].contains(&settings.v5_medium_mode.as_str()) {
        return Err(HttpError::new(422, "V5 降档设置只能是 off 或 on").into());
    }
    state
        .db
        .set_settings_bulk(&[
            (
                gate_db::repo::member_policy::BUDGETS_KEY.into(),
                serde_json::to_string(&settings.budgets)
                    .map_err(|e| InternalError(e.to_string()))?,
            ),
            (
                V5_PAUSED.key.into(),
                if settings.v5_paused { "1" } else { "0" }.into(),
            ),
            (V5_MEDIUM_MODE.key.into(), settings.v5_medium_mode),
        ])
        .await?;
    get_settings(State(state), _session).await
}

// ------------------------------------------------------------------ 排队与冷却

/// 当前生效的运行期限制（取整；每 Key 出图间隔、账号出图间隔已删除，见 [`RUNTIME_LIMIT_BOUNDS`]）。
fn runtime_limits_snapshot(state: &AppState) -> Value {
    let l = state.limits();
    json!({
        "queue_timeout": l.queue_timeout as i64,
        "image_429_cooldown_seconds": l.image_429_cooldown_seconds as i64,
    })
}

async fn get_runtime_limits(State(state): State<AppState>, _s: AdminSession) -> ApiResult {
    Ok(ok_json(&runtime_limits_snapshot(&state)))
}

async fn put_runtime_limits(
    State(state): State<AppState>,
    _s: AdminSession,
    req: Request,
) -> ApiResult {
    let (parts, body) = req.into_parts();
    let body = read_json_body(&parts.headers, body, BODY_LIMIT).await?;
    if body.is_empty()
        || body
            .keys()
            .any(|k| !RUNTIME_LIMIT_BOUNDS.iter().any(|(n, _, _)| n == k))
    {
        return Err(HttpError::new(422, "包含未知或空的运行限制设置").into());
    }
    let mut values: Vec<(&'static str, i64)> = Vec::new();
    for (name, value) in &body {
        let &(name, lo, hi) = RUNTIME_LIMIT_BOUNDS
            .iter()
            .find(|(n, _, _)| n == name)
            .expect("上面已校验过名字");
        match values::strict_int(value) {
            Some(n) if (lo..=hi).contains(&n) => values.push((name, n)),
            _ => {
                return Err(HttpError::new(422, format!("{name} 必须是 {lo}～{hi} 的整数")).into());
            }
        }
    }
    // 写库 + 生效必须连在一起（`ops.runtime_limits` 锁）
    let shared = &state.ops;
    let _lock = shared.runtime_limits.lock().await;
    let writes: Vec<(String, String)> = values
        .iter()
        .map(|(k, v)| (format!("runtime_{k}"), v.to_string()))
        .collect();
    state.db.set_settings_bulk(&writes).await?;
    state.apply_runtime_limits(&values).await;
    Ok(ok_json(&runtime_limits_snapshot(&state)))
}

// ------------------------------------------------------------------ 账号保护

/// 账号保护与排队的当前设置和实时用量。
async fn guard_view(state: &AppState) -> Result<Value, ApiError> {
    let now = state.now();
    let mut data = state.guard.with(|g| g.describe());
    let day = state.day();
    let mut accounts = Vec::new();
    let queue_timeout = state.limits().queue_timeout;
    for (t, f) in state.nai.status_rows().await {
        let c = state.db.get_upstream_counter(&t.token_id, &day).await?;
        // 账号的出图节奏：周期 = 出图耗时 + 出完后歇的秒数（手动）；429 只让账号冷却
        let p = state.nai.pacing(&t);
        let r1 = |x: f64| primitives::round(x, 1);
        // 今天出满了：账号本身可用，但不会再接图（不能显示成「可用 / 0 秒后能开始」）
        let full = state
            .guard
            .with(|g| g.at_daily_cap(&t.token_id, c.images, now));
        accounts.push(json!({
            "position": f.position,
            "usable": t.usable(now),
            "full": full,
            "today": c.images,
            "cycle": r1(p.cycle),
            "gen_secs": r1(p.gen_secs),
            "last_429": if p.last_429 > 0.0 { values::float(p.last_429) } else { Value::Null },
            "cooling": if f.admin_enabled && !f.disabled { ((f.blocked_until - now) as i64).max(0) } else { 0 },
            "waiting": p.waiting,
            "next_start_in": if full { Value::Null } else { json!(r1(p.next_start_in)) },
            // 上游并发锁（上一张还没释放）今天的次数：网关自动在同一账号上重试，不算 429
            "locked_today": p.locked_today,
            "queue_cap": gate_upstream::pace::realistic_queue(queue_timeout, p.cycle),
        }));
    }
    let queued: i64 = state.guard.with(|g| g.image_inflight.values().sum());
    let queue_cap = crate::admission::queue_cap(state).await;
    if let Value::Object(m) = &mut data {
        m.insert("accounts".into(), Value::Array(accounts));
        m.insert("queued_images".into(), Value::from(queued));
        m.insert("queue_cap".into(), json!(queue_cap));
        m.insert("queue_timeout".into(), json!(queue_timeout as i64));
    }
    Ok(data)
}

async fn guard_get(State(state): State<AppState>, _s: AdminSession) -> ApiResult {
    Ok(ok_json(&guard_view(&state).await?))
}

async fn guard_put(State(state): State<AppState>, _s: AdminSession, req: Request) -> ApiResult {
    let (parts, body) = req.into_parts();
    let body = read_json_body(&parts.headers, body, BODY_LIMIT).await?;
    match state.guard.save(&body).await {
        Ok(_) => {}
        // 值不合法 → 422 原文；其余（库写失败等）是 500
        Err(GuardIoError::Guard(ValidationError::InvalidValue(msg))) => {
            return Err(HttpError::new(422, msg).into());
        }
        Err(e) => return Err(InternalError(e.to_string()).into()),
    }
    Ok(ok_json(&guard_view(&state).await?))
}
