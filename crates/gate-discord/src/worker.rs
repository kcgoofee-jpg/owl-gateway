//! Private model boundary. This process never receives a Discord token.
use crate::chat::{Chat, ChatCfg, ChatError, Turn};
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, State},
    http::{HeaderMap, StatusCode},
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::Arc;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub system: String,
    pub history: Vec<Turn>,
    pub user: Value,
    #[serde(default)]
    pub actor: Option<crate::naimei::capabilities::ToolRequest>,
}
#[derive(Clone)]
struct Worker {
    chat: Chat,
    secret: String,
    slots: Arc<tokio::sync::Semaphore>,
    core: String,
}
async fn reply(
    State(w): State<Worker>,
    headers: HeaderMap,
    Json(r): Json<Request>,
) -> (StatusCode, Json<Value>) {
    if headers.get("authorization").and_then(|v| v.to_str().ok())
        != Some(&format!("Bearer {}", w.secret))
    {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error":"unauthorized"})),
        );
    }
    let Ok(_slot) = w.slots.try_acquire() else {
        return (StatusCode::TOO_MANY_REQUESTS, Json(json!({"kind":"quota"})));
    };
    let result = tokio::time::timeout(std::time::Duration::from_secs(150), async {
        if r.actor.is_some() {
            w.chat.reply_agent(r, &w.core, &w.secret).await
        } else {
            w.chat.reply(&r.system, &r.history, r.user).await
        }
    })
    .await;
    let body = match result {
        Ok(Ok(answer)) => json!({"answer":answer}),
        Err(_) | Ok(Err(ChatError::Timeout)) => json!({"kind":"timeout"}),
        Ok(Err(ChatError::Quota)) => json!({"kind":"quota"}),
        Ok(Err(ChatError::ImageInput(_))) => json!({"kind":"image"}),
        Ok(Err(e)) => {
            tracing::warn!(http_status=?e.http_status(),transient=e.transient(),"[chat-worker] upstream request failed");
            json!({"kind":if e.transient(){"transient"}else{"upstream"},"http_status":e.http_status()})
        }
    };
    (StatusCode::OK, Json(body))
}
pub async fn run() -> anyhow::Result<()> {
    anyhow::ensure!(
        std::env::var("DISCORD_BOT_TOKEN").is_err(),
        "model worker must not receive Discord credentials"
    );
    let secret = std::env::var("NAIMEI_WORKER_SECRET")?;
    anyhow::ensure!(!secret.is_empty(), "worker secret required");
    let chat = Chat::local(ChatCfg {
        base_url: std::env::var("NAIMEI_LLM_BASE_URL")?
            .trim_end_matches('/')
            .to_owned(),
        api_key: std::env::var("NAIMEI_LLM_API_KEY")?,
        model: std::env::var("NAIMEI_LLM_MODEL").unwrap_or_else(|_| "glm-5.3-flash".into()),
        stream: false,
    });
    let core = std::env::var("NAIMEI_CORE_URL").unwrap_or_else(|_| "http://naimei:8012".into());
    let app = router(chat, secret, core);
    let bind = std::env::var("NAIMEI_WORKER_BIND").unwrap_or_else(|_| "0.0.0.0:8011".into());
    axum::serve(tokio::net::TcpListener::bind(bind).await?, app).await?;
    Ok(())
}

pub fn router(chat: Chat, secret: String, core: String) -> Router {
    Router::new()
        .route("/healthz", get(|| async { Json(json!({"ok":true})) }))
        .route("/reply", post(reply))
        .layer(DefaultBodyLimit::max(512 * 1024))
        .with_state(Worker {
            chat,
            secret,
            core,
            slots: Arc::new(tokio::sync::Semaphore::new(4)),
        })
}
