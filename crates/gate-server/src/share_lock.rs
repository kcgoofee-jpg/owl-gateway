//! 分享闸：昨日用满后需分享，或所有非豁免成员每天先分享。
//!
//! 不把 Key 停用：停用会摘掉身份组，人就进不了那个频道，发不了图，也解不开。
//! 数据库暂时读失败时放行，避免一次存储抖动把所有出图都拦住。

use crate::error::{ApiError, GateError};
use crate::key::Key;
use crate::state::AppState;
use gate_core::clock::Clock;
use gate_core::reasons;
use gate_core::share_detection::Report;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub const ENABLED_KEY: &str = "share_lock_enabled";
pub const MODE_KEY: &str = "share_lock_mode";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    #[default]
    Quota,
    Daily,
}
impl Mode {
    pub fn from_setting(raw: Option<&str>) -> Self {
        if raw == Some("daily") {
            Self::Daily
        } else {
            Self::Quota
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Quota => "quota",
            Self::Daily => "daily",
        }
    }
}

pub async fn detection(state: &AppState) -> Result<Value, crate::error::InternalError> {
    let enabled = state.db.get_setting(ENABLED_KEY).await?.as_deref() != Some("0");
    let mode = Mode::from_setting(state.db.get_setting(MODE_KEY).await?.as_deref());
    let raw = state
        .db
        .get_setting(gate_core::bot_config::STATUS_KEY)
        .await?;
    let status: Value = raw
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or(Value::Null);
    let report = status
        .get("share_detection")
        .cloned()
        .and_then(|v| serde_json::from_value::<Report>(v).ok())
        .unwrap_or_default();
    let now = state.now();
    let day_start = state.clock.day_start(now);
    let seen_at = status["seen_at"].as_f64().unwrap_or(0.0);
    let enforcing = enabled && report.enforceable(seen_at, now, day_start);
    Ok(json!({
        "enabled": enabled,
        "mode": mode,
        "enforcing": enforcing,
        "healthy": report.healthy(seen_at, now, day_start),
        "bot_online": now - seen_at < gate_core::bot_config::ONLINE_WINDOW,
        "channel_url": channel_url(),
        "detection": report,
    }))
}

/// 存图备份专用（论坛频道）。新帖和帖子里的新内容都算。
pub const FORUM_CHANNEL_ID: &str = "1558068804934705222";
/// 规则开始生效的服务日。当天 7:00（北京时间）起，看上一个重置日。
const START_DAY: &str = "2026-11-13";

pub const MESSAGE: &str = "昨天的 V4.5 或 V5 额度已经用完。请到 Discord「存图备份专用」发一张图（新开一帖，或在已有帖子里再补一张），发成功后会自动恢复出图。";

pub const DAILY_MESSAGE: &str = "今天还没有分享图片。请到 Discord「存图备份专用」发一张图（新开一帖，或在已有帖子里再补一张），确认后自动恢复生图。每天北京时间 07:00 重置。";

fn message(mode: Mode) -> &'static str {
    match mode {
        Mode::Quota => MESSAGE,
        Mode::Daily => DAILY_MESSAGE,
    }
}

pub fn channel_url() -> String {
    format!("https://discord.com/channels/1549726297876594720/{FORUM_CHANNEL_ID}")
}

/// 给客户端看的整句（带频道链接）。首页另有按钮，正文不重复贴链接。
pub fn client_message() -> String {
    format!("{MESSAGE} {}", channel_url())
}

pub fn starts_at() -> f64 {
    Clock::service()
        .service_day_start(START_DAY)
        .expect("生效日必须能换算")
}

/// 上一个重置日的用量，和这把 Key 现在的个人上限。上限是 0 表示这项停用，不算用满。
#[derive(Clone, Copy)]
pub struct DayUsage {
    pub legacy: i64,
    pub v5: i64,
    pub daily_images: i64,
    pub daily_v5: i64,
}

pub fn quota_filled(u: DayUsage) -> bool {
    (u.daily_images > 0 && u.legacy >= u.daily_images) || (u.daily_v5 > 0 && u.v5 >= u.daily_v5)
}

/// `exempt`：站长号或已开通的赞助席。`shared_today`：本重置日已经发过图。
pub fn locked(now: f64, yesterday: DayUsage, shared_today: bool, exempt: bool) -> bool {
    locked_for_mode(Mode::Quota, now, yesterday, shared_today, exempt)
}

pub fn locked_for_mode(
    mode: Mode,
    now: f64,
    yesterday: DayUsage,
    shared_today: bool,
    exempt: bool,
) -> bool {
    now >= starts_at()
        && !exempt
        && !shared_today
        && (mode == Mode::Daily || quota_filled(yesterday))
}

/// Requests, landing and assistant context share the same restriction decision.
pub async fn restriction(state: &AppState, key: &Key) -> Option<&'static str> {
    match lookup(state, key).await {
        Ok(mode) => mode.map(message),
        Err(e) => {
            tracing::warn!(target: "share_lock", "读取分享闸失败，本轮放行：{}", e.0);
            None
        }
    }
}

pub async fn is_locked(state: &AppState, key: &Key) -> bool {
    restriction(state, key).await.is_some()
}

pub async fn enforce(state: &AppState, key: &Key) -> Result<(), ApiError> {
    if let Some(message) = restriction(state, key).await {
        return Err(GateError::with_code(
            403,
            format!("{message} {}", channel_url()),
            reasons::SHARE_LOCK,
        )
        .into());
    }
    Ok(())
}

async fn lookup(state: &AppState, key: &Key) -> Result<Option<Mode>, crate::error::InternalError> {
    let now = state.now();
    if now < starts_at() || key.owner_image_full || key.patron {
        return Ok(None);
    }
    let detection = detection(state).await?;
    if !detection["enforcing"].as_bool().unwrap_or(false) {
        return Ok(None);
    }
    let discord_id = key.subject();
    if discord_id.is_empty() {
        return Ok(None);
    }
    let day_start = state.clock.day_start(now);
    if state.db.has_share_since(discord_id, day_start).await? {
        return Ok(None);
    }
    let mode = Mode::from_setting(detection["mode"].as_str());
    if mode == Mode::Daily {
        return Ok(Some(mode));
    }
    let yesterday = state.clock.day(day_start - 1.0);
    let counter = state.db.get_counter(key.id(), &yesterday).await?;
    Ok(quota_filled(DayUsage {
        legacy: counter.legacy_free_images,
        v5: counter.v5,
        daily_images: key.daily_images(),
        daily_v5: key.daily_v5(),
    })
    .then_some(mode))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn full_v45() -> DayUsage {
        DayUsage {
            legacy: 100,
            v5: 3,
            daily_images: 100,
            daily_v5: 10,
        }
    }

    #[test]
    fn starts_on_the_november_13_reset() {
        let clock = Clock::service();
        let start = starts_at();
        assert_eq!(clock.day(start), "2026-11-13");
        assert_eq!(clock.day(start - 1.0), "2026-11-12");
        assert_eq!(clock.local_hour(start), 7);
    }

    #[test]
    fn medium_tier_locks_a_full_previous_day_until_a_share() {
        let start = starts_at();
        assert!(locked(start, full_v45(), false, false));
        assert!(locked(
            start,
            DayUsage {
                legacy: 0,
                v5: 10,
                daily_images: 100,
                daily_v5: 10,
            },
            false,
            false,
        ));
        assert!(!locked(start - 1.0, full_v45(), false, false));
        assert!(!locked(start, full_v45(), true, false));
        assert!(!locked(start, full_v45(), false, true));
        assert!(!locked(
            start,
            DayUsage {
                legacy: 99,
                v5: 9,
                daily_images: 100,
                daily_v5: 10,
            },
            false,
            false,
        ));
        assert!(
            !locked(
                start,
                DayUsage {
                    legacy: 5,
                    v5: 5,
                    daily_images: 0,
                    daily_v5: 0,
                },
                false,
                false,
            ),
            "上限是 0 的项目不算用满"
        );
    }

    #[test]
    fn client_message_is_classified_as_share_lock() {
        assert_eq!(
            gate_core::reasons::classify(&client_message()),
            reasons::SHARE_LOCK
        );
    }
}
