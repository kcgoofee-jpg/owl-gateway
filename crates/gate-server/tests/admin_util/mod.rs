//! 本机假 Discord（真实的注册服务把请求发到它）。后台请求客户端在 `common`。
#![allow(dead_code)]

use axum::body::Body;
use serde_json::{Value, json};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

// ------------------------------------------------------------------ 本机假 Discord

#[derive(Default)]
struct DState {
    seen: Mutex<Vec<Value>>,
    /// `GET /users/{id}` 的脚本回复：id → (状态码, JSON)。没写的回 `{}`。
    users: Mutex<std::collections::HashMap<String, (u16, String)>>,
}

/// 记录每个请求的假 Discord REST 服务。`DISCORD_API_BASE` 指向它，真实的 `RegistrationService` 照常发请求。
pub struct MockDiscord {
    addr: SocketAddr,
    state: Arc<DState>,
    _shutdown: tokio::sync::oneshot::Sender<()>,
}

async fn discord_handler(
    axum::extract::State(st): axum::extract::State<Arc<DState>>,
    req: axum::extract::Request,
) -> axum::response::Response {
    let (parts, body) = req.into_parts();
    let bytes = axum::body::to_bytes(body, 1 << 20).await.unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    let (method, path) = (parts.method.to_string(), parts.uri.path().to_string());
    st.seen.lock().unwrap().push(json!([method, path, body]));
    let user = path
        .strip_prefix("/users/")
        .and_then(|id| st.users.lock().unwrap().get(id).cloned());
    let (status, text) = match (method.as_str(), path.as_str(), user) {
        ("GET", _, Some((status, body))) => (status, body),
        ("POST", "/users/@me/channels", _) => (200, r#"{"id": "dm-1"}"#.into()),
        _ => (200, "{}".into()),
    };
    let mut r = axum::response::Response::new(Body::from(text));
    *r.status_mut() = axum::http::StatusCode::from_u16(status).unwrap();
    r.headers_mut()
        .insert("content-type", "application/json".parse().unwrap());
    r
}

impl MockDiscord {
    pub async fn start() -> Self {
        let state = Arc::new(DState::default());
        let app = axum::Router::new()
            .fallback(discord_handler)
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = rx.await;
                })
                .await;
        });
        Self {
            addr,
            state,
            _shutdown: tx,
        }
    }
    pub fn url(&self) -> String {
        format!("http://{}", self.addr)
    }
    /// 到目前为止收到的请求：`[方法, 路径, JSON 体]`。
    pub fn calls(&self) -> Vec<Value> {
        self.state.seen.lock().unwrap().clone()
    }
    pub fn clear(&self) {
        self.state.seen.lock().unwrap().clear();
    }
    /// 让 `GET /users/{id}`（机器人读用户资料）回 `status` + `body`。
    pub fn set_user(&self, id: &str, status: u16, body: Value) {
        self.state
            .users
            .lock()
            .unwrap()
            .insert(id.to_string(), (status, body.to_string()));
    }
}
