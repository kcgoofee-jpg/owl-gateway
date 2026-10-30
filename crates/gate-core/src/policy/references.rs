//! 参考图与 Vibe：base64 校验、旧格式迁移、张数与体积限制、精确参考的加价。

use super::*;

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

pub(crate) fn is_hex64(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// `[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}`（fullmatch）
pub(crate) fn is_uuid_v4(s: &str) -> bool {
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
pub(crate) fn legacy_reference_message(p: &Obj) -> Option<String> {
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
