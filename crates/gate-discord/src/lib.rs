//! 猫头鹰公益站的 Discord 机器人「奶妹」（`nai-bots naimei`）。
//!
//! 命令：`/register`（领 Key）、`/quota`（今日额度）、`/resetkey`（换 Key）、`/help`、`/反馈`（写几句话给站长），
//! 以及每分钟一次的心跳（后台据此显示在线状态）。管理员操作都在网页后台，机器人里没有管理员命令，
//! 也不回应频道里的 @；执法用的燕双鹰已删除（它什么都不做）。
//!
//! 重要约束（Discord 曾因 OAuth 突发 + 群发私信 + 机器人在 NSFW 论坛自动点赞给应用打过标记，错误码 680009）：
//! 命令只注册到一个服务器（公会级），回复一律 ephemeral（只有本人可见）；Intent 只有 GUILDS，
//! 没有任何特权 Intent；没有私信；没有论坛点赞 / AI 评论；`allowed_mentions` 不允许任何 @。

pub mod api;
pub mod bridge;
pub mod commands;
pub mod config;
pub mod events;
pub mod handlers;
pub mod naimei;
