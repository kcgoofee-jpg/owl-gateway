//! 请求体 JSON 解析（`app/body.py` 的纯逻辑部分）：`parse_json` / `_reject_constant` / `_too_deep` /
//! `parse_json_body` 里的判定。读取请求体的异步函数（`read_bounded_body` / `read_json_body`）不在这里。
//!
//! 接受 / 拒绝的判定（实现在 `pyloads`）：拒绝 NaN / Infinity、嵌套超过 `MAX_JSON_DEPTH` 层的容器、
//! 非法 UTF-8 / 语法错误；重复键以最后一个为准（位置保持第一次出现的位置）；`1.0` 与 `1` 类型分开；
//! `-0` 是整数 0；bytes 输入识别 BOM / UTF-16 / UTF-32。
//!
//! **不可约差异（Rust 的 `serde_json::Value` 装不下，但请求照样被接受）**——都记在 [`Lossy`] 里，
//! 由调用方决定是否改成拒绝（标记是「文档级」的：只要文档里出现过就为真，哪怕那个值后来被重复键覆盖了）：
//! - 字符串里的孤立代理（`"\ud800"`）：Python 得到含代理的 str，这里换成 U+FFFD；
//! - 超出 `i64` / `u64` 的整数：Python 保留任意精度，这里取最近的 f64；
//! - 溢出的浮点（`1e999` → `inf`）：这里取 ±`f64::MAX`。
//!
//! 这三类只会出现在刻意构造的请求里。放行的风险：`1e999`（inf）和孤立代理要等到序列化转发给上游时才抛错
//! （多半变成 500，而不是 400）；超大整数则带着失真的值被原样转发。
//! 建议网关层看到 `Lossy::any()` 就直接按「请求体不是合法 JSON」拒绝。
//!
//! 另一处只影响错误「类型」而不影响接受与否的边界：容器嵌套接近递归上限（约 1 万层，见
//! `pyloads::RECURSION_DEPTH`）时，报的是递归超限错误而不是普通 JSON 语法错误。

use serde_json::{Map, Number, Value};

use crate::pyloads::{self, JsonError, Loaded, Options};
use crate::pyobj::{DictKey, Obj};

/// `body.MAX_JSON_DEPTH`
pub const MAX_JSON_DEPTH: usize = 32;
/// 超过这个大小的请求体放到阻塞线程里解析（是否 `spawn_blocking` 由调用方决定）。
pub const THREAD_PARSE_BYTES: usize = 256 * 1024;

/// `parse_json_body` 的两种 400 文案。
pub const MSG_INVALID_JSON: &str =
    "请求格式错误，未处理。请更新或更换客户端。开发者参考：请求体不是合法 JSON";
pub const MSG_NOT_OBJECT: &str =
    "请求格式错误，未处理。请更新或更换客户端。开发者参考：请求体必须是 JSON 对象";

/// 解析成功但丢失了信息的情形（见模块文档）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Lossy {
    pub lone_surrogate: bool,
    pub big_int: bool,
    pub float_overflow: bool,
}

impl Lossy {
    pub fn any(&self) -> bool {
        self.lone_surrogate || self.big_int || self.float_overflow
    }
}

#[derive(Debug, Clone)]
pub struct Parsed {
    pub value: Value,
    pub lossy: Lossy,
}

fn strict_options() -> Options {
    Options {
        allow_constants: false,
        // 超过 32 层的内容不构建（反正会被拒绝），避免恶意深层请求体吃内存
        max_build_depth: Some(MAX_JSON_DEPTH),
    }
}

fn finish(loaded: Loaded) -> Result<Parsed, JsonError> {
    if loaded.max_depth > MAX_JSON_DEPTH {
        return Err(JsonError::TooDeep);
    }
    let lossy = Lossy {
        lone_surrogate: loaded.lone_surrogate,
        big_int: loaded.big_int,
        float_overflow: loaded.float_overflow,
    };
    let value = to_value(&loaded.value);
    Ok(Parsed { value, lossy })
}

/// `body.parse_json(raw)`（raw 是 bytes）。
pub fn parse_json(raw: &[u8]) -> Result<Parsed, JsonError> {
    finish(pyloads::loads_bytes(raw, strict_options())?)
}

fn int_to_value(i: i128) -> Value {
    if let Ok(v) = i64::try_from(i) {
        return Value::Number(v.into());
    }
    if let Ok(v) = u64::try_from(i) {
        return Value::Number(v.into());
    }
    float_value(i as f64)
}

fn float_value(f: f64) -> Value {
    let f = if f.is_finite() {
        f
    } else if f > 0.0 {
        f64::MAX
    } else {
        f64::MIN
    };
    Value::Number(Number::from_f64(f).expect("有限浮点"))
}

/// 深度 ≤ 32，递归安全。
fn to_value(o: &Obj) -> Value {
    match o {
        Obj::None => Value::Null,
        Obj::Bool(b) => Value::Bool(*b),
        Obj::Int(i) => int_to_value(*i),
        Obj::BigInt { neg, digits } => {
            // 超过 f64 范围的大整数：Python 的 float(int) 抛 OverflowError，这里夹到 ±MAX
            let f: f64 = format!("{}{digits}", if *neg { "-" } else { "" })
                .parse()
                .unwrap_or(f64::INFINITY);
            float_value(if f.is_infinite() && *neg {
                f64::NEG_INFINITY
            } else {
                f
            })
        }
        Obj::Float(f) => float_value(*f),
        Obj::Str(s) => Value::String(s.clone()),
        Obj::List(items) => Value::Array(items.iter().map(to_value).collect()),
        Obj::Dict(d) => {
            let mut m = Map::new();
            for (k, v) in d.iter() {
                let DictKey::Str(k) = k else { continue };
                m.insert(k.clone(), to_value(v));
            }
            Value::Object(m)
        }
        // JSON 解析不会产生这些
        Obj::Bytes(_) | Obj::Ext { .. } | Obj::Timestamp { .. } => Value::Null,
    }
}

/// `parse_json_body` 的失败：对应 HTTP 400 的两种文案。
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum BodyJsonError {
    #[error("请求格式错误，未处理。请更新或更换客户端。开发者参考：请求体不是合法 JSON")]
    Invalid,
    #[error("请求格式错误，未处理。请更新或更换客户端。开发者参考：请求体必须是 JSON 对象")]
    NotObject,
}

impl BodyJsonError {
    /// 400 响应里的 detail 文案。
    pub fn message(&self) -> &'static str {
        match self {
            BodyJsonError::Invalid => MSG_INVALID_JSON,
            BodyJsonError::NotObject => MSG_NOT_OBJECT,
        }
    }
}

/// `parse_json_body(raw)` 的判定部分：解析失败 → 400「不是合法 JSON」；不是对象 → 400「必须是 JSON 对象」。
pub fn parse_json_body(raw: &[u8]) -> Result<(Map<String, Value>, Lossy), BodyJsonError> {
    let parsed = parse_json(raw).map_err(|_| BodyJsonError::Invalid)?;
    match parsed.value {
        Value::Object(m) => Ok((m, parsed.lossy)),
        _ => Err(BodyJsonError::NotObject),
    }
}
