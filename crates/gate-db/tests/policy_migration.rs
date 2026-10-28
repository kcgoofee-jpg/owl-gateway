use gate_core::member_policy::MemberOverrides;
use gate_db::policy_migration::preview;
use rusqlite::Connection;

#[path = "support/policy.rs"]
mod storage;
fn setup() -> Connection {
    storage::legacy()
}

#[test]
fn restricted_v5_keeps_only_authorized_subjects_and_equal_manual_limits_merge() {
    let conn = setup();
    conn.execute_batch(
        "UPDATE api_keys SET quota_auto=-1,daily_images=150,daily_v5=9999,monthly_anlas=99999;
         INSERT INTO api_keys(id,name,token,created_at,discord_id,image_model_scope,rpm)
         VALUES(2,'synthetic-2','nai-synthetic-policy-two',1,'900000000000000002','all',10);",
    )
    .unwrap();
    let plan = preview(&conn, 1000.0, 3).unwrap();
    assert!(!plan.defaults.v5);
    assert_eq!(plan.summary.v5_authorized, 1);
    let first = &plan.members[0].overrides;
    assert_eq!(first.v5, Some(true));
    assert_eq!(first.daily_images, None);
    assert_eq!(first.daily_v5, None);
    assert_eq!(plan.members[1].overrides.v5, None);
}

#[test]
fn invite_grace_does_not_change_last_generation_or_usage() {
    let conn = setup();
    conn.execute_batch(
        "INSERT INTO key_tags(key_id,tag,note,\"by\",ts) VALUES(1,'邀请','','synthetic',1);
         UPDATE api_keys SET last_image_at=5,exclude_global_v5=1;
         INSERT INTO counters(key_id,day,images,v5) VALUES(1,'2030-01-01',7,2);",
    )
    .unwrap();
    let plan = preview(&conn, 1000.0, 3).unwrap();
    assert_eq!(
        plan.members[0].recycle_not_before,
        Some(1000.0 + 3.0 * 86400.0)
    );
    assert_eq!(plan.summary.removed_shared_v5_exemptions, 1);
    assert_eq!(
        conn.query_row("SELECT last_image_at FROM api_keys WHERE id=1", [], |r| r
            .get::<_, f64>(
            0
        ))
        .unwrap(),
        5.0
    );
    assert_eq!(
        conn.query_row("SELECT images FROM counters WHERE key_id=1", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        7
    );
}

#[test]
fn migration_is_backed_up_atomic_and_repeatable_without_old_policy_reads() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("gate.db");
    let mut conn = setup();
    conn.execute_batch(
        "INSERT INTO counters(key_id,day,images,v5) VALUES(1,'2030-01-01',7,2);
        INSERT INTO request_settlements(request_id,rid,key_id,status,day,updated_at)
        VALUES('synthetic-request','synthetic',1,'uncertain','2030-01-01',1);",
    )
    .unwrap();
    let first = gate_db::policy_migration::run(&mut conn, &path, 1000.0, 3)
        .unwrap()
        .unwrap();
    assert_eq!(first.members, 1);
    let backup_root = directory.path().join("gate.db.before-member-policy");
    let attempt = std::fs::read_dir(&backup_root)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let recovery = Connection::open(attempt.join("database.sqlite")).unwrap();
    assert_eq!(
        recovery
            .query_row("PRAGMA integrity_check", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "ok"
    );
    assert_eq!(
        recovery
            .query_row("SELECT count(*) FROM member_rules", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        conn.query_row("SELECT images FROM counters WHERE key_id=1", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        7
    );
    assert_eq!(
        conn.query_row("SELECT status FROM request_settlements", [], |r| r
            .get::<_, String>(0))
            .unwrap(),
        "uncertain"
    );
    assert!(
        gate_db::policy_migration::run(&mut conn, &path, 1500.0, 3)
            .unwrap()
            .is_none()
    );
    assert_eq!(std::fs::read_dir(&backup_root).unwrap().count(), 1);
    conn.execute(
        "INSERT OR REPLACE INTO site_settings(key,value) VALUES('v5_access_mode','invalid')",
        [],
    )
    .unwrap();
    assert!(
        gate_db::policy_migration::run(&mut conn, &path, 2000.0, 3)
            .unwrap()
            .is_none()
    );
    assert_eq!(std::fs::read_dir(&backup_root).unwrap().count(), 2);
    conn.execute("UPDATE member_rules SET overrides='{}'", [])
        .unwrap();
    conn.execute("UPDATE member_rules SET overrides='invalid'", [])
        .unwrap();
    assert!(gate_db::policy_migration::run(&mut conn, &path, 3000.0, 3).is_err());
}

#[test]
fn backup_failure_leaves_source_permissions_intact_and_retry_succeeds() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("gate.db");
    let backup_root = directory.path().join("gate.db.before-member-policy");
    let mut conn = setup();
    std::fs::write(&backup_root, b"block-directory").unwrap();
    assert!(gate_db::policy_migration::run(&mut conn, &path, 1000.0, 3).is_err());
    assert_eq!(
        conn.query_row("SELECT count(*) FROM member_rules", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        conn.query_row("SELECT image_model_scope FROM api_keys", [], |r| r
            .get::<_, String>(0))
            .unwrap(),
        "all"
    );
    assert_eq!(
        conn.query_row(
            "SELECT count(*) FROM site_settings WHERE key='member_policy_v1_migrated'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    std::fs::remove_file(backup_root).unwrap();
    assert!(
        gate_db::policy_migration::run(&mut conn, &path, 1000.0, 3)
            .unwrap()
            .is_some()
    );
}

#[test]
fn preflight_is_repeatable_and_rejects_corruption_or_identity_conflicts() {
    let conn = setup();
    let first = preview(&conn, 1000.0, 3).unwrap();
    let second = preview(&conn, 1000.0, 3).unwrap();
    assert_eq!(first.defaults, second.defaults);
    assert_eq!(first.members[0].overrides, second.members[0].overrides);
    conn.execute("UPDATE api_keys SET quota_limits='{}'", [])
        .unwrap();
    assert!(preview(&conn, 1000.0, 3).is_err());
    conn.execute("UPDATE api_keys SET quota_limits=NULL", [])
        .unwrap();
    conn.execute_batch(
        "INSERT INTO discord_registrations(discord_id,key_id,created_at) VALUES('900000000000000002',1,1);"
    ).unwrap();
    assert!(preview(&conn, 1000.0, 3).is_err());
}

#[test]
fn invalid_modes_do_not_create_default_permissions() {
    let conn = setup();
    conn.execute(
        "UPDATE site_settings SET value='unknown' WHERE key='v5_access_mode'",
        [],
    )
    .unwrap();
    assert!(preview(&conn, 1000.0, 3).is_err());
    assert_eq!(MemberOverrides::default().v5, None);
}

#[test]
fn explicit_pre_authorization_survives_without_a_current_credential() {
    let mut conn = setup();
    conn.execute(
        "UPDATE site_settings SET value=? WHERE key='v5_authorized_discord_ids'",
        [r#"["900000000000000001","900000000000000002"]"#],
    )
    .unwrap();
    let plan = preview(&conn, 1000.0, 3).unwrap();
    let pending = plan
        .members
        .iter()
        .find(|member| member.discord_id == "900000000000000002")
        .unwrap();
    assert_eq!(pending.overrides.v5, Some(true));
    assert_eq!(
        conn.query_row("SELECT count(*) FROM api_keys", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
    let dir = tempfile::tempdir().unwrap();
    gate_db::policy_migration::run(&mut conn, &dir.path().join("gate.db"), 1000.0, 3).unwrap();
    assert!(conn.query_row("SELECT json_extract(overrides,'$.v5') FROM member_rules WHERE discord_id='900000000000000002'",[],|row|row.get::<_,bool>(0)).unwrap());
}
