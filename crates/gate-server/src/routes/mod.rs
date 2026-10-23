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
use axum::Router;

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
        // 与 Starlette 一致：没有的路径 / 不支持的方法回 JSON
        .fallback(|uri: axum::http::Uri| async move {
            crate::error::HttpError::new(404, not_found_message(uri.path()))
        })
        .method_not_allowed_fallback(|| async {
            crate::error::HttpError::new(405, "Method Not Allowed")
        })
        .with_state(state)
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
