//! 复刻 CPython 3.12 `json.loads`（C 扫描器）的接受 / 拒绝行为与解析结果。
//!
//! 为什么不用 serde_json：Python 与 serde_json 在这些地方不一样，而请求体 / 上游事件流的判定
//! 必须和旧网关一致——
//! - `NaN` / `Infinity` / `-Infinity` 字面量：`json.loads` 默认接受（可选拒绝）；
//! - 孤立代理对 `"\ud800"`：Python 接受并得到含孤立代理的 str；
//! - 超过 u64 的整数：Python 是任意精度 int（上限 4300 位，超过抛 ValueError）；
//! - `-0` 是整数 0；`1e999` 是 `inf`（不报错）；
//! - bytes 输入按 `detect_encoding` 识别 UTF-8 / UTF-8-SIG / UTF-16 / UTF-32，并以 `surrogatepass` 解码；
//! - 容器嵌套达到约 1 万层时 C 扫描器抛 RecursionError。
//!
//! 解析器是迭代的（显式栈），任意深度都不会爆栈。

use std::borrow::Cow;

use crate::pyobj::{DictKey, Obj, PyDict};

/// C 扫描器在第几层容器嵌套抛 RecursionError（CPython 3.12.14 实测：9997 层通过，9998 层抛）。
/// 不同补丁版本 / 平台可能不同，但这只影响错误「类型」（RecursionError 还是 ValueError），
/// 不影响接受与否——超过 32 层本来就被 `_too_deep` 拒掉。
pub const RECURSION_DEPTH: usize = 9998;
/// `int(str)` 的位数上限（`sys.int_info.default_max_str_digits`）。
pub const MAX_INT_DIGITS: usize = 4300;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum JsonError {
    /// `json.JSONDecodeError`
    #[error("JSON 语法错误")]
    Syntax,
    /// `parse_constant` 抛的 `ValueError("不支持 NaN")`
    #[error("不支持 {0}")]
    Constant(&'static str),
    /// 整数超过 4300 位的 `ValueError`
    #[error("整数位数超过上限")]
    IntTooLong,
    /// bytes 解码失败（`UnicodeDecodeError`）
    #[error("编码无效")]
    Decode,
    /// 嵌套过深触发的 `RecursionError`
    #[error("嵌套过深（递归上限）")]
    Recursion,
    /// `body._too_deep` 抛的 `ValueError("JSON 嵌套过深")`
    #[error("JSON 嵌套过深")]
    TooDeep,
}

impl JsonError {
    /// 对应的 Python 异常类型名（Python `json.loads` 抛出的异常类型）。
    pub fn py_type(&self) -> &'static str {
        match self {
            JsonError::Syntax => "JSONDecodeError",
            JsonError::Constant(_) | JsonError::IntTooLong | JsonError::TooDeep => "ValueError",
            JsonError::Decode => "UnicodeDecodeError",
            JsonError::Recursion => "RecursionError",
        }
    }
}

/// 解析选项。
#[derive(Debug, Clone, Copy)]
pub struct Options {
    /// `false` 对应 `parse_constant=_reject_constant`：遇到 NaN / Infinity 抛 `Constant`。
    pub allow_constants: bool,
    /// 容器嵌套超过这一层就不再构建子树（只校验语法、丢弃内容），防止恶意深层请求体吃内存；
    /// 调用方应看 `Loaded::max_depth` 判断是否超限。`None` = 全部构建。
    pub max_build_depth: Option<usize>,
}

impl Options {
    /// 等价于 `json.loads(raw)`。
    pub const LOADS: Options = Options {
        allow_constants: true,
        max_build_depth: None,
    };
}

/// 解析结果。
#[derive(Debug)]
pub struct Loaded {
    pub value: Obj,
    /// 最深的容器嵌套层数（根容器 = 1，标量 = 0）。
    pub max_depth: usize,
    /// 文档里出现过孤立代理（已替换成 U+FFFD，Rust 的 String 装不下代理码位）。
    pub lone_surrogate: bool,
    /// 文档里出现过超出 `i64` / `u64` 范围的整数。
    pub big_int: bool,
    /// 文档里出现过溢出成 ±inf 的浮点（如 `1e999`）。
    pub float_overflow: bool,
}

/// `json.loads(bytes)`
pub fn loads_bytes(raw: &[u8], opts: Options) -> Result<Loaded, JsonError> {
    let text = decode_bytes(raw)?;
    parse_text(&text, opts)
}

/// `json.loads(str)`：以 U+FEFF 开头直接报错（与 bytes 路径不同）。
pub fn loads_str(s: &str, opts: Options) -> Result<Loaded, JsonError> {
    if s.starts_with('\u{feff}') {
        return Err(JsonError::Syntax);
    }
    parse_text(s.as_bytes(), opts)
}

// ---------------------------------------------------------------- bytes 解码

#[derive(Clone, Copy, PartialEq, Eq)]
enum Enc {
    Utf8,
    Utf8Sig,
    Utf16,
    Utf16Le,
    Utf16Be,
    Utf32,
    Utf32Le,
    Utf32Be,
}

/// `json.detect_encoding`
fn detect_encoding(b: &[u8]) -> Enc {
    if b.starts_with(&[0xff, 0xfe, 0, 0]) || b.starts_with(&[0, 0, 0xfe, 0xff]) {
        return Enc::Utf32;
    }
    if b.starts_with(&[0xff, 0xfe]) || b.starts_with(&[0xfe, 0xff]) {
        return Enc::Utf16;
    }
    if b.starts_with(&[0xef, 0xbb, 0xbf]) {
        return Enc::Utf8Sig;
    }
    if b.len() >= 4 {
        if b[0] == 0 {
            return if b[1] != 0 {
                Enc::Utf16Be
            } else {
                Enc::Utf32Be
            };
        }
        if b[1] == 0 {
            return if b[2] != 0 || b[3] != 0 {
                Enc::Utf16Le
            } else {
                Enc::Utf32Le
            };
        }
    } else if b.len() == 2 {
        if b[0] == 0 {
            return Enc::Utf16Be;
        }
        if b[1] == 0 {
            return Enc::Utf16Le;
        }
    }
    Enc::Utf8
}

/// 把码点（含孤立代理）追加成「广义 UTF-8」：代理码位按 3 字节编码（与 `surrogatepass` 的字节形式一致）。
fn push_cp(out: &mut Vec<u8>, cp: u32) {
    match char::from_u32(cp) {
        Some(c) => {
            let mut b = [0u8; 4];
            out.extend_from_slice(c.encode_utf8(&mut b).as_bytes());
        }
        None => {
            // 0xD800..=0xDFFF
            out.push(0xe0 | (cp >> 12) as u8);
            out.push(0x80 | ((cp >> 6) & 0x3f) as u8);
            out.push(0x80 | (cp & 0x3f) as u8);
        }
    }
}

/// UTF-8 校验，额外接受 `surrogatepass` 允许的代理码位 3 字节编码（ED A0..BF 80..BF）。
fn valid_surrogatepass_utf8(mut b: &[u8]) -> bool {
    loop {
        match std::str::from_utf8(b) {
            Ok(_) => return true,
            Err(e) => {
                let rest = &b[e.valid_up_to()..];
                if e.error_len().is_some()
                    && rest.len() >= 3
                    && rest[0] == 0xed
                    && (0xa0..=0xbf).contains(&rest[1])
                    && (0x80..=0xbf).contains(&rest[2])
                {
                    b = &rest[3..];
                } else {
                    return false;
                }
            }
        }
    }
}

fn decode_utf16(b: &[u8], big_endian: bool) -> Result<Vec<u8>, JsonError> {
    if !b.len().is_multiple_of(2) {
        return Err(JsonError::Decode);
    }
    let unit = |i: usize| -> u32 {
        let (x, y) = (u32::from(b[i]), u32::from(b[i + 1]));
        if big_endian {
            (x << 8) | y
        } else {
            (y << 8) | x
        }
    };
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let u = unit(i);
        i += 2;
        if (0xd800..0xdc00).contains(&u) && i < b.len() {
            let lo = unit(i);
            if (0xdc00..0xe000).contains(&lo) {
                i += 2;
                push_cp(&mut out, 0x10000 + ((u - 0xd800) << 10) + (lo - 0xdc00));
                continue;
            }
        }
        push_cp(&mut out, u);
    }
    Ok(out)
}

fn decode_utf32(b: &[u8], big_endian: bool) -> Result<Vec<u8>, JsonError> {
    if !b.len().is_multiple_of(4) {
        return Err(JsonError::Decode);
    }
    let mut out = Vec::with_capacity(b.len());
    for q in b.as_chunks::<4>().0 {
        let cp = if big_endian {
            u32::from_be_bytes(*q)
        } else {
            u32::from_le_bytes(*q)
        };
        if cp > 0x10ffff {
            return Err(JsonError::Decode);
        }
        push_cp(&mut out, cp);
    }
    Ok(out)
}

/// `raw.decode(detect_encoding(raw), "surrogatepass")`，结果是广义 UTF-8 字节。
fn decode_bytes(raw: &[u8]) -> Result<Cow<'_, [u8]>, JsonError> {
    match detect_encoding(raw) {
        Enc::Utf8 => {
            if valid_surrogatepass_utf8(raw) {
                Ok(Cow::Borrowed(raw))
            } else {
                Err(JsonError::Decode)
            }
        }
        Enc::Utf8Sig => {
            let body = &raw[3..];
            if valid_surrogatepass_utf8(body) {
                Ok(Cow::Borrowed(body))
            } else {
                Err(JsonError::Decode)
            }
        }
        Enc::Utf16 => {
            let big = raw[0] == 0xfe;
            decode_utf16(&raw[2..], big).map(Cow::Owned)
        }
        Enc::Utf16Le => decode_utf16(raw, false).map(Cow::Owned),
        Enc::Utf16Be => decode_utf16(raw, true).map(Cow::Owned),
        Enc::Utf32 => {
            let big = raw[0] == 0;
            decode_utf32(&raw[4..], big).map(Cow::Owned)
        }
        Enc::Utf32Le => decode_utf32(raw, false).map(Cow::Owned),
        Enc::Utf32Be => decode_utf32(raw, true).map(Cow::Owned),
    }
}

// ---------------------------------------------------------------- 解析

fn parse_text(text: &[u8], opts: Options) -> Result<Loaded, JsonError> {
    let mut p = Parser {
        s: text,
        pos: 0,
        opts,
        max_depth: 0,
        lone_surrogate: false,
        big_int: false,
        float_overflow: false,
    };
    let value = p.document()?;
    Ok(Loaded {
        value,
        max_depth: p.max_depth,
        lone_surrogate: p.lone_surrogate,
        big_int: p.big_int,
        float_overflow: p.float_overflow,
    })
}

struct Parser<'a> {
    s: &'a [u8],
    pos: usize,
    opts: Options,
    max_depth: usize,
    lone_surrogate: bool,
    big_int: bool,
    float_overflow: bool,
}

enum Frame {
    Arr(Vec<Obj>),
    Map {
        dict: PyDict,
        key: String,
    },
    /// 超过 `max_build_depth` 的容器：只校验，不构建。
    SkipArr,
    SkipMap,
}

impl Parser<'_> {
    fn peek(&self) -> Option<u8> {
        self.s.get(self.pos).copied()
    }

    /// JSON 空白只有这四个。
    fn skip_ws(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.pos += 1;
        }
    }

    /// 进入第 `depth` 层容器：记录最大深度、检查递归上限；返回是否需要构建子树。
    fn enter(&mut self, depth: usize) -> Result<bool, JsonError> {
        if depth >= RECURSION_DEPTH {
            return Err(JsonError::Recursion);
        }
        self.max_depth = self.max_depth.max(depth);
        Ok(self.opts.max_build_depth.is_none_or(|m| depth <= m))
    }

    fn document(&mut self) -> Result<Obj, JsonError> {
        let mut stack: Vec<Frame> = Vec::new();
        self.skip_ws();
        loop {
            // 1. 读一个值（容器只处理开头，内容留给下一轮循环）
            let mut value = match self.peek() {
                None => return Err(JsonError::Syntax),
                Some(b'{') => {
                    self.pos += 1;
                    let build = self.enter(stack.len() + 1)?;
                    self.skip_ws();
                    if self.peek() == Some(b'}') {
                        self.pos += 1;
                        if build {
                            Obj::Dict(PyDict::new())
                        } else {
                            Obj::None
                        }
                    } else {
                        let key = self.key()?;
                        stack.push(if build {
                            Frame::Map {
                                dict: PyDict::new(),
                                key,
                            }
                        } else {
                            Frame::SkipMap
                        });
                        continue;
                    }
                }
                Some(b'[') => {
                    self.pos += 1;
                    let build = self.enter(stack.len() + 1)?;
                    self.skip_ws();
                    if self.peek() == Some(b']') {
                        self.pos += 1;
                        if build {
                            Obj::List(Vec::new())
                        } else {
                            Obj::None
                        }
                    } else {
                        stack.push(if build {
                            Frame::Arr(Vec::new())
                        } else {
                            Frame::SkipArr
                        });
                        continue;
                    }
                }
                Some(b'"') => Obj::Str(self.string()?),
                Some(_) => self.scalar()?,
            };

            // 2. 把值挂到父容器上；容器闭合就继续往上挂
            loop {
                let Some(top) = stack.last_mut() else {
                    self.skip_ws();
                    return if self.pos == self.s.len() {
                        Ok(value)
                    } else {
                        Err(JsonError::Syntax) // Extra data
                    };
                };
                match top {
                    Frame::Arr(items) => items.push(value),
                    Frame::Map { dict, key } => {
                        dict.insert(DictKey::Str(std::mem::take(key)), value);
                    }
                    Frame::SkipArr | Frame::SkipMap => drop(value),
                }
                let is_map = matches!(top, Frame::Map { .. } | Frame::SkipMap);
                self.skip_ws();
                match (self.peek(), is_map) {
                    (Some(b','), false) => {
                        self.pos += 1;
                        self.skip_ws();
                        break; // 去读下一个元素
                    }
                    (Some(b','), true) => {
                        self.pos += 1;
                        self.skip_ws();
                        let new_key = self.key()?;
                        if let Some(Frame::Map { key, .. }) = stack.last_mut() {
                            *key = new_key;
                        }
                        break;
                    }
                    (Some(b']'), false) | (Some(b'}'), true) => {
                        self.pos += 1;
                        value = match stack.pop() {
                            Some(Frame::Arr(items)) => Obj::List(items),
                            Some(Frame::Map { dict, .. }) => Obj::Dict(dict),
                            _ => Obj::None,
                        };
                    }
                    _ => return Err(JsonError::Syntax),
                }
            }
        }
    }

    /// 读对象键：`"key" ws : ws`，光标停在值的第一个字符。
    fn key(&mut self) -> Result<String, JsonError> {
        if self.peek() != Some(b'"') {
            return Err(JsonError::Syntax);
        }
        let k = self.string()?;
        self.skip_ws();
        if self.peek() != Some(b':') {
            return Err(JsonError::Syntax);
        }
        self.pos += 1;
        self.skip_ws();
        Ok(k)
    }

    /// 字面量 / 常量 / 数字（容器与字符串之外的值）。
    fn scalar(&mut self) -> Result<Obj, JsonError> {
        let rest = &self.s[self.pos..];
        let constant = |name: &'static str, v: f64| -> Option<(&'static str, f64)> {
            rest.starts_with(name.as_bytes()).then_some((name, v))
        };
        match rest[0] {
            b'n' if rest.starts_with(b"null") => {
                self.pos += 4;
                return Ok(Obj::None);
            }
            b't' if rest.starts_with(b"true") => {
                self.pos += 4;
                return Ok(Obj::Bool(true));
            }
            b'f' if rest.starts_with(b"false") => {
                self.pos += 5;
                return Ok(Obj::Bool(false));
            }
            b'N' | b'I' | b'-' => {
                let hit = match rest[0] {
                    b'N' => constant("NaN", f64::NAN),
                    b'I' => constant("Infinity", f64::INFINITY),
                    _ => constant("-Infinity", f64::NEG_INFINITY),
                };
                if let Some((name, v)) = hit {
                    if !self.opts.allow_constants {
                        return Err(JsonError::Constant(name));
                    }
                    self.pos += name.len();
                    return Ok(Obj::Float(v));
                }
            }
            _ => {}
        }
        self.number()
    }

    /// `-?(0|[1-9]\d*)(\.\d+)?([eE][-+]?\d+)?`，与 C 扫描器一样：`1.` / `1e` 只吃到 `1`，剩下的由外层判错。
    fn number(&mut self) -> Result<Obj, JsonError> {
        let s = self.s;
        let start = self.pos;
        let mut i = start;
        let neg = s[i] == b'-';
        if neg {
            i += 1;
        }
        match s.get(i) {
            Some(b'1'..=b'9') => {
                i += 1;
                while s.get(i).is_some_and(u8::is_ascii_digit) {
                    i += 1;
                }
            }
            Some(b'0') => i += 1,
            _ => return Err(JsonError::Syntax),
        }
        let int_end = i;
        let mut is_float = false;
        if s.get(i) == Some(&b'.') && s.get(i + 1).is_some_and(u8::is_ascii_digit) {
            is_float = true;
            i += 2;
            while s.get(i).is_some_and(u8::is_ascii_digit) {
                i += 1;
            }
        }
        if matches!(s.get(i), Some(b'e' | b'E')) {
            let mut j = i + 1;
            if matches!(s.get(j), Some(b'+' | b'-')) {
                j += 1;
            }
            let digits_from = j;
            while s.get(j).is_some_and(u8::is_ascii_digit) {
                j += 1;
            }
            if j > digits_from {
                is_float = true;
                i = j;
            }
        }
        self.pos = i;
        // 数字全是 ASCII
        let text = std::str::from_utf8(&s[start..i]).map_err(|_| JsonError::Syntax)?;
        if is_float {
            // 溢出得到 ±inf，与 Python 的 float("1e999") 一致
            let v: f64 = text.parse().map_err(|_| JsonError::Syntax)?;
            self.float_overflow |= v.is_infinite();
            return Ok(Obj::Float(v));
        }
        let digits = &text[usize::from(neg)..int_end - start];
        if digits.len() > MAX_INT_DIGITS {
            return Err(JsonError::IntTooLong);
        }
        let n = Obj::int_from_decimal(neg, digits);
        self.big_int |= match &n {
            Obj::Int(i) => *i < i128::from(i64::MIN) || *i > i128::from(u64::MAX),
            _ => true,
        };
        Ok(n)
    }

    fn hex4(&self, at: usize) -> Result<u32, JsonError> {
        let h = self.s.get(at..at + 4).ok_or(JsonError::Syntax)?;
        let mut v = 0u32;
        for &b in h {
            let d = (b as char).to_digit(16).ok_or(JsonError::Syntax)?;
            v = (v << 4) | d;
        }
        Ok(v)
    }

    /// 光标在开头的 `"` 上；返回后光标在结尾 `"` 之后。严格模式：字符串里不许有未转义的控制字符。
    fn string(&mut self) -> Result<String, JsonError> {
        let s = self.s;
        self.pos += 1;
        let mut i = self.pos;
        let mut run_start = i;
        let mut buf: Vec<u8> = Vec::new();
        let mut escaped = false;
        loop {
            let Some(&b) = s.get(i) else {
                return Err(JsonError::Syntax); // Unterminated string
            };
            match b {
                b'"' => {
                    self.pos = i + 1;
                    return Ok(if escaped {
                        buf.extend_from_slice(&s[run_start..i]);
                        self.lossy_text(&buf)
                    } else {
                        self.lossy_text(&s[run_start..i])
                    });
                }
                b'\\' => {
                    escaped = true;
                    buf.extend_from_slice(&s[run_start..i]);
                    i += 1;
                    let Some(&e) = s.get(i) else {
                        return Err(JsonError::Syntax);
                    };
                    i += 1;
                    match e {
                        b'"' => buf.push(b'"'),
                        b'\\' => buf.push(b'\\'),
                        b'/' => buf.push(b'/'),
                        b'b' => buf.push(0x08),
                        b'f' => buf.push(0x0c),
                        b'n' => buf.push(b'\n'),
                        b'r' => buf.push(b'\r'),
                        b't' => buf.push(b'\t'),
                        b'u' => {
                            let mut cp = self.hex4(i)?;
                            i += 4;
                            if (0xd800..0xdc00).contains(&cp) && s[i..].starts_with(b"\\u") {
                                // 高代理后紧跟 \uXXXX：是低代理就合成，否则高代理孤立、第二个转义留给下一轮
                                let lo = self.hex4(i + 2)?;
                                if (0xdc00..0xe000).contains(&lo) {
                                    cp = 0x10000 + ((cp - 0xd800) << 10) + (lo - 0xdc00);
                                    i += 6;
                                }
                            }
                            match char::from_u32(cp) {
                                Some(c) => {
                                    let mut tmp = [0u8; 4];
                                    buf.extend_from_slice(c.encode_utf8(&mut tmp).as_bytes());
                                }
                                None => {
                                    // 孤立代理：Rust 的 String 装不下，换成 U+FFFD 并留标记
                                    self.lone_surrogate = true;
                                    buf.extend_from_slice("\u{fffd}".as_bytes());
                                }
                            }
                        }
                        _ => return Err(JsonError::Syntax), // Invalid \escape
                    }
                    run_start = i;
                }
                0x00..=0x1f => return Err(JsonError::Syntax), // Invalid control character
                _ => i += 1,
            }
        }
    }

    /// 广义 UTF-8（可能含 `surrogatepass` 解码出的孤立代理 ED A0..BF xx）→ String，代理换成 U+FFFD。
    fn lossy_text(&mut self, bytes: &[u8]) -> String {
        match std::str::from_utf8(bytes) {
            Ok(s) => s.to_string(),
            Err(_) => {
                let mut out = String::with_capacity(bytes.len());
                let mut rest = bytes;
                loop {
                    match std::str::from_utf8(rest) {
                        Ok(s) => {
                            out.push_str(s);
                            break;
                        }
                        Err(e) => {
                            let (good, bad) = rest.split_at(e.valid_up_to());
                            out.push_str(std::str::from_utf8(good).unwrap_or(""));
                            self.lone_surrogate = true;
                            out.push('\u{fffd}');
                            // 入口已校验过，这里只会是 3 字节的代理编码
                            rest = bad.get(3..).unwrap_or(&[]);
                        }
                    }
                }
                out
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skip_mode_keeps_error_precedence() {
        let opts = Options {
            allow_constants: false,
            max_build_depth: Some(2),
        };
        let l = loads_bytes(b"[[[[1]]]]", opts).unwrap();
        assert_eq!(l.max_depth, 4);
        assert_eq!(
            loads_bytes(b"[[[[1]]]] x", opts).unwrap_err(),
            JsonError::Syntax
        );
    }
}
