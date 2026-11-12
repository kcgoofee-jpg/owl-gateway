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
