//! Discord 机器人（奶妹）在线状态（总览的「奶妹离线」提醒用）与维护模式。

use crate::body::read_json_body;
use crate::error::{ApiResult, HttpError, InternalError, ok_json};
use crate::session::AdminSession;
use crate::state::AppState;
use axum::Router;
use axum::extract::{Request, State};
use axum::routing::{get, put};
use gate_core::bot_config::{self, BotConfigError};
use serde_json::json;

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/admin/api/discord-bot", get(discord_bot_get))
        .route("/admin/api/ops/maintenance", put(maintenance_put))
}

async fn maintenance_put(
    State(state): State<AppState>,
    _s: AdminSession,
    req: Request,
) -> ApiResult {
    let (parts, body) = req.into_parts();
    let body = read_json_body(&parts.headers, body, super::super::BODY_LIMIT).await?;
    use crate::ops::Maintenance;
    // 只接受 {mode} 或旧的 {enabled}，多带字段一律 422（后台表单不会这么发，挡住手滑/试探）
    let exact = body.len() == 1 && body.contains_key("mode");
    if !exact {
        return Err(HttpError::new(422, "只接受 {mode} 单字段").into());
    }
    let mode = match (
        body.get("mode").and_then(serde_json::Value::as_str),
        body.get("enabled").and_then(serde_json::Value::as_bool),
    ) {
        (Some("off"), None) => Maintenance::Off,
        (Some("all"), None) => Maintenance::All,
        (Some("owner"), None) => Maintenance::OwnerOnly,
        _ => {
            return Err(HttpError::new(422, "mode 必须是 off / all / owner").into());
        }
    };
    if mode == Maintenance::OwnerOnly && state.env.maintenance_owner_ids.is_empty() {
        return Err(HttpError::new(
            422,
            "先设置 MAINTENANCE_OWNER_IDS，否则「只有站长可用」等于全站关闭",
        )
        .into());
    }
    crate::ops::set_maintenance_mode(&state, mode).await?;
    Ok(ok_json(
        &json!({"enabled": mode != Maintenance::Off, "mode": mode.as_str()}),
    ))
}

// ------------------------------------------------------------------ Discord 机器人

fn bot_internal(e: BotConfigError) -> InternalError {
    InternalError(e.to_string())
}

/// 机器人在线状态（心跳）。跑图分享设置和最近动作已删除（机器人从未实现）。
async fn discord_bot_get(State(state): State<AppState>, _s: AdminSession) -> ApiResult {
    let status = state.db.get_setting(bot_config::STATUS_KEY).await?;
    let snapshot = bot_config::snapshot(status.as_deref(), state.now()).map_err(bot_internal)?;
    Ok(ok_json(&snapshot))
}
