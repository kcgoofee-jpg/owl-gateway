//! 猫头鹰公益站的 Discord 机器人「奶妹」（`nai-bots naimei`）。
//!
//! 核心进程负责 Discord、成员命令、实时分享和补扫；模型与图片解码运行在独立 worker。
//! 查询与分享不依赖 LLM。聊天有持久化、有界队列，同频道串行、跨频道公平调度。
//! 分享只记录成员、消息、频道与时间，提交成功后可加一次确认反应；不自动私信或 AI 评论。
//! 私密响应通过成员命令或消息菜单实现。受限作者内容保留来源与边界，不自动发放或审批。
//! 公会级命令注册；回复禁用 everyone/here 提及。Token 与模型凭据分别交给对应进程。

pub mod api;
pub mod bridge;
pub mod chat;
mod chat_images;
pub mod chatlog;
pub mod commands;
pub mod config;
pub mod events;
pub mod handlers;
pub mod kb;
pub mod naimei;
pub mod share;

pub mod worker;
