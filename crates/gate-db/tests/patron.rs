use gate_db::Db;
use gate_db::repo::patron::PatronRecord;

const SUBJECT: &str = "900000000000000001";
const OTHER: &str = "900000000000000002";

fn seat(subject: &str, token: Option<&str>) -> PatronRecord {
    PatronRecord {
        subject: subject.into(),
        enabled: true,
        queue_jump: true,
        self_service: true,
        bound_token_id: token.map(str::to_string),
        password_hash: "scrypt$x$x".into(),
        portal_mode: "basic".into(),
    }
}

#[tokio::test]
async fn patron_profiles_bind_one_enabled_token_and_overlay_member_policy() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(dir.path().join("gate.db")).unwrap();
    db.save_patron(None, seat(SUBJECT, Some("token-a")))
        .await
        .unwrap()
        .unwrap();
    let taken = db.save_patron(None, seat(OTHER, Some("token-a"))).await;
    assert_eq!(
        taken.unwrap(),
        Err(gate_db::repo::patron::SeatConflict::TokenTaken)
    );
    db.save_patron(None, seat(OTHER, Some("token-b")))
        .await
        .unwrap()
        .unwrap();
    let ids = db.exclusive_token_ids().await.unwrap();
    assert!(ids.contains("token-a") && ids.contains("token-b"));

    let old_policy = db.member_policy(SUBJECT).await.unwrap();
    let mut closed = seat(SUBJECT, Some("token-a"));
    closed.enabled = false;
    let open = seat(SUBJECT, Some("token-a"));
    db.save_patron(Some(open.clone()), closed)
        .await
        .unwrap()
        .unwrap();
    assert!(
        !db.set_patron_overrides(open.clone(), old_policy.clone(), old_policy.overrides)
            .await
            .unwrap()
    );
    let mut stale = open.clone();
    stale.password_hash = "new".into();
    assert_eq!(
        db.save_patron(Some(open), stale).await.unwrap(),
        Err(gate_db::repo::patron::SeatConflict::Changed)
    );
    let ids = db.exclusive_token_ids().await.unwrap();
    assert!(!ids.contains("token-a"));
    assert!(ids.contains("token-b"));

    db.unbind_patron_token("token-b").await.unwrap();
    assert!(
        db.patron(OTHER)
            .await
            .unwrap()
            .unwrap()
            .bound_token_id
            .is_none()
    );
    let view = db.member_policy(OTHER).await.unwrap().patron.unwrap();
    assert!(view.enabled);
    assert!(view.queue_jump);
    assert!(view.bound_token_id.is_none());
}

#[tokio::test]
async fn portal_mode_defaults_to_basic_for_legacy_records_and_roundtrips_full() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(dir.path().join("gate.db")).unwrap();
    // 旧记录没有 portal_mode 字段：读出来必须是 basic，而不是报错。
    db.set_setting(
        gate_db::repo::patron::PATRON_KEY,
        &format!(
            r#"{{"profiles":[{{"subject":"{SUBJECT}","enabled":true,"queue_jump":true,"self_service":true,"bound_token_id":null,"password_hash":"scrypt$x$x"}}]}}"#
        ),
    )
    .await
    .unwrap();
    let legacy = db.patron(SUBJECT).await.unwrap().unwrap();
    assert_eq!(legacy.portal_mode, "basic");
    assert!(!legacy.full_portal());
    assert_eq!(
        db.member_policy(SUBJECT)
            .await
            .unwrap()
            .patron
            .unwrap()
            .portal_mode,
        "basic"
    );

    let mut full = legacy.clone();
    full.portal_mode = "full".into();
    db.save_patron(Some(legacy), full).await.unwrap().unwrap();
    let stored = db.patron(SUBJECT).await.unwrap().unwrap();
    assert!(stored.full_portal());
    assert_eq!(
        db.member_policy(SUBJECT)
            .await
            .unwrap()
            .patron
            .unwrap()
            .portal_mode,
        "full"
    );

    let mut bogus = stored.clone();
    bogus.portal_mode = "root".into();
    assert!(db.save_patron(Some(stored), bogus).await.is_err());
}
