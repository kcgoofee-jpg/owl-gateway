//! 调 Discord REST 用的 User-Agent（网关的注册服务 / 站长告警、运维命令的公告 / 私信 / 巡检告警共用）。
//!
//! Discord 要求机器人带 `DiscordBot ($url, $versionNumber)` 形式的 UA；空的或 HTTP 库默认的 UA
//! 会被前面的 Cloudflare 判成机器人直接 403（error code 1010）。以前站长告警没设 UA、运维命令写死站点域名，
//! 现在三处共用这里。

/// 不带站点地址的中性写法：`DiscordBot (nai-gate, <版本>)`。网关一直用它；读不到站点配置时也用它。
pub const DISCORD_USER_AGENT: &str =
    concat!("DiscordBot (nai-gate, ", env!("CARGO_PKG_VERSION"), ")");

/// 带站点地址的写法（`site_url` 来自已有配置，如 `SITE_URL`）：去掉首尾空白和末尾 `/`；
/// 为空，或含有不能放进这个格式 / 请求头的字符（空白、括号、逗号、非可见 ASCII）时用 [`DISCORD_USER_AGENT`]。
pub fn discord_user_agent(site_url: &str) -> String {
    let url = site_url.trim().trim_end_matches('/');
    let usable = !url.is_empty()
        && url
            .bytes()
            .all(|b| b.is_ascii_graphic() && !matches!(b, b'(' | b')' | b','));
    if usable {
        format!("DiscordBot ({url}, {})", env!("CARGO_PKG_VERSION"))
    } else {
        DISCORD_USER_AGENT.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn neutral_default() {
        assert!(DISCORD_USER_AGENT.starts_with("DiscordBot (nai-gate, "));
        assert!(DISCORD_USER_AGENT.ends_with(')'));
        for bad in [
            "",
            "   ",
            "https://a b",
            "x(y)",
            "a,b",
            "https://站点",
            "a\tb",
        ] {
            assert_eq!(discord_user_agent(bad), DISCORD_USER_AGENT, "{bad:?}");
        }
    }

    #[test]
    fn uses_site_url() {
        assert_eq!(
            discord_user_agent(" https://owl.example/ "),
            format!(
                "DiscordBot (https://owl.example, {})",
                env!("CARGO_PKG_VERSION")
            )
        );
    }
}
