//! 领取条件、全局功能开关与请求来源保留设置。

use super::super::BODY_LIMIT;
use super::upstream_health;
use crate::body::read_json_body;
use crate::error::{ApiError, ApiResult, HttpError, ok_json};
use crate::ops;
use crate::session::AdminSession;
use crate::state::AppState;
use axum::Router;
use axum::extract::{Request, State};
use axum::routing::{get, put};
use gate_core::features::FEATURES;
use serde_json::{Map, Value, json};

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/admin/api/ops", get(ops_get))
        .route("/admin/api/ops/registration", put(set_registration))
        .route("/admin/api/ops/features", put(set_features))
        .route("/admin/api/ops/log-retention", put(set_log_retention))
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
        },
        "log_retention": {"origin_keep_days": crate::settings::usage_origin_keep_days(&state.db).await?},
        "maintenance": {"enabled": ops::maintenance_mode(state).await?},
        "upstream": upstream_health(state),
    }))
}

async fn ops_get(State(state): State<AppState>, _s: AdminSession) -> ApiResult {
    Ok(ok_json(&snapshot(&state).await?))
}

/// 保存统一领取条件；机器人下一条命令即生效。
async fn set_registration(
    State(state): State<AppState>,
    _s: AdminSession,
    req: Request,
) -> ApiResult {
    let (parts, body) = req.into_parts();
    let body = read_json_body(&parts.headers, body, BODY_LIMIT).await?;
    ops::set_registration(&state, &body).await?;
    Ok(ok_json(&snapshot(&state).await?))
}

/// 全局功能开关：`{"flags": {"upscale": false, ...}}`。关闭后所有成员立即不可用。
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

/// 设置请求来源信息的保留天数。
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
