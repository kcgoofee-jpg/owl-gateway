//! 成员规则即时生效；不依赖定时同步，也没有凭据级预算豁免。
mod common;
use common::*;
use serde_json::{Value, json};

async fn subscription(e: &Env, token: &str) -> Value {
    let (status, _, body) = Call::new("GET", "/user/subscription")
        .bearer(token)
        .send(&e.state)
        .await;
    assert_eq!(status, 200);
    serde_json::from_slice::<Value>(&body).unwrap()["naiGate"].clone()
}

#[tokio::test]
async fn defaults_and_exceptions_apply_without_a_sync_job() {
    let e = setup(&[]).await;
    let second = member_key(&e.state, 2, &[("discord_id", json!("900000000000000002"))]).await;
    defaults_for(&e.state, json!({"daily_images":120})).await;
    assert_eq!(subscription(&e, TOKEN).await["v45DailyLimit"], 100);
    assert_eq!(
        subscription(&e, "nai-synthetic-0002").await["v45DailyLimit"],
        120
    );
    policy_for(&e.state, second, json!({"daily_images":33})).await;
    defaults_for(&e.state, json!({"daily_images":130})).await;
    assert_eq!(
        subscription(&e, "nai-synthetic-0002").await["v45DailyLimit"],
        33
    );
    policy_for(&e.state, second, json!({"daily_images":null})).await;
    assert_eq!(
        subscription(&e, "nai-synthetic-0002").await["v45DailyLimit"],
        130
    );
    assert!(gate_server::jobs::find("quota").is_none());
}

#[tokio::test]
async fn manual_v5_pause_refuses_every_member_without_dispatch() {
    let e = setup(&[]).await;
    let second = member_key(&e.state, 2, &[("discord_id", json!("900000000000000002"))]).await;
    policy_for(&e.state, second, json!({"v5":true,"daily_v5":20})).await;
    let api = Admin::login_bare(&e.state).await;
    let (_, mut settings) = api.json("GET", "/settings", None).await;
    settings["v5_paused"] = json!(true);
    let (status, response) = api.json("PUT", "/settings", Some(settings.clone())).await;
    assert_eq!(status, 200, "{response}");
    for token in [TOKEN, "nai-synthetic-0002"] {
        let info = subscription(&e, token).await;
        assert_eq!(info["v5Paused"], true);
        assert_eq!(info["v5LeftToday"], 0);
        let (status, _, body) = Call::new("POST", "/ai/generate-image")
            .bearer(token)
            .json(json!({
                "input":"synthetic", "model":"nai-diffusion-5-full", "action":"generate",
                "parameters":{"width":832,"height":1216,"steps":23,"scale":5,"n_samples":1,"seed":1}
            }))
            .send(&e.state)
            .await;
        assert_eq!(status, 403, "{}", String::from_utf8_lossy(&body));
        assert!(String::from_utf8_lossy(&body).contains(gate_core::quota::V5_PAUSED_TEXT));
    }
    assert!(e.mock.requests_to("/ai/generate-image").is_empty());
    settings["v5_paused"] = json!(false);
    assert_eq!(api.json("PUT", "/settings", Some(settings)).await.0, 200);
    assert!(!gate_server::ops::v5_paused(&e.state.db).await.unwrap());
    assert!(
        subscription(&e, TOKEN).await["v5DailyLimit"]
            .as_i64()
            .unwrap()
            > 0
    );
}
