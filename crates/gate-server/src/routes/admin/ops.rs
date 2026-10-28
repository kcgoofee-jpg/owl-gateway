//! 后台「运营 / 监控」接口：
//!
//! - [`monitor`]：状态码分布、总览、运行状态、上游表现；
//! - [`upstream`]：「上游」页的聚合读数、上游 Token 增删改、Anlas 核对与未知预留；
//! - [`account_stats`]：每账号统计的内部实现与站付 V5 到期建议；
//! - [`limits`]：预算、排队与冷却、账号保护；
//! - [`onboarding`]：自助领 Key、全局功能、生成记录、首页加入按钮；
//! - [`system`]：站点公告、测试告警、Discord 机器人；
//! - [`manual`]：运维（Claude）手动改临时设置，改动和「手动（Claude）」记录在同一个事务里。
//!
//! 已删除功能的残留接口不移植：`/quota-shadow`、`/shadow`（调度影子模式）、`/anlas-pool`、`/share-guard`
//! （新前端都不再调用）。动态额度参数和定时任务（`quota-algo`、`jobs`）在 `routes/admin/jobs.rs`。

mod account_stats;
mod limits;
mod manual;
mod monitor;
mod onboarding;
mod system;
mod upstream;
mod upstream_details;
mod upstream_information;

use crate::state::AppState;
use axum::Router;
use serde_json::{Value, json};

pub fn router() -> Router<AppState> {
    Router::new()
        .merge(monitor::router())
        .merge(upstream::router())
        .merge(upstream_information::router())
        .merge(upstream_details::router())
        .merge(account_stats::router())
        .merge(limits::router())
        .merge(manual::router())
        .merge(onboarding::router())
        .merge(system::router())
}

/// 上游健康：最近 10 分钟上游调用成败 + 全站图片冷却剩余秒数。
fn upstream_health(state: &AppState) -> Value {
    let now = state.now();
    let (status, recent, failed) = state.health.status(now);
    json!({
        "status": status,
        "recent": recent,
        "failed": failed,
        "image_cooldown_seconds": state.cooldown.remaining(now),
    })
}
