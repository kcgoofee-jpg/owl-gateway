//! 容量相关的定时任务「上游 V5 快照」：距上次快照满 1 小时就主动读一次订阅，写一行 `upstream_snapshots`
//! （动态额度的安静时段测速要用：夜里没人用 V5 时余量的变化就是真实的恢复速度）。
//!
//! 以前这里还有「每小时上限自动调整」（`hourly_cap`，平稳一天 +10）：每小时上限 10-22 删除，
//! 账号节奏改由上游客户端按 429 自动调整出图间隔（`gate_upstream::pace`），不需要定时任务。

use super::JobError;
use super::quota::{allowance_accounts, refresh_allowance};
use crate::state::AppState;
use gate_core::quota::allowance_summary;

/// 距离上次快照 ≥ 1 小时（或从未记录）才记一次。`last` = `SELECT MAX(ts) FROM upstream_snapshots`。
pub fn snapshot_due(last: Option<f64>, now: f64) -> bool {
    match last {
        // 0.0 也算没有
        None | Some(0.0) => true,
        Some(t) => now - t >= 3600.0,
    }
}

/// 一位小数，去掉末尾的 `.0`（63.0 → 63，11.5 → 11.5）。
fn short(x: f64) -> String {
    let s = format!("{x:.1}");
    s.strip_suffix(".0").map(str::to_string).unwrap_or(s)
}

/// 快照任务的摘要：「已记录：剩余 63%，恢复 11.0%/天」。
pub fn snapshot_summary(pct: Option<f64>, rate: Option<f64>, fresh: bool) -> String {
    let Some(p) = pct else {
        return "已记录：没有读数（上游账号都读不到）".to_string();
    };
    let mut s = format!("已记录：剩余 {}%", short(p));
    if let Some(r) = rate {
        s.push_str(&format!("，恢复 {r:.1}%/天"));
    }
    if !fresh {
        s.push_str("（这次没读到，用的是缓存）");
    }
    s
}

pub(super) async fn upstream_snapshot(state: &AppState) -> Result<String, JobError> {
    let now = state.now();
    let last = state.db.latest_snapshot_ts().await?;
    if !snapshot_due(last, now) {
        return Ok("距上次不到 1 小时".to_string());
    }
    // 主动读一次，夜里没人用 V5 时也是真实值
    let fresh = refresh_allowance(state).await;
    let (pct, rate) = allowance_summary(&allowance_accounts(state).await?);
    let ids: Vec<String> = state
        .nai
        .tokens()
        .await
        .iter()
        .map(|t| t.token_id.clone())
        .collect();
    // 账号 Anlas 真实余额（订阅接口顺带返回）；一个账号都没读到就写 NULL，不写 0
    let anlas = state.allowance.anlas_total(&ids);
    state
        .db
        .insert_upstream_snapshot(now, pct, rate, anlas, i64::from(fresh))
        .await?;
    Ok(snapshot_summary(pct, rate, fresh))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_due_once_an_hour() {
        assert!(snapshot_due(None, 100.0));
        assert!(snapshot_due(Some(0.0), 100.0));
        assert!(!snapshot_due(Some(1000.0), 1000.0 + 3599.0));
        assert!(snapshot_due(Some(1000.0), 1000.0 + 3600.0));
    }

    #[test]
    fn summaries() {
        assert_eq!(
            snapshot_summary(Some(63.0), Some(11.0), true),
            "已记录：剩余 63%，恢复 11.0%/天"
        );
        assert_eq!(
            snapshot_summary(Some(62.5), None, false),
            "已记录：剩余 62.5%（这次没读到，用的是缓存）"
        );
        assert_eq!(
            snapshot_summary(None, None, false),
            "已记录：没有读数（上游账号都读不到）"
        );
    }
}
