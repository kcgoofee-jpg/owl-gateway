//! 对外公开的上游错误文案（移植自 `app/upstream_errors.py`）：只描述失败，不带任何上游响应内容。
//!
//! 上游的 JSON / HTML 可能含账号信息、URL 或回显的凭据，所以按状态码挑本地文案，而不是尝试脱敏。

use crate::primitives;
use serde_json::Value;

pub fn upstream_error_message(status: i64) -> String {
    let message = match status {
        400 | 422 => "NovelAI 未接受本次参数，未扣费。请检查提示词和客户端设置。",
        401 => "本站 NovelAI 账号登录失效，未扣费。请稍后重试。",
        402 => "上游账号额度不足，未扣费。请稍后再试。",
        403 => "本次请求被上游拒绝，未扣费。请稍后重试。",
        404 => "上游暂不支持该功能或模型，未扣费。请稍后重试或更换模型。",
        413 => "请求内容过大，未扣费。请缩小图片或减少参考图。",
        429 => "上游正在限流，未扣费。请稍后重试。",
        503 => "出图通道这会儿接不了（繁忙或维护），未扣费。请稍后重试。",
        s if s >= 500 => "出图通道暂时故障，未扣费。请稍后重试。",
        _ => "出图请求失败，未扣费。请稍后重试。",
    };
    format!("{message}状态码 {status}。")
}

/// 从上游 JSON 错误体取出短说明，给后台日志用。不是 JSON 或没有 `message`/`detail`/`error` 字符串就空。
pub fn json_error_message(body: &[u8]) -> String {
    let Ok(Value::Object(m)) = serde_json::from_slice::<Value>(body) else {
        return String::new();
    };
    for key in ["message", "detail", "error"] {
        let Some(Value::String(s)) = m.get(key) else {
            continue;
        };
        let s = s.trim();
        if !s.is_empty() {
            return primitives::take_chars(s, 200).to_string();
        }
    }
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_share_gateway_message() {
        let body = serde_json::to_vec(&serde_json::json!({
            "statusCode": 422,
            "message": "未知字段：foo。"
        }))
        .unwrap();
        assert_eq!(json_error_message(&body), "未知字段：foo。");
        assert!(json_error_message(b"<html>nope</html>").is_empty());
    }
}
