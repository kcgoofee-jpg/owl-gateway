use gate_db::Db;
use gate_db::repo::keys::NewKey;
use gate_db::repo::usage::{NewLog, Success};
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
            1.0,
            0,
            1,
            1.0,
        )
        .await
        .unwrap();
        db.dispatch_request_settlement(
            id,
            "ABC123",
            "11deadbeef",
            "2026-10-25",
            "2026-10",
            false,
            1.0,
            0,
            1,
            2.0,
        )
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
            "ABC123",
            "11deadbeef",
            "2026-10-26",
            "2026-10",
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
        10.0,
        0,
        1,
        3.1,
    )
    .await
    .unwrap();
    db.dispatch_request_settlement(
        "fault",
        "ABC123",
        "11deadbeef",
        "2026-10-26",
        "2026-10",
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
    let fault_payload_after_failure: Option<String> = db
        .read(|c| {
            c.query_row(
                "SELECT result_json FROM request_settlements WHERE request_id='fault'",
                [],
                |r| r.get(0),
            )
        })
        .await
        .unwrap();
    assert_eq!(
        fault_payload_after_failure.as_deref(),
        Some(fault_payload.as_str())
    );

    db.reserve_request_settlement(
        "unknown",
        "rid-unknown",
        key_id,
        "2026-10-26",
        "2026-10",
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
        "ABC123",
        "11deadbeef",
        "2026-10-26",
        "2026-10",
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
    let payload_after_commit: Option<String> = db
        .read(|c| {
            c.query_row(
                "SELECT result_json FROM request_settlements WHERE request_id='known'",
                [],
                |r| r.get(0),
            )
        })
        .await
        .unwrap();
    assert!(
        payload_after_commit.is_none(),
        "success payload is removed after atomic commit"
    );
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
            unknown.anlas,
            unknown.v5,
            unknown.legacy
        ),
        ("2026-10-26", "2026-10", 20.0, 2, 2)
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

#[tokio::test]
async fn unknown_failure_usage_log_is_linked_by_internal_id_and_exposes_remaining_budget() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(dir.path().join("gate.db")).unwrap();
    let key: NewKey = serde_json::from_value(json!({
        "name":"unknown owner", "token":"nai-unknown-owner-test-key-00000000",
        "daily_images":10, "monthly_anlas":1000.0, "daily_text_tokens":1000, "rpm":60
    }))
    .unwrap();
    let key_id = db.create_key(&key, 1.0).await.unwrap()["id"]
        .as_i64()
        .unwrap();
    db.reserve_request_settlement(
        "internal-unknown",
        "short-rid",
        key_id,
        "2026-10-25",
        "2026-10",
        false,
        4.0,
        1,
        1,
        1.0,
    )
    .await
    .unwrap();
    db.dispatch_request_settlement(
        "internal-unknown",
        "ABC123",
        "11deadbeef",
        "2026-10-25",
        "2026-10",
        false,
        4.0,
        1,
        1,
        2.0,
    )
    .await
    .unwrap();
    db.record_settled_failure(
        &NewLog {
            key_id: Some(key_id),
            key_name: "unknown owner".into(),
            kind: "image".into(),
            model: "nai-diffusion-4-5-full".into(),
            status: "error".into(),
            up_status: 500,
            rid: "short-rid".into(),
            detail: "500 执行结果未知".into(),
            ..Default::default()
        },
        "internal-unknown",
        true,
        3.0,
    )
    .await
    .unwrap();
    let rows = db.member_logs(key_id, 0.0, 0, "", 10).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["settlement_status"], "uncertain");
    assert_eq!(rows[0]["remaining_anlas"], 4.0);
    assert_eq!(rows[0]["remaining_v5"], 1);
    assert_eq!(rows[0]["remaining_legacy"], 1);
}

#[tokio::test]
async fn upstream_anlas_budget_serializes_reservations_and_retains_unknown_results() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(dir.path().join("gate.db")).unwrap();
    let day = "2026-11-13";
    for id in ["one", "two", "three"] {
        db.reserve_request_settlement(id, id, 1, day, "2026-11", false, 3.0, 0, 0, 1.0)
            .await
            .unwrap();
        db.dispatch_request_settlement(id, "abc", id, day, "2026-11", false, 3.0, 0, 0, 2.0)
            .await
            .unwrap();
    }
    let (one, two) = tokio::join!(
        db.reserve_upstream_anlas("one", "share-a", day, 5, true),
        db.reserve_upstream_anlas("two", "share-a", day, 5, true)
    );
    assert_ne!(one.unwrap(), two.unwrap());
    let held = if db.upstream_anlas_used("share-a", day).await.unwrap() == 3.0 {
        "one"
    } else {
        panic!()
    };
    // Both IDs can be tested idempotently: the admitted one succeeds, the other still fails.
    let admitted = if db
        .reserve_upstream_anlas(held, "share-a", day, 5, true)
        .await
        .unwrap()
    {
        held
    } else {
        "two"
    };
    assert!(
        db.reserve_upstream_anlas(admitted, "share-a", day, 5, true)
            .await
            .unwrap()
    );
    assert!(
        db.reserve_upstream_anlas("three", "share-b", day, 5, true)
            .await
            .unwrap()
    );
    assert!(
        !db.reserve_upstream_anlas("three", "share-c", day, 5, false)
            .await
            .unwrap()
    );
    db.record_settled_failure(&NewLog::default(), admitted, true, 3.0)
        .await
        .unwrap();
    db.recover_request_settlements(4.0).await.unwrap();
    assert_eq!(db.upstream_anlas_used("share-a", day).await.unwrap(), 3.0);
    assert!(
        db.manually_release_unknown_request(admitted, "admin", "confirmed uncharged", 5.0)
            .await
            .unwrap()
    );
    assert_eq!(db.upstream_anlas_used("share-a", day).await.unwrap(), 0.0);
    db.record_settled_failure(&NewLog::default(), "three", false, 6.0)
        .await
        .unwrap();
    assert_eq!(db.upstream_anlas_used("share-b", day).await.unwrap(), 0.0);
    assert_eq!(
        db.upstream_anlas_used("share-a", "2026-11-14")
            .await
            .unwrap(),
        0.0
    );
}

#[tokio::test]
async fn retry_saved_success_commits_once_without_replacing_the_result() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(dir.path().join("gate.db")).unwrap();
    let key: NewKey =
        serde_json::from_value(json!({"name":"synthetic", "token":"nai-synthetic-retry-key"}))
            .unwrap();
    let id = db.create_key(&key, 1.0).await.unwrap()["id"]
        .as_i64()
        .unwrap();
    db.reserve_request_settlement(
        "retry",
        "rid",
        id,
        "2026-10-25",
        "2026-10",
        false,
        0.0,
        0,
        1,
        1.0,
    )
    .await
    .unwrap();
    db.dispatch_request_settlement(
        "retry",
        "correlation",
        "rid",
        "2026-10-25",
        "2026-10",
        false,
        0.0,
        0,
        1,
        2.0,
    )
    .await
    .unwrap();
    let success = Success {
        key_id: id,
        key_name: "synthetic".into(),
        kind: "image".into(),
        day: "2026-10-25".into(),
        images: 1,
        legacy_free_images: 1,
        ..Default::default()
    };
    db.run_script(vec!["CREATE TRIGGER reject_retry BEFORE INSERT ON usage_log BEGIN SELECT RAISE(ABORT, 'injected failure'); END".into()]).await.unwrap();
    assert!(
        db.record_success_settled(&success, "retry", 3.0)
            .await
            .is_err()
    );
    db.run_script(vec!["DROP TRIGGER reject_retry".into()])
        .await
        .unwrap();
    let mut different = success.clone();
    different.images = 2;
    assert!(
        db.record_success_settled(&different, "retry", 4.0)
            .await
            .is_err()
    );
    assert!(
        db.commit_saved_request_result("retry", &serde_json::to_string(&different).unwrap(), 4.0)
            .await
            .is_err()
    );
    db.record_success_settled(&success, "retry", 5.0)
        .await
        .unwrap();
    db.record_success_settled(&success, "retry", 6.0)
        .await
        .unwrap();
    let counter = db.get_counter(id, "2026-10-25").await.unwrap();
    assert_eq!(counter.images, 1);
    let logs = db.member_logs(id, 0.0, 0, "", 10).await.unwrap();
    assert_eq!(logs.len(), 1);
    assert_eq!(logs[0]["ts"], 3.0);
}

#[tokio::test]
async fn in_flight_free_v5_follows_token_replacement_and_counts_once() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(dir.path().join("gate.db")).unwrap();
    let key: NewKey =
        serde_json::from_value(json!({"name":"synthetic","token":"nai-synthetic-key"})).unwrap();
    let id = db.create_key(&key, 1.0).await.unwrap()["id"]
        .as_i64()
        .unwrap();
    let day = "2026-11-14";
    db.reserve_request_settlement("switch", "rid", id, day, "2026-11", false, 0.0, 1, 0, 1.0)
        .await
        .unwrap();
    assert!(
        db.dispatch_request_settlement_with_upstream(
            "switch",
            "corr",
            "rid",
            day,
            "2026-11",
            false,
            0.0,
            1,
            0,
            2.0,
            Some(("old-token", true))
        )
        .await
        .unwrap()
    );
    db.move_upstream_token("old-token", "new-token")
        .await
        .unwrap();
    assert_eq!(
        db.upstream_v5_used_and_held("new-token", day)
            .await
            .unwrap(),
        1
    );
    let success = Success {
        key_id: id,
        key_name: "synthetic".into(),
        kind: "image".into(),
        day: day.into(),
        images: 1,
        v5: 1,
        token_id: Some("old-token".into()),
        ..Default::default()
    };
    db.record_success_settled(&success, "switch", 3.0)
        .await
        .unwrap();
    db.record_success_settled(&success, "switch", 4.0)
        .await
        .unwrap();
    assert_eq!(
        db.get_upstream_counter("new-token", day).await.unwrap().v5,
        1
    );
    assert_eq!(
        db.upstream_v5_used_and_held("new-token", day)
            .await
            .unwrap(),
        1
    );
}
