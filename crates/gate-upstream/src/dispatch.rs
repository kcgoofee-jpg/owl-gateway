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
//!
//! 上游明确的 5xx 响应（收到了 5xx 状态码；传输故障与「结果不明」不在此列）在结算完成后
//! 于同一账号上不占排队位重发一次：官方对 5xx 不扣费，线上占比 0.15%，同 Key 几分钟内可重试成功。

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

mod outcome;
mod stream;
mod v5;

pub(crate) use outcome::InFlight;
use outcome::{
    ADJUSTING, Attempt, MAX_MOVES, POLL, Wait, dispatch_block, locked_out, on_402, on_403, on_429,
    on_other_status, queue_timed_out, retry_after_lock,
};
pub use stream::ImageStream;

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
            let last_token = fl.token.clone();
            fl.finish().await;
            match outcome {
                Attempt::Done(r) => {
                    // 上游明确的 5xx 响应（非传输故障、非并发锁、非 401→502）：settle 已完成（未扣费），
                    // 在同一账号上不占排队位重发一次（acquire 无票 = 只等出图间隔）。
                    // 只试一次，重试的结果（无论成败）直接返回；重试路径要求换号 / V5 异常时保守返回原始 5xx。
                    if o.image_lane && r.as_ref().is_ok_and(|resp| resp.status >= 500) {
                        obs.picked(&last_token.token_id);
                        tracing::warn!(
                            target: "upstream",
                            token_id = %last_token.token_id,
                            status = r.as_ref().unwrap().status,
                            "上游 5xx：已结算（未扣费），在同一账号上不占排队位重发一次"
                        );
                        let mut fl2 =
                            InFlight::new(inner.clone(), last_token, o.v5_free, o.image_count);
                        let outcome2 = self
                            .request_once(&mut o, cb, allowance, obs, &mut fl2, !switched)
                            .await;
                        fl2.finish().await;
                        return match outcome2 {
                            Attempt::Done(r2) => r2,
                            Attempt::Retry | Attempt::SwitchV5 => r,
                        };
                    }
                    return r;
                }
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
            if let Some(correlation_id) = cb.correlation_id.as_deref() {
                req = req.header("x-correlation-id", correlation_id);
            }
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
                        if uncertain {
                            "与 NovelAI 的连接中断或超时，上游执行结果不明；本次额度暂时保留，请勿自动重试。"
                        } else {
                            "未能连接 NovelAI，请求未执行、未扣费。请稍后手动重试，请勿自动重试。"
                        },
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
}
