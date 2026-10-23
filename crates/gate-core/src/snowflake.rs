//! Discord 雪花 ID → 账号创建时刻（注册防小号、滥用扫描、后台显示号龄用）。

/// Discord 雪花 ID 的纪元（2015-01-01，毫秒）。
const DISCORD_EPOCH_MS: u64 = 1_420_070_400_000;

/// 雪花 ID → 账号创建时刻（Unix 秒）；不是合法的数字 ID 为 None。
pub fn snowflake_created_at(discord_id: &str) -> Option<f64> {
    let id: u64 = discord_id.trim().parse().ok()?;
    if id == 0 {
        return None;
    }
    Some(((id >> 22) + DISCORD_EPOCH_MS) as f64 / 1000.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snowflake_decodes() {
        // Discord 文档里的例子：175928847299117063 → 2016-04-30 11:18:25.796 UTC
        let t = snowflake_created_at("175928847299117063").unwrap();
        assert!((t - 1_462_015_105.796).abs() < 1e-6);
        assert_eq!(snowflake_created_at(""), None);
        assert_eq!(snowflake_created_at("abc"), None);
        assert_eq!(snowflake_created_at("0"), None);
    }
}
