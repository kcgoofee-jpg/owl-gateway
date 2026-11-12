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

/// 后台日志把上游失败归成几类，芯片和行上用短名，不把请求形状当成原因。
pub const ISSUE_LABELS: &[(&str, &str)] = &[
    ("up_503", "出图通道接不了"),
    ("up_5xx", "出图通道故障"),
    ("up_422", "参数不被接受"),
    ("up_429", "上游限流"),
    ("up_401", "上游登录失效"),
    ("up_402", "上游额度不足"),
    ("up_403", "上游拒绝"),
    ("up_404", "上游不支持"),
    ("up_413", "请求过大"),
    ("up_other", "上游出错"),
];

pub fn is_issue_code(code: &str) -> bool {
    ISSUE_LABELS.iter().any(|(c, _)| *c == code)
}

/// `usage_log.up_status` 优先；没记状态码时再从 `upstream 503` 这种说明里认。
pub fn issue_code(up_status: i64, detail: &str) -> &'static str {
    let status = if up_status > 0 {
        up_status
    } else {
        status_from_detail(detail)
    };
    match status {
        503 => "up_503",
        s if (500..600).contains(&s) => "up_5xx",
        400 | 422 => "up_422",
        429 => "up_429",
        401 => "up_401",
        402 => "up_402",
        403 => "up_403",
        404 => "up_404",
        413 => "up_413",
        _ => "up_other",
    }
}

pub fn issue_label(code: &str) -> &'static str {
    ISSUE_LABELS
        .iter()
        .find(|(c, _)| *c == code)
        .map_or("上游出错", |(_, label)| *label)
}

/// 给站长看的第二行：上游点名的字段，或「上游 503」。请求形状不放这里。
pub fn issue_hint(up_status: i64, detail: &str) -> String {
    if let Some((_, msg)) = detail.split_once('：') {
        let msg = primitives::strip(msg);
        if !msg.is_empty() {
            return primitives::take_chars(msg, 140).to_string();
        }
    }
    let status = if up_status > 0 {
        up_status
    } else {
        status_from_detail(detail)
    };
    if status > 0 {
        format!("上游 {status}")
    } else {
        String::new()
    }
}

fn status_from_detail(detail: &str) -> i64 {
    let Some(rest) = detail.strip_prefix("upstream ") else {
        return 0;
    };
    rest.chars()
        .take_while(|c| c.is_ascii_digit())
        .collect::<String>()
        .parse()
        .unwrap_or(0)
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

    #[test]
    fn classifies_upstream_failures() {
        assert_eq!(issue_code(503, ""), "up_503");
        assert_eq!(issue_code(500, ""), "up_5xx");
        assert_eq!(issue_code(422, ""), "up_422");
        assert_eq!(issue_code(400, ""), "up_422");
        assert_eq!(
            issue_code(0, "upstream 503（nai-diffusion-4-5-curated · 1024x1024）"),
            "up_503"
        );
        assert_eq!(issue_code(0, "network down"), "up_other");
        assert_eq!(issue_label("up_503"), "出图通道接不了");
        assert_eq!(
            issue_hint(422, "upstream 422：未知字段：foo。"),
            "未知字段：foo。"
        );
        assert_eq!(
            issue_hint(503, "upstream 503（nai-diffusion-4-5-curated · 1024x1024）"),
            "上游 503"
        );
        assert!(is_issue_code("up_422"));
        assert!(!is_issue_code("queue_full"));
    }
}
