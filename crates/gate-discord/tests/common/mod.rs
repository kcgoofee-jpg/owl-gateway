//! 测试用的假服务器：同一个端口同时扮演「网关桥接」（`/self-register/*`）与「Discord REST」（`/api/v10/*`），
//! 按场景脚本回话，并逐条记录收到的请求。测试里不连任何真实的 Discord。
#![allow(dead_code)]

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::{Request, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::Response;
use gate_discord::config::NaimeiConfig;
use gate_discord::events::{GuildInfo, User};
use gate_discord::naimei::Naimei;
use serde_json::{Map, Value, json};

/// 合成数据。
pub const GUILD: &str = "900000000000000001";
pub const APP: &str = "900000000000000003";
pub const BOT_ID: &str = "900000000000000099";
pub const SECRET: &str = "test-secret";
pub const SITE: &str = "https://site.test";
/// 固定的「现在」（Unix 秒），Key 剩余有效期才不会随时间漂移。
pub const NOW: f64 = 1_760_000_000.0;
/// 发命令的成员。
pub const MEMBER: &str = "900000000000000005";

pub fn naimei(server: &MockServer) -> Arc<Naimei> {
    let cfg = NaimeiConfig {
        token: "test-token".into(),
        guild_id: GUILD.parse().unwrap(),
        backend: server.backend_url(),
        site: SITE.into(),
        secret: SECRET.into(),
        chat: None,
    };
    let app = Naimei::with_clock(cfg, Some(server.discord_proxy()), Box::new(|| NOW));
    app.set_bot_user(User {
        id: BOT_ID.into(),
        username: "naimei".into(),
        discriminator: Some("0".into()),
        ..User::default()
    });
    app.upsert_guild(GuildInfo {
        id: GUILD.into(),
        name: Some("测试服".into()),
        unavailable: false,
    });
    app
}

pub fn member() -> Value {
    json!({"user": {"id": MEMBER, "username": "alice", "global_name": "Alice", "avatar": "a1b2c3",
                    "discriminator": "0"}, "nick": null, "roles": []})
}

pub fn bridge_paths(server: &MockServer) -> Vec<String> {
    server
        .bridge_log()
        .iter()
        .map(|c| c["path"].as_str().unwrap().to_owned())
        .collect()
}

#[derive(Default)]
struct Script {
    /// 路径 → 响应规格：`{"status", "json"}` / `{"status", "text"}` / `{"error": ...}`；`"*"` 为兜底。
    bridge: Map<String, Value>,
    /// `callback` / `followup` → `{"status", "json"}`：让对应的 Discord 请求失败。
    discord: Map<String, Value>,
}

#[derive(Default)]
struct Log {
    bridge: Vec<Value>,
    discord: Vec<Value>,
}

#[derive(Default)]
struct Shared {
    script: Mutex<Script>,
    log: Mutex<Log>,
}

pub struct MockServer {
    pub addr: SocketAddr,
    shared: Arc<Shared>,
}

impl MockServer {
    pub async fn start() -> MockServer {
        let shared = Arc::new(Shared::default());
        let app = Router::new().fallback(handle).with_state(shared.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        MockServer { addr, shared }
    }

    /// 网关地址（`REGISTRATION_BACKEND_URL`）。
    pub fn backend_url(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// 传给 `Naimei::new` 的 Discord REST 代理地址。
    pub fn discord_proxy(&self) -> String {
        self.addr.to_string()
    }

    /// 换一套脚本并清空记录。
    pub fn reset(&self, bridge: &Value, discord: &Value) {
        *self.shared.script.lock().unwrap() = Script {
            bridge: bridge.as_object().cloned().unwrap_or_default(),
            discord: discord.as_object().cloned().unwrap_or_default(),
        };
        *self.shared.log.lock().unwrap() = Log::default();
    }

    pub fn bridge_log(&self) -> Vec<Value> {
        self.shared.log.lock().unwrap().bridge.clone()
    }

    pub fn discord_log(&self) -> Vec<Value> {
        self.shared.log.lock().unwrap().discord.clone()
    }
}

fn query_map(query: Option<&str>) -> Value {
    let mut out = Map::new();
    for pair in query.unwrap_or("").split('&').filter(|p| !p.is_empty()) {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        out.insert(k.to_owned(), Value::String(v.to_owned()));
    }
    Value::Object(out)
}

fn json_response(status: u16, body: &Value) -> Response {
    let mut r = Response::new(Body::from(body.to_string()));
    *r.status_mut() = StatusCode::from_u16(status).unwrap();
    r.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    r
}

/// 传输层失败：先发状态行与响应头，再让响应体报错（客户端 `bytes()` / `json()` 失败）。
fn broken_response() -> Response {
    let stream = futures::stream::iter([Err::<Bytes, std::io::Error>(std::io::Error::other(
        "synthetic",
    ))]);
    let mut r = Response::new(Body::from_stream(stream));
    r.headers_mut()
        .insert(header::CONTENT_LENGTH, HeaderValue::from_static("100"));
    r
}

fn scripted(spec: &Value) -> Response {
    if spec.get("error").is_some() {
        return broken_response();
    }
    let status = spec["status"].as_u64().unwrap_or(200) as u16;
    if let Some(body) = spec.get("json") {
        json_response(status, body)
    } else {
        let mut r = Response::new(Body::from(spec["text"].as_str().unwrap_or("").to_owned()));
        *r.status_mut() = StatusCode::from_u16(status).unwrap();
        r
    }
}

async fn handle(State(shared): State<Arc<Shared>>, req: Request) -> Response {
    let (parts, body) = req.into_parts();
    let bytes = axum::body::to_bytes(body, 1 << 20)
        .await
        .unwrap_or_default();
    let body_json = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes)
            .unwrap_or(Value::String(String::from_utf8_lossy(&bytes).into_owned()))
    };
    let path = parts.uri.path().to_owned();
    let method = parts.method.as_str().to_owned();

    if path.starts_with("/self-register/") {
        let auth = parts
            .headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .map_or(Value::Null, |s| Value::String(s.to_owned()));
        let content_type = parts
            .headers
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map_or(Value::Null, |s| Value::String(s.to_owned()));
        let mut log = shared.log.lock().unwrap();
        log.bridge.push(json!({
            "method": method, "path": path, "authorization": auth, "content_type": content_type, "json": body_json,
        }));
        drop(log);
        let script = shared.script.lock().unwrap();
        return match script.bridge.get(&path).or_else(|| script.bridge.get("*")) {
            Some(spec) => scripted(spec),
            None => json_response(500, &json!({"detail": "mock: 场景没有给这个路径配置响应"})),
        };
    }

    if let Some(rest) = path.strip_prefix("/api/v10") {
        shared.log.lock().unwrap().discord.push(json!({
            "method": method, "path": rest, "query": query_map(parts.uri.query()), "json": body_json,
            "authorization": parts.headers.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()),
        }));
        let kind = if rest.ends_with("/callback") {
            "callback"
        } else if rest.starts_with("/webhooks/") {
            "followup"
        } else {
            ""
        };
        let script = shared.script.lock().unwrap();
        if let Some(spec) = script.discord.get(kind) {
            return scripted(spec);
        }
        return json_response(
            200,
            &if method == "PUT" {
                json!([])
            } else {
                json!({})
            },
        );
    }

    json_response(404, &json!({"detail": "mock: unknown path"}))
}
