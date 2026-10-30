//! 出图准入与额度预留。
//!
//! 每一步都是一个 RAII 守卫：被拒或客户端断开时自动归还。真正发往上游之后，守卫随上游操作一起
//! 移进后台任务（见 `routes::image::shielded`），保证「先记账、再释放额度预留和并发槽」。
//!
//! 每把 Key 的限速只有两层：这里的排队（同时生成 1 张，每人最多同时排 `queue_per_member` 张，默认 2）
//! 和每分钟次数（RPM，`auth::check_rpm`）。

mod budget;
mod queue;
mod quota;

pub use budget::{
    DispatchRequirements, Reserved, budget_snapshot, release_reservations, reserve_image_budget,
};
pub use queue::{
    Admission, Slots, acquire_slots, guard_hard_block, image_admission, queue_cap, usable_token_ids,
};
pub use quota::{member_anlas_cap_f64, quota_image_check};

/// 一次出图的估算消耗。
pub use queue::Est;
