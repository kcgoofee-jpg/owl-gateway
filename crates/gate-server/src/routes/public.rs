//! 公开页面、实时看板、状态与客户端兼容视图。
//!
//! - `/public/live`（JSON）、`/public/live/stream`（SSE，3.0 新增）、`/public/status`、`/queue-status`；
//! - `/user/*`、`/ai/user/*`：给原生 NAI 客户端的虚拟订阅 / 数据 / 优先级 / 密钥库 / 账户信息；
//! - `GET /admin/api/events`（3.0 新增）：后台 SSE（实时快照 + 角标数字）；
//! - 首页首屏数据（[`landing_boot`]，由 pages.rs 的 `/` 使用）。
//!
//! `/healthz`、`/`、`/admin*`、`/_owl/*`、图标在 pages.rs。

use super::registration::no_store;
use crate::auth::{authenticate, client_ip, header_str};
use crate::error::{ApiResult, HttpError, InternalError, ok_json};
use crate::key::Key;
use crate::live::{self, Snapshot, StreamKind, StreamSlot, wire_text};
use crate::session::{self, ADMIN_COOKIE, AdminSession};
use crate::state::AppState;
use axum::Router;
use axum::body::Body;
use axum::extract::{Query, Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use futures::StreamExt;
use gate_core::primitives::take_chars;
use serde::Deserialize;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::convert::Infallible;
use std::hash::{Hash, Hasher};
use std::io::Read;
use std::sync::Arc;
use tokio::sync::{OnceCell, watch};
use tokio::time::{Duration, Instant};

pub fn router() -> Router<AppState> {
    let mut r = Router::new()
        .route("/public/live", get(public_live))
        .route("/public/live/stream", get(public_live_stream))
        .route("/public/device-fp", post(device_fp))
        .route("/public/tokenizer/{model}", get(public_tokenizer))
        .route("/public/status", get(public_status))
        .route("/queue-status", get(queue_status))
        .route("/admin/api/events", get(admin_events));
    for p in ["/user/subscription", "/ai/user/subscription"] {
        r = r.route(p, get(user_subscription));
    }
    for p in ["/user/data", "/ai/user/data"] {
        r = r.route(p, get(user_data));
    }
    for p in ["/user/priority", "/ai/user/priority"] {
        r = r.route(p, get(user_priority));
    }
    for p in ["/user/keystore", "/ai/user/keystore"] {
        r = r.route(p, get(user_keystore).put(user_keystore_put));
    }
    for p in ["/user/inventory", "/ai/user/inventory"] {
        r = r.route(p, get(user_inventory));
    }
    for p in ["/user/login", "/ai/user/login"] {
        r = r.route(p, post(user_login));
    }
    for p in ["/user/clientsettings", "/ai/user/clientsettings"] {
        r = r.route(p, get(user_settings).put(user_settings_put));
    }
    r = r.route("/ai/trial-status", get(trial_status));
    // 账单 / 礼包：本站没有付费和礼包体系，官网前端仍会探这两个接口，回空数组/空对象避免 404。
    r = r.route("/user/giftkeys", get(empty_list));
    for p in ["/user/cb/get-customer", "/ai/user/cb/get-customer"] {
        r = r.route(p, get(empty_object));
    }
    // 官网镜像页会探这些账号/对象接口：本站没有对应能力，回空结构而不是 404（404 会在页面里报错）
    r = r.route("/user/self", get(empty_object));
    for p in ["/usage/link", "/usage/event"] {
        r = r.route(p, post(empty_object)).route(p, get(empty_object));
    }
    for p in ["/user/objects", "/user/objects/{kind}"] {
        r = r.route(p, get(empty_objects));
    }
    for p in ["/user/information", "/ai/user/information"] {
        r = r.route(p, get(user_information));
    }
    r
}

// The URL pins NovelAI's v2 tokenizer assets; hashes below intentionally keep one stable model per release.
static QWEN_TOKENIZER: OnceCell<Vec<u8>> = OnceCell::const_new();
static T5_TOKENIZER: OnceCell<Vec<u8>> = OnceCell::const_new();
static CLIP_TOKENIZER: OnceCell<Vec<u8>> = OnceCell::const_new();
const TOKENIZER_COMPRESSED_LIMIT: usize = 8 * 1024 * 1024;
const TOKENIZER_JSON_LIMIT: usize = 32 * 1024 * 1024;

fn tokenizer_sha256(model: &str) -> Option<&'static str> {
    match model {
        "qwen" => Some("f4040d875827d2f9edc30dd4d736bc4a853a19ceeb0d04f6e3648f17796d1667"),
        "t5" => Some("3cbf698ff2c19efb5c75165d781c1d1b9c035baf119c3e71f1a380cbbfb9dfcb"),
        "clip" => Some("05852800e8ca59374d5b6d6d3993d9ccea63b7613dc827d12abff64f601a1744"),
        _ => None,
    }
}

fn valid_tokenizer_data(model: &str, compressed: &[u8]) -> bool {
    let Some(expected) = tokenizer_sha256(model) else {
        return false;
    };
    let digest = Sha256::digest(compressed);
    let actual: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    if actual != expected {
        return false;
    }
    let decoder = flate2::read::DeflateDecoder::new(compressed);
    let mut json_bytes = Vec::new();
    if decoder
        .take((TOKENIZER_JSON_LIMIT + 1) as u64)
        .read_to_end(&mut json_bytes)
        .is_err()
        || json_bytes.len() > TOKENIZER_JSON_LIMIT
    {
        return false;
    }
    let Ok(data) = serde_json::from_slice::<Value>(&json_bytes) else {
        return false;
    };
    match model {
        "qwen" => {
            data.get("config").is_some_and(Value::is_object)
                && data.get("vocab").is_some_and(Value::is_object)
                && data.get("merges").is_some_and(Value::is_array)
                && data.get("specialTokens").is_some_and(Value::is_array)
        }
        "t5" => {
            data.get("model")
                .and_then(|value| value.get("type"))
                .and_then(Value::as_str)
                == Some("Unigram")
                && data.get("normalizer").is_some()
                && data.get("pre_tokenizer").is_some()
        }
        "clip" => data
            .get("text")
            .and_then(Value::as_str)
            .is_some_and(|text| text.starts_with("bpe_simple_vocab_16e6.txt")),
        _ => false,
    }
}

#[derive(Deserialize)]
struct TokenizerQuery {
    sha256: String,
}

async fn public_tokenizer(
    axum::extract::Path(model): axum::extract::Path<String>,
    Query(query): Query<TokenizerQuery>,
) -> Response {
    let Some(expected_hash) = tokenizer_sha256(&model) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if query.sha256 != expected_hash {
        let mut response = StatusCode::CONFLICT.into_response();
        response
            .headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
        return response;
    }
    let (filename, cache) = match model.as_str() {
        "qwen" => ("qwen35_tokenizer.def", &QWEN_TOKENIZER),
        "t5" => ("t5_tokenizer.def", &T5_TOKENIZER),
        "clip" => ("clip_tokenizer.def", &CLIP_TOKENIZER),
        _ => return StatusCode::NOT_FOUND.into_response(),
    };
    let bytes = cache
        .get_or_try_init(|| async move {
            let client = reqwest::Client::builder()
                .timeout(Duration::from_secs(20))
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .map_err(|_| StatusCode::BAD_GATEWAY)?;
            let response = client
                .get(format!(
                    "https://novelai.net/tokenizer/compressed/{filename}?v=2&static=true"
                ))
                .send()
                .await
                .map_err(|_| StatusCode::BAD_GATEWAY)?;
            if !response.status().is_success() {
                return Err(StatusCode::BAD_GATEWAY);
            }
            if response
                .content_length()
                .is_some_and(|length| length > TOKENIZER_COMPRESSED_LIMIT as u64)
            {
                return Err(StatusCode::BAD_GATEWAY);
            }
            let mut stream = response.bytes_stream();
            let mut bytes = Vec::new();
            while let Some(chunk) = stream.next().await {
                let chunk = chunk.map_err(|_| StatusCode::BAD_GATEWAY)?;
                if bytes.len().saturating_add(chunk.len()) > TOKENIZER_COMPRESSED_LIMIT {
                    return Err(StatusCode::BAD_GATEWAY);
                }
                bytes.extend_from_slice(&chunk);
            }
            // Check the version-pinned static asset before caching it for this process lifetime.
            if !valid_tokenizer_data(model.as_str(), &bytes) {
                return Err(StatusCode::BAD_GATEWAY);
            }
            Ok::<_, StatusCode>(bytes)
        })
        .await;
    let Ok(bytes) = bytes else {
        return StatusCode::BAD_GATEWAY.into_response();
    };
    let mut response = Response::new(Body::from(bytes.clone()));
    *response.status_mut() = StatusCode::OK;
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=31536000, immutable"),
    );
    headers.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    response
}

// ------------------------------------------------------------------ 小件

fn json_text(text: String) -> Response {
    let mut r = Response::new(Body::from(text));
    r.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    r
}

fn no_content() -> Response {
    StatusCode::NO_CONTENT.into_response()
}

/// 请求体的前 `n` 个字节（多余的不读）。
async fn head_bytes(body: Body, n: usize) -> Vec<u8> {
    let mut stream = body.into_data_stream();
    let mut buf: Vec<u8> = Vec::new();
    while buf.len() < n {
        match stream.next().await {
            Some(Ok(chunk)) => buf.extend_from_slice(&chunk),
            _ => break,
        }
    }
    buf.truncate(n);
    buf
}

// ------------------------------------------------------------------ 实时看板

/// 登录的成员：这把 Key 的排队位置（没登录 / 没 Key / Key 被停用为 None）。
async fn viewer_key_id(state: &AppState, headers: &HeaderMap) -> Option<i64> {
    match session::member_state(state, headers).await {
        Ok(Some((_, Some(key)))) if key.enabled() => Some(key.id()),
        _ => None,
    }
}

/// 首页实时架构图：只含汇总数字 + meta；登录的成员附上「我的排队」，看板第一格直接显示排在第几位。
async fn public_live(State(state): State<AppState>, req: Request) -> ApiResult {
    let (parts, _) = req.into_parts();
    let snap = state.live.current(&state).await?;
    let me = viewer_key_id(&state, &parts.headers)
        .await
        .map(|id| live::mine(&state, id));
    Ok(no_store(json_text(snap.render(me.as_ref()))))
}

fn sse_response<S>(stream: S, keepalive: Duration) -> Response
where
    S: futures::Stream<Item = Result<Event, Infallible>> + Send + 'static,
{
    let mut resp = Sse::new(stream)
        .keep_alive(KeepAlive::new().interval(keepalive).text("keep-alive"))
        .into_response();
    let h = resp.headers_mut();
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    // 反向代理（nginx 系）不要缓冲事件流；Caddy 本来就是即时转发
    h.insert("x-accel-buffering", HeaderValue::from_static("no"));
    resp
}

fn too_many_streams() -> Response {
    let mut r = HttpError::new(503, "实时连接太多，请稍后再试").into_response();
    r.headers_mut()
        .insert(header::RETRY_AFTER, HeaderValue::from_static("10"));
    r
}

/// 一条公开 SSE 连接的状态。
struct LiveConn {
    state: AppState,
    rx: watch::Receiver<Option<Arc<Snapshot>>>,
    /// 只用来每 30 秒重验一次成员会话（Key 被停用 / 换新 Key 后「我的排队」要消失）。
    headers: HeaderMap,
    key_id: Option<i64>,
    checked: Instant,
    last: Option<u64>,
    waited: bool,
    _slot: StreamSlot,
}

fn combine(sig: u64, extra: &str) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    sig.hash(&mut h);
    extra.hash(&mut h);
    h.finish()
}

/// 下一条要推的事件：快照每秒更新一次，但只有内容变了（含我的排队）才推。
async fn next_live(mut c: LiveConn) -> Option<(Result<Event, Infallible>, LiveConn)> {
    loop {
        if c.waited {
            tokio::select! {
                _ = c.state.shutdown.cancelled() => return None,
                r = c.rx.changed() => if r.is_err() { return None },
            }
        }
        c.waited = true;
        let Some(snap) = c.rx.borrow_and_update().clone() else {
            continue;
        };
        if c.checked.elapsed() >= Duration::from_secs(30) {
            c.key_id = viewer_key_id(&c.state, &c.headers).await;
            c.checked = Instant::now();
        }
        let me = c.key_id.map(|id| wire_text(&live::mine(&c.state, id)));
        let sig = match &me {
            Some(m) => combine(snap.sig, m),
            None => snap.sig,
        };
        if c.last == Some(sig) {
            continue;
        }
        c.last = Some(sig);
        let text = snap.render_raw(me.as_deref().unwrap_or("null"));
        return Some((Ok(Event::default().data(text)), c));
    }
}

/// 同样的看板数据，SSE 版（3.0 新增）：连上立刻推一条，之后每次内容变化推一条，15 秒无事件发保活注释。
async fn public_live_stream(State(state): State<AppState>, req: Request) -> Response {
    let (parts, _) = req.into_parts();
    let ip = crate::auth::client_ip(&parts, &state.env.forwarded_allow_ips);
    let Some(slot) = state.live.open_stream(&state, StreamKind::Public, &ip) else {
        return too_many_streams();
    };
    let rx = match state.live.subscribe(&state).await {
        Ok(rx) => rx,
        Err(e) => return e.into_response(),
    };
    let key_id = viewer_key_id(&state, &parts.headers).await;
    let keepalive = state.live.keepalive();
    let conn = LiveConn {
        state: state.clone(),
        rx,
        headers: parts.headers,
        key_id,
        checked: Instant::now(),
        last: None,
        waited: false,
        _slot: slot,
    };
    sse_response(futures::stream::unfold(conn, next_live), keepalive)
}

/// 落地页使用的公开状态：不含任何密钥、成员信息或计数细节。缓存 8 秒。
async fn public_status(State(state): State<AppState>) -> ApiResult {
    let body = state.live.status(&state).await?;
    Ok(no_store(ok_json(&body)))
}

async fn queue_status(State(state): State<AppState>) -> ApiResult {
    let now = state.now();
    let mono = state.nai.mono();
    let pool = state.nai.tokens().await;
    let usable: Vec<_> = pool.iter().filter(|t| t.usable(now)).collect();
    let next = usable
        .iter()
        .map(|t| (t.fields().image_next_at - mono).max(0.0))
        .reduce(f64::min)
        .unwrap_or(0.0);
    let lim = state.limits();
    let (active, waiting) = (
        state
            .global_active
            .load(std::sync::atomic::Ordering::Relaxed),
        state
            .global_waiting
            .load(std::sync::atomic::Ordering::Relaxed),
    );
    Ok(ok_json(&json!({
        "global": {
            "active": active,
            "waiting": waiting,
            "concurrency": usable.iter().map(|t| t.image_slots.limit() as i64).sum::<i64>(),
        },
        "image_next_slot_in": gate_core::primitives::round(next, 1),
        "image_cooldown_remaining": state.cooldown.remaining(now),
        "queue_timeout": lim.queue_timeout as i64,
    })))
}

// ------------------------------------------------------------------ 网页脚本报错上报

/// `urllib.parse.urlsplit(url).netloc`
fn netloc(url: &str) -> String {
    // urlsplit 先去掉首部的控制字符 / 空格，并删掉 \t \r \n
    let url: String = url
        .trim_start_matches(|c: char| c <= ' ')
        .chars()
        .filter(|c| !matches!(c, '\t' | '\r' | '\n'))
        .collect();
    let mut rest = url.as_str();
    if let Some(i) = rest.find(':')
        && i > 0
        && rest.as_bytes()[0].is_ascii_alphabetic()
        && rest[..i]
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
    {
        rest = &rest[i + 1..];
    }
    match rest.strip_prefix("//") {
        Some(r) => r.split(['/', '?', '#']).next().unwrap_or("").to_string(),
        None => String::new(),
    }
}

/// 请求明确来自别的站点的页面：取 Origin（没有就取 Referer），其 netloc 解析得出来、Host 也有、两者不同。
/// 解析不出 netloc（如 `Origin: null`）或没带来源一律放行。
fn foreign_page(h: &HeaderMap) -> bool {
    let origin = header_str(h, "origin");
    let from = if origin.is_empty() {
        header_str(h, "referer")
    } else {
        origin
    };
    if from.is_empty() {
        return false;
    }
    let host = header_str(h, "host").to_lowercase();
    let theirs = netloc(&from).to_lowercase();
    !theirs.is_empty() && !host.is_empty() && theirs != host
}

/// 网页登录前，前端把设备指纹的各项（显卡 / 屏幕 / 时区 / 硬件 / Canvas 与 WebGL 的哈希 …）发到这里：
/// `{"data": {...}}`。服务端把各项规整成固定格式（键排序、值一律转成字符串、限长限个数），
/// 指纹 id = 规整结果 sha256 的前 32 位十六进制，按 id 存一份明细，回 `{"id": id, "bid": 浏览器标记}`；前端拿 id 带去 /login?dev=。
/// 同时续种浏览器标记 Cookie `owl_bid`（见 [`BROWSER_COOKIE`]；请求体可带 `bid`，是前端 localStorage 里存的那份）。
/// id 由内容决定：谁也改不了别人那条明细，也没法凭空编一个 id 栽给别人（登录时服务端不认得的 id 记为疑似伪造）。
/// 只接受同源请求、限长、限频；防滥用用途。各项不合格时回 204（前端就不带 dev）。
async fn device_fp(State(state): State<AppState>, req: Request) -> Response {
    let (parts, body) = req.into_parts();
    let h = &parts.headers;
    if foreign_page(h) {
        return no_content();
    }
    let ip = client_ip(&parts, &state.env.forwarded_allow_ips);
    if !state.live.device_fp_slot(&ip, state.now()) {
        return no_content();
    }
    let raw = head_bytes(body, 8192).await;
    // 浏览器标记：Cookie 优先；没有就认前端 localStorage 里的那份（只清 Cookie 不会换标记）；都没有就新发一个
    let bid = session::cookie(h, BROWSER_COOKIE)
        .filter(|b| valid_browser_id(b))
        .or_else(|| {
            serde_json::from_slice::<Value>(&raw)
                .ok()?
                .get("bid")?
                .as_str()
                .filter(|b| valid_browser_id(b))
                .map(str::to_string)
        })
        .unwrap_or_else(|| hex::encode(rand::random::<[u8; 16]>()));
    let bid_cookie = session::set_cookie_header(
        BROWSER_COOKIE,
        &bid,
        BROWSER_COOKIE_DAYS * 86400,
        true,
        "lax",
    );
    let with_bid = |mut resp: Response| {
        resp.headers_mut().append(header::SET_COOKIE, bid_cookie);
        resp
    };
    let Some((id, canonical)) = canonical_device_fp(&raw) else {
        return with_bid(no_content());
    };
    if let Err(e) = state
        .db
        .upsert_device_fingerprint(&id, &canonical, state.now())
        .await
    {
        // 存不了就不给 id：登录时服务端认不出来，会被误记成「疑似伪造」
        tracing::warn!(target: "web-login", "记设备指纹失败：{e}");
        return with_bid(no_content());
    }
    // 补充项和电量（站长 10-23）：按 (设备指纹, 浏览器标记) 另存，尽力而为
    if let Ok(v) = serde_json::from_slice::<Value>(&raw) {
        let extra = canonical_device_extra(&v);
        let battery = device_battery(&v);
        if extra.is_some() || battery.is_some() {
            let (xid, data) = extra.unwrap_or_default();
            if let Err(e) = state
                .db
                .upsert_device_extra(&id, &bid, &data, &xid, battery, state.now())
                .await
            {
                tracing::warn!(target: "web-login", "记设备指纹补充项失败：{e}");
            }
        }
    }
    with_bid(ok_json(&json!({"id": id, "bid": bid})))
}

/// 浏览器标记 Cookie：第一次打开本站时种下的长期随机 ID（前端在 localStorage 另存一份）。
/// 同一个浏览器换 Discord 账号登录时标记不变（设备指纹变了也一样）= 几乎可以确定是同一个人。防滥用用途。
pub(crate) const BROWSER_COOKIE: &str = "owl_bid";
/// 浏览器标记 Cookie 的有效期（Chrome 最多认 400 天）；每次请求 /public/device-fp 都会续期。
const BROWSER_COOKIE_DAYS: i64 = 400;

/// 浏览器标记的格式：32 位小写十六进制（16 个随机字节）。
pub(crate) fn valid_browser_id(b: &str) -> bool {
    b.len() == 32 && b.bytes().all(|c| matches!(c, b'0'..=b'9' | b'a'..=b'f'))
}

/// 设备指纹最多收几项、每项的键 / 值最长多少字。
const FP_MAX_PARTS: usize = 32;
const FP_MAX_KEY: usize = 32;
const FP_MAX_VALUE: usize = 512;

/// 规整设备指纹各项：键只留字母数字下划线（最多 32 字），值转成字符串（最多 512 字；嵌套的数组 / 对象丢掉），
/// 按键排序后序列化成 JSON；返回 (id, 规整后的 JSON)。没有可用的项返回 None。
fn canonical_device_fp(raw: &[u8]) -> Option<(String, String)> {
    let v: Value = serde_json::from_slice(raw).ok()?;
    canonical_parts(v.get("data")?.as_object()?)
}

/// 设备指纹的补充项（请求体的 `extra`，站长 10-23）：同样规整，返回 (sha256 前 16 位, 规整后的 JSON)。
/// 和 `data` 分开算：指纹 id 不受补充项影响。
fn canonical_device_extra(v: &Value) -> Option<(String, String)> {
    let (id, canonical) = canonical_parts(v.get("extra")?.as_object()?)?;
    Some((id[..16].to_string(), canonical))
}

/// 电量快照（请求体的 `battery: {level, charging}`）：level 须在 0–1 之间。
fn device_battery(v: &Value) -> Option<gate_db::repo::footprint::Battery> {
    let b = v.get("battery")?;
    let level = b
        .get("level")?
        .as_f64()
        .filter(|l| (0.0..=1.0).contains(l))?;
    let charging = b.get("charging")?.as_bool()?;
    Some(gate_db::repo::footprint::Battery { level, charging })
}

/// 规整一组指纹分项，返回 (sha256 前 32 位, 规整后的 JSON)。
fn canonical_parts(data: &serde_json::Map<String, Value>) -> Option<(String, String)> {
    use sha2::{Digest, Sha256};
    let mut out = std::collections::BTreeMap::new();
    for (k, val) in data {
        let key: String = k
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '_')
            .take(FP_MAX_KEY)
            .collect();
        if key.is_empty() {
            continue;
        }
        let text = match val {
            Value::String(s) => s.clone(),
            Value::Number(n) => n.to_string(),
            Value::Bool(b) => b.to_string(),
            Value::Null => String::new(),
            Value::Array(_) | Value::Object(_) => continue,
        };
        out.insert(key, take_chars(&text, FP_MAX_VALUE).to_string());
    }
    // 键排好序后截前 32 项：同样的输入永远得到同样的结果
    let out: std::collections::BTreeMap<String, String> =
        out.into_iter().take(FP_MAX_PARTS).collect();
    if out.values().all(String::is_empty) {
        return None;
    }
    let canonical = serde_json::to_string(&out).ok()?;
    let id = hex::encode(Sha256::digest(canonical.as_bytes()))[..32].to_string();
    Some((id, canonical))
}

// ------------------------------------------------------------------ 客户端兼容视图（/user/*）

/// 虚拟订阅视图：数字由 gate_core 算（纯函数），这里只负责把库里的数查好。
async fn subscription(state: &AppState, key: &Key) -> Result<Value, InternalError> {
    let (day, month) = (state.day(), state.month());
    let shared = state.db.shared_budgets().await?;
    let data = gate_core::client_views::SubscriptionData {
        month_anlas_used: state.db.month_anlas(key.id(), &month).await?,
        daily_v5_used: state.db.get_counter(key.id(), &day).await?.v5,
        global_v5_limit: shared.daily_v5,
        global_v5_total: state.db.day_v5_total(&day).await?,
        v5_paused: crate::ops::v5_paused(&state.db).await?,
    };
    Ok(gate_core::client_views::subscription_payload(
        &key.policy,
        key.expires_at(),
        state.now(),
        &data,
    ))
}

/// 官方 /user/priority 的形状（2026-10-13 实测 image.novelai.net 返回 200）：
/// {maxPriorityActions, nextRefillAt, taskPriority}。本站没有「买优先队列」这回事，
/// 如实回 0 次优先操作、下个整点刷新，别编造余额。
fn virtual_priority(now: f64) -> Value {
    let now = now as i64;
    json!({"maxPriorityActions": 0, "nextRefillAt": now + (3600 - now % 3600), "taskPriority": 10})
}

/// 给原生 NAI 客户端的虚拟订阅视图，不暴露真实上游账户余额。
async fn user_subscription(State(state): State<AppState>, req: Request) -> ApiResult {
    let (parts, _) = req.into_parts();
    let key = authenticate(&state, &parts, false).await?;
    Ok(ok_json(&subscription(&state, &key).await?))
}

async fn user_data(State(state): State<AppState>, req: Request) -> ApiResult {
    let (parts, _) = req.into_parts();
    let key = authenticate(&state, &parts, false).await?;
    let sub = subscription(&state, &key).await?;
    let paused = crate::ops::v5_paused(&state.db).await?;
    let public_sub: Map<String, Value> = sub
        .as_object()
        .map(|m| {
            m.iter()
                .filter(|(k, _)| k.as_str() != "naiGate")
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect()
        })
        .unwrap_or_default();
    let steps = sub["trainingStepsLeft"].clone();
    // 官网生图页登录后必须拿到 keystore 才能进登录态（客户端自己加密，服务端只存密文）。
    let keystore = state
        .db
        .get_key_keystore(key.id())
        .await?
        .map(|(blob, change_index)| json!({"keystore": blob, "changeIndex": change_index}));
    // 官网页面按 settings.defaultModel 选默认模型；本站默认用 V4.5（V5 可能没给这把 Key 开）
    let settings = state
        .db
        .get_key_settings(key.id())
        .await?
        .or_else(|| Some(r#"{"defaultModel":"nai-diffusion-4-5-curated"}"#.to_string()));
    Ok(ok_json(&json!({
        "subscription": public_sub,
        "priority": virtual_priority(state.now()),
        "trainingStepsLeft": steps,
        "anlas": steps["fixedTrainingStepsLeft"],
        "v5Access": {"allowed": key.policy.allows("v5"), "paused": paused},
        "keystore": keystore,
        "settings": settings,
    })))
}

/// 原生客户端会探这个端点；404 会被当成「渠道不支持」。官方是 200，我们也回同样的形状。
async fn user_priority(State(state): State<AppState>, req: Request) -> ApiResult {
    let (parts, _) = req.into_parts();
    authenticate(&state, &parts, false).await?;
    Ok(ok_json(&virtual_priority(state.now())))
}

/// 官网生图页的密钥库：客户端用本地钥匙加密后存这里，服务端只做不透明存取。
async fn user_keystore(State(state): State<AppState>, req: Request) -> ApiResult {
    let (parts, _) = req.into_parts();
    let key = authenticate(&state, &parts, false).await?;
    let stored = state.db.get_key_keystore(key.id()).await?;
    Ok(ok_json(&match stored {
        Some((blob, change_index)) => json!({"keystore": blob, "changeIndex": change_index}),
        None => json!({"keystore": null, "changeIndex": 0}),
    }))
}

#[derive(Deserialize)]
struct KeystoreBody {
    #[serde(default)]
    keystore: Option<String>,
    #[serde(default, rename = "changeIndex")]
    change_index: Option<i64>,
}

async fn user_keystore_put(State(state): State<AppState>, req: Request) -> ApiResult {
    let (parts, body) = req.into_parts();
    let key = authenticate(&state, &parts, false).await?;
    let bytes = axum::body::to_bytes(body, 1 << 20).await.map_err(|_| {
        crate::error::GateError::with_code(413, "密钥库太大", gate_core::reasons::PARAM)
    })?;
    let parsed: KeystoreBody = serde_json::from_slice(&bytes).map_err(|_| {
        crate::error::GateError::with_code(400, "密钥库格式无效", gate_core::reasons::PARAM)
    })?;
    let blob = parsed.keystore.unwrap_or_default();
    state
        .db
        .set_key_keystore(
            key.id(),
            &blob,
            parsed.change_index.unwrap_or(0),
            state.now(),
        )
        .await?;
    Ok(ok_json(&json!({"ok": true})))
}

/// 官网客户端设置：主题、默认模型、许可标记等整块 JSON。
async fn user_settings(State(state): State<AppState>, req: Request) -> ApiResult {
    let (parts, _) = req.into_parts();
    let key = authenticate(&state, &parts, false).await?;
    let stored = state.db.get_key_settings(key.id()).await?;
    Ok(ok_json(&match stored {
        Some(json) => serde_json::from_str::<Value>(&json).unwrap_or_else(|_| json!({})),
        None => json!({}),
    }))
}

async fn user_settings_put(State(state): State<AppState>, req: Request) -> ApiResult {
    let (parts, body) = req.into_parts();
    let key = authenticate(&state, &parts, false).await?;
    let bytes = axum::body::to_bytes(body, 1 << 20).await.map_err(|_| {
        crate::error::GateError::with_code(413, "设置太大", gate_core::reasons::PARAM)
    })?;
    let text = String::from_utf8_lossy(&bytes).to_string();
    if serde_json::from_str::<Value>(&text).is_err() {
        return Err(crate::error::GateError::with_code(
            400,
            "设置格式无效",
            gate_core::reasons::PARAM,
        )
        .into());
    }
    state
        .db
        .set_key_settings(key.id(), &text, state.now())
        .await?;
    Ok(ok_json(&json!({"ok": true})))
}

/// 空对象：官网镜像页会探测、但本站没有对应能力的接口（/user/self、/usage/link）。
async fn empty_object() -> ApiResult {
    Ok(ok_json(&json!({})))
}

/// 官网前端把礼包当成数组直接 `.json()` 用，回 `[]` 比 `{}` 安全。
async fn empty_list() -> ApiResult {
    Ok(ok_json(&json!([])))
}

/// 官网对象库（提示词宏、预设、脚本…）：本站不存远端对象，回空列表。
async fn empty_objects(State(state): State<AppState>, req: Request) -> ApiResult {
    let (parts, _) = req.into_parts();
    authenticate(&state, &parts, false).await?;
    Ok(ok_json(&json!({"objects": []})))
}

/// 官网文本后端的试用额度：本站没有试用，回 0（前端不再提示试用次数）。
async fn trial_status(State(state): State<AppState>, req: Request) -> ApiResult {
    let (parts, _) = req.into_parts();
    authenticate(&state, &parts, false).await?;
    Ok(ok_json(&json!({
        "remaining_image_actions": 0,
        "remaining_text_actions": 0,
    })))
}

/// 官网背包：本站没有道具系统，回空列表（前端按空背包渲染）。
async fn user_inventory(State(state): State<AppState>, req: Request) -> ApiResult {
    let (parts, _) = req.into_parts();
    authenticate(&state, &parts, false).await?;
    Ok(ok_json(&json!({"items": []})))
}

#[derive(Deserialize)]
struct LoginBody {
    #[serde(default)]
    key: Option<String>,
    #[serde(default)]
    access_key: Option<String>,
}

/// 官网登录入口：body `{key, recaptcha}`，成功返回 `{accessToken}`；本站的 Key 本身就是令牌。
async fn user_login(State(state): State<AppState>, req: Request) -> ApiResult {
    let (_parts, body) = req.into_parts();
    let bytes = axum::body::to_bytes(body, 64 * 1024).await.map_err(|_| {
        crate::error::GateError::with_code(413, "登录请求太大", gate_core::reasons::PARAM)
    })?;
    let parsed: LoginBody = serde_json::from_slice(&bytes).map_err(|_| {
        crate::error::GateError::with_code(400, "登录请求格式无效", gate_core::reasons::PARAM)
    })?;
    let token = parsed
        .key
        .or(parsed.access_key)
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .ok_or_else(|| {
            crate::error::GateError::with_code(400, "缺少 Key", gate_core::reasons::PARAM)
        })?;
    let Some(row) = state.db.get_key_by_token(&token).await? else {
        return Err(crate::error::GateError::with_code(
            401,
            "API Key 无效。请检查客户端中的 Key 是否完整，或在 Discord 用 /resetkey 重新获取。",
            gate_core::reasons::KEY_INVALID,
        )
        .into());
    };
    if row["enabled"].as_i64().unwrap_or(1) != 1 {
        return Err(crate::error::GateError::with_code(
            403,
            "这个 Key 已停用。",
            gate_core::reasons::KEY_INVALID,
        )
        .into());
    }
    Ok(ok_json(&json!({"accessToken": token})))
}

async fn user_information(State(state): State<AppState>, req: Request) -> ApiResult {
    let (parts, _) = req.into_parts();
    let key = authenticate(&state, &parts, false).await?;
    let now = state.now();
    // int(key["expires_at"] or time.time() + 3650 * 86400)
    let expires = key
        .expires_at()
        .filter(|e| *e != 0.0)
        .map_or((now + 3650.0 * 86400.0) as i64, |e| e as i64);
    Ok(ok_json(&json!({
        "tier": 3, "active": true, "username": key.name(), "expiresAt": expires,
    })))
}

// ------------------------------------------------------------------ 后台 SSE

/// 后台 SSE 的最短推送间隔与保底心跳。页面把任何一条消息都当作「该刷新了」（admin/Admin.tsx 的 `onmessage = bump`），
/// 所以：真有变化才推，且至少隔 15 秒——后台每次刷新会重拉成员/Key/角标（几百 KB），
/// 2 秒一次在队列忙时等于持续打满 DB；15 秒对单人后台足够新。
/// 没变化也每 30 秒推一条心跳（日志 / Bug / 成员列表这些不在快照里的数据靠它更新）。
const ADMIN_MIN_GAP: Duration = Duration::from_secs(15);
const ADMIN_HEARTBEAT: Duration = Duration::from_secs(30);
const BADGE_TTL: Duration = Duration::from_secs(5);

struct AdminConn {
    state: AppState,
    rx: watch::Receiver<Option<Arc<Snapshot>>>,
    headers: HeaderMap,
    badges: Option<(Instant, String)>,
    last_sig: Option<u64>,
    last_sent: Option<Instant>,
    _slot: StreamSlot,
}

/// 侧栏角标用的数字：未处理 Bug。
async fn badges(c: &mut AdminConn) -> String {
    if let Some((at, text)) = &c.badges
        && at.elapsed() < BADGE_TTL
    {
        return text.clone();
    }
    let text = "{}".to_string();
    c.badges = Some((Instant::now(), text.clone()));
    text
}

async fn next_admin(mut c: AdminConn) -> Option<(Result<Event, Infallible>, AdminConn)> {
    loop {
        if let Some(sent) = c.last_sent {
            let due = sent + ADMIN_HEARTBEAT;
            tokio::select! {
                _ = c.state.shutdown.cancelled() => return None,
                r = c.rx.changed() => if r.is_err() { return None },
                _ = tokio::time::sleep_until(due) => {}
            }
        }
        // 改了密码 / 会话过期：结束这条流（EventSource 重连会拿到 401）
        let raw = session::cookie(&c.headers, ADMIN_COOKIE).unwrap_or_default();
        if !session::check_admin_cookie(&c.state, &raw) {
            return None;
        }
        let Some(snap) = c.rx.borrow_and_update().clone() else {
            if c.last_sent.is_none() {
                c.last_sent = Some(Instant::now());
            }
            continue;
        };
        let badge_text = badges(&mut c).await;
        let sig = combine(snap.admin_sig, &badge_text);
        let first = c.last_sent.is_none();
        let since = c.last_sent.map(|t| t.elapsed());
        let changed = c.last_sig != Some(sig);
        let heartbeat = since.is_some_and(|s| s >= ADMIN_HEARTBEAT);
        let gap_ok = since.is_none_or(|s| s >= ADMIN_MIN_GAP);
        if !(first || heartbeat || (changed && gap_ok)) {
            continue;
        }
        c.last_sig = Some(sig);
        c.last_sent = Some(Instant::now());
        let text = format!("{{\"live\":{},\"badges\":{}}}", snap.json(), badge_text);
        return Some((Ok(Event::default().data(text)), c));
    }
}

/// 后台实时事件：快照（与 /public/live 同一份，不含「我的排队」）+ 角标数字。需要后台会话。
async fn admin_events(State(state): State<AppState>, _s: AdminSession, req: Request) -> Response {
    let (parts, _) = req.into_parts();
    let ip = crate::auth::client_ip(&parts, &state.env.forwarded_allow_ips);
    let Some(slot) = state.live.open_stream(&state, StreamKind::Admin, &ip) else {
        return too_many_streams();
    };
    let rx = match state.live.subscribe(&state).await {
        Ok(rx) => rx,
        Err(e) => return e.into_response(),
    };
    let keepalive = state.live.keepalive();
    let conn = AdminConn {
        state: state.clone(),
        rx,
        headers: parts.headers,
        badges: None,
        last_sig: None,
        last_sent: None,
        _slot: slot,
    };
    sse_response(futures::stream::unfold(conn, next_admin), keepalive)
}

// ------------------------------------------------------------------ 首页首屏数据

/// 登录成员的个人状态（`/public/me` 的内容）。
pub(super) async fn me_payload(
    state: &AppState,
    discord_id: &str,
    key: Option<&Key>,
) -> Result<Value, InternalError> {
    let discord = if state.env.registration.is_some() {
        match state.db.discord_profile_row(discord_id).await? {
            None => json!({"id": discord_id}),
            Some((username, display_name, avatar)) => {
                json!({"id": discord_id, "username": username, "display_name": display_name, "avatar": avatar})
            }
        }
    } else {
        Value::Null
    };
    let mut out = Map::new();
    out.insert("logged_in".into(), json!(true));
    out.insert("discord".into(), discord);
    // 站长 10-24：「网页登录一次」闸门生效时，首页要能提示受影响成员，而不是等出图 403。
    if crate::auth::requires_web_login(state, discord_id).await {
        out.insert("requires_web_login".into(), json!(true));
    }
    let Some(key) = key.filter(|k| k.enabled()) else {
        out.insert("has_key".into(), json!(false));
        return Ok(Value::Object(out));
    };
    let c = state.db.get_counter(key.id(), &state.day()).await?;
    let mut queue = state
        .guard
        .with(|g| g.queue_view(Some(key.id())))
        .get("mine")
        .cloned()
        .unwrap_or(json!([]));
    // 预计开始时刻只给实时看板（`live::mine` 换算成 eta），这里保持原来的字段
    if let Value::Array(items) = &mut queue {
        for item in items.iter_mut().filter_map(Value::as_object_mut) {
            item.remove("start_at");
        }
    }
    // 全站今天还剩多少张 V5（首页 V5 圆环取「个人剩余」和它的较小值）。
    // null = 不受全站限制（只剩一种情况：这把 Key 不计入全站 V5）。10-27 起全站池 0 = 停用（0 剩余）。
    let g = i64::from(state.db.shared_budgets().await?.daily_v5);
    let paused = crate::ops::v5_paused(&state.db).await?;
    let allowed = key.policy.allows("v5");
    let used = state.db.day_v5_total(&state.day()).await?;
    let site_v5_left = if paused || !allowed {
        0
    } else {
        (g - used).max(0)
    };
    let now = state.now();
    out.insert("has_key".into(), json!(true));
    out.insert("key".into(), json!(key.token()));
    out.insert("name".into(), json!(key.name()));
    out.insert("expires_at".into(), json!(key.expires_at()));
    out.insert("policy".into(), json!(&key.policy));
    out.insert(
        "today".into(),
        json!({
            "images": c.images, "v5": c.v5, "legacy": c.legacy_free_images,
            "daily_images": key.daily_images(), "daily_v5": key.daily_v5(),
            "site_v5_left": site_v5_left, "site_v5_cap": g,
            "upstream_v5_free": live::upstream_v5_free(state).await && !paused,
            "v5_paused": paused,
            "v5_access_allowed": allowed,
            "v5_access_reason": if allowed { "" } else { "此成员未开放 V5" },
            "anlas": gate_core::primitives::round0(c.anlas),
            "daily_anlas": live::member_anlas_cap(state, key).await?,
            "reset_hour": state.guard.with(|g| g.reset_hour),
        }),
    );
    out.insert("queue".into(), queue);
    out.insert(
        "coupons".into(),
        json!(state.db.list_coupons(key.id(), true, now).await?),
    );
    out.insert(
        "timing".into(),
        match state.db.user_image_timing(key.id(), now).await {
            Ok(t) => serde_json::to_value(t).unwrap_or(Value::Null),
            Err(_) => Value::Null,
        },
    );
    Ok(Value::Object(out))
}

/// 首页 HTML 里的 `<script type="application/json" id="owl-boot">`：`{me, live, status}`，
/// 形状与 `/public/me`、`/public/live`、`/public/status` 的响应一样（web/src/pages/landing/store.ts 的 `Boot`）。
/// 页面拿到就不用再等三个请求；任何一块取不到就缺省，页面自己走接口。
pub async fn landing_boot(state: &AppState, headers: &HeaderMap) -> Option<Value> {
    let mut boot = Map::new();
    let session = session::member_state(state, headers).await.ok().flatten();
    let mut key_id = None;
    match &session {
        None => {
            boot.insert("me".into(), json!({"logged_in": false}));
        }
        Some((discord_id, key)) => {
            key_id = key.as_ref().filter(|k| k.enabled()).map(Key::id);
            match me_payload(state, discord_id, key.as_ref()).await {
                Ok(me) => {
                    boot.insert("me".into(), me);
                }
                Err(e) => tracing::warn!(target: "boot", "首屏 me 取数失败：{}", e.0),
            }
        }
    }
    match state.live.current(state).await {
        Ok(snap) => {
            boot.insert(
                "live".into(),
                snap.value(key_id.map(|id| live::mine(state, id))),
            );
        }
        Err(e) => tracing::warn!(target: "boot", "首屏 live 取数失败：{}", e.0),
    }
    match state.live.status(state).await {
        Ok(status) => {
            boot.insert("status".into(), (*status).clone());
        }
        Err(e) => tracing::warn!(target: "boot", "首屏 status 取数失败：{}", e.0),
    }
    (!boot.is_empty()).then_some(Value::Object(boot))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenizer_asset_names_are_whitelisted_and_version_pinned() {
        assert!(tokenizer_sha256("qwen").is_some());
        assert!(tokenizer_sha256("t5").is_some());
        assert!(tokenizer_sha256("clip").is_some());
        assert!(tokenizer_sha256("https://example.com/file").is_none());
    }

    #[test]
    fn netloc_like_urlsplit() {
        assert_eq!(netloc("https://Gate.Example.top"), "Gate.Example.top");
        assert_eq!(
            netloc("https://gate.example.top:8443/a?b#c"),
            "gate.example.top:8443"
        );
        assert_eq!(netloc("https://u:p@h.test/x"), "u:p@h.test");
        assert_eq!(netloc("null"), "");
        assert_eq!(netloc("//h.test/x"), "h.test");
        assert_eq!(netloc("h.test/x"), "");
        assert_eq!(netloc(" \thttps://h.te\nst/x"), "h.test");
    }
}
