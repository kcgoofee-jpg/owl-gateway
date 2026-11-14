//! 数据库 bootstrap 的幂等性、事务与结构约束。
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

#[tokio::test]
async fn v6_migration_preserves_unknown_v5_and_unattributed_holds() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("v6.db");
    drop(gate_db::Db::open(&path).unwrap());
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch("DROP TABLE upstream_v5_reservations; DROP TABLE admin_dm_attempts; PRAGMA user_version=6; \
      INSERT INTO request_settlements(request_id,rid,key_id,status,day,v5,updated_at) VALUES \
      ('linked','rid1',1,'uncertain','2026-11-14',1,1), \
      ('orphan','rid2',1,'dispatched','2026-11-14',1,1), \
      ('saved','rid3',1,'result_saved','2026-11-14',1,1), \
      ('released','rid4',1,'released','2026-11-14',1,1); \
      INSERT INTO usage_log(id,ts,key_id,key_name,kind,model,status,token_id) VALUES(1,1,1,'synthetic','image','v5','error','synthetic-token'); \
      INSERT INTO usage_request_links(request_id,usage_log_id) VALUES('linked',1); \
      INSERT INTO admin_actions(ts,actor,action) VALUES(strftime('%s','now'),'admin','私信成员'),(1,'admin','私信成员');").unwrap();
    drop(conn);
    let script =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../deploy/ops/migrate-schema.sh");
    let output = std::process::Command::new("bash")
        .arg(&script)
        .arg(&path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let conn = Connection::open(&path).unwrap();
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM admin_dm_attempts", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        conn.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        7
    );
    let rows: Vec<(String, String)> = conn
        .prepare("SELECT request_id,token_id FROM upstream_v5_reservations ORDER BY request_id")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(
        rows,
        vec![
            ("linked".into(), "synthetic-token".into()),
            ("orphan".into(), "".into())
        ]
    );
    drop(conn);
    let db = gate_db::Db::open(&path).unwrap();
    assert_eq!(
        db.upstream_v5_used_and_held("synthetic-token", "2026-11-14")
            .await
            .unwrap(),
        2
    );
    assert_eq!(
        db.upstream_v5_used_and_held("another-token", "2026-11-14")
            .await
            .unwrap(),
        1
    );
    assert!(
        db.manually_release_unknown_request("linked", "synthetic", "confirmed not executed", 3.0)
            .await
            .unwrap()
    );
    assert_eq!(
        db.upstream_v5_used_and_held("synthetic-token", "2026-11-14")
            .await
            .unwrap(),
        1
    );
    drop(db);
    let backups: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(Result::unwrap)
        .filter(|e| {
            e.file_name()
                .to_string_lossy()
                .starts_with("v6.db.before-v7.")
        })
        .collect();
    assert_eq!(backups.len(), 1);
    let backup = Connection::open(backups[0].path()).unwrap();
    assert_eq!(
        backup
            .query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        6
    );
    assert_eq!(
        backup
            .query_row("SELECT COUNT(*) FROM request_settlements", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        4
    );
    drop(backup);
    let second = std::process::Command::new("bash")
        .arg(&script)
        .arg(&path)
        .output()
        .unwrap();
    assert!(
        second.status.success(),
        "{}",
        String::from_utf8_lossy(&second.stderr)
    );
    assert_eq!(
        Connection::open(&path)
            .unwrap()
            .query_row("SELECT COUNT(*) FROM upstream_v5_reservations", [], |r| r
                .get::<_, i64>(
                0
            ))
            .unwrap(),
        2
    );
}
