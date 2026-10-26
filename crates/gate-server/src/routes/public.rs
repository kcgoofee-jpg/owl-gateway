//! 公开页面、实时看板、状态与客户端兼容视图。
//!
//! - `/public/live`（JSON）、`/public/live/stream`（SSE，3.0 新增）、`/public/status`、`/queue-status`；
//! - `POST /public/client-error`：网页脚本报错上报；
//! - `/user/*`、`/ai/user/*`：给原生 NAI 客户端的虚拟订阅 / 数据 / 优先级 / 密钥库 / 账户信息；
//! - `GET /admin/api/events`（3.0 新增）：后台 SSE（实时快照 + 角标数字）；
//! - 首页首屏数据（[`landing_boot`]，由 pages.rs 的 `/` 使用）。
//!
//! `/healthz`、`/`、`/admin*`、`/_owl/*`、图标在 pages.rs。

use super::registration::no_store;
use crate::auth::{authenticate, client_ip, ctx_of, header_str};
use crate::error::{ApiResult, HttpError, InternalError, ok_json};
use crate::key::Key;
use crate::live::{self, Snapshot, StreamKind, StreamSlot, wire_text};
use crate::session::{self, ADMIN_COOKIE, AdminSession};
use crate::state::AppState;
use axum::Router;
use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use futures::StreamExt;
use gate_core::pyc::{strip, take_chars};
use serde_json::{Map, Value, json};
use std::convert::Infallible;
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use tokio::sync::watch;
use tokio::time::{Duration, Instant};

pub fn router() -> Router<AppState> {
    let mut r = Router::new()
        .route("/public/live", get(public_live))
        .route("/public/live/stream", get(public_live_stream))
        .route("/public/client-error", post(client_error))
        .route("/public/device-fp", post(device_fp))
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
        r = r.route(p, get(user_keystore));
    }
    for p in ["/user/information", "/ai/user/information"] {
        r = r.route(p, get(user_information));
    }
    r
}

// ------------------------------------------------------------------ 小件

/// 已经序列化好的 JSON 文本 → 响应（与 Starlette `JSONResponse` 一样的 content-type）。
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

/// 请求体解析成 JSON 对象；读不懂或不是对象返回 None。
fn loads_object(raw: &[u8]) -> Option<serde_json::Map<String, Value>> {
    match serde_json::from_slice::<Value>(raw).ok()? {
        Value::Object(d) => Some(d),
        _ => None,
    }
}

/// 假值：null / false / 0 / "" / [] / {}。
fn falsy(v: &Value) -> bool {
    match v {
        Value::Null => true,
        Value::Bool(b) => !*b,
        Value::Number(n) => n.as_f64() == Some(0.0),
        Value::String(s) => s.is_empty(),
        Value::Array(a) => a.is_empty(),
        Value::Object(o) => o.is_empty(),
    }
}

/// 值转文本：字符串原样，其它用 JSON 写法。
fn value_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// 字段文本：缺失或假值给空串。
fn text_of(d: &serde_json::Map<String, Value>, name: &str) -> String {
    d.get(name)
        .filter(|v| !falsy(v))
        .map(value_text)
        .unwrap_or_default()
}

// ------------------------------------------------------------------ 实时看板

/// 登录的成员：这把 Key 的排队位置（没登录 / 没 Key / Key 被停用为 None）。
async fn viewer_key_id(state: &AppState, headers: &HeaderMap) -> Option<i64> {
    match session::member_state(state, headers).await {
        Ok(Some((_, Some(key)))) if key.enabled() => Some(key.id()),
        _ => None,
    }
}

/// 首页实时架构图：只含汇总数字 + meta；登录的成员附上「我的排队」，看板第一格直接显示排第几。
async fn public_live(State(state): State<AppState>, req: Request) -> ApiResult {
    let (parts, _) = req.into_parts();
    let snap = state.live.current(&state).await?;
    let me = viewer_key_id(&state, &parts.headers)
        .await
        .map(|id| live::mine(&state, id, &snap));
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
        let me = c
            .key_id
            .map(|id| wire_text(&live::mine(&c.state, id, &snap)));
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
        "image_next_slot_in": gate_core::pyc::round(next, 1),
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

/// 钱包类浏览器扩展（MetaMask 等）往每个页面注入 window.ethereum，断线时抛出的错：没有堆栈，文字固定，跟本站无关。
const EXTENSION_MESSAGES: &[&str] = &[
    "disconnected from all chains", // EIP-1193 错误 4901，MetaMask / 各类钱包
    "metamask",
    "chrome-extension://",
    "moz-extension://",
];

/// 手机自带浏览器（荣耀 / 华为 / 小米等）、翻译、广告和钱包插件往页面注入的脚本出的错：我们修不了，不进 Bug 追踪。
/// 特征：堆栈里有调用帧，但没有一帧来自本站文件（全是 <anonymous> / 扩展协议）；或者是没有来源的「Script error.」；
/// 或者是已知的钱包扩展报错文字（没有堆栈，只能按文字认）。
fn injected_script_error(msg: &str, stack: &str, host: &str) -> bool {
    if strip(msg) == "Script error." {
        return true;
    }
    let lower = msg.to_lowercase();
    if EXTENSION_MESSAGES.iter().any(|m| lower.contains(m)) {
        return true;
    }
    let frames: Vec<&str> = stack
        .lines()
        .map(strip)
        .filter(|l| l.starts_with("at ") || l.contains('@'))
        .collect();
    if frames.is_empty() {
        return false;
    }
    // 前端文件在 /_owl/（/static/ 也会一起查，老链接不 404）
    let ours: Vec<&str> = [
        "/static/",
        "/_owl/",
        "/admin",
        host,
        "127.0.0.1",
        "localhost",
    ]
    .into_iter()
    .filter(|x| !x.is_empty())
    .collect();
    !frames.iter().any(|f| ours.iter().any(|o| f.contains(o)))
}

/// 报错上报里的文本：连续的控制字符（`\x00-\x1f` `\x7f`）换成一个空格，去首尾空白，最多 256 个字符。
fn one_line(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_ctrl = false;
    for c in text.chars() {
        if c <= '\u{1f}' || c == '\u{7f}' {
            if !in_ctrl {
                out.push(' ');
                in_ctrl = true;
            }
        } else {
            out.push(c);
            in_ctrl = false;
        }
    }
    take_chars(strip(&out), 256).to_string()
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

/// 首页 / 后台网页的脚本报错上报（只收本站页面的错误；每 IP 每 10 分钟 10 条，全站每 10 分钟 100 条）。
///
/// 3.0 起另外丢弃两类：浏览器网络失败（断网 / 请求被取消，不是我们的缺陷），
/// 以及出错脚本不在 `/_owl/` 下的 `Uncaught …`（多半是浏览器插件）。
async fn client_error(State(state): State<AppState>, req: Request) -> Response {
    let (parts, body) = req.into_parts();
    let h = &parts.headers;
    if foreign_page(h) {
        return no_content();
    }
    let host = header_str(h, "host").to_lowercase();
    let ip = client_ip(&parts, &state.env.forwarded_allow_ips);
    if !state.live.client_error_slot(&ip, state.now()) {
        return no_content();
    }
    let raw = head_bytes(body, 4096).await;
    let raw = if raw.is_empty() { b"{}".to_vec() } else { raw };
    let Some(d) = loads_object(&raw) else {
        return no_content();
    };
    let d = &d;
    let page = take_chars(&text_of(d, "page"), 20).to_string();
    let page = if page == "landing" || page == "admin" {
        page
    } else {
        "other".to_string()
    };
    let msg = one_line(&text_of(d, "msg"));
    let msg = take_chars(&msg, 200);
    let msg = if msg.is_empty() { "unknown" } else { msg };
    let src = text_of(d, "src");
    let place = one_line(&format!("{src}:{}", text_of(d, "line")));
    let stack = take_chars(&text_of(d, "stack"), 1500).to_string();
    // 3.0 前端对脚本错误（ErrorEvent）带 `filename`，Promise 拒绝没有
    let filename = match d.get("filename") {
        Some(Value::String(f)) => Some(f.as_str()),
        _ => None,
    };
    if gate_core::errors::is_network_failure(msg)
        || injected_script_error(msg, &stack, &host)
        || gate_core::errors::is_foreign_script_report(msg, filename, &src, &stack)
    {
        return no_content();
    }
    let ua = one_line(&header_str(h, "user-agent"));
    let source = format!("web:{page}");
    let detail = format!("{place}\n{ua}\n{stack}");
    state.bugs.capture(crate::bugs::Report {
        source: &source,
        title: msg,
        detail: &detail,
        path: &page,
        rid: &ctx_of(&parts).rid(),
        level: "warn",
        ..Default::default()
    });
    no_content()
}

// ------------------------------------------------------------------ 客户端兼容视图（/user/*）

/// 虚拟订阅视图：数字由 gate_core 算（纯函数），这里只负责把库里的数查好。
async fn subscription(state: &AppState, key: &Key) -> Result<Value, InternalError> {
    let (day, month) = (state.day(), state.month());
    let access = crate::v5_access::evaluate(state, Some(key.str("discord_id"))).await?;
    let data = gate_core::client_views::SubscriptionData {
        month_anlas_used: state.db.month_anlas(key.id(), &month).await?,
        daily_v5_used: state.db.get_counter(key.id(), &day).await?.v5,
        global_v5_limit: crate::ops::global_daily_v5(&state.db).await?,
        global_v5_total: state.db.day_v5_total(&day).await?,
        v5_paused: access.paused,
        v5_access_allowed: access.decision.allowed,
        global_quota_modes: crate::ops::global_quota_modes(&state.db).await?,
    };
    gate_core::client_views::subscription_payload(&key.0, state.now(), &data)
        .map_err(|e| InternalError(e.to_string()))
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
    let access = crate::v5_access::evaluate(&state, Some(key.str("discord_id"))).await?;
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
    Ok(ok_json(&json!({
        "subscription": public_sub,
        "priority": virtual_priority(state.now()),
        "trainingStepsLeft": steps,
        "anlas": steps["fixedTrainingStepsLeft"],
        "v5Access": {"mode": access.mode.as_str(), "allowed": access.decision.allowed, "paused": access.paused},
    })))
}

/// 原生客户端会探这个端点；404 会被当成「渠道不支持」。官方是 200，我们也回同样的形状。
async fn user_priority(State(state): State<AppState>, req: Request) -> ApiResult {
    let (parts, _) = req.into_parts();
    authenticate(&state, &parts, false).await?;
    Ok(ok_json(&virtual_priority(state.now())))
}

/// 官方这个端点只允许浏览器会话（2026-10-13 实测：api.novelai.net → 403「Persistent access token is not
/// allowed to access this endpoint.」）。照官方的说法回 403，客户端就知道这是「令牌无权访问」而不是「网关坏了」。
async fn user_keystore(State(state): State<AppState>, req: Request) -> ApiResult {
    let (parts, _) = req.into_parts();
    authenticate(&state, &parts, false).await?;
    Err(crate::error::GateError::with_code(
        403,
        "该功能仅限 NovelAI 官网使用，本站不提供，未扣费。开发者参考：NovelAI 官方对持久令牌同样返回 403", gate_core::reasons::FEATURE_OFF)
    .into())
}

async fn user_information(State(state): State<AppState>, req: Request) -> ApiResult {
    let (parts, _) = req.into_parts();
    let key = authenticate(&state, &parts, false).await?;
    let now = state.now();
    // int(key["expires_at"] or time.time() + 3650 * 86400)
    let expires = key
        .opt_float("expires_at")
        .filter(|e| *e != 0.0)
        .map_or((now + 3650.0 * 86400.0) as i64, |e| e as i64);
    Ok(ok_json(&json!({
        "tier": 3, "active": true, "username": key.get("name"), "expiresAt": expires,
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
    let bugs = c.state.db.open_error_count().await.unwrap_or(0);
    let text = format!("{{\"bugs\":{bugs}}}");
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
    // null = 不受全站限制：全站不设限（global_daily_v5=0），或这把 Key 不计入全站 V5
    let g = crate::ops::global_daily_v5(&state.db).await?;
    let v5_access = crate::v5_access::evaluate(state, Some(key.str("discord_id"))).await?;
    let (site_v5_left, site_v5_cap) = if v5_access.paused || !v5_access.decision.allowed {
        // 暂停：全站今天没有 V5（不能按「不限」显示）
        (json!(0), Value::Null)
    } else if g > 0 && !key.exclude_global_v5() {
        let used = state.db.day_v5_total(&state.day()).await?;
        (json!((g - used).max(0)), json!(g))
    } else {
        (Value::Null, Value::Null)
    };
    let now = state.now();
    out.insert("has_key".into(), json!(true));
    out.insert("key".into(), key.get("token").clone());
    out.insert("name".into(), key.get("name").clone());
    out.insert("expires_at".into(), key.get("expires_at").clone());
    out.insert("image_scope".into(), key.get("image_model_scope").clone());
    out.insert(
        "subject_roles".into(),
        json!({"owner": v5_access.owner, "admin": v5_access.admin}),
    );
    out.insert(
        "today".into(),
        json!({
            "images": c.images, "v5": c.v5, "legacy": c.legacy_free_images,
            "daily_images": key.get("daily_images"), "daily_v5": key.get("daily_v5"),
            "quota_limits": key.quota_limits(),
            "quota_limits_valid": key.try_quota_limits().is_ok(),
            "site_v5_left": site_v5_left, "site_v5_cap": site_v5_cap,
            "upstream_v5_free": live::upstream_v5_free(state).await && !v5_access.paused,
            "v5_paused": v5_access.paused,
            "v5_access_allowed": v5_access.decision.allowed,
            "v5_access_reason": v5_access.decision.reason,
            "anlas": gate_core::pyc::round0(c.anlas),
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
                snap.value(key_id.map(|id| live::mine(state, id, &snap))),
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

    #[test]
    fn injected_scripts_are_recognised() {
        let host = "gate.example.top";
        assert!(injected_script_error("Script error.", "", host));
        assert!(injected_script_error(
            "x",
            "TypeError: x\n    at hit_test (<anonymous>:1:2)\n    at native_call (<anonymous>:3:4)",
            host
        ));
        assert!(!injected_script_error(
            "x",
            "TypeError: x\n    at f (https://gate.example.top/_owl/assets/app-abc.js:1:2)",
            host
        ));
        assert!(!injected_script_error("x", "no frames here", host));
        // 钱包扩展的报错：没有堆栈，按文字认（线上 10-17 晚就是这条把总览顶出「1 个 Bug 没处理」）
        assert!(injected_script_error(
            "The provider is disconnected from all chains.",
            "",
            host
        ));
        assert!(injected_script_error(
            "Failed to connect to MetaMask",
            "",
            host
        ));
        assert!(!injected_script_error(
            "Cannot read properties of undefined",
            "",
            host
        ));
        assert!(!injected_script_error(
            "x",
            "f@https://other.test/_owl/a.js:1:1",
            host
        ));
    }
}
