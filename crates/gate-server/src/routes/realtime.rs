//! 公开看板和后台实时事件。每条连接独立持有名额，断开时释放。
use super::registration::no_store;
use crate::error::{ApiResult, HttpError, ok_json};
use crate::live::{self, Snapshot, StreamKind, StreamSlot, wire_text};
use crate::session::{self, ADMIN_COOKIE, AdminSession};
use crate::state::AppState;
use axum::Router;
use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{HeaderMap, HeaderValue, header};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use serde_json::json;
use std::convert::Infallible;
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use tokio::sync::watch;
use tokio::time::{Duration, Instant};

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/public/live", get(public_live))
        .route("/public/live/stream", get(public_live_stream))
        .route("/public/status", get(public_status))
        .route("/queue-status", get(queue_status))
        .route("/admin/api/events", get(admin_events))
}
fn json_text(text: String) -> Response {
    let mut r = Response::new(Body::from(text));
    r.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    r
}
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
/// 后台 SSE 的最短推送间隔与保底心跳。页面把任何一条消息都当作「该刷新了」（admin/Admin.tsx 的 `onmessage = bump`），
/// 所以：真有变化才推，且至少隔 15 秒——后台每次刷新会重拉成员/Key/角标（几百 KB），
/// 2 秒一次在队列忙时等于持续打满 DB；15 秒对单人后台足够新。
/// 没变化也每 30 秒推一条心跳（日志 / Bug / 成员列表这些不在快照里的数据靠它更新）。
const ADMIN_MIN_GAP: Duration = Duration::from_secs(15);
const ADMIN_HEARTBEAT: Duration = Duration::from_secs(30);

struct AdminConn {
    state: AppState,
    rx: watch::Receiver<Option<Arc<Snapshot>>>,
    headers: HeaderMap,
    last_sig: Option<u64>,
    last_sent: Option<Instant>,
    _slot: StreamSlot,
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
        let sig = snap.admin_sig;
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
        let text = format!("{{\"live\":{},\"badges\":{{}}}}", snap.json());
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
        last_sig: None,
        last_sent: None,
        _slot: slot,
    };
    sse_response(futures::stream::unfold(conn, next_admin), keepalive)
}
