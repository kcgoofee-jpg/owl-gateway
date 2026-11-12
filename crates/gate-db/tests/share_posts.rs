//! 存图分享记录：同一条消息只记一次，时间用来判断能不能解开今天的闸。

use gate_db::Db;

#[tokio::test]
async fn a_share_is_recorded_once_and_only_counts_from_its_time() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(dir.path().join("shares.db")).unwrap();
    db.record_share(
        "900000000000000005",
        "900000000000000011",
        "900000000000000082",
        100.0,
    )
    .await
    .unwrap();
    db.record_share(
        "900000000000000005",
        "900000000000000011",
        "900000000000000082",
        999.0,
    )
    .await
    .unwrap();
    db.record_share(
        "900000000000000005",
        "900000000000000012",
        "900000000000000082",
        200.0,
    )
    .await
    .unwrap();
    assert!(
        !db.has_share_since("900000000000000005", 250.0)
            .await
            .unwrap()
    );
    assert!(
        db.has_share_since("900000000000000005", 200.0)
            .await
            .unwrap()
    );
    assert!(!db.has_share_since("900000000000000009", 0.0).await.unwrap());
    let at: f64 = db
        .read(|c| {
            c.query_row(
                "SELECT created_at FROM share_posts WHERE message_id='900000000000000011'",
                [],
                |r| r.get(0),
            )
        })
        .await
        .unwrap();
    assert_eq!(at, 100.0);
}
