//! 数据库访问层。
//!
//! 约定：
//! - 时间一律由调用方传入 `now: f64`，SQL 里不取当前时间；按日 / 按月的计算用 [`gate_core::clock::Clock`]。
//! - 一个方法里的多条语句放进同一个 `db.write` 闭包 = 一个事务；只读方法走只读连接池。
//! - 返回「原样的行」（`SELECT *` 之类）用 `serde_json::Value`（[`crate::value::row_json`]，保持列顺序与
//!   INTEGER / REAL 区分，键名 = 列名）；调用方自己拼的映射用同名字段的结构体。
//! - 需要「映射」语义的查询返回 `Vec<(K, V)>`，保持插入顺序。
//!

pub mod account_stats;
pub mod admin_actions;
pub mod admin_members;
pub mod admin_records;
pub mod counters;
pub mod coupons;
pub mod discord;
pub mod footprint;
pub mod guard;
pub mod keys;
pub mod member_policy;
pub mod member_stats;
pub mod patron;
pub mod public;
pub mod registration;
pub mod settings;
pub mod settlement;
pub mod share_state;
pub mod tasks;
pub mod upstream;
pub mod usage;
pub mod usage_stats;

use rusqlite::types::Value as SqlValue;

/// 由邀请凭据领取的来源标签，不提供权限或生命周期豁免。
pub const INVITE_TAG: &str = "邀请";

/// 后台请求日志列表读出的列：usage_log 的全部列。
/// `client` = 原始 User-Agent、`src` = 来源 IP，长期保留。
pub(crate) const LOG_COLUMNS: &str = "id, ts, key_id, key_name, kind, model, status, images, anlas, tokens, \
unconfirmed_anlas, detail, reason, wait_ms, dur_ms, up_status, rid, src, token_id, client";
/// 写 usage_log 一行（成功记账传空串 reason——和列默认值一样）。
/// `client` = 请求的 User-Agent（截到 160 字符），`src` = 来源 IP（原文，长期保留）。
pub(crate) const INSERT_LOG: &str = "INSERT INTO usage_log (ts, key_id, key_name, kind, model, status, \
images, anlas, tokens, detail, unconfirmed_anlas, wait_ms, dur_ms, up_status, rid, reason, token_id, client, src) \
VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)";
/// 计数器累加（列顺序：key_id, day, images, anlas, text_tokens, requests, v5, legacy_free_images）。
pub(crate) const UPSERT_COUNTERS: &str = "INSERT INTO counters \
(key_id, day, images, anlas, text_tokens, requests, v5, legacy_free_images) \
VALUES (?,?,?,?,?,?,?,?) \
ON CONFLICT(key_id, day) DO UPDATE SET \
images = images + excluded.images, \
anlas = anlas + excluded.anlas, \
text_tokens = text_tokens + excluded.text_tokens, \
requests = requests + excluded.requests, \
v5 = v5 + excluded.v5, \
legacy_free_images = legacy_free_images + excluded.legacy_free_images";

/// 保持插入顺序的键值列表。
pub type OrderedPairs<K, V> = Vec<(K, V)>;

/// `?,?,?` 占位符串。
pub(crate) fn marks(n: usize) -> String {
    vec!["?"; n].join(",")
}

pub(crate) fn json_to_sql(v: &serde_json::Value) -> crate::Result<SqlValue> {
    use serde_json::Value;
    Ok(match v {
        Value::Null => SqlValue::Null,
        Value::Bool(b) => SqlValue::Integer(i64::from(*b)),
        Value::Number(n) if n.is_f64() => SqlValue::Real(n.as_f64().unwrap_or(0.0)),
        Value::Number(n) => SqlValue::Integer(
            n.as_i64()
                .ok_or_else(|| crate::DbError::Invalid(format!("整数超出 SQLite 范围：{n}")))?,
        ),
        Value::String(s) => SqlValue::Text(s.clone()),
        Value::Array(_) | Value::Object(_) => {
            return Err(crate::DbError::Invalid(
                "不支持的绑定参数类型（list / dict）".into(),
            ));
        }
    })
}

/// Preserve a domain or serialization error inside a rusqlite transaction callback.
fn conversion_error(error: impl std::error::Error + Send + Sync + 'static) -> rusqlite::Error {
    rusqlite::Error::ToSqlConversionFailure(Box::new(error))
}
