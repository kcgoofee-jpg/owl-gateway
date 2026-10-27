//! JSON field validation with native Rust numbers and errors.
use serde_json::{Map, Number, Value};
pub type Obj = Map<String, Value>;
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ValidationError {
    #[error("{0}")]
    InvalidValue(String),
    #[error("{0}")]
    InvalidType(String),
    #[error("{0}")]
    InvalidStructure(String),
    #[error("{0}")]
    OutOfRange(String),
    #[error("{0}")]
    DivisionByZero(String),
}
pub type ValidationResult<T> = Result<T, ValidationError>;
impl ValidationError {
    pub fn type_name(&self) -> &'static str {
        match self {
            Self::InvalidValue(_) => "invalid_value",
            Self::InvalidType(_) => "invalid_type",
            Self::InvalidStructure(_) => "invalid_structure",
            Self::OutOfRange(_) => "out_of_range",
            Self::DivisionByZero(_) => "division_by_zero",
        }
    }
    pub fn message(&self) -> &str {
        match self {
            Self::InvalidValue(s)
            | Self::InvalidType(s)
            | Self::InvalidStructure(s)
            | Self::OutOfRange(s)
            | Self::DivisionByZero(s) => s,
        }
    }
    pub fn value(s: impl Into<String>) -> Self {
        Self::InvalidValue(s.into())
    }
}
pub fn type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}
pub fn int_of_number(n: &Number) -> Option<i128> {
    n.as_i64()
        .map(i128::from)
        .or_else(|| n.as_u64().map(i128::from))
}
pub fn int_like(v: &Value) -> Option<i128> {
    match v {
        Value::Number(n) => int_of_number(n),
        _ => None,
    }
}
pub fn strict_int(v: &Value) -> Option<i128> {
    int_like(v)
}
pub fn enabled_opt(v: Option<&Value>) -> bool {
    v.is_some_and(crate::values::enabled)
}
pub fn num_eq_int(v: Option<&Value>, lit: i128) -> bool {
    v.and_then(Value::as_f64).is_some_and(|n| n == lit as f64)
}
pub fn is_nd(c: char) -> bool {
    c.is_ascii_digit()
}
pub fn is_decimal(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}
pub fn nd_value(c: char) -> Option<u32> {
    c.to_digit(10)
}
pub fn is_digit(c: char) -> bool {
    c.is_ascii_digit()
}
pub fn nd_class() -> &'static str {
    "0-9"
}
pub fn space_class() -> &'static str {
    r"\s"
}
pub fn quoted(s: &str) -> String {
    serde_json::to_string(s).expect("valid string")
}
pub fn format_value(v: &Value) -> String {
    v.to_string()
}
pub fn text(v: &Value) -> String {
    crate::values::text(v)
}
pub fn str_of_get(d: &Obj, k: &str) -> String {
    d.get(k).map(text).unwrap_or_default()
}
pub fn integer_string(s: &str) -> ValidationResult<i128> {
    s.trim()
        .parse()
        .map_err(|_| ValidationError::InvalidValue("expected a decimal integer".into()))
}
pub fn integer(v: &Value) -> ValidationResult<i128> {
    int_like(v).ok_or_else(|| ValidationError::InvalidType("expected an integer".into()))
}
pub fn int_or(v: Option<&Value>, missing: i128, _fallback: i128) -> ValidationResult<i128> {
    match v {
        None | Some(Value::Null) => Ok(missing),
        Some(v) => integer(v),
    }
}
pub fn length(v: &Value) -> ValidationResult<usize> {
    match v {
        Value::String(s) => Ok(s.chars().count()),
        Value::Array(a) => Ok(a.len()),
        Value::Object(m) => Ok(m.len()),
        _ => Err(ValidationError::InvalidType(
            "expected a string, array, or object".into(),
        )),
    }
}
pub fn len_or_empty(v: Option<&Value>) -> ValidationResult<usize> {
    match v {
        None | Some(Value::Null) => Ok(0),
        Some(v) => length(v),
    }
}
pub fn json_int(i: i128) -> Value {
    i64::try_from(i)
        .map(Value::from)
        .or_else(|_| u64::try_from(i).map(Value::from))
        .unwrap_or(Value::Null)
}
