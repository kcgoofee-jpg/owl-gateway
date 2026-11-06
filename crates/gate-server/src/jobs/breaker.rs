//! 全站熔断（规则在 `gate_core::breaker`，这里取数、落库）。

use super::JobError;
use crate::action_log::log_action;
use crate::state::AppState;
use gate_core::breaker::{self, BREAKER_SECS};

/// 上次熔断结束的时刻存在这个设置里：下一轮统计从这个时刻之后算起，
/// 免得同一次事故在冷却结束后立刻把全站又按下去。
const BREAKER_UNTIL_KEY: &str = "autopilot_breaker_until";

pub(super) async fn run(state: &AppState) -> Result<String, JobError> {
    let now = state.now();
    let raw = state.db.get_setting(BREAKER_UNTIL_KEY).await?;
    // 设置读不懂就按「没有上次熔断」处理，不让整轮失败
    let last = raw
        .as_deref()
        .and_then(|v| v.trim().parse::<f64>().ok())
        .filter(|v| v.is_finite())
        .unwrap_or(0.0);
    let rows = state
        .db
        .breaker_rows(breaker::breaker_since(now, last))
        .await?;
    let (trip, summary) = breaker::breaker_rule(
        rows.fails.unwrap_or(0),
        rows.total.unwrap_or(0),
        rows.fail_keys,
    );
    if !trip {
        return Ok(summary);
    }
    let until = now + BREAKER_SECS as f64;
    // 已经在熔断中就不重复触发，只报一句
    if state.guard.with(|g| g.breaker_until) > now {
        return Ok(format!("{summary}（正在熔断中，不重复触发）"));
    }
    state
        .guard
        .with(|g| g.trip_breaker(BREAKER_SECS as f64, "", now))
        .map_err(|e| JobError::new("invalid_value", format!("{e:?}")))?;
    state
        .db
        .set_setting(BREAKER_UNTIL_KEY, &until.to_string())
        .await?;
    log_action(
        &state.db,
        "系统",
        "全站熔断",
        "全部上游账号",
        &summary,
        true,
        now,
    )
    .await;
    tracing::warn!(target: "breaker", "{summary}");
    Ok(summary)
}
