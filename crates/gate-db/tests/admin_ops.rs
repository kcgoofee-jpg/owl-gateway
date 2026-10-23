//! 后台运营接口用到的两条查询（`repo/admin_ops.rs`）。

use gate_db::Db;
use gate_db::repo::keys::NewKey;
use gate_db::repo::usage::NewLog;
use serde_json::json;

fn log(up_status: i64) -> NewLog {
    NewLog {
        kind: "image".into(),
        model: "nai-diffusion-4-5-full".into(),
        status: "error".into(),
        up_status,
        ..Default::default()
    }
}

#[tokio::test]
async fn upstream_status_distribution_counts_and_orders() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(dir.path().join("gate.db")).unwrap();
    assert_eq!(db.upstream_status_distribution(0).await.unwrap(), vec![]);
    for (ts, up) in [
        (1000.0, 200),
        (1001.0, 200),
        (1002.0, 200),
        (1003.0, 429),
        (1004.0, 0),
        (900.0, 500),
    ] {
        db.add_log(&log(up), ts).await.unwrap();
    }
    // 次数多的在前；窗口之外（ts < since）的不算
    let rows = db.upstream_status_distribution(1000).await.unwrap();
    assert_eq!(rows[0], (200, 3));
    let mut rest = rows[1..].to_vec();
    rest.sort();
    assert_eq!(rest, vec![(0, 1), (429, 1)]);
    // 窗口放宽后多出 500
    assert_eq!(db.upstream_status_distribution(0).await.unwrap().len(), 4);
}

#[tokio::test]
async fn registered_count_skips_expired_and_deleted_keys() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(dir.path().join("gate.db")).unwrap();
    let mut ids = Vec::new();
    for (i, expires) in [None, Some(5000.0), Some(500.0)].into_iter().enumerate() {
        let key: NewKey = serde_json::from_value(json!({
            "name": format!("m{i}"), "token": format!("nai-synthetic-{i:0>30}"), "daily_images": 5,
            "monthly_anlas": 0.0, "daily_text_tokens": 0, "rpm": 5, "expires_at": expires,
        }))
        .unwrap();
        let created = db.create_key(&key, 1.0).await.unwrap();
        let kid = created["id"].as_i64().unwrap();
        ids.push(kid);
        db.write(move |tx| {
            tx.execute(
                "INSERT INTO discord_registrations(discord_id, key_id, created_at, username, display_name) VALUES (?,?,?,?,?)",
                (format!("90000000000000020{i}"), kid, 1.0_f64, "u", "d"),
            )?;
            Ok(())
        })
        .await
        .unwrap();
    }
    // 没有过期时间 / 还没过期的占名额；已过期的释放
    assert_eq!(db.registered_member_count(1000.0).await.unwrap(), 2);
    assert_eq!(db.registered_member_count(0.0).await.unwrap(), 3);
    assert_eq!(db.registered_member_count(6000.0).await.unwrap(), 1);
    // Key 被删（登记行同时没了或悬空）不再占名额
    db.delete_key(ids[0], 1000.0).await.unwrap();
    assert_eq!(db.registered_member_count(1000.0).await.unwrap(), 1);
}
