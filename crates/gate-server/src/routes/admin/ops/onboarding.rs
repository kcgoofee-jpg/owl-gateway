//! 自助领 Key、全局功能开关、生成记录开关。

use super::super::BODY_LIMIT;
use super::upstream_health;
use crate::body::read_json_body;
use crate::defaults;
use crate::error::{ApiError, ApiResult, HttpError, ok_json};
use crate::ops;
use crate::session::AdminSession;
use crate::state::AppState;
use axum::Router;
use axum::extract::{Request, State};
use axum::routing::{get, put};
use gate_core::features::{self, FEATURES};
use serde_json::{Map, Value, json};

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/admin/api/ops", get(ops_get))
        .route("/admin/api/ops/registration", put(set_registration))
        .route("/admin/api/ops/features", put(set_features))
        .route("/admin/api/ops/log-retention", put(set_log_retention))
}

/// 后台「新建 Key」弹窗的默认功能：沿用「新成员默认开通的功能」（默认只有文生图），而不是全部功能。
async fn new_key_features(state: &AppState) -> Result<Value, ApiError> {
    let Some(dumped) = ops::default_new_key_features(state).await? else {
        return Ok(Value::Null); // 站长明确把默认设为「全部已开放功能」
    };
    Ok(match features::parse_list(&Value::String(dumped)) {
        Some(list) => json!(list),
        None => Value::Null,
    })
}

/// 运行中开关的当前状态（`GET /ops` 与三个 `PUT /ops/*` 共用）。
async fn snapshot(state: &AppState) -> Result<Value, ApiError> {
    let reg = ops::registration_settings(state).await?;
    let mut registration = reg.to_map();
    registration.insert(
        "active".into(),
        json!(state.db.registered_member_count(state.now()).await?),
    );
    let labels: Map<String, Value> = FEATURES
        .iter()
        .map(|(n, l)| ((*n).to_string(), json!(l)))
        .collect();
    Ok(json!({
        "registration": registration,
        "features": {
            "global": ops::global_flags(state).await?,
            "labels": labels,
            // 后台「新建 Key」弹窗的默认功能，与服务端缺省逻辑一致
            "new_key": new_key_features(state).await?,
        },
        // 后台「新建 Key」弹窗里没有数据库设置的默认值（每日 V4.5 / V5 张数、有效天数在上面的 registration 里）：
        // 前端不另写一份
        "new_key": {
            "rpm": defaults::MANUAL_KEY_RPM,
            "daily_anlas": defaults::MANUAL_KEY_DAILY_ANLAS,
            "monthly_anlas": defaults::MANUAL_KEY_MONTHLY_ANLAS,
        },
        "log_retention": {"origin_keep_days": crate::settings::usage_origin_keep_days(&state.db).await?},
        "upstream": upstream_health(state),
    }))
}

async fn ops_get(State(state): State<AppState>, _s: AdminSession) -> ApiResult {
    Ok(ok_json(&snapshot(&state).await?))
}

/// 开放 / 关闭自助注册、名额上限、新成员默认功能与额度；机器人下一条命令即生效。
async fn set_registration(
    State(state): State<AppState>,
    _s: AdminSession,
    req: Request,
) -> ApiResult {
    let (parts, body) = req.into_parts();
    let body = read_json_body(&parts.headers, body, BODY_LIMIT).await?;
    match body.get("features") {
        None | Some(Value::Null) => {}
        Some(Value::Array(list))
            if list.iter().all(|v| {
                v.as_str()
                    .is_some_and(|n| FEATURES.iter().any(|(f, _)| *f == n))
            }) => {}
        Some(_) => return Err(HttpError::new(422, "features 必须是功能名列表").into()),
    }
    ops::set_registration(&state, &body).await?;
    Ok(ok_json(&snapshot(&state).await?))
}

/// 全局功能开关：`{"flags": {"upscale": false, ...}}`。关闭后所有成员（管理员 Key 除外）立即不可用。
async fn set_features(State(state): State<AppState>, _s: AdminSession, req: Request) -> ApiResult {
    let (parts, body) = req.into_parts();
    let body = read_json_body(&parts.headers, body, BODY_LIMIT).await?;
    let flags = match body.get("flags") {
        Some(Value::Object(m))
            if !m.is_empty()
                && m.iter()
                    .all(|(k, v)| FEATURES.iter().any(|(f, _)| f == k) && v.is_boolean()) =>
        {
            m
        }
        _ => return Err(HttpError::new(422, "flags 必须是 {功能名: true/false}").into()),
    };
    ops::set_global_features(&state, flags).await?;
    Ok(ok_json(&snapshot(&state).await?))
}

/// 开关生成记录与保留天数。
async fn set_log_retention(
    State(state): State<AppState>,
    _s: AdminSession,
    req: Request,
) -> ApiResult {
    let (parts, body) = req.into_parts();
    let body = read_json_body(&parts.headers, body, BODY_LIMIT).await?;
    ops::set_log_retention(&state, &body).await?;
    Ok(ok_json(&snapshot(&state).await?))
}
