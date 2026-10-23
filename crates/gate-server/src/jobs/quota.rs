//! 定时任务「额度同步」：把站长手动定的每人额度下发给 `quota_auto=1` 的成员 Key，并更新首页那句「今日额度」
//! （纯逻辑见 `gate_core::quota::sync`）。站长 10-23 删掉了全部自动额度算法，这里不再计算任何额度。
//! 后台改了额度、把 Key 交回统一额度时，[`run_now`] 立即同步一次。
//!
//! 另外放上游账号 V5 读数的取数（「上游 V5 快照」任务和后台「V5 电量记录」用，只显示）。

use super::JobError;
use crate::action_log::log_action;
use crate::audit::{site_bool, site_int};
use crate::error::ApiError;
use crate::state::AppState;
use gate_core::quota::{self, AccountAllowance, Effect, Manual};
use gate_core::site_flags::{
    GLOBAL_DAILY_V5, MEMBER_DAILY_ANLAS, REGISTER_DAILY_IMAGES, REGISTER_DAILY_V5, V5_PAUSED,
};
use serde_json::{Value, json};
use std::time::Instant;

/// 站长手动定的额度（没设过用登记表的默认值）。
pub async fn manual(state: &AppState) -> Result<Manual, gate_db::DbError> {
    let db = &state.db;
    Ok(Manual {
        daily_images: site_int(db, &REGISTER_DAILY_IMAGES).await?,
        daily_v5: site_int(db, &REGISTER_DAILY_V5).await?,
        global_v5: site_int(db, &GLOBAL_DAILY_V5).await?,
        v5_paused: site_bool(db, &V5_PAUSED).await?,
        member_anlas: site_int(db, &MEMBER_DAILY_ANLAS).await?,
    })
}

/// 上游账号 V5 缓存里的读数（`allowance.snapshot(pool)["accounts"]`；只读缓存，不发请求）。
async fn allowance_snapshot(state: &AppState) -> Result<Vec<AccountAllowance>, JobError> {
    let ids: Vec<String> = state
        .nai
        .tokens()
        .await
        .iter()
        .map(|t| t.token_id.clone())
        .collect();
    let snap = state
        .allowance
        .snapshot(&ids)
        .await
        .map_err(|e| JobError::new("AllowanceError", e.to_string()))?;
    let accounts = snap
        .get("accounts")
        .and_then(Value::as_array)
        .map(|a| a.as_slice())
        .unwrap_or(&[]);
    Ok(accounts
        .iter()
        .map(|a| AccountAllowance {
            percent: a.get("percent").and_then(Value::as_f64),
            recharge_per_day: a.get("recharge_per_day").and_then(Value::as_f64),
        })
        .collect())
}

/// 对每个可用的上游账号主动读一次订阅信息（失败忽略）；至少一个成功返回 true。
pub(crate) async fn refresh_allowance(state: &AppState) -> bool {
    use gate_upstream::client::Allowance;
    let now = state.now();
    let mut ok = false;
    for t in state.nai.tokens().await {
        if t.usable(now) && state.allowance.with(&state.nai).resolve(&t).await.is_ok() {
            ok = true;
        }
    }
    ok
}

/// 账号 V5 剩余 % 与恢复 %/天。刚重启时缓存里还没有读数，主动读一次（读失败也不报错）。
pub(crate) async fn allowance_accounts(
    state: &AppState,
) -> Result<Vec<AccountAllowance>, JobError> {
    let mut accounts = allowance_snapshot(state).await?;
    if accounts.iter().all(|a| a.percent.is_none()) {
        refresh_allowance(state).await;
        accounts = allowance_snapshot(state).await?;
    }
    Ok(accounts)
}

/// 把 gate-core 给出的写入按顺序执行（动态额度与自动驾驶共用）。
pub(crate) async fn apply_effects(state: &AppState, effects: Vec<Effect>) -> Result<(), JobError> {
    for e in effects {
        match e {
            Effect::Set { key, value } => state.db.set_setting(&key, &value).await?,
            Effect::Bulk { pairs } => state.db.set_settings_bulk(&pairs).await?,
            Effect::Log {
                actor,
                action,
                target,
                detail,
            } => {
                log_action(
                    &state.db,
                    &actor,
                    &action,
                    &target,
                    &detail,
                    true,
                    state.now(),
                )
                .await;
            }
            Effect::UpdateKeys {
                daily_images,
                daily_v5,
            } => state.db.quota_apply_to_keys(daily_images, daily_v5).await?,
            Effect::TripBreaker {
                seconds,
                reason,
                now,
            } => {
                state
                    .guard
                    .with(|g| g.trip_breaker(seconds as f64, &reason, now))
                    .map_err(|e| JobError::new("ValueError", format!("{e:?}")))?;
            }
            // 临时暂停单把 Key：写 share_state 表（现在只存这个，表名为兼容旧库保留）并更新内存里的暂停表
            Effect::PauseKey {
                key_id,
                seconds,
                reason,
                now,
            } => {
                let until = now + (seconds as f64).max(0.0);
                state.db.upsert_pause(key_id, until, &reason).await?;
                state.pauses.set(key_id, until, reason);
            }
        }
    }
    Ok(())
}

/// 同步一次，返回结果（后台和运行记录用）。
pub async fn compute(state: &AppState) -> Result<Value, JobError> {
    let _serial = state.jobs.quota_guard().await;
    let m = manual(state).await?;
    let keys = state.db.quota_managed_keys().await?;
    let notice = state.db.get_setting(quota::NOTICE_KEY).await?;
    let (effects, changed) = quota::sync(&m, &keys, notice.as_deref(), state.clock.reset_hour());
    apply_effects(state, effects).await?;
    Ok(json!({
        "daily_images": m.daily_images,
        "daily_v5": m.daily_v5,
        "global_v5": m.global_v5,
        "v5_paused": m.v5_paused,
        "changed_keys": changed,
    }))
}

/// 一轮结果的摘要：「V4.5 每人 150 · V5 暂停 · 同步 12 把」。
pub fn summary(r: &Value) -> String {
    let n = |k: &str| r.get(k).and_then(Value::as_i64).unwrap_or(0);
    let lim = |x: i64| {
        if x > 0 {
            x.to_string()
        } else {
            "不限".into()
        }
    };
    let mut parts = vec![format!("V4.5 每人 {}", lim(n("daily_images")))];
    parts.push(if r.get("v5_paused") == Some(&Value::Bool(true)) {
        "V5 暂停".to_string()
    } else {
        format!(
            "V5 每人 {}（全站 {}）",
            lim(n("daily_v5")),
            lim(n("global_v5"))
        )
    });
    if n("changed_keys") > 0 {
        parts.push(format!("同步 {} 把", n("changed_keys")));
    }
    parts.join(" · ")
}

/// 定时任务的一轮。
pub(super) async fn tick(state: &AppState) -> Result<String, JobError> {
    Ok(summary(&compute(state).await?))
}

/// 立即同步一次（后台改了额度、把 Key 交回统一额度时），同样记进 `job_last.quota`
/// （先记在内存里，随下一次定时任务一起落库，见 [`super::registry::Origin::Manual`]）。
/// 出错时不在这里记 Bug：调用方（接口返回 500 / 改 Key 的接口）各自已经记录。
pub async fn run_now(state: &AppState) -> Result<Value, ApiError> {
    let at = state.now();
    let t0 = Instant::now();
    let r = super::registry::guarded(compute(state)).await;
    let ms = i64::try_from(t0.elapsed().as_millis()).unwrap_or(i64::MAX);
    let shown = r.as_ref().map(summary).map_err(Clone::clone);
    state
        .jobs
        .record(
            state,
            "quota",
            at,
            ms,
            &shown,
            super::registry::Origin::Manual,
        )
        .await;
    Ok(r?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_texts() {
        let r = json!({"daily_images": 150, "daily_v5": 0, "global_v5": 150, "v5_paused": true, "changed_keys": 3});
        assert_eq!(summary(&r), "V4.5 每人 150 · V5 暂停 · 同步 3 把");
        let r = json!({"daily_images": 0, "daily_v5": 5, "global_v5": 0, "v5_paused": false, "changed_keys": 0});
        assert_eq!(summary(&r), "V4.5 每人 不限 · V5 每人 5（全站 不限）");
    }
}
