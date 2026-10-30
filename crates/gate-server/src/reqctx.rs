//! 每个请求的上下文：请求编号、计时（排队 / 上游耗时）、上游状态码、当前 Key、是否已记日志。
//!
//! 这里用 `Arc<Mutex<..>>` 放进请求扩展，同时用 tokio task-local 让错误响应等「拿不到 Request 的地方」
//! 也能读到请求编号；派生出去的任务只要带上 `ReqCtx` 的克隆，同样能写回。

use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

tokio::task_local! {
    static CURRENT: ReqCtx;
}

#[derive(Clone)]
pub struct ReqCtx(Arc<Mutex<Inner>>, Arc<RequestLifecycle>);

const REQUEST_WAITING: u8 = 0;
const REQUEST_DISPATCHING: u8 = 1;
const REQUEST_DISPATCHED: u8 = 2;
const REQUEST_CANCELLED: u8 = 3;
const REQUEST_FINISHED: u8 = 4;

struct RequestLifecycle {
    stage: AtomicU8,
}

impl std::fmt::Debug for ReqCtx {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("ReqCtx").field(&self.rid()).finish()
    }
}

struct Inner {
    internal_id: String,
    correlation_id: String,
    settlement_reserved: bool,
    settlement_day: Option<String>,
    execution_unknown: bool,
    rid: String,
    t0: Instant,
    sent: Option<Instant>,
    up_status: u16,
    logged: bool,
    /// 本请求的 5xx 是上游造成的（见 `error::upstream_transport`）：错误账本不记 Bug。
    upstream_fault: bool,
    model: String,
    /// 最后选中的上游账号（token_id）；没走到选号为 None。
    token_id: Option<String>,
    key_id: Option<i64>,
    key_name: String,
    /// 来源：直连对端地址、X-Forwarded-For、User-Agent（中间件登记；写用量日志时再按信任列表算出真实 IP）。
    peer: String,
    xff: String,
    ua: String,
    /// 其余客户端特征（App 包名、语言、Sec-CH-UA、请求头指纹、浏览器标记哈希、Origin），保留期同 UA / IP。
    hints: gate_db::repo::usage::ClientHints,
    /// 第一次发往上游时调用一次（实时图把这张图从「排队」移到「生成中」）。
    on_sent: Option<Box<dyn FnOnce() + Send>>,
    /// 上游客户端估计这一张何时开始（墙钟）：排队位次按它排。
    on_queued: Option<Arc<dyn Fn(f64) + Send + Sync>>,
    /// 这个成员今天已经出了几张（出图准入时取）：上游公平排队按它排，少的先。
    share: i64,
    /// 本请求遇到上游「并发锁」的次数（网关已在同一账号上重试；不算 429）。
    locked: u32,
    client_disconnected: bool,
}

/// `snapshot()`：写进用量日志的计时字段。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Timing {
    pub wait_ms: i64,
    pub dur_ms: i64,
    pub up_status: u16,
    pub rid: String,
    pub token_id: Option<String>,
}

impl Default for ReqCtx {
    fn default() -> Self {
        Self::new()
    }
}

impl ReqCtx {
    pub fn new() -> Self {
        Self(
            Arc::new(Mutex::new(Inner {
                internal_id: hex::encode(rand::random::<[u8; 16]>()),
                correlation_id: new_correlation_id(),
                settlement_reserved: false,
                settlement_day: None,
                execution_unknown: false,
                rid: new_rid(),
                t0: Instant::now(),
                sent: None,
                up_status: 0,
                logged: false,
                upstream_fault: false,
                model: String::new(),
                token_id: None,
                key_id: None,
                key_name: String::new(),
                peer: String::new(),
                xff: String::new(),
                ua: String::new(),
                hints: Default::default(),
                on_sent: None,
                on_queued: None,
                share: 0,
                locked: 0,
                client_disconnected: false,
            })),
            Arc::new(RequestLifecycle {
                stage: AtomicU8::new(REQUEST_WAITING),
            }),
        )
    }

    fn with<R>(&self, f: impl FnOnce(&mut Inner) -> R) -> R {
        f(&mut self.0.lock().expect("reqctx"))
    }

    /// Atomically claim the right to persist dispatch. Once claimed, disconnect handling must
    /// let this operation finish: the durable write and upstream POST form one conservative unit.
    pub fn claim_dispatch(&self) -> bool {
        match self.1.stage.compare_exchange(
            REQUEST_WAITING,
            REQUEST_DISPATCHING,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) => true,
            Err(REQUEST_DISPATCHING | REQUEST_DISPATCHED) => true,
            Err(_) => false,
        }
    }

    pub fn dispatch_persisted(&self) {
        self.1.stage.store(REQUEST_DISPATCHED, Ordering::Release);
    }

    /// Returns true only when a caller disconnect wins before dispatch has begun.
    pub fn cancel_before_dispatch(&self) -> bool {
        self.1
            .stage
            .compare_exchange(
                REQUEST_WAITING,
                REQUEST_CANCELLED,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
    }

    pub fn finish_lifecycle(&self) {
        let mut stage = self.1.stage.load(Ordering::Acquire);
        while stage != REQUEST_CANCELLED && stage != REQUEST_FINISHED {
            match self.1.stage.compare_exchange_weak(
                stage,
                REQUEST_FINISHED,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => break,
                Err(current) => stage = current,
            }
        }
    }

    pub fn lifecycle_stage(&self) -> &'static str {
        match self.1.stage.load(Ordering::Acquire) {
            REQUEST_WAITING => "queued",
            REQUEST_DISPATCHING => "dispatching",
            REQUEST_DISPATCHED => "dispatched",
            REQUEST_CANCELLED => "cancelled_before_dispatch",
            _ => "finished",
        }
    }

    pub fn mark_client_disconnected(&self) -> bool {
        self.with(|inner| {
            if inner.client_disconnected {
                false
            } else {
                inner.client_disconnected = true;
                true
            }
        })
    }

    pub fn elapsed_ms(&self) -> u64 {
        self.with(|inner| inner.t0.elapsed().as_millis().min(u128::from(u64::MAX)) as u64)
    }

    pub fn rid(&self) -> String {
        self.with(|i| i.rid.clone())
    }

    /// Cryptographically unique internal ID for durable accounting; unlike the short public rid,
    /// this value is never exposed as the user's request number.
    pub fn internal_id(&self) -> String {
        self.with(|i| i.internal_id.clone())
    }

    /// Short NovelAI support correlation code. This is separate from both the internal ID and rid.
    pub fn correlation_id(&self) -> String {
        self.with(|i| i.correlation_id.clone())
    }

    pub fn mark_settlement_reserved(&self) {
        self.with(|i| i.settlement_reserved = true);
    }
    pub fn has_settlement_reservation(&self) -> bool {
        self.with(|i| i.settlement_reserved)
    }
    pub fn set_settlement_day(&self, day: String) {
        self.with(|i| i.settlement_day = Some(day));
    }
    pub fn settlement_day(&self) -> Option<String> {
        self.with(|i| i.settlement_day.clone())
    }
    pub fn mark_execution_unknown(&self) {
        self.with(|i| i.execution_unknown = true);
    }
    pub fn execution_unknown(&self) -> bool {
        self.with(|i| i.execution_unknown)
    }

    /// 第一次真正发往上游时调用；重试不覆盖。
    pub fn mark_sent(&self) {
        let hook = self.with(|i| {
            if i.sent.is_none() {
                i.sent = Some(Instant::now());
                i.on_sent.take()
            } else {
                None
            }
        });
        if let Some(h) = hook {
            h();
        }
    }

    /// 登记「第一次发往上游」的回调（只保留最后一次登记的）。
    pub fn on_sent(&self, hook: impl FnOnce() + Send + 'static) {
        self.with(|i| i.on_sent = Some(Box::new(hook)));
    }

    /// 登记「排到账号、估计开始时刻变了」的回调（参数是墙钟时刻；只保留最后一次登记的）。
    pub fn on_queued(&self, hook: impl Fn(f64) + Send + Sync + 'static) {
        self.with(|i| i.on_queued = Some(Arc::new(hook)));
    }

    /// 记下这个成员今天已经出了几张（公平排队用）。
    pub fn set_share(&self, images_today: i64) {
        self.with(|i| i.share = images_today.max(0));
    }
    pub fn was_sent(&self) -> bool {
        self.with(|i| i.sent.is_some())
    }

    pub fn mark_status(&self, code: u16) {
        self.with(|i| i.up_status = code);
    }

    pub fn mark_logged(&self) {
        self.with(|i| i.logged = true);
    }

    pub fn logged(&self) -> bool {
        self.with(|i| i.logged)
    }

    /// 标记本请求的失败原因在上游。
    pub fn mark_upstream_fault(&self) {
        self.with(|i| i.upstream_fault = true);
    }

    pub fn upstream_fault(&self) -> bool {
        self.with(|i| i.upstream_fault)
    }

    /// 记下本请求的模型（截到 80 个码点），被拒的请求也能按模型统计。
    pub fn set_model(&self, model: &str) {
        let m = gate_core::primitives::take_chars(model, 80).to_string();
        self.with(|i| i.model = m);
    }

    pub fn model(&self) -> String {
        self.with(|i| i.model.clone())
    }

    pub fn set_key(&self, id: i64, name: &str) {
        let name = name.to_string();
        self.with(|i| {
            i.key_id = Some(id);
            i.key_name = name;
        });
    }

    /// 登记来源（站长 10-18：为防脚本撞限速 / 转售，用量日志记客户端 UA 和来源 IP；保留天数按后台设置，0 = 永久）。
    pub fn set_origin(&self, peer: &str, xff: &str, ua: &str) {
        let (peer, xff, ua) = (peer.to_string(), xff.to_string(), ua.to_string());
        self.with(|i| {
            i.peer = peer;
            i.xff = xff;
            i.ua = ua;
        });
    }

    /// 登记其余客户端特征（站长 10-23：防一人多号 / 共用 Key；见 `crate::client_hints`）。
    pub fn set_hints(&self, hints: gate_db::repo::usage::ClientHints) {
        self.with(|i| i.hints = hints);
    }

    pub fn hints(&self) -> gate_db::repo::usage::ClientHints {
        self.with(|i| i.hints.clone())
    }

    /// `(直连对端, X-Forwarded-For, User-Agent)`
    pub fn origin(&self) -> (String, String, String) {
        self.with(|i| (i.peer.clone(), i.xff.clone(), i.ua.clone()))
    }

    pub fn key(&self) -> Option<(i64, String)> {
        self.with(|i| i.key_id.map(|id| (id, i.key_name.clone())))
    }

    /// wait_ms：收到请求到发往上游；dur_ms：上游耗时（没发到上游为 0）。
    pub fn snapshot(&self) -> Timing {
        self.with(|i| {
            let now = Instant::now();
            let (wait, dur) = match i.sent {
                None => (now - i.t0, std::time::Duration::ZERO),
                Some(s) => (s - i.t0, now - s),
            };
            Timing {
                wait_ms: wait.as_millis() as i64,
                dur_ms: dur.as_millis() as i64,
                up_status: i.up_status,
                rid: i.rid.clone(),
                token_id: i.token_id.clone(),
            }
        })
    }

    /// 在这个上下文里运行一个 future（task-local 生效）。
    pub async fn scope<F: std::future::Future>(self, fut: F) -> F::Output {
        CURRENT.scope(self, fut).await
    }
}

/// 当前请求的上下文（不在请求里时为 None）。
pub fn current() -> Option<ReqCtx> {
    CURRENT.try_with(Clone::clone).ok()
}

/// 当前请求编号（不在请求里时为空串）。
pub fn current_rid() -> String {
    current().map(|c| c.rid()).unwrap_or_default()
}

fn new_correlation_id() -> String {
    const ALPHANUM: &[u8; 62] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    rand::random::<[u8; 6]>()
        .into_iter()
        .map(|b| ALPHANUM[(b as usize) % ALPHANUM.len()] as char)
        .collect()
}

pub fn current_execution_unknown() -> bool {
    current().is_some_and(|c| c.execution_unknown())
}

/// 8 位十六进制（`secrets.token_hex(4)`）。
fn new_rid() -> String {
    let n: u32 = rand::random();
    format!("{n:08x}")
}

/// 上游观察者：把 gate-upstream 的「已发送 / 状态码」写回请求上下文。
impl gate_upstream::client::Observer for ReqCtx {
    fn picked(&self, token_id: &str) {
        self.with(|i| i.token_id = Some(token_id.to_string()));
    }
    fn queued(&self, _token_id: &str, wait: f64) {
        let hook = self.with(|i| i.on_queued.clone());
        if let Some(h) = hook {
            h(crate::time::now() + wait.max(0.0));
        }
    }
    fn sent(&self) {
        self.mark_sent();
    }
    fn status(&self, code: u16) {
        self.mark_status(code);
    }
    fn share(&self) -> i64 {
        self.with(|i| i.share)
    }
    fn locked(&self, token_id: &str) {
        let n = self.with(|i| {
            i.locked += 1;
            i.locked
        });
        tracing::info!(
            target: "upstream",
            rid = %self.rid(),
            token_id,
            code = gate_core::reasons::UPSTREAM_LOCKED,
            count = n,
            "上游并发锁（不算限流），网关在同一账号上稍后重试"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::ReqCtx;

    #[test]
    fn disconnect_wins_before_dispatch_and_prevents_dispatch() {
        let ctx = ReqCtx::new();
        assert!(ctx.cancel_before_dispatch());
        assert_eq!(ctx.lifecycle_stage(), "cancelled_before_dispatch");
        assert!(!ctx.claim_dispatch());
    }

    #[test]
    fn dispatch_claim_wins_and_cannot_be_cancelled() {
        let ctx = ReqCtx::new();
        assert!(ctx.claim_dispatch());
        assert_eq!(ctx.lifecycle_stage(), "dispatching");
        assert!(!ctx.cancel_before_dispatch());
        assert!(
            ctx.claim_dispatch(),
            "dispatch retries after a lock are allowed"
        );
        ctx.dispatch_persisted();
        assert_eq!(ctx.lifecycle_stage(), "dispatched");
    }

    #[test]
    fn correlation_id_is_six_alphanumeric_and_separate_from_request_ids() {
        let ctx = ReqCtx::new();
        let correlation = ctx.correlation_id();
        assert_eq!(correlation.len(), 6);
        assert!(correlation.bytes().all(|b| b.is_ascii_alphanumeric()));
        assert_eq!(ctx.correlation_id(), correlation);
        assert_eq!(ctx.internal_id().len(), 32);
        assert_eq!(ctx.rid().len(), 8);
        assert_ne!(correlation, ctx.rid());
    }
}
