//! Small numeric and Unicode-safe string helpers.
use crate::validation::ValidationError;
pub fn round0(x: f64) -> i64 {
    x.round() as i64
}
pub fn round(x: f64, n: usize) -> f64 {
    let scale = 10_f64.powi(n.min(15) as i32);
    (x * scale).round() / scale
}
pub fn sum<I: IntoIterator<Item = f64>>(items: I) -> f64 {
    items.into_iter().sum()
}
pub fn int_of(x: f64) -> Option<i64> {
    (x.is_finite() && x >= i64::MIN as f64 && x < i64::MAX as f64).then_some(x.trunc() as i64)
}
pub fn floordiv_f(a: f64, b: f64) -> f64 {
    (a / b).floor()
}
pub fn minimum(a: f64, b: f64) -> f64 {
    a.min(b)
}
pub fn maximum(a: f64, b: f64) -> f64 {
    a.max(b)
}
pub fn int_checked(x: f64) -> Result<i64, ValidationError> {
    int_of(x).ok_or_else(|| ValidationError::OutOfRange("number is not a finite i64".into()))
}
pub fn float_text(x: f64) -> String {
    x.to_string()
}
pub fn is_space(c: char) -> bool {
    c.is_whitespace()
}
pub fn strip(s: &str) -> &str {
    s.trim()
}
pub fn take_chars(s: &str, n: usize) -> &str {
    s.char_indices().nth(n).map_or(s, |(i, _)| &s[..i])
}
pub fn last_chars(s: &str, n: usize) -> &str {
    let count = s.chars().count();
    if n >= count {
        s
    } else {
        s.char_indices().nth(count - n).map_or("", |(i, _)| &s[i..])
    }
}
pub fn len_chars(s: &str) -> usize {
    s.chars().count()
}
pub fn decimal_value(c: char) -> Option<u32> {
    c.to_digit(10)
}
pub fn parse_int(s: &str) -> Option<i64> {
    s.trim().parse().ok()
}
pub fn parse_float(s: &str) -> Option<f64> {
    s.trim().parse::<f64>().ok().filter(|f| f.is_finite())
}
pub fn parse_int_via_float(s: &str) -> Option<i64> {
    parse_float(s).and_then(int_of)
}
pub fn bool_text(v: bool) -> &'static str {
    if v { "true" } else { "false" }
}
