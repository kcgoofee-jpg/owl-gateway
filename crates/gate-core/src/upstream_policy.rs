//! 上游响应策略：Retry-After 解析与钳制、403 双义判别（移植自 `app/upstream_policy.py`）。
//!
//! 纯函数模块：不做任何标记 / 记账 / 抛错。429 / 403 / 401 的后续动作按通道各不相同
//! （图片 / 图片流两处的消息、冷却与熔断策略都在调用点），这里只提供统一的无副作用判定。
//! 依赖「当前时间」的函数一律由调用方传入 `now`（Unix 秒，f64），这里不读时钟。

use crate::pyc;
use crate::pyext::py_int_str;

pub const RETRY_AFTER_MIN: f64 = 5.0;
pub const RETRY_AFTER_MAX: f64 = 3600.0;

/// 任何来源的冷却秒数都钳制到 5..3600 的有限值（inf / NaN / 超大值不能造成永久停摆）。
/// 浮点入口：NaN 按「无法解析」处理，回 20 秒。
pub fn clamp_retry_after_f64(seconds: f64) -> f64 {
    if seconds.is_nan() {
        return 20.0;
    }
    seconds.clamp(RETRY_AFTER_MIN, RETRY_AFTER_MAX)
}

// ----------------------------------------------------------------------------- HTTP 日期（复刻 email.utils.parsedate_to_datetime）

const MONTHNAMES: [&str; 24] = [
    "jan",
    "feb",
    "mar",
    "apr",
    "may",
    "jun",
    "jul",
    "aug",
    "sep",
    "oct",
    "nov",
    "dec",
    "january",
    "february",
    "march",
    "april",
    "may",
    "june",
    "july",
    "august",
    "september",
    "october",
    "november",
    "december",
];
const DAYNAMES: [&str; 7] = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"];

/// `email._parseaddr._timezones`
fn named_timezone(tz: &str) -> Option<i128> {
    Some(match tz {
        "UT" | "UTC" | "GMT" | "Z" => 0,
        "AST" => -400,
        "ADT" => -300,
        "EST" => -500,
        "EDT" => -400,
        "CST" => -600,
        "CDT" => -500,
        "MST" => -700,
        "MDT" => -600,
        "PST" => -800,
        "PDT" => -700,
        _ => return None,
    })
}

struct Parsed {
    year: i128,
    month: i128,
    day: i128,
    hour: i128,
    minute: i128,
    second: i128,
    /// 时区偏移秒；`None` 表示没写时区、`-0000` 或认不出来（按 UTC 处理）。
    tz_seconds: Option<i128>,
}

/// `email._parseaddr._parsedate_tz`（CPython 3.12）。
fn parsedate_tz(data: &str) -> Option<Parsed> {
    if data.is_empty() {
        return None;
    }
    let mut data: Vec<String> = data
        .split(pyc::is_space)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    if data.is_empty() {
        return None; // 全是空白
    }
    // 星期几后面的逗号后空白是可选的，所以要两种情况都兼顾
    if data[0].ends_with(',') || DAYNAMES.contains(&data[0].to_lowercase().as_str()) {
        data.remove(0);
    } else if let Some(i) = data[0].rfind(',') {
        data[0] = data[0][i + 1..].to_string();
    }
    if data.len() == 3 {
        // RFC 850 日期（已废弃）：dd-mon-yy
        let stuff: Vec<String> = data[0].split('-').map(str::to_string).collect();
        if stuff.len() == 3 {
            let mut merged = stuff;
            merged.extend(data[1..].iter().cloned());
            data = merged;
        }
    }
    if data.len() == 4 {
        let s = data[3].clone();
        let i = s.find('+').or_else(|| s.find('-'));
        match i {
            Some(i) if i > 0 => {
                data.truncate(3);
                data.push(s[..i].to_string());
                data.push(s[i..].to_string());
            }
            _ => data.push(String::new()), // 补一个空的时区
        }
    }
    if data.len() < 5 {
        return None;
    }
    data.truncate(5);
    let mut it = data.into_iter();
    let (mut dd, mut mm, mut yy, mut tm, mut tz) =
        (it.next()?, it.next()?, it.next()?, it.next()?, it.next()?);
    if dd.is_empty() || mm.is_empty() || yy.is_empty() {
        return None;
    }
    mm = mm.to_lowercase();
    if !MONTHNAMES.contains(&mm.as_str()) {
        let swapped = dd.to_lowercase();
        dd = mm;
        mm = swapped;
        if !MONTHNAMES.contains(&mm.as_str()) {
            return None;
        }
    }
    let mut month = MONTHNAMES.iter().position(|m| *m == mm)? as i128 + 1;
    if month > 12 {
        month -= 12;
    }
    if dd.ends_with(',') {
        dd.pop();
    }
    if yy.find(':').is_some_and(|i| i > 0) {
        std::mem::swap(&mut yy, &mut tm);
    }
    if yy.ends_with(',') {
        yy.pop();
        if yy.is_empty() {
            return None;
        }
    }
    // `yy[0].isdigit()`：yy 此时一定非空
    if !yy.chars().next().is_some_and(crate::pyext::is_digit) {
        std::mem::swap(&mut yy, &mut tz);
    }
    if tm.ends_with(',') {
        tm.pop();
    }
    let parts: Vec<&str> = tm.split(':').collect();
    let (thh, tmm, tss): (String, String, String) = match parts.len() {
        2 => (parts[0].into(), parts[1].into(), "0".into()),
        3 => (parts[0].into(), parts[1].into(), parts[2].into()),
        1 if parts[0].contains('.') => {
            // 个别不规范的客户端用 '.' 分隔时间
            let sub: Vec<&str> = parts[0].split('.').collect();
            match sub.len() {
                2 => (sub[0].into(), sub[1].into(), "0".into()),
                3 => (sub[0].into(), sub[1].into(), sub[2].into()),
                _ => return None,
            }
        }
        _ => return None,
    };
    let mut year = py_int_str(&yy).ok()?;
    let day = py_int_str(&dd).ok()?;
    let hour = py_int_str(&thh).ok()?;
    let minute = py_int_str(&tmm).ok()?;
    let second = py_int_str(&tss).ok()?;
    // 两位数年份按 POSIX 约定换算成四位
    if year < 100 {
        year += if year > 68 { 1900 } else { 2000 };
    }
    let tz = tz.to_uppercase();
    let tzoffset: Option<i128> = match named_timezone(&tz) {
        Some(v) => Some(v),
        None => match py_int_str(&tz).ok() {
            // "-0000"：显式声明不知道源时区，按未写时区处理
            Some(0) if tz.starts_with('-') => None,
            other => other,
        },
    };
    // 把 -0500 这种偏移换成秒：-18000
    let tz_seconds = tzoffset.map(|off| {
        if off == 0 {
            return 0;
        }
        let (sign, mag) = if off < 0 {
            (-1, off.saturating_neg())
        } else {
            (1, off)
        };
        // mag 为正，`//` 和 `%` 与 Rust 的 `/` `%` 一致
        sign * ((mag / 100)
            .saturating_mul(3600)
            .saturating_add((mag % 100) * 60))
    });
    Some(Parsed {
        year,
        month,
        day,
        hour,
        minute,
        second,
        tz_seconds,
    })
}

fn is_leap(y: i128) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

fn days_in_month(y: i128, m: i128) -> i128 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ => {
            if is_leap(y) {
                29
            } else {
                28
            }
        }
    }
}

/// 公历日期 → 距 1970-01-01 的天数（Howard Hinnant 的 days_from_civil）。
fn days_from_civil(y: i128, m: i128, d: i128) -> i128 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn fits_c_int(v: i128) -> bool {
    i32::try_from(v).is_ok()
}

/// 一段日期的解析结果。
enum DateParse {
    /// UTC 时间戳（秒）。
    Ts(f64),
    /// 格式不对 / 日期不合法：跳过这一段，试下一段。
    Skip,
    /// 数值大到整数装不下：整个 header 按默认 20 秒。
    Abort,
}

/// 解析 RFC 5322 日期（`email.utils.parsedate_to_datetime` 的口径），取 UTC 时间戳（秒，f64）。
fn parsedate_to_timestamp(data: &str) -> DateParse {
    let Some(p) = parsedate_tz(data) else {
        return DateParse::Skip;
    };
    if let Some(tz) = p.tz_seconds {
        // timedelta(seconds=tz) 的天数上限 999999999
        if !(-86_399_999_913_600..=86_399_999_999_999).contains(&tz) {
            return DateParse::Abort;
        }
        if tz <= -86_400 || tz >= 86_400 {
            return DateParse::Skip;
        }
    }
    // datetime(year, month, day, hour, minute, second)：先把六个参数都转成 C int，再逐项检查范围
    if ![p.year, p.month, p.day, p.hour, p.minute, p.second]
        .into_iter()
        .all(fits_c_int)
    {
        return DateParse::Abort;
    }
    let valid = (1..=9999).contains(&p.year)
        && (1..=12).contains(&p.month)
        && (1..=days_in_month(p.year, p.month)).contains(&p.day)
        && (0..=23).contains(&p.hour)
        && (0..=59).contains(&p.minute)
        && (0..=59).contains(&p.second);
    if !valid {
        return DateParse::Skip;
    }
    let days = days_from_civil(p.year, p.month, p.day);
    let secs = days * 86_400 + p.hour * 3600 + p.minute * 60 + p.second - p.tz_seconds.unwrap_or(0);
    DateParse::Ts(secs as f64)
}

/// 十进制秒数或 RFC 7231 HTTP 日期（Cloudflare / WAF 常用后者）；其它格式按默认 20 秒处理。
///
/// 只认数字会让「Retry-After: Wed, 21 Oct 2026 07:28:00 GMT」退化成 20 秒，在真实限流窗口内提前重试。
/// RFC 允许「Wed, 21 Oct …」这种带星期前缀的写法：先按逗号拆开，第一段解析失败就跳过，
/// 靠第二段 `21 Oct 2026 07:28:00 GMT` 解析成功。
///
/// 超长（> 4300 位）或溢出的值不抛异常，一律按默认 20 秒，保证冷却生效。
pub fn parse_retry_after(header: Option<&str>, now: f64) -> f64 {
    let value = pyc::strip(header.unwrap_or(""));
    if value.is_empty() {
        return 20.0;
    }
    if value.is_ascii() && value.bytes().all(|b| b.is_ascii_digit()) {
        if value.len() > 4300 {
            return 20.0;
        }
        // float(int(value))：十进制串到最近浮点数，与 Rust 的 parse 同为正确舍入
        let f: f64 = value.parse().unwrap_or(f64::INFINITY);
        if f.is_infinite() {
            return 20.0;
        }
        return clamp_retry_after_f64(f);
    }
    // 允许多个日期值（RFC 允许逗号分隔），取第一个能解析的
    for part in value.split(',') {
        let part = pyc::strip(part);
        if part.is_empty() {
            continue;
        }
        match parsedate_to_timestamp(part) {
            DateParse::Ts(ts) => return clamp_retry_after_f64(ts - now),
            DateParse::Skip => continue,
            DateParse::Abort => return 20.0,
        }
    }
    20.0
}

// ----------------------------------------------------------------------------- 403 双义判别

/// NovelAI 用 403 表示两类完全不同的东西：账号 / 风控被拒与「模型不存在」。
/// 后者纯粹是成员填错模型，不能报「账号被隔离」，更不能污染账号健康度。
pub const MODEL_403_MARKERS: [&str; 4] = [
    "doesn't exist",
    "does not exist",
    "is only available via",
    "model must be a valid enum",
];

pub fn is_model_level_403(body_text: &str) -> bool {
    let low = body_text.to_lowercase();
    MODEL_403_MARKERS.iter().any(|m| low.contains(m))
}

/// NovelAI「并发锁」的原文片段（小写比较）：同一个账号的上一张还没释放时，出图接口回 429（有时 409）
/// `{"statusCode":429,"message":"Concurrent generation is locked"}`。
pub const CONCURRENCY_LOCK_MARKER: &str = "concurrent generation is locked";

/// 上游这次是「并发锁」而不是限流：状态码 429 / 409，正文（前几百字节即可）含 [`CONCURRENCY_LOCK_MARKER`]（不分大小写）。
/// 不是限流：不冷却账号、不算 429；网关在同一账号上稍后重试。
pub fn is_concurrency_lock(status: u16, body_text: &str) -> bool {
    matches!(status, 429 | 409) && body_text.to_lowercase().contains(CONCURRENCY_LOCK_MARKER)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn concurrency_lock_is_told_apart_from_rate_limits() {
        let body = r#"{"statusCode":429,"message":"Concurrent generation is locked"}"#;
        assert!(is_concurrency_lock(429, body));
        assert!(is_concurrency_lock(409, "CONCURRENT GENERATION IS LOCKED"));
        assert!(!is_concurrency_lock(
            429,
            r#"{"statusCode":429,"message":"Too many requests"}"#
        ));
        assert!(!is_concurrency_lock(500, body), "只认 429 / 409");
        assert!(!is_concurrency_lock(409, "conflict"));
    }

    // 2026-10-21 07:28:00 UTC
    const T: f64 = 1_792_567_680.0;

    #[test]
    fn clamp_bounds_and_nan_default() {
        assert_eq!(clamp_retry_after_f64(0.0), 5.0);
        assert_eq!(clamp_retry_after_f64(-3.0), 5.0);
        assert_eq!(clamp_retry_after_f64(1e9), 3600.0);
        assert_eq!(clamp_retry_after_f64(30.0), 30.0);
        assert_eq!(clamp_retry_after_f64(f64::INFINITY), 3600.0);
        assert_eq!(clamp_retry_after_f64(f64::NAN), 20.0);
    }

    #[test]
    fn retry_after_quirks_are_pinned() {
        // 非纯数字一律 20 秒（"-5"、"1e9"、"inf" 都不走 float() 路径）
        for junk in ["-5", "1e9", "tomorrow", "inf", ""] {
            assert_eq!(parse_retry_after(Some(junk), T), 20.0, "{junk}");
        }
        assert_eq!(parse_retry_after(None, T), 20.0);
        assert_eq!(parse_retry_after(Some("7"), T), 7.0);
        // HTTP 日期里的逗号会把它拆成 "Wed" 和 "21 Oct ..." 两段，靠第二段解析成功
        let date = "Wed, 21 Oct 2026 07:28:00 GMT";
        assert_eq!(parse_retry_after(Some(date), T - 120.0), 120.0);
        assert_eq!(parse_retry_after(Some(date), T + 3600.0), 5.0); // 过去的时间钳到下限
        assert_eq!(parse_retry_after(Some(date), T - 7200.0), 3600.0);
        assert_eq!(
            parse_retry_after(Some("21 Oct 2026 15:28:00 +0800"), T - 60.0),
            60.0
        );
    }

    #[test]
    fn overflowing_headers_fall_back_to_the_default() {
        let huge_year = "21 Oct 99999999999 00:00:00 GMT";
        assert_eq!(parse_retry_after(Some(huge_year), T), 20.0);
        let too_long = "1".repeat(4301);
        assert_eq!(parse_retry_after(Some(&too_long), T), 20.0);
        // 日期不合法只是跳过这一段
        assert_eq!(parse_retry_after(Some("30 Feb 2026 00:00:00 GMT"), T), 20.0);
    }
}
