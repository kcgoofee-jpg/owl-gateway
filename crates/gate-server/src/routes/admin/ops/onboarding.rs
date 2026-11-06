//! 领取条件（成员规则另有一套接口）。

use super::super::BODY_LIMIT;
use super::upstream_health;
use crate::body::read_json_body;
use crate::error::{ApiError, ApiResult, ok_json};
use crate::ops;
use crate::session::AdminSession;
use crate::state::AppState;
use axum::Router;
use axum::extract::{Request, State};
use axum::routing::{get, put};
use serde_json::{Value, json};

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/admin/api/ops", get(ops_get))
        .route("/admin/api/ops/registration", put(set_registration))
}

/// 运行中开关的当前状态（`GET /ops` 与 `PUT /ops/registration` 共用）。
async fn snapshot(state: &AppState) -> Result<Value, ApiError> {
    let reg = ops::registration_settings(state).await?;
    let mut registration = reg.to_map();
    registration.insert(
        "active".into(),
        json!(state.db.registered_member_count(state.now()).await?),
    );
    Ok(json!({
        "registration": registration,
        "maintenance": {"enabled": ops::maintenance_mode(state).await? != ops::Maintenance::Off, "mode": ops::maintenance_mode(state).await?.as_str()},
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
