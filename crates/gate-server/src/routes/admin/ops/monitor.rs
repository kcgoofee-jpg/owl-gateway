//! 监控类接口：上游令牌池、告警 / 生成记录状态、V5 官方额度。
//! 「系统」页的服务器状态卡片（磁盘、上游健康、暴力破解防护）10-23 删除，`/admin/api/status` 只剩告警渠道和生成记录两块。

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

/// 上游令牌池（掩码后的 Token）。总览页 10-23 删除（站长：日志留全，要看什么让 Claude 查），这个接口只剩「上游」页用的两项。
async fn overview(State(state): State<AppState>, _s: AdminSession) -> ApiResult {
    Ok(ok_json(&json!({
        "pool": upstream::pool_status(&state).await?,
        "pool_configured": state.nai.configured().await,
    })))
}

/// 告警与记录功能的当前状态（不含任何密钥）。
async fn server_status(State(state): State<AppState>, _s: AdminSession) -> ApiResult {
    Ok(ok_json(&json!({
        "alerts": {
            "configured": state.alerter.configured(),
            "sent": state.alerter.sent.load(std::sync::atomic::Ordering::Relaxed),
        },
    })))
}
