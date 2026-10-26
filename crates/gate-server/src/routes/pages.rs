//! 页面与静态资源：`/`、`/admin`（含 `/admin/*` 前端路由，`/admin/api/*` 除外）、`/_owl/*`、图标、健康检查。

use crate::AppState;
use crate::error::ok_json;
use crate::web::Page;
use axum::Router;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use serde_json::json;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/livez", get(livez))
        .route("/readyz", get(readyz))
        .route("/healthz", get(healthz))
        .route("/", get(landing))
        .route("/admin", get(admin))
        // Starlette 的 redirect_slashes：/admin/ → 307 /admin
        .route(
            "/admin/",
            get(|| async { axum::response::Redirect::temporary("/admin") }),
        )
        .route("/admin/{*rest}", get(admin_spa))
        .route("/_owl/{*path}", get(asset))
        .route("/favicon.svg", get(favicon))
        .route("/favicon.ico", get(favicon))
}

/// 进程存活探针不依赖数据库或上游。
async fn livez() -> Response {
    ok_json(&json!({"ok": true}))
}

/// 就绪要求数据库读写都成功；不触发任何上游探测。
async fn readyz(State(st): State<AppState>) -> Response {
    match st.db.health(HEALTH_DB_LIMIT).await {
        Ok(()) => ok_json(&json!({"ok": true})),
        Err(e) => (
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            axum::Json(json!({"ok": false, "reason": e.code()})),
        )
            .into_response(),
    }
}

/// `/healthz` 保持原有数据库读写检查，供现有部署脚本与容器探针继续使用。
/// 上游未被探测，故明确回报 unknown；token 已配置不能证明上游可用。
async fn healthz(State(st): State<AppState>) -> Response {
    match st.db.health(HEALTH_DB_LIMIT).await {
        Ok(()) => ok_json(&json!({"ok": true, "upstream": "unknown"})),
        Err(e) => (
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            axum::Json(json!({"ok": false, "reason": e.code(), "upstream": "unknown"})),
        )
            .into_response(),
    }
}

/// 每条路的时限。读写两条加起来最多 2 秒，要小于所有调用方的超时：cutover.sh 的 `curl -m 3`、
/// 容器内 `nai-gate healthcheck` 的 4 秒、Docker HEALTHCHECK 的 5 秒。正常时 `SELECT 1` 不到 1 毫秒。
const HEALTH_DB_LIMIT: std::time::Duration = std::time::Duration::from_secs(1);

/// 首页：HTML 里注入首屏数据（我 / 实时看板 / 站点状态，见 `public::landing_boot`），取不到就照常返回页面。
async fn landing(State(st): State<AppState>, headers: HeaderMap) -> Response {
    let boot = super::public::landing_boot(&st, &headers).await;
    st.assets.page(Page::Landing, boot.as_ref())
}

async fn admin(State(st): State<AppState>) -> Response {
    st.assets.page(Page::Admin, None)
}

async fn admin_spa(State(st): State<AppState>, Path(rest): Path<String>) -> Response {
    if rest.starts_with("api/") || rest == "api" {
        return (
            axum::http::StatusCode::NOT_FOUND,
            axum::Json(json!({"detail": "Not Found"})),
        )
            .into_response();
    }
    st.assets.page(Page::Admin, None)
}

async fn asset(State(st): State<AppState>, Path(path): Path<String>) -> Response {
    st.assets.asset(&path)
}

async fn favicon(State(st): State<AppState>) -> Response {
    let mut resp = st.assets.asset("favicon.svg");
    resp.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=86400"),
    );
    resp
}
