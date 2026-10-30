//! 全局中间件（外 → 内）：
//! 1. `/v1/ai/*`、`/v1/user/*` 去掉多余的 `/v1`（成员常把 OpenAI 的 Base URL 填进 NovelAI 客户端）——必须在路由之前；
//! 2. 请求上下文：生成请求编号、写 `X-Request-Id`、计时、客户端名；
//! 3. `/admin/api/*` 一律 `Cache-Control: no-store`（后台 JSON 含敏感凭据和账号信息）。

use crate::reqctx::ReqCtx;
use axum::body::Body;
use axum::extract::Request;
use axum::http::{HeaderValue, Uri};
use axum::middleware::Next;
use axum::response::Response;

/// 第 1 步：URI 改写（用 `tower::util::MapRequestLayer` 套在 Router 外面）。
pub fn strip_v1_alias(mut req: Request<Body>) -> Request<Body> {
    let path = req.uri().path();
    if path.starts_with("/v1/ai/") || path.starts_with("/v1/user/") {
        let rest = &req
            .uri()
            .path_and_query()
            .map(|p| p.as_str())
            .unwrap_or(path)[3..];
        if let Ok(pq) = rest.parse() {
            let mut parts = req.uri().clone().into_parts();
            parts.path_and_query = Some(pq);
            if let Ok(uri) = Uri::from_parts(parts) {
                *req.uri_mut() = uri;
            }
        }
    }
    req
}

/// 请求头的文本值（取不到或不是合法文本就是空串）。
fn header_text(headers: &axum::http::HeaderMap, name: &str) -> String {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string()
}

/// 第 2 步：请求上下文（请求编号、计时、来源）。不「识别」客户端（按 UA 猜不准），只原样记下 User-Agent 和来源 IP 供站长筛查。
pub async fn request_context(mut req: Request, next: Next) -> Response {
    let ctx = ReqCtx::new();
    let peer = req
        .extensions()
        .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
        .map(|c| c.0.ip().to_string())
        .unwrap_or_default();
    let xff = header_text(req.headers(), "x-forwarded-for");
    let mut ua = header_text(req.headers(), "user-agent");
    // 本站生图页（官网镜像）会在桥接层带上这个头：日志里直接标成 workbench，方便成员和站长辨认来源
    if let Ok(kind) = header_text(req.headers(), "x-owl-client").parse::<String>()
        && !kind.trim().is_empty()
    {
        ua = format!("{} ({})", kind.trim(), ua);
    }
    ctx.set_origin(&peer, &xff, &ua);
    ctx.set_hints(crate::client_hints::from_headers(req.headers()));
    req.extensions_mut().insert(ctx.clone());
    let rid = ctx.rid();
    let mut resp = ctx.clone().scope(next.run(req)).await;
    // 500 响应自己已经带了编号：不重复
    if !resp.headers().contains_key("x-request-id")
        && let Ok(v) = HeaderValue::from_str(&rid)
    {
        resp.headers_mut().insert("x-request-id", v);
    }
    resp
}

/// 第 3 步：后台接口禁止缓存。
pub async fn admin_no_store(req: Request, next: Next) -> Response {
    let is_admin_api = req.uri().path().starts_with("/admin/api/");
    let mut resp = next.run(req).await;
    if is_admin_api {
        let h = resp.headers_mut();
        h.insert("cache-control", HeaderValue::from_static("no-store"));
        h.insert(
            "x-content-type-options",
            HeaderValue::from_static("nosniff"),
        );
    }
    resp
}

/// 跨域（允许任意来源 `CORS_ORIGINS=*`、任意方法与头，不带凭据）。浏览器里的网页客户端直连网关要靠它。
/// 预检请求原样回显请求的头：通配符 `*` 不包括 `Authorization`（Fetch 规范），而成员接口要带 Key。
pub fn cors(origins: &[String]) -> Option<tower_http::cors::CorsLayer> {
    use axum::http::Method;
    use tower_http::cors::{AllowHeaders, AllowOrigin, CorsLayer};
    if origins.is_empty() {
        return None;
    }
    let allow = if origins.iter().any(|o| o == "*") {
        AllowOrigin::any()
    } else {
        AllowOrigin::list(origins.iter().filter_map(|o| HeaderValue::from_str(o).ok()))
    };
    Some(
        CorsLayer::new()
            .allow_origin(allow)
            .allow_methods([
                Method::DELETE,
                Method::GET,
                Method::HEAD,
                Method::OPTIONS,
                Method::PATCH,
                Method::POST,
                Method::PUT,
            ])
            .allow_headers(AllowHeaders::mirror_request())
            .expose_headers([
                axum::http::HeaderName::from_static("retry-after"),
                axum::http::HeaderName::from_static("x-request-id"),
            ])
            .max_age(std::time::Duration::from_secs(600)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v1_alias() {
        let req = Request::builder()
            .uri("/v1/ai/generate-image?x=1")
            .body(Body::empty())
            .unwrap();
        assert_eq!(
            strip_v1_alias(req).uri().to_string(),
            "/ai/generate-image?x=1"
        );
        // 只改 /v1/ai/* 与 /v1/user/*；其它 /v1/* 原样（比如 /v1/me）
        let req = Request::builder()
            .uri("/v1/me")
            .body(Body::empty())
            .unwrap();
        assert_eq!(strip_v1_alias(req).uri().to_string(), "/v1/me");
        let req = Request::builder()
            .uri("/v1/user/subscription")
            .body(Body::empty())
            .unwrap();
        assert_eq!(strip_v1_alias(req).uri().path(), "/user/subscription");
    }
}
