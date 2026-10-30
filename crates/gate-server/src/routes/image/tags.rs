//! 标签补全：同一把 Key 只保留最新的一次查询，排队有上限，超时记一条失败。

use super::{binary, up_to_gate};
use crate::accounting::Entry;
use crate::admission::acquire_slots;
use crate::auth::{authenticate, check_image_cooldown, check_rpm, ctx_of, require_feature};
use crate::body::read_json;
use crate::error::{ApiError, GateError, InternalError, upstream_transport};
use crate::key::Key;
use crate::reqctx::ReqCtx;
use crate::state::AppState;
use axum::body::Body;
use axum::extract::{Query, Request};
use axum::http::Method;
use axum::response::Response;
use gate_core::reasons;
use gate_core::upstream_errors::upstream_error_message;
use gate_core::values::{enabled, text};
use gate_upstream::client::{Callbacks, RequestOpts};
use serde_json::json;
use std::collections::HashMap;
use std::time::Duration;

pub(super) async fn tags(state: AppState, req: Request) -> Result<Response, ApiError> {
    let (parts, body) = req.into_parts();
    let ctx = ctx_of(&parts);
    let key = authenticate(&state, &parts, false).await?;
    // 10-31：标签补全以前完全不计数，客户端可以无节制地灌（线上 24 小时 120 次里 40 次等到 90 秒超时）。
    // 它仍然是轻请求，所以只按每把 Key 的每分钟次数算，不占图片队列、不占出图额度。
    check_rpm(&state, &key)?;
    check_image_cooldown(&state)?;
    require_feature(&state, &key, "tags").await?;
    struct Finish<'a>(&'a AppState, i64, bool);
    impl Drop for Finish<'_> {
        fn drop(&mut self) {
            if self.2 {
                self.0.tags.finish(self.1);
            }
        }
    }
    let mut fin = Finish(&state, key.id(), false);
    let l = state.limits();
    let run = async {
        let admitted = state
            .tags
            .wait(key.id(), crate::runtime::TAG_CONCURRENCY)
            .await;
        match admitted {
            // 已被同一 Key 更新的补全查询取代：回空结果，不记拒绝
            crate::runtime::TagAdmit::Superseded => {
                return Ok(crate::error::ok_json(&json!({"tags": []})));
            }
            crate::runtime::TagAdmit::Full => {
                return Err(GateError::with_code(
                    429,
                    "标签补全请求过多，请稍后重试。",
                    reasons::QUEUE_FULL,
                )
                .into());
            }
            crate::runtime::TagAdmit::Admitted => fin.2 = true,
        }
        suggest(&state, &ctx, &key, &parts, body).await
    };
    match tokio::time::timeout(Duration::from_secs_f64(l.queue_timeout.max(0.0)), run).await {
        Ok(r) => r,
        Err(_) => {
            state.record(
                &ctx,
                &key,
                Entry::new("tags", "", "error").detail("补全查询排队或请求超时"),
            );
            Err(GateError::with_code(
                429,
                "标签补全等待超时，请稍后重试。",
                reasons::QUEUE_TIMEOUT,
            )
            .into())
        }
    }
}

async fn suggest(
    state: &AppState,
    ctx: &ReqCtx,
    key: &Key,
    parts: &axum::http::request::Parts,
    body: Body,
) -> Result<Response, ApiError> {
    let (prompt, model) = if parts.method == Method::GET {
        // 重复的键以最后一个为准
        let Query(mut q) = Query::<HashMap<String, String>>::try_from_uri(&parts.uri)
            .unwrap_or_else(|_| Query(HashMap::new()));
        (
            q.remove("prompt").unwrap_or_default(),
            q.remove("model").unwrap_or_default(),
        )
    } else {
        let b = read_json(&parts.headers, body, 64 * 1024).await?;
        let s = |k: &str| match b.get(k) {
            Some(v) if enabled(v) => text(v),
            _ => String::new(),
        };
        (s("prompt"), s("model"))
    };
    let _slots = acquire_slots(state);
    check_image_cooldown(state)?;
    let url = reqwest::Url::parse_with_params(
        &format!("{}/ai/generate-image/suggest-tags", state.nai.image_host()),
        [("prompt", &prompt), ("model", &model)],
    )
    .map_err(|e| InternalError(e.to_string()))?;
    let mut opts = RequestOpts::get(url.to_string());
    opts.accept = "application/json";
    opts.image_lane = true;
    opts.wait_for_image_slot = false;
    let (s, c, k) = (state.clone(), ctx.clone(), key.clone());
    let cb = Callbacks {
        on_rate_limited: Some(Box::new(move |retry_after| {
            let (s, c, k) = (s.clone(), c.clone(), k.clone());
            Box::pin(async move {
                let secs = s.limits().image_429_cooldown_seconds.max(retry_after);
                let cooldown = s.block_image_generation(secs).await;
                s.record(
                    &c,
                    &k,
                    Entry::new("tags", "", "error").detail(format!(
                        "上游限流(429)：全站图片生成暂停约 {cooldown} 秒，保护上游 Token"
                    )),
                );
            })
        })),
        ..Default::default()
    };
    let resp = match state.nai.request(opts, &cb, None, ctx).await {
        Ok(r) => r,
        Err(e) if e.transport => return Err(upstream_transport(ctx, e.message)),
        Err(e) => {
            if e.status != 429 {
                // 真实上游 429 已由回调记录
                state.record(
                    ctx,
                    key,
                    Entry::new("tags", "", "error").detail(e.message.clone()),
                );
            }
            return Err(up_to_gate(e).into());
        }
    };
    if resp.status != 200 {
        state.record(
            ctx,
            key,
            Entry::new("tags", "", "error")
                .detail(format!("上游标签接口返回 HTTP {}", resp.status)),
        );
        return Err(
            GateError::new(resp.status, upstream_error_message(i64::from(resp.status)))
                .from_upstream(true)
                .into(),
        );
    }
    state.record(ctx, key, Entry::new("tags", "", "ok"));
    Ok(binary(resp.body, "application/json", false))
}
