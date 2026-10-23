//! 后台数值字段的严格解析，所有后台接口共用这一份。

use crate::error::HttpError;
use gate_core::{pyc, pyval};
use serde_json::Value;

/// 数字字段的类型（`_num` 的 `kind`）。
#[derive(Clone, Copy, PartialEq)]
pub(super) enum Kind {
    Int,
    Float,
}

/// 严格解析并校验范围，返回 f64。空值、布尔、非数字、越界、小数（整数字段）一律 422。
fn parse(value: &Value, kind: Kind, lo: f64, hi: f64, name: &str) -> Result<f64, HttpError> {
    let empty = match value {
        Value::Null | Value::Bool(_) => true,
        Value::String(s) => pyc::strip(s).is_empty(),
        _ => false,
    };
    if empty {
        return Err(HttpError::new(422, format!("{name} 不能为空")));
    }
    let invalid = || HttpError::new(422, format!("{name} 必须是有效数字"));
    let v = match value {
        Value::Number(n) => n.as_f64().ok_or_else(invalid)?,
        Value::String(s) => pyc::parse_float(s).ok_or_else(invalid)?,
        _ => return Err(invalid()),
    };
    if !v.is_finite() {
        return Err(invalid());
    }
    if kind == Kind::Int && v.fract() != 0.0 {
        return Err(HttpError::new(422, format!("{name} 必须是整数")));
    }
    if !(lo <= v && v <= hi) {
        let show = |x: f64| {
            if kind == Kind::Int {
                (x as i64).to_string()
            } else {
                pyc::float_repr(x)
            }
        };
        return Err(HttpError::new(
            422,
            format!("{name} 必须在 {}～{} 之间", show(lo), show(hi)),
        ));
    }
    Ok(v)
}

/// 严格解析后台数值。
/// 这些字段里 0 代表「不限」，绝不能把空值或非法值静默变成 0，否则一次误操作就会拆掉额度保护。
/// 整数字段返回 JSON 整数，浮点字段返回 JSON 浮点。
pub(super) fn strict_num(
    value: &Value,
    kind: Kind,
    lo: f64,
    hi: f64,
    name: &str,
) -> Result<Value, HttpError> {
    let v = parse(value, kind, lo, hi, name)?;
    Ok(match kind {
        Kind::Int => Value::from(v as i64),
        Kind::Float => pyval::float(v),
    })
}

/// 整数字段，直接返回 i64。
pub(super) fn strict_int(value: &Value, lo: i64, hi: i64, name: &str) -> Result<i64, HttpError> {
    Ok(parse(value, Kind::Int, lo as f64, hi as f64, name)? as i64)
}

/// 浮点字段，直接返回 f64。
pub(super) fn strict_float(value: &Value, lo: f64, hi: f64, name: &str) -> Result<f64, HttpError> {
    parse(value, Kind::Float, lo, hi, name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn strict_num_matches_python_messages() {
        let int = |v: Value| strict_num(&v, Kind::Int, 0.0, 100000.0, "n").map_err(|e| e.detail);
        assert_eq!(int(json!(150)).unwrap(), json!(150));
        assert_eq!(int(json!("150")).unwrap(), json!(150));
        assert_eq!(int(json!(12.0)).unwrap(), json!(12));
        for (bad, msg) in [
            (json!(true), "n 不能为空"),
            (json!(null), "n 不能为空"),
            (json!(" "), "n 不能为空"),
            (json!([1]), "n 必须是有效数字"),
            (json!("abc"), "n 必须是有效数字"),
            (json!(1.5), "n 必须是整数"),
            (json!(-1), "n 必须在 0～100000 之间"),
        ] {
            assert_eq!(int(bad).unwrap_err(), json!(msg));
        }
        let float =
            |v: Value| strict_num(&v, Kind::Float, 0.0, 1_000_000.0, "m").map_err(|e| e.detail);
        assert_eq!(float(json!(5000)).unwrap(), json!(5000.0));
        assert_eq!(
            float(json!(2e6)).unwrap_err(),
            json!("m 必须在 0.0～1000000.0 之间")
        );
    }

    #[test]
    fn int_and_float_wrappers() {
        assert_eq!(strict_int(&json!(" 25 "), 1, 500, "x").unwrap(), 25);
        assert_eq!(strict_int(&json!("1e1"), 1, 500, "x").unwrap(), 10);
        assert_eq!(
            strict_int(&json!(501), 1, 500, "x").unwrap_err().detail,
            json!("x 必须在 1～500 之间")
        );
        assert_eq!(strict_float(&json!("2.5"), 0.0, 30.0, "x").unwrap(), 2.5);
        assert_eq!(
            strict_float(&json!(31), 0.0, 30.0, "x").unwrap_err().detail,
            json!("x 必须在 0.0～30.0 之间")
        );
    }
}
