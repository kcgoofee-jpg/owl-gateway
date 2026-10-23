//! 闲置回收的计时窗口：站点自己出不了图的服务日不算成员闲置。
//!
//! 「连续 N 天没成功出图」本来是「从现在往回 N 天」。上游 Token 失效、站长关生图、熔断等情况下，
//! 成员请求照样写日志（error / rejected），只是一张也出不来——这些天不该算到成员头上，
//! 否则一场持续 N 天的故障过后会把全体成员的 Key 删光。
//!
//! 规则：从现在往回数，跳过「不健康」的已完结服务日，凑够 N 天。
//! - 健康服务日 = 全站当日成功出图 ≥ [`HEALTHY_DAY_MIN_IMAGES`] 张的**已完结**服务日；
//! - 窗口先取 N 天，统计窗口内（与窗口有重叠）的不健康已完结服务日个数 k，窗口改为 N + k 天，迭代到稳定；
//! - 最多往回看 [`MAX_LOOKBACK_DAYS`] 天（N 本身更大时以 N 为准）。
//!
//! 平时（窗口里没有不健康的服务日）截止时间与原来的「现在 − N 天」完全相同。
//! 当前未完结的服务日不在这里判断，由调用方看「最近 60 分钟有没有成功出图」决定本轮要不要跳过。

/// 一个已完结的服务日全站成功出图不到这么多张，就算这天站点没在正常服务。
pub const HEALTHY_DAY_MIN_IMAGES: i64 = 50;
/// 跳过不健康的服务日时最多往回看的天数。
pub const MAX_LOOKBACK_DAYS: i64 = 30;
/// 当前服务日：最近这么多秒全站一张都没出成功，本轮不回收。
pub const RECENT_OK_WINDOW: f64 = 3600.0;

const DAY: f64 = 86400.0;

/// 以前闲置回收拉黑时写进黑名单的理由（2.x 也是这个写法）。10-23 起闲置回收只删 Key、不再拉黑；
/// 这里只为认出库里留下的旧行（[`banned_notice`]）。
pub fn ban_reason(days: i64) -> String {
    format!("连续 {days} 天未活动自动回收")
}

/// 被拉黑的人再来领 Key 时看到的话：以前闲置回收拉黑的旧行说明原因，其余（站长手动拉黑）照旧。不提解封。
pub fn banned_notice(reason: &str) -> String {
    let idle_days = reason
        .strip_prefix("连续 ")
        .and_then(|r| r.strip_suffix(" 天未活动自动回收"))
        .and_then(|d| d.parse::<i64>().ok());
    match idle_days {
        Some(days) => format!(
            "你之前的 Key 连续 {days} 天没有出图，已被自动回收，这个 Discord 账号不能再领取 Key。"
        ),
        None => "这个 Discord 账号已被站长停用，无法领取 Key。".to_string(),
    }
}

/// 一个已完结的服务日 `[start, end)` 与当天全站成功出图张数。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ServedDay {
    pub start: f64,
    pub end: f64,
    pub ok_images: i64,
}

impl ServedDay {
    pub fn healthy(&self) -> bool {
        self.ok_images >= HEALTHY_DAY_MIN_IMAGES
    }
}

/// 回收截止时间及跳过的不健康服务日个数。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct IdleWindow {
    /// 最后一次成功出图早于它（且不在宽限期内）的 Key 算闲置。
    pub cutoff: f64,
    /// 窗口里跳过的不健康服务日个数（平时为 0）。
    pub skipped_days: i64,
}

/// 往回看的最大秒数：`max(idle_days, MAX_LOOKBACK_DAYS)` 天。
pub fn max_lookback_days(idle_days: i64) -> i64 {
    idle_days.max(MAX_LOOKBACK_DAYS)
}

/// 按 `days`（已完结服务日，顺序不限；未完结的当天不要放进来）算回收窗口。
///
/// 一个服务日只要与窗口 `[cutoff, now)` 有重叠且不健康就计入 k——部分落在窗口里的那天也整天跳过，
/// 宁可多给成员一点时间，也不在故障刚开始的那天就把人回收掉。
pub fn idle_window(now: f64, idle_days: i64, days: &[ServedDay]) -> IdleWindow {
    let base = idle_days.max(0);
    let cap = max_lookback_days(base);
    let mut window_days = base;
    loop {
        let cutoff = now - window_days as f64 * DAY;
        let k = days
            .iter()
            .filter(|d| !d.healthy() && d.end > cutoff && d.end <= now)
            .count() as i64;
        let next = (base + k).min(cap);
        // 窗口只会变大（k 随窗口单调不减），有上限，所以一定会停
        if next <= window_days {
            return IdleWindow {
                cutoff,
                skipped_days: window_days - base,
            };
        }
        window_days = next;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn banned_notice_explains_idle_recycling_and_never_mentions_unbanning() {
        // 2.x 写进库里的理由也是这个写法
        assert_eq!(ban_reason(3), "连续 3 天未活动自动回收");
        let idle = banned_notice(&ban_reason(3));
        assert_eq!(
            idle,
            "你之前的 Key 连续 3 天没有出图，已被自动回收，这个 Discord 账号不能再领取 Key。"
        );
        for manual in ["站长手动封禁", "", "连续 x 天未活动自动回收"] {
            assert_eq!(
                banned_notice(manual),
                "这个 Discord 账号已被站长停用，无法领取 Key。"
            );
        }
        assert!(!idle.contains("解封") && !idle.contains("联系"));
    }

    /// 2026-10-14 07:00 +08:00（服务日 10-14 的起点）
    const D0: f64 = 1_791_932_400.0;

    /// 今天之前的 n 个已完结服务日（从近到远），张数依次取 `counts`。
    fn days(today_start: f64, counts: &[i64]) -> Vec<ServedDay> {
        counts
            .iter()
            .enumerate()
            .map(|(i, &n)| {
                let end = today_start - i as f64 * DAY;
                ServedDay {
                    start: end - DAY,
                    end,
                    ok_images: n,
                }
            })
            .collect()
    }

    #[test]
    fn healthy_history_keeps_plain_cutoff() {
        let now = D0 + 5.0 * 3600.0;
        let d = days(D0, &[500; 30]);
        let w = idle_window(now, 3, &d);
        assert_eq!(w.cutoff, now - 3.0 * DAY);
        assert_eq!(w.skipped_days, 0);
        // 没有任何历史（全新部署之外的情况由调用方保证）：同样是现在 − N 天
        assert_eq!(idle_window(now, 3, &[]).cutoff, now - 3.0 * DAY);
    }

    #[test]
    fn exactly_threshold_is_healthy() {
        let now = D0 + 3600.0;
        let d = days(D0, &[HEALTHY_DAY_MIN_IMAGES; 10]);
        assert_eq!(idle_window(now, 3, &d).skipped_days, 0);
        let d = days(D0, &[HEALTHY_DAY_MIN_IMAGES - 1, 500, 500, 500, 500, 500]);
        assert_eq!(idle_window(now, 3, &d).skipped_days, 1);
    }

    #[test]
    fn three_dead_days_push_window_back_three_days() {
        let now = D0 + 5.0 * 3600.0;
        let mut counts = vec![0, 0, 0];
        counts.extend([500; 27]);
        let w = idle_window(now, 3, &days(D0, &counts));
        assert_eq!(w.skipped_days, 3);
        assert_eq!(w.cutoff, now - 6.0 * DAY);
    }

    #[test]
    fn iterates_until_stable() {
        // 近 3 天里只有第 2 天不健康 → 窗口 4 天，又碰到第 4 天也不健康 → 5 天，第 5 天健康 → 停
        let now = D0 + 5.0 * 3600.0;
        let w = idle_window(now, 3, &days(D0, &[500, 0, 500, 0, 500, 500, 500]));
        assert_eq!(w.skipped_days, 2);
        assert_eq!(w.cutoff, now - 5.0 * DAY);
    }

    #[test]
    fn partially_covered_dead_day_counts_whole() {
        // 现在 12:00；窗口 3 天的起点落在第 3 个已完结日的 12:00，那天有一半在窗口里且不健康 → 整天跳过
        let now = D0 + 5.0 * 3600.0;
        let w = idle_window(now, 3, &days(D0, &[500, 500, 0, 500, 500]));
        assert_eq!(w.skipped_days, 1);
        // 完全在窗口之前的不健康日不影响
        let w = idle_window(now, 3, &days(D0, &[500, 500, 500, 500, 0]));
        assert_eq!(w.skipped_days, 0);
    }

    #[test]
    fn lookback_is_capped() {
        let now = D0 + 3600.0;
        let w = idle_window(now, 3, &days(D0, &[0; 40]));
        assert_eq!(w.cutoff, now - MAX_LOOKBACK_DAYS as f64 * DAY);
        assert_eq!(w.skipped_days, MAX_LOOKBACK_DAYS - 3);
        // N 本身超过上限时不缩短
        let w = idle_window(now, 45, &days(D0, &[0; 60]));
        assert_eq!(w.cutoff, now - 45.0 * DAY);
    }

    #[test]
    fn unfinished_days_are_ignored() {
        // 调用方误把还没结束的当天放进来：end > now 的不计
        let now = D0 + 3600.0;
        let today = ServedDay {
            start: D0,
            end: D0 + DAY,
            ok_images: 0,
        };
        let mut d = days(D0, &[500; 10]);
        d.push(today);
        assert_eq!(idle_window(now, 3, &d).skipped_days, 0);
    }
}
