//! 由最近一次真实对账推算每个上游账号的 Anlas 余额与续费倒计时（只读推算，不查上游）。
//!
//! 输入是对账快照里这个账号的固定 / 购买 Anlas 和 `expires_at`，再减去快照之后记在用量日志里的 Anlas。
//! 推算依据（都是推测，页面上要标明）：
//! - 先扣固定额度（随订阅周期到期清零），用完才扣购买的（不过期）——未经官方核实；
//! - 日均用量取最近 3 天的账本用量 ÷ 3；
//! - 到了 `expires_at` 之后周期已经滚动，固定额度会按订阅重置，这里不去猜新周期的数，
//!   只提示「周期已滚动，请重新核对」。

/// 最近一次真实核对超过这么多秒就提示数据偏旧（3 天）。
pub const STALE_SECS: f64 = 3.0 * 86400.0;
/// 日均用量的统计窗口（天）。
pub const AVG_DAYS: f64 = 3.0;

/// 对账快照里一个账号的数据。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SnapshotAccount {
    pub checked_at: f64,
    pub fixed: f64,
    pub purchased: f64,
    /// 订阅周期结束（Unix 秒）；官方没给为 None。
    pub expires_at: Option<f64>,
}

/// 账本用量（来自用量日志）。
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Usage {
    /// 对账快照之后记下的 Anlas。
    pub since_snapshot: f64,
    /// 最近 3 天记下的 Anlas。
    pub last_3d: f64,
    /// 有一部分用量没法确定属于这个账号（日志没记账号且有多个账号）：数字只是近似。
    pub approximate: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Projection {
    /// 没有到期时间，或周期已滚动：给不出预测。
    Unknown,
    /// 按日均用量，固定额度会在到期前用完；还能撑 `days` 天。
    RunsOut { days: f64 },
    /// 到期时预计还剩 `anlas` 点固定额度用不掉。
    Waste { anlas: f64 },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Estimate {
    /// 周期已滚动（`now > expires_at`）：不推算余额，需要重新核对。
    pub cycle_rolled: bool,
    pub balance: Option<f64>,
    pub fixed: Option<f64>,
    pub purchased: Option<f64>,
    pub expires_at: Option<f64>,
    pub days_left: Option<f64>,
    pub avg_daily: f64,
    pub projection: Projection,
    pub snapshot_age_days: f64,
    pub stale: bool,
    pub approximate: bool,
}

/// 纯函数：`now` 由调用方传入。
pub fn estimate(now: f64, snap: SnapshotAccount, usage: Usage) -> Estimate {
    let age = (now - snap.checked_at).max(0.0);
    let avg_daily = (usage.last_3d / AVG_DAYS).max(0.0);
    let base = Estimate {
        cycle_rolled: false,
        balance: None,
        fixed: None,
        purchased: None,
        expires_at: snap.expires_at,
        days_left: None,
        avg_daily,
        projection: Projection::Unknown,
        snapshot_age_days: age / 86400.0,
        stale: age > STALE_SECS,
        approximate: usage.approximate,
    };
    if snap.expires_at.is_some_and(|e| now > e) {
        return Estimate {
            cycle_rolled: true,
            ..base
        };
    }
    let used = usage.since_snapshot.max(0.0);
    let fixed = (snap.fixed - used).max(0.0);
    let purchased = (snap.purchased - (used - snap.fixed).max(0.0)).max(0.0);
    let days_left = snap.expires_at.map(|e| (e - now) / 86400.0);
    let projection = match days_left {
        None => Projection::Unknown,
        Some(d) => {
            let left = fixed - avg_daily * d;
            if left < 0.0 {
                Projection::RunsOut {
                    days: if avg_daily > 0.0 {
                        fixed / avg_daily
                    } else {
                        d
                    },
                }
            } else {
                Projection::Waste { anlas: left }
            }
        }
    };
    Estimate {
        balance: Some(fixed + purchased),
        fixed: Some(fixed),
        purchased: Some(purchased),
        days_left,
        projection,
        ..base
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: f64 = 1_900_000_000.0;
    const DAY: f64 = 86400.0;

    fn snap(fixed: f64, purchased: f64, days_left: Option<f64>, age_days: f64) -> SnapshotAccount {
        SnapshotAccount {
            checked_at: NOW - age_days * DAY,
            fixed,
            purchased,
            expires_at: days_left.map(|d| NOW + d * DAY),
        }
    }

    fn usage(since: f64, last3: f64) -> Usage {
        Usage {
            since_snapshot: since,
            last_3d: last3,
            approximate: false,
        }
    }

    #[test]
    fn subtracts_ledger_fixed_first() {
        let e = estimate(NOW, snap(1000.0, 500.0, Some(10.0), 1.0), usage(300.0, 0.0));
        assert_eq!(e.fixed, Some(700.0));
        assert_eq!(e.purchased, Some(500.0));
        assert_eq!(e.balance, Some(1200.0));
        assert!(!e.stale);
    }

    #[test]
    fn overflow_spills_into_purchased() {
        let e = estimate(NOW, snap(100.0, 500.0, Some(10.0), 1.0), usage(250.0, 0.0));
        assert_eq!(e.fixed, Some(0.0));
        assert_eq!(e.purchased, Some(350.0));
        let e = estimate(NOW, snap(100.0, 50.0, Some(10.0), 1.0), usage(900.0, 0.0));
        assert_eq!(e.balance, Some(0.0));
    }

    #[test]
    fn will_run_out_before_expiry() {
        // 固定 600，日均 100，还剩 10 天：6 天后用完
        let e = estimate(NOW, snap(600.0, 0.0, Some(10.0), 0.5), usage(0.0, 300.0));
        assert_eq!(e.avg_daily, 100.0);
        assert_eq!(e.projection, Projection::RunsOut { days: 6.0 });
        assert_eq!(e.days_left, Some(10.0));
    }

    #[test]
    fn will_waste_at_expiry() {
        // 固定 1500，日均 100，还剩 10 天：到期浪费 500
        let e = estimate(NOW, snap(1500.0, 20.0, Some(10.0), 0.5), usage(0.0, 300.0));
        assert_eq!(e.projection, Projection::Waste { anlas: 500.0 });
    }

    #[test]
    fn no_usage_wastes_everything() {
        let e = estimate(NOW, snap(800.0, 0.0, Some(5.0), 0.5), usage(0.0, 0.0));
        assert_eq!(e.projection, Projection::Waste { anlas: 800.0 });
    }

    #[test]
    fn cycle_rolled_is_not_guessed() {
        let e = estimate(NOW, snap(800.0, 10.0, Some(-0.5), 5.0), usage(100.0, 300.0));
        assert!(e.cycle_rolled);
        assert_eq!(e.balance, None);
        assert_eq!(e.projection, Projection::Unknown);
        assert!(e.stale);
    }

    #[test]
    fn no_expiry_gives_no_projection() {
        let e = estimate(NOW, snap(800.0, 10.0, None, 0.1), usage(0.0, 300.0));
        assert_eq!(e.projection, Projection::Unknown);
        assert_eq!(e.days_left, None);
        assert_eq!(e.balance, Some(810.0));
    }

    #[test]
    fn stale_after_three_days() {
        assert!(!estimate(NOW, snap(1.0, 0.0, Some(9.0), 2.9), usage(0.0, 0.0)).stale);
        let e = estimate(NOW, snap(1.0, 0.0, Some(9.0), 3.1), usage(0.0, 0.0));
        assert!(e.stale);
        assert!((e.snapshot_age_days - 3.1).abs() < 1e-9);
    }

    #[test]
    fn approximate_flag_is_carried() {
        let mut u = usage(0.0, 0.0);
        u.approximate = true;
        assert!(estimate(NOW, snap(1.0, 0.0, Some(9.0), 1.0), u).approximate);
    }
}
