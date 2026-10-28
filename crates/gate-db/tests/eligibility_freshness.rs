use gate_db::Db;
const USER: &str = "900000000000000001";
const GUILD: &str = "900000000000000010";
const ROLE: &str = "900000000000000011";
#[tokio::test]
async fn verification_is_subject_scoped_and_expires_at_the_explicit_boundary() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(dir.path().join("gate.db")).unwrap();
    db.upsert_role_verification(USER, GUILD, ROLE, 1000.0)
        .await
        .unwrap();
    assert!(
        db.role_verified_since(USER, GUILD, ROLE, 1000.0)
            .await
            .unwrap()
    );
    assert!(
        !db.role_verified_since(USER, GUILD, ROLE, 1000.001)
            .await
            .unwrap()
    );
    assert!(
        !db.role_verified_since("900000000000000002", GUILD, ROLE, 0.0)
            .await
            .unwrap()
    );
    assert!(
        !db.role_verified_since(USER, GUILD, "900000000000000012", 0.0)
            .await
            .unwrap()
    );
}
#[tokio::test]
async fn a_new_failed_verification_cannot_reuse_previous_success() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(dir.path().join("gate.db")).unwrap();
    db.upsert_role_verification(USER, GUILD, ROLE, 1000.0)
        .await
        .unwrap();
    db.clear_role_verifications(USER).await.unwrap();
    assert!(
        !db.role_verified_since(USER, GUILD, ROLE, 0.0)
            .await
            .unwrap()
    );
    db.upsert_role_verification(USER, GUILD, ROLE, 1100.0)
        .await
        .unwrap();
    assert!(
        db.role_verified_since(USER, GUILD, ROLE, 1100.0)
            .await
            .unwrap()
    );
}
