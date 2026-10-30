//! 模型判定与计费：V5 / V4 分层、每张图的 Anlas 与步数上限。

use super::*;

// ----------------------------------------------------------------------------- 模型判定

pub fn is_v5_model(model: &str) -> bool {
    let m = primitives::strip(model).to_lowercase();
    m == "nai-diffusion-5"
        || m == "nai-v5"
        || m.starts_with("nai-diffusion-5-")
        || m.starts_with("nai-v5-")
}

/// 返回 `"legacy"` / `"v5"`；未列入白名单的模型返回 None。
pub fn image_model_tier(model: &str) -> Option<&'static str> {
    let m = primitives::strip(model).to_lowercase();
    if is_v5_model(&m) {
        return Some("v5");
    }
    if in_models(&LEGACY_IMAGE_MODEL_EXACT, &m)
        || LEGACY_IMAGE_MODEL_PREFIXES
            .iter()
            .any(|prefix| m == *prefix || m.starts_with(&format!("{prefix}-")))
    {
        return Some("legacy");
    }
    None
}

// ----------------------------------------------------------------------------- 图片计费

pub(crate) fn checked_ceil(x: f64) -> ValidationResult<f64> {
    primitives::int_checked(x)?;
    Ok(x.ceil() + 0.0)
}

/// V3+ 通用单张价格（Anlas）。1024x1024/28steps -> 20A。
pub fn per_image_cost(
    width: i128,
    height: i128,
    steps: i128,
    smea: bool,
    smea_dyn: bool,
) -> ValidationResult<f64> {
    let r = width.saturating_mul(height) as f64;
    let smea_factor = if smea && smea_dyn {
        1.4
    } else if smea {
        1.2
    } else {
        1.0
    };
    let base = 2.951823174884865e-6 * r + 5.753298233447344e-7 * r * (steps as f64);
    // 模型倍率、SMEA 和强度之后才做最终取整，避免多次取整累积误差。
    Ok(checked_ceil(base)? * smea_factor)
}

pub fn has_paid_extras(params: &Value) -> ValidationResult<bool> {
    let p = params_of(params)?;
    if enabled_opt(p.get("controlnet_model")) || enabled_opt(p.get("controlnet_condition")) {
        return Ok(true);
    }
    // V4/V5 角色参考 / 精确参考按张额外收费
    if enabled_opt(p.get("characterReferences")) {
        return Ok(true);
    }
    Ok(reference_surcharge(params)? != 0)
}

/// 免费出图的步数上限（上游 Opus 免费条件；也是本站免费档钳制的上限）。
pub const FREE_MAX_STEPS: i64 = 28;
/// 免费出图的面积上限 1024×1024（同上）。
pub const FREE_MAX_PIXELS: i64 = 1024 * 1024;

/// 首张是否符合免费条件；批次内其余图片仍按张计费。
pub fn opus_free_eligible(params: &Value) -> ValidationResult<bool> {
    let p = params_of(params)?;
    if has_paid_extras(params)? {
        return Ok(false);
    }
    if int_or(p.get("steps"), 28, 0)? > i128::from(FREE_MAX_STEPS) {
        return Ok(false);
    }
    let w = int_or(p.get("width"), 0, 0)?;
    let h = int_or(p.get("height"), 0, 0)?;
    if w <= 0 || h <= 0 || w.saturating_mul(h) > i128::from(FREE_MAX_PIXELS) {
        return Ok(false);
    }
    Ok(true)
}

/// 按长宽比吸附到最近的 V5 Normal 预设。
pub fn snap_v5_preset(width: i128, height: i128) -> (i128, i128) {
    let ar = width as f64 / std::cmp::max(1, height) as f64;
    if ar > 1.1 {
        (1216, 832)
    } else if ar < 0.9 {
        (832, 1216)
    } else {
        (1024, 1024)
    }
}

/// `estimate_image_cost` 的返回：`anlas` 扣多少 Anlas，`v5` 占多少个 V5 额度单位。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImageCost {
    pub anlas: i64,
    pub v5: i64,
}

pub(crate) fn sat_i64(i: i128) -> i64 {
    i64::try_from(i).unwrap_or(if i < 0 { i64::MIN } else { i64::MAX })
}

/// 估算一次 `/ai/generate-image` 的消耗。V5 多图批次可同时占首张额度并支付其余图片的 Anlas。
///
pub fn estimate_image_cost(
    params: &Value,
    is_opus: bool,
    v5_allowance_available: bool,
) -> ValidationResult<ImageCost> {
    if let Some(problem) = validate_image_references(params) {
        return Err(ValidationError::InvalidValue(problem));
    }
    let p = params_of(params)?;
    let w = int_or(p.get("width"), 832, 832)?;
    let h = int_or(p.get("height"), 1216, 1216)?;
    let steps = int_or(p.get("steps"), 28, 28)?;
    let smea = enabled_opt(p.get("sm"));
    let smea_dyn = enabled_opt(p.get("sm_dyn"));
    let n = std::cmp::max(1, int_or(p.get("n_samples"), 1, 1)?);

    let mut per = per_image_cost(w, h, steps, smea, smea_dyn)?;
    let model = str_of_get(params.as_object().unwrap_or(empty_obj()), "model");
    let is_v5 = is_v5_model(&model);
    if is_v5 {
        per *= V5_COST_MULTIPLIER;
    }
    if enabled_opt(p.get("mask")) {
        // 局部重绘使用 img2img.strength。
        let inpaint = match p.get("img2img") {
            Some(x) if values::enabled(x) => x,
            _ => &Value::Null,
        };
        let inpaint_obj = match inpaint {
            Value::Object(m) => m,
            Value::Null => empty_obj(),
            _ => return Err(ValidationError::value("img2img 必须是 JSON 对象")),
        };
        let strength = inpaint_obj
            .get("strength")
            .cloned()
            .unwrap_or_else(|| float_value(1.0));
        if !unit_value(&strength) {
            return Err(ValidationError::value(
                "img2img.strength 必须是 0 到 1 的有限数值",
            ));
        }
        let outer = p
            .get("strength")
            .cloned()
            .unwrap_or_else(|| float_value(0.0));
        if !unit_value(&outer) {
            return Err(ValidationError::value("strength 必须是 0 到 1 的有限数值"));
        }
        // 上游可能按任一字段计价，取较大者
        let strength = f64::max(unit_f64(&strength), unit_f64(&outer));
        // V4 按强度折算；2026-09-24 实测 V5 Full 重绘使用完整基础价。
        if model.to_lowercase().starts_with("nai-diffusion-4") {
            per *= strength;
        }
    } else if enabled_opt(p.get("image")) {
        let strength = p
            .get("strength")
            .cloned()
            .unwrap_or_else(|| float_value(1.0));
        if !unit_value(&strength) {
            return Err(ValidationError::value("strength 必须是 0 到 1 的有限数值"));
        }
        per *= unit_f64(&strength);
    }
    let per = std::cmp::max(2, checked_ceil(per)? as i128);

    // 先排除参考附加费，判断基础生成是否满足 Opus 首张减免条件。
    let base_p: Obj = p
        .iter()
        .filter(|(name, _)| !REFERENCE_FIELDS.contains(&name.as_str()))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    let base_params = match params.as_object() {
        Some(root) if root.contains_key("parameters") => {
            let mut b = root.clone();
            b.insert("parameters".into(), Value::Object(base_p));
            Value::Object(b)
        }
        _ => Value::Object(base_p),
    };
    let shaped = opus_free_eligible(&base_params)?;

    if is_v5 {
        let free_first = is_opus && v5_allowance_available && opus_free_eligible(params)?;
        let free = i128::from(free_first);
        return Ok(ImageCost {
            anlas: sat_i64(per.saturating_mul(n - free)),
            v5: free as i64,
        });
    }

    let paid_outputs = n - i128::from(is_opus && shaped);
    // 官方余额实测：符合条件的多图首张参考费减免，单张照收。
    let reference_cost = i128::from(reference_surcharge(params)?) * std::cmp::max(1, paid_outputs);
    Ok(ImageCost {
        anlas: sat_i64(
            per.saturating_mul(paid_outputs)
                .saturating_add(reference_cost),
        ),
        v5: 0,
    })
}
