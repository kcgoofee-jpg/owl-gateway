//! Private authenticated tool bridge. Identity is fixed by the Discord dispatch path.
use super::capabilities::ToolRequest;
use super::*;
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, State},
    http::{HeaderMap, StatusCode},
    routing::post,
};
pub(super) async fn run(app: Arc<Naimei>) -> anyhow::Result<()> {
    let bind = std::env::var("NAIMEI_CORE_BIND").unwrap_or_else(|_| "0.0.0.0:8012".into());
    let routes = Router::new()
        .route("/tools", post(execute))
        .layer(DefaultBodyLimit::max(4096))
        .with_state(app);
    axum::serve(tokio::net::TcpListener::bind(bind).await?, routes).await?;
    Ok(())
}
async fn execute(
    State(app): State<Arc<Naimei>>,
    headers: HeaderMap,
    Json(r): Json<ToolRequest>,
) -> (StatusCode, Json<Value>) {
    let secret = std::env::var("NAIMEI_WORKER_SECRET").unwrap_or_default();
    if secret.is_empty()
        || headers.get("authorization").and_then(|v| v.to_str().ok())
            != Some(&format!("Bearer {secret}"))
    {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error":"unauthorized"})),
        );
    }
    (StatusCode::OK, Json(app.capability(r).await))
}
