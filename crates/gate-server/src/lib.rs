//! 网关应用：把各模块组装成一个 axum Router。`main.rs` 只负责命令行与进程生命周期。

pub mod accounting;
pub mod action_log;
pub mod admission;
pub mod alerts;
pub mod allowance;
pub mod asn;
pub mod audit;
pub mod auth;
pub mod ban_link;
pub mod body;
pub mod bugs;
pub mod client_hints;
pub mod defaults;
pub mod env;
pub mod error;
pub mod exporter;
pub mod feedback;
pub mod fsutil;
pub mod guard;
pub mod hooks;
pub mod jobs;
pub mod key;
pub mod live;
pub mod media;
pub mod middleware;
pub mod multipart;
pub mod ops;
pub mod reconciliation;
pub mod ref_images;
pub mod registration;
pub mod reqctx;
pub mod runtime;
pub mod session;
pub mod state;
pub mod time;
pub mod v5_medium;
pub mod web;

pub mod routes;

pub use state::AppState;

use axum::Router;
use axum::middleware::{from_fn, from_fn_with_state};
use tower::Layer;

/// 组装完整应用（含路由之前的 URI 改写）。返回可直接交给 `axum::serve` 的服务。
pub fn app(
    state: AppState,
) -> tower::util::MapRequest<Router, fn(axum::extract::Request) -> axum::extract::Request> {
    let stats = state.status_stats.clone();
    let cors = middleware::cors(&state.env.cors_origins);
    let mut router = routes::router(state.clone())
        .layer(from_fn(middleware::admin_no_store))
        .layer(from_fn_with_state(state, accounting::error_ledger))
        .layer(from_fn_with_state(stats, middleware::status_stats))
        .layer(from_fn(middleware::request_context));
    // 最外层（与旧版 add_middleware 的顺序一致）：浏览器里的网页客户端直连网关要靠它
    if let Some(cors) = cors {
        router = router.layer(cors);
    }
    tower::util::MapRequestLayer::new(middleware::strip_v1_alias as fn(_) -> _).layer(router)
}
