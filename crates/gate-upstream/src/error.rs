//! 上游错误：回给成员的状态码与中文说明，外加记账要用的几个标记。

/// 上游错误：`status` 是回给成员的 HTTP 状态，`message` 是成员可见的中文说明。
#[derive(Debug, Clone, thiserror::Error)]
#[error("{status} {message}")]
pub struct UpstreamError {
    pub status: u16,
    pub message: String,
    /// 请求可能已经在上游开始生成（可能已扣费），调用方据此决定记账口径。
    pub billing_uncertain: bool,
    /// 非出图通道的传输层错误：旧版直接把 httpx 异常抛给路由（变成 500 / 502），这里单独标记。
    pub transport: bool,
    /// 拒绝原因码（账号保护挡住时带上 gate_core::reasons 的码；旧版靠 str 子类 Reason 携带）。
    pub code: String,
    /// 网关自己的回调（全站冷却 / V5 续费确认 / 额度复核）拒绝的：旧版是 GateError 原样穿过 `request()`，
    /// 状态码与原因码不能再按上游错误改写成 502。
    pub local: bool,
}

impl UpstreamError {
    pub fn new(status: u16, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
            billing_uncertain: false,
            transport: false,
            code: String::new(),
            local: false,
        }
    }

    /// 网关回调产生的拒绝（见 `local`）。
    pub fn local(status: u16, message: impl Into<String>, code: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            local: true,
            ..Self::new(status, message)
        }
    }

    pub(crate) fn uncertain(
        status: u16,
        message: impl Into<String>,
        billing_uncertain: bool,
    ) -> Self {
        Self {
            billing_uncertain,
            ..Self::new(status, message)
        }
    }

    pub(crate) fn internal(e: impl std::fmt::Display) -> Self {
        Self {
            transport: true,
            ..Self::new(500, format!("内部错误：{e}"))
        }
    }
}

/// 一次请求的传输层失败。
pub(crate) enum Transport {
    Http(wreq::Error),
    Timeout,
}

impl Transport {
    /// 是否发生在请求已经发出之后（读写中断 / 读超时 / 协议错误）——可能已扣费。
    /// 建连失败、建连超时、TLS 握手失败明确没有发出，不算。
    pub(crate) fn after_send(&self) -> bool {
        match self {
            Transport::Timeout => true,
            Transport::Http(e) => !e.is_connect() && !e.is_builder() && !e.is_redirect(),
        }
    }
}

impl std::fmt::Display for Transport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Transport::Http(e) => write!(f, "{e}"),
            Transport::Timeout => write!(f, "超时"),
        }
    }
}
