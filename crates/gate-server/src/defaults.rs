//! 新建 Key 与自助领取的默认值：代码常量，只在这里定义一次。
//!
//! 数据库设置（后台「领 Key」页、动态额度）没设过时才用这里的值；环境变量 `DEFAULT_*` / `REGISTER_*`
//! 已删除（同一件事只留一个来源）。后台手动新建 Key、一键邀请 Key、`/register` 自助领取用的是同一份
//! 「新成员默认」：每日 V4.5 / V5 张数、模型范围、有效天数、默认功能、名额都读
//! [`crate::ops::registration_settings`]；后台新建 Key 的弹窗从 `/admin/api/ops` 取同一份（前端不另写默认值）。
//! 下面只放没有数据库设置的几项。

/// 自助领取的名额上限（后台「领 Key」改过的 `register_max_users` 优先；0 = 不限）。
pub const REGISTER_MAX_USERS: i64 = 10;
/// 新领的 Key 几天后过期，0 = 不过期（后台改过的 `register_expires_days` 优先）。
pub const REGISTER_EXPIRES_DAYS: i64 = 0;
/// Discord 账号注册满多少天才能自助领取（防刷号抢名额）；0 = 不限。
pub const REGISTER_MIN_ACCOUNT_DAYS: i64 = 7;
