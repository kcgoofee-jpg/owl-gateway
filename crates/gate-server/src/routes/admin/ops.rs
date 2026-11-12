//! 后台「运营 / 监控」接口：
//!
//! - [`monitor`]：状态码分布、总览、运行状态、上游表现；
//! - [`upstream`]：「上游」页的聚合读数、上游 Token 增删改、Anlas 核对与未知预留；
//! - [`account_stats`]：每账号统计的内部实现与站付 V5 到期建议；
//! - [`limits`]：预算、排队与冷却、账号保护；
//! - [`onboarding`]：自助领 Key、全局功能、生成记录、首页加入按钮；
//! - [`system`]：维护模式、测试告警、Discord 机器人；
//! - [`manual`]：运维（Claude）手动改临时设置，改动和「手动（Claude）」记录在同一个事务里。

mod account_stats;
mod limits;
mod manual;
mod monitor;
mod onboarding;
mod system;
mod upstream;
mod upstream_details;
mod upstream_information;
mod upstream_share;

use crate::state::AppState;
use axum::Router;
use serde_json::{Value, json};

/// 账号卡片和运行保护共用的出图节奏：周期、上次 429、排队、并发锁、排队上限。
pub(super) fn pacing_fields(
    pacing: Option<&gate_upstream::client::Pacing>,
    queue_timeout: f64,
) -> Value {
    let cycle = pacing.map(|item| item.cycle).unwrap_or(0.0);
    let last_429 = pacing.map(|item| item.last_429).unwrap_or(0.0);
    json!({
        "cycle": gate_core::primitives::round(cycle, 1),
        "last_429": if last_429 > 0.0 { gate_core::values::float(last_429) } else { Value::Null },
        "waiting": pacing.map(|item| item.waiting).unwrap_or(0),
        "locked_today": pacing.map(|item| item.locked_today).unwrap_or(0),
        "queue_cap": gate_upstream::pace::realistic_queue(queue_timeout, cycle)
            .map_or(Value::Null, |n| json!(n)),
    })
}

pub(super) fn merge_json(row: &mut Value, extra: &Value) {
    let (Some(row), Some(extra)) = (row.as_object_mut(), extra.as_object()) else {
        return;
    };
    for (key, value) in extra {
        row.insert(key.clone(), value.clone());
    }
}

pub fn router() -> Router<AppState> {
    Router::new()
        .merge(monitor::router())
        .merge(upstream::router())
        .merge(upstream_share::router())
        .merge(upstream_information::router())
        .merge(upstream_details::router())
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
