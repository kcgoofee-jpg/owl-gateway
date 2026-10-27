//! 全新库：Rust 建出来的表结构必须与旧版 Python `Database.connect()` 建的逐字一致；
//! 再跑一遍 bootstrap 必须是空操作（线上库每次重启都会跑）。

use rusqlite::Connection;
use serde_json::{Value, json};

fn dump(path: &std::path::Path) -> Value {
    dump_conn(&Connection::open(path).unwrap())
}

fn dump_conn(con: &Connection) -> Value {
    let mut stmt = con
        .prepare("SELECT type, name, tbl_name, sql FROM sqlite_master WHERE name NOT LIKE 'sqlite_%' ORDER BY type, name")
        .unwrap();
    let objects: Vec<Value> = stmt
        .query_map([], |r| {
            Ok(
                json!({"type": r.get::<_, String>(0)?, "name": r.get::<_, String>(1)?,
                      "tbl_name": r.get::<_, String>(2)?, "sql": r.get::<_, Option<String>>(3)?}),
            )
        })
        .unwrap()
        .map(Result::unwrap)
        .collect();
    let tables: Vec<String> = con
        .prepare("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    let mut columns = serde_json::Map::new();
    for t in tables {
        let cols: Vec<Value> = con
            .prepare(&format!("PRAGMA table_info({t})"))
            .unwrap()
            .query_map([], |r| {
                Ok(json!([
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, i64>(3)?,
                    r.get::<_, Option<String>>(4)?,
                    r.get::<_, i64>(5)?
                ]))
            })
            .unwrap()
            .map(Result::unwrap)
            .collect();
        columns.insert(t, Value::Array(cols));
    }
    json!({"objects": objects, "columns": columns})
}

#[tokio::test]
async fn bootstrap_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("again.db");
    drop(gate_db::Db::open(&path).unwrap());
    let first = dump(&path);
    drop(gate_db::Db::open(&path).unwrap());
    assert_eq!(dump(&path), first);
}

#[tokio::test]
async fn write_then_read() {
    let dir = tempfile::tempdir().unwrap();
    let db = gate_db::Db::open(dir.path().join("rw.db")).unwrap();
    db.write(|t| {
        t.execute(
            "INSERT INTO site_settings(key, value) VALUES ('k', '15')",
            [],
        )
    })
    .await
    .unwrap();
    let v: String = db
        .read(|c| {
            c.query_row("SELECT value FROM site_settings WHERE key='k'", [], |r| {
                r.get(0)
            })
        })
        .await
        .unwrap();
    assert_eq!(v, "15");
    // 闭包失败 → 整个事务回滚
    let r = db
        .write(|t| {
            t.execute(
                "INSERT INTO site_settings(key, value) VALUES ('x', '1')",
                [],
            )?;
            t.execute("INSERT INTO no_such_table VALUES (1)", [])
        })
        .await;
    assert!(r.is_err());
    let n: i64 = db
        .read(|c| {
            c.query_row(
                "SELECT COUNT(*) FROM site_settings WHERE key='x'",
                [],
                |r| r.get(0),
            )
        })
        .await
        .unwrap();
    assert_eq!(n, 0);
}
