//! 测试公用：本机假 HTTP 服务（Discord REST / 站点接口）与合成数据库。
//! 所有数据都是合成的（公开仓库）：Discord ID 用 9000000000000000xx 段，Key / Token 都是明显的假串。

#![allow(dead_code)]

use axum::Router;
use rusqlite::Connection;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// 一次收到的请求。
#[derive(Debug, Clone)]
pub struct Req {
    pub method: String,
    pub path: String,
    pub auth: String,
    pub user_agent: String,
    pub body: serde_json::Value,
}

/// 跑在 127.0.0.1 随机端口上的假服务；Drop 时停掉。
pub struct Mock {
    pub base: String,
    handle: tokio::task::JoinHandle<()>,
}

impl Mock {
    pub async fn start(app: Router) -> Mock {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Mock { base, handle }
    }
}

impl Drop for Mock {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

pub type Log = Arc<Mutex<Vec<Req>>>;

/// 假 Discord REST：记录每个请求；可编排失败。
#[derive(Default)]
pub struct DiscordScript {
    /// 所有「发消息」返回这个状态码（0 = 正常 200）
    pub message_status: u16,
    /// 返回 403 且正文带 1010（模拟 Cloudflare 拦 UA）
    pub cloudflare_1010: bool,
}

pub async fn discord_mock(script: DiscordScript) -> (Mock, Log) {
    use axum::extract::{Path as AxPath, State};
    use axum::http::{HeaderMap, StatusCode};
    use axum::routing::post;
    use axum::{Json, response::IntoResponse};
    use serde_json::{Value, json};
    use std::sync::atomic::{AtomicU64, Ordering};

    struct St {
        log: Log,
        script: DiscordScript,
        seq: AtomicU64,
    }
    fn record(st: &St, method: &str, path: String, headers: &HeaderMap, body: Value) {
        let h = |k: &str| {
            headers
                .get(k)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
                .to_string()
        };
        st.log.lock().unwrap().push(Req {
            method: method.into(),
            path,
            auth: h("authorization"),
            user_agent: h("user-agent"),
            body,
        });
    }
    let log: Log = Arc::default();
    let st = Arc::new(St {
        log: log.clone(),
        script,
        seq: AtomicU64::new(0),
    });

    let app = Router::new()
        .route(
            "/users/@me/channels",
            post(|State(st): State<Arc<St>>, headers: HeaderMap, Json(body): Json<Value>| async move {
                record(&st, "POST", "/users/@me/channels".into(), &headers, body.clone());
                let who = body["recipient_id"].as_str().unwrap_or("").to_string();
                (StatusCode::OK, Json(json!({"id": format!("dm-{who}")}))).into_response()
            }),
        )
        .route(
            "/channels/{channel}/messages",
            post(|State(st): State<Arc<St>>, AxPath(channel): AxPath<String>, headers: HeaderMap, Json(body): Json<Value>| async move {
                record(&st, "POST", format!("/channels/{channel}/messages"), &headers, body);
                if st.script.cloudflare_1010 {
                    return (StatusCode::FORBIDDEN, "error code: 1010").into_response();
                }
                if st.script.message_status != 0 && st.script.message_status != 200 {
                    let code = StatusCode::from_u16(st.script.message_status).unwrap();
                    return (code, Json(json!({"message": "scripted failure"}))).into_response();
                }
                let n = st.seq.fetch_add(1, Ordering::SeqCst) + 1;
                (StatusCode::OK, Json(json!({"id": format!("msg-{n}")}))).into_response()
            }),
        )
        .with_state(st);
    (Mock::start(app).await, log)
}

/// 全新数据库：用 gate-db 建好全部表（与线上启动一致），返回路径。
pub fn new_db(dir: &Path) -> PathBuf {
    let path = dir.join("nai_gate.db");
    drop(gate_db::Db::open(&path).unwrap());
    path
}

/// 读写连接，用来灌合成数据。
pub fn rw(path: &Path) -> Connection {
    let c = Connection::open(path).unwrap();
    c.busy_timeout(std::time::Duration::from_secs(5)).unwrap();
    c
}

pub fn add_key(c: &Connection, id: i64, token: &str, enabled: bool) {
    c.execute(
        "INSERT INTO api_keys(id, name, token, enabled, created_at) VALUES (?,?,?,?,?)",
        rusqlite::params![id, format!("synthetic-{id}"), token, enabled, 1.7e9],
    )
    .unwrap();
}

#[derive(Clone)]
pub struct Log1 {
    pub ts: f64,
    pub key_id: Option<i64>,
    pub kind: &'static str,
    pub status: &'static str,
    pub images: i64,
    pub wait_ms: i64,
    pub reason: &'static str,
    pub up_status: i64,
    pub detail: &'static str,
    pub model: &'static str,
}

impl Log1 {
    pub fn image_ok(ts: f64, key_id: i64) -> Self {
        Log1 {
            ts,
            key_id: Some(key_id),
            kind: "image",
            status: "ok",
            images: 1,
            wait_ms: 0,
            reason: "",
            up_status: 200,
            detail: "",
            model: "",
        }
    }
}

pub fn add_log(c: &Connection, l: &Log1) {
    c.execute(
        "INSERT INTO usage_log(ts, key_id, key_name, kind, model, status, images, wait_ms, reason, up_status, detail) \
         VALUES (?,?,?,?,?,?,?,?,?,?,?)",
        rusqlite::params![l.ts, l.key_id, "synthetic", l.kind, l.model, l.status, l.images, l.wait_ms, l.reason, l.up_status, l.detail],
    )
    .unwrap();
}

pub fn set_setting(c: &Connection, k: &str, v: &str) {
    c.execute(
        "INSERT OR REPLACE INTO site_settings(key, value) VALUES (?,?)",
        [k, v],
    )
    .unwrap();
}
