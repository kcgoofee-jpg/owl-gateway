use axum::{Json, Router, extract::Request};
use gate_discord::{
    config::NaimeiConfig,
    events::User,
    naimei::{
        Naimei,
        capabilities::{ToolRequest, render},
    },
};
use serde_json::{Value, json};
const G: &str = "900000000000000001";
const A: &str = "900000000000000005";
const C: &str = "900000000000000006";
const B: &str = "900000000000000099";
const SOURCE: &str = "900000000000000082";
const DEST: &str = "900000000000000083";
async fn endpoint(r: Request) -> Json<Value> {
    let path = r.uri().path();
    let query = r.uri().query().unwrap_or("");
    let v = if path.ends_with("/roles") {
        json!([{"id":G,"permissions":((1u64<<10)|(1<<16)).to_string()}])
    } else if path.contains("/members/") {
        json!({"roles":[]})
    } else if path.contains("/messages/search") {
        let mut m = json!({"id":"900000000000000091","channel_id":DEST,"author":{"id":A,"username":"synthetic-reader"},"timestamp":"2026-11-12T16:00:01Z"});
        if query.contains("has=snapshot") {
            m["message_reference"] =
                json!({"type":1,"channel_id":SOURCE,"message_id":"900000000000000090"});
            m["message_snapshots"] = json!([{"message":{"content":"snapshot without author"}}]);
        } else if query.contains("replied_to_user_id") {
            m["id"] = json!("900000000000000092");
            m["message_reference"] =
                json!({"type":0,"channel_id":SOURCE,"message_id":"900000000000000090"});
        } else if query.contains("mentions=") {
            m["id"] = json!("900000000000000093");
            m["mentions"] = json!([{"id":C}]);
        } else {
            m["id"] = json!("900000000000000094");
            m["content"] = json!(format!(
                "https://discord.com/channels/{G}/{SOURCE}/900000000000000090"
            ));
        }
        json!({"total_results":1,"messages":[[m]],"doing_deep_historical_index":false})
    } else if path.contains("/messages/") {
        json!({"author":{"id":C},"content":"synthetic original"})
    } else if path.ends_with(SOURCE) {
        json!({"id":SOURCE,"guild_id":G,"type":11,"owner_id":C,"parent_id":DEST})
    } else {
        json!({"id":DEST,"guild_id":G,"type":0,"permission_overwrites":[]})
    };
    Json(v)
}
#[tokio::test]
async fn audit_verifies_origins_and_never_invents_download_records() {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    let task = tokio::spawn(async move {
        axum::serve(l, Router::new().fallback(endpoint))
            .await
            .unwrap()
    });
    let app = Naimei::new(
        NaimeiConfig {
            token: "synthetic".into(),
            guild_id: G.parse().unwrap(),
            backend: format!("http://{addr}"),
            site: String::new(),
            secret: "synthetic".into(),
            chat: None,
            owner_ids: vec![A.parse().unwrap()],
            chat_log: None,
        },
        Some(addr.to_string()),
    );
    app.set_bot_user(User {
        id: B.into(),
        ..User::default()
    });
    let request = ToolRequest {
        actor: A.into(),
        channel: SOURCE.into(),
        name: "creator_audit".into(),
        arguments: json!({}),
    };
    assert_eq!(
        app.capability(request.clone()).await["data"]["status"],
        "pending"
    );
    let result = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let v = app.capability(request.clone()).await;
            if v["data"]["status"] == "finished" {
                break v;
            }
            tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(result["data"]["target"], C);
    assert_eq!(result["data"]["complete"], true);
    for k in ["原生转发", "回复作者", "提及作者", "原帖链接"] {
        assert_eq!(result["data"]["counts"][k]["messages"], 1, "{result}");
    }
    assert!(result["data"]["unavailable"]["downloaders"].is_null());
    assert_eq!(result["data"]["evidence"].as_array().unwrap().len(), 4);
    let text = render("creator_audit", &result);
    assert!(text.contains("已核实行为"));
    assert!(text.contains("Discord 不提供"));
    assert!(text.contains("默认全部可见历史"));
    task.abort();
}
