//! 旧版 `app/database.py` 里 `class Database` 的逐方法移植。
//!
//! 约定：
//! - 方法名、参数顺序、SQL 语义与旧版一致；旧版里的 `time.time()` 一律改成调用方传入的 `now: f64`，
//!   按日 / 按月的计算用 [`gate_core::clock::Clock`]。
//! - 旧版一个方法里「多条语句 + 一次 commit」= 这里一个 `db.write` 闭包 = 一个事务；
//!   只读方法走只读连接池。`record_success` 的重试循环、`_record_lock` 在单写线程下不改变可见结果，不移植。
//! - 返回「原样的行」（`SELECT *` 之类）用 `serde_json::Value`（[`crate::value::row_json`]，保持列顺序与
//!   INTEGER / REAL 区分，序列化出来与旧版 `dict(row)` 一致）；旧代码自己拼的 dict 用同名字段的结构体。
//! - 旧版返回 `dict[int, ...]` / `dict[str, ...]` 的，这里返回 `Vec<(K, V)>`，顺序就是旧版 dict 的插入顺序。
//!
//! 旧版没有放进 `class Database` 的表（error_events / status_hourly / feedback / discord_registrations /
//! pending_role_removals / upstream_snapshots / key_tags 的读写）是各业务模块直接用 `db._db` 写的 SQL，
//! 不在本次移植范围内。

pub mod abuse;
pub mod account_stats;
pub mod admin_actions;
pub mod admin_members;
pub mod admin_ops;
pub mod admin_records;
pub mod audit;
pub mod counters;
pub mod coupons;
pub mod discord;
pub mod errors;
pub mod feedback;
pub mod footprint;
pub mod guard;
pub mod kernel;
pub mod keys;
pub mod public;
pub mod reconciliation;
pub mod registration;
pub mod settings;
pub mod share_state;
pub mod status_page;
pub mod upstream;
pub mod usage;

use rusqlite::types::Value as SqlValue;

/// 站长手动发的邀请 Key：只由站长手动删除，闲置回收跳过（10-11 站长）。
pub const INVITE_TAG: &str = "邀请";

pub const ROLE_INVITE: &str = "invite";
pub const ROLE_REGULAR: &str = "regular";
/// 身份组 → 中文标签（顺序与旧版 `ROLE_LABELS` 一致）。
pub const ROLE_LABELS: [(&str, &str); 2] = [(ROLE_INVITE, "邀请"), (ROLE_REGULAR, "普通")];

/// 原图在 BLOB（老记录）或文件里。
pub const HAS_IMAGE: &str = "(image IS NOT NULL OR image_file IS NOT NULL)";
pub const IMAGE_SIZE: &str = "COALESCE(image_size, LENGTH(image), 0)";

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
/// 计数器累加（旧版 `_UPSERT_COUNTERS`，列顺序：key_id, day, images, anlas, text_tokens, requests, v5, legacy_free_images）。
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

/// 旧版返回 `dict[K, V]` 的方法：保持 dict 的插入顺序。
pub type PyMap<K, V> = Vec<(K, V)>;

/// `?,?,?` 占位符串（旧版 `",".join("?" * n)`）。
pub(crate) fn marks(n: usize) -> String {
    vec!["?"; n].join(",")
}

/// JSON 值 → SQLite 绑定值（Python sqlite3 绑定参数的规则：bool 当整数；list / dict 不支持）。
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
