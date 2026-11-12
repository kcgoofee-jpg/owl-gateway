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
        .route(
            "/admin/api/ops/share-lock",
            get(share_lock_get).put(share_lock_put),
        )
}

async fn share_lock_get(State(state): State<AppState>, _s: AdminSession) -> ApiResult {
    Ok(ok_json(&crate::share_lock::detection(&state).await?))
}

async fn share_lock_put(
    State(state): State<AppState>,
    _s: AdminSession,
    req: Request,
) -> ApiResult {
    let (parts, body) = req.into_parts();
    let body = read_json_body(&parts.headers, body, BODY_LIMIT).await?;
    let enabled = body
        .get("enabled")
        .and_then(Value::as_bool)
        .filter(|_| body.len() == 1)
        .ok_or_else(|| crate::error::HttpError::new(422, "只接受 {enabled: true/false}"))?;
    state
        .db
        .set_setting_logged(
            crate::share_lock::ENABLED_KEY,
            Some(if enabled { "1" } else { "0" }),
            &super::super::actor(&state, &parts),
            "分享限制",
            "成员规则",
            if enabled { "开启" } else { "关闭" },
            state.now(),
        )
        .await?;
    Ok(super::super::mark_logged(ok_json(
        &crate::share_lock::detection(&state).await?,
    )))
}

/// 运行中开关的当前状态（`GET /ops` 与 `PUT /ops/registration` 共用）。
async fn snapshot(state: &AppState) -> Result<Value, ApiError> {
    let reg = ops::registration_settings(state).await?;
    let mut registration = reg.to_map();
    registration.insert(
        "active".into(),
        json!(state.db.registered_member_count(state.now()).await?),
    );
    registration.insert("fields".into(), gate_core::member_policy::claim_fields());
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
