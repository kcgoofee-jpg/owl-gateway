mod common;
use common::{Call, TOKEN, settle, setup};
use gate_db::repo::usage::LogFilter;
use gate_testkit::Reply;
use serde_json::{Value, json};

fn body(n: i64) -> Value {
    json!({"input":"cat", "model":"nai-diffusion-4-5-full", "action":"generate", "parameters":{"width":832,"height":1216,"steps":28,"n_samples":n,"scale":5,"sampler":"k_euler_ancestral"}})
}

#[tokio::test]
async fn success_and_known_rejection_release_live_budget_once() {
    let e = setup(&[]).await;
    let (status, _, _) = Call::new("POST", "/ai/generate-image")
        .bearer(TOKEN)
        .json(body(1))
        .send(&e.state)
        .await;
    assert_eq!(status, 200);
    settle().await;
    assert!(e.state.budget.lock().await.reservations.is_empty());
    assert_eq!(e.mock.requests_to("/ai/generate-image").len(), 1);
    let counters = e.state.db.get_counter(1, &e.state.day()).await.unwrap();
    assert_eq!(counters.images, 1);
    let status: String = e
        .state
        .db
        .read(|c| c.query_row("SELECT status FROM request_settlements", [], |r| r.get(0)))
        .await
        .unwrap();
    assert_eq!(status, "committed");

    let e = setup(&[]).await;
    e.mock
        .push("/ai/generate-image", Reply::status(401, "invalid token"));
    let _ = Call::new("POST", "/ai/generate-image")
        .bearer(TOKEN)
        .json(body(1))
        .send(&e.state)
        .await;
    settle().await;
    assert!(e.state.budget.lock().await.reservations.is_empty());
    let status: String = e
        .state
        .db
        .read(|c| c.query_row("SELECT status FROM request_settlements", [], |r| r.get(0)))
        .await
        .unwrap();
    assert_eq!(status, "released");
    assert_eq!(e.mock.requests_to("/ai/generate-image").len(), 1);
}

#[tokio::test]
async fn unknown_result_stays_in_live_budget_and_is_restored_after_restart() {
    let e = setup(&[]).await;
    e.mock
        .push("/ai/generate-image", Reply::status(500, "upstream failure"));
    let _ = Call::new("POST", "/ai/generate-image")
        .bearer(TOKEN)
        .json(body(1))
        .send(&e.state)
        .await;
    settle().await;
    let reservations = e.state.budget.lock().await.reservations.clone();
    assert_eq!(reservations.len(), 1);
    assert!(reservations.values().next().unwrap().anlas >= 0.0);
    let status: String = e
        .state
        .db
        .read(|c| c.query_row("SELECT status FROM request_settlements", [], |r| r.get(0)))
        .await
        .unwrap();
    assert_eq!(status, "uncertain");
    assert_eq!(e.mock.requests_to("/ai/generate-image").len(), 1);

    let restarted = e.restart().await;
    let recovered = restarted.budget.lock().await.reservations.clone();
    assert_eq!(recovered.len(), 1);
    assert_eq!(
        recovered.values().next().unwrap().request_id,
        reservations.values().next().unwrap().request_id
    );
    assert_eq!(
        e.mock.requests_to("/ai/generate-image").len(),
        1,
        "recovery never replays POST"
    );
}

#[tokio::test]
async fn stream_missing_final_frame_holds_unknown_request_without_replay() {
    let e = setup(&[]).await;
    e.mock.push(
        "/ai/generate-image-stream",
        Reply::Chunks {
            status: 200,
            content_type: "text/event-stream".into(),
            chunks: gate_testkit::image_stream(0, "sse"),
        },
    );
    let (status, _, _) = Call::new("POST", "/ai/generate-image-stream")
        .bearer(TOKEN)
        .json(body(2))
        .send(&e.state)
        .await;
    assert_eq!(status, 200);
    settle().await;
    let logs = e
        .state
        .db
        .list_logs(10, 0, &LogFilter::default())
        .await
        .unwrap();
    assert!(logs.iter().all(|r| r["status"] != "ok"));
    let counters = e.state.db.get_counter(1, &e.state.day()).await.unwrap();
    assert_eq!(counters.images, 0);
    let held = e.state.budget.lock().await.reservations.clone();
    assert_eq!(held.len(), 1, "logs={logs:?}, held={held:?}");
    assert!(
        held.values().next().unwrap().anlas > 0.0
            || held.values().next().unwrap().v5 > 0
            || held.values().next().unwrap().legacy > 0
    );
    let status: String = e
        .state
        .db
        .read(|c| c.query_row("SELECT status FROM request_settlements", [], |r| r.get(0)))
        .await
        .unwrap();
    assert_eq!(status, "uncertain");
    assert_eq!(e.mock.requests_to("/ai/generate-image-stream").len(), 1);
}
