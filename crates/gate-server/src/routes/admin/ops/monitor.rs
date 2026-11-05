//! 后台上游池摘要与告警通道状态。

use super::upstream;
use crate::error::{ApiResult, ok_json};
use crate::session::AdminSession;
use crate::state::AppState;
use axum::Router;
use axum::extract::State;
use axum::routing::get;
use serde_json::json;

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/admin/api/overview", get(overview))
        .route("/admin/api/status", get(server_status))
}

/// 上游令牌池（掩码后的 Token）。总览页 10-23 删除（站长：日志留全，要看什么让 Claude 查），保留原接口的两个字段。
async fn overview(State(state): State<AppState>, _s: AdminSession) -> ApiResult {
    Ok(ok_json(&json!({
        "pool": upstream::pool_status(&state).await?,
        "pool_configured": state.nai.configured().await,
    })))
}

/// 告警通道的当前状态（不含任何密钥）。
async fn server_status(State(state): State<AppState>, _s: AdminSession) -> ApiResult {
    Ok(ok_json(&json!({
        "alerts": {
            "configured": state.alerter.configured(),
            "sent": state.alerter.sent.load(std::sync::atomic::Ordering::Relaxed),
        },
    })))
}
