//! 自动驾驶的两个定时任务：全站熔断、单 Key 限流（规则在 `gate_core::autopilot`，这里取数、落库）。

use super::JobError;
use super::support::apply_effects;
use crate::state::AppState;
use gate_core::autopilot::{
    self, BREAKER_UNTIL_KEY, BreakerInputs, BreakerRows, KEY_GUARD_HALF_WEIGHT,
    KEY_GUARD_SKIP_REASONS, KeyGuardInputs,
};
use std::collections::HashMap;

/// 全站熔断：只看上次熔断结束之后的上游失败；`autopilot_breaker_until` 读不懂时按没有失败处理。
pub(super) async fn breaker(state: &AppState) -> Result<String, JobError> {
    let now = state.now();
    let raw = state.db.get_setting(BREAKER_UNTIL_KEY).await?;
    let rows = match autopilot::breaker_since(now, raw.as_deref()) {
        Ok(since) => state.db.autopilot_breaker_rows(since).await?,
        Err(_) => BreakerRows::default(),
    };
    let breaker_until = state.guard.with(|g| g.breaker_until);
    let out = autopilot::breaker(&BreakerInputs {
        now,
        rows,
        breaker_until,
    });
    apply_effects(state, out.effects).await?;
    Ok(out.summary)
}

/// 单 Key 限流：1 小时内被拒 ≥ 120 次（站点层面的拒绝不算，「上一张没出完」按半次算）→ 暂停这把 Key 1 小时。
pub(super) async fn key_guard(state: &AppState) -> Result<String, JobError> {
    let now = state.now();
    let rejects = state
        .db
        .autopilot_rejects(
            now - 3600.0,
            &KEY_GUARD_SKIP_REASONS,
            &KEY_GUARD_HALF_WEIGHT,
        )
        .await?;
    let keys = state.db.autopilot_keys().await?;
    let paused: HashMap<i64, f64> = state
        .pauses
        .snapshot()
        .into_iter()
        .map(|(k, (until, _))| (k, until))
        .collect();
    let out = autopilot::key_guard(&KeyGuardInputs {
        now,
        rejects,
        keys,
        paused,
    });
    apply_effects(state, out.effects).await?;
    Ok(out.summary)
}
