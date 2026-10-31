//! 生图参数：校验与整理、免费档钳制、费用预估。

use super::size_text;
use crate::accounting::Entry;
use crate::admission::{self, Est};
use crate::auth::require_feature;
use crate::error::{ApiError, GateError, InternalError};
use crate::key::Key;
use crate::reqctx::ReqCtx;
use crate::state::AppState;
use gate_core::policy::{self, REFERENCE_FIELDS};
use gate_core::primitives::take_chars;
use gate_core::reasons;
use gate_core::values::{enabled, text};
use serde_json::{Map, Value, json};

/// 回给「连接测试」请求的说明（Key 有效，但这类请求不出图）。
pub(super) const CONNECTION_TEST_MESSAGE: &str =
    "连接正常，Key 有效。本站不处理连接测试请求，以免占用共享额度。请使用正常参数生图。";

/// 生图请求经过校验、整理后的结果。
pub(super) struct Prepared {
    pub(super) key: Key,
    pub(super) body: Value,
    pub(super) model: String,
    pub(super) image_count: i64,
    pub(super) est: Est,
    pub(super) legacy_free_images: i64,
    pub(super) notes: Vec<String>,
    pub(super) detail: String,
}

/// 参数校验与整理（发往上游前的最后一道额度检查也在这里）。
pub(super) async fn prepare(
    state: &AppState,
    ctx: &ReqCtx,
    key: Key,
    body: Map<String, Value>,
) -> Result<Prepared, ApiError> {
    let mut body = Value::Object(body);
    let model_str = |b: &Value| match b.get("model") {
        Some(v) if enabled(v) => text(v),
        _ => String::new(),
    };
    ctx.set_model(&model_str(&body));
    let reject = |model: &str, detail: &str| {
        state.record(
            ctx,
            &key,
            Entry::new("image", model, "rejected").detail(detail),
        );
    };
    if !body.get("parameters").is_some_and(Value::is_object) {
        return Err(GateError::with_code(400, "缺少生图参数，未扣费。请更新或更换客户端。给客户端作者：parameters 缺失或不是 JSON 对象", reasons::PARAM).into());
    }
    match policy::normalize_image_request(&mut body, &mut || rand::random::<u32>()) {
        Ok(()) => {}
        Err(gate_core::validation::ValidationError::InvalidValue(m)) => {
            let model = text(body.get("model").unwrap_or(&Value::String("?".into())));
            reject(take_chars(&model, 80), &m);
            return Err(GateError::with_code(
                400,
                format!("生图参数有误，未扣费。请检查客户端中的模型和参数设置。给客户端作者：{m}"),
                reasons::PARAM,
            )
            .into());
        }
        Err(e) => return Err(InternalError(e.to_string()).into()),
    }
    let mut model = text(body.get("model").unwrap_or(&Value::Null));
    if let Err(problem) = policy::convert_legacy_references(&mut body) {
        reject(&model, take_chars(&problem, 120));
        return Err(GateError::with_code(400, problem, reasons::PARAM).into());
    }
    policy::fix_name_digit_weights(&mut body); // bm94199:: → bm94199 ::（否则上游当成权重 → NaN）
    policy::sanitize_and_translate_prompts(&mut body); // WebUI 语法 (tag:1.2) → 1.2::tag::
    if let Some(problem) = policy::upstream_parameter_problem(&body) {
        reject(&model, take_chars(&problem, 120));
        return Err(GateError::with_code(400, problem, reasons::PARAM).into());
    }
    let params_has = |b: &Value, names: &[&str]| {
        names.iter().any(|n| {
            b.get("parameters")
                .and_then(|p| p.get(*n))
                .is_some_and(enabled)
        })
    };
    if params_has(&body, &REFERENCE_FIELDS) {
        require_feature(state, &key, "vibe").await?; // 参考图 / Vibe 经由 generate-image 传入时同样受功能开关约束
    }
    let Some(tier) = policy::image_model_tier(&model) else {
        reject(&model, "未列入本站图片模型白名单");
        return Err(GateError::with_code(
            400,
            "本站不支持该模型，未扣费。请改用本站支持的模型，如 nai-diffusion-4-5-full。",
            reasons::PARAM,
        )
        .into());
    };
    if tier == "v5" && !key.policy.v5 {
        reject(&model, "模型权限：仅允许 V4.5 及更低");
        return Err(GateError::with_code(
            403,
            "你的 Key 仅可使用 V4.5 及以下模型，未扣费。请切换到 V4.5。",
            reasons::FEATURE_OFF,
        )
        .into());
    }
    // 客户端的「连接测试」（小图、几步、短提示词）：Key 已验证有效，在这里直接答复，不出图、不占额度 / 队列 / 上游。
    // 拦截会经错误路径以 reason=test_request 落库，可审计。
    if policy::is_connection_test(&body) {
        return Err(
            GateError::with_code(400, CONNECTION_TEST_MESSAGE, reasons::TEST_REQUEST).into(),
        );
    }
    // 「四无」裸空请求（正文 0 字、负面 0 字、无角色参考、无 Vibe / 图生图材料）：上游对空提示词
    // 直接 500（10-30 两次线上实锤），在网关挡下来省一次排队和上游调用；带任一材料的放行。
    if policy::is_blank_generation(&body) {
        reject(&model, "提示词全空（正文/负面 0 字，无角色参考、无 Vibe）");
        return Err(GateError::with_code(
            400,
            "提示词不能全空：正文和负面提示词都是 0 字，也没有角色参考或 Vibe。NovelAI 会直接拒绝空提示词，写几个词再生成吧。",
            reasons::PARAM,
        )
        .into());
    }
    if body.get("image").is_some_and(enabled) || body.get("mask").is_some_and(enabled) {
        return Err(GateError::with_code(400, "图生图请求格式错误，未扣费。请更新或更换客户端。给客户端作者：image 和 mask 要放在 parameters 里", reasons::PARAM).into());
    }
    // 官网页面第二次用同一张参考图时只发 cache_secret_key；把数据从进程内缓存补回来再校验
    crate::ref_cache::expand(&mut body, key.id());
    crate::ref_cache::absorb(&body, key.id());
    if let Some(problem) = policy::validate_image_references(&body) {
        return Err(GateError::with_code(400, problem, reasons::PARAM).into());
    }
    if params_has(&body, &["image", "mask"]) && !key.policy.img2img {
        reject(&model, "img2img 未开放");
        return Err(GateError::with_code(
            400,
            "本站不支持图生图和局部重绘，未扣费。请在客户端中去掉原图。",
            reasons::PARAM,
        )
        .into());
    }
    // 未开放高规格生图时钳制到免费规格；费用仍由个人与全站预算共同限制。
    // 上限写死（与 policy 的免费条件、SYSTEM-FACTS 的文案同一口径），不再读环境变量
    let mut notes: Vec<String> = Vec::new();
    if !key.allows_paid_parameters() {
        let (b, n, problem) = policy::clamp_image_params(
            &body,
            policy::FREE_MAX_PIXELS,
            policy::FREE_MAX_STEPS,
            true,
        )
        .map_err(|e| validation_400(&e, "生图参数有误，未扣费。请检查客户端中的参数设置。"))?;
        if let Some(problem) = problem {
            reject(&model, &problem);
            return Err(GateError::with_code(400, problem, reasons::PARAM).into());
        }
        body = b;
        notes = n;
    }
    let (b, more) = policy::fill_v4_prompt(&body); // V4 / V4.5 缺结构化提示词时上游回 500：照官方网页端补上
    body = b;
    notes.extend(more);
    if tier == "v5" {
        // 站长临时降档：V5 Full（含局部重绘）改走 Medium，放在整理参数和费用估算之前，这样价格和免费额度都按 Medium 算。
        if let Some(medium) = policy::v5_medium_of(&text(body.get("model").unwrap_or(&Value::Null)))
            && crate::v5_medium::active(state).await
        {
            body["model"] = Value::String(medium.into());
            notes.push("站长已临时切到 V5 Medium".to_string());
        }
        if policy::is_v5_medium(&text(body.get("model").unwrap_or(&Value::Null))) {
            let (b, more) = policy::medium_normalize(&body);
            body = b;
            notes.extend(more);
        }
        model = text(body.get("model").unwrap_or(&Value::Null));
    }
    // 局部重绘缺蒙版 / 图生图带 SMEA → 上游只会回 500：放在费用估算之前，坏请求不占额度
    let (b, more, problem) = policy::guard_image_action(&body);
    if let Some(problem) = problem {
        reject(&model, &problem);
        return Err(GateError::with_code(400, problem, reasons::PARAM).into());
    }
    body = b;
    notes.extend(more);
    let p = body.get("parameters").cloned().unwrap_or(Value::Null);
    let n = p
        .get("n_samples")
        .filter(|v| enabled(v))
        .cloned()
        .unwrap_or(json!(1));
    let image_count = integer(&n).ok_or_else(|| {
        GateError::with_code(
            400,
            "生成张数设置无效，未扣费。请改为 1 张。给客户端作者：n_samples 须为正整数",
            reasons::PARAM,
        )
    })?;
    if image_count < 1 {
        return Err(GateError::with_code(
            400,
            "生成张数设置无效，未扣费。请改为 1 张。给客户端作者：n_samples 须为正整数",
            reasons::PARAM,
        )
        .into());
    }
    // 2.x 这里还有「V5 续杯」：`anlas_auto` 的 Key 当天个人 V5 用完后改用 Anlas 生成同规格 V5。
    // Anlas 自动分配早已下线、没有 Key 再是 anlas_auto，3.0 删除（列保留）
    let mut est = Est::from_cost(policy::estimate_image_cost(&body, true, true).map_err(|e| {
        validation_400(
            &e,
            "生图参数有误，无法计算费用，未扣费。请检查客户端中的参数设置。",
        )
    })?);
    // 站长 10-22：V5 不再花成员的 Anlas。单张超规格时估价里根本没有免费额度（v5 = 0），
    // 不会走到 V5 额度确认那一步，所以在入口就拒；多张等情况在确认时也会再拦一次
    if tier == "v5" && est.anlas > 0.0 {
        reject(&model, "V5 超出免费规格");
        return Err(
            GateError::with_code(403, super::settle::V5_PAID_REFUSED, reasons::PARAM).into(),
        );
    }
    let legacy_free_images = if tier == "legacy"
        && policy::opus_free_eligible(&body).map_err(|e| InternalError(e.to_string()))?
    {
        1
    } else {
        0
    };
    if params_has(&body, &REFERENCE_FIELDS) {
        est.is_vibe = true;
    }
    // 入口预检对所有请求都做（含 V5）：用快照、算上别的请求已拿到的预留，额度不够就立即 403，
    // 不让成员先排队 / 先在按时段放行里等位，轮到了才被拒（10-21：同 Key 第二张 V5 排了 47 秒才拿到 402）。
    // 以前只对非 V5 请求做（给已删除的「V5 续杯」留的口子），现在 V5 也一视同仁。
    let snapshot = admission::budget_snapshot(state).await;
    admission::quota_image_check(state, &snapshot, &key, &est, legacy_free_images, None).await?;
    let cost: Vec<String> = [
        (est.anlas > 0.0).then(|| format!("est={}A", est.anlas as i64)),
        (est.v5 > 0).then(|| format!("V5额度+{}", est.v5)),
    ]
    .into_iter()
    .flatten()
    .collect();
    let cost = if cost.is_empty() {
        "免费".to_string()
    } else {
        cost.join("; ")
    };
    // 尺寸 / 步数 / 费用始终在前：钳制说明只追加在后面，不能顶掉记账信息
    let detail = std::iter::once(format!("{} {cost}", size_text(&body)))
        .chain(notes.iter().cloned())
        .collect::<Vec<_>>()
        .join("; ");
    Ok(Prepared {
        key,
        body,
        model,
        image_count,
        est,
        legacy_free_images,
        notes,
        detail,
    })
}

/// Sample counts are JSON integers.
fn integer(v: &Value) -> Option<i64> {
    v.as_i64()
}

fn validation_400(
    _error: &gate_core::validation::ValidationError,
    msg: impl Into<String>,
) -> ApiError {
    GateError::with_code(400, msg, reasons::PARAM).into()
}
