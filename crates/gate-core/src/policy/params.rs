//! 参数规范化、钳制与守卫：宽高落在 `DIMENSION_STEP` 网格上、步数 / 采样器收敛、图生图与局部重绘的前置检查。

use super::*;

// ----------------------------------------------------------------------------- 小工具

pub(crate) fn attribute_error_get(v: &Value) -> ValidationError {
    ValidationError::InvalidStructure(format!(
        "expected an object, got {}",
        validation::type_name(v)
    ))
}

/// `payload.get("parameters", payload)` 再要求它是 dict（否则后面的 `.get` 抛 AttributeError）。
pub(crate) fn params_of(payload: &Value) -> ValidationResult<&Obj> {
    let obj = payload
        .as_object()
        .ok_or_else(|| attribute_error_get(payload))?;
    let p = obj.get("parameters").unwrap_or(payload);
    p.as_object().ok_or_else(|| attribute_error_get(p))
}

/// `str(d.get(key, "") or "")`。
pub(crate) fn str_or_empty(v: Option<&Value>) -> String {
    match v {
        Some(x) if values::enabled(x) => text(x),
        _ => String::new(),
    }
}

/// `str(payload.get("model", "")).strip().lower()`
pub(crate) fn model_key(payload: &Obj) -> String {
    primitives::strip(&str_of_get(payload, "model")).to_lowercase()
}

pub(crate) fn in_models(list: &[&str], model: &str) -> bool {
    list.contains(&model)
}

/// `isinstance(v, (int, float)) and not bool and 0 <= v <= 1 and math.isfinite(v)`
pub fn unit_value(v: &Value) -> bool {
    match v {
        Value::Number(n) => match validation::int_of_number(n) {
            Some(i) => (0..=1).contains(&i),
            None => n
                .as_f64()
                .is_some_and(|f| f.is_finite() && (0.0..=1.0).contains(&f)),
        },
        _ => false,
    }
}

pub(crate) fn unit_f64(v: &Value) -> f64 {
    v.as_f64().unwrap_or(0.0)
}

pub(crate) fn float_value(x: f64) -> Value {
    values::float(x)
}

pub(crate) fn empty_obj() -> &'static Obj {
    static E: OnceLock<Obj> = OnceLock::new();
    E.get_or_init(Obj::new)
}

pub(crate) fn dict_copy(payload: &Value) -> ValidationResult<Obj> {
    payload.as_object().cloned().ok_or_else(|| {
        ValidationError::InvalidType(format!(
            "'{}' object is not iterable",
            validation::type_name(payload)
        ))
    })
}

/// `dict(out["parameters"]) if isinstance(out.get("parameters"), dict) else {}`
pub(crate) fn params_copy(out: &Obj) -> Obj {
    match out.get("parameters") {
        Some(Value::Object(m)) => m.clone(),
        _ => Obj::new(),
    }
}

// ----------------------------------------------------------------------------- 规范化

/// 路由入口统一规范化：模型名小写去空格并回写；尺寸 / 步数 / 张数必须是整数。
///
/// 计价按 `int()` 截断而上游原样接收：放过 28.99 步或 1.99 张会让「免费」估价与实际扣费不一致。
/// 出错时入参可能已被部分改写（先写回 model、再逐项转换浮点整数）。
/// `randint` 是 `random.randint(0, 4294967295)` 的注入源（seed 超出 uint32 时重抽）。
pub fn normalize_image_request(
    body: &mut Value,
    randint: &mut dyn FnMut() -> u32,
) -> ValidationResult<()> {
    let body_type = validation::type_name(body);
    let Some(obj) = body.as_object_mut() else {
        return Err(ValidationError::InvalidStructure(format!(
            "'{body_type}' object has no attribute 'get'"
        )));
    };
    let model = match obj.get("model") {
        Some(Value::String(m)) if !primitives::strip(m).is_empty() => {
            primitives::strip(m).to_lowercase()
        }
        _ => return Err(ValidationError::value("缺少 model")),
    };
    obj.insert("model".into(), Value::String(model));
    let Some(Value::Object(p)) = obj.get_mut("parameters") else {
        return Ok(());
    };
    for name in INTEGER_IMAGE_PARAMS {
        let Some(mut value) = p.get(name).cloned() else {
            continue;
        };
        if value.is_null() {
            continue;
        }
        if let Some(f) = float_integer(&value) {
            // 整数值的浮点（5.0）转成 int；超出 u64 的天文数字后面必然报错（> 65536），JSON 也表示不了，保持原样
            if let Some(i) =
                f_to_i128(f).filter(|i| i64::try_from(*i).is_ok() || u64::try_from(*i).is_ok())
            {
                value = json_int(i);
                p.insert(name.into(), value.clone());
            }
        }
        let ok =
            !value.is_boolean() && strict_int(&value).is_some_and(|i| (0..=65536).contains(&i));
        if !ok {
            return Err(ValidationError::value(format!("{name} 必须是非负整数")));
        }
    }

    // seed 规范化：客户端经常传 -1 表示随机种子；上游只接受 0..4294967295 的 uint32
    if let Some(seed) = p.get("seed").cloned() {
        let mut seed_int: Option<i128> = None;
        let mut seed_val = seed;
        if let Some(f) = float_integer(&seed_val) {
            let i = f_to_i128(f).unwrap_or(if f < 0.0 { i128::MIN } else { i128::MAX });
            if let Ok(small) = i64::try_from(i) {
                p.insert("seed".into(), Value::from(small));
                seed_val = Value::from(small);
            }
            seed_int = Some(i);
        } else if let Some(i) = int_like(&seed_val) {
            // bool 也是 int：True / False 原样保留
            seed_int = Some(i);
        }
        match seed_int {
            Some(i) => {
                if !(0..=4_294_967_295).contains(&i) {
                    p.insert("seed".into(), Value::from(randint()));
                }
            }
            None => {
                if !seed_val.is_null() {
                    p.insert("seed".into(), Value::from(randint()));
                }
            }
        }
    }

    // scale 校验
    if let Some(scale) = p.get("scale")
        && !scale.is_null()
    {
        let ok = match scale {
            Value::Number(n) => n
                .as_f64()
                .is_some_and(|f| f.is_finite() && (0.0..=50.0).contains(&f)),
            _ => false,
        };
        if !ok {
            return Err(ValidationError::value(
                "scale 必须是 0 到 50 之间的有效数值",
            ));
        }
    }
    Ok(())
}

/// `isinstance(v, float) and v.is_integer()` → 浮点值。
pub(crate) fn float_integer(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) if n.is_f64() => n.as_f64().filter(|f| f.is_finite() && f.fract() == 0.0),
        _ => None,
    }
}

pub(crate) fn f_to_i128(f: f64) -> Option<i128> {
    if f.is_finite() && f.abs() < 1.0e38 {
        Some(f as i128)
    } else {
        None
    }
}

// ----------------------------------------------------------------------------- 图片钳制

/// 等比缩小到像素面积上限内，边长落在 [`DIMENSION_STEP`] 网格上。
pub fn fit_size(width: i128, height: i128, max_pixels: i128) -> ValidationResult<(i128, i128)> {
    let area = width.saturating_mul(height);
    if area <= max_pixels {
        return Ok((width, height));
    }
    if area == 0 {
        return Err(ValidationError::DivisionByZero("division by zero".into()));
    }
    let q = max_pixels as f64 / area as f64;
    if q < 0.0 {
        return Err(ValidationError::value("math domain error"));
    }
    let scale = q.sqrt();
    let step = i128::from(DIMENSION_STEP);
    let snap = |side: i128| -> i128 {
        std::cmp::max(
            step,
            i128::from(primitives::round0(side as f64 * scale / step as f64)) * step,
        )
    };
    let mut w2 = snap(width);
    let mut h2 = snap(height);
    // 取整到 `DIMENSION_STEP` 之后可能仍然略超上限：一档一档缩长的那一边。
    // 两边相等时同时缩——只缩一边会把 4096×4096 变成 1408×1472 这种歪掉的尺寸。
    while w2.saturating_mul(h2) > max_pixels && (w2 > step || h2 > step) {
        if w2 == h2 {
            if w2 <= step {
                break;
            }
            (w2, h2) = (w2 - step, h2 - step);
        } else if w2 > h2 {
            if w2 <= step {
                break;
            }
            w2 -= step;
        } else {
            if h2 <= step {
                break;
            }
            h2 -= step;
        }
    }
    Ok((w2, h2))
}

/// 把请求改写进「V5 额度条件 / 老模型免费档」的形状。返回 `(新 payload, 变更说明, 错误)`；
/// 错误非 None 表示该请求被拒绝（例如 img2img 未开放）。
pub fn clamp_image_params(
    payload: &Value,
    max_pixels: i64,
    max_steps: i64,
    allow_img2img: bool,
) -> ValidationResult<(Value, Vec<String>, Option<String>)> {
    let mut notes: Vec<String> = Vec::new();
    let mut out = dict_copy(payload)?;
    let mut p = params_copy(&out);
    // `out["parameters"] = p`：先占位，保证 JSON 键位置稳定
    out.insert("parameters".into(), Value::Null);

    macro_rules! finish {
        ($err:expr) => {{
            out.insert("parameters".into(), Value::Object(p));
            return Ok((Value::Object(out), notes, $err));
        }};
    }

    // img2img / inpaint 审查
    let is_img2img = enabled_opt(p.get("image")) || enabled_opt(p.get("mask"));
    if is_img2img && !allow_img2img {
        finish!(Some(
            "本站不支持图生图和局部重绘，未扣费。请在客户端中去掉原图。".into()
        ));
    }
    if i128::from(max_pixels) < i128::from(DIMENSION_STEP) * i128::from(DIMENSION_STEP) {
        finish!(Some(
            "站点尺寸上限配置异常（小于最小可出图尺寸），未扣费。请联系站长。".into()
        ));
    }

    // 批量张数 -> 1
    if int_or(p.get("n_samples"), 1, 1)? != 1 {
        p.insert("n_samples".into(), Value::from(1));
        notes.push("n_samples 已限制为 1，避免额外图片产生 Anlas".into());
    }

    // steps -> 上限
    if int_or(p.get("steps"), 0, 0)? > i128::from(max_steps) {
        p.insert("steps".into(), Value::from(max_steps));
        notes.push(format!("steps 已钳制到 {max_steps}"));
    }

    // 尺寸：两条规则，分开走，不叠着用。
    //
    // - V5：上游只认三个 Normal 预设。落在某个预设的框内就照原样发（文档口径是「之内」），
    //   超出框才按长宽比吸附到最接近的预设。
    // - V4.5 及以下：支持任意 `DIMENSION_STEP` 的倍数，所以只按面积等比缩小，**不吸附 V5 预设**——
    //   吸附会把 1920×1080 变成 1216×832、把 65536×64 变成 1216×832，那是换了个构图，不是缩小。
    let w = int_or(p.get("width"), 0, 0)?;
    let h = int_or(p.get("height"), 0, 0)?;
    if w > 0 && h > 0 {
        let (pw, ph) = if is_v5_model(&str_of_get(&out, "model")) {
            let inside = V5_NORMAL_PRESETS
                .iter()
                .any(|&(pw, ph)| w <= i128::from(pw) && h <= i128::from(ph));
            let (pw, ph) = if inside { (w, h) } else { snap_v5_preset(w, h) };
            // 预设本身都在面积上限以内，这一步只是兜底
            fit_size(pw, ph, i128::from(max_pixels))?
        } else {
            fit_size(w, h, i128::from(max_pixels))?
        };
        if (w, h) != (pw, ph) {
            p.insert("width".into(), json_int(pw));
            p.insert("height".into(), json_int(ph));
            notes.push(format!("分辨率 {w}x{h} 已按安全钳制调整为 {pw}x{ph}"));
        }
    }

    // 沿用免费 Key 的保守钳制；SMEA 本身不排除首张减免资格。
    if enabled_opt(p.get("sm")) || enabled_opt(p.get("sm_dyn")) {
        p.insert("sm".into(), Value::Bool(false));
        p.insert("sm_dyn".into(), Value::Bool(false));
        notes.push("SMEA 已按安全钳制规则关闭".into());
    }

    // ControlNet / 角色参考（额外计费）
    if enabled_opt(p.get("controlnet_model")) || enabled_opt(p.get("controlnet_condition")) {
        finish!(Some(
            "本站未开放 ControlNet，未扣费。请在客户端中关闭 ControlNet。".into()
        ));
    }
    if enabled_opt(p.get("characterReferences")) {
        values::pop(&mut p, "characterReferences");
        notes.push("已移除角色参考（精确参考会消耗 Anlas）".into());
    }
    finish!(None)
}

// ----------------------------------------------------------------------------- 图生图 / 局部重绘守卫

/// 图生图 / 局部重绘的两条上游地雷（2026-10-13 逐组合实测）：
/// 1. `action=infill` 不带 `parameters.mask` → 上游 500；
/// 2. `action=infill` 或 `img2img` 带 `sm` / `sm_dyn`（SMEA）→ 上游 500，这里照官方前端置为 false。
///
/// 普通出图（`action=generate`）不受影响。
pub fn guard_image_action(body: &Value) -> (Value, Vec<String>, Option<String>) {
    let mut out = body.as_object().cloned().unwrap_or_default();
    let mut p = params_copy(&out);
    out.insert("parameters".into(), Value::Null);
    let mut notes = Vec::new();
    let action = primitives::strip(&str_or_empty(out.get("action"))).to_lowercase();
    let mut err = None;
    if action == "infill" || action == "img2img" {
        if action == "infill" && !enabled_opt(p.get("mask")) {
            err = Some(
                "局部重绘缺少蒙版，未扣费。请在客户端中绘制蒙版后重新提交。给客户端作者：infill 须带 parameters.mask"
                    .to_string(),
            );
        } else if enabled_opt(p.get("sm")) || enabled_opt(p.get("sm_dyn")) {
            p.insert("sm".into(), Value::Bool(false));
            p.insert("sm_dyn".into(), Value::Bool(false));
            notes.push(
                "局部重绘 / 图生图不接受 SMEA，已按官方前端置为 false（否则上游 500）".into(),
            );
        }
    }
    out.insert("parameters".into(), Value::Object(p));
    (Value::Object(out), notes, err)
}

// ----------------------------------------------------------------------------- 工具

pub fn mask_token(token: &str, keep: usize) -> String {
    let len = primitives::len_chars(token);
    if len <= keep {
        return "*".repeat(len);
    }
    format!(
        "{}…{}",
        primitives::take_chars(token, keep),
        primitives::last_chars(token, 4)
    )
}

/// `f"{prefix}-{secrets.token_urlsafe(24)}"`：24 个随机字节由调用方提供（CSPRNG），
/// base64url 无填充编码成 32 个字符。
pub fn gen_key(prefix: &str, random: &[u8; 24]) -> String {
    use base64::Engine;
    format!(
        "{prefix}-{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(random)
    )
}
