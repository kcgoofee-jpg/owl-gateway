//! 出图、流式出图、Vibe 编码、图片工具、标签补全。
//!
//! 按职责拆成几个文件：
//! - 这里：路由表、各接口的入口（鉴权 → 读请求体 → 交给下面的模块），以及几处共用的小件
//!   （上游错误转换、429 冷却回调、「开始上游操作后放进后台任务跑完」的 `shielded`）；
//! - `params`：生图参数校验与整理、免费档钳制、费用预估（`prepare`）；
//! - `settle`：V5 额度确认后的重估（`resolve_v5`）、非流式出图的转发与记账；
//! - `stream`：流式出图（推流、断开取消、收尾记账）；
//! - `tools`：放大 / 导演工具、Vibe 编码；
//! - `tags`：标签补全。
//!
//! 取消语义：
//! - 持久化派发边界之前客户端断开 = 取消排队并释放额度，不向上游发送；
//! - 派发边界开始后，保留后台任务完成结算；普通与流式请求使用相同的非幂等请求保护原则。
//!
//! 每把 Key 的限速只有排队与 RPM 两层：2.x 的 15 秒出图间隔、「同时在途 ≤ 4」对出图请求已删除
//! （都与排队重复）。

mod params;
mod settle;
mod stream;
mod tags;
mod tools;

use crate::accounting::Entry;
use crate::admission::{acquire_slots, guard_hard_block, image_admission, reserve_image_budget};
use crate::auth::{authenticate, check_image_cooldown, check_rpm, ctx_of, require_feature};
use crate::body::{IMAGE_BODY_LIMIT, read_image_payload};
use crate::error::{ApiError, ApiResult, GateError, InternalError};
use crate::reqctx::ReqCtx;
use crate::state::AppState;
use axum::Router;
use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::HeaderValue;
use axum::response::Response;
use axum::routing::post;
use bytes::Bytes;
use gate_core::primitives::take_chars;
use gate_core::values::text;
use gate_upstream::client::{BoxFuture, UpstreamError};
use serde_json::Value;
use std::future::Future;
use std::sync::Arc;

pub fn router() -> Router<AppState> {
    let mut r = Router::new();
    for p in ["/ai/generate-image", "/nai/ai/generate-image", "/"] {
        r = r.route(p, post(generate_image));
    }
    for p in ["/ai/generate-image-stream", "/nai/ai/generate-image-stream"] {
        r = r.route(p, post(generate_image_stream));
    }
    for p in ["/ai/encode-vibe", "/nai/ai/encode-vibe"] {
        r = r.route(p, post(encode_vibe));
    }
    for p in ["/ai/upscale", "/nai/ai/upscale"] {
        r = r.route(p, post(upscale));
    }
    for p in ["/ai/augment-image", "/nai/ai/augment-image"] {
        r = r.route(p, post(augment));
    }
    for p in [
        "/ai/generate-image/suggest-tags",
        "/nai/ai/generate-image/suggest-tags",
    ] {
        r = r.route(p, post(suggest_tags).get(suggest_tags));
    }
    r
}

// ------------------------------------------------------------------ 公共小件

/// 上游错误 → 成员接口错误的统一转换。网关回调的拒绝原样穿过。
pub(super) fn up_to_gate(e: UpstreamError) -> GateError {
    if e.local {
        let mut error = GateError::with_code(e.status, e.message, e.code);
        if let Some(secs) = e.retry_after {
            error = error.retry_after(secs);
        }
        return error;
    }
    let status = if matches!(e.status, 429 | 503) {
        e.status
    } else {
        502
    };
    let retry_after = e.retry_after;
    let mut error = GateError::with_code(status, e.message, e.code)
        .uncertain(e.billing_uncertain)
        .from_upstream(true);
    if let Some(secs) = retry_after {
        error = error.retry_after(secs);
    }
    error
}

pub(super) fn local(e: ApiError) -> UpstreamError {
    match e {
        ApiError::Gate(g) => UpstreamError {
            retry_after: g.retry_after,
            ..UpstreamError::local(g.status, g.message, g.code)
        },
        ApiError::Http(h) => UpstreamError::local(h.status, text(&h.detail), ""),
        ApiError::Internal(i) => UpstreamError::local(500, i.0, ""),
    }
}

pub(super) fn log_client_disconnect(
    ctx: &ReqCtx,
    stage: &str,
    reason: &str,
    dispatch_claimed: bool,
    dispatched: bool,
) {
    if ctx.mark_client_disconnected() {
        tracing::info!(
            rid = %ctx.rid(),
            correlation_id = %ctx.correlation_id(),
            elapsed_ms = ctx.elapsed_ms(),
            stage,
            reason,
            dispatch_claimed,
            dispatched,
            event = "client_disconnected",
            "客户端连接在请求响应完整结束前关闭"
        );
    }
}

/// 用量日志里的尺寸文案：`{宽}x{高}/{步数}step`（取自请求体的 parameters）。
pub(super) fn size_text(body: &Value) -> String {
    let g = |k: &str| {
        text(
            body.get("parameters")
                .and_then(|p| p.get(k))
                .unwrap_or(&Value::Null),
        )
    };
    format!("{}x{}/{}step", g("width"), g("height"), g("steps"))
}

/// 上游非 200 时回给成员的状态码：4xx 原样，其余一律 502。
pub(super) fn tool_status(upstream: u16) -> u16 {
    if upstream == 402 {
        503
    } else if (400..500).contains(&upstream) {
        upstream
    } else {
        502
    }
}

pub(super) fn detail160(status: u16, message: &str) -> String {
    take_chars(&format!("{status} {message}"), 160).to_string()
}

/// 上游 429：暂停全站图片请求（两种响应都在收尾时记日志，回调只更新冷却）。
pub(super) fn on_429(state: &AppState) -> Box<dyn Fn(f64) -> BoxFuture<'static, ()> + Send + Sync> {
    let s = state.clone();
    Box::new(move |retry_after| {
        let s = s.clone();
        Box::pin(async move {
            let secs = s.limits().image_429_cooldown_seconds.max(retry_after);
            s.block_image_generation(secs).await;
        })
    })
}

pub(super) fn before_dispatch(
    state: &AppState,
) -> Box<dyn Fn() -> BoxFuture<'static, Result<(), UpstreamError>> + Send + Sync> {
    let state = state.clone();
    Box::new(move || {
        let state = state.clone();
        Box::pin(async move { check_image_cooldown(&state).map_err(|error| local(error.into())) })
    })
}

/// Persist the dispatch boundary before any non-idempotent upstream POST is sent.
pub(super) fn on_dispatch(
    state: &AppState,
    ctx: &ReqCtx,
    reserved: Arc<crate::admission::Reserved>,
) -> Box<dyn Fn() -> BoxFuture<'static, Result<(), UpstreamError>> + Send + Sync> {
    let (s, c, reserved) = (state.clone(), ctx.clone(), reserved.clone());
    Box::new(move || {
        let (_s, c, reserved) = (s.clone(), c.clone(), reserved.clone());
        Box::pin(async move {
            if !c.claim_dispatch() {
                return Err(local(
                    GateError::with_code(
                        499,
                        "客户端已断开，未向上游派发。",
                        "client_disconnected",
                    )
                    .into(),
                ));
            }
            match reserved.prepare_dispatch(&c.correlation_id()).await {
                Ok((day, _month)) => {
                    c.set_settlement_day(day);
                    c.dispatch_persisted();
                    Ok(())
                }
                Err(e) => Err(local(e)),
            }
        })
    })
}

/// 所有可能产生非幂等上游请求的工作都由后台任务统一管理。
/// 调用方在派发竞争前断开时丢弃排队 future；派发边界获胜后则继续完成并结算。
pub(super) async fn shielded<T: Send + 'static>(
    state: &AppState,
    ctx: &ReqCtx,
    cancelled: Entry,
    fut: impl Future<Output = Result<T, ApiError>> + Send + 'static,
) -> Result<T, ApiError> {
    let (mut tx, rx) = tokio::sync::oneshot::channel();
    let (c, s) = (ctx.clone(), state.clone());
    state.tasks.spawn(ctx.clone().scope(async move {
        let mut fut = Box::pin(fut);
        let mut disconnected = false;
        loop {
            tokio::select! {
                biased;
                _ = tx.closed(), if !disconnected => {
                    disconnected = true;
                    let stage = c.lifecycle_stage();
                    let cancelled_before_dispatch = c.cancel_before_dispatch();
                    log_client_disconnect(
                        &c,
                        stage,
                        "response_not_returned",
                        matches!(stage, "dispatching" | "dispatched") || c.was_sent(),
                        c.was_sent(),
                    );
                    if cancelled_before_dispatch {
                        // Dropping the queued future releases Admission, Reserved, and upstream
                        // scheduler guards. Reserved's durable close is idempotent with the log.
                        drop(fut);
                        c.finish_lifecycle();
                        match s
                            .db
                            .release_request_reservation(&c.internal_id(), s.now())
                            .await
                        {
                            Ok(true) => c.mark_settlement_reserved(),
                            Ok(false) => {}
                            Err(error) => tracing::error!(%error, rid = %c.rid(), "failed to release reservation after client disconnect"),
                        }
                        if let Some(task) = s.record_context(&c, cancelled)
                            && let Err(error) = task.await
                        {
                            tracing::error!(%error, "client disconnect accounting task failed");
                        }
                        return;
                    }

                    // The dispatch claim won the race. Keep the POST and accounting alive, but
                    // persist the lost response immediately (the body wrapper is installed later).
                    let db = s.db.clone();
                    let request_id = c.internal_id();
                    let now = s.now();
                    s.tasks.spawn(async move {
                        if let Err(error) = db.set_request_delivery(&request_id, "transport_interrupted", now).await {
                            tracing::error!(%error, request_id, "delivery state update failed after client disconnect");
                        }
                    });
                }
                result = &mut fut => {
                    let stage = c.lifecycle_stage();
                    c.finish_lifecycle();
                    if tx.send(result).is_err() {
                        log_client_disconnect(
                            &c,
                            stage,
                            "response_not_returned",
                            c.was_sent(),
                            c.was_sent(),
                        );
                        let db = s.db.clone();
                        let request_id = c.internal_id();
                        let now = s.now();
                        s.tasks.spawn(async move {
                            if let Err(error) = db.set_request_delivery(&request_id, "transport_interrupted", now).await {
                                tracing::error!(%error, request_id, "delivery state update failed after client disconnect");
                            }
                        });
                    }
                    return;
                }
            }
        }
    }));
    rx.await
        .unwrap_or_else(|_| Err(InternalError("后台任务异常结束".into()).into()))
}

pub(super) fn binary(content: Bytes, media: &str, no_store: bool) -> Response {
    let mut r = Response::new(Body::from(content));
    r.headers_mut().insert(
        "content-type",
        HeaderValue::from_str(media)
            .unwrap_or(HeaderValue::from_static("application/octet-stream")),
    );
    if no_store {
        r.headers_mut()
            .insert("cache-control", HeaderValue::from_static("no-store"));
    }
    r
}

pub(super) fn track_delivery(mut response: Response, state: &AppState, ctx: &ReqCtx) -> Response {
    use futures::Stream;
    use std::{
        pin::Pin,
        task::{Context, Poll},
    };
    struct Delivery {
        inner: Pin<Box<dyn Stream<Item = Result<Bytes, axum::Error>> + Send>>,
        state: AppState,
        request_id: String,
        ctx: ReqCtx,
        terminal: bool,
    }
    impl Delivery {
        fn mark(&self, value: &'static str, reason: &'static str) {
            if value == "transport_interrupted" {
                log_client_disconnect(
                    &self.ctx,
                    "response_body",
                    reason,
                    self.ctx.was_sent(),
                    self.ctx.was_sent(),
                );
            }
            let (s, id, value) = (
                self.state.clone(),
                self.request_id.clone(),
                value.to_string(),
            );
            let tasks = s.tasks.clone();
            tasks.spawn(async move {
                if let Err(e) = s.db.set_request_delivery(&id, &value, s.now()).await {
                    tracing::error!(error = %e, request_id = %id, "delivery state update failed");
                }
            });
        }
    }
    impl Stream for Delivery {
        type Item = Result<Bytes, axum::Error>;
        fn poll_next(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
        ) -> Poll<Option<Result<Bytes, axum::Error>>> {
            match self.inner.as_mut().poll_next(cx) {
                Poll::Ready(None) => {
                    if !self.terminal {
                        self.terminal = true;
                        self.mark("transport_complete", "response_body_complete");
                    }
                    Poll::Ready(None)
                }
                Poll::Ready(Some(Err(error))) => {
                    if !self.terminal {
                        self.terminal = true;
                        self.mark("transport_interrupted", "response_body_error");
                    }
                    Poll::Ready(Some(Err(error)))
                }
                other => other,
            }
        }
    }
    impl Drop for Delivery {
        fn drop(&mut self) {
            if !self.terminal {
                self.mark("transport_interrupted", "response_body_dropped_before_eof");
            }
        }
    }
    let body = std::mem::replace(response.body_mut(), Body::empty());
    *response.body_mut() = Body::from_stream(Delivery {
        inner: Box::pin(body.into_data_stream()),
        state: state.clone(),
        request_id: ctx.internal_id(),
        ctx: ctx.clone(),
        terminal: false,
    });
    response
}

// ------------------------------------------------------------------ 出图

async fn generate_image(State(state): State<AppState>, req: Request) -> ApiResult {
    generate(state, req, false).await
}

async fn generate_image_stream(State(state): State<AppState>, req: Request) -> ApiResult {
    generate(state, req, true).await
}

async fn generate(state: AppState, req: Request, streaming: bool) -> ApiResult {
    let (parts, body) = req.into_parts();
    let ctx = ctx_of(&parts);
    let key = authenticate(&state, &parts, false).await?;
    check_image_cooldown(&state)?;
    // 安静时段 / 熔断这类全站拒绝排在每分钟次数之前：重试拿到的是真正的原因，不算成 key_rate
    guard_hard_block(&state).await?;
    check_rpm(&state, &key)?;
    require_feature(&state, &key, "image").await?;
    let body = read_image_payload(&parts.headers, body, IMAGE_BODY_LIMIT).await?;
    // 参考图哈希：参数校验之前记，被拒的请求也留证据
    state.record_ref_images(
        &ctx,
        key.id(),
        crate::ref_images::images_in_params(body.get("parameters")),
    );
    let prep = params::prepare(&state, &ctx, key, body).await?;
    if streaming {
        return stream::stream(state, ctx, prep).await;
    }
    let key = prep.key.clone();
    let cancelled = Entry::new("image", &prep.model, "cancelled")
        .detail("派发前客户端断开，未向上游发送、未扣费")
        .reason("client_disconnected");
    let (s, c) = (state.clone(), ctx.clone());
    shielded(&state, &ctx, cancelled, async move {
        let admission = image_admission(&s, &c, &key).await?;
        let slots = acquire_slots(&s);
        // 排队 / 预算等待之后再查一次冷却和额度
        check_image_cooldown(&s)?;
        let reserved = reserve_image_budget(
            &s,
            &c,
            &key,
            &prep.est,
            prep.legacy_free_images,
            crate::admission::DispatchRequirements::image(&prep.body)?,
        )
        .await?;
        let reserved = Arc::new(reserved);
        let _held = (admission, slots, reserved.clone());
        settle::perform_generation(&s, &c, prep, reserved).await
    })
    .await
}

// ------------------------------------------------------------------ 图片工具 / Vibe 编码 / 标签补全

async fn upscale(State(state): State<AppState>, req: Request) -> ApiResult {
    tools::tool(state, req, "upscale").await
}

async fn augment(State(state): State<AppState>, req: Request) -> ApiResult {
    tools::tool(state, req, "augment-image").await
}

async fn encode_vibe(State(state): State<AppState>, req: Request) -> ApiResult {
    tools::vibe(state, req).await
}

async fn suggest_tags(State(state): State<AppState>, req: Request) -> ApiResult {
    tags::tags(state, req).await
}
