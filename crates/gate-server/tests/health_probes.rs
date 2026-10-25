use axum::{body::Body, http::Request};
use gate_server::{AppState, app, env::EnvConfig, web::Assets};
use serde_json::Value;
use std::{collections::HashMap, time::Duration};
use tower::ServiceExt;

async fn response(state: &AppState, path: &str) -> (axum::http::StatusCode, Value) {
    let response = app(state.clone())
        .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, serde_json::from_slice(&body).unwrap())
}

async fn state(upstream: &str) -> (tempfile::TempDir, AppState) {
    let dir = tempfile::tempdir().unwrap();
    let vars: HashMap<String, String> = [
        ("DATA_DIR", dir.path().to_str().unwrap()),
        ("NAI_TOKENS", "synthetic-token-must-not-be-probed"),
        ("NAI_IMAGE_HOST", upstream),
        ("SECRET_KEY", "test-secret-key-test-secret-key-0123456789"),
        ("ADMIN_PASSWORD", "dev-admin-password-123"),
    ]
    .into_iter()
    .map(|(key, value)| (key.to_owned(), value.to_owned()))
    .collect();
    let state = AppState::build(EnvConfig::load(&vars), Assets::new(None))
        .await
        .unwrap();
    (dir, state)
}

#[tokio::test]
async fn probes_separate_liveness_readiness_and_legacy_health_without_upstream_probe() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream = format!("http://{}", listener.local_addr().unwrap());
    let (_dir, state) = state(&upstream).await;
    let (status, body) = response(&state, "/livez").await;
    assert_eq!(status, 200);
    assert_eq!(body["ok"], true);

    let (status, body) = response(&state, "/readyz").await;
    assert_eq!(status, 200);
    assert_eq!(body["ok"], true);

    let (status, body) = response(&state, "/healthz").await;
    assert_eq!(status, 200);
    assert_eq!(body["ok"], true);
    assert_eq!(body["upstream"], "unknown");

    assert!(
        tokio::time::timeout(Duration::from_millis(100), listener.accept())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn database_write_lock_makes_ready_unhealthy_but_keeps_process_live_and_recovers() {
    let (_dir, state) = state("http://127.0.0.1:9").await;
    let db = state.db.clone();
    let blocker = tokio::spawn(async move {
        db.write_raw(|_| {
            std::thread::sleep(Duration::from_secs(4));
            Ok(())
        })
        .await
    });
    tokio::time::sleep(Duration::from_millis(30)).await;

    let (status, body) = response(&state, "/livez").await;
    assert_eq!(status, 200);
    assert_eq!(body["ok"], true);

    let (status, body) = response(&state, "/readyz").await;
    assert_eq!(status, 503);
    assert_eq!(body["ok"], false);
    assert_eq!(body["reason"], "db_write");

    let (status, body) = response(&state, "/healthz").await;
    assert_eq!(status, 503);
    assert_eq!(body["upstream"], "unknown");
    blocker.await.unwrap().unwrap();
    let (status, body) = response(&state, "/readyz").await;
    assert_eq!(status, 200);
    assert_eq!(body["ok"], true);
}
