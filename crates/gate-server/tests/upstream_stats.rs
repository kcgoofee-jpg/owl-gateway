//! 每个上游账号的统计与 Anlas 推算：usage_log.token_id 的写入、「上游」页聚合读数里的每账号行、
//! 重启后按账号补回每小时窗口。上游是本地假服务，数据全部合成。

mod common;
use common::{Admin, Call, TOKEN, generate, settle, setup};
use gate_db::repo::usage::{LogFilter, NewLog, Success};
use serde_json::{Value, json};

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
async fn per_account_stats_and_balance() {
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
    // 对账快照：1 天前，每个账号固定 1000、购买 50，10 天后到期；之前账本里 0 条
    let snap = json!({
        "checked_at": now - 86400.0,
        "accounts": {
            a: {"fixed": 1000, "purchased": 50, "expires_at": (now + 10.0 * 86400.0) as i64},
            b: {"fixed": 1000, "purchased": 50, "expires_at": (now + 10.0 * 86400.0) as i64},
        },
        "ledger": {"anlas": 0, "unconfirmed_anlas": 0, "unconfirmed_requests": 0, "last_log_id": 0},
        "balance": 2100,
    });
    db.save_reconciliation(&snap).await.unwrap();
    // a 成功 3 张（各 10 Anlas）、b 成功 1 张；a 还有一次上游 429 的出错和一次选号后被拒
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
    // 老行：没记账号，不能算给任何账号（两个账号时）
    db.record_success(&ok_row(None, 40.0), now - 7200.0)
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
    assert_eq!(fa["w1h"]["timing_samples"], 3);
    assert_eq!(fa["w1h"]["avg_wait_ms"], 200.0);
    assert_eq!(fa["w1h"]["p90_wait_ms"], 400);
    assert_eq!(fa["w1h"]["avg_dur_ms"], 2000.0);
    assert_eq!(fb["w24h"]["ok"], 1);
    assert_eq!(fa["load_share_24h"], 75);
    // 余额：a 用了 30、b 用了 10；老行 40 不归属 → 近似
    assert_eq!(fa["balance"]["fixed"], 970.0);
    assert_eq!(fb["balance"]["fixed"], 990.0);
    assert_eq!(fa["balance"]["purchased"], 50.0);
    assert_eq!(fa["balance"]["approximate"], true);
    assert_eq!(fa["balance"]["unattributed_anlas"], 40.0);
    assert_eq!(fa["balance"]["stale"], false);
    assert_eq!(fa["balance"]["projection"], "waste");
    assert_eq!(fa["balance"]["cycle_rolled"], false);
    assert!((fa["balance"]["days_left"].as_f64().unwrap() - 10.0).abs() < 0.01);
    // 余额占比（1020 : 1040）
    assert_eq!(fa["balance_share"], 50);
}

#[tokio::test]
async fn single_account_counts_untagged_rows_and_rolled_cycle() {
    let e = setup(&[]).await;
    let id = e.state.nai.tokens().await[0].token_id.clone();
    let now = e.state.now();
    let db = &e.state.db;
    db.save_reconciliation(&json!({
        "checked_at": now - 5.0 * 86400.0,
        "accounts": {id.as_str(): {"fixed": 500, "purchased": 0, "expires_at": (now - 3600.0) as i64}},
        "ledger": {"anlas": 0, "unconfirmed_anlas": 0, "unconfirmed_requests": 0, "last_log_id": 0},
        "balance": 500,
    }))
    .await
    .unwrap();
    db.record_success(&ok_row(None, 25.0), now - 60.0)
        .await
        .unwrap();
    let admin = Admin::login_bare(&e.state).await;
    let (_, v) = admin.json("GET", "/upstream", None).await;
    let bal = &v["accounts"][0]["balance"];
    assert_eq!(bal["cycle_rolled"], true);
    assert_eq!(bal["balance"], Value::Null);
    assert_eq!(bal["stale"], true);
    assert_eq!(bal["approximate"], false);
    assert!(v["accounts"][0]["balance_share"].is_null());
}

/// 站付 V5 到期建议：到期 ≤ 3 天且固定 Anlas 剩余 > 300 才提醒；关闭后同一个到期日不再提醒；
/// 已经开了站付 V5 的账号不提醒。
#[tokio::test]
async fn sitepaid_hint_appears_near_expiry_and_can_be_dismissed() {
    let e = setup(&[]).await;
    let id = e.state.nai.tokens().await[0].token_id.clone();
    let now = e.state.now();
    let expires = (now + 2.0 * 86400.0) as i64;
    macro_rules! save {
        ($fixed:expr, $exp:expr) => {
            e.state
                .db
                .save_reconciliation(&json!({
                    "checked_at": now - 60.0,
                    "accounts": {id.as_str(): {"fixed": $fixed, "purchased": 0, "expires_at": $exp}},
                    "ledger": {"anlas": 0, "unconfirmed_anlas": 0, "unconfirmed_requests": 0, "last_log_id": 0},
                    "balance": $fixed,
                }))
                .await
                .unwrap()
        };
    }
    let admin = Admin::login_bare(&e.state).await;
    let hints = || async { admin.json("GET", "/sitepaid-hints", None).await.1 };

    // 余额不够：不提醒
    save!(200, expires);
    assert_eq!(hints().await["hints"], json!([]));
    // 离到期还远：不提醒
    save!(900, (now + 10.0 * 86400.0) as i64);
    assert_eq!(hints().await["hints"], json!([]));
    // 满足条件：提醒
    save!(900, expires);
    let v = hints().await;
    let h = &v["hints"][0];
    assert_eq!(
        (h["token_id"].as_str(), h["manual"].clone()),
        (Some(id.as_str()), json!(true))
    );
    assert_eq!(h["fixed"], 900.0);
    assert_eq!(h["expires_at"], expires as f64);
    assert_eq!(h["setting_key"], "v5_paid_tokens");
    assert!(h["message"].as_str().unwrap().contains("900"));
    // 关闭：同一个到期日不再提醒
    let (s, _) = admin
        .json(
            "POST",
            "/sitepaid-hints/dismiss",
            Some(json!({"token_id": id, "expires_at": expires})),
        )
        .await;
    assert_eq!(s, 200);
    assert_eq!(hints().await["hints"], json!([]));
    // 缺参数是 422
    let (s, _) = admin
        .json(
            "POST",
            "/sitepaid-hints/dismiss",
            Some(json!({"token_id": id})),
        )
        .await;
    assert_eq!(s, 422);
    // 到期日变了（续费）再提醒
    save!(900, expires + 86400);
    assert_eq!(hints().await["hints"].as_array().unwrap().len(), 1);
    // 已经开了站付 V5：不提醒
    e.state
        .db
        .set_setting(
            "v5_paid_tokens",
            &json!({id.as_str(): {"until": now + 86400.0, "max_anlas": 500}}).to_string(),
        )
        .await
        .unwrap();
    assert_eq!(hints().await["hints"], json!([]));
}
