//! 测试告警、Discord 机器人（奶妹）在线状态（总览的「奶妹离线」提醒用）。

use crate::body::read_json_body;
use crate::error::{ApiResult, HttpError, InternalError, ok_json};
use crate::session::AdminSession;
use crate::state::AppState;
use axum::Router;
use axum::extract::{Request, State};
use axum::routing::{get, post, put};
use gate_core::bot_config::{self, BotConfigError};
use serde_json::json;

/// 两次测试告警至少间隔这么多秒。
const ALERT_TEST_GAP: f64 = 10.0;

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/admin/api/alerts/test", post(alerts_test))
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
    let enabled = match body.get("enabled").and_then(serde_json::Value::as_bool) {
        Some(enabled) if body.len() == 1 => enabled,
        _ => return Err(HttpError::new(422, "enabled 必须是布尔值").into()),
    };
    crate::ops::set_maintenance_mode(&state, enabled).await?;
    Ok(ok_json(&json!({"enabled": enabled})))
}

// ------------------------------------------------------------------ 测试告警

async fn alerts_test(State(state): State<AppState>, _s: AdminSession) -> ApiResult {
    let shared = &state.ops;
    let now = state.now();
    let last = *shared.alert_test_at.lock().expect("alert_test_at");
    if now - last < ALERT_TEST_GAP {
        return Err(HttpError::new(
            429,
            format!(
                "测试告警发送过于频繁，请等待 {} 秒后再试",
                (ALERT_TEST_GAP - (now - last)) as i64
            ),
        )
        .into());
    }
    if !state.alerter.configured() {
        return Err(HttpError::new(
            409,
            "尚未配置告警私信（DISCORD_BOT_TOKEN 与 ALERT_USER_ID）",
        )
        .into());
    }
    *shared.alert_test_at.lock().expect("alert_test_at") = now;
    state
        .alerter
        .notify("test", "这是一条测试告警，收到说明告警渠道正常。", 0.0);
    Ok(ok_json(&json!({"ok": true})))
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
