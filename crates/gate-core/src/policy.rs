//! Image model policy, request normalization, reference validation and cost estimation.
//! Functions use JSON values and native validation errors; random values are supplied by callers.

use crate::validation::{
    self, Obj, ValidationError, ValidationResult, enabled_opt, int_like, int_or, json_int,
    len_or_empty, nd_class, num_eq_int, space_class, str_of_get, strict_int, text,
};
use crate::{primitives, values};
use fancy_regex::Regex as FancyRegex;
use regex::Regex;
use serde_json::Value;
use std::sync::OnceLock;

// ----------------------------------------------------------------------------- 常量

pub const V5_COST_MULTIPLIER: f64 = 1.5;

/// 仅允许明确认识的图片模型族；未知模型不能当旧模型，否则上游新增模型会绕过 V5 / Anlas 保护。
pub const LEGACY_IMAGE_MODEL_PREFIXES: [&str; 5] = [
    "nai-diffusion-4-5",
    "nai-diffusion-4",
    "nai-diffusion-3",
    "nai-diffusion-2",
    "nai-diffusion-furry-3",
];
pub const LEGACY_IMAGE_MODEL_EXACT: [&str; 3] =
    ["safe-diffusion", "nai-diffusion", "nai-diffusion-furry"];

/// 免费 Key 按预设钳制尺寸。
pub const V5_NORMAL_PRESETS: [(i64, i64); 3] = [(832, 1216), (1216, 832), (1024, 1024)];

/// V3 传原图，V4 / V4.5 传预编码 Vibe，V5 两者都没有。
pub const VIBE_RAW_MODELS: [&str; 4] = [
    "nai-diffusion-3",
    "nai-diffusion-3-inpainting",
    "nai-diffusion-furry-3",
    "nai-diffusion-furry-3-inpainting",
];
pub const VIBE_ENCODED_MODELS: [&str; 11] = [
    "nai-diffusion-4",
    "nai-diffusion-4-full",
    "nai-diffusion-4-full-inpainting",
    "nai-diffusion-4-curated",
    "nai-diffusion-4-curated-preview",
    "nai-diffusion-4-curated-inpainting",
    "nai-diffusion-4-5",
    "nai-diffusion-4-5-full",
    "nai-diffusion-4-5-full-inpainting",
    "nai-diffusion-4-5-curated",
    "nai-diffusion-4-5-curated-inpainting",
];
/// `{m for m in VIBE_ENCODED_MODELS if m.startswith("nai-diffusion-4-5")}`
pub const PRECISE_REFERENCE_MODELS: [&str; 5] = [
    "nai-diffusion-4-5",
    "nai-diffusion-4-5-full",
    "nai-diffusion-4-5-full-inpainting",
    "nai-diffusion-4-5-curated",
    "nai-diffusion-4-5-curated-inpainting",
];
pub const REFERENCE_LIMIT: usize = 16;
pub const VIBE_ENCODING_ANLAS: i64 = 2;
pub const REFERENCE_FIELDS: [&str; 9] = [
    "reference_image_multiple",
    "reference_image_multiple_cached",
    "reference_information_extracted_multiple",
    "reference_strength_multiple",
    "director_reference_images_cached",
    "director_reference_descriptions",
    "director_reference_information_extracted",
    "director_reference_strength_values",
    "director_reference_secondary_strength_values",
];

pub const INTEGER_IMAGE_PARAMS: [&str; 4] = ["width", "height", "steps", "n_samples"];

/// 上游实测限制：提示词上限 51200（按 UTF-8 字节）、文生图宽高必须是 64 的整数倍、
/// V4 / V4.5 / V5 用下列采样器上游直接 500。
pub const MAX_PROMPT_BYTES: usize = 51200;
pub const DIMENSION_STEP: i64 = 64;
pub const V4_V5_REJECTED_SAMPLERS: [&str; 8] = [
    "plms",
    "ddim",
    "ddim_v3",
    "k_dpm_adaptive",
    "k_dpm_fast",
    "k_dpmpp_3m_sde",
    "nai_smea",
    "nai_smea_dyn",
];
/// 只有 V5 不支持的采样器（V4.5 能用）：线上 V5 配 k_dpm_2 每次都是上游 500。
pub const V5_REJECTED_SAMPLERS: [&str; 1] = ["k_dpm_2"];
/// NovelAI V4 / V4.5 角色提示词上限。
pub const MAX_CHARACTERS: usize = 6;
/// NovelAI V5 Full / Curated 角色提示词上限。
pub const MAX_V5_CHARACTERS: usize = 22;

/// V5 Medium 档（蒸馏模型，锁 14 步 / Euler-a / Heavy 负面预设）。
pub const V5_MEDIUM_OF: [(&str, &str); 2] = [
    ("nai-diffusion-5-full", "nai-diffusion-5-full-medium"),
    (
        "nai-diffusion-5-full-inpainting",
        "nai-diffusion-5-full-medium-inpainting",
    ),
];
pub const V5_MEDIUM_STEPS: i64 = 14;
pub const V5_MEDIUM_SAMPLER: &str = "k_euler_ancestral";
pub const V5_MEDIUM_UC_PRESET: i64 = 0;
pub const V5_MEDIUM_UC: &str = "lowres, artistic error, film grain, scan artifacts, worst quality, bad quality, jpeg artifacts, \
very displeasing, chromatic aberration, dithering, halftone, screentone, multiple views, logo, \
too many watermarks, negative space, blank page";

const PNG_MAGIC: &[u8] = b"\x89PNG\r\n\x1a\n";

// ----------------------------------------------------------------------------- 小工具

fn attribute_error_get(v: &Value) -> ValidationError {
    ValidationError::InvalidStructure(format!(
        "expected an object, got {}",
        validation::type_name(v)
    ))
}

/// `payload.get("parameters", payload)` 再要求它是 dict（否则后面的 `.get` 抛 AttributeError）。
fn params_of(payload: &Value) -> ValidationResult<&Obj> {
    let obj = payload
        .as_object()
        .ok_or_else(|| attribute_error_get(payload))?;
    let p = obj.get("parameters").unwrap_or(payload);
    p.as_object().ok_or_else(|| attribute_error_get(p))
}

/// `str(d.get(key, "") or "")`。
fn str_or_empty(v: Option<&Value>) -> String {
    match v {
        Some(x) if values::enabled(x) => text(x),
        _ => String::new(),
    }
}

/// `str(payload.get("model", "")).strip().lower()`
fn model_key(payload: &Obj) -> String {
    primitives::strip(&str_of_get(payload, "model")).to_lowercase()
}

fn in_models(list: &[&str], model: &str) -> bool {
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

fn unit_f64(v: &Value) -> f64 {
    v.as_f64().unwrap_or(0.0)
}

fn float_value(x: f64) -> Value {
    values::float(x)
}

fn empty_obj() -> &'static Obj {
    static E: OnceLock<Obj> = OnceLock::new();
    E.get_or_init(Obj::new)
}

fn dict_copy(payload: &Value) -> ValidationResult<Obj> {
    payload.as_object().cloned().ok_or_else(|| {
        ValidationError::InvalidType(format!(
            "'{}' object is not iterable",
            validation::type_name(payload)
        ))
    })
}

/// `dict(out["parameters"]) if isinstance(out.get("parameters"), dict) else {}`
fn params_copy(out: &Obj) -> Obj {
    match out.get("parameters") {
        Some(Value::Object(m)) => m.clone(),
        _ => Obj::new(),
    }
}

// ----------------------------------------------------------------------------- 引用校验

/// Decode standard padded Base64 and retain only the requested prefix.
pub(crate) fn strict_b64(s: &str, keep: usize) -> Option<(Vec<u8>, usize)> {
    use base64::Engine;
    let mut bytes = base64::engine::general_purpose::STANDARD.decode(s).ok()?;
    let size = bytes.len();
    bytes.truncate(keep);
    Some((bytes, size))
}

pub fn base64_data(value: Option<&Value>, source_image: bool, png: bool) -> bool {
    let Some(Value::String(s)) = value else {
        return false;
    };
    // `len(value) > 25 MiB`（码点数）；非 ASCII 的串反正会在 ASCII 编码处失败，所以直接看字节数
    if s.is_empty() || s.len() > 25 * 1024 * 1024 {
        return false;
    }
    let Some((head, total)) = strict_b64(s, 12) else {
        return false;
    };
    if png {
        return head.starts_with(PNG_MAGIC);
    }
    if source_image {
        return head.starts_with(PNG_MAGIC)
            || head.starts_with(b"\xff\xd8\xff")
            || (head.starts_with(b"RIFF") && head.get(8..12) == Some(b"WEBP".as_slice()));
    }
    total > 0
}

/// `_reference_list(p, name)`
pub fn reference_list<'a>(p: &'a Obj, name: &str) -> ValidationResult<&'a [Value]> {
    match p.get(name) {
        None => Ok(&[]),
        Some(Value::Array(a)) => Ok(a),
        Some(_) => Err(ValidationError::value(format!("{name} 必须是数组"))),
    }
}

pub fn validate_vibe_encoding(body: &Value) -> Option<String> {
    let model_ok = matches!(values::get(body, "model"), Some(Value::String(m)) if in_models(&VIBE_ENCODED_MODELS, m));
    if !model_ok {
        return Some("Vibe 编码只支持 V4 / V4.5 模型，未扣费。请切换到 V4 或 V4.5。".into());
    }
    if !base64_data(values::get(body, "image"), true, false) {
        return Some("Vibe 参考图无法读取，未扣费。请改用 PNG、JPEG 或 WebP 图片。给客户端作者：image 须为不带 data URL 前缀的 base64".into());
    }
    if !values::get(body, "informationExtracted").is_some_and(unit_value) {
        return Some("Vibe 的「信息提取」须在 0 到 1 之间，未扣费。请在客户端中修改。给客户端作者：informationExtracted 须为 0 到 1 的有限数值".into());
    }
    None
}

fn is_hex64(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// `[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}`（fullmatch）
fn is_uuid_v4(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() != 36 {
        return false;
    }
    let hex = |c: u8| c.is_ascii_digit() || (b'a'..=b'f').contains(&c);
    b.iter().enumerate().all(|(i, &c)| match i {
        8 | 13 | 18 | 23 => c == b'-',
        14 => c == b'4',
        19 => matches!(c, b'8' | b'9' | b'a' | b'b'),
        _ => hex(c),
    })
}

/// 本站还不收的参考图写法（第三方客户端常见）：说清收到的是哪个字段、成员现在能怎么办、客户端作者该改成什么。
/// 前半句给成员看，冒号后给客户端作者看；都说明没扣费，免得成员以为白花了额度。
fn legacy_reference_message(p: &Obj) -> Option<String> {
    let (field, what, fix) = [
        (
            "director_reference_images",
            "旧版角色参考写法",
            "精确参考请改发 director_reference_images_cached（官方网页端格式）",
        ),
        (
            "characterReferences",
            "NovelAI 网页端内部的角色参考字段",
            "精确参考请改发 director_reference_images_cached（官方网页端格式）",
        ),
        (
            "reference_image",
            "旧版单张 Vibe 写法",
            "Vibe 请改发 reference_image_multiple",
        ),
    ]
    .into_iter()
    .find(|(n, _, _)| enabled_opt(p.get(*n)))?;
    Some(format!(
        "本站不支持此客户端的参考图写法：{what} {field}，未扣费。请去掉参考图或更换客户端。开发者参考：{fix}"
    ))
}

/// Accept the older third-party shapes the official web app no longer sends, by rewriting them into the
/// shape it does send, before validation:
/// - `director_reference_images: [png base64, ...]` (NAIS2/3, NAI-Blue, ComfyUI, yuzuki-phone, ppnai,
///   nai-atelier) → `director_reference_images_cached: [{cache_secret_key: sha256(png), data}]`; the four
///   parallel arrays stay as they are and are validated as usual;
/// - `uc` without `negative_prompt` (moe-atelier) → `negative_prompt`.
///
/// Returns `Err(中文说明)` for a raw reference that cannot be accepted (not PNG, or sent twice).
pub fn convert_legacy_references(body: &mut Value) -> Result<(), String> {
    use sha2::{Digest, Sha256};
    let Some(Value::Object(p)) = body.get_mut("parameters") else {
        return Ok(());
    };
    if !p.contains_key("negative_prompt")
        && let Some(Value::String(uc)) = p.get("uc")
        && !uc.trim().is_empty()
    {
        let uc = uc.clone();
        p.insert("negative_prompt".into(), Value::String(uc));
    }
    let Some(raw) = p.get("director_reference_images") else {
        return Ok(());
    };
    let Value::Array(raw) = raw else {
        return Err("精确参考格式错误，未扣费。请更新或更换客户端。给客户端作者：director_reference_images 须为 PNG base64 字符串数组".into());
    };
    if raw.is_empty() {
        p.remove("director_reference_images");
        return Ok(());
    }
    if p.get("director_reference_images_cached")
        .is_some_and(values::enabled)
    {
        return Err("精确参考图重复提交，未扣费。请更新或更换客户端。给客户端作者：director_reference_images 和 director_reference_images_cached 只能选一种".into());
    }
    let mut cached = Vec::with_capacity(raw.len());
    for item in raw {
        let data = match item {
            Value::String(s) => s
                .strip_prefix("data:image/png;base64,")
                .unwrap_or(s)
                .to_string(),
            _ => String::new(),
        };
        let value = Value::String(data.clone());
        if !base64_data(Some(&value), false, true) {
            return Err("精确参考图须为 PNG 格式，未扣费。请去掉参考图或更换客户端。给客户端作者：director_reference_images 每项须为 PNG 的 base64".into());
        }
        let bytes = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, &data)
            .unwrap_or_default();
        let key: String = Sha256::digest(&bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        cached.push(serde_json::json!({"cache_secret_key": key, "data": data}));
    }
    p.remove("director_reference_images");
    p.insert(
        "director_reference_images_cached".into(),
        Value::Array(cached),
    );
    Ok(())
}

/// 完整校验 JSON 里的参考图数据（0..1 区间与 16 张上限是本站自己的输入限制）。返回 `Some(中文说明)` 表示拒绝。
pub fn validate_image_references(payload: &Value) -> Option<String> {
    let Some(root) = payload.as_object() else {
        return Some("生图参数格式错误，未扣费。请更新或更换客户端。给客户端作者：parameters 必须是 JSON 对象".into());
    };
    let p = root.get("parameters").unwrap_or(payload);
    let Some(p) = p.as_object() else {
        return Some("生图参数格式错误，未扣费。请更新或更换客户端。给客户端作者：parameters 必须是 JSON 对象".into());
    };
    let model = model_key(root);
    let mut groups: Vec<(&str, &[Value])> = Vec::with_capacity(REFERENCE_FIELDS.len());
    for name in REFERENCE_FIELDS {
        match reference_list(p, name) {
            Ok(list) => groups.push((name, list)),
            Err(e) => return Some(e.message().to_string()),
        }
    }
    let group = |name: &str| -> &[Value] {
        groups
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, l)| *l)
            .unwrap_or(&[])
    };
    if let Some(msg) = legacy_reference_message(p) {
        return Some(msg);
    }
    let vibes = group("reference_image_multiple");
    let cached_vibes = group("reference_image_multiple_cached");
    let precise = group("director_reference_images_cached");
    if !vibes.is_empty() && !cached_vibes.is_empty() {
        return Some("参考图重复提交，未扣费。请更新或更换客户端。给客户端作者：reference_image_multiple 和 reference_image_multiple_cached 只能选一种".into());
    }
    let vibe_count = if vibes.is_empty() {
        cached_vibes.len()
    } else {
        vibes.len()
    };
    let precise_count = precise.len();
    if vibe_count > REFERENCE_LIMIT || precise_count > REFERENCE_LIMIT {
        return Some("参考图过多，每次最多 16 张，未扣费。请减少参考图。".into());
    }
    if vibe_count > 0 && precise_count > 0 {
        return Some("精确参考和 Vibe 不能同时使用，未扣费。请只保留一种。".into());
    }
    let is_raw = in_models(&VIBE_RAW_MODELS, &model);
    let is_encoded = in_models(&VIBE_ENCODED_MODELS, &model);
    if vibe_count > 0 && !(is_raw || is_encoded) {
        return Some(
            "该模型不支持 Vibe，未扣费。请切换到 V3、V4 或 V4.5，或去掉 Vibe 参考图。".into(),
        );
    }
    if precise_count > 0 && !in_models(&PRECISE_REFERENCE_MODELS, &model) {
        return Some("精确参考只支持 V4.5，未扣费。请切换到 V4.5，或去掉精确参考图。".into());
    }

    for name in [
        "reference_information_extracted_multiple",
        "reference_strength_multiple",
    ] {
        // V4/V4.5 的编码里已含提取量；V3 原图仍然需要。显式提供的数组必须合法。
        if name == "reference_information_extracted_multiple" && is_encoded && !p.contains_key(name)
        {
            continue;
        }
        let values = group(name);
        if values.len() != vibe_count || !values.iter().all(unit_value) {
            return Some(format!(
                "Vibe 强度或信息提取的设置与参考图数量不一致，或不在 0 到 1 之间，未扣费。请检查每张参考图的设置。给客户端作者：{name} 须与 Vibe 数量一致，且各值在 0 到 1 之间"
            ));
        }
    }
    for name in [
        "director_reference_information_extracted",
        "director_reference_strength_values",
        "director_reference_secondary_strength_values",
    ] {
        let values = group(name);
        if values.len() != precise_count || !values.iter().all(unit_value) {
            return Some(format!(
                "精确参考强度的设置与参考图数量不一致，或不在 0 到 1 之间，未扣费。请检查每张参考图的设置。给客户端作者：{name} 须与精确参考数量一致，且各值在 0 到 1 之间"
            ));
        }
    }
    let descriptions = group("director_reference_descriptions");
    if descriptions.len() != precise_count {
        return Some("精确参考类型的设置与参考图数量不一致，未扣费。请检查每张参考图的设置。给客户端作者：director_reference_descriptions 须与参考图片数量一致".into());
    }
    for item in descriptions {
        let caption = item
            .as_object()
            .and_then(|m| m.get("caption"))
            .and_then(Value::as_object);
        let bad = match caption {
            None => true,
            Some(c) => {
                !matches!(c.get("base_caption"), Some(Value::String(s)) if matches!(s.as_str(), "character" | "style" | "character&style"))
                    || !matches!(c.get("char_captions"), Some(Value::Array(a)) if a.is_empty())
                    || !matches!(item.get("legacy_uc"), Some(Value::Bool(false)))
            }
        };
        if bad {
            return Some("精确参考的类型只能是「角色」「风格」或「角色和风格」，未扣费。请在客户端中重新选择。给客户端作者：base_caption 须为 character、style 或 character&style".into());
        }
    }
    for item in vibes {
        if !base64_data(Some(item), is_raw, false)
            || (is_encoded && base64_data(Some(item), true, false))
        {
            return Some("Vibe 参考图数据无法读取，未扣费。请重新添加参考图。给客户端作者：须为有效 base64，V3 要传原图而不是 V4 编码".into());
        }
    }
    // `cached_vibes + precise`：前半是 Vibe 缓存，后半是精确参考（按下标区分）
    for (idx, item) in cached_vibes.iter().chain(precise.iter()).enumerate() {
        let is_precise = idx >= cached_vibes.len();
        let obj = item.as_object();
        let cache_key = obj
            .and_then(|m| m.get("cache_secret_key"))
            .and_then(Value::as_str);
        // BaiBai 的 Vibe 缓存标识用 UUIDv4；网关不在本地查这些 key，仍要求完整的参考数据
        let valid_key = cache_key.is_some_and(|k| is_hex64(k) || (!is_precise && is_uuid_v4(k)));
        let data = obj.and_then(|m| m.get("data"));
        let bad = obj.is_none()
            || !valid_key
            || !base64_data(data, !is_precise && is_raw, is_precise)
            || (!is_precise && is_encoded && base64_data(data, true, false));
        if bad {
            return Some(
                "参考图缓存数据不完整，未扣费。请重新添加参考图。开发者参考：缓存参考须包含 64 位小写十六进制 cache_secret_key，Vibe 也可用小写 UUIDv4，以及完整 base64 data；精确参考须为 PNG"
                    .into(),
            );
        }
    }
    None
}

/// 单张参考附加费，批次减免在总价中处理；编码另行收费。
pub fn reference_surcharge(payload: &Value) -> ValidationResult<i64> {
    let p = params_of(payload)?;
    let precise = len_or_empty(p.get("director_reference_images_cached"))?;
    let vibes = match p.get("reference_image_multiple") {
        Some(x) if values::enabled(x) => validation::length(x)?,
        _ => len_or_empty(p.get("reference_image_multiple_cached"))?,
    };
    let model = model_key(payload.as_object().unwrap_or(empty_obj()));
    let extra = if in_models(&VIBE_ENCODED_MODELS, &model) {
        vibes.saturating_sub(4) as i64 * 2
    } else {
        0
    };
    Ok(precise as i64 * 5 + extra)
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
fn float_integer(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) if n.is_f64() => n.as_f64().filter(|f| f.is_finite() && f.fract() == 0.0),
        _ => None,
    }
}

fn f_to_i128(f: f64) -> Option<i128> {
    if f.is_finite() && f.abs() < 1.0e38 {
        Some(f as i128)
    } else {
        None
    }
}

// ----------------------------------------------------------------------------- 提示词

/// 按固定顺序列出所有提示词位置：`(路径, 值)`，值可能不是字符串（由调用方判断）。
pub fn prompt_texts(body: &Value) -> Vec<(String, Option<&Value>)> {
    let mut out: Vec<(String, Option<&Value>)> =
        vec![("input".to_string(), values::get(body, "input"))];
    let Some(Value::Object(p)) = values::get(body, "parameters") else {
        return out;
    };
    for key in ["prompt", "negative_prompt"] {
        out.push((format!("parameters.{key}"), p.get(key)));
    }
    if let Some(Value::Array(items)) = p.get("characterPrompts") {
        for (i, item) in items.iter().enumerate() {
            if let Value::Object(m) = item {
                out.push((
                    format!("parameters.characterPrompts[{i}].prompt"),
                    m.get("prompt"),
                ));
                out.push((format!("parameters.characterPrompts[{i}].uc"), m.get("uc")));
            }
        }
    }
    for key in ["v4_prompt", "v4_negative_prompt"] {
        let caption = match p.get(key) {
            Some(Value::Object(m)) => m.get("caption"),
            _ => None,
        };
        let Some(Value::Object(cap)) = caption else {
            continue;
        };
        out.push((format!("{key}.base_caption"), cap.get("base_caption")));
        if let Some(Value::Array(chars)) = cap.get("char_captions") {
            for (i, item) in chars.iter().enumerate() {
                if let Value::Object(m) = item {
                    out.push((format!("{key}.char_captions[{i}]"), m.get("char_caption")));
                }
            }
        }
    }
    out
}

/// 对请求里每个字符串形态的提示词字段原地改写（顺序固定，快照测试按它对拍）。
fn rewrite_prompt_strings(body: &mut Value, f: &mut dyn FnMut(&str) -> String) {
    fn fix(slot: Option<&mut Value>, f: &mut dyn FnMut(&str) -> String) {
        if let Some(Value::String(s)) = slot {
            *s = f(s);
        }
    }
    fix(body.get_mut("input"), f);
    let Some(Value::Object(p)) = body.get_mut("parameters") else {
        return;
    };
    for key in ["prompt", "negative_prompt"] {
        fix(p.get_mut(key), f);
    }
    if let Some(Value::Array(items)) = p.get_mut("characterPrompts") {
        for item in items {
            if let Value::Object(m) = item {
                for key in ["prompt", "uc"] {
                    fix(m.get_mut(key), f);
                }
            }
        }
    }
    for key in ["v4_prompt", "v4_negative_prompt"] {
        let Some(Value::Object(v4)) = p.get_mut(key) else {
            continue;
        };
        let Some(Value::Object(cap)) = v4.get_mut("caption") else {
            continue;
        };
        fix(cap.get_mut("base_caption"), f);
        if let Some(Value::Array(chars)) = cap.get_mut("char_captions") {
            for item in chars {
                if let Value::Object(m) = item {
                    fix(m.get_mut("char_caption"), f);
                }
            }
        }
    }
}

struct Regexes {
    /// `(?<=[A-Za-z_])(\d+(?:\.\d+)?)::`
    name_digits: FancyRegex,
    /// `\(([^:\(\)]+):(\d+(?:\.\d+)?)\)`
    webui: Regex,
    /// `(?:^|(?<=,))\s*::\s*(?:$|(?=,))`
    isolated_colons: FancyRegex,
    /// `,\s*,+`
    consecutive_commas: Regex,
    /// `\s*,\s*`
    around_comma: Regex,
    /// `(?<![\d.])([-+]?\d+(?:\.\d+)?)::`
    weight: FancyRegex,
}

fn fancy(pattern: &str) -> FancyRegex {
    // 回溯上限关掉：这些模式在输入长度上是线性的，关掉上限避免长输入误报错
    fancy_regex::RegexBuilder::new(pattern)
        .backtrack_limit(usize::MAX)
        .build()
        .expect("内置正则必须合法")
}

/// `\d` / `\s` 展开成固定的字符类，避免 regex 自带 Unicode 表版本更新带来的差异。
fn regexes() -> &'static Regexes {
    static R: OnceLock<Regexes> = OnceLock::new();
    R.get_or_init(|| {
        let nd = nd_class();
        let d = format!("[{nd}]");
        let s = format!("[{}]", space_class());
        Regexes {
            name_digits: fancy(&format!(r"(?<=[A-Za-z_])({d}+(?:\.{d}+)?)::")),
            webui: Regex::new(&format!(r"\(([^:()]+):({d}+(?:\.{d}+)?)\)"))
                .expect("内置正则必须合法"),
            isolated_colons: fancy(&format!(r"(?:^|(?<=,)){s}*::{s}*(?:$|(?=,))")),
            consecutive_commas: Regex::new(&format!(r",{s}*,+")).expect("内置正则必须合法"),
            around_comma: Regex::new(&format!(r"{s}*,{s}*")).expect("内置正则必须合法"),
            weight: fancy(&format!(r"(?<![{nd}.])([-+]?{d}+(?:\.{d}+)?)::")),
        }
    })
}

/// `re.subn`：返回替换后的文本和替换次数；`rep` 往输出里写替换内容。
fn fancy_sub(
    re: &FancyRegex,
    text: &str,
    mut rep: impl FnMut(&fancy_regex::Captures<'_, str>, &mut String),
) -> (String, usize) {
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    let mut n = 0;
    for caps in re.captures_iter(text) {
        let caps = caps.expect("回溯上限已关闭，不会有运行期错误");
        let m = caps.get(0).expect("第 0 组总存在");
        out.push_str(&text[last..m.start()]);
        rep(&caps, &mut out);
        last = m.end();
        n += 1;
    }
    out.push_str(&text[last..]);
    (out, n)
}

/// 名字结尾的数字紧挨着 `::`（如画师 `bm94199::`）时，在 `::` 前补一个空格，原地改写，返回改了几处。
///
/// 本意是用 `::` 结束一段加权，但上游会把 94199 当成权重，结果 NaN。
/// 只改「数字前面紧挨着字母或下划线」的情况，独立的 `1.2::` / `-3::` 这类正常权重不动。
pub fn fix_name_digit_weights(body: &mut Value) -> usize {
    let re = &regexes().name_digits;
    let mut n = 0;
    rewrite_prompt_strings(body, &mut |text| {
        if !text.contains("::") {
            return text.to_string();
        }
        let (new, k) = fancy_sub(re, text, |caps, out| {
            out.push_str(&caps[1]);
            out.push_str(" ::");
        });
        n += k;
        new
    });
    n
}

/// 清理单条提示词文本：
/// 1. 将 WebUI 权重语法 `(tag:1.2)` 转译为 NAI 原生 `1.2::tag::`；
/// 2. 清理因客户端切词手抖留下的孤立破损 `::`；
/// 3. 合并多余连续逗号与修剪首尾。
pub fn clean_prompt_syntax(text: &str) -> String {
    if text.is_empty() {
        return String::new();
    }
    let r = regexes();
    let t1 = r
        .webui
        .replace_all(text, |c: &regex::Captures<'_>| {
            format!("{}::{}::", &c[2], &c[1])
        })
        .into_owned();
    let (t2, _) = fancy_sub(&r.isolated_colons, &t1, |_, _| {});
    let t3 = r.consecutive_commas.replace_all(&t2, ",").into_owned();
    let t4 = r.around_comma.replace_all(&t3, ", ").into_owned();
    t4.trim_matches(|c| c == ' ' || c == ',').to_string()
}

/// 扫描请求中所有的提示词字段，执行语法自愈与转译，返回修改字段数。
pub fn sanitize_and_translate_prompts(body: &mut Value) -> usize {
    let mut n = 0;
    rewrite_prompt_strings(body, &mut |text| {
        if text.is_empty() {
            return String::new();
        }
        let cleaned = clean_prompt_syntax(text);
        if cleaned != text {
            n += 1;
        }
        cleaned
    });
    n
}

/// 在排队之前拦下上游一定会拒绝的请求，避免白占全站唯一的出图队列。
pub fn upstream_parameter_problem(body: &Value) -> Option<String> {
    let weight = &regexes().weight;
    for (path, text) in prompt_texts(body) {
        let Some(Value::String(text)) = text else {
            continue;
        };
        let size = text.len(); // UTF-8 字节数
        if size >= MAX_PROMPT_BYTES {
            return Some(format!(
                "提示词过长：{size} 字节，上限 {MAX_PROMPT_BYTES} 字节，每个汉字计 3 字节，未扣费。请缩短提示词。开发者参考：超长字段为 {path}"
            ));
        }
        // 不按括号层数拦截（NAI 花括号每层只 ×1.05）。NaN 的真实原因是超大数值权重。
        // 回看断言 `(?<![\d.])`：长串数字不再 O(n²)。
        for caps in weight.captures_iter(text) {
            let caps = caps.expect("回溯上限已关闭，不会有运行期错误");
            let g = caps.get(1).expect("第 1 组总参与匹配").as_str();
            let Some(val) = primitives::parse_float(g) else {
                continue;
            };
            if val.abs() > 100.0 {
                return Some(format!(
                    "提示词中有异常权重 {g}::，通常是画师名中的数字紧挨着 ::，未扣费。请在 :: 前加逗号或空格。"
                ));
            }
        }
    }
    let p = match values::get(body, "parameters") {
        Some(Value::Object(m)) => m,
        _ => empty_obj(),
    };
    let generate = match values::get(body, "action") {
        None => true,
        Some(Value::String(a)) => a == "generate",
        Some(_) => false,
    };
    if generate {
        for key in ["width", "height"] {
            let Some(value) = p.get(key).and_then(int_like) else {
                continue;
            };
            let step = i128::from(DIMENSION_STEP);
            if value > 0 && value % step != 0 {
                let lower = std::cmp::max(step, value / step * step);
                // bool 也是 int：True 会走到这里并被打印成 "True"
                let shown = match p.get(key) {
                    Some(Value::Bool(b)) => primitives::bool_text(*b).to_string(),
                    _ => value.to_string(),
                };
                let side = if key == "width" { "宽度" } else { "高度" };
                return Some(format!(
                    "图片{side} {shown} 不是 64 的倍数，未扣费。请改为 {lower} 或 {}。",
                    lower + step
                ));
            }
        }
    }
    let model = str_of_get(body.as_object().unwrap_or(empty_obj()), "model");
    if model.starts_with("nai-diffusion-4") || model.starts_with("nai-diffusion-5") {
        // V4 / V4.5 最多 6 个角色，V5 最多 22 个。
        let mut counts = vec![match p.get("characterPrompts") {
            Some(Value::Array(a)) => a.len(),
            _ => 0,
        }];
        for key in ["v4_prompt", "v4_negative_prompt"] {
            let caption = match p.get(key) {
                Some(Value::Object(m)) => m.get("caption"),
                _ => None,
            };
            if let Some(Value::Object(cap)) = caption
                && let Some(Value::Array(a)) = cap.get("char_captions")
            {
                counts.push(a.len());
            }
        }
        let max = counts.into_iter().max().unwrap_or(0);
        let limit = if model.starts_with("nai-diffusion-5") {
            MAX_V5_CHARACTERS
        } else {
            MAX_CHARACTERS
        };
        if max > limit {
            let family = if model.starts_with("nai-diffusion-5") {
                "V5"
            } else {
                "V4/V4.5"
            };
            return Some(format!(
                "角色数量超限：本次 {max} 个，{family} 最多 {limit} 个，未扣费。请减少角色。"
            ));
        }
    }
    if let Some(sampler) = p.get("sampler")
        && !sampler.is_null()
    {
        let valid = match sampler {
            Value::String(s) => {
                let stripped = s.replace('_', "");
                s.is_ascii()
                    && primitives::len_chars(s) <= 64
                    && !stripped.is_empty()
                    && stripped.bytes().all(|b| b.is_ascii_alphanumeric())
            }
            _ => false,
        };
        if !valid {
            return Some(
                "采样器名称无效，未扣费。请改用官方标准采样器，如 k_euler_ancestral。".into(),
            );
        }
        let name = sampler.as_str().unwrap_or("");
        if in_models(&V4_V5_REJECTED_SAMPLERS, name)
            && (is_v5_model(&model) || model.starts_with("nai-diffusion-4"))
        {
            return Some(format!(
                "采样器 {name} 不支持 V4/V4.5/V5 模型，未扣费。请改用 k_euler_ancestral、k_dpmpp_2m 等。"
            ));
        }
        if in_models(&V5_REJECTED_SAMPLERS, name) && is_v5_model(&model) {
            return Some(format!(
                "采样器 {name} 不支持 V5 模型，未扣费。请改用 k_euler_ancestral、k_dpmpp_2m 等。"
            ));
        }
    }
    if let Some(schedule) = p.get("noise_schedule")
        && !schedule.is_null()
    {
        let valid =
            matches!(schedule, Value::String(s) if s.is_ascii() && primitives::len_chars(s) <= 32);
        if !valid {
            return Some("噪声调度 noise_schedule 设置无效，未扣费。请在客户端中恢复默认值。给客户端作者：noise_schedule 须为不超过 32 个字符的 ASCII 字符串".into());
        }
    }
    None
}

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

fn checked_ceil(x: f64) -> ValidationResult<f64> {
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

fn sat_i64(i: i128) -> i64 {
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

// ----------------------------------------------------------------------------- 图片钳制

/// 等比缩小到像素面积上限内，边长取 64 的倍数。
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
    let snap = |side: i128| -> i128 {
        std::cmp::max(
            64,
            i128::from(primitives::round0(side as f64 * scale / 64.0)) * 64,
        )
    };
    let mut w2 = snap(width);
    let mut h2 = snap(height);
    while w2.saturating_mul(h2) > max_pixels && (w2 > 64 || h2 > 64) {
        if w2 >= h2 && w2 > 64 {
            w2 -= 64;
        } else if h2 > 64 {
            h2 -= 64;
        } else {
            break;
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
    if max_pixels < 64 * 64 {
        finish!(Some("MAX_PIXELS 不能小于 4096（最小尺寸 64x64）".into()));
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

    // 老模型保留免费面积内的自定义尺寸；V5 继续使用预设边界。
    let w = int_or(p.get("width"), 0, 0)?;
    let h = int_or(p.get("height"), 0, 0)?;
    if w > 0 && h > 0 {
        let eligible = if is_v5_model(&str_of_get(&out, "model")) {
            V5_NORMAL_PRESETS
                .iter()
                .any(|&(pw, ph)| w <= i128::from(pw) && h <= i128::from(ph))
        } else {
            w.saturating_mul(h) <= i128::from(FREE_MAX_PIXELS)
        };
        let (pw, ph) = if eligible {
            (w, h)
        } else {
            snap_v5_preset(w, h)
        };
        // 先保留原有预设限制，再缩到站点面积上限内，避免预设调整反而超限。
        let (pw, ph) = fit_size(pw, ph, i128::from(max_pixels))?;
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

// ----------------------------------------------------------------------------- V4 结构化提示词

fn centers(c: &Obj) -> Value {
    let default = || float_value(0.5);
    let cen = match c.get("center") {
        Some(Value::Object(m)) => Some(m),
        _ => None,
    };
    let get = |k: &str| cen.and_then(|m| m.get(k)).cloned().unwrap_or_else(default);
    let mut pt = Obj::new();
    pt.insert("x".into(), get("x"));
    pt.insert("y".into(), get("y"));
    Value::Array(vec![Value::Object(pt)])
}

/// V4 / V4.5 / V5 使用 `v4_prompt` / `v4_negative_prompt` 角色结构。
/// 官方网页端用 input / negative_prompt / characterPrompts 拼出这两个结构，这里照做。
pub fn fill_v4_prompt(payload: &Value) -> (Value, Vec<String>) {
    let Some(src) = payload.as_object() else {
        return (payload.clone(), Vec::new());
    };
    let model = str_of_get(src, "model");
    let Some(Value::Object(p0)) = src.get("parameters") else {
        return (payload.clone(), Vec::new());
    };
    if !model.starts_with("nai-diffusion-4") && !model.starts_with("nai-diffusion-5") {
        return (payload.clone(), Vec::new());
    }
    let mut notes = Vec::new();
    let mut out = src.clone();
    let mut p = p0.clone();
    let chars: Vec<&Obj> = match p0.get("characterPrompts") {
        Some(Value::Array(a)) => a.iter().filter_map(Value::as_object).collect(),
        _ => Vec::new(),
    };
    let obj = |entries: Vec<(&str, Value)>| -> Obj {
        entries
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect()
    };
    if !matches!(p.get("v4_prompt"), Some(Value::Object(_))) {
        let char_captions: Vec<Value> = chars
            .iter()
            .map(|c| {
                Value::Object(obj(vec![
                    ("char_caption", Value::String(str_or_empty(c.get("prompt")))),
                    ("centers", centers(c)),
                ]))
            })
            .collect();
        let caption = obj(vec![
            (
                "base_caption",
                Value::String(str_or_empty(src.get("input"))),
            ),
            ("char_captions", Value::Array(char_captions)),
        ]);
        p.insert(
            "v4_prompt".into(),
            Value::Object(obj(vec![
                ("caption", Value::Object(caption)),
                ("use_coords", Value::Bool(enabled_opt(p0.get("use_coords")))),
                ("use_order", Value::Bool(true)),
            ])),
        );
        notes.push("已按官方格式补上 v4_prompt".to_string());
    }
    if !matches!(p.get("v4_negative_prompt"), Some(Value::Object(_))) {
        let char_captions: Vec<Value> = chars
            .iter()
            .map(|c| {
                Value::Object(obj(vec![
                    ("char_caption", Value::String(str_or_empty(c.get("uc")))),
                    ("centers", centers(c)),
                ]))
            })
            .collect();
        let caption = obj(vec![
            (
                "base_caption",
                Value::String(str_or_empty(p0.get("negative_prompt"))),
            ),
            ("char_captions", Value::Array(char_captions)),
        ]);
        p.insert(
            "v4_negative_prompt".into(),
            Value::Object(obj(vec![
                ("caption", Value::Object(caption)),
                ("legacy_uc", Value::Bool(false)),
            ])),
        );
        notes.push("已按官方格式补上 v4_negative_prompt".to_string());
    }
    out.insert("parameters".into(), Value::Object(p));
    (Value::Object(out), notes)
}

// ----------------------------------------------------------------------------- V5 Medium

/// V5 Full（含局部重绘）对应的 Medium 模型；不是这两个模型（含已经是 Medium、Curated、别名）返回 None。
pub fn v5_medium_of(model: &str) -> Option<&'static str> {
    let m = primitives::strip(model).to_lowercase();
    V5_MEDIUM_OF
        .iter()
        .find(|(full, _)| *full == m)
        .map(|(_, medium)| *medium)
}

pub fn is_v5_medium(model: &str) -> bool {
    let m = primitives::strip(model).to_lowercase();
    V5_MEDIUM_OF.iter().any(|(_, medium)| *medium == m)
}

/// `{**c, key: ""}`：保留原键顺序，键已存在则原位覆盖，不存在则追加。
fn with_blank(c: &Value, key: &str) -> Value {
    match c {
        Value::Object(m) => {
            let mut m2 = m.clone();
            m2.insert(key.into(), Value::String(String::new()));
            Value::Object(m2)
        }
        other => other.clone(),
    }
}

/// 按官方前端对 Medium 的做法整理参数：步数 / 采样器 / 负面预设固定，自定义负面词清空，rescale 归零。
pub fn medium_normalize(payload: &Value) -> (Value, Vec<String>) {
    let mut out = payload.as_object().cloned().unwrap_or_default();
    let mut p = params_copy(&out);
    out.insert("parameters".into(), Value::Null);
    let mut notes = Vec::new();
    // `p.get("steps") != 14`：14.0 == 14，不会被改写
    if !num_eq_int(p.get("steps"), i128::from(V5_MEDIUM_STEPS)) {
        p.insert("steps".into(), Value::from(V5_MEDIUM_STEPS));
    }
    if p.get("sampler").and_then(Value::as_str) != Some(V5_MEDIUM_SAMPLER) {
        p.insert("sampler".into(), Value::String(V5_MEDIUM_SAMPLER.into()));
    }
    // `(x or 0) != 0`：任何真值都不等于 0
    if enabled_opt(p.get("cfg_rescale")) {
        p.insert("cfg_rescale".into(), Value::from(0));
        notes.push("Medium 不支持 rescale，已设为 0".into());
    }
    let user_uc = primitives::strip(&str_or_empty(p.get("negative_prompt"))).to_string();
    if !user_uc.is_empty() && user_uc != V5_MEDIUM_UC {
        notes.push(
            "Medium 不支持自定义负面词，已换成官方 Heavy 预设（可在正面提示词里用 -2::xx:: 排除）"
                .into(),
        );
    }
    p.insert("negative_prompt".into(), Value::String(V5_MEDIUM_UC.into()));
    p.insert("ucPreset".into(), Value::from(V5_MEDIUM_UC_PRESET));
    if let Some(Value::Object(neg0)) = p.get("v4_negative_prompt") {
        let mut neg = neg0.clone();
        let mut cap = match neg.get("caption") {
            Some(Value::Object(m)) => m.clone(),
            _ => Obj::new(),
        };
        cap.insert("base_caption".into(), Value::String(V5_MEDIUM_UC.into()));
        if let Some(Value::Array(cc)) = cap.get("char_captions") {
            let blanked: Vec<Value> = cc.iter().map(|c| with_blank(c, "char_caption")).collect();
            cap.insert("char_captions".into(), Value::Array(blanked));
        }
        neg.insert("caption".into(), Value::Object(cap));
        p.insert("v4_negative_prompt".into(), Value::Object(neg));
    }
    if let Some(Value::Array(cps)) = p.get("characterPrompts") {
        let blanked: Vec<Value> = cps.iter().map(|c| with_blank(c, "uc")).collect();
        p.insert("characterPrompts".into(), Value::Array(blanked));
    }
    out.insert("parameters".into(), Value::Object(p));
    (Value::Object(out), notes)
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

// ----------------------------------------------------------------------------- 连接测试请求

/// 「连接测试」请求的判定阈值。依据（线上日志 7 天、12,650 张成功出图）：流行的 SillyTavern 预设把「测试连接」
/// 当成真实生图发（提示词 `simple circle, white background`、512x512、4 步，还吃掉过 V5 额度）；
/// 另有客户端发提示词 `test`、64x64、1 步。7 天里没有任何正常请求同时满足「步数 ≤ 5 且面积 ≤ 512x512」，
/// 正常提示词平均约 1000 字符。三个阈值必须同时满足（再加「普通生图、单张、无角色」）才算测试。
pub const TEST_REQUEST_MAX_STEPS: i64 = 5;
/// 见 [`TEST_REQUEST_MAX_STEPS`]：宽 × 高不超过 512 × 512。
pub const TEST_REQUEST_MAX_PIXELS: i64 = 512 * 512;
/// 见 [`TEST_REQUEST_MAX_STEPS`]：提示词（去首尾空白后）不超过 40 个字符。
pub const TEST_REQUEST_MAX_PROMPT_CHARS: usize = 40;

/// 参数里出现任何一个（值为真）就说明不是单纯的文生图：图生图 / 局部重绘 / ControlNet / 参考图。
const NON_PLAIN_PARAMS: [&str; 6] = [
    "image",
    "mask",
    "controlnet_condition",
    "controlnet_model",
    "controlnet_strength",
    "reference_image",
];

/// 这是不是客户端的「连接测试」请求（见 [`TEST_REQUEST_MAX_STEPS`] 的依据）。要在
/// [`normalize_image_request`] 和提示词清理之后调用（尺寸 / 步数已校验为整数，提示词已按流水线规范化）。
///
/// 全部满足才算：动作是普通 generate（没有图生图 / 局部重绘 / 蒙版 / Vibe / 参考图 / ControlNet）、
/// 步数 ≤ 5、宽 × 高 ≤ 512 × 512、一次 ≤ 1 张、没有角色提示词、提示词 ≤ 40 字符
/// （V4 / V4.5 结构化提示词取 `base_caption`，否则取 `input`）。
/// 缺步数或尺寸（上游按默认值出大图）不算。
pub fn is_connection_test(body: &Value) -> bool {
    let Some(Value::Object(p)) = values::get(body, "parameters") else {
        return false;
    };
    let action = primitives::strip(&str_or_empty(values::get(body, "action"))).to_lowercase();
    if !action.is_empty() && action != "generate" {
        return false;
    }
    if NON_PLAIN_PARAMS
        .iter()
        .chain(REFERENCE_FIELDS.iter())
        .any(|k| enabled_opt(p.get(*k)))
        || ["image", "mask"]
            .iter()
            .any(|k| enabled_opt(values::get(body, k)))
    {
        return false;
    }
    let int = |k: &str| p.get(k).and_then(Value::as_i64);
    let (Some(steps), Some(width), Some(height)) = (int("steps"), int("width"), int("height"))
    else {
        return false;
    };
    let n = int("n_samples").filter(|n| *n != 0).unwrap_or(1);
    if steps > TEST_REQUEST_MAX_STEPS
        || width.saturating_mul(height) > TEST_REQUEST_MAX_PIXELS
        || n > 1
    {
        return false;
    }
    if matches!(p.get("characterPrompts"), Some(Value::Array(a)) if !a.is_empty()) {
        return false;
    }
    let v4 = match p.get("v4_prompt") {
        Some(Value::Object(m)) => match m.get("caption") {
            Some(Value::Object(cap)) => Some(cap),
            _ => None,
        },
        _ => None,
    };
    if let Some(cap) = v4
        && matches!(cap.get("char_captions"), Some(Value::Array(a)) if !a.is_empty())
    {
        return false;
    }
    let prompt = match v4.and_then(|c| c.get("base_caption")) {
        Some(Value::String(s)) => s.as_str(),
        _ => match values::get(body, "input") {
            Some(Value::String(s)) => s.as_str(),
            _ => "",
        },
    };
    primitives::len_chars(primitives::strip(prompt)) <= TEST_REQUEST_MAX_PROMPT_CHARS
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn v5_structured_prompts_include_base_and_character_captions() {
        let payload = json!({
            "model": "nai-diffusion-5-curated",
            "input": "fur dataset, red owl",
            "parameters": {
                "negative_prompt": "lowres",
                "use_coords": true,
                "characterPrompts": [{
                    "prompt": "owl girl",
                    "uc": "bad hands",
                    "center": {"x": 0.25, "y": 0.75}
                }]
            }
        });
        let (filled, _) = fill_v4_prompt(&payload);
        assert_eq!(
            filled["parameters"]["v4_prompt"],
            json!({
                "caption": {
                    "base_caption": "fur dataset, red owl",
                    "char_captions": [{
                        "char_caption": "owl girl",
                        "centers": [{"x": 0.25, "y": 0.75}]
                    }]
                },
                "use_coords": true,
                "use_order": true
            })
        );
        assert_eq!(
            filled["parameters"]["v4_negative_prompt"],
            json!({
                "caption": {
                    "base_caption": "lowres",
                    "char_captions": [{
                        "char_caption": "bad hands",
                        "centers": [{"x": 0.25, "y": 0.75}]
                    }]
                },
                "legacy_uc": false
            })
        );
    }

    #[test]
    fn v5_structured_prompts_are_added_even_without_characters() {
        let payload = json!({
            "model": "nai-diffusion-5-full",
            "input": "simple circle",
            "parameters": {"negative_prompt": "lowres"}
        });
        let (filled, _) = fill_v4_prompt(&payload);
        assert_eq!(
            filled["parameters"]["v4_prompt"],
            json!({
                "caption": {"base_caption": "simple circle", "char_captions": []},
                "use_coords": false,
                "use_order": true
            })
        );
        assert_eq!(
            filled["parameters"]["v4_negative_prompt"]["caption"]["char_captions"],
            json!([])
        );
    }

    #[test]
    fn character_limits_follow_model_family() {
        let v5_chars: Vec<Value> = (0..MAX_V5_CHARACTERS)
            .map(|i| json!({"prompt": format!("character {i}")}))
            .collect();
        let v5 =
            json!({"model":"nai-diffusion-5-curated","parameters":{"characterPrompts":v5_chars}});
        assert_eq!(upstream_parameter_problem(&v5), None);

        let too_many_v5: Vec<Value> = (0..=MAX_V5_CHARACTERS)
            .map(|i| json!({"prompt": format!("character {i}")}))
            .collect();
        let v5 =
            json!({"model":"nai-diffusion-5-full","parameters":{"characterPrompts":too_many_v5}});
        assert!(
            upstream_parameter_problem(&v5)
                .unwrap()
                .contains("最多 22 个")
        );

        let v4_chars: Vec<Value> = (0..=MAX_CHARACTERS)
            .map(|i| json!({"prompt": format!("character {i}")}))
            .collect();
        let v4 =
            json!({"model":"nai-diffusion-4-5-full","parameters":{"characterPrompts":v4_chars}});
        assert!(
            upstream_parameter_problem(&v4)
                .unwrap()
                .contains("最多 6 个")
        );
    }

    #[test]
    fn legacy_reference_fields_are_named_in_the_error() {
        let req = |field: &str| json!({"model": "nai-diffusion-4-5-full", "parameters": {field: ["iVBORw0KGgo="]}});
        for (field, fix) in [
            (
                "director_reference_images",
                "director_reference_images_cached",
            ),
            ("characterReferences", "director_reference_images_cached"),
            ("reference_image", "reference_image_multiple"),
        ] {
            let msg = validate_image_references(&req(field)).expect("应拒绝");
            assert!(msg.contains(&format!(" {field}，")), "{msg}");
            assert!(msg.contains(fix) && msg.contains("未扣费"), "{msg}");
        }
        // 空数组不算用了旧写法
        assert_eq!(
            validate_image_references(
                &json!({"model": "nai-diffusion-4-5-full", "parameters": {"director_reference_images": []}})
            ),
            None
        );
    }

    // 8-byte PNG signature + IHDR start, enough for the magic check
    const PNG_B64: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAAB";
    const JPEG_B64: &str = "/9j/4AAQSkZJRgABAQAAAQABAAD";

    fn precise_req(refs: Value) -> Value {
        let n = refs.as_array().map_or(0, Vec::len);
        let desc = json!({"caption": {"base_caption": "character&style", "char_captions": []}, "legacy_uc": false});
        json!({"model": "nai-diffusion-4-5-full", "parameters": {
            "director_reference_images": refs,
            "director_reference_descriptions": vec![desc; n],
            "director_reference_information_extracted": vec![1; n],
            "director_reference_strength_values": vec![1; n],
            "director_reference_secondary_strength_values": vec![0; n],
        }})
    }

    #[test]
    fn raw_precise_references_become_cached() {
        let mut body = precise_req(json!([PNG_B64, format!("data:image/png;base64,{PNG_B64}")]));
        convert_legacy_references(&mut body).expect("PNG is accepted");
        let p = &body["parameters"];
        assert!(p.get("director_reference_images").is_none());
        let cached = p["director_reference_images_cached"].as_array().unwrap();
        assert_eq!(cached.len(), 2);
        let key = cached[0]["cache_secret_key"].as_str().unwrap();
        assert!(is_hex64(key), "{key}");
        assert_eq!(cached[1]["cache_secret_key"], cached[0]["cache_secret_key"]); // same bytes, same key
        assert_eq!(cached[1]["data"], json!(PNG_B64)); // data URL prefix dropped
        assert_eq!(p["director_reference_strength_values"], json!([1, 1]));
        assert_eq!(validate_image_references(&body), None);
    }

    #[test]
    fn raw_precise_references_that_cannot_be_accepted() {
        let msg = convert_legacy_references(&mut precise_req(json!([JPEG_B64]))).unwrap_err();
        assert!(msg.contains("PNG") && msg.contains("未扣费"), "{msg}");
        let mut both = precise_req(json!([PNG_B64]));
        both["parameters"]["director_reference_images_cached"] =
            json!([{"cache_secret_key": "a".repeat(64), "data": PNG_B64}]);
        let msg = convert_legacy_references(&mut both).unwrap_err();
        assert!(msg.contains("重复提交"), "{msg}");
        let mut empty = precise_req(json!([]));
        convert_legacy_references(&mut empty).unwrap();
        assert!(
            empty["parameters"]
                .get("director_reference_images")
                .is_none()
        );
        // characterReferences stays rejected
        let mut cr = json!({"model": "nai-diffusion-4-5-full", "parameters": {"characterReferences": [PNG_B64]}});
        convert_legacy_references(&mut cr).unwrap();
        assert!(
            validate_image_references(&cr)
                .unwrap()
                .contains("characterReferences")
        );
    }

    #[test]
    fn uc_fills_a_missing_negative_prompt_only() {
        let mut b = json!({"model": "m", "parameters": {"uc": "lowres, bad hands"}});
        convert_legacy_references(&mut b).unwrap();
        assert_eq!(
            b["parameters"]["negative_prompt"],
            json!("lowres, bad hands")
        );
        let mut both = json!({"model": "m", "parameters": {"uc": "a", "negative_prompt": "b"}});
        convert_legacy_references(&mut both).unwrap();
        assert_eq!(both["parameters"]["negative_prompt"], json!("b"));
        let mut blank = json!({"model": "m", "parameters": {"uc": "  "}});
        convert_legacy_references(&mut blank).unwrap();
        assert!(blank["parameters"].get("negative_prompt").is_none());
    }

    fn probe(prompt: &str, steps: i64, w: i64, h: i64) -> Value {
        json!({"input": prompt, "model": "nai-diffusion-4-5-full", "action": "generate",
               "parameters": {"width": w, "height": h, "steps": steps, "n_samples": 1}})
    }

    #[test]
    fn connection_test_known_signatures() {
        // SillyTavern 预设
        let st = probe("simple circle, white background", 4, 512, 512);
        assert!(is_connection_test(&st));
        // 另一个客户端
        assert!(is_connection_test(&probe("test", 1, 64, 64)));
        // 同样的提示词，正常参数不算
        let normal = probe("simple circle, white background", 28, 832, 1216);
        assert!(!is_connection_test(&normal));
    }

    #[test]
    fn connection_test_boundaries() {
        let t = is_connection_test;
        assert!(t(&probe("a", 5, 512, 512)));
        assert!(!t(&probe("a", 6, 512, 512)));
        assert!(!t(&probe("a", 5, 513, 512)));
        assert!(t(&probe("a", 5, 256, 1024))); // 面积相同，形状不限
        assert!(t(&probe(&"x".repeat(40), 5, 512, 512)));
        assert!(!t(&probe(&"x".repeat(41), 5, 512, 512)));
        // 首尾空白不算；按字符数而不是字节数
        assert!(t(&probe(&format!("  {}  ", "x".repeat(40)), 5, 512, 512)));
        assert!(t(&probe(&"猫".repeat(40), 5, 512, 512)));
        assert!(!t(&probe(&"猫".repeat(41), 5, 512, 512)));
    }

    #[test]
    fn connection_test_requires_plain_single_generate() {
        let t = is_connection_test;
        let mut p = probe("test", 1, 64, 64);
        p["action"] = json!("img2img");
        assert!(!t(&p));
        p["action"] = json!("infill");
        assert!(!t(&p));
        p["action"] = json!("GENERATE");
        assert!(t(&p));
        let mut p = probe("test", 1, 64, 64);
        p["parameters"]["image"] = json!("QUJD");
        assert!(!t(&p));
        let mut p = probe("test", 1, 64, 64);
        p["parameters"]["mask"] = json!("QUJD");
        assert!(!t(&p));
        let mut p = probe("test", 1, 64, 64);
        p["parameters"]["reference_image_multiple"] = json!(["QUJD"]);
        assert!(!t(&p));
        let mut p = probe("test", 1, 64, 64);
        p["parameters"]["director_reference_images_cached"] = json!([{"cache_secret_key": "k"}]);
        assert!(!t(&p));
        let mut p = probe("test", 1, 64, 64);
        p["parameters"]["controlnet_condition"] = json!("QUJD");
        assert!(!t(&p));
        // 空的参考图列表不算
        let mut p = probe("test", 1, 64, 64);
        p["parameters"]["reference_image_multiple"] = json!([]);
        assert!(t(&p));
        let mut p = probe("test", 1, 64, 64);
        p["parameters"]["n_samples"] = json!(2);
        assert!(!t(&p));
        let mut p = probe("test", 1, 64, 64);
        p["parameters"]["characterPrompts"] = json!([{"prompt": "girl"}]);
        assert!(!t(&p));
        // 缺步数 / 尺寸：上游按默认值出大图，不算测试
        let mut p = probe("test", 1, 64, 64);
        p["parameters"].as_object_mut().unwrap().remove("steps");
        assert!(!t(&p));
        let mut p = probe("test", 1, 64, 64);
        p["parameters"].as_object_mut().unwrap().remove("width");
        assert!(!t(&p));
        assert!(!t(&json!({"input": "test", "model": "x"})));
    }

    #[test]
    fn connection_test_uses_v4_base_caption() {
        let t = is_connection_test;
        let mut p = probe("", 4, 512, 512);
        p["parameters"]["v4_prompt"] =
            json!({"caption": {"base_caption": "simple circle", "char_captions": []}});
        assert!(t(&p));
        p["parameters"]["v4_prompt"]["caption"]["base_caption"] = json!("x".repeat(41));
        assert!(!t(&p));
        p["parameters"]["v4_prompt"]["caption"]["base_caption"] = json!("ok");
        p["parameters"]["v4_prompt"]["caption"]["char_captions"] =
            json!([{"char_caption": "girl"}]);
        assert!(!t(&p));
    }

    #[test]
    fn strict_base64_matches_binascii_rules() {
        assert!(strict_b64("QQ==", 12).is_some());
        assert!(strict_b64("QUI=", 12).is_some());
        assert!(strict_b64("QUJD", 12).is_some());
        assert!(strict_b64("", 12).is_some());
        for bad in [
            "QQ=", "QQ", "Q", "A=", "=QQ=", "QQ==QQ==", "QUJD====", "QQ ==", "QUJD\n", "QUI==",
            "QQ=Q", "-_",
        ] {
            assert!(strict_b64(bad, 12).is_none(), "{bad}");
        }
        let (head, total) = strict_b64("QUJDREVGR0hJSktMTU4=", 4).unwrap();
        assert_eq!(head, b"ABCD");
        assert_eq!(total, 14);
    }

    #[test]
    fn huge_base64_is_rejected_without_decoding() {
        let big = Value::String("A".repeat(25 * 1024 * 1024 + 4));
        assert!(!base64_data(Some(&big), false, false));
        let edge = Value::String("A".repeat(25 * 1024 * 1024));
        assert!(base64_data(Some(&edge), false, false));
    }

    #[test]
    fn prompt_cleanup_samples() {
        assert_eq!(
            clean_prompt_syntax("(red hair:1.2), (blue eyes:0.8)"),
            "1.2::red hair::, 0.8::blue eyes::"
        );
        assert_eq!(clean_prompt_syntax("a,, b ,::, c"), "a, b, c");
        let mut body = json!({"input": "bm94199::, 1.2::tag::"});
        assert_eq!(fix_name_digit_weights(&mut body), 1);
        assert_eq!(body["input"], "bm94199 ::, 1.2::tag::");
    }

    #[test]
    fn gen_key_format() {
        let k = gen_key("nai", &[0; 24]);
        assert_eq!(k, format!("nai-{}", "A".repeat(32)));
    }

    #[test]
    fn k_dpm_2_rejected_on_v5_only() {
        let body = |model: &str| json!({"model": model, "parameters": {"sampler": "k_dpm_2", "width": 832, "height": 1216, "steps": 28}});
        let v5 = upstream_parameter_problem(&body("nai-diffusion-5-full")).expect("V5 rejected");
        assert!(v5.contains("k_dpm_2 不支持 V5"), "{v5}");
        assert_eq!(
            upstream_parameter_problem(&body("nai-diffusion-4-5-full")),
            None
        );
    }
}
