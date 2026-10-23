//! 后台「待审」：`abuse_signals` 的 upsert / 重新打开规则、列表结构、确认打标签并只记一条操作日志、鉴权。

mod common;
use common::{Admin, mk_key, setup};
use gate_server::AppState;
use serde_json::{Value, json};

const A: &str = "/admin/api";

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
    assert_eq!(db.count_open_abuse_signals().await.unwrap(), 1);

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
        .list_abuse_signals(Some("open"), 50)
        .await
        .unwrap()
        .iter()
        .map(|r| r["kind"].clone())
        .collect();
    assert_eq!(
        kinds,
        vec![json!("fp_missing"), json!("new_account"), json!("behavior")]
    );
    assert_eq!(db.list_abuse_signals(None, 50).await.unwrap().len(), 4);
    assert_eq!(db.list_abuse_signals(None, 2).await.unwrap().len(), 2);
}

#[tokio::test]
async fn list_shape_and_confirm_tags_keys_once() {
    let e = setup(&[]).await;
    let a = Admin::login(&e.state).await;
    let k1 = mk_key(&e.state, "alice", "nai-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa").await;
    let k2 = mk_key(&e.state, "bob", "nai-bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb").await;
    a.sql(&format!(
        "UPDATE api_keys SET discord_id='111' WHERE id={k1}; \
         INSERT INTO discord_registrations(discord_id, key_id, created_at, display_name) \
         VALUES ('111', {k1}, 0, 'Alice D'); \
         INSERT INTO web_profiles(discord_id, username, display_name, updated_at) VALUES ('222','bobby','',0); \
         UPDATE api_keys SET discord_id='222' WHERE id={k2}; \
         INSERT INTO deleted_key_owners(key_id, discord_id, name, deleted_at) VALUES (77, '333', 'gone', 0);"
    ))
    .await;
    let id = e
        .state
        .db
        .upsert_abuse_signal(
            "device_exact",
            "111,222,333",
            &format!("{k1},{k2},77"),
            88.0,
            &detail("三个号同一台设备"),
            e.state.now(),
        )
        .await
        .unwrap();
    e.state
        .db
        .upsert_abuse_signal("key_sharing", "key:9", "9", 40.0, "not json", 1.0)
        .await
        .unwrap();

    let r = a.get(&format!("{A}/abuse")).await;
    assert_eq!(r.status, 200, "{}", r.text());
    let v = r.json();
    assert_eq!(v["open"], 2);
    let s = &v["signals"][0];
    assert_eq!(s["id"], id);
    assert_eq!(s["kind_label"], "同设备");
    assert_eq!(s["status"], "open");
    assert_eq!(s["detail"]["summary"], "三个号同一台设备");
    assert_eq!(
        s["detail"]["evidence"],
        json!([{"k": "同 IP", "v": "3 个"}])
    );
    assert_eq!(
        s["accounts"],
        json!([
            {"key_id": k1, "name": "alice", "discord_id": "111", "discord_name": "Alice D", "deleted": false},
            {"key_id": k2, "name": "bob", "discord_id": "222", "discord_name": "bobby", "deleted": false},
            {"key_id": 77, "name": "gone", "discord_id": "333", "discord_name": null, "deleted": true},
        ])
    );
    // detail 不是 JSON：原文当 summary
    assert_eq!(
        v["signals"][1]["detail"],
        json!({"summary": "not json", "evidence": []})
    );
    assert_eq!(
        v["signals"][1]["accounts"],
        json!([{"key_id": 9, "name": null, "discord_id": null, "discord_name": null, "deleted": true}])
    );
    a.get(&format!("{A}/abuse?status=bogus"))
        .await
        .assert_detail(422, "status 只能是 open / dismissed / confirmed / all");

    // 确认：状态 + 两把还在的 Key 打标签，一条可读的操作日志
    let r = a
        .post(&format!("{A}/abuse/{id}"), json!({"action": "confirm"}))
        .await;
    assert_eq!(
        r.json(),
        json!({"ok": true, "id": id, "status": "confirmed", "tagged": 2})
    );
    let tags = common::query(
        &e.state.db,
        "SELECT key_id, tag, note, by FROM key_tags ORDER BY key_id",
    )
    .await;
    assert_eq!(
        tags,
        vec![
            json!({"key_id": k1, "tag": "疑似小号", "note": "device_exact", "by": "站长"}),
            json!({"key_id": k2, "tag": "疑似小号", "note": "device_exact", "by": "站长"}),
        ]
    );
    let acts = a.actions().await;
    assert_eq!(acts.len(), 2, "{acts:?}"); // 登录 + 确认
    assert_eq!(acts[0].0, "确认小号");
    assert_eq!(acts[0].1, format!("待审 #{id} 同设备"));
    assert_eq!(
        acts[0].2,
        format!("给 2 把 Key 打「疑似小号」：#{k1} Alice D、#{k2} bobby、#77 gone")
    );
    assert!(acts[0].3);
    assert_eq!(a.get(&format!("{A}/abuse")).await.json()["open"], 1);
    let v = a.get(&format!("{A}/abuse?status=confirmed")).await.json();
    assert_eq!(v["signals"].as_array().unwrap().len(), 1);
    assert_eq!(
        a.get(&format!("{A}/abuse?status=all")).await.json()["signals"]
            .as_array()
            .unwrap()
            .len(),
        2
    );

    // 忽略 / 重开
    let r = a
        .post(&format!("{A}/abuse/{id}"), json!({"action": "reopen"}))
        .await;
    assert_eq!(r.json()["status"], "open");
    let r = a
        .post(&format!("{A}/abuse/{id}"), json!({"action": "dismiss"}))
        .await;
    assert_eq!(r.json()["status"], "dismissed");
    assert_eq!(a.actions().await[0].0, "忽略待审");

    // 错误：未知动作 / 不存在（中间件记一条失败）
    a.post(&format!("{A}/abuse/{id}"), json!({"action": "nuke"}))
        .await
        .assert_detail(422, "action 只能是 dismiss / confirm / reopen");
    a.post(&format!("{A}/abuse/9999"), json!({"action": "dismiss"}))
        .await
        .assert_detail(404, "这条待审不存在");
    let acts = a.actions().await;
    assert_eq!(
        (acts[0].0.as_str(), acts[0].1.as_str(), acts[0].3),
        ("处理待审", "待审 #9999", false)
    );
}
