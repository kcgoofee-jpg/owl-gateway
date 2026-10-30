//! V5 计费确认：这把 Token 的免费额度是否见底、见底后是改派、站付还是按成员 Anlas 计。
//!
//! 确认发生在排队前后各一次（排队期间额度可能被别人用完），所以普通请求与流式请求共用同一套判断，
//! 只是流式那边多一层「换号后对新账号再确认」。

use super::*;
use outcome::InFlight;

impl<H: Hooks> NaiClient<H> {
    pub(crate) async fn confirm_v5(
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
                let mut req = inner
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
                if let Some(correlation_id) = cb.correlation_id.as_deref() {
                    req = req.header("x-correlation-id", correlation_id);
                }
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
                    return Err(UpstreamError::local(
                        503,
                        "所选上游账号状态已变化，本次请求尚未发送；请稍后重试。",
                        "",
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
    pub(crate) async fn confirm_v5_stream(
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
pub(crate) enum V5Cost {
    Free,
    /// 这把 Token 的免费额度见底，改按 Anlas 计（已交给调用方回调复核）。
    Paid,
    /// 这把 Token 的免费额度见底，但别的 Token 还能免费出：换一把，不扣成员的 Anlas。
    Switch,
}

/// V5 是否已按 Anlas 计费的最终确认：先查官方免费额度（与停用串行），再交给调用方的回调复核。
pub(crate) async fn resolve_v5_cost<H: Hooks>(
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
