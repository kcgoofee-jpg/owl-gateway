//! 状态页的库层：事件读写、小时计数、上游请求统计（旧版 `app/status_page.py` 里的 SQL）。
//! 与旧版输出逐字节对拍在 gate-server 的 `tests/status_page.rs`；这里测库层自己的约定。

use gate_db::Db;
use gate_db::repo::status_page::{BUCKET_SECS, UpstreamRecent};
use serde_json::json;

const T0: f64 = 1_791_950_400.0; // 2026-10-14 12:00:00 +08:00

fn open() -> (Db, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(dir.path().join("t.db")).unwrap();
    (db, dir)
}

async fn sql(db: &Db, text: String) {
    db.write(move |tx| tx.execute_batch(&text)).await.unwrap();
}

fn usage(ts: f64, key: Option<i64>, up: i64) -> String {
    let key = key.map_or("NULL".to_string(), |k| k.to_string());
    format!(
        "INSERT INTO usage_log(ts, key_id, kind, status, up_status) VALUES ({ts:?}, {key}, 'image', 'x', {up});"
    )
}

#[tokio::test]
async fn incidents_roundtrip_and_reopen() {
    let (db, _d) = open();
    let comps = vec!["api".to_string(), "upstream".to_string()];
    let id = db
        .create_incident("标题", "major", &comps, "investigating", "正文", T0)
        .await
        .unwrap();
    assert_eq!(id, 1);
    // 组件按 json.dumps 的默认格式落库
    let raw: String = db
        .read(|c| c.query_row("SELECT components FROM incidents", [], |r| r.get(0)))
        .await
        .unwrap();
    assert_eq!(raw, r#"["api", "upstream"]"#);

    assert!(
        db.add_incident_update(1, "resolved", "完", T0 + 60.0)
            .await
            .unwrap()
    );
    let list = db.list_incidents(0.0).await.unwrap();
    assert_eq!(list[0].resolved_at, Some(T0 + 60.0));
    assert_eq!(list[0].components, json!(["api", "upstream"]));
    // 最新的更新在前
    assert_eq!(list[0].updates[0].status, "resolved");
    assert_eq!(list[0].updates[1].status, "investigating");
    // 解决后再来一条非 resolved：重新打开
    assert!(
        db.add_incident_update(1, "update", "又来", T0 + 120.0)
            .await
            .unwrap()
    );
    assert_eq!(db.list_incidents(0.0).await.unwrap()[0].resolved_at, None);
    // 不存在的事件：什么都不写
    assert!(!db.add_incident_update(9, "update", "x", T0).await.unwrap());
    let n: i64 = db
        .read(|c| c.query_row("SELECT COUNT(*) FROM incident_updates", [], |r| r.get(0)))
        .await
        .unwrap();
    assert_eq!(n, 3);
}

#[tokio::test]
async fn list_filters_by_resolution_time_and_orders_by_start() {
    let (db, _d) = open();
    let api = vec!["api".to_string()];
    for (title, at, status) in [
        ("很早就解决", T0 - 100.0 * 86400.0, "resolved"),
        ("较早", T0 - 5000.0, "resolved"),
        ("很早开始、至今未解决", T0 - 90.0 * 86400.0, "investigating"),
        ("最新", T0 - 10.0, "investigating"),
    ] {
        db.create_incident(title, "minor", &api, status, "x", at)
            .await
            .unwrap();
    }
    let since = T0 - 30.0 * 86400.0;
    let titles: Vec<String> = db
        .list_incidents(since)
        .await
        .unwrap()
        .into_iter()
        .map(|i| i.title)
        .collect();
    // 结束时间不早于 since 的才算，未解决的一律算；开始时间新的在前
    assert_eq!(titles, ["最新", "较早", "很早开始、至今未解决"]);
}

#[tokio::test]
async fn broken_components_column_is_empty_list() {
    let (db, _d) = open();
    sql(
        &db,
        "INSERT INTO incidents(title, impact, components, started_at) VALUES ('a', 'minor', '{oops', 1.5);\
         INSERT INTO incidents(title, impact, components, started_at) VALUES ('b', 'minor', '', 2.5);"
            .to_string(),
    )
    .await;
    let list = db.list_incidents(0.0).await.unwrap();
    assert!(list.iter().all(|i| i.components == json!([])));
    assert_eq!(list[0].started_at, 2.5);
}

#[tokio::test]
async fn status_hourly_since_filters_by_hour() {
    let (db, _d) = open();
    sql(
        &db,
        "INSERT INTO status_hourly(hour, code, n) VALUES (3600, 200, 5), (7200, 500, 2), (7200, 200, 8);".into(),
    )
    .await;
    let mut rows = db.status_hourly_since(7200).await.unwrap();
    rows.sort();
    assert_eq!(rows, [(7200, 200, 8), (7200, 500, 2)]);
}

#[tokio::test]
async fn upstream_buckets_group_by_quarter_hour_and_ignore_unsent() {
    let (db, _d) = open();
    let base = 900_000.0; // BUCKET_SECS 的整数倍
    assert_eq!(base as i64 % BUCKET_SECS, 0);
    let mut s = String::new();
    s += &usage(base + 10.0, Some(1), 200);
    s += &usage(base + 899.0, Some(1), 500);
    s += &usage(base + 900.0, Some(2), 429);
    s += &usage(base + 901.0, Some(2), 403);
    s += &usage(base + 902.0, Some(2), 404); // 404 不算失败
    s += &usage(base + 903.0, Some(2), 0); // 没发到上游
    s += &usage(base - 1.0, Some(2), 500); // 早于 start
    sql(&db, s).await;
    let rows = db.upstream_buckets_since(base).await.unwrap();
    assert_eq!(rows, [(900_000, 2, 1), (900_900, 3, 2)]);
}

#[tokio::test]
async fn upstream_recent_counts_distinct_failing_keys() {
    let (db, _d) = open();
    let mut s = String::new();
    for i in 0..20 {
        s += &usage(T0 - 3000.0 + f64::from(i), Some(i64::from(i % 3)), 200);
    }
    s += &usage(T0 - 100.0, Some(7), 500);
    s += &usage(T0 - 99.0, Some(7), 429);
    s += &usage(T0 - 98.0, Some(8), 403);
    s += &usage(T0 - 97.0, None, 502); // key_id 为空：计失败，不计 Key 数
    s += &usage(T0 - 96.0, Some(9), 0); // 没发到上游：不算
    s += &usage(T0 - 7200.0, Some(10), 500); // 一小时之外
    sql(&db, s).await;
    assert_eq!(
        db.upstream_recent(T0 - 3600.0).await.unwrap(),
        UpstreamRecent {
            total: 24,
            bad: 4,
            bad_keys: 2
        }
    );
    // 空表：全 0（SUM 为 NULL 也不出错）
    let (empty, _e) = open();
    assert_eq!(
        empty.upstream_recent(0.0).await.unwrap(),
        UpstreamRecent::default()
    );
}
