//! 宽松取值：旧库靠 SQLite 的弱类型，同一列里可能混着 INTEGER / REAL / TEXT / NULL
//! （例如 `REAL` 列里存了整数、设置表里的数字是字符串）。Python 拿到什么就用什么，
//! 这里按「Python 会怎么理解这个值」来转换，而不是像 rusqlite 默认那样直接报类型错误。

use rusqlite::types::ValueRef;
use rusqlite::{Row, RowIndex};

/// 浮点：INTEGER / REAL 直接转；TEXT 按 `float()` 解析；NULL 或无法解析 → None。
pub fn f64_opt<I: RowIndex>(row: &Row<'_>, idx: I) -> rusqlite::Result<Option<f64>> {
    Ok(match row.get_ref(idx)? {
        ValueRef::Null => None,
        ValueRef::Integer(i) => Some(i as f64),
        ValueRef::Real(f) => Some(f),
        ValueRef::Text(t) => std::str::from_utf8(t)
            .ok()
            .and_then(|s| s.trim().parse().ok()),
        ValueRef::Blob(_) => None,
    })
}

pub fn f64_or<I: RowIndex>(row: &Row<'_>, idx: I, default: f64) -> rusqlite::Result<f64> {
    Ok(f64_opt(row, idx)?.unwrap_or(default))
}

/// 整数：INTEGER 直接取；REAL 按 `int()` 向零截断；TEXT 按 `int(float(s))`；NULL → None。
pub fn i64_opt<I: RowIndex>(row: &Row<'_>, idx: I) -> rusqlite::Result<Option<i64>> {
    Ok(match row.get_ref(idx)? {
        ValueRef::Null => None,
        ValueRef::Integer(i) => Some(i),
        ValueRef::Real(f) if f.is_finite() => Some(f.trunc() as i64),
        ValueRef::Real(_) => None,
        ValueRef::Text(t) => std::str::from_utf8(t)
            .ok()
            .and_then(|s| s.trim().parse::<f64>().ok())
            .filter(|f| f.is_finite())
            .map(|f| f.trunc() as i64),
        ValueRef::Blob(_) => None,
    })
}

pub fn i64_or<I: RowIndex>(row: &Row<'_>, idx: I, default: i64) -> rusqlite::Result<i64> {
    Ok(i64_opt(row, idx)?.unwrap_or(default))
}

/// 布尔：Python 的 `bool(row[x])`。
pub fn flag<I: RowIndex>(row: &Row<'_>, idx: I) -> rusqlite::Result<bool> {
    Ok(match row.get_ref(idx)? {
        ValueRef::Null => false,
        ValueRef::Integer(i) => i != 0,
        ValueRef::Real(f) => f != 0.0,
        ValueRef::Text(t) => !t.is_empty(),
        ValueRef::Blob(b) => !b.is_empty(),
    })
}

/// 文本：TEXT 原样；数字按 Python `str()`；NULL → None。
pub fn text_opt<I: RowIndex>(row: &Row<'_>, idx: I) -> rusqlite::Result<Option<String>> {
    Ok(match row.get_ref(idx)? {
        ValueRef::Null => None,
        ValueRef::Text(t) => Some(String::from_utf8_lossy(t).into_owned()),
        ValueRef::Integer(i) => Some(i.to_string()),
        ValueRef::Real(f) => Some(gate_core::pyc::float_repr(f)),
        ValueRef::Blob(b) => Some(String::from_utf8_lossy(b).into_owned()),
    })
}

pub fn text_or<I: RowIndex>(row: &Row<'_>, idx: I, default: &str) -> rusqlite::Result<String> {
    Ok(text_opt(row, idx)?.unwrap_or_else(|| default.to_string()))
}

/// 整行转成 JSON 对象（列名 → 值，按查询列顺序）。后台列表接口与对拍测试用。
pub fn row_json(row: &Row<'_>) -> rusqlite::Result<serde_json::Value> {
    let stmt = row.as_ref();
    let mut map = serde_json::Map::new();
    for i in 0..stmt.column_count() {
        let name = stmt.column_name(i)?.to_string();
        let v = match row.get_ref(i)? {
            ValueRef::Null => serde_json::Value::Null,
            ValueRef::Integer(n) => serde_json::Value::from(n),
            ValueRef::Real(f) => serde_json::Number::from_f64(f)
                .map(serde_json::Value::Number)
                .unwrap_or(serde_json::Value::Null),
            ValueRef::Text(t) => serde_json::Value::String(String::from_utf8_lossy(t).into_owned()),
            ValueRef::Blob(b) => serde_json::Value::from(b.len()),
        };
        map.insert(name, v);
    }
    Ok(serde_json::Value::Object(map))
}
