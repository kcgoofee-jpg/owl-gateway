use gate_core::clock::Clock;
use gate_db::Db;

#[tokio::test]
async fn member_statistics_roll_at_seven_beijing_time() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(dir.path().join("stats.db")).unwrap();
    let clock = Clock::service();
    let boundary = clock.local_midnight("2026-11-05").unwrap() + 7.0 * 3600.0;
    db.write(move |tx| {
        for ts in [boundary - 0.001, boundary] {
            tx.execute(
                "INSERT INTO usage_log (ts,key_id,kind,status,images) VALUES (?,42,'image','ok',1)",
                [ts],
            )?;
        }
        Ok(())
    })
    .await
    .unwrap();
    let before = db
        .member_request_stats(42, clock.day_start(boundary - 0.001))
        .await
        .unwrap();
    let after = db
        .member_request_stats(42, clock.day_start(boundary))
        .await
        .unwrap();
    // Both rows exist in this fixture: the service-day lower bound changes exactly at 07:00.
    assert_eq!(before.summary.total, 2);
    assert_eq!(after.summary.total, 1);
    assert_eq!(after.images, 1);
    assert_eq!(after.avg_dur_s, None);
}
