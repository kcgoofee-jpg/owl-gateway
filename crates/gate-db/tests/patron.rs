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
    db.upsert_patron(seat(SUBJECT, Some("token-a")))
        .await
        .unwrap();
    let taken = db.upsert_patron(seat(OTHER, Some("token-a"))).await;
    assert!(taken.is_err(), "{taken:?}");
    db.upsert_patron(seat(OTHER, Some("token-b")))
        .await
        .unwrap();
    let ids = db.exclusive_token_ids().await.unwrap();
    assert!(ids.contains("token-a") && ids.contains("token-b"));

    let mut closed = seat(SUBJECT, Some("token-a"));
    closed.enabled = false;
    db.upsert_patron(closed).await.unwrap();
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
