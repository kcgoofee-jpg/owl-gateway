//! Synthetic REST and bridge failures, pagination, restart and gateway recovery.
use axum::{
    Router,
    body::Body,
    extract::{Request, State},
    response::Response,
};
use gate_discord::{
    config::NaimeiConfig, events::MessageInfo, naimei::Naimei, share::FORUM_CHANNEL_ID,
};
use serde_json::{Value, json};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU16, AtomicU64, Ordering},
};

const GUILD: &str = "900000000000000001";
const MEMBER: &str = "900000000000000005";
const THREAD: &str = "900000000000000082";
const ARCHIVED: &str = "900000000000000083";
const NOW: u64 = 1_800_000_000;

#[derive(Default)]
struct Mock {
    messages: Mutex<Vec<Value>>,
    calls: Mutex<Vec<Value>>,
    share_status: AtomicU16,
    history_status: AtomicU16,
    share_delay: AtomicU64,
    archived: Mutex<bool>,
}
fn id(at: u64) -> String {
    (((at * 1000 - 1_420_070_400_000) << 22) + 1).to_string()
}
fn image(at: u64, channel: &str) -> Value {
    json!({"id":id(at),"channel_id":channel,"guild_id":GUILD,
    "author":{"id":MEMBER,"username":"synthetic"},
    "attachments":[{"id":"1","filename":"image.png","content_type":"image/png"}]})
}
fn reply(status: u16, body: Value) -> Response {
    Response::builder()
        .status(status)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}
async fn handle(State(mock): State<Arc<Mock>>, req: Request) -> Response {
    let (parts, body) = req.into_parts();
    let bytes = axum::body::to_bytes(body, 1 << 20).await.unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    let path = parts.uri.path();
    mock.calls
        .lock()
        .unwrap()
        .push(json!({"path":path,"query":parts.uri.query(),"body":body}));
    if path == "/self-register/bot/share" {
        tokio::time::sleep(std::time::Duration::from_millis(
            mock.share_delay.load(Ordering::Relaxed),
        ))
        .await;
        let code = mock.share_status.load(Ordering::Relaxed);
        return reply(
            if code == 0 { 200 } else { code },
            json!({"ok":code==0 || code==200}),
        );
    }
    if path == "/self-register/bot/report" {
        return reply(200, json!({"ok":true}));
    }
    if path == format!("/api/v10/channels/{FORUM_CHANNEL_ID}") {
        return reply(200, json!({"type":15,"guild_id":GUILD}));
    }
    if path.ends_with("/threads/active") {
        return reply(
            200,
            json!({"threads":[{"id":THREAD,"parent_id":FORUM_CHANNEL_ID}]}),
        );
    }
    if path.ends_with("/threads/archived/public") {
        let threads = if *mock.archived.lock().unwrap() {
            json!([{"id":ARCHIVED,"parent_id":FORUM_CHANNEL_ID,
            "thread_metadata":{"archived":true,"archive_timestamp":"2027-01-15T07:00:00+00:00"}}])
        } else {
            json!([])
        };
        return reply(200, json!({"threads":threads,"has_more":false}));
    }
    if path.ends_with("/thread-members/@me") {
        return reply(200, json!({}));
    }
    if path.ends_with("/messages") {
        let code = mock.history_status.load(Ordering::Relaxed);
        if code != 0 {
            return reply(
                code,
                json!({"code":50013,"message":"synthetic permission failure"}),
            );
        }
        let after = parts
            .uri
            .query()
            .unwrap_or("")
            .split('&')
            .find_map(|s| s.strip_prefix("after="))
            .unwrap_or("0")
            .parse::<u64>()
            .unwrap();
        let cid = path.split('/').nth(4).unwrap();
        let messages: Vec<_> = mock
            .messages
            .lock()
            .unwrap()
            .iter()
            .filter(|m| {
                m["channel_id"] == cid && m["id"].as_str().unwrap().parse::<u64>().unwrap() > after
            })
            .take(100)
            .cloned()
            .collect();
        return reply(200, json!(messages));
    }
    if path == "/api/v10/channels/900000000000000090" {
        return reply(200, json!({"parent_id":"900000000000000091"}));
    }
    if path.starts_with("/api/v10/channels/") {
        return reply(200, json!({"parent_id":FORUM_CHANNEL_ID}));
    }
    reply(404, json!({}))
}
async fn setup() -> (Arc<Mock>, Arc<Naimei>, Arc<AtomicU64>, String) {
    let mock = Arc::new(Mock::default());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let router = Router::new().fallback(handle).with_state(mock.clone());
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let clock = Arc::new(AtomicU64::new(NOW));
    let cfg = NaimeiConfig {
        token: "synthetic".into(),
        guild_id: GUILD.parse().unwrap(),
        backend: format!("http://{addr}"),
        site: "https://site.test".into(),
        secret: "synthetic".into(),
        chat: None,
        owner_ids: vec![],
        chat_log: None,
    };
    let app = Naimei::with_clock(
        cfg,
        Some(addr.to_string()),
        Box::new({
            let clock = clock.clone();
            move || clock.load(Ordering::Relaxed) as f64
        }),
    );
    app.set_gateway_connected(true);
    (mock, app, clock, addr.to_string())
}
fn shares(mock: &Mock) -> usize {
    mock.calls
        .lock()
        .unwrap()
        .iter()
        .filter(|r| r["path"] == "/self-register/bot/share")
        .count()
}

#[tokio::test]
async fn paginates_over_fifty_messages_and_archived_threads_without_duplicate_deliveries() {
    let (mock, app, _, _) = setup().await;
    *mock.messages.lock().unwrap() = (0..205).map(|i| image(NOW - 1000 + i, THREAD)).collect();
    mock.messages.lock().unwrap().push(image(NOW - 1, ARCHIVED));
    *mock.archived.lock().unwrap() = true;
    assert!(app.catch_up_forum().await);
    assert_eq!(shares(&mock), 206);
    assert!(app.share_detection_report().last_complete_at > 0.0);
    assert!(app.catch_up_forum().await);
    assert_eq!(shares(&mock), 206);
}

#[tokio::test]
async fn bridge_failure_keeps_cursor_retries_and_requires_a_complete_sweep_to_recover() {
    let (mock, app, clock, _) = setup().await;
    mock.messages.lock().unwrap().push(image(NOW - 1, THREAD));
    mock.share_status.store(503, Ordering::Relaxed);
    assert!(!app.catch_up_forum().await);
    assert_eq!(app.share_detection_report().pending, 1);
    assert_eq!(app.share_detection_report().last_complete_at, 0.0);
    mock.share_status.store(200, Ordering::Relaxed);
    clock.store(NOW + 4, Ordering::Relaxed);
    app.retry_shares_once().await;
    assert_eq!(app.share_detection_report().pending, 0);
    assert!(app.share_detection_report().error.is_some());
    assert_eq!(app.share_detection_report().last_complete_at, 0.0);
    assert!(app.catch_up_forum().await);
    assert!(app.share_detection_report().error.is_none());
    assert_eq!(shares(&mock), 2);
}

#[tokio::test]
async fn restart_replays_unconfirmed_records_and_permission_loss_never_reports_complete() {
    let (mock, app, clock, addr) = setup().await;
    mock.messages.lock().unwrap().push(image(NOW - 1, THREAD));
    mock.share_status.store(422, Ordering::Relaxed);
    assert!(!app.catch_up_forum().await);
    assert_eq!(
        app.share_detection_report().pending,
        0,
        "permanent errors must not hot-loop"
    );
    assert!(
        app.share_detection_report()
            .error
            .as_ref()
            .unwrap()
            .contains("422")
    );
    let restarted = Naimei::with_clock(
        app.cfg.clone(),
        Some(addr),
        Box::new(move || clock.load(Ordering::Relaxed) as f64),
    );
    restarted.set_gateway_connected(true);
    mock.share_status.store(200, Ordering::Relaxed);
    mock.history_status.store(403, Ordering::Relaxed);
    assert!(!restarted.catch_up_forum().await);
    assert_eq!(restarted.share_detection_report().last_complete_at, 0.0);
    mock.history_status.store(0, Ordering::Relaxed);
    assert!(restarted.catch_up_forum().await);
    assert_eq!(shares(&mock), 2);
}

#[tokio::test]
async fn ignores_text_bots_webhooks_and_wrong_guild_even_without_chat_configured() {
    let (mock, app, _, _) = setup().await;
    let mut text = image(NOW - 1, THREAD);
    text["attachments"] = json!([]);
    let mut bot = image(NOW - 2, THREAD);
    bot["author"]["bot"] = json!(true);
    let mut webhook = image(NOW - 3, THREAD);
    webhook["webhook_id"] = json!("900000000000000099");
    let mut other = image(NOW - 4, THREAD);
    other["guild_id"] = json!("900000000000000009");
    for value in [
        text,
        bot,
        webhook,
        other,
        image(NOW - 6, "900000000000000090"),
        image(NOW - 5, THREAD),
        image(NOW - 5, THREAD),
    ] {
        app.handle_message(serde_json::from_value::<MessageInfo>(value).unwrap())
            .await;
    }
    assert_eq!(shares(&mock), 1);
    assert_eq!(app.share_detection_report().pending, 0);
}

#[tokio::test]
async fn cancellation_does_not_leave_a_permanent_inflight_record() {
    let (mock, app, _, _) = setup().await;
    mock.share_delay.store(100, Ordering::Relaxed);
    let m: MessageInfo = serde_json::from_value(image(NOW - 1, THREAD)).unwrap();
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(20), app.handle_message(m))
            .await
            .is_err()
    );
    assert_eq!(app.share_detection_report().pending, 1);
    mock.share_delay.store(0, Ordering::Relaxed);
    app.retry_shares_once().await;
    assert_eq!(app.share_detection_report().pending, 0);
}

#[tokio::test]
async fn rate_limited_share_delivery_is_retryable() {
    let (mock, app, clock, _) = setup().await;
    mock.messages.lock().unwrap().push(image(NOW - 1, THREAD));
    mock.share_status.store(429, Ordering::Relaxed);
    assert!(!app.catch_up_forum().await);
    assert_eq!(app.share_detection_report().pending, 1);
    mock.share_status.store(200, Ordering::Relaxed);
    clock.store(NOW + 4, Ordering::Relaxed);
    app.retry_shares_once().await;
    assert_eq!(app.share_detection_report().pending, 0);
    assert!(app.catch_up_forum().await);
}

#[tokio::test]
async fn joining_other_new_threads_preserves_chat_access_without_counting_them_as_shares() {
    let (mock, app, _, _) = setup().await;
    gate_discord::naimei::dispatch(
        &app,
        &json!({"op":0,"s":1,"t":"THREAD_CREATE",
        "d":{"id":THREAD,"parent_id":"900000000000000090"}})
        .to_string(),
    );
    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        loop {
            if mock
                .calls
                .lock()
                .unwrap()
                .iter()
                .any(|r| r["path"].as_str().unwrap().ends_with("/thread-members/@me"))
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(shares(&mock), 0);
}

#[tokio::test]
async fn new_post_starter_is_recorded_before_thread_join_can_delay_it() {
    let (mock, app, _, _) = setup().await;
    gate_discord::naimei::dispatch(
        &app,
        &json!({"op":0,"s":1,"t":"THREAD_CREATE",
        "d":{"id":THREAD,"parent_id":FORUM_CHANNEL_ID,"message":image(NOW-1,THREAD)}})
        .to_string(),
    );
    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        loop {
            if mock
                .calls
                .lock()
                .unwrap()
                .iter()
                .any(|r| r["path"].as_str().unwrap().ends_with("/thread-members/@me"))
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        }
    })
    .await
    .unwrap();
    let calls = mock.calls.lock().unwrap();
    let share = calls
        .iter()
        .position(|r| r["path"] == "/self-register/bot/share")
        .unwrap();
    let join = calls
        .iter()
        .position(|r| r["path"].as_str().unwrap().ends_with("/thread-members/@me"))
        .unwrap();
    assert!(share < join);
}
