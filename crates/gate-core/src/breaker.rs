//! 全站熔断：上游连续出错时，暂停所有出图，给上游账号喘口气。
//!
//! 这不是"按历史数据自适应调阈值"——阈值是写死的常量（15 分钟窗口 / 失败 ≥ 5 次 / 占比 ≥ 30% /
//! 来自 ≥ 2 把 Key），只有固定一条规则。删掉它的风险不是"判错了"，而是上游账号被封或持续 5xx 时
//! 请求继续打过去，把账号彻底打死——那是全站停摆。
//!
//! 单 Key 限流（按 1 小时被拒次数暂停某把 Key）不在这里：那条要推断"成员客户端是否在死循环"，
//! 属于推断性规则，已经删除。

/// 熔断持续多久（秒）。
pub const BREAKER_SECS: i64 = 300;
/// 统计窗口：最近 15 分钟。
pub const BREAKER_WINDOW_SECS: f64 = 900.0;
/// 窗口内失败次数下限。
pub const BREAKER_MIN_FAILS: i64 = 5;
/// 窗口内失败占比下限。
pub const BREAKER_MIN_RATIO: f64 = 0.3;
/// 失败至少要来自这么多把不同的 Key 才熔断（只来自一把 Key 多半是这把 Key 的请求有问题）。
pub const BREAKER_MIN_KEYS: i64 = 2;

/// 一次统计结果。
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct BreakerRows {
    /// 上游真正出错的次数（5xx，或上游回了 2xx 但内容不对）。没有行时为 `None`。
    pub fails: Option<i64>,
    /// 窗口内的出图请求总数（成功 + 失败）。
    pub total: Option<i64>,
    /// 这些失败来自多少把不同的 Key。
    pub fail_keys: i64,
}

/// 统计下界：只 `max(now - 900, 上次熔断结束时间)`——上次熔断之前的数据不再计入，
/// 否则一次熔断结束后立刻又会被同一批历史失败触发。
pub fn breaker_since(now: f64, breaker_until: f64) -> f64 {
    let window_start = now - BREAKER_WINDOW_SECS;
    if breaker_until > window_start {
        breaker_until
    } else {
        window_start
    }
}

/// 判定是否要熔断，并给出一句摘要（写进后台的定时任务记录）。
pub fn breaker_rule(fails: i64, total: i64, fail_keys: i64) -> (bool, String) {
    let ratio = if total == 0 {
        0.0
    } else {
        fails as f64 / total as f64
    };
    if fails < BREAKER_MIN_FAILS || ratio < BREAKER_MIN_RATIO {
        return (false, format!("最近 15 分钟上游失败 {fails}/{total}：正常"));
    }
    if fail_keys < BREAKER_MIN_KEYS {
        return (
            false,
            format!(
                "最近 15 分钟上游失败 {fails}/{total}，只来自 {fail_keys} 把 Key（多半是这把 Key 的请求有问题）：不熔断"
            ),
        );
    }
    (
        true,
        format!(
            "最近 15 分钟上游失败 {fails}/{total}：暂停出图 {} 秒",
            BREAKER_SECS
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trips_only_on_wide_spread_failures() {
        assert!(breaker_rule(6, 12, 2).0, "50% 失败、来自两把 Key");
        assert!(!breaker_rule(4, 12, 3).0, "失败次数不够");
        assert!(!breaker_rule(2, 12, 3).0, "占比不够");
        assert!(!breaker_rule(6, 12, 1).0, "只来自一把 Key");
        assert!(!breaker_rule(0, 0, 0).0, "没有请求");
    }

    #[test]
    fn since_never_goes_behind_the_last_trip() {
        assert_eq!(breaker_since(10_000.0, 0.0), 9_100.0);
        assert_eq!(breaker_since(10_000.0, 9_500.0), 9_500.0);
    }
}
