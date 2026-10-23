//! 后台接口的请求解析：路径 / 查询参数用 axum 的提取器，写法统一成 `State + AdminSession + 提取器`。
//!
//! 解析失败回 422 + `{"detail": "<name> 必须是整数"}`（detail 是一句中文字符串）。这些错误都是 `HttpError`，
//! 所以操作日志会把写请求记成「失败」（不会被当成成功的操作）。提取器写在 `AdminSession` 后面：
//! 没登录的人先拿到 401，不会看到参数校验的结果。
//! 参数只认规范写法：整数是十进制（可带正负号），布尔是 `true` / `false`；同名参数取最后一个。

use crate::body::{JsonObject, read_json_body};
use crate::error::{ApiError, ApiResult, HttpError, InternalError};
use crate::state::AppState;
use axum::body::Body;
use axum::extract::path::ErrorKind;
use axum::extract::rejection::PathRejection;
use axum::extract::{FromRequestParts, Path, Query, RawPathParams, Request};
use axum::http::request::Parts;
use axum::response::Response;
use gate_core::{pyext, pyval};
use serde::de::DeserializeOwned;
use serde_json::Value;

pub type R = ApiResult;

/// 路径参数（`Path<T>` 的包装）：解析失败回 422「<name> 必须是整数」，不是 axum 默认的 400 纯文本。
pub struct AdminPath<T>(pub T);

impl<T: DeserializeOwned + Send> FromRequestParts<AppState> for AdminPath<T> {
    type Rejection = HttpError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, HttpError> {
        match Path::<T>::from_request_parts(parts, state).await {
            Ok(Path(v)) => Ok(Self(v)),
            Err(PathRejection::FailedToDeserializePathParams(e)) => {
                if let ErrorKind::ParseErrorAtKey { key, .. } = e.kind() {
                    return Err(HttpError::new(422, format!("{key} 必须是整数")));
                }
                // 单个参数解析失败时错误里没有名字：取第一个不是整数的路径参数
                let bad = RawPathParams::from_request_parts(parts, state)
                    .await
                    .ok()
                    .and_then(|raw| {
                        raw.iter()
                            .find(|(_, v)| v.parse::<i64>().is_err())
                            .map(|(k, _)| k.to_string())
                    });
                Err(match bad {
                    Some(k) => HttpError::new(422, format!("{k} 必须是整数")),
                    None => HttpError::new(422, "路径参数不合法"),
                })
            }
            Err(_) => Err(HttpError::new(422, "路径参数不合法")),
        }
    }
}

/// 查询串：同名参数取最后一个，`?all` 这种没有等号的值是空串。
/// `int` / `opt_int` / `flag` 取值，格式不对回 422「<name> 必须是整数 / 布尔值」。
pub struct AdminQuery(Vec<(String, String)>);

impl FromRequestParts<AppState> for AdminQuery {
    type Rejection = HttpError;

    async fn from_request_parts(parts: &mut Parts, _: &AppState) -> Result<Self, HttpError> {
        let pairs = Query::<Vec<(String, String)>>::try_from_uri(&parts.uri)
            .map(|q| q.0)
            .unwrap_or_default();
        Ok(Self(pairs))
    }
}

impl AdminQuery {
    pub fn get(&self, name: &str) -> Option<&str> {
        self.0
            .iter()
            .rev()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    /// 查询参数原文，没有就是空串。
    pub fn text(&self, name: &str) -> &str {
        self.get(name).unwrap_or("")
    }

    pub fn int(&self, name: &str, default: i64) -> Result<i64, HttpError> {
        Ok(self.opt_int(name)?.unwrap_or(default))
    }

    pub fn opt_int(&self, name: &str) -> Result<Option<i64>, HttpError> {
        self.get(name)
            .map(|raw| {
                raw.parse::<i64>()
                    .map_err(|_| HttpError::new(422, format!("{name} 必须是整数")))
            })
            .transpose()
    }

    pub fn flag(&self, name: &str) -> Result<bool, HttpError> {
        match self.get(name) {
            None => Ok(false),
            Some("true") => Ok(true),
            Some("false") => Ok(false),
            Some(_) => Err(HttpError::new(422, format!("{name} 必须是布尔值"))),
        }
    }
}

/// 读请求体（≤ 1 MiB，必须是 JSON 对象），同时把 `Parts` 还给处理函数。
pub async fn json_of(req: Request) -> Result<(Parts, JsonObject), HttpError> {
    let (parts, body) = req.into_parts();
    let body = read_json_body(&parts.headers, body, super::BODY_LIMIT).await?;
    Ok((parts, body))
}

// ----------------------------------------------------------------------------- 下载响应

/// `urllib.parse.quote(s)`（默认 safe="/"）：字母数字和 `_.-~/` 原样，其余按 UTF-8 百分号编码。
pub fn py_quote(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-' | b'~' | b'/') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// 二进制下载 / 图片响应。
pub fn bytes_response(data: Vec<u8>, media_type: &str, headers: &[(&str, String)]) -> Response {
    let mut resp = Response::new(Body::from(data));
    let h = resp.headers_mut();
    if let Ok(v) = axum::http::HeaderValue::from_str(media_type) {
        h.insert("content-type", v);
    }
    for (k, v) in headers {
        if let (Ok(name), Ok(val)) = (
            axum::http::HeaderName::from_bytes(k.as_bytes()),
            axum::http::HeaderValue::from_str(v),
        ) {
            h.insert(name, val);
        }
    }
    resp
}

/// 带状态码的错误（`{"detail": ...}`）。
pub fn http(status: u16, detail: impl Into<String>) -> ApiError {
    HttpError::new(status, detail).into()
}

/// `str(v or default)`：缺省、null、false、0、空串都取默认值。
pub fn py_str_or(v: Option<&Value>, default: &str) -> String {
    match v {
        Some(v) if pyval::truthy(v) => pyext::py_str(v),
        _ => default.to_string(),
    }
}

/// `str(body.get(key) or "")`
pub fn pstr(body: &JsonObject, key: &str) -> String {
    py_str_or(body.get(key), "")
}

/// `str(body.get(key) or default)`
pub fn pstr_or(body: &JsonObject, key: &str, default: &str) -> String {
    py_str_or(body.get(key), default)
}

/// 数据库 / 内部错误统一成 500（带请求编号）。
pub fn internal(msg: impl std::fmt::Display) -> ApiError {
    InternalError(msg.to_string()).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quote_keeps_slash_and_tilde() {
        assert_eq!(
            py_quote("owl-月见草-3.zip"),
            "owl-%E6%9C%88%E8%A7%81%E8%8D%89-3.zip"
        );
        assert_eq!(py_quote("a b/c~d"), "a%20b/c~d");
    }
}
