use gate_db::Db;
use gate_db::repo::keys::NewKey;
use gate_db::repo::usage::Success;
use serde_json::json;

#[tokio::test]
async fn identical_short_rid_links_only_to_its_internal_request_and_owner() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(dir.path().join("gate.db")).unwrap();
    let create = |name: &str, token: &str| -> NewKey {
        serde_json::from_value(
            json!({"name":name,"token":token,"daily_images":10,"monthly_anlas":0.0,"daily_text_tokens":1000,"rpm":60}),
        )
        .unwrap()
    };
    let key1 = db
        .create_key(&create("one", "nai-owner-one-000000000000000000"), 1.0)
        .await
        .unwrap()["id"]
        .as_i64()
        .unwrap();
    let key2 = db
        .create_key(&create("two", "nai-owner-two-000000000000000000"), 1.0)
        .await
        .unwrap()["id"]
        .as_i64()
        .unwrap();
    for (id, key_id) in [("internal-one", key1), ("internal-two", key2)] {
        db.reserve_request_settlement(
            id,
            "same-rid",
            key_id,
            "2026-10-25",
            "2026-10",
            false,
            false,
            1.0,
            0,
            1,
            1.0,
        )
        .await
        .unwrap();
        db.dispatch_request_settlement(id, "2026-10-25", "2026-10", false, false, 1.0, 0, 1, 2.0)
            .await
            .unwrap();
        let success = Success {
            key_id,
            key_name: format!("owner-{key_id}"),
            kind: "image".into(),
            model: "model".into(),
            day: "2026-10-25".into(),
            images: 1,
            anlas: 1.0,
            rid: "same-rid".into(),
            ..Default::default()
        };
        db.record_success_settled(&success, id, 3.0).await.unwrap();
    }
    let first = db.member_logs(key1, 0.0, 0, "", 10).await.unwrap();
    let second = db.member_logs(key2, 0.0, 0, "", 10).await.unwrap();
    assert_eq!(first.len(), 1);
    assert_eq!(second.len(), 1);
    assert_eq!(first[0]["rid"], "same-rid");
    assert_eq!(second[0]["rid"], "same-rid");
    assert_eq!(first[0]["id"], 1);
    assert_eq!(second[0]["id"], 2);
}

#[tokio::test]
async fn recovery_commits_saved_success_once_and_retains_unknown_budget() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("gate.db");
    let db = Db::open(&path).unwrap();
    let key: NewKey = serde_json::from_value(json!({
        "name":"settlement test", "token":"nai-settlement-test-key-0000000000",
        "daily_images":10, "monthly_anlas":1000.0, "daily_text_tokens":1000, "rpm":60
    }))
    .unwrap();
    let key_id = db.create_key(&key, 1.0).await.unwrap()["id"]
        .as_i64()
        .unwrap();

    db.reserve_request_settlement(
        "known",
        "rid-known",
        key_id,
        "2026-10-25",
        "2026-10",
        false,
        false,
        10.0,
        1,
        1,
        1.0,
    )
    .await
    .unwrap();
    assert!(
        db.dispatch_request_settlement(
            "known",
            "2026-10-26",
            "2026-10",
            false,
            false,
            10.0,
            1,
            1,
            2.0
        )
        .await
        .unwrap()
    );
    let success = Success {
        key_id,
        key_name: "settlement test".into(),
        kind: "image".into(),
        model: "nai-diffusion-4-5-full".into(),
        day: "2026-10-26".into(),
        images: 1,
        anlas: 10.0,
        legacy_free_images: 1,
        rid: "rid-known".into(),
        ..Default::default()
    };
    let payload = serde_json::to_string(&success).unwrap();
    assert!(
        db.save_request_result("known", &payload, 2.5, 3.0)
            .await
            .unwrap()
    );

    let mut fault_success = success.clone();
    fault_success.rid = "rid-fault".into();
    let fault_payload = serde_json::to_string(&fault_success).unwrap();
    db.reserve_request_settlement(
        "fault",
        "rid-fault",
        key_id,
        "2026-10-26",
        "2026-10",
        false,
        false,
        10.0,
        0,
        1,
        3.1,
    )
    .await
    .unwrap();
    db.dispatch_request_settlement(
        "fault",
        "2026-10-26",
        "2026-10",
        false,
        false,
        10.0,
        0,
        1,
        3.2,
    )
    .await
    .unwrap();
    db.save_request_result("fault", &fault_payload, 3.1, 3.3)
        .await
        .unwrap();
    db.run_script(vec!["CREATE TRIGGER reject_success BEFORE INSERT ON usage_log BEGIN SELECT RAISE(ABORT, 'injected write failure'); END".into()]).await.unwrap();
    assert!(
        db.commit_saved_request_result("fault", &fault_payload, 3.4)
            .await
            .is_err()
    );
    db.run_script(vec!["DROP TRIGGER reject_success".into()])
        .await
        .unwrap();
    let status: String = db
        .read(|c| {
            c.query_row(
                "SELECT status FROM request_settlements WHERE request_id='fault'",
                [],
                |r| r.get(0),
            )
        })
        .await
        .unwrap();
    assert_eq!(status, "result_saved");

    db.reserve_request_settlement(
        "unknown",
        "rid-unknown",
        key_id,
        "2026-10-26",
        "2026-10",
        true,
        false,
        20.0,
        2,
        2,
        4.0,
    )
    .await
    .unwrap();
    db.dispatch_request_settlement(
        "unknown",
        "2026-10-26",
        "2026-10",
        true,
        false,
        20.0,
        2,
        2,
        5.0,
    )
    .await
    .unwrap();
    db.reserve_request_settlement(
        "not-sent",
        "rid-not-sent",
        key_id,
        "2026-10-26",
        "2026-10",
        false,
        false,
        1.0,
        0,
        1,
        6.0,
    )
    .await
    .unwrap();

    drop(db);
    let db = Db::open(&path).unwrap();
    let recovery = db.recover_request_settlements(7.0).await.unwrap();
    assert_eq!(
        recovery
            .iter()
            .find(|r| r.request_id == "known")
            .unwrap()
            .status,
        "result_saved"
    );
    assert_eq!(
        recovery
            .iter()
            .filter(|r| r.status == "result_saved")
            .count(),
        2
    );
    db.commit_saved_request_result("known", &payload, 8.0)
        .await
        .unwrap();
    db.commit_saved_request_result("known", &payload, 9.0)
        .await
        .unwrap();
    db.commit_saved_request_result("fault", &fault_payload, 9.1)
        .await
        .unwrap();
    let log_link: (String, f64) = db
        .read(|c| {
            c.query_row(
                "SELECT request_id,ts FROM usage_log JOIN usage_request_links ON usage_log.id=usage_log_id WHERE rid='rid-known'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
        })
        .await
        .unwrap();
    assert_eq!(log_link, ("known".into(), 2.5));
    db.set_request_delivery("known", "transport_complete", 9.2)
        .await
        .unwrap();
    let member_log = db.member_logs(key_id, 0.0, 0, "", 10).await.unwrap();
    assert_eq!(
        member_log.iter().find(|r| r["rid"] == "rid-known").unwrap()["delivery_state"],
        "transport_complete"
    );
    let counter = db.get_counter(key_id, "2026-10-26").await.unwrap();
    assert_eq!((counter.images, counter.legacy_free_images), (2, 2));

    let recovery = db.recover_request_settlements(10.0).await.unwrap();
    let unknown = recovery.iter().find(|r| r.request_id == "unknown").unwrap();
    assert_eq!(unknown.status, "uncertain");
    assert_eq!(
        (
            unknown.day.as_str(),
            unknown.month.as_str(),
            unknown.exclude_global_v5,
            unknown.anlas,
            unknown.v5,
            unknown.legacy
        ),
        ("2026-10-26", "2026-10", true, 20.0, 2, 2)
    );
    assert!(!recovery.iter().any(|r| r.request_id == "not-sent"));
    let rows: Vec<(String, String)> = db
        .read(|c| {
            let mut s =
                c.prepare("SELECT request_id,status FROM request_settlements ORDER BY request_id")?;
            s.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect()
        })
        .await
        .unwrap();
    assert_eq!(
        rows,
        vec![
            ("fault".into(), "committed".into()),
            ("known".into(), "committed".into()),
            ("not-sent".into(), "released".into()),
            ("unknown".into(), "uncertain".into())
        ]
    );

    assert!(
        db.manually_release_unknown_request(
            "unknown",
            "site admin",
            "Verified no image was generated",
            11.0
        )
        .await
        .unwrap()
    );
    assert!(
        !db.manually_release_unknown_request("unknown", "site admin", "duplicate resolution", 12.0)
            .await
            .unwrap()
    );
    let actions: i64 = db
        .read(|c| {
            c.query_row(
                "SELECT COUNT(*) FROM admin_actions WHERE action='request_settlement_release'",
                [],
                |r| r.get(0),
            )
        })
        .await
        .unwrap();
    assert_eq!(actions, 1);
}
