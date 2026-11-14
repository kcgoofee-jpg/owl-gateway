//! 猫头鹰公益站的纯领域逻辑。
//!
//! 这个 crate 不做 I/O、不依赖 tokio：所有函数都是「输入 → 输出」，
//! 用 `tests/snapshots/` 里的快照和手写规则测试守住行为。
//! 时间一律由调用方传入（Unix 秒，f64），不在这里读时钟。

pub mod actor;
pub mod allowance;
pub mod bot_config;
pub mod breaker;
pub mod clock;
pub mod discord_ua;
pub mod features;
pub mod guard;
pub mod json;
pub mod key_status;
pub mod landing_quota;
pub mod lane;
pub mod member_flags;
pub mod member_policy;
pub mod paid_v5;
pub mod policy;
pub mod primitives;
pub mod quota;
pub mod reasons;
pub mod share_detection;
pub mod site_flags;
pub mod snowflake;
pub mod upstream_errors;
pub mod upstream_policy;
pub mod v5_medium;
pub mod validation;
pub mod values;

pub mod client_views;
pub mod errors;
pub mod image_events;
pub mod json_body;
pub mod sse;

/// 「不限」的唯一写法。**0 一律表示 0**（没有 / 不允许 / 停用），任何地方都不得再把 0
/// 解释成「不限」「永久」「不设上限」。需要放开时填 [`UNLIMITED`]。
pub const UNLIMITED: i64 = 9999;

/// 这个值是不是「不限」。
pub const fn is_unlimited(value: i64) -> bool {
    value >= UNLIMITED
}
