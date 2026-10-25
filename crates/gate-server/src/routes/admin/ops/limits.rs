//! 额度与保护类设置（全部手动）：每人每日 V4.5 / V5、全站 V5、V5 暂停、Anlas、图片排队与冷却、账号保护与排队。

use super::super::BODY_LIMIT;
use super::super::num::{Kind, strict_num};
use crate::auth::{site_flag, site_flag_f64};
use crate::body::read_json_body;
use crate::error::{ApiError, ApiResult, HttpError, InternalError, ok_json};
use crate::guard::GuardIoError;
use crate::session::AdminSession;
use crate::state::{AppState, RUNTIME_LIMIT_BOUNDS};
use axum::Router;
use axum::extract::{Request, State};
use axum::routing::get;
use gate_core::pyext::PyErr;
use gate_core::site_flags::{
    self, GLOBAL_DAILY_ANLAS, GLOBAL_DAILY_V5, GLOBAL_MONTHLY_ANLAS, MEMBER_DAILY_ANLAS,
    REGISTER_DAILY_IMAGES, REGISTER_DAILY_V5, V5_MEDIUM_MODE, V5_PAUSED,
};
use gate_core::{pyc, pyval};
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

/// V5 Medium 降档此刻是否生效（手动开关）。
async fn medium_view(state: &AppState) -> Value {
    json!({"active": crate::v5_medium::active(state).await})
}

async fn get_settings(State(state): State<AppState>, _s: AdminSession) -> ApiResult {
    let policy = crate::v5_access::configuration(&state).await?;
    let policy_mode = policy.mode.as_str();
    Ok(ok_json(&json!({
        "global_monthly_anlas": pyval::float(site_flag_f64(&state, &GLOBAL_MONTHLY_ANLAS).await?),
        "global_daily_v5": crate::ops::global_daily_v5(&state.db).await?,
        "global_daily_anlas": site_flag(&state, &GLOBAL_DAILY_ANLAS).await?,
        "member_daily_anlas": site_flag(&state, &MEMBER_DAILY_ANLAS).await?,
        "register_daily_images": site_flag(&state, &REGISTER_DAILY_IMAGES).await?,
        "register_daily_v5": site_flag(&state, &REGISTER_DAILY_V5).await?,
        "v5_paused": policy.paused,
        "v5_access_mode": policy_mode,
        "v5_authorized_discord_ids": policy.authorized,
        "gateway_owner_discord_ids": policy.owners,
        "gateway_admin_discord_ids": policy.admins,
        "v5_medium_mode": site_flag(&state, &V5_MEDIUM_MODE).await?,
        "v5_medium_state": medium_view(&state).await,
    })))
}

async fn put_settings(State(state): State<AppState>, _s: AdminSession, req: Request) -> ApiResult {
    let (parts, body) = req.into_parts();
    let body = read_json_body(&parts.headers, body, BODY_LIMIT).await?;
    let policy_keys = [
        crate::v5_access::MODE_KEY,
        crate::v5_access::SUBJECTS_KEY,
        crate::v5_access::OWNERS_KEY,
        crate::v5_access::ADMINS_KEY,
    ];
    let has_policy = policy_keys.iter().any(|key| body.contains_key(*key));
    if has_policy && policy_keys.iter().any(|key| !body.contains_key(*key)) {
        return Err(HttpError::new(422, "V5 策略需同时提交模式、授权主体与身份角色列表").into());
    }
    let policy_mode = match body.get(crate::v5_access::MODE_KEY) {
        None => None,
        Some(Value::String(s))
            if ["legacy", "closed", "restricted", "everyone"].contains(&s.as_str()) =>
        {
            Some(s.clone())
        }
        Some(_) => {
            return Err(HttpError::new(
                422,
                "v5_access_mode 必须是 legacy / closed / restricted / everyone",
            )
            .into());
        }
    };
    fn subjects(v: Option<&Value>, name: &str) -> Result<Option<String>, ApiError> {
        let Some(Value::Array(values)) = v else {
            if v.is_none() {
                return Ok(None);
            }
            return Err(HttpError::new(422, format!("{name} 必须是 Discord ID 数组")).into());
        };
        let mut ids = Vec::new();
        for value in values {
            let Some(id) = value.as_str() else {
                return Err(HttpError::new(422, format!("{name} 中的 ID 必须是字符串")).into());
            };
            if id.is_empty() || !id.bytes().all(|b| b.is_ascii_digit()) {
                return Err(HttpError::new(422, format!("{name} 中包含无效 Discord ID")).into());
            }
            if !ids.iter().any(|seen| seen == id) {
                ids.push(id.to_string());
            }
        }
        Ok(Some(
            serde_json::to_string(&ids).map_err(|e| InternalError(e.to_string()))?,
        ))
    }
    let subject_settings = [
        (
            crate::v5_access::SUBJECTS_KEY,
            subjects(
                body.get(crate::v5_access::SUBJECTS_KEY),
                crate::v5_access::SUBJECTS_KEY,
            )?,
        ),
        (
            crate::v5_access::OWNERS_KEY,
            subjects(
                body.get(crate::v5_access::OWNERS_KEY),
                crate::v5_access::OWNERS_KEY,
            )?,
        ),
        (
            crate::v5_access::ADMINS_KEY,
            subjects(
                body.get(crate::v5_access::ADMINS_KEY),
                crate::v5_access::ADMINS_KEY,
            )?,
        ),
    ];
    let policy_values = if let Some(mode) = &policy_mode {
        Some(vec![
            (crate::v5_access::MODE_KEY.to_string(), mode.clone()),
            (
                crate::v5_access::SUBJECTS_KEY.to_string(),
                subject_settings[0].1.clone().unwrap(),
            ),
            (
                crate::v5_access::OWNERS_KEY.to_string(),
                subject_settings[1].1.clone().unwrap(),
            ),
            (
                crate::v5_access::ADMINS_KEY.to_string(),
                subject_settings[2].1.clone().unwrap(),
            ),
        ])
    } else {
        None
    };
    // 降档模式：只认 off / on 两个小写字符串，其他（含已删除的 auto）一律 422；缺省保持原值
    let medium_mode = match body.get(V5_MEDIUM_MODE.key) {
        Some(v) => v.clone(),
        None => site_flag(&state, &V5_MEDIUM_MODE).await?,
    };
    let medium_text =
        site_flags::encode(&V5_MEDIUM_MODE, &medium_mode).map_err(|e| HttpError::new(422, e.0))?;
    // 缺省字段保持原值（以前缺省会被写成 0 = 不限）
    async fn field(
        state: &AppState,
        body: &serde_json::Map<String, Value>,
        spec: &site_flags::Spec,
        kind: Kind,
        hi: f64,
    ) -> Result<Value, ApiError> {
        let value = match body.get(spec.key) {
            Some(v) => v.clone(),
            None => site_flag(state, spec).await?,
        };
        Ok(strict_num(&value, kind, 0.0, hi, spec.key)?)
    }
    let monthly = field(
        &state,
        &body,
        &GLOBAL_MONTHLY_ANLAS,
        Kind::Float,
        1_000_000.0,
    )
    .await?;
    let daily_v5 = field(&state, &body, &GLOBAL_DAILY_V5, Kind::Int, 100_000.0).await?;
    let daily_anlas = field(&state, &body, &GLOBAL_DAILY_ANLAS, Kind::Int, 100_000.0).await?;
    let member = field(&state, &body, &MEMBER_DAILY_ANLAS, Kind::Int, 100_000.0).await?;
    // 每人每日 V4.5 / V5（0 = 不限）、V5 暂停（开关）
    let v45 = field(&state, &body, &REGISTER_DAILY_IMAGES, Kind::Int, 100_000.0).await?;
    let v5_each = field(&state, &body, &REGISTER_DAILY_V5, Kind::Int, 100_000.0).await?;
    let paused = match body.get(V5_PAUSED.key) {
        None => crate::v5_access::evaluate(&state, None).await?.paused,
        Some(Value::Bool(b)) => *b,
        Some(_) => return Err(HttpError::new(422, "v5_paused 必须是 true / false").into()),
    };
    // 这几项改了要立刻同步给成员 Key 和首页文案（不等下一轮定时任务）
    let sync = [
        GLOBAL_DAILY_V5.key,
        MEMBER_DAILY_ANLAS.key,
        REGISTER_DAILY_IMAGES.key,
        REGISTER_DAILY_V5.key,
        V5_PAUSED.key,
    ]
    .iter()
    .any(|k| body.contains_key(*k));
    let mut v5_settings = vec![(
        V5_PAUSED.key.to_string(),
        if paused { "1" } else { "0" }.to_string(),
    )];
    if let Some(policy) = policy_values {
        v5_settings.extend(policy);
    }
    state.db.set_settings_bulk(&v5_settings).await?;
    for (spec, v) in [
        (&GLOBAL_MONTHLY_ANLAS, &monthly),
        (&GLOBAL_DAILY_V5, &daily_v5),
        (&GLOBAL_DAILY_ANLAS, &daily_anlas),
        (&MEMBER_DAILY_ANLAS, &member),
        (&REGISTER_DAILY_IMAGES, &v45),
        (&REGISTER_DAILY_V5, &v5_each),
    ] {
        let text = site_flags::encode(spec, v).map_err(|e| InternalError(e.0))?;
        state.db.set_setting(spec.key, &text).await?;
    }
    state
        .db
        .set_setting(V5_MEDIUM_MODE.key, &medium_text)
        .await?;
    let policy_mode_out = policy_mode.unwrap_or(
        crate::v5_access::configuration(&state)
            .await?
            .mode
            .as_str()
            .to_string(),
    );
    let out = json!({
        "ok": true,
        "global_monthly_anlas": monthly,
        "global_daily_v5": daily_v5,
        "global_daily_anlas": daily_anlas,
        "member_daily_anlas": member,
        "register_daily_images": v45,
        "register_daily_v5": v5_each,
        "v5_paused": paused,
        "v5_access_mode": policy_mode_out,
        "v5_medium_mode": medium_text,
    });
    if sync {
        crate::jobs::quota::run_now(&state).await?;
    }
    Ok(ok_json(&out))
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
        match pyval::strict_int(value) {
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
        let r1 = |x: f64| pyc::round(x, 1);
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
            "last_429": if p.last_429 > 0.0 { pyval::float(p.last_429) } else { Value::Null },
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
        Err(GuardIoError::Guard(PyErr::Value(msg))) => {
            return Err(HttpError::new(422, msg).into());
        }
        Err(e) => return Err(InternalError(e.to_string()).into()),
    }
    Ok(ok_json(&guard_view(&state).await?))
}
