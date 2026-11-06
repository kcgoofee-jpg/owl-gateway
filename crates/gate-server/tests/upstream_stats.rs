//! 每个上游账号的统计与 Anlas 推算：usage_log.token_id 的写入、「上游」页聚合读数里的每账号行、
//! 重启后按账号补回每小时窗口。上游是本地假服务，数据全部合成。

mod common;
use common::{Admin, Call, TOKEN, generate, settle, setup};
use gate_db::repo::usage::{LogFilter, NewLog, Success};
use serde_json::json;

const TWO_TOKENS: &str = "pst-synthetic-upstream-a,pst-synthetic-upstream-b";

fn ok_row(token: Option<&str>, anlas: f64) -> Success {
    Success {
        key_id: 1,
        key_name: "测试成员".into(),
        kind: "image".into(),
        model: "nai-diffusion-4-5-full".into(),
        images: 1,
        anlas,
        wait_ms: 100,
        dur_ms: 2000,
        token_id: token.map(str::to_string),
        ..Default::default()
    }
}

#[tokio::test]
async fn image_request_records_the_serving_account() {
    let e = setup(&[]).await;
    assert_eq!(generate(&e.state, TOKEN).await.0, 200);
    settle().await;
    let rows = e
        .state
        .db
        .list_logs(10, 0, &LogFilter::default())
        .await
        .unwrap();
    let id = e.state.nai.tokens().await[0].token_id.clone();
    assert_eq!(rows[0]["token_id"], id);
}

#[tokio::test]
async fn rejected_before_upstream_has_no_account() {
    let e = setup(&[]).await;
    // 没有的模型：准入阶段就被拒，没走到选号
    let (status, _, _) = Call::new("POST", "/ai/generate-image")
        .bearer(TOKEN)
        .json(json!({"input": "x", "model": "no-such-model", "parameters": {}}))
        .send(&e.state)
        .await;
    assert!(status.is_client_error());
    settle().await;
    let rows = e
        .state
        .db
        .list_logs(10, 0, &LogFilter::default())
        .await
        .unwrap();
    assert!(rows.iter().all(|r| r["token_id"].is_null()), "{rows:?}");
}

#[tokio::test]
async fn stats_endpoint_needs_admin() {
    let e = setup(&[]).await;
    let (status, _, _) = Call::new("GET", "/admin/api/upstream").send(&e.state).await;
    assert_eq!(status, 401);
    let (status, _, _) = Call::new("GET", "/admin/api/upstream")
        .bearer(TOKEN)
        .send(&e.state)
        .await;
    assert_ne!(status, 200, "成员 Key 不能看");
    let admin = Admin::login_bare(&e.state).await;
    let (_, v) = admin.json("GET", "/upstream", None).await;
    let w = &v["accounts"][0]["w1h"];
    assert_eq!(w["timing_samples"], 0);
    assert!(w["avg_wait_ms"].is_null());
    assert!(w["p90_wait_ms"].is_null());
    assert!(w["avg_dur_ms"].is_null());
}

#[tokio::test]
async fn per_account_stats_read_balance_from_snapshots() {
    let e = setup(&[("NAI_TOKENS", TWO_TOKENS)]).await;
    let ids: Vec<String> = e
        .state
        .nai
        .tokens()
        .await
        .iter()
        .map(|t| t.token_id.clone())
        .collect();
    let (a, b) = (ids[0].as_str(), ids[1].as_str());
    let now = e.state.now();
    let db = &e.state.db;
    // 上游快照只记实测读数：a 剩 970、b 剩 990（1 天前读的，所以不算 stale）
    db.insert_upstream_snapshot(now - 60.0, a, Some(4.0), None, Some(970), 1)
        .await
        .unwrap();
    db.insert_upstream_snapshot(now - 60.0, b, Some(4.0), None, Some(990), 1)
        .await
        .unwrap();
    // a 成功 3 张、b 成功 1 张；a 还有一次上游 429 的出错和一次选号后被拒
    for i in 0..3 {
        let mut s = ok_row(Some(a), 10.0);
        s.rid = format!("a{i}");
        s.wait_ms = i * 200;
        s.dur_ms = (i + 1) * 1000;
        db.record_success(&s, now - 60.0).await.unwrap();
    }
    db.record_success(&ok_row(Some(b), 10.0), now - 60.0)
        .await
        .unwrap();
    db.add_log(
        &NewLog {
            key_id: Some(1),
            kind: "image".into(),
            status: "error".into(),
            up_status: 429,
            token_id: Some(a.into()),
            ..Default::default()
        },
        now - 30.0,
    )
    .await
    .unwrap();
    db.add_log(
        &NewLog {
            key_id: Some(1),
            kind: "image".into(),
            status: "rejected".into(),
            token_id: Some(a.into()),
            ..Default::default()
        },
        now - 20.0,
    )
    .await
    .unwrap();

    let admin = Admin::login_bare(&e.state).await;
    let (status, v) = admin.json("GET", "/upstream", None).await;
    assert_eq!(status, 200);
    let accts = v["accounts"].as_array().unwrap();
    assert_eq!(accts.len(), 2);
    let fa = accts.iter().find(|x| x["token_id"] == a).unwrap();
    let fb = accts.iter().find(|x| x["token_id"] == b).unwrap();
    assert_eq!(fa["w1h"]["ok"], 3);
    assert_eq!(fa["w1h"]["error"], 1);
    assert_eq!(fa["w1h"]["rejected"], 1);
    assert_eq!(fa["w1h"]["up_429"], 1);
    assert_eq!(fa["w1h"]["avg_wait_ms"], 200.0);
    assert_eq!(fa["w1h"]["p90_wait_ms"], 400);
    assert_eq!(fa["w1h"]["avg_dur_ms"], 2000.0);
    assert_eq!(fb["w24h"]["ok"], 1);
    assert_eq!(fa["load_share_24h"], 75);
    // 余额直接是快照里的实测读数，不再按账本推算
    assert_eq!(fa["balance"]["balance"], 970.0);
    assert_eq!(fb["balance"]["balance"], 990.0);
    assert_eq!(fa["balance"]["v5_percent"], 4.0);
    assert_eq!(fa["balance"]["stale"], false);
    assert_eq!(fa["balance_share"], 49);
}
