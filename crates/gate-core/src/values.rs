//! Explicit JSON value accessors.
use serde_json::{Map, Value};
pub fn enabled(v: &Value) -> bool {
    match v {
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|n| n != 0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(m) => !m.is_empty(),
        Value::Null => false,
    }
}
pub fn int_like(v: &Value) -> Option<i64> {
    v.as_i64()
}
pub fn strict_int(v: &Value) -> Option<i64> {
    v.as_i64()
}
pub fn number(v: &Value) -> Option<f64> {
    v.as_f64()
}
pub fn strict_number(v: &Value) -> Option<f64> {
    v.as_f64()
}
pub fn is_float(v: &Value) -> bool {
    v.is_f64()
}
pub fn equal(a: &Value, b: &Value) -> bool {
    a == b
}
pub fn get<'a>(v: &'a Value, k: &str) -> Option<&'a Value> {
    v.get(k)
}
pub fn pop(m: &mut Map<String, Value>, k: &str) -> Option<Value> {
    m.shift_remove(k)
}
pub fn text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}
pub fn float(x: f64) -> Value {
    serde_json::Number::from_f64(x)
        .map(Value::Number)
        .unwrap_or(Value::Null)
}
