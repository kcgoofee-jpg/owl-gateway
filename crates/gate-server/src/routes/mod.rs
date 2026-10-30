//! 路由表。按接口分组：公开页面与状态、成员接口（出图 / 工具 / 补全 / 查询自己的额度）、成员页、机器人桥接、后台。

pub mod admin;
mod image;
mod me;
mod member;
mod pages;
pub(crate) mod public;
mod registration;

use crate::AppState;
use crate::web::PUBLIC_CSP;
use axum::Json;
use axum::Router;
use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, Method, Request, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use serde_json::json;

pub fn router(state: AppState) -> Router {
    Router::new()
        .merge(pages::router())
        .merge(image::router())
        .merge(public::router())
        .merge(me::router())
        .merge(registration::router())
        .merge(member::router())
        .merge(admin::router(state.clone()))
        // 没有的路径：浏览器（Accept: text/html）给一个简单的 404 页面，API / 客户端仍回 JSON
        .fallback(not_found)
        .method_not_allowed_fallback(|| async {
            crate::error::HttpError::new(405, "Method Not Allowed")
        })
        .layer(middleware::from_fn_with_state(
            state.clone(),
            maintenance_gate,
        ))
        .with_state(state)
}

/// 管理后台、静态资源与探针必须在维护时仍可用，以便恢复站点和检查容器。
fn maintenance_bypass(path: &str) -> bool {
    path == "/admin"
        || path.starts_with("/admin/")
        || path.starts_with("/_owl/")
        || matches!(
            path,
            "/favicon.svg" | "/favicon.ico" | "/healthz" | "/readyz" | "/livez"
        )
}

/// 「只有站长可用」维护模式下判断当前请求是不是站长本人（看网页登录的成员会话）。
async fn is_maintenance_owner(state: &AppState, headers: &axum::http::HeaderMap) -> bool {
    let owners = &state.env.maintenance_owner_ids;
    if owners.is_empty() {
        return false;
    }
    match crate::session::member_state(state, headers).await {
        Ok(Some((discord_id, _))) => owners.iter().any(|id| id == &discord_id),
        _ => false,
    }
}

async fn maintenance_gate(
    State(state): State<AppState>,
    request: Request<Body>,
    next: Next,
) -> Response {
    if maintenance_bypass(request.uri().path()) {
        return next.run(request).await;
    }
    // 维护开关读取失败时 fail closed：避免数据库异常时继续接受新请求。
    let mode = crate::ops::maintenance_mode(&state)
        .await
        .unwrap_or(crate::ops::Maintenance::All);
    match mode {
        crate::ops::Maintenance::Off => return next.run(request).await,
        crate::ops::Maintenance::All => {}
        crate::ops::Maintenance::OwnerOnly => {
            // 站长名单为空时等同于全关，避免误配置把站打开。
            if is_maintenance_owner(&state, request.headers()).await {
                return next.run(request).await;
            }
        }
    }
    if (request.method() == Method::GET || request.method() == Method::HEAD)
        && wants_html(request.headers())
    {
        return maintenance_page(request.method() == Method::HEAD);
    }
    let mut response = (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({"detail": "本站维护中，服务暂时暂停"})),
    )
        .into_response();
    response
        .headers_mut()
        .insert(header::RETRY_AFTER, HeaderValue::from_static("30"));
    response
}

/// 与 Caddy 的停机页保持同一深色卡片样式；页面无站内资源依赖，并定时重新检查维护状态。
const MAINTENANCE_HTML: &str = r#"<!doctype html><html lang="zh-CN"><head><meta charset="utf-8"><title>维护中 · 猫头鹰公益站</title><meta name="viewport" content="width=device-width,initial-scale=1"><meta http-equiv="refresh" content="30"><style>body{font-family:system-ui,-apple-system,sans-serif;background:#0f1117;color:#e6edf3;display:flex;align-items:center;justify-content:center;min-height:100vh;margin:0;padding:20px;box-sizing:border-box}.card{background:#161b22;border:1px solid #30363d;border-radius:12px;padding:32px;max-width:520px;width:100%;text-align:center;box-shadow:0 8px 24px rgba(0,0,0,.5)}h1{font-size:22px;margin:0 0 16px;color:#f0883e}p{color:#8b949e;line-height:1.6;font-size:14px;margin:0 0 16px}.badge{display:inline-block;padding:4px 10px;background:#f0883e22;color:#f0883e;border-radius:20px;font-size:12px;font-weight:600;margin-bottom:12px}</style></head><body><main class="card"><div class="badge">维护中</div><h1>猫头鹰公益站暂停服务</h1><p>站点正在维护，公共页面与服务接口暂不可用。维护完成后会自动恢复，请稍后重试。</p><p>页面每 30 秒自动检查一次。</p></main></body></html>"#;

fn maintenance_page(head_only: bool) -> Response {
    let body = if head_only {
        Body::empty()
    } else {
        Body::from(MAINTENANCE_HTML)
    };
    let mut response = Response::new(body);
    *response.status_mut() = StatusCode::OK;
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(header::RETRY_AFTER, HeaderValue::from_static("30"));
    headers.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    headers.insert("x-frame-options", HeaderValue::from_static("DENY"));
    headers.insert("referrer-policy", HeaderValue::from_static("no-referrer"));
    headers.insert(
        "content-security-policy",
        HeaderValue::from_static("default-src 'none'; style-src 'unsafe-inline'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'"),
    );
    response
}

/// 没有的路径：浏览器拿到 HTML 404（带站名与回首页的链接），
/// API / 客户端（`Accept` 不含 `text/html`）仍拿原来的 JSON `{"detail": …}`，文案不变。
async fn not_found(uri: axum::http::Uri, headers: HeaderMap) -> Response {
    if wants_html(&headers) {
        html_not_found()
    } else {
        crate::error::HttpError::new(404, not_found_message(uri.path())).into_response()
    }
}

/// `Accept` 里真的列了 `text/html` 才算浏览器请求。
/// 浏览器一定带 `text/html`；脚本客户端一般只带 `application/json`、`*/*` 或没有 Accept。
fn wants_html(headers: &HeaderMap) -> bool {
    headers
        .get(header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|accept| {
            accept.split(',').any(|part| {
                part.split(';')
                    .next()
                    .is_some_and(|mime| mime.trim().eq_ignore_ascii_case("text/html"))
            })
        })
}

/// 静态 HTML：不写 style 属性 / `<style>`（与首页同一个 CSP：`style-src 'self'`），
/// 用浏览器默认排版即可；`color-scheme` 让暗色系统下的系统默认样式也是暗的。
const NOT_FOUND_HTML: &str = r#"<!doctype html>
<html lang="zh-CN">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<meta name="color-scheme" content="light dark">
<title>页面不存在 · 猫头鹰公益站</title>
</head>
<body>
<main>
<h1>页面不存在</h1>
<p>这个地址没有内容，可能已经下线，或者链接写错了。</p>
<nav aria-label="站内导航">
<p><a href="/">回首页</a></p>
</nav>
</main>
</body>
</html>
"#;

fn html_not_found() -> Response {
    let mut resp = Response::new(Body::from(NOT_FOUND_HTML));
    *resp.status_mut() = StatusCode::NOT_FOUND;
    let h = resp.headers_mut();
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
    h.insert(
        "content-security-policy",
        HeaderValue::from_static(PUBLIC_CSP),
    );
    h.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    h.insert("x-frame-options", HeaderValue::from_static("DENY"));
    h.insert("referrer-policy", HeaderValue::from_static("no-referrer"));
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    resp
}

/// 没有的路径仍回 404；成员最常走错的两类说清原因，其余照旧「Not Found」。
/// - 客户端选了 OpenAI 格式：会去请求 `/v1/models`、`/v1/images/generations` 等，表现为「无可用模型」；
/// - 旧客户端还在调文本生成（10-15 已删）。
pub fn not_found_message(path: &str) -> &'static str {
    let p = path.trim_end_matches('/').to_ascii_lowercase();
    let text = ["/ai/generate", "/ai/generate-stream", "/ai/generate-voice"]
        .iter()
        .any(|tail| p.ends_with(tail));
    if text {
        return "本站只提供生图，文本生成和语音接口已停用";
    }
    let openai = p == "/v1"
        || p.starts_with("/v1/")
        || [
            "/models",
            "/chat/completions",
            "/completions",
            "/images/generations",
            "/images/edits",
        ]
        .iter()
        .any(|tail| p.ends_with(tail));
    if openai {
        return "本站是 NovelAI 格式的生图接口，不兼容 OpenAI 格式。请在客户端将 API 格式设为 NovelAI，地址只填站点根地址，不加 /v1，模型名手动填写，如 nai-diffusion-4-5-full。";
    }
    "Not Found"
}

#[cfg(test)]
mod tests {
    use super::*;

    fn accept(value: &'static str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(header::ACCEPT, HeaderValue::from_static(value));
        h
    }

    #[test]
    fn browser_accept_is_html() {
        assert!(wants_html(&accept(
            "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8"
        )));
        assert!(wants_html(&accept("text/html")));
        assert!(wants_html(&accept("TEXT/HTML;q=0.9, */*")));
        // 没有 Accept / 只要 JSON → 保持 JSON
        assert!(!wants_html(&HeaderMap::new()));
        assert!(!wants_html(&accept("application/json")));
        assert!(!wants_html(&accept("*/*")));
        assert!(!wants_html(&accept("text/plain")));
    }

    #[test]
    fn html_404_has_no_inline_style_and_links_home() {
        // 与首页共用 CSP（style-src 'self'）：页面里不能有 style 属性 / <style>
        assert!(!NOT_FOUND_HTML.contains("style="));
        assert!(!NOT_FOUND_HTML.contains("<style"));
        assert!(!NOT_FOUND_HTML.contains("<script"));
        assert!(NOT_FOUND_HTML.contains("猫头鹰公益站"));
        assert!(NOT_FOUND_HTML.contains(r#"href="/""#));
        let resp = html_not_found();
        assert_eq!(resp.status(), axum::http::StatusCode::NOT_FOUND);
        assert_eq!(resp.headers()["content-type"], "text/html; charset=utf-8");
        assert_eq!(resp.headers()["content-security-policy"], PUBLIC_CSP);
        assert_eq!(resp.headers()["x-content-type-options"], "nosniff");
    }

    #[test]
    fn openai_and_text_hints_are_kept() {
        assert!(not_found_message("/v1/models").contains("不兼容 OpenAI 格式"));
        assert!(not_found_message("/v1").contains("不兼容 OpenAI 格式"));
        assert!(not_found_message("/ai/generate").contains("文本生成和语音接口已停用"));
        assert_eq!(not_found_message("/nope"), "Not Found");
    }
}
