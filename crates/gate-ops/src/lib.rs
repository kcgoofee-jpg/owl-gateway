//! 运维命令集合（二进制 `nai-ops`）。每个子命令一个模块，`main.rs` 只做参数解析与退出码。
//!
//! 约定：只读命令（traffic / snapshot）用 `mode=ro` 直接打开数据库，不写任何业务表。
//! 巡检与周报 10-27 删除：告警出口只剩网关自己的 `alerts.rs`（事件驱动）与 GitHub 的站点探测 workflow。

pub mod sanitize;
pub mod setting;
pub mod smoke;
pub mod snapshot;
pub mod traffic;
pub mod util;
