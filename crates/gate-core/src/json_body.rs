//! Bounded, standard UTF-8 JSON request parsing.
use serde_json::{Map, Value};
pub const MAX_JSON_DEPTH: usize = 32;
pub const THREAD_PARSE_BYTES: usize = 256 * 1024;
pub const MSG_INVALID_JSON: &str = "请求体不是合法的 UTF-8 JSON，或嵌套超过 32 层";
pub const MSG_NOT_OBJECT: &str = "请求体必须是 JSON 对象";
#[derive(Debug, Clone)]
pub struct Parsed {
    pub value: Value,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum BodyJsonError {
    #[error("请求体不是合法的 UTF-8 JSON，或嵌套超过 32 层")]
    Invalid,
    #[error("请求体必须是 JSON 对象")]
    NotObject,
}
impl BodyJsonError {
    pub fn message(&self) -> &'static str {
        match self {
            Self::Invalid => MSG_INVALID_JSON,
            Self::NotObject => MSG_NOT_OBJECT,
        }
    }
}
pub fn parse_json(raw: &[u8]) -> Result<Parsed, BodyJsonError> {
    let value: Value = serde_json::from_slice(raw).map_err(|_| BodyJsonError::Invalid)?;
    let mut stack = vec![(&value, 0)];
    while let Some((v, depth)) = stack.pop() {
        match v {
            Value::Array(a) => {
                if depth >= MAX_JSON_DEPTH {
                    return Err(BodyJsonError::Invalid);
                }
                stack.extend(a.iter().map(|v| (v, depth + 1)));
            }
            Value::Object(m) => {
                if depth >= MAX_JSON_DEPTH {
                    return Err(BodyJsonError::Invalid);
                }
                stack.extend(m.values().map(|v| (v, depth + 1)));
            }
            Value::Number(n) if !(n.is_i64() || n.is_u64() || n.is_f64()) => {
                return Err(BodyJsonError::Invalid);
            }
            _ => {}
        }
    }
    Ok(Parsed { value })
}
pub fn parse_json_body(raw: &[u8]) -> Result<Map<String, Value>, BodyJsonError> {
    match parse_json(raw)?.value {
        Value::Object(m) => Ok(m),
        _ => Err(BodyJsonError::NotObject),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn standard_json_and_depth() {
        assert!(parse_json_body(br#"{"n":1,"ok":true}"#).is_ok());
        for raw in [b"NaN".as_slice(), b"{\"n\":1e999}", b"\xff", b"\"\\ud800\""] {
            assert!(parse_json(raw).is_err());
        }
        assert!(parse_json(format!("{}0{}", "[".repeat(32), "]".repeat(32)).as_bytes()).is_ok());
        assert!(parse_json(format!("{}0{}", "[".repeat(33), "]".repeat(33)).as_bytes()).is_err());
        assert_eq!(parse_json_body(b"[]"), Err(BodyJsonError::NotObject));
    }
}
