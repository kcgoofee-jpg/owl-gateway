//! 路由表。按接口分组：公开页面与状态、成员接口（出图 / 工具 / 补全 / 查询自己的额度）、成员页、机器人桥接、后台、状态页。

pub mod admin;
mod image;
mod me;
mod member;
mod pages;
pub(crate) mod public;
mod registration;
pub mod status;

use crate::AppState;
use crate::web::PUBLIC_CSP;
use axum::Router;
use axum::body::Body;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};

pub fn router(state: AppState) -> Router {
    Router::new()
        .merge(pages::router())
        .merge(image::router())
        .merge(status::router())
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
        .with_state(state)
}

/// 没有的路径：浏览器拿到 HTML 404（带站名与回首页 / 状态页的链接），
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
<p><a href="/status">查看服务状态</a></p>
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
    fn html_404_has_no_inline_style_and_links_home_and_status() {
        // 与首页共用 CSP（style-src 'self'）：页面里不能有 style 属性 / <style>
        assert!(!NOT_FOUND_HTML.contains("style="));
        assert!(!NOT_FOUND_HTML.contains("<style"));
        assert!(!NOT_FOUND_HTML.contains("<script"));
        assert!(NOT_FOUND_HTML.contains("猫头鹰公益站"));
        assert!(NOT_FOUND_HTML.contains(r#"href="/""#));
        assert!(NOT_FOUND_HTML.contains(r#"href="/status""#));
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
