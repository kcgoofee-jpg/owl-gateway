//! 上游响应策略：Retry-After 解析与钳制、403 双义判别（移植自 `app/upstream_policy.py`）。
//!
//! 纯函数模块：不做任何标记 / 记账 / 抛错。429 / 403 / 401 的后续动作按通道各不相同
//! （图片 / 图片流两处的消息、冷却与熔断策略都在调用点），这里只提供统一的无副作用判定。
//! 依赖「当前时间」的函数一律由调用方传入 `now`（Unix 秒，f64），这里不读时钟。

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

/// Retry-After accepts a finite number of seconds or a standard HTTP date.
pub fn parse_retry_after(header: Option<&str>, now: f64) -> f64 {
    let value = header.unwrap_or_default().trim();
    if !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit()) {
        return value
            .parse::<f64>()
            .ok()
            .filter(|n| n.is_finite())
            .map(clamp_retry_after_f64)
            .unwrap_or(20.0);
    }
    httpdate::parse_http_date(value)
        .ok()
        .and_then(|date| date.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|date| clamp_retry_after_f64(date.as_secs_f64() - now))
        .unwrap_or(20.0)
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
    fn retry_after_accepts_standard_http_dates() {
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
            20.0
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
