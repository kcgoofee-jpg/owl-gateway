//! 请求体读取。
//! 先按 Content-Length / 实际字节数限长，再严格解析 JSON（拒绝 NaN / Infinity、嵌套超过 32 层）。

use crate::error::{GateError, HttpError};
use crate::multipart::{parse_image_multipart, read_bounded_body};
use axum::body::Body;
use axum::http::HeaderMap;
use gate_core::json_body::{BodyJsonError, THREAD_PARSE_BYTES, parse_json_body};
use serde_json::{Map, Value};

pub type JsonObject = Map<String, Value>;

fn parse(raw: &[u8]) -> Result<JsonObject, HttpError> {
    match parse_json_body(raw) {
        // 超出 i64 的整数 / 孤立代理 / 溢出的浮点：直接按非法 JSON 拒绝，不等到转发上游才失败
        Ok((_, lossy)) if lossy.any() => Err(HttpError::new(400, BodyJsonError::Invalid.message())),
        Ok((m, _)) => Ok(m),
        Err(e) => Err(HttpError::new(400, e.message())),
    }
}

/// `parse_json_body`：大请求体放到阻塞线程里解析，避免拖慢其他成员。
pub async fn parse_json_bytes(raw: bytes::Bytes) -> Result<JsonObject, HttpError> {
    if raw.len() > THREAD_PARSE_BYTES {
        tokio::task::spawn_blocking(move || parse(&raw))
            .await
            .map_err(|_| HttpError::new(500, "请求体解析失败"))?
    } else {
        parse(&raw)
    }
}

/// `read_json_body(request, limit)`（后台 / 成员页接口：错误是 `{"detail": ...}`）。
pub async fn read_json_body(
    headers: &HeaderMap,
    body: Body,
    limit: usize,
) -> Result<JsonObject, HttpError> {
    parse_json_bytes(read_bounded_body(headers, body, limit).await?).await
}

/// 成员接口版本（`main.read_json`）：同样的检查，错误换成 NovelAI 格式。
pub async fn read_json(
    headers: &HeaderMap,
    body: Body,
    limit: usize,
) -> Result<JsonObject, GateError> {
    read_json_body(headers, body, limit).await.map_err(to_gate)
}

/// 出图类请求体：JSON 或 Launcher 的 multipart（附件按图片字段引用展开成 Base64）。
pub async fn read_image_payload(
    headers: &HeaderMap,
    body: Body,
    limit: usize,
) -> Result<JsonObject, GateError> {
    let ct = crate::auth::header_str(headers, "content-type");
    if !ct.to_lowercase().starts_with("multipart/form-data") {
        return read_json(headers, body, limit).await;
    }
    let raw = read_bounded_body(headers, body, limit)
        .await
        .map_err(to_gate)?;
    tokio::task::spawn_blocking(move || parse_image_multipart(&ct, raw, limit))
        .await
        .map_err(|_| GateError::new(500, "请求体解析失败"))?
        .map_err(to_gate)
}

/// 请求体读不下来 / 格式不对：客户端的问题（上传中断单算），带上原因码。
fn to_gate(e: HttpError) -> GateError {
    use gate_core::reasons::{BODY_ABORT, PARAM};
    let detail = gate_core::pyval::py_str(&e.detail);
    let code = if e.status >= 500 {
        ""
    } else if detail == crate::multipart::MSG_INTERRUPTED {
        BODY_ABORT
    } else {
        PARAM
    };
    GateError::with_code(e.status, detail, code)
}

/// 25 MB：生图请求体（含 Base64 图片）的上限。
pub const IMAGE_BODY_LIMIT: usize = 25 * 1024 * 1024;
