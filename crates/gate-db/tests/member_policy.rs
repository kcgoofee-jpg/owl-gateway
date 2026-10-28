use gate_core::member_policy::{MemberOverrides, MemberPolicy};
use gate_db::Db;
use gate_db::repo::keys::NewKey;
use serde_json::json;

const SUBJECT: &str = "900000000000000001";

fn defaults() -> MemberPolicy {
    MemberPolicy {
        daily_images: 150,
        daily_v5: 8,
        daily_anlas: 15,
        monthly_anlas: 99999.0,
        rpm: 10,
        image: true,
        v5: false,
        vibe: true,
        tags: true,
        upscale: false,
        augment: false,
        img2img: false,
        paid_parameters: false,
    }
}

#[tokio::test]
async fn defaults_apply_immediately_and_zero_exceptions_can_be_cleared() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(dir.path().join("gate.db")).unwrap();
    let mut policy = defaults();
    db.set_member_defaults(&policy).await.unwrap();
    db.set_member_overrides(
        SUBJECT,
        &MemberOverrides {
            daily_images: Some(150),
            daily_v5: Some(0),
            v5: Some(true),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let snapshot = db.member_policy(SUBJECT).await.unwrap();
    assert_eq!(snapshot.overrides.daily_images, None);
    assert_eq!(snapshot.effective.daily_v5, 0);
    policy.daily_images = 200;
    policy.daily_v5 = 12;
    db.set_member_defaults(&policy).await.unwrap();
    let snapshot = db.member_policy(SUBJECT).await.unwrap();
    assert_eq!(snapshot.effective.daily_images, 200);
    assert_eq!(snapshot.effective.daily_v5, 0);
    assert!(snapshot.effective.v5);
    db.set_member_overrides(SUBJECT, &MemberOverrides::default())
        .await
        .unwrap();
    assert_eq!(db.member_policy(SUBJECT).await.unwrap().effective, policy);
}

#[tokio::test]
async fn key_rotation_and_replacement_do_not_move_member_permissions() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(dir.path().join("gate.db")).unwrap();
    db.set_member_defaults(&defaults()).await.unwrap();
    db.set_member_overrides(
        SUBJECT,
        &MemberOverrides {
            v5: Some(true),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let key: NewKey = serde_json::from_value(json!({
        "name":"synthetic", "token":"nai-synthetic-first", "discord_id":SUBJECT,
        "daily_images":150,"monthly_anlas":99999,"daily_text_tokens":0,"rpm":10
    }))
    .unwrap();
    let id = db.create_key(&key, 1.0).await.unwrap()["id"]
        .as_i64()
        .unwrap();
    let before = db.member_policy(SUBJECT).await.unwrap();
    assert!(
        db.rotate_key_token(id, "nai-synthetic-replacement")
            .await
            .unwrap()
    );
    assert_eq!(db.member_policy(SUBJECT).await.unwrap(), before);
    assert!(
        !db.member_policy("900000000000000002")
            .await
            .unwrap()
            .effective
            .v5
    );
}

#[tokio::test]
async fn corrupt_defaults_or_exceptions_fail_closed_and_bad_writes_are_atomic() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(dir.path().join("gate.db")).unwrap();
    assert_eq!(
        db.member_policy(SUBJECT).await.unwrap().effective,
        MemberPolicy::initial()
    );
    db.run_script(vec![
        "DELETE FROM site_settings WHERE key='member_policy_defaults'".into(),
    ])
    .await
    .unwrap();
    assert!(db.member_policy(SUBJECT).await.is_err());
    db.set_member_defaults(&defaults()).await.unwrap();
    assert!(
        db.set_member_overrides(
            SUBJECT,
            &MemberOverrides {
                daily_images: Some(10000),
                ..Default::default()
            }
        )
        .await
        .is_err()
    );
    assert_eq!(
        db.member_policy(SUBJECT).await.unwrap().effective,
        defaults()
    );
    db.run_script(vec![format!(
        "INSERT INTO member_rules(discord_id,overrides) VALUES('{SUBJECT}','invalid')"
    )])
    .await
    .unwrap();
    assert!(db.member_policy(SUBJECT).await.is_err());
    assert!(db.member_policy("").await.is_err());
    assert!(db.member_policy("0001").await.is_err());
}

#[tokio::test]
async fn matching_exceptions_merge_when_defaults_change() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(dir.path().join("gate.db")).unwrap();
    let mut policy = defaults();
    db.set_member_defaults(&policy).await.unwrap();
    db.set_member_overrides(
        SUBJECT,
        &MemberOverrides {
            daily_images: Some(200),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    policy.daily_images = 200;
    db.set_member_defaults(&policy).await.unwrap();
    assert_eq!(
        db.member_policy(SUBJECT)
            .await
            .unwrap()
            .overrides
            .daily_images,
        None
    );
    policy.daily_images = 250;
    db.set_member_defaults(&policy).await.unwrap();
    assert_eq!(
        db.member_policy(SUBJECT)
            .await
            .unwrap()
            .effective
            .daily_images,
        250
    );
}
