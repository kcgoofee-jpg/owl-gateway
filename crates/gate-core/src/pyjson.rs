//! 与 Python `json.dumps` 逐字节一致的序列化。
//!
//! 用在三处，字节格式固定，下游依赖确切的字节：
//! - 转发给上游的请求体（紧凑分隔符 `ensure_ascii=False, separators=(",", ":")`）——请求体本身就是指纹的一部分；
//! - 返回给客户端的 JSON（同上，紧凑分隔符）；
//! - 写进库 / 状态文件的 JSON（`ensure_ascii=False`，分隔符 `", "` / `": "`；非 ASCII 原样输出，不转义成 `\uXXXX`）。
//!
//! 浮点用 Python 的 repr（`1e-05`、`1e+16`），不是 Rust 的 `1e-5`。

use serde::Serialize;
use serde_json::Value;
use std::io::{self, Write};

/// Python `json.dumps` 的参数组合。
#[derive(Debug, Clone, Copy)]
pub struct Style {
    /// `separators=(",", ":")`；否则用默认的 `(", ", ": ")`。
    pub compact: bool,
}

/// httpx 发请求 / Starlette 回响应用的格式。
pub const WIRE: Style = Style { compact: true };

/// `json.dumps(x, ensure_ascii=False)`：默认分隔符，非 ASCII 原样输出（`site_settings` 与状态文件用它）。
pub const SPACED: Style = Style { compact: false };

struct PyFormatter {
    style: Style,
}

impl serde_json::ser::Formatter for PyFormatter {
    fn write_f64<W: ?Sized + Write>(&mut self, writer: &mut W, value: f64) -> io::Result<()> {
        writer.write_all(crate::pyc::float_repr(value).as_bytes())
    }

    fn write_f32<W: ?Sized + Write>(&mut self, writer: &mut W, value: f32) -> io::Result<()> {
        self.write_f64(writer, f64::from(value))
    }

    fn begin_array_value<W: ?Sized + Write>(
        &mut self,
        writer: &mut W,
        first: bool,
    ) -> io::Result<()> {
        if first {
            Ok(())
        } else if self.style.compact {
            writer.write_all(b",")
        } else {
            writer.write_all(b", ")
        }
    }

    fn begin_object_key<W: ?Sized + Write>(
        &mut self,
        writer: &mut W,
        first: bool,
    ) -> io::Result<()> {
        self.begin_array_value(writer, first)
    }

    fn begin_object_value<W: ?Sized + Write>(&mut self, writer: &mut W) -> io::Result<()> {
        if self.style.compact {
            writer.write_all(b":")
        } else {
            writer.write_all(b": ")
        }
    }
}

/// 序列化任意值（通常是 `serde_json::Value`）。值里不可能有 NaN / Infinity（`Value` 表示不了），
/// 与 `allow_nan=False` 的行为一致。
pub fn dumps_with<T: Serialize + ?Sized>(value: &T, style: Style) -> Vec<u8> {
    let mut out = Vec::with_capacity(128);
    let mut ser = serde_json::Serializer::with_formatter(&mut out, PyFormatter { style });
    value.serialize(&mut ser).expect("序列化到内存不会失败");
    out
}

/// `json.dumps(v, ensure_ascii=False)`，返回字符串。
/// 额度 / 自动驾驶把状态序列化成字符串写进 `site_settings`，下一轮再 `json.loads` 读回来；
/// 新旧两边要写出完全相同的字符串，所以分隔符是 `", "` / `": "`，浮点是 `repr(float)`。
/// 不支持 NaN / Infinity（`serde_json::Value` 放不下）。
pub fn dumps(v: &Value) -> String {
    String::from_utf8(dumps_with(v, SPACED)).expect("serde_json 只输出合法 UTF-8")
}

/// `json.loads(s)`：解析失败返回 None（Python 抛 `JSONDecodeError`）。
/// 与 Python 的差异：不接受 `NaN` / `Infinity` 字面量（旧代码从不写出它们）。
pub fn loads(s: &str) -> Option<Value> {
    serde_json::from_str(s).ok()
}

/// 线上格式（紧凑、保留 UTF-8）。
pub fn wire(value: &Value) -> Vec<u8> {
    dumps_with(value, WIRE)
}
