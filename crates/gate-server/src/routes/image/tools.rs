//! 图片工具（放大 / 导演工具）与 Vibe 编码：只有成功返回结果才记费。

use super::{
    before_dispatch, binary, on_429, on_dispatch, shielded, tool_status, track_delivery, up_to_gate,
};
use crate::accounting::Entry;
use crate::admission::{self, Est, acquire_slots, image_admission, reserve_image_budget};
use crate::auth::{authenticate, check_image_cooldown, check_rpm, ctx_of, require_feature};
use crate::body::{IMAGE_BODY_LIMIT, read_image_payload};
use crate::error::{ApiError, GateError, InternalError};
use crate::media::{MAX_RESPONSE_BYTES, prepare_tool, validate_result};
use crate::state::AppState;
use axum::extract::Request;
use axum::response::Response;
use gate_core::policy::{self, VIBE_ENCODING_ANLAS};
use gate_core::reasons;
use gate_core::values::text;
use gate_upstream::client::{Callbacks, RequestOpts};
use serde_json::{Value, json};
use std::sync::Arc;

pub(super) async fn tool(
    state: AppState,
    req: Request,
    operation: &'static str,
) -> Result<Response, ApiError> {
    let (parts, body) = req.into_parts();
    let ctx = ctx_of(&parts);
    let key = authenticate(&state, &parts, false).await?;
    check_image_cooldown(&state)?;
    check_rpm(&state, &key)?;
    require_feature(
        &state,
        &key,
        if operation == "upscale" {
            "upscale"
        } else {
            "augment"
        },
    )
    .await?;
    let body = read_image_payload(&parts.headers, body, IMAGE_BODY_LIMIT).await?;
    let body = Value::Object(body);
    let op = operation.to_string();
    let (payload, cost) = tokio::task::spawn_blocking(move || prepare_tool(&body, &op))
        .await
        .map_err(|e| InternalError(e.to_string()))?
        .map_err(|m| GateError::with_code(400, m, reasons::PARAM))?;
    let model = payload
        .get("model")
        .or_else(|| payload.get("req_type"))
        .map(text)
        .unwrap_or_default();
    let est = Est {
        anlas: cost as f64,
        v5: 0,
        is_vibe: false,
    };
    let (s, c) = (state.clone(), ctx.clone());
    let cancelled = Entry::new(operation, &model, "cancelled")
        .detail("派发前客户端断开，未向上游发送、未扣费")
        .reason("client_disconnected");
    shielded(&state, &ctx, cancelled, async move {
        let admission = image_admission(&s, &c, &key).await?;
        let slots = acquire_slots(&s);
        check_image_cooldown(&s)?;
        let reserved = Arc::new(
            reserve_image_budget(
                &s,
                &c,
                &key,
                &est,
                0,
                crate::admission::DispatchRequirements::feature(if operation == "upscale" {
                    "upscale"
                } else {
                    "augment"
                }),
            )
            .await?,
        );
        let _held = (admission, slots, reserved.clone());
        let state = s;
        let ctx = c;
        let mut billing_uncertain = false;
        let failed: Result<Response, (GateError, bool)> = async {
            let mut opts = RequestOpts::post(
                format!("{}/ai/{operation}", state.nai.image_host()),
                &payload,
            );
            opts.requires_anlas = cost > 0;
            opts.image_lane = true;
            opts.max_response_bytes = Some(MAX_RESPONSE_BYTES);
            opts.queue_timeout = state.limits().queue_timeout;
            let cb = Callbacks {
                correlation_id: Some(ctx.correlation_id()),
                on_rate_limited: Some(on_429(&state)),
                before_dispatch: Some(before_dispatch(&state)),
                on_dispatch: Some(on_dispatch(&state, &ctx, reserved.clone())),
                ..Default::default()
            };
            let resp = match state.nai.request(opts, &cb, None, &ctx).await {
                Ok(r) => r,
                Err(e) if e.transport => return Err((GateError::new(502, ""), true)),
                Err(e) => return Err((up_to_gate(e), false)),
            };
            billing_uncertain = matches!(resp.status, 200 | 201) || resp.status >= 500;
            if !matches!(resp.status, 200 | 201) {
                let uncertain = resp.status >= 500;
                let error = GateError::new(
                    tool_status(resp.status),
                    if uncertain {
                        format!(
                            "NovelAI 图片工具返回 HTTP {}，执行结果未知。请先在“我的请求”中核对。",
                            resp.status
                        )
                    } else {
                        format!(
                            "NovelAI 未完成图片处理，未扣费。请稍后重试。状态码 {}。",
                            resp.status
                        )
                    },
                )
                .from_upstream(true);
                let error = if uncertain {
                    error.uncertain(true).retryable(false)
                } else {
                    error
                };
                return Err((error, false));
            }
            let content = resp.body.clone();
            let req_type = text(
                payload
                    .get("req_type")
                    .unwrap_or(&Value::String(String::new())),
            );
            let checked = tokio::task::spawn_blocking(move || {
                validate_result(&content, &req_type, None).map(|(m, n)| (m.to_string(), n))
            })
            .await
            .map_err(|_| (GateError::new(502, ""), true))?;
            let (media, count) = checked.map_err(|_| (GateError::new(502, ""), true))?;
            state
                .settle_record(
                    &ctx,
                    &key,
                    Entry {
                        images: count as i64,
                        anlas: cost as f64,
                        detail: format!("图片工具 {cost} Anlas；返回 {count} 张"),
                        ..Entry::new(operation, &model, "ok")
                    },
                )
                .await;
            Ok(track_delivery(
                binary(resp.body, &media, true),
                &state,
                &ctx,
            ))
        }
        .await;
        match failed {
            Ok(r) => Ok(r),
            Err((exc, opaque)) => {
                billing_uncertain |= exc.billing_uncertain;
                let detail = if opaque {
                    "图片工具连接失败或返回无效结果，执行结果未知".to_string()
                } else {
                    exc.message.clone()
                };
                state
                    .settle_record(
                        &ctx,
                        &key,
                        Entry::new(operation, &model, "error")
                            .detail(detail)
                            .unconfirmed(if billing_uncertain { cost as f64 } else { 0.0 })
                            .execution_unknown(
                                billing_uncertain || opaque,
                                if billing_uncertain || opaque {
                                    cost as f64
                                } else {
                                    0.0
                                },
                                0,
                                0,
                            ),
                    )
                    .await;
                if opaque {
                    return Err(GateError::new(
                        502,
                        "图片工具连接中断，执行结果未知。请先在“我的请求”中核对；请勿自动重试。",
                    )
                    .uncertain(true)
                    .retryable(false)
                    .from_upstream(true)
                    .into());
                }
                let exc = if billing_uncertain || opaque {
                    exc.uncertain(true).retryable(false)
                } else {
                    exc
                };
                Err(exc.into())
            }
        }
    })
    .await
}

/// V4 / V4.5 参考图编码：只有成功返回二进制结果才扣 2 Anlas。
pub(super) async fn vibe(state: AppState, req: Request) -> Result<Response, ApiError> {
    let (parts, body) = req.into_parts();
    let ctx = ctx_of(&parts);
    let key = authenticate(&state, &parts, false).await?;
    check_image_cooldown(&state)?;
    check_rpm(&state, &key)?;
    require_feature(&state, &key, "vibe").await?;
    let mut body = read_image_payload(&parts.headers, body, IMAGE_BODY_LIMIT).await?;
    // 兼容 Launcher 的写法
    if let Some(value) = body.shift_remove("information_extracted") {
        if body
            .get("informationExtracted")
            .is_some_and(|v| *v != value)
        {
            return Err(GateError::with_code(400, "Vibe 的「信息提取」数值不一致，未扣费。请更新或更换客户端。给客户端作者：两种 informationExtracted 字段的值须一致", reasons::PARAM).into());
        }
        body.insert("informationExtracted".into(), value);
    }
    let body = Value::Object(body);
    if let Some(problem) = policy::validate_vibe_encoding(&body) {
        return Err(GateError::with_code(400, problem, reasons::PARAM).into());
    }
    if let Some(img) = body["image"].as_str().filter(|s| !s.is_empty()) {
        state.record_ref_images(&ctx, key.id(), vec![("vibe_encode", img.to_string())]);
    }
    let model = text(&body["model"]);
    let payload = json!({
        "image": body["image"],
        "model": body["model"],
        "information_extracted": body["informationExtracted"],
    });
    let anlas = VIBE_ENCODING_ANLAS as f64;
    let est = Est {
        anlas,
        v5: 0,
        is_vibe: true,
    };
    let (s, c) = (state.clone(), ctx.clone());
    let cancelled = Entry::new("vibe_encode", &model, "cancelled")
        .detail("派发前客户端断开，未向上游发送、未扣费")
        .reason("client_disconnected");
    shielded(&state, &ctx, cancelled, async move {
        let snapshot = s.budget.lock().await.clone();
        admission::quota_image_check(&s, &snapshot, &key, &est, 0, None).await?;
        let admission = image_admission(&s, &c, &key).await?;
        let slots = acquire_slots(&s);
        check_image_cooldown(&s)?;
        let reserved = Arc::new(
            reserve_image_budget(
                &s,
                &c,
                &key,
                &est,
                0,
                crate::admission::DispatchRequirements::feature("vibe"),
            )
            .await?,
        );
        let _held = (admission, slots, reserved.clone());
        let state = s;
        let ctx = c;
        let mut opts = RequestOpts::post(
            format!("{}/ai/encode-vibe", state.nai.image_host()),
            &payload,
        );
        opts.accept = "application/octet-stream";
        opts.requires_anlas = true;
        opts.image_lane = true;
        opts.max_response_bytes = Some(MAX_RESPONSE_BYTES);
        opts.queue_timeout = state.limits().queue_timeout;
        let cb = Callbacks {
            correlation_id: Some(ctx.correlation_id()),
            on_rate_limited: Some(on_429(&state)),
            before_dispatch: Some(before_dispatch(&state)),
            on_dispatch: Some(on_dispatch(&state, &ctx, reserved.clone())),
            ..Default::default()
        };
        let err_entry = |detail: String, unconfirmed: f64, unknown: bool| {
            Entry::new("vibe_encode", &model, "error")
                .detail(detail)
                .unconfirmed(unconfirmed)
                .execution_unknown(unknown, if unknown { anlas } else { 0.0 }, 0, 0)
        };
        let resp = match state.nai.request(opts, &cb, None, &ctx).await {
            Ok(r) => r,
            Err(e) if e.transport => {
                state
                    .settle_record(
                        &ctx,
                        &key,
                        err_entry("上游编码请求失败，未记费".into(), anlas, true),
                    )
                    .await;
                return Err(GateError::new(
                    502,
                    "Vibe 编码连接中断，执行结果未知。请先在“我的请求”中核对；请勿自动重试。",
                )
                .uncertain(true)
                .retryable(false)
                .from_upstream(true)
                .into());
            }
            Err(e) => {
                let exc = up_to_gate(e);
                let unconfirmed = if exc.billing_uncertain { anlas } else { 0.0 };
                state
                    .settle_record(
                        &ctx,
                        &key,
                        err_entry(exc.message.clone(), unconfirmed, exc.billing_uncertain),
                    )
                    .await;
                return Err(exc.into());
            }
        };
        if !matches!(resp.status, 200 | 201) {
            let unconfirmed = if resp.status >= 500 { anlas } else { 0.0 };
            let uncertain = resp.status >= 500;
            state
                .settle_record(
                    &ctx,
                    &key,
                    err_entry(format!("upstream {}", resp.status), unconfirmed, uncertain),
                )
                .await;
            let error = GateError::new(
                tool_status(resp.status),
                if uncertain {
                    format!(
                        "NovelAI Vibe 编码返回 HTTP {}，执行结果未知。请先在“我的请求”中核对。",
                        resp.status
                    )
                } else {
                    format!(
                        "NovelAI 未完成 Vibe 编码，未扣费。请稍后重试。状态码 {}。",
                        resp.status
                    )
                },
            )
            .from_upstream(true);
            return Err(if uncertain {
                error.uncertain(true).retryable(false).into()
            } else {
                error.into()
            });
        }
        let ct = resp
            .headers
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("application/octet-stream")
            .to_lowercase();
        if resp.body.is_empty() || ct.contains("json") || ct.starts_with("text/") {
            state
                .settle_record(
                    &ctx,
                    &key,
                    err_entry("上游未返回有效二进制编码，未记费".into(), anlas, true),
                )
                .await;
            return Err(GateError::new(
                502,
                "Vibe 编码返回的数据无效，执行结果未知。请先在“我的请求”中核对。",
            )
            .uncertain(true)
            .retryable(false)
            .from_upstream(true)
            .into());
        }
        state
            .settle_record(
                &ctx,
                &key,
                Entry {
                    anlas,
                    detail: format!("Vibe 编码 {VIBE_ENCODING_ANLAS} Anlas"),
                    ..Entry::new("vibe_encode", &model, "ok")
                },
            )
            .await;
        Ok(track_delivery(
            binary(resp.body, "application/octet-stream", true),
            &state,
            &ctx,
        ))
    })
    .await
}
