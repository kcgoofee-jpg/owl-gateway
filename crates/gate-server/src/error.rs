//! 错误响应：三种固定格式，客户端依赖这些错误文案与字段布局，改动要先看客户端。
//!
//! 1. [`GateError`]（成员接口）：顶层 `statusCode` / `message` 与 NovelAI 官方错误格式一致，
//!    客户端直接显示这句话；消息统一带「猫头鹰公益站提醒：」前缀，免得成员以为是 NovelAI 官方出错。
//!    `{"statusCode": s, "message": m, "error": {"message": m, "status": s, "request_id": rid, "code": code, "execution_unknown": bool, "retryable": bool}}`
//! 3. 未预料的内部错误（500）：`{"error": {"message": "...错误编号 rid", "status": 500, "request_id": rid}}`，
//!    并带 `X-Request-Id`。
//!
//! 请求编号 rid 来自 [`crate::reqctx`]：错误在请求处理过程中产生时一定能拿到。

use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

pub const NOTICE_PREFIX: &str = "猫头鹰公益站提醒：";

/// 成员接口的拒绝 / 失败。`code` 是拒绝原因码（gate_core::reasons），用于用量日志统计。
#[derive(Debug, Clone)]
pub struct GateError {
    pub status: u16,
    pub message: String,
    pub code: String,
    pub billing_uncertain: bool,
    pub retryable: bool,
    /// 失败的原因在上游（上游返回 5xx / 连不上 / 返回内容无效）。给成员的响应不变，
    /// 但错误账本（[`crate::accounting::error_ledger`]）不把它记成 Bug：上游故障在用量日志和「上游」页里看得到，不是本站的缺陷。
    pub upstream: bool,
    /// `Retry-After` seconds for a 429 (None = [`retry_after_default`] by reason code).
    pub retry_after: Option<u64>,
}

/// Shown instead of an upstream NovelAI 402 relayed to a member.
pub const UPSTREAM_402_MESSAGE: &str = "上游账号额度不足，未扣费。请稍后再试。";

/// Status code a member client actually sees. Clients (notably 柏宝绘) replace a 402 with their own
/// "subscription required" text and auto-retry 429s, so a member never gets 402: a local refusal
/// becomes 403 and a relayed upstream 402 becomes 503 with [`UPSTREAM_402_MESSAGE`].
pub fn member_status(status: u16, upstream: bool) -> (u16, Option<&'static str>) {
    match status {
        402 if upstream => (503, Some(UPSTREAM_402_MESSAGE)),
        402 => (403, None),
        s => (s, None),
    }
}

/// Default `Retry-After` for a transient 429, by reason code.
pub fn retry_after_default(code: &str) -> u64 {
    use gate_core::reasons as r;
    match code {
        r::MEMBER_QUEUE_FULL | r::KEY_BUSY | r::UPSTREAM_LOCKED => 10,
        r::KEY_RATE | r::COOLDOWN | r::BREAKER => 30,
        _ => 15,
    }
}

impl GateError {
    pub fn new(status: u16, message: impl Into<String>) -> Self {
        Self::with_code(status, message, String::new())
    }

    pub fn with_code(status: u16, message: impl Into<String>, code: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
            code: code.into(),
            billing_uncertain: false,
            retryable: status == 429,
            upstream: false,
            retry_after: None,
        }
    }

    /// `Retry-After` seconds for a 429.
    pub fn retry_after(mut self, secs: u64) -> Self {
        self.retry_after = Some(secs.max(1));
        self
    }

    /// 标记「原因在上游」，见 [`GateError::upstream`]。
    pub fn from_upstream(mut self, yes: bool) -> Self {
        self.upstream = yes;
        self
    }

    pub fn uncertain(mut self, yes: bool) -> Self {
        self.billing_uncertain = yes;
        if yes {
            self.retryable = false;
        }
        self
    }

    pub fn retryable(mut self, yes: bool) -> Self {
        self.retryable = yes && !self.billing_uncertain;
        self
    }

    /// 带前缀的成员可见文案。
    pub fn display_message(&self) -> String {
        let base = if self.message.starts_with(NOTICE_PREFIX) {
            self.message.clone()
        } else {
            format!("{NOTICE_PREFIX}{}", self.message)
        };
        if self.billing_uncertain {
            let base = base.replace("未扣费", "尚未确认记账");
            format!("{base} 执行结果未知，请勿自动重发；请先在“我的请求”中核对。")
        } else {
            base
        }
    }

    pub fn response_body(&self) -> Value {
        let rid = crate::reqctx::current_rid();
        member_error_body(
            self.status,
            &self.message,
            &self.code,
            self.upstream,
            self.billing_uncertain,
            self.retryable,
            &rid,
        )
    }
}

pub fn member_error_body(
    status: u16,
    detail: &str,
    code: &str,
    upstream: bool,
    execution_unknown: bool,
    retryable: bool,
    rid: &str,
) -> Value {
    let (status, replaced) = member_status(status, upstream);
    let message = replaced.map_or_else(
        || {
            let base = if detail.starts_with(NOTICE_PREFIX) {
                detail.to_string()
            } else {
                format!("{NOTICE_PREFIX}{detail}")
            };
            if execution_unknown {
                let base = base.replace("未扣费", "尚未确认记账");
                format!("{base} 执行结果未知，请勿自动重发；请先在“我的请求”中核对。")
            } else {
                base
            }
        },
        |m| format!("{NOTICE_PREFIX}{m}"),
    );
    json!({
        "statusCode": status,
        "message": message,
        "error": {"message": message, "status": status, "request_id": rid,
            "code": code, "execution_unknown": execution_unknown,
            "retryable": retryable && !execution_unknown && status == 429},
    })
}

/// 附在响应扩展里，让外层中间件（记拒绝日志 / Bug 追踪）知道这是哪种错误。
#[derive(Debug, Clone)]
pub struct GateErrorInfo(pub GateError);

impl IntoResponse for GateError {
    fn into_response(self) -> Response {
        let (status, _) = member_status(self.status, self.upstream);
        let body = self.response_body();
        let mut resp = json_response(status, &body);
        if status == 429 {
            let secs = self
                .retry_after
                .unwrap_or_else(|| retry_after_default(&self.code));
            resp.headers_mut()
                .insert("retry-after", HeaderValue::from(secs));
        }
        resp.extensions_mut().insert(GateErrorInfo(self));
        resp
    }
}

#[derive(Debug, Clone)]
pub struct HttpError {
    pub status: u16,
    pub detail: Value,
}

impl HttpError {
    pub fn new(status: u16, detail: impl Into<String>) -> Self {
        Self {
            status,
            detail: Value::String(detail.into()),
        }
    }
}

/// 附在 `{"detail": ...}` 错误响应上（后台操作日志据此记「失败」）。
#[derive(Debug, Clone)]
pub struct HttpErrorInfo(pub HttpError);

impl IntoResponse for HttpError {
    fn into_response(self) -> Response {
        let mut resp = json_response(self.status, &json!({"detail": self.detail}));
        resp.extensions_mut().insert(HttpErrorInfo(self));
        resp
    }
}

/// 未预料的内部错误：记 Bug，回给成员一个可以报给站长的请求编号。
#[derive(Debug)]
pub struct InternalError(pub String);

impl<E: std::fmt::Display> From<E> for InternalError {
    fn from(e: E) -> Self {
        InternalError(e.to_string())
    }
}

/// 附在 500 响应上，交给外层中间件记 Bug。
#[derive(Debug, Clone)]
pub struct InternalErrorInfo(pub String);

impl IntoResponse for InternalError {
    fn into_response(self) -> Response {
        let rid = crate::reqctx::current_rid();
        let execution_unknown = crate::reqctx::current_execution_unknown();
        let message = if execution_unknown {
            format!(
                "{NOTICE_PREFIX}请求可能已到达上游，执行结果未知。请勿自动重发；先在“我的请求”中核对。反馈时请附上错误编号 {rid}"
            )
        } else {
            format!("{NOTICE_PREFIX}服务器内部错误，已自动记录。反馈时请附上错误编号 {rid}")
        };
        let body = json!({"message": message, "error": {
            "message": message,
            "status": 500,
            "request_id": rid,
            "code": if execution_unknown { "execution_unknown" } else { "internal_error" },
            "execution_unknown": execution_unknown,
            "retryable": false,
        }});
        let mut resp = json_response(500, &body);
        if !rid.is_empty()
            && let Ok(v) = HeaderValue::from_str(&rid)
        {
            resp.headers_mut().insert("x-request-id", v);
        }
        resp.extensions_mut().insert(InternalErrorInfo(self.0));
        resp
    }
}

/// 上游传输层失败（连不上 / 超时 / 读断）。对成员仍是 500，但原因在上游：
/// 在请求上下文里留个记号，错误账本据此不记 Bug（用量日志里照常有一条 error）。
pub fn upstream_transport(ctx: &crate::reqctx::ReqCtx, message: String) -> ApiError {
    ctx.mark_upstream_fault();
    ctx.mark_execution_unknown();
    InternalError(message).into()
}

pub fn json_response(status: u16, body: &Value) -> Response {
    let mut resp = Response::new(axum::body::Body::from(gate_core::json::wire(body)));
    *resp.status_mut() = StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    resp.headers_mut()
        .insert("content-type", HeaderValue::from_static("application/json"));
    resp
}

/// 普通 200 JSON 响应。
pub fn ok_json(body: &Value) -> Response {
    json_response(200, body)
}

/// 路由处理函数的统一返回类型。
pub type ApiResult = Result<Response, ApiError>;

/// 三种错误的合集，方便 `?`。
#[derive(Debug)]
pub enum ApiError {
    Gate(GateError),
    Http(HttpError),
    Internal(InternalError),
}

impl From<GateError> for ApiError {
    fn from(e: GateError) -> Self {
        ApiError::Gate(e)
    }
}
impl From<HttpError> for ApiError {
    fn from(e: HttpError) -> Self {
        ApiError::Http(e)
    }
}
impl From<InternalError> for ApiError {
    fn from(e: InternalError) -> Self {
        ApiError::Internal(e)
    }
}
impl From<gate_db::DbError> for ApiError {
    fn from(e: gate_db::DbError) -> Self {
        ApiError::Internal(InternalError(e.to_string()))
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        match self {
            ApiError::Gate(e) => e.into_response(),
            ApiError::Http(e) => e.into_response(),
            ApiError::Internal(e) => e.into_response(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use http_body_util::BodyExt;

    #[tokio::test]
    async fn members_never_see_402() {
        // Local refusal with 402 → 403, message kept; Retry-After only on 429
        let resp = GateError::with_code(402, "V5 暂停，未扣费。请使用 V4.5。", "v5_exhausted")
            .into_response();
        assert_eq!(resp.status(), 403);
        assert!(resp.headers().get("retry-after").is_none());
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let v: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["statusCode"], 403);
        assert_eq!(
            v["message"],
            "猫头鹰公益站提醒：V5 暂停，未扣费。请使用 V4.5。"
        );
        // Relayed upstream 402 → 503 with the fixed text
        let resp = GateError::with_code(402, "x", "")
            .from_upstream(true)
            .into_response();
        assert_eq!(resp.status(), 503);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let v: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(
            v["message"],
            format!("{NOTICE_PREFIX}{UPSTREAM_402_MESSAGE}")
        );
        // Transient 429 carries Retry-After
        let resp = GateError::with_code(429, "q", gate_core::reasons::QUEUE_FULL).into_response();
        assert_eq!(resp.headers()["retry-after"], "15");
        let resp = GateError::with_code(429, "c", gate_core::reasons::COOLDOWN)
            .retry_after(42)
            .into_response();
        assert_eq!(resp.headers()["retry-after"], "42");
        let resp = GateError::with_code(502, "unknown", "execution_unknown")
            .uncertain(true)
            .into_response();
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let v: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["error"]["execution_unknown"], true);
        assert_eq!(v["error"]["retryable"], false);
        assert_eq!(v["error"]["code"], "execution_unknown");
        assert!(v["message"].as_str().unwrap().contains("请勿自动重发"));
        let resp = GateError::with_code(403, "quota", "quota").into_response();
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let v: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["error"]["retryable"], false);
        // No member-facing code path constructs a 402 any more
        let root = concat!(env!("CARGO_MANIFEST_DIR"), "/..");
        for dir in ["gate-server/src", "gate-upstream/src", "gate-core/src"] {
            for f in walk(&format!("{root}/{dir}")) {
                let src = std::fs::read_to_string(&f).unwrap();
                for pat in ["with_code(402", "local(402", "::new(402"] {
                    assert!(!src.contains(pat) || f.ends_with("error.rs"), "{f}: {pat}");
                }
            }
        }
    }

    fn walk(dir: &str) -> Vec<String> {
        let mut out = Vec::new();
        for e in std::fs::read_dir(dir).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                out.extend(walk(&p.to_string_lossy()));
            } else if p.extension().is_some_and(|x| x == "rs") {
                out.push(p.to_string_lossy().into_owned());
            }
        }
        out
    }

    #[tokio::test]
    async fn gate_error_shape_matches_python() {
        let resp = GateError::with_code(
            429,
            "请求过于频繁，上限每分钟 10 次，请稍后重试。",
            gate_core::reasons::KEY_RATE,
        )
        .into_response();
        assert_eq!(resp.status(), 429);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(
            std::str::from_utf8(&body).unwrap(),
            r#"{"statusCode":429,"message":"猫头鹰公益站提醒：请求过于频繁，上限每分钟 10 次，请稍后重试。","error":{"message":"猫头鹰公益站提醒：请求过于频繁，上限每分钟 10 次，请稍后重试。","status":429,"request_id":"","code":"key_rate","execution_unknown":false,"retryable":true}}"#
        );
    }

    #[tokio::test]
    async fn http_error_is_detail() {
        let resp = HttpError::new(401, "未登录或会话已过期").into_response();
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(
            std::str::from_utf8(&body).unwrap(),
            r#"{"detail":"未登录或会话已过期"}"#
        );
    }
}
