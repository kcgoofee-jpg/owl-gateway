//! 猫头鹰公益站的 Discord 机器人「奶妹」（`nai-bots naimei`）。
//!
//! 命令：`/register`（领 Key）、`/resetkey`（换 Key），
//! 以及每分钟一次的心跳（后台据此显示在线状态）。额度看网页，管理员操作都在网页后台，
//! 机器人里没有管理员命令；执法用的燕双鹰已删除（它什么都不做）。
//!
//! AI 对话（可缺省）：配置 `NAIMEI_LLM_*` 后，成员 @奶妹 或回复奶妹的消息会触发对话
//! （检索知识库 → 调 OpenAI 兼容接口 → 出口过滤 → 分片发送）。这是全机器人唯一会读取
//! 普通消息文本的功能，只读触发消息本身，不存库、不转发；未配置时对话整体关闭。
//!
//! 重要约束（Discord 曾因 OAuth 突发 + 群发私信 + 机器人在 NSFW 论坛自动点赞给应用打过标记，错误码 680009）：
//! 命令只注册到一个服务器（公会级）；斜杠命令的回复一律 ephemeral（只有本人可见）；
//! 没有私信；没有论坛点赞 / AI 评论。10-31 起为对话功能开启了特权 Intent（MESSAGE_CONTENT），
//! 这是经过站长确认的例外：只读 @/回复奶妹的消息；频道回复一律带
//! `allowed_mentions`（只允许 @ 成员，不允许 here / everyone）；除此之外的旧约束不变。

pub mod api;
pub mod bridge;
pub mod chat;
pub mod commands;
pub mod config;
pub mod events;
pub mod handlers;
pub mod kb;
pub mod naimei;
