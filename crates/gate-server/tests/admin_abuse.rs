//! 后台「待审」：`abuse_signals` 的 upsert / 重新打开规则、列表结构、确认打标签并只记一条操作日志、鉴权。

mod common;
use common::setup;
use gate_server::AppState;
use serde_json::{Value, json};

fn detail(summary: &str) -> String {
    json!({"summary": summary, "evidence": [{"k": "同 IP", "v": "3 个"}]}).to_string()
}

async fn status_of(state: &AppState, id: i64) -> String {
    state.db.get_abuse_signal(id).await.unwrap().unwrap()["status"]
        .as_str()
        .unwrap()
        .to_string()
}

#[tokio::test]
async fn upsert_refreshes_and_reopens_only_on_big_jump() {
    let e = setup(&[]).await;
    let db = &e.state.db;
    let id = db
        .upsert_abuse_signal("device_exact", "1,2", "1,2", 60.0, &detail("a"), 100.0)
        .await
        .unwrap();
    // 同 (kind, subject) 再来：同一行，刷新内容
    let again = db
        .upsert_abuse_signal("device_exact", "1,2", "1,2,3", 65.0, &detail("b"), 200.0)
        .await
        .unwrap();
    assert_eq!(id, again);
    let row = db.get_abuse_signal(id).await.unwrap().unwrap();
    assert_eq!(
        (
            &row["key_ids"],
            &row["score"],
            &row["first_ts"],
            &row["last_ts"]
        ),
        (&json!("1,2,3"), &json!(65.0), &json!(100.0), &json!(200.0))
    );
    assert_eq!(db.abuse_signal_counts(None).await.unwrap().open, 1);

    // 已忽略：涨不到 10 分不重开，涨够了重开
    assert!(db.set_abuse_status(id, "dismissed").await.unwrap());
    db.upsert_abuse_signal("device_exact", "1,2", "1,2", 74.0, &detail("c"), 300.0)
        .await
        .unwrap();
    assert_eq!(status_of(&e.state, id).await, "dismissed");
    db.upsert_abuse_signal("device_exact", "1,2", "1,2", 84.0, &detail("d"), 400.0)
        .await
        .unwrap();
    assert_eq!(status_of(&e.state, id).await, "open");

    // 已确认：不管分数怎么变都保持
    db.set_abuse_status(id, "confirmed").await.unwrap();
    db.upsert_abuse_signal("device_exact", "1,2", "1,2", 100.0, &detail("e"), 500.0)
        .await
        .unwrap();
    assert_eq!(status_of(&e.state, id).await, "confirmed");
    assert!(!db.set_abuse_status(9999, "open").await.unwrap());

    // 排序：分高在前，同分按最近命中
    db.upsert_abuse_signal("behavior", "3,4", "", 50.0, "{}", 10.0)
        .await
        .unwrap();
    db.upsert_abuse_signal("new_account", "5", "", 50.0, "{}", 20.0)
        .await
        .unwrap();
    db.upsert_abuse_signal("fp_missing", "6", "", 90.0, "{}", 1.0)
        .await
        .unwrap();
    let kinds: Vec<Value> = db
        .list_abuse_signals(&gate_db::repo::abuse::AbuseFilter {
            status: Some("open".into()),
            limit: 50,
            ..Default::default()
        })
        .await
        .unwrap()
        .iter()
        .map(|r| r["kind"].clone())
        .collect();
    assert_eq!(
        kinds,
        vec![json!("fp_missing"), json!("new_account"), json!("behavior")]
    );
    assert_eq!(
        db.list_abuse_signals(&gate_db::repo::abuse::AbuseFilter {
            limit: 50,
            ..Default::default()
        })
        .await
        .unwrap()
        .len(),
        4
    );
    assert_eq!(
        db.list_abuse_signals(&gate_db::repo::abuse::AbuseFilter {
            limit: 2,
            ..Default::default()
        })
        .await
        .unwrap()
        .len(),
        2
    );
}
