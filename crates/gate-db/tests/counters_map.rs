//! `Db::counter_map`：一次取回某服务日全部 Key 的计数，口径与逐把 `get_counter` 一致
//! （`anlas` / `v5` / `legacy_free_images` 扣「重置额度」基线）。后台 `/admin/api/keys` 用它替代 370 次逐把查询。

use gate_db::Db;
use gate_db::repo::keys::NewKey;
use serde_json::json;

async fn open() -> (Db, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(dir.path().join("t.db")).unwrap();
    (db, dir)
}

async fn key(db: &Db, name: &str) -> i64 {
    let k: NewKey = serde_json::from_value(json!({
        "name": name, "token": format!("nai-{name}"), "daily_images": 10, "monthly_anlas": 0.0,
        "daily_text_tokens": 0, "rpm": 5,
    }))
    .unwrap();
    db.create_key(&k, 1_000.0).await.unwrap()["id"]
        .as_i64()
        .unwrap()
}

async fn sql(db: &Db, s: &str) {
    let s = s.to_string();
    db.write(move |tx| tx.execute_batch(&s)).await.unwrap();
}

#[tokio::test]
async fn counter_map_batches_the_same_numbers_as_get_counter() {
    let (db, _d) = open().await;
    let (a, b) = (key(&db, "甲").await, key(&db, "乙").await);
    let day = "2026-10-24";
    sql(
        &db,
        &format!(
            "INSERT INTO counters(key_id, day, images, anlas, text_tokens, requests, v5, legacy_free_images) VALUES \
             ({a}, '{day}', 7, 12.5, 30, 9, 4, 6);\
             INSERT INTO counters(key_id, day, images, anlas, text_tokens, requests, v5, legacy_free_images) VALUES \
             ({b}, '{day}', 3, 1.25, 0, 2, 0, 3);\
             INSERT INTO counters(key_id, day, images, anlas, text_tokens, requests, v5, legacy_free_images) VALUES \
             ({a}, '2026-10-23', 99, 99, 99, 99, 99, 99);"
        ),
    )
    .await;
    // 甲今天重置过额度：V4.5 免费图基线 6、Anlas 12.5、V5 4 全部扣到 0；乙没有 offset
    sql(
        &db,
        &format!(
            "INSERT INTO daily_quota_offsets(key_id, day, anlas, v5, legacy) VALUES ({a}, '{day}', 12.5, 4, 6);"
        ),
    )
    .await;

    let map = db.counter_map(day).await.unwrap();
    assert_eq!(map.len(), 2, "只取当天、只取有计数记录的 Key");
    let ca = &map[&a];
    assert_eq!(
        (
            ca.images,
            ca.legacy_free_images,
            ca.anlas,
            ca.text_tokens,
            ca.requests,
            ca.v5
        ),
        (7, 0, 0.0, 30, 9, 0),
        "重置后：用量原样、可扣的三项归零"
    );
    let cb = &map[&b];
    assert_eq!(
        (
            cb.images,
            cb.legacy_free_images,
            cb.anlas,
            cb.text_tokens,
            cb.requests,
            cb.v5
        ),
        (3, 3, 1.25, 0, 2, 0)
    );
    // 逐把 get_counter 的结果必须逐字段一致（这是替换 N+1 的前提）
    for (key_id, expected) in [(&a, ca), (&b, cb)] {
        assert_eq!(db.get_counter(*key_id, day).await.unwrap(), *expected);
    }
    // 没有计数记录的 Key 不在 map 里（调用方按 0 处理）
    let c = key(&db, "丙").await;
    assert!(!db.counter_map(day).await.unwrap().contains_key(&c));
    assert!(db.counter_map("2026-10-22").await.unwrap().is_empty());
}
