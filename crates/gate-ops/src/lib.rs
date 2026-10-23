//! 运维命令集合（二进制 `nai-ops`）。每个子命令一个模块，`main.rs` 只做参数解析与退出码。
//!
//! 约定：
//! - 巡检只读：用 `mode=ro` 直接打开数据库，不写任何业务表；
//! - Discord REST 的基地址读环境变量 `DISCORD_API_BASE`，测试里指向本机假服务，不会真的发消息；
//! - 只给站长发告警（巡检「危险」），没有任何面向成员的消息出口。

pub mod checks;
pub mod discord;
pub mod patrol;
pub mod report;
pub mod sanitize;
pub mod setting;
pub mod smoke;
pub mod snapshot;
pub mod traffic;
pub mod util;
