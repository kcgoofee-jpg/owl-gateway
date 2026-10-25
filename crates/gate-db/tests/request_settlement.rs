use gate_db::Db;

#[tokio::test]
async fn durable_dispatch_result_and_close_transitions_are_conservative() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("gate.db");
    let db = Db::open(&path).unwrap();

    db.reserve_request_settlement("request-a", "short-rid", 7, "2026-10-25", 10.0, 1, 1, 1.0)
        .await
        .unwrap();
    assert!(
        db.dispatch_request_settlement("request-a", "2026-10-26", 2.0)
            .await
            .unwrap()
    );
    assert!(
        !db.dispatch_request_settlement("request-a", "2026-10-26", 3.0)
            .await
            .unwrap()
    );
    assert!(
        !db.release_request_reservation("request-a", 4.0)
            .await
            .unwrap()
    );
    assert!(
        db.save_request_result("request-a", r#"{"images":1}"#, 5.0)
            .await
            .unwrap()
    );
    assert!(
        !db.save_request_result("request-a", r#"{"images":1}"#, 6.0)
            .await
            .unwrap()
    );

    // A process restart must observe durable result state and never turn it back into a reservation.
    drop(db);
    let db = Db::open(&path).unwrap();
    let status: String = db
        .read(|c| {
            c.query_row(
                "SELECT status FROM request_settlements WHERE request_id='request-a'",
                [],
                |r| r.get(0),
            )
        })
        .await
        .unwrap();
    assert_eq!(status, "result_saved");

    db.reserve_request_settlement("request-b", "short-rid", 8, "2026-10-25", 10.0, 1, 1, 1.0)
        .await
        .unwrap();
    db.close_request_reservation("request-b", 2.0)
        .await
        .unwrap();
    db.reserve_request_settlement("request-c", "short-rid", 9, "2026-10-25", 10.0, 1, 1, 1.0)
        .await
        .unwrap();
    db.dispatch_request_settlement("request-c", "2026-10-25", 2.0)
        .await
        .unwrap();
    db.close_request_reservation("request-c", 3.0)
        .await
        .unwrap();
    let rows: Vec<(String, String)> = db
        .read(|c| {
            let mut stmt =
                c.prepare("SELECT request_id,status FROM request_settlements ORDER BY request_id")?;
            stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect()
        })
        .await
        .unwrap();
    assert_eq!(
        rows,
        vec![
            ("request-a".into(), "result_saved".into()),
            ("request-b".into(), "released".into()),
            ("request-c".into(), "uncertain".into()),
        ]
    );
}
