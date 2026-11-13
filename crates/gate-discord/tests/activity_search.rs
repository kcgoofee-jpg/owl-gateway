//! Real capability handler with synthetic official search responses; no external traffic.
use axum::{
    Json, Router,
    extract::{Request, State},
    http::StatusCode,
    response::IntoResponse,
};
use gate_discord::{
    config::NaimeiConfig,
    events::User,
    naimei::{Naimei, capabilities::ToolRequest},
};
use serde_json::json;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
const G: &str = "900000000000000001";
const U: &str = "900000000000000005";
const B: &str = "900000000000000099";
const CH: &str = "900000000000000004";
async fn api(State(calls): State<Arc<AtomicUsize>>, r: Request) -> impl IntoResponse {
    let p = r.uri().path();
    let mut code = StatusCode::OK;
    let v = if p.ends_with("/messages/search") {
        if calls.fetch_add(1, Ordering::SeqCst) == 0 {
            code = StatusCode::ACCEPTED;
            json!({"retry_after":0,"code":110000})
        } else {
            let at = gate_core::clock::Clock::service()
                .local_midnight("2026-11-13")
                .unwrap()
                - 1000.0;
            let mid = (((at * 1000.0) as u64 - 1420070400000) << 22).to_string();
            json!({"total_results":3,"doing_deep_historical_index":false,"messages":[[{"id":mid,"channel_id":CH,"author":{"id":U},"timestamp":"2026-11-12T15:43:20Z","attachments":[{"filename":"a.png","content_type":"image/png"}]}],[{"id":(mid.parse::<u64>().unwrap()+1).to_string(),"channel_id":"900000000000000007","author":{"id":U},"timestamp":"2026-11-12T15:43:20Z"}],[{"id":(mid.parse::<u64>().unwrap()+2).to_string(),"channel_id":"900000000000000008","author":{"id":U},"timestamp":"2026-11-12T15:43:20Z"}]]})
        }
    } else if p.ends_with("/roles") {
        json!([{"id":G,"permissions":((1u64<<10)|(1<<16)).to_string()}])
    } else if p.contains("/members/") {
        json!({"roles":[]})
    } else if p.ends_with("900000000000000007") {
        json!({"guild_id":G,"type":0,"permission_overwrites":[{"id":G,"type":0,"deny":(1u64<<10).to_string(),"allow":"0"}]})
    } else if p.ends_with("900000000000000008") {
        json!({"guild_id":G,"type":0,"permission_overwrites":[{"id":U,"type":1,"deny":(1u64<<16).to_string(),"allow":"0"}]})
    } else {
        json!({"guild_id":G,"type":0,"permission_overwrites":[]})
    };
    (code, Json(v))
}
#[tokio::test]
async fn indexed_search_filters_private_and_unreadable_channels_and_reports_completeness() {
    let calls = Arc::new(AtomicUsize::new(0));
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    let router = Router::new().fallback(api).with_state(calls.clone());
    let task = tokio::spawn(async move { axum::serve(l, router).await.unwrap() });
    let app = Naimei::new(
        NaimeiConfig {
            token: "synthetic".into(),
            guild_id: G.parse().unwrap(),
            backend: format!("http://{addr}"),
            site: String::new(),
            secret: "synthetic".into(),
            chat: None,
            owner_ids: vec![],
            chat_log: None,
        },
        Some(addr.to_string()),
    );
    app.set_bot_user(User {
        id: B.into(),
        ..User::default()
    });
    let r = ToolRequest {
        actor: U.into(),
        channel: CH.into(),
        name: "activity".into(),
        arguments: json!({}),
    };
    assert_eq!(app.capability(r.clone()).await["data"]["status"], "pending");
    let result = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let v = app.capability(r.clone()).await;
            if v["data"]["status"] == "finished" {
                break v;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(result["data"]["complete"], true);
    assert_eq!(result["data"]["messages"], 1);
    assert_eq!(result["data"]["image_shares"], 1);
    assert_eq!(result["data"]["active_days"], 1);
    assert_eq!(result["data"]["links"].as_array().unwrap().len(), 1);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    let mut other = r;
    other.arguments = json!({"target":"900000000000000006"});
    assert!(
        app.capability(other).await["data"]["error"]
            .as_str()
            .unwrap()
            .contains("仅可查本人")
    );
    task.abort();
}
