//! 数据库访问层。
//!
//! 约定：
//! - 时间一律由调用方传入 `now: f64`，SQL 里不取当前时间；按日 / 按月的计算用 [`gate_core::clock::Clock`]。
//! - 一个方法里的多条语句放进同一个 `db.write` 闭包 = 一个事务；只读方法走只读连接池。
//! - 返回「原样的行」（`SELECT *` 之类）用 `serde_json::Value`（[`crate::value::row_json`]，保持列顺序与
//!   INTEGER / REAL 区分，键名 = 列名）；调用方自己拼的映射用同名字段的结构体。
//! - 需要「映射」语义的查询返回 `Vec<(K, V)>`，保持插入顺序。
//!

pub mod abuse;
pub mod abuse_scan;
pub mod account_stats;
pub mod admin_actions;
pub mod admin_members;
pub mod admin_ops;
pub mod admin_records;
pub mod counters;
pub mod coupons;
pub mod discord;
pub mod footprint;
pub mod guard;
pub mod kernel;
pub mod keys;
pub mod member_policy;
pub mod public;
pub mod reconciliation;
pub mod registration;
pub mod settings;
pub mod share_state;
pub mod upstream;
pub mod usage;

use rusqlite::types::Value as SqlValue;

/// 由邀请凭据领取的来源标签，不提供权限或生命周期豁免。
pub const INVITE_TAG: &str = "邀请";

/// 后台请求日志列表读出的列：usage_log 的全部列，顺序同 schema.sql（`client` 放在最后）。
/// `client` = 原始 User-Agent、`src` = 来源 IP，保留期（`usage_origin_keep_days`）过后被清成空串。
pub(crate) const LOG_COLUMNS: &str = "id, ts, key_id, key_name, kind, model, status, images, anlas, tokens, \
unconfirmed_anlas, detail, reason, wait_ms, dur_ms, up_status, rid, src, ver, token_id, client, \
xrw, lang, chua, hfp, bid, origin";
/// 写 usage_log 一行（成功记账传空串 reason——和列默认值一样）。
/// `client` = 请求的 User-Agent（截到 160 字符），`src` = 来源 IP（原文；站长 10-18 决定：为筛脚本撞限速 / 转售等滥用，
/// 保留天数按后台设置 usage_origin_keep_days（0 = 永久，默认永久），到期由维护任务清成空串，见 `Db::purge_usage_origin`）。末尾六列是 `usage::ClientHints`（保留期相同）。`ver`（网关版本）还在表里，3.0 不再写。
pub(crate) const INSERT_LOG: &str = "INSERT INTO usage_log (ts, key_id, key_name, kind, model, status, \
images, anlas, tokens, detail, unconfirmed_anlas, wait_ms, dur_ms, up_status, rid, reason, token_id, client, src, \
xrw, lang, chua, hfp, bid, origin) \
VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)";
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
