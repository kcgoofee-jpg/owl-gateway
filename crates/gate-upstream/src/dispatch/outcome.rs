//! 结算守卫（[`InFlight`]）与上游状态码的处理：并发锁、429、403、401、402、其它 5xx。

use super::*;

/// 请求进行中的结算守卫：正常路径调用 `finish().await`；被取消（Drop）时在后台补做同样的结算。
pub(crate) struct InFlight<H: Hooks> {
    pub(crate) inner: Arc<Inner<H>>,
    pub(crate) token: Arc<Token>,
    pub(crate) succeeded: bool,
    pub(crate) v5_free: bool,
    pub(crate) image_count: usize,
    pub(crate) send_started: bool,
    /// 单调时钟：发往上游的时刻（算实测出图耗时）。
    sent_at: Option<f64>,
    pub(crate) permit: Option<Permit>,
    /// 占到的出图间隔：没发出去就退还（见 `Scheduler::refund`）。
    pub(crate) turn: Option<Turn>,
    /// 排队票：轮到开始时清掉（Drop 时从账号的等待队列里移除）。
    pub(crate) ticket: Option<Ticket>,
    done: bool,
}

pub(crate) struct Settle {
    token: Arc<Token>,
    succeeded: bool,
    v5_free: bool,
    image_count: usize,
    send_started: bool,
    sent_at: Option<f64>,
    turn: Option<Turn>,
}

impl<H: Hooks> InFlight<H> {
    pub(crate) fn new(
        inner: Arc<Inner<H>>,
        token: Arc<Token>,
        v5_free: bool,
        image_count: usize,
    ) -> Self {
        Self {
            inner,
            token,
            succeeded: false,
            v5_free,
            image_count,
            send_started: false,
            sent_at: None,
            permit: None,
            turn: None,
            ticket: None,
            done: false,
        }
    }

    /// 已决定发往上游。
    pub(crate) fn mark_sent(&mut self) {
        self.send_started = true;
        self.sent_at = Some(self.inner.scheduler.mono());
    }

    fn take_settle(&mut self) -> Settle {
        self.ticket = None;
        Settle {
            token: self.token.clone(),
            succeeded: self.succeeded,
            v5_free: self.v5_free,
            image_count: self.image_count,
            send_started: self.send_started,
            sent_at: self.sent_at,
            turn: self.turn.take(),
        }
    }

    pub(crate) async fn finish(mut self) {
        self.done = true;
        let permit = self.permit.take();
        let s = self.take_settle();
        settle(&self.inner, s).await;
        drop(permit); // 先结算（含出图后静默）再放槽：下一位一定能看到新的 image_next_at
    }
}

impl<H: Hooks> Drop for InFlight<H> {
    fn drop(&mut self) {
        if self.done {
            return;
        }
        let inner = self.inner.clone();
        let permit = self.permit.take();
        let s = self.take_settle();
        if let Ok(rt) = tokio::runtime::Handle::try_current() {
            rt.spawn(async move {
                settle(&inner, s).await;
                drop(permit);
            });
        }
    }
}

pub(crate) async fn settle<H: Hooks>(inner: &Inner<H>, s: Settle) {
    let token = &s.token;
    inner
        .pool
        .finish_v5_reservation(&inner.hooks, token, s.succeeded, s.v5_free)
        .await;
    if s.succeeded {
        record_images(&inner.hooks, token, s.image_count).await;
    }
    if s.send_started {
        // 出图后静默只对真正发往上游的请求生效；成功的图记一次实测耗时（估算排队用）
        if s.succeeded
            && let Some(at) = s.sent_at
        {
            let took = inner.scheduler.mono() - at;
            token.with(|f| f.gen_secs = crate::pace::ewma(f.gen_secs, took));
        }
        inner.scheduler.after_request(token, &inner.hooks);
    } else if let Some(turn) = s.turn {
        // 占了间隔却没发出去（排完队被拒 / 换号 / 断开）：间隔还回去，不白白浪费一个周期
        inner.scheduler.refund(token, turn);
    }
}

/// 按上游实际成功的张数记账（失败、拒绝、限流不计）。
pub(crate) async fn record_images(hooks: &dyn Hooks, token: &Token, image_count: usize) {
    if image_count > 0 {
        let day = hooks.day();
        if let Err(e) = hooks
            .bump_upstream_images(&token.token_id, &day, image_count as i64)
            .await
        {
            tracing::error!(error = %e, "记录上游出图计数失败");
        }
    }
}

/// Retry-After 解析；超长数字 / 越界日期按默认 20 秒（见 `parse_retry_after`）。
pub(crate) fn retry_after_or_default(headers: &HeaderMap, now: f64) -> f64 {
    let header = headers.get("retry-after").and_then(|v| v.to_str().ok());
    gate_core::upstream_policy::parse_retry_after(header, now)
}

/// 等出图槽位的结果。
pub(crate) enum Wait {
    /// 轮到了：占到并发槽和出图间隔。
    Ready,
    TimedOut,
    /// 换到这个账号（已在它上面登记新票、预留好 V5）。
    Move(Arc<Token>, Ticket),
}

/// 排队期间多久看一次「能不能换号 / 还要等多久」。
pub(crate) const POLL: Duration = Duration::from_millis(500);
/// 一张图排队期间最多换几次号（防止来回挪）。
pub(crate) const MAX_MOVES: usize = 3;

pub(crate) enum Attempt {
    Done(Result<UpResponse, UpstreamError>),
    Retry,
    /// 这把 Token 的 V5 免费额度见底，换一把还能免费出（不占重试次数）。
    SwitchV5,
}

const QUEUE_TIMEOUT: &str = "NovelAI 出图排队超时，未扣费。请稍后重试。";
/// 排队等 NovelAI 超时：站点忙，不是成员的问题。带上原因码，用量日志和单 Key 限流统计才认得出来。
pub(crate) fn queue_timed_out() -> UpstreamError {
    UpstreamError {
        code: gate_core::reasons::QUEUE_TIMEOUT.into(),
        ..UpstreamError::new(429, QUEUE_TIMEOUT)
    }
}
/// 排队期间可能触发了熔断或用满了当日上限：选号时的检查已经过时，发出前再看一次。
pub(crate) async fn dispatch_block(hooks: &dyn Hooks, token: &Token) -> Result<(), UpstreamError> {
    use gate_core::reasons::{BREAKER, DAILY_CAP};
    if !hooks.has_guard() {
        return Ok(());
    }
    let day = hooks.day();
    match hooks
        .token_block_reason(&token.token_id, &day)
        .await
        .map_err(UpstreamError::internal)?
    {
        // Daily cap won't clear before the reset: a local 403 so clients don't auto-retry it
        Some((text, code)) if code == DAILY_CAP => Err(UpstreamError::local(403, text, code)),
        Some((text, code)) if code == BREAKER => Err(UpstreamError {
            code,
            ..UpstreamError::new(429, text)
        }),
        _ => Ok(()),
    }
}
const LOCKED: &str = "NovelAI 账号上一张图仍未释放，多次重试未成功，未扣费。请稍后重试。";
/// 并发锁重试用完 / 排队超时前等不到：不是限流，也不是成员的问题。
pub(crate) fn locked_out() -> UpstreamError {
    UpstreamError {
        code: gate_core::reasons::UPSTREAM_LOCKED.into(),
        ..UpstreamError::new(503, LOCKED)
    }
}

/// 上游回了「并发锁」（这个账号的上一张还没释放）：记一次（后台按账号数，不算 429），
/// 还能重试就把这个账号的下一张定在退避秒数后、占着并发槽等到点并占好新的出图间隔，返回 true（调用方在同一账号上再发）；
/// 次数用完或等不到排队超时之前返回 false。`retry` 是已经重试过几次。
pub(crate) async fn retry_after_lock<H: Hooks>(
    inner: &Inner<H>,
    fl: &mut InFlight<H>,
    obs: &dyn Observer,
    retry: u32,
    deadline: tokio::time::Instant,
) -> bool {
    let token = fl.token.clone();
    let hooks: &dyn Hooks = &inner.hooks;
    token.note_locked(hooks);
    obs.locked(&token.token_id);
    let left = deadline
        .saturating_duration_since(tokio::time::Instant::now())
        .as_secs_f64();
    let Some(wait) = pace::lock_wait(retry).filter(|w| *w < left) else {
        tracing::warn!(
            target: "upstream",
            token_id = %token.token_id,
            code = gate_core::reasons::UPSTREAM_LOCKED,
            retries = retry,
            "上游并发锁：重试次数用完或等不到排队超时，这张不再试（不算限流）"
        );
        return false;
    };
    tracing::info!(
        target: "upstream",
        token_id = %token.token_id,
        code = gate_core::reasons::UPSTREAM_LOCKED,
        retry = retry + 1,
        "上游并发锁（上一张还没释放，不算限流）：{wait:.0} 秒后在同一账号重试"
    );
    inner.scheduler.hold_for_lock(&token, fl.turn, wait);
    let InFlight { permit, turn, .. } = fl;
    tokio::time::timeout_at(
        deadline,
        inner.scheduler.acquire(&token, permit, turn, None),
    )
    .await
    .is_ok()
}

const RATE_LIMITED_IMAGE: &str = "NovelAI 正在限流，全站出图暂停冷却，未扣费。请稍后重试。";
const MODEL_403: &str =
    "NovelAI 不支持该模型，未扣费。请改用本站支持的模型，如 nai-diffusion-4-5-full。";
const ACCOUNT_403: &str = "NovelAI 拒绝了本次请求，本站已暂停使用该账号，未扣费。请稍后重试。";
pub(crate) const ADJUSTING: &str = "本站正在调整 NovelAI 账号，请求未发出，未扣费。请稍后重试。";
const TOKEN_401: &str = "本站 NovelAI 账号登录失效，未扣费。请稍后重试。";

/// 上游 429：按 Retry-After 冷却这把 Token（出图间隔不变）并回调调用方，返回给成员的错误。
pub(crate) async fn on_429(
    sched: &crate::scheduler::Scheduler,
    hooks: &dyn Hooks,
    token: &Token,
    cb: &Callbacks<'_>,
    headers: &HeaderMap,
) -> UpstreamError {
    let retry_after = retry_after_or_default(headers, hooks.now());
    sched.on_429(token, hooks);
    token.mark_rate_limited(hooks, retry_after);
    if let Some(cb) = cb.on_rate_limited.as_ref() {
        cb(retry_after).await;
    }
    UpstreamError {
        retry_after: Some(retry_after.max(1.0).ceil() as u64),
        ..UpstreamError::new(429, RATE_LIMITED_IMAGE)
    }
}

/// 上游 403（`peek` 是正文前 300 字节）：「模型不存在」也是 403，不算账号被拒；其余记一次账号级 403。
pub(crate) fn on_403(hooks: &dyn Hooks, token: &Token, peek: &str) -> UpstreamError {
    if gate_core::upstream_policy::is_model_level_403(peek) {
        return UpstreamError::new(400, MODEL_403);
    }
    token.mark_forbidden(hooks);
    UpstreamError::new(403, ACCOUNT_403)
}

/// 出图时上游回 402（需要付费 / Anlas 不足）：这把 Token 一段时间不接要花 Anlas 的图；
/// 这次本来是免费 V5 的话，它的免费 V5 也先让给别的 Token。返回 true = 换一把重试
/// （只在免费 V5 还有别的 Token 能出、且这次请求还能换号时）。给成员的 402 文案不变。
pub(crate) async fn on_402(
    pool: &crate::pool::Pool,
    hooks: &dyn Hooks,
    token: &Token,
    v5_attempt: bool,
    may_switch: bool,
) -> bool {
    token.mark_anlas_out(hooks);
    if !v5_attempt {
        return false;
    }
    pool.mark_v5_out(hooks, token);
    let switch = may_switch && pool.other_v5_open(hooks, token).await;
    if switch {
        // 不换号时由 on_other_status 照常告警
        pool::warn_account(hooks, 402);
    }
    switch
}

/// 其余状态码：账号告警；401 停用这把 Token 并报警。
pub(crate) async fn on_other_status(
    hooks: &dyn Hooks,
    token: &Token,
    status: u16,
) -> Option<UpstreamError> {
    pool::warn_account(hooks, status);
    if status == 401 {
        pool::isolate_unauthorized(hooks, token).await;
        return Some(UpstreamError::new(502, TOKEN_401));
    }
    None
}
