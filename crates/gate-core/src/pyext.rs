//! Python 动态语义的补充工具（`policy` / `upstream_policy` 共用）。
//!
//! 请求体是任意 JSON，旧 Python 代码直接对 dict 做 `int(x)`、`len(x)`、`str(x)`、`x.get(..)`；
//! 值不合适时抛 `ValueError` / `TypeError` / `AttributeError` / `OverflowError` / `ZeroDivisionError`，
//! 调用方（路由层）按异常类型决定回 400 还是 500，所以这里把异常类型和消息都复刻出来。
//!
//! 整数一律用 `i128`：Python 是任意精度，线上边长等字段已被 `normalize_image_request` 限在 65536 以内，
//! `i128` 足够覆盖夹具里所有「很大」的输入；溢出处按饱和处理（注释里会写明）。

use crate::{pyc, pyval};
use serde_json::{Map, Number, Value};
use std::sync::OnceLock;

pub type Obj = Map<String, Value>;

/// 复刻的 Python 异常。`type_name()` 与 Python `type(e).__name__` 一致，`message()` 与 `str(e)` 一致
/// （`OverflowError` 的消息按 Python 的随场景变化，只保证类型）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PyErr {
    Value(String),
    Type(String),
    Attribute(String),
    Overflow(String),
    ZeroDivision(String),
}

pub type PyResult<T> = Result<T, PyErr>;

impl PyErr {
    pub fn type_name(&self) -> &'static str {
        match self {
            PyErr::Value(_) => "ValueError",
            PyErr::Type(_) => "TypeError",
            PyErr::Attribute(_) => "AttributeError",
            PyErr::Overflow(_) => "OverflowError",
            PyErr::ZeroDivision(_) => "ZeroDivisionError",
        }
    }

    pub fn message(&self) -> &str {
        match self {
            PyErr::Value(m)
            | PyErr::Type(m)
            | PyErr::Attribute(m)
            | PyErr::Overflow(m)
            | PyErr::ZeroDivision(m) => m,
        }
    }

    pub fn value(msg: impl Into<String>) -> Self {
        PyErr::Value(msg.into())
    }
}

impl std::fmt::Display for PyErr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message())
    }
}

impl std::error::Error for PyErr {}

/// `type(v).__name__`（JSON 能表示的那几种）。
pub fn type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "NoneType",
        Value::Bool(_) => "bool",
        Value::Number(n) if n.is_f64() => "float",
        Value::Number(_) => "int",
        Value::String(_) => "str",
        Value::Array(_) => "list",
        Value::Object(_) => "dict",
    }
}

// ----------------------------------------------------------------------------- 数值判定

/// JSON 整数（i64 / u64）→ i128；浮点返回 None。
pub fn int_of_number(n: &Number) -> Option<i128> {
    if n.is_f64() {
        None
    } else if let Some(i) = n.as_i64() {
        Some(i128::from(i))
    } else {
        n.as_u64().map(i128::from)
    }
}

/// `isinstance(v, int)`（含 bool）。
pub fn int_like(v: &Value) -> Option<i128> {
    match v {
        Value::Bool(b) => Some(i128::from(*b)),
        Value::Number(n) => int_of_number(n),
        _ => None,
    }
}

/// `isinstance(v, int) and not isinstance(v, bool)`。
pub fn strict_int(v: &Value) -> Option<i128> {
    match v {
        Value::Number(n) => int_of_number(n),
        _ => None,
    }
}

/// `bool(d.get(k))`：键不存在 / None 都是假。
pub fn truthy_opt(v: Option<&Value>) -> bool {
    v.is_some_and(pyval::truthy)
}

/// `x == y` 中 x 是 JSON 数字、y 是整数字面量：`14.0 == 14` 为真，bool 不是 Number。
pub fn num_eq_int(v: Option<&Value>, lit: i128) -> bool {
    match v {
        Some(Value::Number(n)) => match int_of_number(n) {
            Some(i) => i == lit,
            None => n.as_f64().is_some_and(|f| f == lit as f64),
        },
        // Python 里 True == 1、False == 0
        Some(Value::Bool(b)) => i128::from(*b) == lit,
        _ => false,
    }
}

// ----------------------------------------------------------------------------- 字符类（Python 3.12 / Unicode 15.0）

/// `str.isdecimal()` / `re` 的 `\d`：Unicode Nd（数字表在 `pyc::DECIMAL_ZEROS`，按 Python 3.12 / Unicode 15.0 整理）。
pub fn is_nd(c: char) -> bool {
    pyc::decimal_value(c).is_some()
}

/// `str.isdecimal()`：非空且每个字符都是 Unicode 十进制数字（Nd）。
pub fn is_decimal(s: &str) -> bool {
    !s.is_empty() && s.chars().all(is_nd)
}

/// Nd 数字的数值（0..=9）。
pub fn nd_value(c: char) -> Option<u32> {
    pyc::decimal_value(c)
}

/// `str.isdigit()` 里 Numeric_Type=Digit 但不是十进制的字符区间（上标、带圈数字等）；
/// 由 Python 3.12.14 的 `unicodedata`（Unicode 15.0）生成。
const DIGIT_NOT_DECIMAL: [(u32, u32); 20] = [
    (178, 179),
    (185, 185),
    (4969, 4977),
    (6618, 6618),
    (8304, 8304),
    (8308, 8313),
    (8320, 8329),
    (9312, 9320),
    (9332, 9340),
    (9352, 9360),
    (9450, 9450),
    (9461, 9469),
    (9471, 9471),
    (10102, 10110),
    (10112, 10120),
    (10122, 10130),
    (68160, 68163),
    (69216, 69224),
    (69714, 69722),
    (127232, 127242),
];

/// `str.isdigit()`（单字符）。
pub fn is_digit(c: char) -> bool {
    let cp = c as u32;
    is_nd(c) || DIGIT_NOT_DECIMAL.iter().any(|&(a, b)| cp >= a && cp <= b)
}

/// Python `re` 里 `\d` 的字符类内部文本（Unicode 十进制数字，Python 3.12 的 Unicode 15.0 表）。
/// 不用 regex 自带的 `\d`：它跟的 Unicode 版本更新，会多认几种新文字的数字。
pub fn nd_class() -> &'static str {
    static S: OnceLock<String> = OnceLock::new();
    S.get_or_init(|| {
        pyc::DECIMAL_ZEROS
            .iter()
            .map(|z| format!("\\x{{{z:x}}}-\\x{{{:x}}}", z + 9))
            .collect()
    })
}

/// Python `re` 里 `\s` 的字符类内部文本（与 `str.isspace()` 同一个集合，含 \x1c-\x1f；
/// 和 [`pyc::is_space`] 逐码点一致，由单元测试守住）。
pub fn space_class() -> &'static str {
    "\\t\\n\\x0b\\x0c\\r\\x1c-\\x1f \\x{85}\\x{a0}\\x{1680}\\x{2000}-\\x{200a}\\x{2028}\\x{2029}\\x{202f}\\x{205f}\\x{3000}"
}

// ----------------------------------------------------------------------------- repr / str

/// 近似 `str.isprintable()`（单字符）：控制字符、常见格式字符、分隔符、私有区、非字符视为不可打印。
/// 未分配码点按可打印处理（Python 按 Cn 不可打印）——只影响 `repr` 里极罕见的字符。
fn is_printable(c: char) -> bool {
    let cp = c as u32;
    !(cp < 0x20
        || (0x7f..=0xa0).contains(&cp)
        || cp == 0xad
        || (0x600..=0x605).contains(&cp)
        || matches!(
            cp,
            0x61c | 0x6dd | 0x70f | 0x890 | 0x891 | 0x8e2 | 0x180e | 0xfeff
        )
        || cp == 0x1680
        || (0x2000..=0x200f).contains(&cp)
        || (0x2028..=0x202f).contains(&cp)
        || (0x205f..=0x206f).contains(&cp)
        || cp == 0x3000
        || (0xd800..=0xdfff).contains(&cp)
        || (0xe000..=0xf8ff).contains(&cp)
        || (0xfdd0..=0xfdef).contains(&cp)
        || (0xfff9..=0xfffb).contains(&cp)
        || (cp & 0xfffe) == 0xfffe
        || (0xe0000..=0xe007f).contains(&cp)
        || cp >= 0xf0000)
}

/// `repr(str)`。
pub fn repr_str(s: &str) -> String {
    let quote = if s.contains('\'') && !s.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::with_capacity(s.len() + 2);
    out.push(quote);
    for c in s.chars() {
        let cp = c as u32;
        match c {
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            _ if (0x20..0x7f).contains(&cp) => out.push(c),
            _ if is_printable(c) => out.push(c),
            _ if cp <= 0xff => out.push_str(&format!("\\x{cp:02x}")),
            _ if cp <= 0xffff => out.push_str(&format!("\\u{cp:04x}")),
            _ => out.push_str(&format!("\\U{cp:08x}")),
        }
    }
    out.push(quote);
    out
}

/// `repr(v)`：dict / list 用 Python 的写法（`{'a': 1}`、`['x', None]`）。
pub fn py_repr(v: &Value) -> String {
    match v {
        Value::Null => "None".into(),
        Value::Bool(b) => pyc::bool_str(*b).into(),
        Value::Number(n) => match int_of_number(n) {
            Some(i) => i.to_string(),
            None => pyc::float_repr(n.as_f64().unwrap_or(f64::NAN)),
        },
        Value::String(s) => repr_str(s),
        Value::Array(a) => format!("[{}]", a.iter().map(py_repr).collect::<Vec<_>>().join(", ")),
        Value::Object(o) => format!(
            "{{{}}}",
            o.iter()
                .map(|(k, x)| format!("{}: {}", repr_str(k), py_repr(x)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

/// `str(v)`：字符串原样，其余同 `repr`。
pub fn py_str(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => py_repr(other),
    }
}

/// `str(d.get(key, ""))`。
pub fn str_of_get(d: &Obj, key: &str) -> String {
    d.get(key).map(py_str).unwrap_or_default()
}

// ----------------------------------------------------------------------------- int() / len()

/// 下划线只能夹在两个数字之间（PEP 515）。
fn underscores_between_digits(s: &str) -> bool {
    let cs: Vec<char> = s.chars().collect();
    cs.iter().enumerate().all(|(i, &c)| {
        c != '_' || (i > 0 && is_nd(cs[i - 1]) && cs.get(i + 1).copied().is_some_and(is_nd))
    })
}

/// `int(str)` 的 ValueError 消息。
fn invalid_literal(s: &str) -> PyErr {
    PyErr::Value(format!(
        "invalid literal for int() with base 10: {}",
        repr_str(s)
    ))
}

/// `int(s)`（base 10）：首尾空白、正负号、下划线、任意 Unicode 十进制数字；
/// 超过 4300 位抛 ValueError（CPython 的整数转换上限）；超出 i128 的数值饱和。
pub fn py_int_str(s: &str) -> PyResult<i128> {
    let t = pyc::strip(s);
    let (neg, body) = match t.chars().next() {
        Some('-') => (true, &t[1..]),
        Some('+') => (false, &t[1..]),
        _ => (false, t),
    };
    if body.is_empty()
        || !body.chars().all(|c| c == '_' || is_nd(c))
        || !underscores_between_digits(body)
    {
        return Err(invalid_literal(s));
    }
    let digits = body.chars().filter(|c| *c != '_').count();
    if digits > 4300 {
        return Err(PyErr::Value(format!(
            "Exceeds the limit (4300 digits) for integer string conversion: value has {digits} digits; use sys.set_int_max_str_digits() to increase the limit"
        )));
    }
    let mut acc: i128 = 0;
    let mut saturated = false;
    for c in body.chars().filter(|c| *c != '_') {
        let d = nd_value(c).unwrap_or(0) as i128;
        match acc.checked_mul(10).and_then(|x| x.checked_add(d)) {
            Some(x) => acc = x,
            None => {
                saturated = true;
                break;
            }
        }
    }
    if saturated {
        acc = i128::MAX;
    }
    Ok(if neg { -acc } else { acc })
}

/// `int(v)`（v 是任意 JSON 值）。
pub fn py_int(v: &Value) -> PyResult<i128> {
    match v {
        Value::Bool(b) => Ok(i128::from(*b)),
        Value::Number(n) => match int_of_number(n) {
            Some(i) => Ok(i),
            // 向零截断；超出 i128 的浮点饱和（Python 是任意精度，线上到不了这里）
            None => Ok(n.as_f64().map_or(0, |f| f.trunc() as i128)),
        },
        Value::String(s) => py_int_str(s),
        other => Err(PyErr::Type(format!(
            "int() argument must be a string, a bytes-like object or a real number, not '{}'",
            type_name(other)
        ))),
    }
}

/// `int(d.get(k, missing) or fallback)`：键缺失取 `missing`，值为假（None / 0 / "" / [] / {} ...）取 `fallback`。
pub fn int_or(v: Option<&Value>, missing: i128, fallback: i128) -> PyResult<i128> {
    match v {
        None => Ok(if missing != 0 { missing } else { fallback }),
        Some(x) if !pyval::truthy(x) => Ok(fallback),
        Some(x) => py_int(x),
    }
}

/// `len(v)`。
pub fn py_len(v: &Value) -> PyResult<usize> {
    match v {
        Value::String(s) => Ok(pyc::len_chars(s)),
        Value::Array(a) => Ok(a.len()),
        Value::Object(o) => Ok(o.len()),
        other => Err(PyErr::Type(format!(
            "object of type '{}' has no len()",
            type_name(other)
        ))),
    }
}

/// `len(d.get(k) or [])`。
pub fn len_or_empty(v: Option<&Value>) -> PyResult<usize> {
    match v {
        Some(x) if pyval::truthy(x) => py_len(x),
        _ => Ok(0),
    }
}

/// 把 `i128` 转成 JSON 整数（超出 u64 / i64 范围时饱和到 i64 边界；夹具不含这种值）。
pub fn json_int(i: i128) -> Value {
    if let Ok(x) = i64::try_from(i) {
        Value::from(x)
    } else if let Ok(x) = u64::try_from(i) {
        Value::from(x)
    } else if i < 0 {
        Value::from(i64::MIN)
    } else {
        Value::from(u64::MAX)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn regex_classes_match_python_sets() {
        let nd = regex::Regex::new(&format!("^[{}]$", nd_class())).unwrap();
        let sp = regex::Regex::new(&format!("^[{}]$", space_class())).unwrap();
        let mut buf = [0u8; 4];
        for cp in 0..=0x10_ffff_u32 {
            let Some(c) = char::from_u32(cp) else {
                continue;
            };
            let s: &str = c.encode_utf8(&mut buf);
            assert_eq!(nd.is_match(s), is_nd(c), "\\d U+{cp:04X}");
            assert_eq!(sp.is_match(s), pyc::is_space(c), "\\s U+{cp:04X}");
        }
    }

    use super::*;
    use serde_json::json;

    #[test]
    fn repr_like_python() {
        assert_eq!(
            py_repr(&json!(["a", 1, null, true, 1.5, {"k": [1]}])),
            "['a', 1, None, True, 1.5, {'k': [1]}]"
        );
        assert_eq!(repr_str("it's"), "\"it's\"");
        assert_eq!(repr_str("both ' and \""), "'both \\' and \"'");
        assert_eq!(repr_str("a\tb\x07"), "'a\\tb\\x07'");
        assert_eq!(py_repr(&json!(1e22)), "1e+22");
    }

    #[test]
    fn int_like_python() {
        assert_eq!(py_int_str(" 1_000 "), Ok(1000));
        assert_eq!(py_int_str("１２"), Ok(12));
        assert_eq!(
            py_int_str("abc").unwrap_err().message(),
            "invalid literal for int() with base 10: 'abc'"
        );
        assert_eq!(
            py_int_str("99999999999999999999999"),
            Ok(99_999_999_999_999_999_999_999)
        );
        assert_eq!(py_int(&json!(28.99)), Ok(28));
        assert_eq!(py_int(&json!(-0.5)), Ok(0));
        assert!(matches!(py_int(&json!([1])), Err(PyErr::Type(_))));
    }
}
