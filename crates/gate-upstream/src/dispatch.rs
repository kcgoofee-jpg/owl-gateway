//! 一次上游请求的完整流程：选号 → 等出图槽位（期间可换到更早空出来的账号）→ V5 计费确认 → 发出
//! → 按状态码处理（并发锁 / 429 / 403 / 401）→ 结算。
//!
//! 上游「并发锁」（429 / 409 + `Concurrent generation is locked`，见 [`gate_core::upstream_policy::is_concurrency_lock`]）
//! 不是限流：不冷却、不回调 `on_rate_limited`、不报 429 状态码；占着这个账号的并发槽等
//! [`pace::lock_wait`] 秒在同一账号上再发（最多 [`pace::LOCK_RETRIES`] 次，不超过排队超时），
//! 用完才以 `upstream_locked` 失败（[`retry_after_lock`]）。
//!
//! 取消安全用 Drop 守卫实现：请求 future 在任何点被丢弃，V5 预留、计数、出图后静默、排队票、
//! 没用上的出图间隔与并发槽都会照常结算（旧版用 `asyncio.shield` + `CancelScope(shield=True)` 达到同样效果）。

use crate::client::{Callbacks, Inner, NaiClient, RequestOpts, ResolveV5, UpResponse};
use crate::error::{Transport, UpstreamError};
use crate::hooks::{Allowance, Hooks, Observer};
use crate::http;
use crate::limiter::Permit;
use crate::pace::{self, Arrival};
use crate::pool::{self, MoveWatch, Ticket, Token};
use crate::scheduler::Turn;
use bytes::Bytes;
use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;
use wreq::header::HeaderMap;

/// 请求进行中的结算守卫：正常路径调用 `finish().await`；被取消（Drop）时在后台补做同样的结算。
pub(crate) struct InFlight<H: Hooks> {
    inner: Arc<Inner<H>>,
    token: Arc<Token>,
    succeeded: bool,
    v5_free: bool,
    image_count: usize,
    send_started: bool,
    /// 单调时钟：发往上游的时刻（算实测出图耗时）。
    sent_at: Option<f64>,
    permit: Option<Permit>,
    /// 占到的出图间隔：没发出去就退还（见 `Scheduler::refund`）。
    turn: Option<Turn>,
    /// 排队票：轮到开始时清掉（Drop 时从账号的等待队列里移除）。
    ticket: Option<Ticket>,
    done: bool,
}

/// 结算要用的字段（从 [`InFlight`] 里取出，Drop 时在后台任务里用）。
struct Settle {
    token: Arc<Token>,
    succeeded: bool,
    v5_free: bool,
    image_count: usize,
    send_started: bool,
    sent_at: Option<f64>,
    turn: Option<Turn>,
}

impl<H: Hooks> InFlight<H> {
    fn new(inner: Arc<Inner<H>>, token: Arc<Token>, v5_free: bool, image_count: usize) -> Self {
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
    fn mark_sent(&mut self) {
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

    async fn finish(mut self) {
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

async fn settle<H: Hooks>(inner: &Inner<H>, s: Settle) {
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
fn retry_after_or_default(headers: &HeaderMap, now: f64) -> f64 {
    let header = headers.get("retry-after").and_then(|v| v.to_str().ok());
    gate_core::upstream_policy::parse_retry_after(header, now)
}

/// 等出图槽位的结果。
enum Wait {
    /// 轮到了：占到并发槽和出图间隔。
    Ready,
    TimedOut,
    /// 换到这个账号（已在它上面登记新票、预留好 V5）。
    Move(Arc<Token>, Ticket),
}

/// 排队期间多久看一次「能不能换号 / 还要等多久」。
const POLL: Duration = Duration::from_millis(500);
/// 一张图排队期间最多换几次号（防止来回挪）。
const MAX_MOVES: usize = 3;

enum Attempt {
    Done(Result<UpResponse, UpstreamError>),
    Retry,
    /// 这把 Token 的 V5 免费额度见底，换一把还能免费出（不占重试次数）。
    SwitchV5,
}

const QUEUE_TIMEOUT: &str = "NovelAI 出图排队超时，未扣费。请稍后重试。";
/// 排队等 NovelAI 超时：站点忙，不是成员的问题。带上原因码，用量日志和单 Key 限流统计才认得出来。
fn queue_timed_out() -> UpstreamError {
    UpstreamError {
        code: gate_core::reasons::QUEUE_TIMEOUT.into(),
        ..UpstreamError::new(429, QUEUE_TIMEOUT)
    }
}
/// 排队期间可能触发了熔断或用满了当日上限：选号时的检查已经过时，发出前再看一次。
async fn dispatch_block(hooks: &dyn Hooks, token: &Token) -> Result<(), UpstreamError> {
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
fn locked_out() -> UpstreamError {
    UpstreamError {
        code: gate_core::reasons::UPSTREAM_LOCKED.into(),
        ..UpstreamError::new(503, LOCKED)
    }
}

/// 上游回了「并发锁」（这个账号的上一张还没释放）：记一次（后台按账号数，不算 429），
/// 还能重试就把这个账号的下一张定在退避秒数后、占着并发槽等到点并占好新的出图间隔，返回 true（调用方在同一账号上再发）；
/// 次数用完或等不到排队超时之前返回 false。`retry` 是已经重试过几次。
async fn retry_after_lock<H: Hooks>(
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
const ADJUSTING: &str = "本站正在调整 NovelAI 账号，请求未发出，未扣费。请稍后重试。";
const TOKEN_401: &str = "本站 NovelAI 账号登录失效，未扣费。请稍后重试。";

/// 上游 429：按 Retry-After 冷却这把 Token（出图间隔不变）并回调调用方，返回给成员的错误。
async fn on_429(
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
    UpstreamError::new(429, RATE_LIMITED_IMAGE)
}

/// 上游 403（`peek` 是正文前 300 字节）：「模型不存在」也是 403，不算账号被拒；其余记一次账号级 403。
fn on_403(hooks: &dyn Hooks, token: &Token, peek: &str) -> UpstreamError {
    if gate_core::upstream_policy::is_model_level_403(peek) {
        return UpstreamError::new(400, MODEL_403);
    }
    token.mark_forbidden(hooks);
    UpstreamError::new(403, ACCOUNT_403)
}

/// 出图时上游回 402（需要付费 / Anlas 不足）：这把 Token 一段时间不接要花 Anlas 的图；
/// 这次本来是免费 V5 的话，它的免费 V5 也先让给别的 Token。返回 true = 换一把重试
/// （只在免费 V5 还有别的 Token 能出、且这次请求还能换号时）。给成员的 402 文案不变。
async fn on_402(
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
async fn on_other_status(hooks: &dyn Hooks, token: &Token, status: u16) -> Option<UpstreamError> {
    pool::warn_account(hooks, status);
    if status == 401 {
        pool::isolate_unauthorized(hooks, token).await;
        return Some(UpstreamError::new(502, TOKEN_401));
    }
    None
}

impl<H: Hooks> NaiClient<H> {
    /// 普通请求（图片工具 / 补全 / 非流式出图）。出图请求不自动重试；普通请求只对明确的 429 换 Token 一次。
    pub async fn request(
        &self,
        mut o: RequestOpts<'_>,
        cb: &Callbacks<'_>,
        allowance: Option<&dyn Allowance>,
        obs: &dyn Observer,
    ) -> Result<UpResponse, UpstreamError> {
        let inner = &self.inner;
        // 换号重试要按最初的费用策略重新选号（V5 确认成「按 Anlas 计」会改这两项）
        let (v5_free0, requires_anlas0) = (o.v5_free, o.requires_anlas);
        // 公平排队的信息：成员今天的用量 + 开始排队的时刻（重试 / 换号都不变）
        let arrival = Arrival::new(obs.share(), inner.scheduler.mono(), o.queue_timeout);
        let mut attempts = 0;
        let mut switched = false;
        while attempts < 2 + usize::from(switched) {
            attempts += 1;
            let Some((token, ticket)) = inner
                .pool
                .pick(
                    &inner.hooks,
                    &inner.scheduler,
                    o.requires_anlas,
                    o.v5_free,
                    // 所有图片车道的请求都要过账号保护（含 tags）：它们都会打到同一个上游账号，
                    // 也都要排队等出图间隔（见 request_once）。
                    o.image_lane,
                    allowance,
                    arrival,
                )
                .await?
            else {
                return Err(self.unavailable(o.requires_anlas, o.v5_free).await);
            };
            obs.picked(&token.token_id);
            let mut fl = InFlight::new(inner.clone(), token.clone(), o.v5_free, o.image_count);
            fl.ticket = ticket;
            let outcome = self
                .request_once(&mut o, cb, allowance, obs, &mut fl, !switched)
                .await;
            fl.finish().await;
            match outcome {
                Attempt::Done(r) => return r,
                Attempt::Retry => continue,
                Attempt::SwitchV5 => {
                    switched = true;
                    o.v5_free = v5_free0;
                    o.requires_anlas = requires_anlas0;
                }
            }
        }
        Err(UpstreamError::new(
            429,
            "NovelAI 正在限流，未扣费。请稍后重试，不要连续重发。",
        ))
    }

    /// 一次尝试。`fl` 是选中的账号的结算守卫；排队期间换号时会被换成新账号的（旧的就地结算）。
    async fn request_once(
        &self,
        o: &mut RequestOpts<'_>,
        cb: &Callbacks<'_>,
        allowance: Option<&dyn Allowance>,
        obs: &dyn Observer,
        fl: &mut InFlight<H>,
        may_switch: bool,
    ) -> Attempt {
        let inner = &self.inner;
        let hooks: &dyn Hooks = &inner.hooks;
        let mut response_status: Option<u16>;
        let mut token = fl.token.clone();
        // 这次是不是按免费 V5 选的号（确认成「按 Anlas 计」之后 o.v5_free 会变）
        let v5_attempt = o.v5_free;
        let deadline =
            tokio::time::Instant::now() + Duration::from_secs_f64(o.queue_timeout.max(0.0));
        // V5 计费确认在排队之前做一次（见底 / 余额不够就当场换号或拒绝，不让成员白等），
        // 排完队若仍是免费再确认一次（排队期间额度可能被别人用完）
        if let Some(a) = self
            .confirm_v5(o, cb, allowance, &token, fl, may_switch)
            .await
        {
            return a;
        }
        if o.image_lane {
            let patient = tokio::time::Instant::now()
                + Duration::from_secs_f64(o.queue_timeout.max(0.0) / 2.0);
            let mut moves = 0;
            loop {
                if let Some(t) = fl.ticket.as_ref() {
                    t.set_flags(o.requires_anlas, o.v5_free);
                }
                let wait = self
                    .wait_slot(
                        fl,
                        deadline,
                        patient,
                        moves < MAX_MOVES,
                        o.requires_anlas,
                        o.v5_free,
                        allowance,
                        obs,
                    )
                    .await;
                match wait {
                    Ok(Wait::Ready) => break,
                    Ok(Wait::TimedOut) => return Attempt::Done(Err(queue_timed_out())),
                    Err(e) => return Attempt::Done(Err(e)),
                    Ok(Wait::Move(to, ticket)) => {
                        moves += 1;
                        self.switch_flight(fl, &to, ticket, o.v5_free, obs).await;
                        token = to;
                        // 新账号的 V5 额度要重新确认（见底就按 Anlas 计 / 换号 / 拒绝，与排队前一样）
                        if let Some(a) = self
                            .confirm_v5(o, cb, allowance, &token, fl, may_switch)
                            .await
                        {
                            return a;
                        }
                    }
                }
            }
            if let Some(a) = self
                .confirm_v5(o, cb, allowance, &token, fl, may_switch)
                .await
            {
                return a;
            }
        }
        let token = &token;
        // 上游回「并发锁」时在同一账号上再发（见 retry_after_lock），其余响应往下处理
        let mut locks: u32 = 0;
        let resp = loop {
            response_status = None;
            // 与 set_admin_enabled 串行：之后发出的请求一定能看到新的启停状态（请求本身不持锁）
            drop(token.dispatch_lock.lock().await);
            if !token.with(|f| f.admin_enabled) {
                return Attempt::Retry;
            }
            if o.image_lane
                && o.wait_for_image_slot
                && let Err(e) = dispatch_block(hooks, token).await
            {
                return Attempt::Done(Err(e));
            }
            // The account can be disabled/cooldown while this request waits on the upstream slot.
            // Retry is safe here: the POST has not started and no dispatch callback has been recorded.
            if !token.usable(hooks.now()) {
                return Attempt::Retry;
            }
            if let Some(check) = cb.before_dispatch.as_ref()
                && let Err(e) = check().await
            {
                return Attempt::Done(Err(e));
            }
            if let Some(on) = cb.on_dispatch.as_ref()
                && let Err(e) = on().await
            {
                return Attempt::Done(Err(e));
            }
            fl.mark_sent();
            obs.sent();
            let mut req = inner.http.build(
                token.profile_index(),
                &token.token,
                o.method.clone(),
                &o.url,
                o.json,
                o.accept,
            );
            if o.image_lane {
                // 生图 / 图片工具是非幂等 POST：上游 3xx 一律不跟随。307/308 会把请求体重发到新地址
                // （等于一次成员请求变成多次上游请求，且可能重复计费、绕过本地的节奏与计数）。
                req = req.redirect(wreq::redirect::Policy::none());
            }
            let sent = async {
                let resp = req.send().await?;
                let status = resp.status().as_u16();
                response_status = Some(status);
                let headers = resp.headers().clone();
                match o.max_response_bytes {
                    None => Ok::<_, wreq::Error>(Ok(UpResponse {
                        status,
                        headers,
                        body: resp.bytes().await?,
                    })),
                    Some(cap) => match http::read_capped(resp, cap).await? {
                        None => {
                            let uncertain =
                                o.image_lane && (status == 200 || status == 201 || status >= 500);
                            Ok(Err(UpstreamError::uncertain(
                                502,
                                "NovelAI 返回的结果过大，未扣费。请缩小图片后重试。",
                                uncertain,
                            )))
                        }
                        Some(body) => {
                            let mut headers = headers;
                            headers.remove(wreq::header::CONTENT_ENCODING);
                            headers.remove(wreq::header::CONTENT_LENGTH);
                            Ok(Ok(UpResponse {
                                status,
                                headers,
                                body,
                            }))
                        }
                    },
                }
            };
            let result = if o.max_response_bytes.is_some() {
                match tokio::time::timeout(Duration::from_secs(300), sent).await {
                    Ok(r) => r.map_err(Transport::Http),
                    Err(_) => Err(Transport::Timeout),
                }
            } else {
                sent.await.map_err(Transport::Http)
            };
            let resp = match result {
                Ok(Ok(r)) => r,
                Ok(Err(e)) => return Attempt::Done(Err(e)),
                Err(t) => {
                    if !o.image_lane {
                        let mut e = UpstreamError::new(502, format!("上游请求失败：{t}"));
                        e.transport = true;
                        return Attempt::Done(Err(e));
                    }
                    // 已发送请求的读写故障可能产生扣款；明确的 4xx、连接没建立起来的除外
                    let uncertain = fl.send_started
                        && response_status.is_none_or(|s| s == 200 || s == 201 || s >= 500)
                        && t.after_send();
                    return Attempt::Done(Err(UpstreamError::uncertain(
                        502,
                        "连接 NovelAI 时中断或超时，未扣费。请稍后手动重试。给客户端作者：请勿自动重试",
                        uncertain,
                    )));
                }
            };
            if o.image_lane
                && gate_core::upstream_policy::is_concurrency_lock(
                    resp.status,
                    &String::from_utf8_lossy(&resp.body[..resp.body.len().min(300)]),
                )
            {
                if retry_after_lock(inner, fl, obs, locks, deadline).await {
                    locks += 1;
                    continue;
                }
                return Attempt::Done(Err(locked_out()));
            }
            break resp;
        };
        obs.status(resp.status);
        if o.image_lane && (300..400).contains(&resp.status) {
            // 图片车道已禁止自动跟随重定向（见上面的 redirect::Policy::none）；
            // 明确报错而不是把 3xx 当成正常响应交给调用方。
            return Attempt::Done(Err(UpstreamError::new(
                502,
                "NovelAI 返回异常跳转，请求未重发，未扣费。请稍后重试。",
            )));
        }
        if (resp.status == 200 || resp.status == 201)
            && o.image_count > 0
            && !hooks
                .validate_image_result(resp.body.clone(), o.image_count)
                .await
        {
            return Attempt::Done(Err(UpstreamError::uncertain(
                502,
                "NovelAI 返回的图片不完整或无效，未扣费。请稍后重试。",
                true,
            )));
        }
        let succeeded = resp.status == 200 || resp.status == 201;
        fl.succeeded = succeeded;
        if resp.status == 429 {
            let e = on_429(&inner.scheduler, hooks, token, cb, &resp.headers).await;
            if o.image_lane {
                return Attempt::Done(Err(e));
            }
            return Attempt::Retry;
        }
        if resp.status == 403 && o.image_lane {
            let peek = String::from_utf8_lossy(&resp.body[..resp.body.len().min(300)]);
            return Attempt::Done(Err(on_403(hooks, token, &peek)));
        }
        if resp.status == 402
            && o.image_lane
            && on_402(&inner.pool, hooks, token, v5_attempt, may_switch).await
        {
            // 402 没出图、没扣费，换号重试是安全的；按最初的费用策略重新选号
            return Attempt::SwitchV5;
        }
        if let Some(e) = on_other_status(hooks, token, resp.status).await {
            return Attempt::Done(Err(e));
        }
        if succeeded && o.wait_for_image_slot {
            // 只有真正生图 / 图片工具的成功（占用出图间隔的那一类）才算账号状态恢复，
            // 才清「连续 403」；标签补全等旁路请求 200 不能把 403 连击打断。
            token.mark_ok(hooks);
        }
        Attempt::Done(Ok(resp))
    }

    /// 等这张图在 `fl` 的账号上轮到（公平排队里排第一）：占并发槽、等出图间隔到点（[`Scheduler::acquire`](crate::scheduler::Scheduler::acquire)）。
    /// 等的同时每 [`POLL`] 看一次：报「还要等多久」给观察者；`can_move` 时问令牌池能不能换到更早开始的账号
    /// （[`Pool::try_move`](crate::pool::Pool::try_move)，过了 `patient` 以后更积极）。到 `deadline` 还没轮到算超时。
    #[allow(clippy::too_many_arguments)] // 费用要求与超时都只在这一层用
    async fn wait_slot(
        &self,
        fl: &mut InFlight<H>,
        deadline: tokio::time::Instant,
        patient: tokio::time::Instant,
        can_move: bool,
        requires_anlas: bool,
        v5_free: bool,
        allowance: Option<&dyn Allowance>,
        obs: &dyn Observer,
    ) -> Result<Wait, UpstreamError> {
        let inner = &self.inner;
        let hooks: &dyn Hooks = &inner.hooks;
        let InFlight {
            token,
            permit,
            turn,
            ticket,
            ..
        } = fl;
        // 轮到谁按这个账号上的公平排队（见 Scheduler::acquire）
        let ticket_id = ticket.as_ref().map(Ticket::id);
        let acquire = inner.scheduler.acquire(token, permit, turn, ticket_id);
        tokio::pin!(acquire);
        let mut tick = tokio::time::interval(POLL);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut watch = MoveWatch::default();
        let mut reported = f64::NAN;
        loop {
            tokio::select! {
                biased;
                () = &mut acquire => {
                    *ticket = None; // 轮到了：不再算在这个账号的等待队列里
                    return Ok(Wait::Ready);
                }
                () = tokio::time::sleep_until(deadline) => return Ok(Wait::TimedOut),
                _ = tick.tick() => {
                    let Some(t) = ticket.as_ref() else { continue };
                    let eta = inner.pool.eta(&inner.scheduler, hooks, t);
                    if reported.is_nan() || (eta - reported).abs() > 1.0 {
                        reported = eta;
                        obs.queued(&t.token().token_id, eta);
                    }
                    if !can_move {
                        continue;
                    }
                    let patient_over = tokio::time::Instant::now() >= patient;
                    if let Some((to, new)) = inner
                        .pool
                        .try_move(
                            hooks,
                            &inner.scheduler,
                            t,
                            requires_anlas,
                            v5_free,
                            allowance,
                            patient_over,
                            &mut watch,
                        )
                        .await?
                    {
                        return Ok(Wait::Move(to, new));
                    }
                }
            }
        }
    }

    /// 排队中换号：先结算旧账号（释放它的 V5 预留、并发槽和排队票），再换成新账号的结算守卫。
    async fn switch_flight(
        &self,
        fl: &mut InFlight<H>,
        to: &Arc<Token>,
        ticket: Ticket,
        v5_free: bool,
        obs: &dyn Observer,
    ) {
        tracing::info!(
            target: "upstream",
            from = %fl.token.token_id,
            to = %to.token_id,
            "排队中换到更早能开始的账号"
        );
        let mut new = InFlight::new(self.inner.clone(), to.clone(), v5_free, fl.image_count);
        new.ticket = Some(ticket);
        let old = std::mem::replace(fl, new);
        old.finish().await;
        obs.picked(&to.token_id);
    }

    /// V5 计费确认（[`resolve_v5_cost`]）。只在这次仍按免费 V5 走时才查；返回 Some 表示这次尝试到此为止。
    async fn confirm_v5(
        &self,
        o: &mut RequestOpts<'_>,
        cb: &Callbacks<'_>,
        allowance: Option<&dyn Allowance>,
        token: &Arc<Token>,
        fl: &mut InFlight<H>,
        may_switch: bool,
    ) -> Option<Attempt> {
        let inner = &self.inner;
        if !o.v5_free {
            return None;
        }
        let (Some(resolve), Some(a)) = (cb.resolve_v5_cost.as_ref(), allowance) else {
            return None;
        };
        match resolve_v5_cost(inner, token, may_switch, a, resolve).await {
            Ok(V5Cost::Paid) => {
                inner
                    .pool
                    .finish_v5_reservation(&inner.hooks, token, false, true)
                    .await;
                o.v5_free = false;
                fl.v5_free = false;
                o.requires_anlas = true;
                None
            }
            Ok(V5Cost::Free) => None,
            Ok(V5Cost::Switch) => Some(Attempt::SwitchV5),
            Err(e) => Some(Attempt::Done(Err(e))),
        }
    }

    /// 打开图片流（不自动重试；免费 V5 见底 / 上游 402 时可换一次号）。返回的 [`ImageStream`]
    /// 读完后调用 `finish(completed_images)`；中途丢弃也会按已确认的张数结算。
    #[allow(clippy::too_many_arguments)] // 与旧版 image_stream 的参数一一对应
    pub async fn image_stream(
        &self,
        url: &str,
        body: &Value,
        _image_count: i64,
        requires_anlas: bool,
        v5_free: bool,
        queue_timeout: f64,
        cb: &Callbacks<'_>,
        allowance: Option<&dyn Allowance>,
        obs: &dyn Observer,
    ) -> Result<ImageStream<H>, UpstreamError> {
        let inner = &self.inner;
        let hooks: &dyn Hooks = &inner.hooks;
        let mut may_switch = true;
        let started = tokio::time::Instant::now();
        let deadline = started + Duration::from_secs_f64(queue_timeout.max(0.0));
        let patient = started + Duration::from_secs_f64(queue_timeout.max(0.0) / 2.0);
        // 公平排队的信息：成员今天的用量 + 开始排队的时刻（换号 / 402 换一把都不变）
        let arrival = Arrival::new(obs.share(), inner.scheduler.mono(), queue_timeout);
        let mut moves = 0;
        'pick: loop {
            let Some((mut token, ticket)) = inner
                .pool
                .pick(
                    hooks,
                    &inner.scheduler,
                    requires_anlas,
                    v5_free,
                    true,
                    allowance,
                    arrival,
                )
                .await?
            else {
                return Err(self.unavailable(requires_anlas, v5_free).await);
            };
            obs.picked(&token.token_id);
            let mut fl = InFlight::new(inner.clone(), token.clone(), v5_free, 0);
            fl.ticket = ticket;
            // V5 计费确认：排队前一次（换号后对新账号再一次），排完队仍是免费再一次（见 request_once）
            match self
                .confirm_v5_stream(&token, &mut fl, cb, allowance, may_switch)
                .await
            {
                Ok(true) => {}
                Ok(false) => {
                    fl.finish().await;
                    may_switch = false;
                    continue;
                }
                Err(e) => {
                    fl.finish().await;
                    return Err(e);
                }
            }
            loop {
                // 免费 V5 确认成按 Anlas 计以后，换去的账号也得能付 Anlas
                let needs_anlas = requires_anlas || (v5_free && !fl.v5_free);
                let free_now = fl.v5_free;
                if let Some(t) = fl.ticket.as_ref() {
                    t.set_flags(needs_anlas, free_now);
                }
                let wait = self
                    .wait_slot(
                        &mut fl,
                        deadline,
                        patient,
                        moves < MAX_MOVES,
                        needs_anlas,
                        free_now,
                        allowance,
                        obs,
                    )
                    .await;
                match wait {
                    Ok(Wait::Ready) => break,
                    Ok(Wait::TimedOut) => {
                        fl.finish().await;
                        return Err(queue_timed_out());
                    }
                    Err(e) => {
                        fl.finish().await;
                        return Err(e);
                    }
                    Ok(Wait::Move(to, ticket)) => {
                        moves += 1;
                        self.switch_flight(&mut fl, &to, ticket, free_now, obs)
                            .await;
                        token = to;
                        match self
                            .confirm_v5_stream(&token, &mut fl, cb, allowance, may_switch)
                            .await
                        {
                            Ok(true) => {}
                            Ok(false) => {
                                fl.finish().await;
                                may_switch = false;
                                continue 'pick;
                            }
                            Err(e) => {
                                fl.finish().await;
                                return Err(e);
                            }
                        }
                    }
                }
            }
            match self
                .confirm_v5_stream(&token, &mut fl, cb, allowance, may_switch)
                .await
            {
                Ok(true) => {}
                Ok(false) => {
                    fl.finish().await;
                    may_switch = false;
                    continue;
                }
                Err(e) => {
                    fl.finish().await;
                    return Err(e);
                }
            }
            let accept = if body
                .get("parameters")
                .and_then(|p| p.get("stream"))
                .and_then(Value::as_str)
                == Some("msgpack")
            {
                "application/x-msgpack"
            } else {
                "text/event-stream"
            };
            // 上游回「并发锁」时在同一账号上再发（见 retry_after_lock）；其余 429 / 409 在这里就结束
            let mut locks: u32 = 0;
            let (resp, status) = loop {
                let req = inner
                    .http
                    .build(
                        token.profile_index(),
                        &token.token,
                        wreq::Method::POST,
                        url,
                        Some(body),
                        accept,
                    )
                    // 流式生图同样是非幂等 POST：3xx 不跟随（避免把请求体重发到新地址）。
                    .redirect(wreq::redirect::Policy::none());
                // 与 set_admin_enabled 串行（只在检查启停时持锁，请求本身不持锁）
                drop(token.dispatch_lock.lock().await);
                if !token.with(|f| f.admin_enabled) {
                    fl.finish().await;
                    return Err(UpstreamError::new(503, ADJUSTING));
                }
                if let Err(e) = dispatch_block(hooks, &token).await {
                    fl.finish().await;
                    return Err(e);
                }
                if let Some(check) = cb.before_dispatch.as_ref()
                    && let Err(e) = check().await
                {
                    fl.finish().await;
                    return Err(e);
                }
                // Streaming POSTs are never retried after an ambiguous send. At this point we
                // still have not persisted dispatch or sent anything, so refuse locally.
                if !token.usable(hooks.now()) {
                    fl.finish().await;
                    return Err(UpstreamError::new(
                        503,
                        "所选上游账号状态已变化，本次请求尚未发送；请稍后重试。",
                    ));
                }
                if let Some(on) = cb.on_dispatch.as_ref()
                    && let Err(e) = on().await
                {
                    fl.finish().await;
                    return Err(e);
                }
                fl.mark_sent();
                obs.sent();
                let resp = match req.send().await {
                    Ok(r) => r,
                    Err(e) => {
                        let uncertain = fl.send_started && Transport::Http(e).after_send();
                        fl.finish().await;
                        return Err(UpstreamError::uncertain(
                            502,
                            "出图时与 NovelAI 的连接中断，未扣费。请稍后手动重试。给客户端作者：请勿自动重试",
                            uncertain,
                        ));
                    }
                };
                let status = resp.status().as_u16();
                if status == 429 || status == 409 {
                    let headers = resp.headers().clone();
                    let peek = http::peek_body(resp).await;
                    if gate_core::upstream_policy::is_concurrency_lock(status, &peek) {
                        if retry_after_lock(inner, &mut fl, obs, locks, deadline).await {
                            locks += 1;
                            continue;
                        }
                        fl.finish().await;
                        return Err(locked_out());
                    }
                    obs.status(status);
                    let e = if status == 429 {
                        on_429(&inner.scheduler, hooks, &token, cb, &headers).await
                    } else {
                        UpstreamError::uncertain(
                            status,
                            format!("NovelAI 未完成出图，未扣费。请稍后重试。状态码 {status}。"),
                            false,
                        )
                    };
                    fl.finish().await;
                    return Err(e);
                }
                obs.status(status);
                break (resp, status);
            };
            if status == 402 && on_402(&inner.pool, hooks, &token, v5_free, may_switch).await {
                // 402 没出图、没扣费：换一把重试
                drop(resp);
                fl.finish().await;
                may_switch = false;
                continue;
            }
            let err = if status == 403 {
                let peek = http::peek_body(resp).await;
                let e = on_403(hooks, &token, &peek);
                fl.finish().await;
                return Err(e);
            } else if let Some(e) = on_other_status(hooks, &token, status).await {
                Some(e)
            } else if status != 200 && status != 201 {
                let s = if (400..=599).contains(&status) {
                    status
                } else {
                    502
                };
                Some(UpstreamError::uncertain(
                    s,
                    format!("NovelAI 未完成出图，未扣费。请稍后重试。状态码 {s}。"),
                    status >= 500,
                ))
            } else {
                None
            };
            if let Some(e) = err {
                fl.finish().await;
                return Err(e);
            }
            return Ok(ImageStream {
                response: Some(resp),
                flight: Some(fl),
                token,
                completed_images: 0,
            });
        }
    }

    /// 图片流的 V5 计费确认：`Ok(true)` 照常发，`Ok(false)` 换一把（免费 V5 让给别的 Token）。
    /// 与旧版一致：图片流只把这次改成「非免费 V5」，不改 requires_anlas。
    async fn confirm_v5_stream(
        &self,
        token: &Arc<Token>,
        fl: &mut InFlight<H>,
        cb: &Callbacks<'_>,
        allowance: Option<&dyn Allowance>,
        may_switch: bool,
    ) -> Result<bool, UpstreamError> {
        let inner = &self.inner;
        if !fl.v5_free {
            return Ok(true);
        }
        let (Some(resolve), Some(a)) = (cb.resolve_v5_cost.as_ref(), allowance) else {
            return Ok(true);
        };
        match resolve_v5_cost(inner, token, may_switch, a, resolve).await? {
            V5Cost::Paid => {
                inner
                    .pool
                    .finish_v5_reservation(&inner.hooks, token, false, true)
                    .await;
                fl.v5_free = false;
                Ok(true)
            }
            V5Cost::Free => Ok(true),
            V5Cost::Switch => Ok(false),
        }
    }
}

/// [`resolve_v5_cost`] 的结论。
enum V5Cost {
    Free,
    /// 这把 Token 的免费额度见底，改按 Anlas 计（已交给调用方回调复核）。
    Paid,
    /// 这把 Token 的免费额度见底，但别的 Token 还能免费出：换一把，不扣成员的 Anlas。
    Switch,
}

/// V5 是否已按 Anlas 计费的最终确认：先查官方免费额度（与停用串行），再交给调用方的回调复核。
async fn resolve_v5_cost<H: Hooks>(
    inner: &Inner<H>,
    token: &Token,
    may_switch: bool,
    allowance: &dyn Allowance,
    callback: &ResolveV5<'_>,
) -> Result<V5Cost, UpstreamError> {
    let exhausted = {
        let _d = token.dispatch_lock.lock().await;
        if !token.with(|f| f.admin_enabled) {
            return Err(UpstreamError::new(503, ADJUSTING));
        }
        allowance
            .resolve(token)
            .await
            .map_err(|m| UpstreamError::new(503, m))?
    };
    if exhausted {
        // 官方确认见底：免费 V5 先派给别的 Token（不论这次是改派、付费还是拒绝）。
        // 站点「免费 V5 是否还有」（Pool::v5_free_open）看的也是这个标记。
        inner.pool.mark_v5_out(&inner.hooks, token);
    }
    let can_switch = exhausted && may_switch && inner.pool.other_v5_open(&inner.hooks, token).await;
    // 付不了 Anlas（后台没允许、刚被上游回过 402、或最近读到的余额不够这张图）：
    // 站付和按成员 Anlas 计都不行，能换号就换，否则当场拒绝
    if exhausted && !token.can_pay_anlas(inner.hooks.now(), Some(allowance)) {
        if can_switch {
            return Ok(V5Cost::Switch);
        }
        return Err(UpstreamError::local(
            403,
            "V5 暂停，未扣费。请使用 V4.5。",
            gate_core::reasons::V5_EXHAUSTED,
        ));
    }
    // 回调决定：站付（账号自己的 Anlas）> 改派给有免费额度的 Token > 按成员的 Anlas 计
    if callback(exhausted, token.token_id.clone(), can_switch).await? {
        return Ok(V5Cost::Switch);
    }
    Ok(if exhausted {
        V5Cost::Paid
    } else {
        V5Cost::Free
    })
}

/// 已打开的上游图片流。调用方逐块读取，确认完整的最终图片后累加 `completed_images`，
/// 最后调用 [`ImageStream::finish`]；被丢弃时按已确认张数在后台结算。
pub struct ImageStream<H: Hooks> {
    response: Option<wreq::Response>,
    flight: Option<InFlight<H>>,
    token: Arc<Token>,
    pub completed_images: usize,
}

impl<H: Hooks> ImageStream<H> {
    pub fn headers(&self) -> Option<&HeaderMap> {
        self.response.as_ref().map(|r| r.headers())
    }

    /// 取走正文字节流（只能取一次）。已按 Content-Encoding 解压，边收边给，不整包缓冲。
    pub fn take_body(
        &mut self,
    ) -> Option<impl futures::Stream<Item = Result<Bytes, wreq::Error>> + use<H>> {
        self.response.take().map(|r| r.bytes_stream())
    }

    /// 结算：有确认的图就记成功并标记账号健康。
    pub async fn finish(mut self) {
        self.response = None;
        if let Some(mut fl) = self.flight.take() {
            let count = self.completed_images;
            fl.succeeded = count > 0;
            fl.image_count = count;
            let inner = fl.inner.clone();
            fl.finish().await;
            if count > 0 {
                self.token.mark_ok(&inner.hooks);
            }
        }
    }
}

impl<H: Hooks> Drop for ImageStream<H> {
    fn drop(&mut self) {
        if let Some(mut fl) = self.flight.take() {
            let count = self.completed_images;
            fl.succeeded = count > 0;
            fl.image_count = count;
            if count > 0 {
                self.token.mark_ok(&fl.inner.hooks);
            }
            drop(fl); // InFlight 的 Drop 在后台完成结算
        }
    }
}
