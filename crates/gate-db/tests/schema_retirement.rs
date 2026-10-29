use gate_db::{Db, policy_migration};
use rusqlite::Connection;
#[path = "support/policy.rs"]
mod storage;

fn columns(connection: &Connection, table: &str) -> Vec<String> {
    connection
        .prepare(&format!("PRAGMA table_info({table})"))
        .unwrap()
        .query_map([], |row| row.get(1))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}
fn retired(connection: &Connection) {
    let keys = columns(connection, "api_keys");
    for name in policy_migration::LEGACY_KEY_COLUMNS {
        assert!(!keys.iter().any(|key| key == name), "{name}");
    }
    assert!(!columns(connection, "request_settlements").contains(&"exclude_global_v5".to_owned()));
    let flags: i64 = connection
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE name='deleted_key_usage_flags'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(flags, 0);
}
#[tokio::test]
async fn fresh_install_is_native_and_reopening_does_not_restore_old_fields() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("gate.db");
    let db = Db::open(&path).unwrap();
    assert!(!db.claim_rules().await.unwrap().open);
    assert!(!db.member_defaults().await.unwrap().v5);
    drop(db);
    retired(&Connection::open(&path).unwrap());
    let db = Db::open_for_gateway(&path, 2000.0, 3).unwrap();
    assert!(db.get_setting("quota_modes_v1").await.unwrap().is_none());
    drop(db);
    retired(&Connection::open(path).unwrap());
}
#[tokio::test]
async fn legacy_retirement_preserves_identity_counters_and_uncertain_holds() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("gate.db");
    let old = storage::legacy();
    old.execute_batch("ALTER TABLE request_settlements ADD COLUMN exclude_global_v5 INTEGER NOT NULL DEFAULT 0;
 INSERT INTO counters(key_id,day,images,legacy_free_images,anlas,v5,requests) VALUES(1,'2030-01-01',7,4,2.5,3,8);
 INSERT INTO request_settlements(request_id,rid,key_id,status,day,month,anlas,v5,legacy,updated_at,exclude_global_v5)
 VALUES('synthetic-request','synthetic',1,'uncertain','2030-01-01','2030-01',2.5,3,4,1,1);").unwrap();
    old.execute("VACUUM INTO ?", [path.to_string_lossy().as_ref()])
        .unwrap();
    drop(old);
    assert!(Db::open(&path).is_err()); // Timestamp/grace must be explicit for an old database.
    let db = Db::open_for_gateway(&path, 1000.0, 3).unwrap();
    let key = db.get_key(1).await.unwrap().unwrap();
    assert_eq!(key["discord_id"], "900000000000000001");
    let counter = db.get_counter(1, "2030-01-01").await.unwrap();
    assert_eq!(
        (counter.images, counter.legacy_free_images, counter.v5),
        (7, 4, 3)
    );
    assert_eq!(counter.anlas, 2.5);
    let pending = db.list_unknown_request_settlements().await.unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(
        (pending[0].anlas, pending[0].v5, pending[0].legacy),
        (2.5, 3, 4)
    );
    assert!(
        db.member_policy("900000000000000001")
            .await
            .unwrap()
            .effective
            .v5
    );
    drop(db);
    retired(&Connection::open(&path).unwrap());
    let db = Db::open_for_gateway(&path, 2000.0, 3).unwrap();
    assert_eq!(
        db.list_unknown_request_settlements().await.unwrap().len(),
        1
    );
    drop(db);
    retired(&Connection::open(path).unwrap());
}
#[test]
fn ddl_failure_rolls_back_permissions_and_can_be_retried() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("gate.db");
    let mut conn = storage::legacy();
    conn.execute(
        "CREATE INDEX synthetic_old_permission ON api_keys(image_model_scope)",
        [],
    )
    .unwrap();
    assert!(policy_migration::run(&mut conn, &path, 1000.0, 3).is_err());
    assert!(columns(&conn, "api_keys").contains(&"daily_images".to_owned()));
    assert_eq!(
        conn.query_row("SELECT count(*) FROM member_rules", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        conn.query_row(
            "SELECT count(*) FROM site_settings WHERE key='member_policy_v1_migrated'",
            [],
            |row| row.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    conn.execute("DROP INDEX synthetic_old_permission", [])
        .unwrap();
    assert!(
        policy_migration::run(&mut conn, &path, 1000.0, 3)
            .unwrap()
            .is_some()
    );
    retired(&conn);
}

#[tokio::test]
async fn status_retirement_preserves_usage_and_pending_settlements() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("gate.db");
    drop(Db::open(&path).unwrap());
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(
        "CREATE TABLE incidents(id INTEGER PRIMARY KEY, title TEXT);
         CREATE TABLE incident_updates(id INTEGER PRIMARY KEY, incident_id INTEGER);
         CREATE TABLE status_hourly(hour INTEGER, code INTEGER, n INTEGER);
         INSERT INTO incidents VALUES(1,'synthetic');
         INSERT INTO status_hourly VALUES(0,200,1);
         INSERT INTO site_settings VALUES('novelai_official_status','{}');
         INSERT INTO usage_log(ts,kind,status,images) VALUES(1,'image','ok',1);
         INSERT INTO request_settlements(request_id,rid,key_id,status,day,updated_at)
         VALUES('synthetic-pending','synthetic',1,'uncertain','2030-01-01',1);",
    )
    .unwrap();
    drop(conn);
    for _ in 0..2 {
        drop(Db::open(&path).unwrap());
        let conn = Connection::open(&path).unwrap();
        let retired: i64 = conn.query_row(
            "SELECT count(*) FROM sqlite_master WHERE name IN ('incidents','incident_updates','status_hourly')",
            [], |row| row.get(0),
        ).unwrap();
        assert_eq!(retired, 0);
        assert_eq!(
            conn.query_row("SELECT count(*) FROM usage_log", [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert_eq!(
            conn.query_row("SELECT status FROM request_settlements", [], |row| row
                .get::<_, String>(
                0
            ))
            .unwrap(),
            "uncertain"
        );
        assert_eq!(
            conn.query_row(
                "SELECT count(*) FROM site_settings WHERE key='novelai_official_status'",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
            0
        );
    }
}
