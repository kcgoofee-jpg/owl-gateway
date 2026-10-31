//! 调 Discord REST 用的 User-Agent（网关的注册服务 / 站长告警、运维命令的公告 / 私信 / 巡检告警共用）。
//!
//! Discord 要求机器人带 `DiscordBot ($url, $versionNumber)` 形式的 UA；空的或 HTTP 库默认的 UA
//! 会被前面的 Cloudflare 判成机器人直接 403（error code 1010）。以前站长告警没设 UA、运维命令写死站点域名，
//! 现在三处共用这里。

/// 不带站点地址的中性写法：`DiscordBot (nai-gate, <版本>)`。网关一直用它；读不到站点配置时也用它。
pub const DISCORD_USER_AGENT: &str =
    concat!("DiscordBot (nai-gate, ", env!("CARGO_PKG_VERSION"), ")");
