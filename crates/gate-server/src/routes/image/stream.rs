//! 流式出图：响应头等上游接受之后才发；排队时断开直接取消、派发之前断开记「cancelled」，
//! 派发之后在后台任务里跑完并按实际完成的张数记账。

use super::params::Prepared;
use super::settle::{Billing, resolve_v5};
use super::{before_dispatch, detail160, on_429, on_dispatch};
use crate::accounting::Entry;
use crate::admission::{Est, acquire_slots, image_admission, reserve_image_budget};
use crate::auth::check_image_cooldown;
use crate::error::{ApiError, GateError, InternalError, json_response};
use crate::reqctx::ReqCtx;
use crate::state::AppState;
use axum::body::Body;
use axum::http::HeaderValue;
use axum::response::{IntoResponse, Response};
use bytes::Bytes;
use futures::StreamExt;
use gate_core::image_events::{ImageEventTracker, WireFormat, encode_image_event};
use gate_core::policy;
use gate_core::pyval::py_str;
use gate_core::reasons;
use gate_upstream::client::Callbacks;
use serde_json::{Value, json};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

/// 流式格式容错：返回 SSE 或 MessagePack；认不出的写法 400。
fn normalize_wire_format(raw: Option<&Value>) -> Result<WireFormat, GateError> {
    let token = match raw {
        None | Some(Value::Null) | Some(Value::Bool(_)) => return Ok(WireFormat::Sse),
        Some(v) => gate_core::pyc::strip(&py_str(v)).to_lowercase(),
    };
    match token.as_str() {
        "msgpack" | "messagepack" | "binary" => Ok(WireFormat::Msgpack),
        "" | "sse" | "text/event-stream" | "event-stream" | "stream" => Ok(WireFormat::Sse),
        _ => Err(GateError::with_code(
            400,
            "本站不支持该流式格式，未扣费。请更新或更换客户端。给客户端作者：stream 只支持 sse 或 msgpack",
            reasons::PARAM,
        )),
    }
}

/// 推流的一端：响应头要等上游接受之后才发；之前出错就回一个普通 JSON 错误。
struct StreamSink {
    head: Option<tokio::sync::oneshot::Sender<Head>>,
    tx: tokio::sync::mpsc::Sender<Bytes>,
    wire: WireFormat,
    disconnected: bool,
    /// 已经发出 200 响应头（之后接收端关闭才算客户端断开；回了 JSON 错误时接收端本来就会关）。
    started: bool,
}

enum Head {
    Start,
    Error(u16, String),
}

impl StreamSink {
    async fn start(&mut self) {
        if let Some(h) = self.head.take() {
            self.started = true;
            let _ = h.send(Head::Start);
        }
    }

    /// 客户端在拿到全部图之前走了（只在已经开始推流时才有意义）。
    fn client_gone(&self) -> bool {
        self.started && (self.disconnected || self.tx.is_closed())
    }

    /// 慢 / 已断开的接收方不能拖住上游结算：每块最多等 10 秒。
    async fn chunk(&mut self, data: Vec<u8>) {
        if self.disconnected {
            return;
        }
        let sent =
            tokio::time::timeout(Duration::from_secs(10), self.tx.send(Bytes::from(data))).await;
        if !matches!(sent, Ok(Ok(()))) {
            self.disconnected = true;
        }
    }

    async fn error(&mut self, status: u16, message: &str) {
        if let Some(h) = self.head.take() {
            let _ = h.send(Head::Error(status, message.to_string()));
        } else {
            let (status, replaced) = crate::error::member_status(status, true);
            let message = replaced.unwrap_or(message);
            let ev = json!({"event_type": "error", "error": message, "message": message, "status_code": status});
            if let Ok(frame) = encode_image_event(&ev, self.wire) {
                self.chunk(frame).await;
            }
        }
    }
}

pub(super) async fn stream(
    state: AppState,
    ctx: ReqCtx,
    mut prep: Prepared,
) -> Result<Response, ApiError> {
    // 现有面板默认 SSE；Launcher 显式要 MessagePack
    let wire = normalize_wire_format(prep.body.get("parameters").and_then(|p| p.get("stream")))?;
    let wire_name = if wire == WireFormat::Msgpack {
        "msgpack"
    } else {
        "sse"
    };
    if let Some(p) = prep
        .body
        .get_mut("parameters")
        .and_then(Value::as_object_mut)
    {
        p.insert("stream".into(), Value::String(wire_name.into()));
    }
    let (head_tx, head_rx) = tokio::sync::oneshot::channel();
    let (tx, rx) = tokio::sync::mpsc::channel::<Bytes>(32);
    let disconnect = CancellationToken::new();
    let sink = StreamSink {
        head: Some(head_tx),
        tx,
        wire,
        disconnected: false,
        started: false,
    };
    let (s, c, d) = (state.clone(), ctx.clone(), disconnect.clone());
    state.tasks.spawn(ctx.clone().scope(async move {
        run_stream(s, c, prep, sink, d).await;
    }));
    // 响应头发出之前客户端断开：通知后台任务（还没发往上游就取消）
    let guard = disconnect.clone().drop_guard();
    let head = head_rx.await;
    guard.disarm();
    match head {
        Ok(Head::Start) => {
            let body =
                Body::from_stream(tokio_stream_from(rx).map(Ok::<Bytes, std::convert::Infallible>));
            let mut r = Response::new(body);
            let h = r.headers_mut();
            h.insert("content-type", HeaderValue::from_static(wire.media_type()));
            h.insert("cache-control", HeaderValue::from_static("no-store"));
            h.insert("x-accel-buffering", HeaderValue::from_static("no"));
            Ok(r)
        }
        Ok(Head::Error(status, message)) => {
            // Head errors here are upstream failures: never show a member a 402 (see member_status)
            let (status, replaced) = crate::error::member_status(status, true);
            let message = replaced.map_or(message, str::to_string);
            let mut r = json_response(status, &json!({"error": message, "message": message}));
            if status == 429 {
                r.headers_mut()
                    .insert("retry-after", HeaderValue::from(15u64));
            }
            let h = r.headers_mut();
            h.insert("cache-control", HeaderValue::from_static("no-store"));
            h.insert("x-accel-buffering", HeaderValue::from_static("no"));
            Ok(r)
        }
        Err(_) => Ok(json_response(
            500,
            &json!({"error": "stream worker ended", "message": "服务器内部错误，已自动记录。"}),
        )),
    }
}

fn tokio_stream_from(
    mut rx: tokio::sync::mpsc::Receiver<Bytes>,
) -> impl futures::Stream<Item = Bytes> {
    futures::stream::poll_fn(move |cx| rx.poll_recv(cx))
}

/// 流式出图的主流程：排队阶段可取消；上游阶段在真正派发之前也可取消。
async fn run_stream(
    state: AppState,
    ctx: ReqCtx,
    prep: Prepared,
    mut sink: StreamSink,
    disconnect: CancellationToken,
) {
    let key = prep.key.clone();
    let model = prep.model.clone();
    let queued = async {
        let admission = image_admission(&state, &ctx, &key).await?;
        let slots = acquire_slots(&state);
        check_image_cooldown(&state)?;
        let reserved =
            reserve_image_budget(&state, &ctx, &key, &prep.est, prep.legacy_free_images).await?;
        Ok::<_, ApiError>((admission, slots, reserved))
    };
    let held = tokio::select! {
        r = queued => r,
        _ = disconnect.cancelled() => return, // 排队时断开：直接取消，不记账
    };
    let res = match held {
        Ok((admission, slots, reserved)) => {
            let _held = (admission, slots);
            perform_stream(
                &state,
                &ctx,
                prep,
                Arc::new(reserved),
                &mut sink,
                &disconnect,
            )
            .await
        }
        Err(e) => Err(e),
    };
    match res {
        Ok(()) => {}
        Err(ApiError::Gate(exc)) => {
            // 和非流式一样记「拒绝」：自动驾驶靠这些记录感知拥挤
            state.record(
                &ctx,
                &key,
                Entry::new("image_stream", &model, "rejected")
                    .detail(detail160(exc.status, &exc.message))
                    .reason(exc.code.clone()),
            );
            sink.error(exc.status, &exc.message).await;
        }
        Err(other) => {
            let resp = other.into_response();
            sink.error(resp.status().as_u16(), "服务器内部错误，已自动记录")
                .await;
        }
    }
}

enum StreamFail {
    /// 上游 / 协议 / 超时：给成员的说明、状态码、是否可能已扣费
    Upstream(u16, String, bool),
}

async fn perform_stream(
    state: &AppState,
    ctx: &ReqCtx,
    prep: Prepared,
    reserved: Arc<crate::admission::Reserved>,
    sink: &mut StreamSink,
    disconnect: &CancellationToken,
) -> Result<(), ApiError> {
    let key = prep.key.clone();
    let model = prep.model.clone();
    let image_count = prep.image_count;
    let wire = sink.wire;
    let dispatched = Arc::new(AtomicBool::new(false));
    let mut delivered = false;
    let billing = Arc::new(Mutex::new(Billing {
        est: prep.est,
        detail: prep.detail.clone(),
        site_paid: 0.0,
    }));
    let est0 = prep.est;
    let mut tracker =
        ImageEventTracker::new(image_count, wire).map_err(|e| InternalError(e.to_string()))?;
    let mut failure: Option<String> = None;
    let mut billing_uncertain = false;
    let mut local_reject = false;
    let mut cancelled = false;

    let d = dispatched.clone();
    let durable_dispatch = on_dispatch(state, ctx);
    let cb = Callbacks {
        on_rate_limited: Some(on_429(state)),
        resolve_v5_cost: (est0.v5 > 0)
            .then(|| resolve_v5(state, &prep, billing.clone(), reserved.clone())),
        before_dispatch: Some(before_dispatch(
            state,
            Some((prep.key.str("discord_id").to_string(), est0.v5 > 0)),
        )),
        on_dispatch: Some(Box::new(move || {
            let future = durable_dispatch();
            let d = d.clone();
            Box::pin(async move {
                future.await?;
                d.store(true, Ordering::SeqCst);
                Ok(())
            })
        })),
    };
    let allowance = state
        .allowance
        .with(&state.nai)
        .paid_cost(super::settle::paid_cost(&prep.body));
    let allow: Option<&dyn gate_upstream::client::Allowance> = Some(&allowance);
    let url = format!("{}/ai/generate-image-stream", state.nai.image_host());
    let outcome: Result<Result<(), StreamFail>, GateError> = async {
        // 总时长上限：上游一直发进度帧也不能无限占着
        let work = async {
            let open = state.nai.image_stream(
                &url,
                &prep.body,
                image_count,
                est0.anlas > 0.0,
                est0.v5 > 0,
                state.limits().queue_timeout,
                &cb,
                allow,
                ctx,
            );
            tokio::pin!(open);
            let mut seen_cancel = false;
            let opened = loop {
                tokio::select! {
                    r = &mut open => break r,
                    _ = disconnect.cancelled(), if !seen_cancel => {
                        seen_cancel = true;
                        if !dispatched.load(Ordering::SeqCst) {
                            return Ok(Err(None)); // 还没发往上游：取消
                        }
                    }
                }
            };
            let mut handle = match opened {
                Ok(h) => h,
                Err(e) if e.local => return Err(GateError::with_code(e.status, e.message, e.code)),
                Err(e) => {
                    return Ok(Err(Some(StreamFail::Upstream(
                        e.status,
                        e.message,
                        e.billing_uncertain,
                    ))));
                }
            };
            billing_uncertain = true;
            let media = handle
                .headers()
                .and_then(|h| h.get("content-type"))
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
                .split(';')
                .next()
                .unwrap_or("")
                .trim()
                .to_lowercase();
            let mut allowed = vec![wire.media_type()];
            if wire == WireFormat::Msgpack {
                // 官方文档对这个端点仍写 SSE：即使是这个头也按二进制帧校验
                allowed.extend([
                    "application/msgpack",
                    "application/octet-stream",
                    "text/event-stream",
                ]);
            }
            if !allowed.contains(&media.as_str()) {
                handle.finish().await;
                return Ok(Err(Some(StreamFail::Upstream(
                    502,
                    "NovelAI 返回的图片数据格式错误，未扣费。请稍后重试。".into(),
                    false,
                ))));
            }
            sink.start().await;
            let mut chunks = handle.take_body().expect("stream body");
            let fail = loop {
                let Some(chunk) = chunks.next().await else {
                    break None;
                };
                let chunk = match chunk {
                    Ok(c) => c,
                    Err(_) => {
                        break Some(StreamFail::Upstream(
                            502,
                            "出图过程中与 NovelAI 的连接中断或超时，未完成的图未扣费。请稍后重试。"
                                .into(),
                            false,
                        ));
                    }
                };
                // 错误事件在认出之前绝不转发一部分（即使跨网络分块）
                let out = tracker.frames(&chunk);
                for f in out.frames {
                    handle.completed_images = f.completed_after;
                    sink.chunk(f.bytes).await;
                }
                if let Some(e) = out.error {
                    break Some(StreamFail::Upstream(502, e.to_string(), false));
                }
                if tracker.failed() {
                    break Some(StreamFail::Upstream(
                        502,
                        "NovelAI 生成失败，未完成的图未扣费。请稍后重试。".into(),
                        false,
                    ));
                }
                if tracker.completed_images() as i64 == image_count {
                    delivered = true;
                    break None;
                }
            };
            let fail = match fail {
                Some(f) => Some(f),
                None => {
                    tracker.finish();
                    ((tracker.completed_images() as i64) < image_count).then(|| {
                        StreamFail::Upstream(
                            502,
                            "NovelAI 提前结束了出图，未完成的图未扣费。请稍后重试。".into(),
                            false,
                        )
                    })
                }
            };
            drop(chunks);
            handle.completed_images = tracker.completed_images();
            handle.finish().await;
            Ok(match fail {
                None => Ok(()),
                Some(f) => Err(Some(f)),
            })
        };
        match tokio::time::timeout(Duration::from_secs(300), work).await {
            Ok(Ok(Ok(()))) => Ok(Ok(())),
            Ok(Ok(Err(None))) => {
                cancelled = true;
                Ok(Ok(()))
            }
            Ok(Ok(Err(Some(f)))) => Ok(Err(f)),
            Ok(Err(g)) => Err(g),
            Err(_) => Ok(Err(StreamFail::Upstream(
                502,
                "出图过程中与 NovelAI 的连接中断或超时，未完成的图未扣费。请稍后重试。".into(),
                dispatched.load(Ordering::SeqCst),
            ))),
        }
    }
    .await;

    let gate_err = match outcome {
        Ok(Ok(())) => None,
        Ok(Err(StreamFail::Upstream(status, message, uncertain))) => {
            billing_uncertain |= uncertain;
            // 上游 error 事件的原话只进日志，不转发给客户端（可能含上游内部信息）
            failure = Some(if tracker.error_detail().is_empty() {
                message.clone()
            } else {
                format!("{message}（上游：{}）", tracker.error_detail())
            });
            sink.error(status, &message).await;
            None
        }
        Err(g) => {
            local_reject = !dispatched.load(Ordering::SeqCst);
            Some(g)
        }
    };

    // 收尾记账（无论成败都走到）
    let completed = tracker.completed_images() as i64;
    let est = billing.lock().expect("billing").est;
    let detail = billing.lock().expect("billing").detail.clone();
    let site_paid = billing.lock().expect("billing").site_paid;
    let was_dispatched = dispatched.load(Ordering::SeqCst);
    if !was_dispatched && completed == 0 && failure.is_none() && !local_reject {
        // 还在排队时客户端就断开了：没发到上游，不能记成「上游出错」，也不能拉低上游健康度
        state
            .settle_record(
                ctx,
                &key,
                Entry::new("image_stream", &model, "cancelled")
                    .detail("排队时客户端断开（未发到上游，未记费）"),
            )
            .await;
    } else if !(local_reject && completed == 0) {
        let mut settled_anlas = 0.0;
        if completed > 0 {
            // 按完整结果重算首张减免，沿用派发前确认的 V5 额度状态
            let mut completed_body = prep.body.clone();
            if let Some(p) = completed_body
                .get_mut("parameters")
                .and_then(Value::as_object_mut)
            {
                p.insert("n_samples".into(), json!(completed));
            }
            let settled = policy::estimate_image_cost(&completed_body, true, est.v5 > 0)
                .map(Est::from_cost)
                .unwrap_or(est);
            settled_anlas = settled.anlas;
            let suffix = if completed < image_count {
                format!("; 完成 {completed}/{image_count}")
            } else {
                String::new()
            };
            state
                .settle_record(
                    ctx,
                    &key,
                    Entry {
                        images: completed,
                        anlas: settled.anlas,
                        v5: settled.v5,
                        legacy_free_images: prep.legacy_free_images,
                        site_paid_anlas: site_paid,
                        detail: format!("{detail}{suffix}"),
                        ..Entry::new("image_stream", &model, "ok")
                    },
                )
                .await;
        }
        let ok = completed > 0 && failure.is_none();
        state
            .audit_generation(
                ctx,
                &key,
                "image_stream",
                &model,
                if ok { "ok" } else { "error" },
                &prep.body,
                if completed > 0 {
                    tracker.first_image().map(Bytes::copy_from_slice)
                } else {
                    None
                },
            )
            .await;
        state.record_upstream(ok);
        if failure.is_some() || completed == 0 {
            // 未结算部分单独记为待核对费用
            state
                .settle_record(
                    ctx,
                    &key,
                    Entry::new("image_stream", &model, "error")
                        .detail(
                            failure
                                .clone()
                                .unwrap_or_else(|| "未收到最终图片，未记费".into()),
                        )
                        .unconfirmed(if billing_uncertain {
                            (est.anlas - settled_anlas).max(0.0)
                        } else {
                            0.0
                        }),
                )
                .await;
        }
    }
    if cancelled {
        return Ok(());
    }
    // 派发之后客户端断开、没拿到全部图（图已生成并计数）：这是客户端的行为（超时、关了页面、换了请求），
    // 不是网关的 Bug，不进 Bug 列表（以前每次都记一条，总览永远挂着「1 个 Bug 没处理」）；只留一行日志，请求本身在用量日志里
    if (disconnect.is_cancelled() || sink.client_gone()) && !delivered && was_dispatched {
        tracing::info!(rid = %ctx.rid(), "客户端在图片返回前断开连接（图已生成并计数）");
    }
    match gate_err {
        Some(g) => Err(g.into()),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_format_aliases() {
        assert_eq!(normalize_wire_format(None).unwrap(), WireFormat::Sse);
        assert_eq!(
            normalize_wire_format(Some(&json!(true))).unwrap(),
            WireFormat::Sse
        );
        assert_eq!(
            normalize_wire_format(Some(&json!(" MessagePack "))).unwrap(),
            WireFormat::Msgpack
        );
        assert_eq!(
            normalize_wire_format(Some(&json!("text/event-stream"))).unwrap(),
            WireFormat::Sse
        );
        assert_eq!(
            normalize_wire_format(Some(&json!(1))).unwrap_err().status,
            400
        );
    }
}
