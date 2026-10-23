//! 桥接超时降级：网关连不上、或接了连接却一直不回时，`Bridge` 都要降级成「服务暂时不可用」。
//! 网关一侧的真实契约由 crates/gate-server/tests/registration.rs 覆盖，机器人一侧的请求形状与回复文案
//! 由 flows.rs / feedback_text.rs 通过 `common::MockServer` 逐键断言。

use std::time::Duration;

use gate_discord::bridge::{Bridge, who_payload};
use gate_discord::events::Interaction;
use gate_discord::handlers::Backend;
use serde_json::json;

const GUILD: &str = "900000000000000001";
const MEMBER: &str = "900000000000000005";
const SECRET: &str = "contract-secret";

fn interaction(user: &str, guild: &str) -> Interaction {
    serde_json::from_value(json!({
        "id": "900000000000000020", "application_id": "900000000000000003", "type": 2, "token": "tok",
        "guild_id": guild, "member": {"user": {"id": user, "username": "someone"}}, "data": {"name": "x"},
    }))
    .unwrap()
}

#[tokio::test]
async fn unreachable_or_hanging_gateway_degrades_to_unavailable() {
    // 连不上（端口没人听）→ (0, 服务暂时不可用)
    let down = Bridge::new("http://127.0.0.1:9", SECRET);
    let payload = who_payload(&interaction(MEMBER, GUILD), &[]).unwrap();
    assert_eq!(
        down.command("/self-register/quota", &payload)
            .await
            .unwrap(),
        Backend::unavailable()
    );
    assert_eq!(down.issue(&json!({})).await, None);
    assert_eq!(down.bot("/report", &json!({})).await, None);

    // 接了连接却一直不回 → 超时后同样降级
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        let mut held = Vec::new();
        while let Ok((sock, _)) = listener.accept().await {
            held.push(sock); // 不读不写，保持打开
        }
    });
    let slow = Bridge::new(url, SECRET)
        .with_timeouts(Duration::from_millis(200), Duration::from_millis(200));
    assert_eq!(
        slow.command("/self-register/quota", &payload)
            .await
            .unwrap(),
        Backend::unavailable()
    );
    assert_eq!(slow.issue(&json!({})).await, None);
    assert_eq!(slow.bot("/report", &json!({})).await, None);
}
