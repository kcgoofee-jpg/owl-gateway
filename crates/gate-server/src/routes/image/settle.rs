//! 计费与结算：V5 额度确认后的重估（流式 / 非流式共用），非流式出图的转发、记账与上游失败的记录。

use super::params::Prepared;
use super::{
    before_dispatch, binary, detail160, local, on_429, on_dispatch, size_text, up_to_gate,
};
use crate::accounting::Entry;
use crate::admission::Est;
use crate::error::{ApiError, GateError, upstream_transport};
use crate::media::MAX_RESPONSE_BYTES;
use crate::reqctx::ReqCtx;
use crate::state::AppState;
use axum::response::Response;
use gate_core::policy;
use gate_core::pyc::take_chars;
use gate_core::pyval::{py_str, truthy};
use gate_core::reasons;
use gate_core::upstream_errors::upstream_error_message;
use gate_upstream::client::{Callbacks, RequestOpts, UpstreamError};
use serde_json::{Map, Value, json};
use std::sync::{Arc, Mutex};

/// V5 额度确认回调用到的可变状态。
pub(super) struct Billing {
    pub(super) est: Est,
    pub(super) detail: String,
    /// Anlas paid from the upstream account itself for this image (site-paid V5), not the member's.
    pub(super) site_paid: f64,
}

/// Site-paid V5 cost for one image on `token_id`, if that account has a live grant with budget left.
async fn site_paid_cost(
    state: &AppState,
    token_id: &str,
    body: &Value,
) -> Result<Option<f64>, UpstreamError> {
    use gate_core::paid_v5;
    let db_err = |_| {
        UpstreamError::local(
            500,
            "读取付费设置失败，请求未发出，未扣费。请稍后重试。",
            "",
        )
    };
    let raw = state.db.get_setting(paid_v5::KEY).await.map_err(db_err)?;
    let Some(grant) = paid_v5::grant(raw.as_deref(), token_id, state.now()) else {
        return Ok(None);
    };
    let cost = Est::from_cost(
        policy::estimate_image_cost(body, true, false)
            .map_err(|e| UpstreamError::local(400, e.message().to_string(), ""))?,
    )
    .anlas;
    // 预算之外还要看账号真实余额（最近一次订阅读数）：不够就不站付，免得上游回 402
    if state
        .allowance
        .anlas_of(token_id)
        .is_some_and(|b| (b as f64) < cost)
    {
        return Ok(None);
    }
    let spent = state
        .db
        .site_paid_spent(token_id, paid_v5::MARKER)
        .await
        .map_err(db_err)?;
    Ok(paid_v5::fits(grant, spent, cost).then_some(cost))
}

/// V5 只开放免费规格：要花成员 Anlas 的 V5（超尺寸、超步数、多张、局部重绘等）一律拒绝（站长 10-22）。
pub(super) const V5_PAID_REFUSED: &str =
    "V5 仅支持免费规格（单张、28 步以内、不超过 832×1216 面积），未扣费。";

/// 这张图如果全部按 Anlas 付的估价（选号 / V5 确认时和账号余额比）；估不出来按 0（不拦）。
pub(super) fn paid_cost(body: &Value) -> f64 {
    policy::estimate_image_cost(body, true, false)
        .map(|c| Est::from_cost(c).anlas)
        .unwrap_or(0.0)
}

pub(super) fn resolve_v5(
    state: &AppState,
    prep: &Prepared,
    billing: Arc<Mutex<Billing>>,
    reserved: Arc<crate::admission::Reserved>,
) -> gate_upstream::client::ResolveV5<'static> {
    let (body, legacy, notes, detail0) = (
        Arc::new(prep.body.clone()),
        prep.legacy_free_images,
        prep.notes.clone(),
        prep.detail.clone(),
    );
    let state = state.clone();
    Box::new(move |exhausted: bool, token_id: String, can_switch: bool| {
        let (body, notes, billing, reserved, state, detail0) = (
            body.clone(),
            notes.clone(),
            billing.clone(),
            reserved.clone(),
            state.clone(),
            detail0.clone(),
        );
        Box::pin(async move {
            // 每次确认都从头算：排队前后各确认一次，换号重试（上游 402）也会再来一次，
            // 上一次的「站付」结论不能留到这一次
            {
                let mut b = billing.lock().expect("billing");
                b.site_paid = 0.0;
                b.detail = detail0;
            }
            // 站付优先（账号自己的 Anlas，不论成员有没有 Anlas 权限）：先用掉快到期账号的 Anlas，
            // 别的账号的免费 V5 留着
            if exhausted && let Some(cost) = site_paid_cost(&state, &token_id, &body).await? {
                // The account pays from its own Anlas; the member's Anlas budget is untouched
                let mut b = billing.lock().expect("billing");
                b.site_paid = cost;
                b.detail = notes
                    .iter()
                    .cloned()
                    .chain([
                        size_text(&body),
                        format!(
                            "{} {}A（账号 Anlas 付费，不计入成员）",
                            gate_core::paid_v5::MARKER,
                            cost as i64
                        ),
                    ])
                    .collect::<Vec<_>>()
                    .join("; ");
                return Ok(false);
            }
            if can_switch {
                return Ok(true);
            }
            if exhausted {
                // 站长 10-22：V5 只用免费额度（或上面的站付）。账号的免费额度用完就直接拒，
                // 不自动转成扣成员的 Anlas（Anlas 只给 V4.5 大图和 Vibe 用）
                return Err(UpstreamError::local(
                    403,
                    "V5 暂停，未扣费。请使用 V4.5。",
                    reasons::V5_EXHAUSTED,
                ));
            }
            let cost = policy::estimate_image_cost(&body, true, true)
                .map_err(|e| UpstreamError::local(400, e.message().to_string(), ""))?;
            if cost.anlas > 0 {
                // 超出免费规格（大图、步数多、多张、局部重绘等）的 V5 要花成员的 Anlas：同样不开放
                return Err(UpstreamError::local(403, V5_PAID_REFUSED, reasons::PARAM));
            }
            let updated = Est::from_cost(cost);
            reserved.update(&updated, legacy).await.map_err(local)?;
            billing.lock().expect("billing").est = updated;
            Ok(false)
        })
    })
}

pub(super) async fn perform_generation(
    state: &AppState,
    ctx: &ReqCtx,
    prep: Prepared,
    reserved: Arc<crate::admission::Reserved>,
) -> Result<Response, ApiError> {
    let billing = Arc::new(Mutex::new(Billing {
        est: prep.est,
        detail: prep.detail.clone(),
        site_paid: 0.0,
    }));
    let (key, model) = (prep.key.clone(), prep.model.clone());
    let est0 = prep.est;
    let cb = Callbacks {
        on_rate_limited: Some(on_429(state)),
        resolve_v5_cost: (est0.v5 > 0)
            .then(|| resolve_v5(state, &prep, billing.clone(), reserved.clone())),
        before_dispatch: Some(before_dispatch(
            state,
            Some((key.str("discord_id").to_string(), est0.v5 > 0)),
        )),
        on_dispatch: Some(on_dispatch(state, ctx, reserved.clone())),
    };
    let url = format!("{}/ai/generate-image", state.nai.image_host());
    let mut opts = RequestOpts::post(url, &prep.body);
    opts.requires_anlas = est0.anlas > 0.0;
    opts.v5_free = est0.v5 > 0;
    opts.image_count = prep.image_count as usize;
    opts.image_lane = true;
    opts.max_response_bytes = Some(MAX_RESPONSE_BYTES);
    opts.queue_timeout = state.limits().queue_timeout;
    let allowance = state
        .allowance
        .with(&state.nai)
        .paid_cost(paid_cost(&prep.body));
    let allow: Option<&dyn gate_upstream::client::Allowance> = Some(&allowance);
    let res = state.nai.request(opts, &cb, allow, ctx).await;
    let est = billing.lock().expect("billing").est;
    let resp = match res {
        Ok(r) => r,
        Err(e) if e.transport => return Err(upstream_transport(ctx, e.message)),
        Err(e) => {
            let exc = up_to_gate(e);
            if !ctx.was_sent() && !exc.billing_uncertain {
                // 还没发到上游就被本地拒绝：记「拒绝」+ 原因码，不算上游失败
                state
                    .settle_record(
                        ctx,
                        &key,
                        Entry::new("image", &model, "rejected")
                            .detail(detail160(exc.status, &exc.message))
                            .reason(exc.code.clone()),
                    )
                    .await;
                return Err(exc.into());
            }
            state
                .settle_record(
                    ctx,
                    &key,
                    Entry::new("image", &model, "error")
                        .detail(exc.message.clone())
                        .unconfirmed(if exc.billing_uncertain {
                            est.anlas
                        } else {
                            0.0
                        })
                        .execution_unknown(
                            exc.billing_uncertain,
                            if exc.billing_uncertain {
                                est.anlas
                            } else {
                                0.0
                            },
                            if exc.billing_uncertain { est.v5 } else { 0 },
                            if exc.billing_uncertain {
                                prep.legacy_free_images
                            } else {
                                0
                            },
                        ),
                )
                .await;
            state
                .audit_generation(ctx, &key, "image", &model, "error", &prep.body, None)
                .await;
            state.record_upstream(false);
            return Err(exc.into());
        }
    };
    if !matches!(resp.status, 200 | 201) {
        let shape = if resp.status >= 500 {
            request_shape(&prep.body)
        } else {
            String::new()
        };
        if !shape.is_empty() {
            tracing::warn!(
                "[upstream-5xx] rid={} status={} {shape}",
                ctx.rid(),
                resp.status
            );
        }
        let detail = if shape.is_empty() {
            format!("upstream {}", resp.status)
        } else {
            format!("upstream {}（{}）", resp.status, take_chars(&shape, 110))
        };
        state
            .settle_record(
                ctx,
                &key,
                Entry::new("image", &model, "error")
                    .detail(detail)
                    .unconfirmed(if resp.status >= 500 { est.anlas } else { 0.0 })
                    .execution_unknown(
                        resp.status >= 500,
                        if resp.status >= 500 { est.anlas } else { 0.0 },
                        if resp.status >= 500 { est.v5 } else { 0 },
                        if resp.status >= 500 {
                            prep.legacy_free_images
                        } else {
                            0
                        },
                    ),
            )
            .await;
        state
            .audit_generation(ctx, &key, "image", &model, "error", &prep.body, None)
            .await;
        state.record_upstream(resp.status < 500);
        return Err(
            GateError::new(resp.status, upstream_error_message(i64::from(resp.status)))
                .from_upstream(true)
                .into(),
        );
    }
    let detail = billing.lock().expect("billing").detail.clone();
    let site_paid = billing.lock().expect("billing").site_paid;
    state
        .settle_record(
            ctx,
            &key,
            Entry {
                images: prep.image_count,
                anlas: est.anlas,
                v5: est.v5,
                legacy_free_images: prep.legacy_free_images,
                site_paid_anlas: site_paid,
                detail,
                ..Entry::new("image", &model, "ok")
            },
        )
        .await;
    state.schedule_audit(
        ctx,
        &key,
        "image",
        &model,
        "ok",
        prep.body.clone(),
        Some(resp.body.clone()),
    );
    state.record_upstream(true);
    let media = resp
        .headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/octet-stream")
        .to_string();
    Ok(binary(resp.body, &media, false))
}

/// 这次请求的形态摘要（只用我们自己发出的数据，绝不碰上游响应体）。上游 5xx 时记下，
/// 判断「这位成员的请求特征」还是「上游抽风」。
fn request_shape(body: &Value) -> String {
    let empty = Map::new();
    let p = body
        .get("parameters")
        .and_then(Value::as_object)
        .unwrap_or(&empty);
    let g = |k: &str| py_str(p.get(k).unwrap_or(&Value::Null));
    let flag = |k: &str| u8::from(p.get(k).is_some_and(truthy));
    let refs = match p.get("reference_image_multiple") {
        Some(Value::Array(a)) => a.len(),
        _ => 0,
    };
    let chars = p
        .get("v4_prompt")
        .and_then(|v| v.get("caption"))
        .and_then(|c| c.get("char_captions"))
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    let text_len = |v: Option<&Value>| match v {
        None | Some(Value::Null) => 0,
        Some(v) if !truthy(v) => 0,
        Some(v) => gate_core::pyc::len_chars(&py_str(v)),
    };
    let mut keys: Vec<&String> = p.keys().collect();
    keys.sort();
    let keys = keys
        .iter()
        .map(|k| k.as_str())
        .collect::<Vec<_>>()
        .join(",");
    let vibe = p.get("director_reference_images").is_some_and(truthy)
        || p.get("director_reference_images_cached")
            .is_some_and(truthy);
    format!(
        "{} · {}x{} · {}步 · n={} · action={} · prompt={}字 uc={}字 角色={} · sampler={} v={} sm={}/{} qt={} ucPreset={} · 图={} 蒙版={} 参考={} vibe={} 原图={} · keys={}",
        py_str(body.get("model").unwrap_or(&Value::String("?".into()))),
        g("width"),
        g("height"),
        g("steps"),
        py_str(p.get("n_samples").unwrap_or(&json!(1))),
        py_str(
            body.get("action")
                .unwrap_or(&Value::String("generate".into()))
        ),
        text_len(body.get("input")),
        text_len(p.get("negative_prompt")),
        chars,
        g("sampler"),
        g("params_version"),
        flag("sm"),
        flag("sm_dyn"),
        flag("qualityToggle"),
        g("ucPreset"),
        flag("image"),
        flag("mask"),
        refs,
        u8::from(vibe),
        flag("add_original_image"),
        take_chars(&keys, 220),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shape_summary() {
        let body = json!({"model": "nai-diffusion-4-5-full", "input": "猫猫", "parameters": {
            "width": 832, "height": 1216, "steps": 28, "sampler": "k_euler", "sm": true, "zeta": 1, "alpha": 2}});
        let s = request_shape(&body);
        assert!(s.starts_with("nai-diffusion-4-5-full · 832x1216 · 28步 · n=1 · action=generate · prompt=2字 uc=0字 角色=0"));
        assert!(s.ends_with("keys=alpha,height,sampler,sm,steps,width,zeta"));
        assert!(s.contains("sm=1/0 qt=0 ucPreset=None"));
    }
}
