//! 容量相关的定时任务「上游 V5 快照」：距上次快照满 1 小时就主动读一次订阅，**每个上游账号各写一行**
//! `upstream_snapshots`（后台「额度记录」页各账号的「实测恢复速度」由此估算：相邻两次主动读之间该账号
//! 没出过 V5 图时，余量的上涨就是纯恢复）。
//!
//! 以前这里还有「每小时上限自动调整」（`hourly_cap`，平稳一天 +10）：每小时上限 10-22 删除，
//! 账号节奏改由上游客户端按 429 自动调整出图间隔（`gate_upstream::pace`），不需要定时任务。

use super::JobError;
use super::quota::{allowance_accounts, refresh_allowance};
use crate::state::AppState;

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

/// 快照任务的摘要：「已记录 2 个账号：#1 剩余 63%（恢复 11%/天）、#2 没有读数」。
pub fn snapshot_summary(items: &[(i64, AccountLine)], fresh: bool) -> String {
    if items.is_empty() {
        return "已记录：没有上游账号".to_string();
    }
    let parts: Vec<String> = items
        .iter()
        .map(|(pos, a)| {
            let body = match a.percent {
                Some(p) => format!(
                    "剩余 {}%{}",
                    short(p),
                    a.rate
                        .map(short)
                        .map_or(String::new(), |r| format!("（恢复 {r}%/天）"))
                ),
                None => "没有读数".to_string(),
            };
            format!("#{pos} {body}")
        })
        .collect();
    let mut summary = format!("已记录 {} 个账号：{}", items.len(), parts.join("、"));
    if !fresh {
        summary.push_str("（这次没读到，用的是缓存）");
    }
    summary
}

/// 单个账号这一轮记下的读数（摘要用；写库在构造它之前就完成了）。
pub struct AccountLine {
    pub percent: Option<f64>,
    pub rate: Option<f64>,
    pub anlas: Option<i64>,
}

pub(super) async fn upstream_snapshot(state: &AppState) -> Result<String, JobError> {
    let now = state.now();
    let last = state.db.latest_snapshot_ts().await?;
    if !snapshot_due(last, now) {
        return Ok("距上次不到 1 小时".to_string());
    }
    // 主动读一次，夜里没人用 V5 时也是真实值
    let fresh = refresh_allowance(state).await;
    let accounts = allowance_accounts(state).await?;
    let tokens: Vec<String> = state
        .nai
        .tokens()
        .await
        .iter()
        .map(|t| t.token_id.clone())
        .collect();
    let rows = state.nai.status_rows().await;
    let position = |id: &str| {
        rows.iter()
            .find(|(t, _)| t.token_id == id)
            .map_or(0, |(_, f)| f.position as i64)
    };
    // 每个上游账号一行自己的读数（余额、电量、恢复速度）；Anlas 读不到写 NULL，不写 0
    let mut items: Vec<(i64, AccountLine)> = Vec::new();
    for (t, a) in tokens.iter().zip(accounts.iter()) {
        let line = AccountLine {
            percent: a.percent,
            rate: a.recharge_per_day,
            anlas: state.allowance.anlas_of(t),
        };
        state
            .db
            .insert_upstream_snapshot(
                now,
                t,
                a.percent,
                a.recharge_per_day,
                line.anlas,
                i64::from(fresh),
            )
            .await?;
        items.push((position(t), line));
    }
    if items.is_empty() {
        // 一个账号都没有：仍记一行空 token 的占位，保住「每小时一次」的节奏
        state
            .db
            .insert_upstream_snapshot(now, "", None, None, None, i64::from(fresh))
            .await?;
    }
    Ok(snapshot_summary(&items, fresh))
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

    fn line(percent: Option<f64>, rate: Option<f64>) -> AccountLine {
        AccountLine {
            percent,
            rate,
            anlas: None,
        }
    }

    #[test]
    fn summaries_per_account() {
        assert_eq!(
            snapshot_summary(
                &[(1, line(Some(63.0), Some(11.0))), (2, line(None, None))],
                true
            ),
            "已记录 2 个账号：#1 剩余 63%（恢复 11%/天）、#2 没有读数"
        );
        assert_eq!(
            snapshot_summary(&[(1, line(Some(62.5), None))], false),
            "已记录 1 个账号：#1 剩余 62.5%（这次没读到，用的是缓存）"
        );
        assert_eq!(snapshot_summary(&[], true), "已记录：没有上游账号");
    }
}
