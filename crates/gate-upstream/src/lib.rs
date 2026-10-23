//! NovelAI 上游客户端。
//!
//! - [`client`]：门面——[`client::NaiClient`] 与调用方用到的类型；
//! - [`headers`]：请求头集合与顺序 + 浏览器指纹池；
//! - [`pool`]：Token、令牌池、选号与 V5 预留、健康 / 冷却标记；
//! - `scheduler`：出图节奏（出图间隔 + 开始抖动、出图后静默、等槽位、估算开始时间）；
//! - [`pace`]：每个账号的出图周期估算（纯函数）；
//! - [`limiter`]：每把 Token 的图片并发槽（可缩容、FIFO、取消不漏槽）；
//! - [`token_store`]：后台管理的上游 Token 文件；
//! - 内部：`http`（wreq 客户端）、`account`（订阅查询）、`dispatch`（请求流程与结算）、`error`、`hooks`。

mod account;
pub mod client;
mod dispatch;
mod error;
pub mod headers;
mod hooks;
mod http;
pub mod limiter;
pub mod pace;
pub mod pool;
mod scheduler;
pub mod token_store;
