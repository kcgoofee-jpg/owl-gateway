//! 服务日：每天 [`DAY_RESET_HOUR`]（7）点切换，时区 [`TZ`]（Asia/Shanghai）。两个都是代码常量，不读环境变量。
//!
//! 与 Python 版 `state.day / day_start / month`、`quota_algo` 里的 `time.mktime(strptime(day))`
//! 逐位一致；时间戳一律是 Unix 秒（f64），由调用方传入，这里不读时钟。

use jiff::{Timestamp, tz::TimeZone};

/// 服务日的时区（北京时间）。
pub const TZ: &str = "Asia/Shanghai";
/// 服务日从几点开始（每日额度在这个点重置）。
pub const DAY_RESET_HOUR: i64 = 7;

#[derive(Debug, Clone)]
pub struct Clock {
    tz: TimeZone,
    reset_hour: i64,
}

impl Clock {
    /// 线上用的服务日：[`TZ`] + [`DAY_RESET_HOUR`]。
    pub fn service() -> Self {
        Self::new(TZ, DAY_RESET_HOUR)
    }

    /// `tz` 是 IANA 名（如 "Asia/Shanghai"）；未知名字回落到 UTC（Python 的 ZoneInfo 会抛异常，
    /// 但配置层已保证这里拿到的是合法值）。`reset_hour` 夹在 0..=23。
    pub fn new(tz: &str, reset_hour: i64) -> Self {
        Self {
            tz: TimeZone::get(tz).unwrap_or(TimeZone::UTC),
            reset_hour: reset_hour.clamp(0, 23),
        }
    }

    pub fn reset_hour(&self) -> i64 {
        self.reset_hour
    }

    pub fn tz(&self) -> &TimeZone {
        &self.tz
    }

    fn zoned(&self, ts: f64) -> jiff::Zoned {
        // 与 datetime.fromtimestamp 一致：小数秒按「半偶」舍入到微秒（可能进位到下一秒）
        let mut secs = ts.floor();
        let mut us = ((ts - secs) * 1e6).round_ties_even();
        if us >= 1e6 {
            secs += 1.0;
            us -= 1e6;
        }
        Timestamp::new(secs as i64, (us as i32) * 1000)
            .unwrap_or(Timestamp::UNIX_EPOCH)
            .to_zoned(self.tz.clone())
    }

    /// `state.day(ts)`：服务日字符串 `YYYY-MM-DD`。
    pub fn day(&self, ts: f64) -> String {
        self.zoned(ts - (self.reset_hour * 3600) as f64)
            .strftime("%Y-%m-%d")
            .to_string()
    }

    /// `state.day_start(ts)`：该时刻所在服务日的起点时间戳。
    pub fn day_start(&self, ts: f64) -> f64 {
        let shift = (self.reset_hour * 3600) as f64;
        let local = self.zoned(ts - shift);
        let midnight = local.start_of_day().expect("一天总有起点");
        midnight.timestamp().as_second() as f64 + shift
    }

    /// `state.month(ts)`：**不**按重置点偏移的自然月 `YYYY-MM`（旧代码就是这样，保持一致）。
    pub fn month(&self, ts: f64) -> String {
        self.zoned(ts).strftime("%Y-%m").to_string()
    }

    /// `time.mktime(time.strptime(day, "%Y-%m-%d"))`：该日期本地 0 点的时间戳（进程 TZ=Asia/Shanghai）。
    pub fn local_midnight(&self, day: &str) -> Option<f64> {
        let date: jiff::civil::Date = day.parse().ok()?;
        let z = date.to_zoned(self.tz.clone()).ok()?;
        Some(z.timestamp().as_second() as f64)
    }

    /// 服务日 `day` 的起点：本地 0 点 + 重置小时（quota_algo 的写法）。
    pub fn service_day_start(&self, day: &str) -> Option<f64> {
        self.local_midnight(day)
            .map(|t| t + (self.reset_hour * 3600) as f64)
    }

    /// `datetime.fromtimestamp(ts, tz).strftime(fmt)`。
    pub fn format(&self, ts: f64, fmt: &str) -> String {
        self.zoned(ts).strftime(fmt).to_string()
    }

    /// 本地小时（0..=23）。
    pub fn local_hour(&self, ts: f64) -> i64 {
        i64::from(self.zoned(ts).hour())
    }

    /// 下一次每日重置在什么时候，给成员看的说法：重置点之前是「今天 7 点」，之后是「明天 7 点」。
    pub fn next_reset_phrase(&self, ts: f64) -> String {
        let day = if self.local_hour(ts) < self.reset_hour {
            "今天"
        } else {
            "明天"
        };
        format!("{day} {} 点", self.reset_hour)
    }

    /// 最近 n 个服务日（含今天），从旧到新：`state.week_days(n)`。
    pub fn last_days(&self, now: f64, n: usize) -> Vec<String> {
        (0..n)
            .rev()
            .map(|i| self.day(now - 86400.0 * i as f64))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // 2026-10-14 06:59:59 +08:00 = 1791932399
    const T0659: f64 = 1_791_932_399.0;

    #[test]
    fn service_day_boundary() {
        let c = Clock::new("Asia/Shanghai", 7);
        assert_eq!(c.day(T0659), "2026-10-13");
        assert_eq!(c.day(T0659 + 1.0), "2026-10-14");
        assert_eq!(c.day_start(T0659 + 1.0), T0659 + 1.0);
        assert_eq!(c.day_start(T0659), T0659 + 1.0 - 86400.0);
        assert_eq!(c.month(T0659), "2026-10");
        assert_eq!(c.service_day_start("2026-10-14"), Some(T0659 + 1.0));
    }

    #[test]
    fn reset_phrase_switches_at_midnight_and_reset() {
        let c = Clock::new("Asia/Shanghai", 7);
        assert_eq!(c.next_reset_phrase(T0659), "今天 7 点");
        let midnight = T0659 + 1.0 - 7.0 * 3600.0;
        assert_eq!(c.next_reset_phrase(midnight), "今天 7 点");
        assert_eq!(c.next_reset_phrase(midnight - 1.0), "明天 7 点"); // 前一天 23:59:59
        assert_eq!(c.next_reset_phrase(T0659 + 1.0), "明天 7 点");
    }
}
