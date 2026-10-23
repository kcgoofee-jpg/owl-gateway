//! 对外公开的上游错误文案（移植自 `app/upstream_errors.py`）：只描述失败，不带任何上游响应内容。
//!
//! 上游的 JSON / HTML 可能含账号信息、URL 或回显的凭据，所以按状态码挑本地文案，而不是尝试脱敏。

pub fn upstream_error_message(status: i64) -> String {
    let message = match status {
        400 | 422 => "NovelAI 未接受本次参数，未扣费。请检查提示词和客户端设置。",
        401 => "本站 NovelAI 账号登录失效，未扣费。请稍后重试。",
        402 => "上游账号额度不足，未扣费。请稍后再试。",
        403 => "NovelAI 拒绝了本次请求，未扣费。请稍后重试。",
        404 => "NovelAI 暂不支持该功能或模型，未扣费。请稍后重试或更换模型。",
        413 => "请求内容过大，未扣费。请缩小图片或减少参考图。",
        429 => "NovelAI 正在限流，未扣费。请稍后重试。",
        s if s >= 500 => "NovelAI 暂时故障，未扣费。请稍后重试。",
        _ => "NovelAI 请求失败，未扣费。请稍后重试。",
    };
    format!("{message}状态码 {status}。")
}
