//! V5 读数统计与运行保护动作。

use crate::{primitives, values};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// V5 暂停时成员看到的话（拒绝 V5 时的 402，原因码 `v5_exhausted`）。
pub const V5_PAUSED_TEXT: &str = "V5 暂停，未扣费。请使用 V4.5。";
/// Meter readings at or above this are treated as full (recovery is clipped, not measurable).
pub const V5_FULL_PERCENT: f64 = 99.5;

// ---------------------------------------------------------------------------
// 错误与副作用
// ---------------------------------------------------------------------------

/// 运行保护读取到无效的数值设置。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct DecideError(pub String);

impl DecideError {
    pub fn error_kind(&self) -> &'static str {
        "invalid_setting"
    }
}

/// 一次写入 / 动作，按发生的顺序排列（额度同步与自动驾驶共用）。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Effect {
    /// `db.set_setting(key, value)`（值已是 `str(...)` 之后的精确字符串）。
    Set { key: String, value: String },
    /// `db.set_settings_bulk({...})`（保持字典顺序）。
    Bulk { pairs: Vec<(String, String)> },
    /// `log_action(db, actor, action, target, detail)`。
    Log {
        actor: String,
        action: String,
        target: String,
        detail: String,
    },
    /// 自动驾驶：`guard.trip_breaker(seconds, reason, now)`（内存态）。
    TripBreaker {
        seconds: i64,
        reason: String,
        now: f64,
    },
    /// 自动驾驶：临时暂停单把 Key（落 `share_state` 表并更新内存暂停表）。
    PauseKey {
        key_id: i64,
        seconds: i64,
        reason: String,
        now: f64,
    },
}

// ---------------------------------------------------------------------------
// V5 电量记录（只显示，不参与任何限额）
// ---------------------------------------------------------------------------

/// 一条上游快照（`upstream_snapshots`）。
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuietSnap {
    pub ts: f64,
    pub percent: f64,
}

/// `quiet_recovery` 的输入：按 `ts` 升序的快照 + 相邻两条之间的 V5 出图条数。
#[derive(Debug, Clone, PartialEq, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct QuietInputs {
    pub snaps: Vec<QuietSnap>,
    /// `counts[i]` = `snaps[i]` 到 `snaps[i+1]` 之间的 V5 成功日志条数（间隔 > 3 小时的不会被读，值任意）。
    pub counts: Vec<i64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Quiet {
    pub rate: f64,
    pub hours: f64,
    pub gain: f64,
}

impl Quiet {
    pub fn to_value(&self) -> Value {
        let mut m = Map::new();
        m.insert("rate".into(), values::float(self.rate));
        m.insert("hours".into(), values::float(self.hours));
        m.insert("gain".into(), values::float(self.gain));
        Value::Object(m)
    }
}

/// `quiet_recovery` 回看的窗口（72 小时）。
pub const QUIET_WINDOW_SECS: f64 = 72.0 * 3600.0;

/// 服务层查快照的下界参数：`ts > now - 72h`。
pub fn quiet_snaps_since(now: f64) -> f64 {
    now - QUIET_WINDOW_SECS
}

/// 要数 V5 条数的相邻快照对（间隔 ≤ 3 小时）的下标 `i`（即 `snaps[i]` → `snaps[i+1]`）。
/// 服务层只需要对这些下标执行 `quiet.counts[i]` 那条查询，其余位置填 0 即可。
pub fn quiet_count_pairs(snaps: &[QuietSnap]) -> Vec<usize> {
    snaps
        .windows(2)
        .enumerate()
        .filter(|(_, w)| {
            let skipped = w[1].ts - w[0].ts > 3.0 * 3600.0;
            !skipped
        })
        .map(|(i, _)| i)
        .collect()
}

/// 独立测量恢复速度：相邻两次「主动读」的快照之间一张 V5 都没出 → 剩余的上涨就是纯恢复。
/// 累计 ≥ 4 个安静小时才给结果。
pub fn quiet_recovery(q: &QuietInputs) -> Option<Quiet> {
    let mut hours = 0.0_f64;
    let mut gain = 0.0_f64;
    for (i, w) in q.snaps.windows(2).enumerate() {
        let (a, b) = (&w[0], &w[1]);
        if b.ts - a.ts > 3.0 * 3600.0 {
            continue;
        }
        // A meter pinned at 100% cannot rise: counting those hours as zero gain under-reads the rate.
        if a.percent >= V5_FULL_PERCENT || b.percent >= V5_FULL_PERCENT {
            continue;
        }
        if q.counts.get(i).copied().unwrap_or(0) == 0 {
            hours += (b.ts - a.ts) / 3600.0;
            gain += b.percent - a.percent;
        }
    }
    if hours < 4.0 {
        return None;
    }
    Some(Quiet {
        rate: primitives::round(gain / hours * 24.0, 2),
        hours: primitives::round(hours, 1),
        gain,
    })
}

/// 一个上游账号的 V5 缓存（`allowance.snapshot` 里的一项）。
/// 线上 `percent` 一定是 int（`allowance.resolve` 拒绝非 int），`recharge_per_day` 是 `round(86400/秒数, 1)` 的 float。
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AccountAllowance {
    pub percent: Option<f64>,
    pub recharge_per_day: Option<f64>,
}

/// `_allowance` 的纯部分：账号 V5 剩余 % 与恢复 %/天（多个账号取平均；没有读数返回 `(None, None)`）。
pub fn allowance_summary(accounts: &[AccountAllowance]) -> (Option<f64>, Option<f64>) {
    let rows: Vec<&AccountAllowance> = accounts.iter().filter(|a| a.percent.is_some()).collect();
    if rows.is_empty() {
        return (None, None);
    }
    let pct = primitives::sum(rows.iter().filter_map(|a| a.percent)) / rows.len() as f64;
    // `if a.get("recharge_per_day")`：0 / None 不算
    let rates: Vec<f64> = rows
        .iter()
        .filter_map(|a| a.recharge_per_day)
        .filter(|r| *r != 0.0)
        .collect();
    let rate = if rates.is_empty() {
        None
    } else {
        Some(primitives::sum(rates.iter().copied()) / rates.len() as f64)
    };
    (Some(pct), rate)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snaps(points: &[(f64, f64)]) -> QuietInputs {
        QuietInputs {
            snaps: points
                .iter()
                .map(|&(h, percent)| QuietSnap {
                    ts: h * 3600.0,
                    percent,
                })
                .collect(),
            counts: vec![0; points.len().saturating_sub(1)],
        }
    }

    #[test]
    fn quiet_recovery_ignores_hours_pinned_at_full() {
        // 0.4%/h for 6 hours, then pinned at 100% for 6 hours: 9.6%/day, not 4.8%/day.
        let mut pts: Vec<(f64, f64)> = (0..=6).map(|h| (h as f64, 97.0 + 0.4 * h as f64)).collect();
        pts.extend((7..=12).map(|h| (h as f64, 100.0)));
        let q = quiet_recovery(&snaps(&pts)).expect("enough quiet hours");
        assert_eq!(q.hours, 6.0);
        assert_eq!(q.rate, 9.6);
    }

    #[test]
    fn quiet_recovery_needs_four_unclipped_hours() {
        let pts: Vec<(f64, f64)> = (0..=10).map(|h| (h as f64, 100.0)).collect();
        assert!(quiet_recovery(&snaps(&pts)).is_none());
    }
}
