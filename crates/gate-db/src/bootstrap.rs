//! 建库与版本校验。
//!
//! 只有一条路径：`sql/schema.sql` 建出最终形态。没有「老库补列」「老库重建」「下线表清理」
//! 这类兼容分支——库不是当前形态就直接报错，不做在线迁移（迁移用 `deploy/ops/migrate-schema.sh`）。

use rusqlite::Connection;

const SCHEMA: &str = include_str!("sql/schema.sql");

/// 结构版本。改 `schema.sql` 之后必须同时 +1，并给 `deploy/ops/migrate-schema.sh` 补一条迁移。
pub const SCHEMA_VERSION: i64 = 4;

/// 连接级设置：WAL + synchronous=NORMAL + 锁等待 15 秒。
pub fn configure(conn: &Connection) -> rusqlite::Result<()> {
    conn.busy_timeout(std::time::Duration::from_secs(15))?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "foreign_keys", "OFF")?;
    Ok(())
}

/// 建表 + 版本校验。全新库建完盖章；已有库必须是当前版本。
pub fn run(conn: &mut Connection) -> rusqlite::Result<()> {
    configure(conn)?;
    let fresh: bool = conn.query_row(
        "SELECT NOT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%')",
        [],
        |r| r.get(0),
    )?;
    conn.execute_batch(SCHEMA)?;
    seed_policy_defaults(conn)?;
    if fresh {
        conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        return Ok(());
    }
    let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if version != SCHEMA_VERSION {
        return Err(rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_MISMATCH),
            Some(format!(
                "库结构版本 {version}，本版要求 {SCHEMA_VERSION}：先跑 deploy/ops/migrate-schema.sh 升级，不做在线迁移"
            )),
        ));
    }
    Ok(())
}

/// 成员规则的三条初始设置：默认值、全站预算、领取条件。
///
/// 以前由成员规则迁移（已删除）在新库上顺手写入；现在建库时直接播，缺哪条补哪条，
/// 已有值一律不动。
fn seed_policy_defaults(conn: &Connection) -> rusqlite::Result<()> {
    use crate::repo::member_policy::{BUDGETS_KEY, CLAIM_KEY, DEFAULTS_KEY};
    use gate_core::member_policy::{ClaimRules, MemberPolicy, SharedBudgets};

    let pairs: [(&str, String); 3] = [
        (DEFAULTS_KEY, to_json(&MemberPolicy::initial())),
        (BUDGETS_KEY, to_json(&SharedBudgets::initial())),
        (CLAIM_KEY, to_json(&ClaimRules::initial())),
    ];
    for (key, value) in pairs {
        conn.execute(
            "INSERT INTO site_settings(key, value) VALUES(?,?) ON CONFLICT(key) DO NOTHING",
            [key, &value],
        )?;
    }
    Ok(())
}

fn to_json<T: serde::Serialize>(value: &T) -> String {
    serde_json::to_string(value).expect("成员规则默认值必须可序列化")
}
