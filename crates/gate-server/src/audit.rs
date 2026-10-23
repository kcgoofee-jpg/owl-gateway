//! 生成记录：提示词与原图。均可单独关闭，自动过期。
//! 画廊小图从原图现场缩小，不另存缩略图。
//!
//! 本模块只放「记什么 / 怎么记」：
//! - [`capture_prompts`] / [`prompt_texts`]：从请求体取提示词与生成参数，字段类型不对时报 `Err`；
//! - [`full_image`] / [`make_thumbnail`]：从上游返回的 zip（或裸图片）里取第一张图；
//! - `audit_*` 设置读取：记录开关 / 保留时长。
//!
//! 调用点（出图成功后的后台任务、后台画廊接口）不在这里。
//!
//! 刻意保留的怪癖（都有快照 `tests/snapshots/audit/` 冻结现状，改动要显式更新快照）：
//! - 请求体里字段类型不对时按失败处理：[`capture_prompts`] 返回 `PyResult`，
//!   调用方（`audit_generation`）要处理 `Err`——丢掉整条记录并记一条 bug，不能当成「没有提示词」；
//! - [`full_image`] 不校验内容：任何不超过 15 MiB 的字节串都原样存下，类型串靠魔数猜，猜不出就当 PNG；
//! - 缩略图要真的解码：像素超过 2500 万、损坏、截断都返回 `None`；
//! - zip 里取「后缀是 .png / .jpg / .jpeg / .webp（不分大小写）的第一个条目」，不看目录层级；
//!   同名条目以最后一个为准；
//! - `audit_flags` 里存着的天数解析不出有限数（空串 / `nan` / `inf` / `1e999` / 乱码）时回到 7 天，不是环境默认值；

//!
//! 图片编解码边界（`image` crate；快照见 tests/audit_snapshot.rs）：
//! - 编码字节不承诺逐位稳定：缩放核、JPEG 量化 / 子采样 / 霍夫曼优化随 `image` crate 的实现走；尺寸、收不收、类型串稳定；
//! - 只认 PNG / JPEG / WebP（上游只会返回这几种）；
//! - zip 只支持 stored / deflate（上游只这样压）；
//! - 截掉了 PNG 尾部（IEND 与最后一个 IDAT 的校验）但像素数据完整的图，这里拒绝；
//! - 16 位灰度 PNG 的颜色取高 8 位；
//! - 动画图（APNG / 动画 WebP）取第一帧；
//! - `repr` 里的未分配码点（`str(非字符串提示词)` 时才会走到）见 `gate_core::pyext::repr_str` 的说明。

use gate_core::pyext::{PyErr, PyResult, py_str, type_name};
use gate_core::{pyc, pyjson, pyval, site_flags};
use gate_db::{Db, DbError};
use image::codecs::jpeg::JpegEncoder;
use image::imageops::FilterType;
use image::{DynamicImage, ImageFormat, ImageReader, Limits};
use serde_json::{Map, Value};
use std::borrow::Cow;
use std::io::{Cursor, Read};

/// 画廊卡片按原图比例整张显示，高清屏上 512 会糊。
pub const THUMB_SIDE: u32 = 768;
pub const THUMB_QUALITY: u8 = 82;
pub const MAX_IMAGE_BYTES: usize = 15 * 1024 * 1024;

/// Pillow 的 `Image.MAX_IMAGE_PIXELS`：超过即报错（默认只是警告），防止解压炸弹。
/// 「超过 1 倍」就直接拒绝（防解压炸弹，宁严勿宽）。
pub const MAX_IMAGE_PIXELS: u64 = 25_000_000;
/// 缩略图只肯读声明大小不超过 40 MiB 的 zip 条目。
const MAX_THUMB_ZIP_ENTRY: u64 = 40 * 1024 * 1024;
const PNG_MAGIC: &[u8] = b"\x89PNG\r\n\x1a\n";

/// `extra.params` 里保留的生成参数（顺序即输出顺序）。
const PARAM_KEYS: [&str; 22] = [
    "seed",
    "sampler",
    "steps",
    "scale",
    "width",
    "height",
    "noise_schedule",
    "cfg_rescale",
    "sm",
    "sm_dyn",
    "ucPreset",
    "qualityToggle",
    "n_samples",
    // 以下影响复现（Variety+、采样行为、角色坐标开关）
    "skip_cfg_above_sigma",
    "uncond_scale",
    "legacy",
    "legacy_v3_extend",
    "legacy_uc",
    "prefer_brownian",
    "deliberate_euler_ancestral_bug",
    "params_version",
    "use_coords",
];

// ----------------------------------------------------------------------------- 提示词

/// `x.get(key)`：不是 dict 时 Python 抛 `AttributeError`。
fn dict_get<'a>(v: &'a Value, key: &str) -> PyResult<Option<&'a Value>> {
    match v {
        Value::Object(m) => Ok(m.get(key)),
        other => Err(PyErr::Attribute(format!(
            "'{}' object has no attribute 'get'",
            type_name(other)
        ))),
    }
}

/// `str(v or "")`。
fn str_or_empty(v: Option<&Value>) -> String {
    match v {
        Some(x) if pyval::truthy(x) => py_str(x),
        _ => String::new(),
    }
}

/// `body.get("parameters") if isinstance(body.get("parameters"), dict) else {}`。
/// `body` 本身不是 dict 时 `.get` 抛 `AttributeError`。
fn params_of(body: &Value) -> PyResult<Option<&Map<String, Value>>> {
    Ok(dict_get(body, "parameters")?.and_then(Value::as_object))
}

fn param<'a>(params: Option<&'a Map<String, Value>>, key: &str) -> Option<&'a Value> {
    params.and_then(|m| m.get(key))
}

/// 正向 / 负向提示词（截断，防止超长内容写库）。
pub fn prompt_texts(body: &Value) -> PyResult<(String, String)> {
    let params = params_of(body)?;
    let mut positive = str_or_empty(dict_get(body, "input")?);
    let negative = str_or_empty(param(params, "negative_prompt"));
    if positive.is_empty() {
        // ((params.get("v4_prompt") or {}).get("caption") or {}) if isinstance(v4_prompt, dict) else {}
        let caption = match param(params, "v4_prompt") {
            Some(Value::Object(v4)) => v4.get("caption").filter(|c| pyval::truthy(c)),
            _ => None,
        };
        // caption 是真值但不是 dict（字符串 / 列表 / 数字）时 `.get` 抛 AttributeError
        positive = match caption {
            Some(c) => str_or_empty(dict_get(c, "base_caption")?),
            None => String::new(),
        };
    }
    Ok((
        pyc::take_chars(&positive, 2000).to_string(),
        pyc::take_chars(&negative, 1000).to_string(),
    ))
}

/// 从 `v4_prompt` / `v4_negative_prompt` 里取 base_caption 和各角色 caption（去掉空的）。
pub fn captions(node: Option<&Value>) -> PyResult<(String, Vec<String>)> {
    let cap = match node {
        Some(Value::Object(m)) => m.get("caption").filter(|c| pyval::truthy(c)),
        _ => None,
    };
    let Some(cap) = cap else {
        return Ok((String::new(), Vec::new()));
    };
    let base = str_or_empty(dict_get(cap, "base_caption")?);
    // cap.get("char_captions") or []：到这里 cap 一定是 dict（否则上一行已经抛了）
    let listed = dict_get(cap, "char_captions")?.filter(|c| pyval::truthy(c));
    let items: &[Value] = match listed {
        None => &[],
        Some(Value::Array(a)) => a,
        // 字符串迭代出单个字符、dict 迭代出键：都是 str，通不过 isinstance(c, dict)，等于空
        Some(Value::String(_) | Value::Object(_)) => &[],
        Some(other) => {
            return Err(PyErr::Type(format!(
                "'{}' object is not iterable",
                type_name(other)
            )));
        }
    };
    let chars = items
        .iter()
        .filter_map(Value::as_object)
        .map(|c| str_or_empty(c.get("char_caption")))
        .filter(|c| !c.is_empty())
        .collect();
    Ok((base, chars))
}

/// 各角色的 `centers` 坐标，和 [`captions`] 留下的角色一一对应（caption 为空的角色同样被跳过）；
/// 没有坐标的角色是空数组。全部角色都没有坐标时返回空 Vec（extra 里不写这个字段）。
/// 只读不抛：类型不对的一律当没有（类型错误已由 [`captions`] 报过）。
fn char_centers(node: Option<&Value>) -> Vec<Value> {
    let list = node
        .and_then(|n| n.get("caption"))
        .and_then(|c| c.get("char_captions"))
        .and_then(Value::as_array);
    let out: Vec<Value> = list
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
        .filter(|c| !str_or_empty(c.get("char_caption")).is_empty())
        .take(12)
        .map(|c| match c.get("centers") {
            Some(v @ Value::Array(_)) => v.clone(),
            _ => Value::Array(Vec::new()),
        })
        .collect();
    if out.iter().all(|v| v.as_array().is_some_and(Vec::is_empty)) {
        Vec::new()
    } else {
        out
    }
}

/// 完整提示词：返回 `(正面, 负面, extra_json)`。
///
/// extra 里是「正面 / 负面」之外的东西——角色提示词（多人图）和生成参数（种子 / 采样器 / 步数 / CFG / 尺寸…），
/// 否则光存正负面会漏掉多角色场景和复现所需的参数。没有任何这类内容时 extra 是空串。
///
/// `Err` 表示请求体字段类型不对（看不出来就没法记）：调用方要丢掉整条记录。
pub fn capture_prompts(body: &Value) -> PyResult<(String, String, String)> {
    let (mut positive, mut negative) = prompt_texts(body)?;
    let params = params_of(body)?;
    let (pos_base, pos_chars) = captions(param(params, "v4_prompt"))?;
    let (neg_base, neg_chars) = captions(param(params, "v4_negative_prompt"))?;
    if positive.is_empty() {
        positive = pos_base;
    }
    if negative.is_empty() {
        // 只在 v4_negative_prompt 里写负面词的客户端，以前会漏记
        negative = neg_base;
    }
    let mut kept = Map::new();
    for key in PARAM_KEYS {
        if let Some(v) = param(params, key).filter(|v| !v.is_null()) {
            kept.insert(key.to_string(), v.clone());
        }
    }
    let to_values = |chars: Vec<String>| -> Vec<Value> {
        chars.into_iter().take(12).map(Value::String).collect()
    };
    let (char_prompts, char_negatives) = (to_values(pos_chars), to_values(neg_chars));
    let has = !char_prompts.is_empty() || !char_negatives.is_empty() || !kept.is_empty();
    let extra = if has {
        let mut extra = Map::new();
        extra.insert("char_prompts".into(), Value::Array(char_prompts));
        extra.insert("char_negatives".into(), Value::Array(char_negatives));
        // 角色坐标：和 char_prompts / char_negatives 一一对应，没有坐标就不写
        let pos_centers = char_centers(param(params, "v4_prompt"));
        if !pos_centers.is_empty() {
            extra.insert("char_centers".into(), Value::Array(pos_centers));
        }
        let neg_centers = char_centers(param(params, "v4_negative_prompt"));
        if !neg_centers.is_empty() {
            extra.insert("char_negative_centers".into(), Value::Array(neg_centers));
        }
        extra.insert("params".into(), Value::Object(kept));
        pyc::take_chars(&pyjson::dumps(&Value::Object(extra)), 4000).to_string()
    } else {
        String::new()
    };
    Ok((
        pyc::take_chars(&positive, 4000).to_string(),
        pyc::take_chars(&negative, 2000).to_string(),
        extra,
    ))
}

// ----------------------------------------------------------------------------- 图片

fn is_image_name(raw: &[u8]) -> bool {
    // Python：n.lower().endswith((".png", ".jpg", ".jpeg", ".webp"))。
    // 后缀全是 ASCII，条目名无论按 UTF-8 还是 cp437 解码，ASCII 部分都一样，直接比字节。
    let lower = raw.to_ascii_lowercase();
    [&b".png"[..], b".jpg", b".jpeg", b".webp"]
        .iter()
        .any(|ext| lower.ends_with(ext))
}

/// zip 里第一个图片条目的内容；没有、声明大小超过 `max_declared`、压缩包损坏、CRC 对不上都返回 `None`。
fn first_zip_image(payload: &[u8], max_declared: u64) -> Option<Vec<u8>> {
    let mut archive = zip::ZipArchive::new(Cursor::new(payload)).ok()?;
    let mut picked = None;
    for i in 0..archive.len() {
        let entry = archive.by_index_data(i).ok()?;
        if is_image_name(entry.name_raw()) {
            picked = Some((i, entry.size()));
            break;
        }
    }
    // 重名条目：zip crate 的索引保留第一次出现的位置、内容取最后一个，与 Python `getinfo(names[0])` 一致
    let (index, declared) = picked?;
    if declared > max_declared {
        return None;
    }
    let mut entry = archive.by_index(index).ok()?;
    let mut data = Vec::with_capacity(usize::try_from(declared).unwrap_or(0).min(MAX_IMAGE_BYTES));
    // 多读 1 字节：读到条目真正结束（返回 0）zip 才会核对 CRC。声明大小说谎（压缩炸弹 / 截断）时长度对不上，
    // CRC 不符同样当作失败
    (&mut entry)
        .take(declared + 1)
        .read_to_end(&mut data)
        .ok()?;
    if data.len() as u64 != declared {
        return None;
    }
    Some(data)
}

/// 缩略图尺寸：长边缩到 [`THUMB_SIDE`]、保持比例、不放大。返回 `None` 表示不用缩（两边都没超过上限）。
/// 现场生成、不落库，舍入方向以本函数为准。
fn thumbnail_size(width: u32, height: u32) -> Option<(u32, u32)> {
    if width <= THUMB_SIDE && height <= THUMB_SIDE {
        return None;
    }
    let scale = |short: u32, long: u32| -> u32 {
        (f64::from(short) * f64::from(THUMB_SIDE) / f64::from(long))
            .round()
            .max(1.0) as u32
    };
    Some(if width >= height {
        (THUMB_SIDE, scale(height, width))
    } else {
        (scale(width, height), THUMB_SIDE)
    })
}

/// JPEG 是否完整：第一个 SOS 之后还能找到 EOI（`FF D9`；熵编码数据里的 `FF` 一定被填充成 `FF 00`，不会冒出 `FF D9`）。
///
/// image crate 用的 zune-jpeg 对截断的文件很宽容（缺的部分补灰，照样返回一张图），
/// 截断的文件在这里显式判掉：缩略图返回 `None` 而不是补灰的图。
fn jpeg_is_complete(data: &[u8]) -> bool {
    let mut i = 2; // 跳过 SOI
    while i + 4 <= data.len() {
        if data[i] != 0xFF {
            return false;
        }
        let marker = data[i + 1];
        if marker == 0xFF {
            i += 1; // 填充字节
            continue;
        }
        let len = usize::from(u16::from_be_bytes([data[i + 2], data[i + 3]]));
        if marker == 0xDA {
            return data
                .get(i + 2 + len..)
                .is_some_and(|scan| scan.windows(2).any(|w| w == [0xFF, 0xD9]));
        }
        i += 2 + len;
    }
    false
}

/// 解码成 RGB：先看头部像素数（超过上限不解码），再完整解码（截断 / 损坏在这里暴露）。
fn decode_rgb(data: &[u8]) -> Option<image::RgbImage> {
    let probe = ImageReader::new(Cursor::new(data))
        .with_guessed_format()
        .ok()?;
    let format = probe.format()?;
    if !matches!(
        format,
        ImageFormat::Png | ImageFormat::Jpeg | ImageFormat::WebP
    ) {
        return None;
    }
    let (w, h) = probe.into_dimensions().ok()?;
    if format == ImageFormat::Jpeg && !jpeg_is_complete(data) {
        return None;
    }
    if w == 0 || h == 0 || u64::from(w) * u64::from(h) > MAX_IMAGE_PIXELS {
        return None;
    }
    let mut reader = ImageReader::with_format(Cursor::new(data), format);
    let mut limits = Limits::default();
    limits.max_image_width = Some(w);
    limits.max_image_height = Some(h);
    limits.max_alloc = Some(1 << 30);
    reader.limits(limits);
    // 与 Pillow 的 convert("RGB") 一样：带透明通道的图只丢掉 alpha，不合成背景
    Some(reader.decode().ok()?.to_rgb8())
}

/// 从上游返回的 zip（或裸图片）里取第一张图，缩成小 JPEG；失败返回 `None`。
pub fn make_thumbnail(payload: &[u8]) -> Option<Vec<u8>> {
    let data: Cow<[u8]> = if payload.starts_with(b"PK") {
        Cow::Owned(first_zip_image(payload, MAX_THUMB_ZIP_ENTRY)?)
    } else {
        Cow::Borrowed(payload)
    };
    let mut rgb = decode_rgb(&data)?;
    if let Some((w, h)) = thumbnail_size(rgb.width(), rgb.height())
        && (w, h) != rgb.dimensions()
    {
        rgb = image::imageops::resize(&rgb, w, h, FilterType::CatmullRom);
    }
    let mut out = Vec::new();
    DynamicImage::ImageRgb8(rgb)
        .write_with_encoder(JpegEncoder::new_with_quality(&mut out, THUMB_QUALITY))
        .ok()?;
    Some(out)
}

/// 取上游返回的第一张图的原始字节（不缩放、不重压），超过大小上限则不存。返回 `(字节, content_type)`。
///
/// 不检查内容是不是图片：类型串按魔数猜，猜不出就是 `image/png`；失败时是 `(None, "")`。
pub fn full_image(payload: &[u8]) -> (Option<Vec<u8>>, String) {
    let data: Cow<[u8]> = if payload.starts_with(b"PK") {
        match first_zip_image(payload, MAX_IMAGE_BYTES as u64) {
            Some(d) => Cow::Owned(d),
            None => return (None, String::new()),
        }
    } else {
        Cow::Borrowed(payload)
    };
    if data.len() > MAX_IMAGE_BYTES {
        return (None, String::new());
    }
    let ctype = if data.starts_with(PNG_MAGIC) {
        "image/png"
    } else if data.starts_with(b"\xff\xd8\xff") {
        "image/jpeg"
    } else if data.starts_with(b"RIFF") && data.get(8..12) == Some(b"WEBP") {
        "image/webp"
    } else {
        "image/png"
    };
    (Some(data.into_owned()), ctype.to_string())
}

// ----------------------------------------------------------------------------- 设置读取

/// `site_flags.get(db, spec)`（不带 settings）：读一行设置，按登记表解析，缺失 / 格式不对回到默认值。
/// 站点设置的读取统一走这里（`site_value` / `site_int` / `site_f64` / `site_bool`），不要各处再写一遍。
pub(crate) async fn site_value(db: &Db, spec: &site_flags::Spec) -> Result<Value, DbError> {
    let raw = db.get_setting(spec.key).await?;
    Ok(site_flags::get_from_raw(spec, raw.as_deref()))
}

/// 数值读成整数：`number_like` 后向零取整，读不懂为 0。
pub(crate) fn int_of(v: &Value) -> i64 {
    pyval::number_like(v).map_or(0, |f| f.trunc() as i64)
}

/// 整数类设置（登记表里是 Int 的 spec，解析后一定是整数，缺失时是登记表的默认值，所以和 `as_i64` 结果相同）。
pub(crate) async fn site_int(db: &Db, spec: &site_flags::Spec) -> Result<i64, DbError> {
    Ok(int_of(&site_value(db, spec).await?))
}

/// 数值型设置；读不懂为 0。
pub(crate) async fn site_f64(db: &Db, spec: &site_flags::Spec) -> Result<f64, DbError> {
    Ok(pyval::number_like(&site_value(db, spec).await?).unwrap_or(0.0))
}

/// 开关类设置：按 Python 的真值判断。
pub(crate) async fn site_bool(db: &Db, spec: &site_flags::Spec) -> Result<bool, DbError> {
    Ok(pyval::truthy(&site_value(db, spec).await?))
}

/// 原图在服务器上保存几小时（0 = 永久）。
pub async fn audit_image_keep_hours(db: &Db) -> Result<i64, DbError> {
    site_int(db, &site_flags::IMAGE_KEEP_HOURS).await
}

/// 用量日志里来源 IP 和客户端保留几天（1～365）。
pub async fn usage_origin_keep_days(db: &Db) -> Result<i64, DbError> {
    site_int(db, &site_flags::ORIGIN_KEEP_DAYS).await
}

/// 原图占用上限（MB，0 = 不限）。
pub async fn audit_image_cap_mb(db: &Db) -> Result<i64, DbError> {
    site_int(db, &site_flags::IMAGE_CAP_MB).await
}

/// 布尔设置的取值：`str(value).strip().lower() in ("1", "true", "yes", "on")`。
fn truth(raw: &str) -> bool {
    matches!(
        raw.trim().to_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

/// 没在后台设置过时的记录开关与保留天数（2.x 取环境变量 `AUDIT_PROMPTS` / `AUDIT_THUMBS` / `AUDIT_RETENTION_DAYS`，
/// 3.0 删除，只剩后台设置）：默认都不记，提示词保留 7 天。
pub const DEFAULT_PROMPTS: bool = false;
pub const DEFAULT_THUMBS: bool = false;
pub const DEFAULT_RETENTION_DAYS: i64 = 7;

/// 记录开关（提示词、生成结果 / 原图、提示词保留天数）：后台保存的设置，没设过用上面的默认值。
/// 第二项沿用旧设置名 `audit_thumbs`，含义已变为「保存原图」。
///
/// 存着的值只要有这一行（哪怕是空串）就压过默认值；天数 0 = 长期保留，范围 0–3650。
pub async fn audit_flags(db: &Db) -> Result<(bool, bool, i64), DbError> {
    let prompts = match db.get_setting("audit_prompts").await? {
        Some(raw) => truth(&raw),
        None => DEFAULT_PROMPTS,
    };
    let thumbs = match db.get_setting("audit_thumbs").await? {
        Some(raw) => truth(&raw),
        None => DEFAULT_THUMBS,
    };
    let days = match db.get_setting("audit_retention_days").await? {
        // 能解析成有限数就向零取整并夹到 0–3650；空串 / nan / inf / 乱码一律回到 7
        Some(raw) => match pyc::parse_float(&raw) {
            Some(f) if f.is_finite() => clamp_days(f.trunc()),
            _ => 7,
        },
        None => DEFAULT_RETENTION_DAYS,
    };
    Ok((prompts, thumbs, days))
}

/// `max(0, min(days, 3650))`（`days` 已向零取整）。
fn clamp_days(days: f64) -> i64 {
    days.clamp(0.0, 3650.0) as i64
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn prompts_basic_shapes() {
        let body = json!({
            "input": "1girl, solo",
            "parameters": {"negative_prompt": "lowres", "seed": 7, "scale": 5.0, "sm": false, "steps": null}
        });
        let (p, n, e) = capture_prompts(&body).unwrap();
        assert_eq!((p.as_str(), n.as_str()), ("1girl, solo", "lowres"));
        assert_eq!(
            e,
            r#"{"char_prompts": [], "char_negatives": [], "params": {"seed": 7, "scale": 5.0, "sm": false}}"#
        );
        // 复现参数和角色坐标
        let body = json!({
            "input": "2girls",
            "parameters": {
                "skip_cfg_above_sigma": 19, "uncond_scale": 1.0, "legacy": false, "use_coords": true,
                "params_version": 3, "prefer_brownian": true, "legacy_uc": null,
                "v4_prompt": {"caption": {"base_caption": "2girls", "char_captions": [
                    {"char_caption": "a", "centers": [{"x": 0.1, "y": 0.5}]},
                    {"char_caption": "", "centers": [{"x": 0.9, "y": 0.9}]},
                    {"char_caption": "b"}
                ]}},
                "v4_negative_prompt": {"caption": {"base_caption": "", "char_captions": [
                    {"char_caption": "c"}
                ]}}
            }
        });
        let (_, _, e) = capture_prompts(&body).unwrap();
        let v: Value = serde_json::from_str(&e).unwrap();
        assert_eq!(v["char_prompts"], json!(["a", "b"]));
        assert_eq!(v["char_centers"], json!([[{"x": 0.1, "y": 0.5}], []]));
        assert!(v.get("char_negative_centers").is_none());
        assert_eq!(
            v["params"],
            json!({"skip_cfg_above_sigma": 19, "uncond_scale": 1.0, "legacy": false,
                   "prefer_brownian": true, "params_version": 3, "use_coords": true})
        );
        // 什么都没有：extra 是空串
        assert_eq!(
            capture_prompts(&json!({"input": "x"})).unwrap(),
            ("x".into(), String::new(), String::new())
        );
        // 字段类型不对：`Err`，整条记录被丢
        let err = capture_prompts(&json!({"parameters": {"v4_prompt": {"caption": "oops"}}}))
            .unwrap_err();
        assert_eq!(err.type_name(), "AttributeError");
        assert_eq!(err.message(), "'str' object has no attribute 'get'");
        let err = capture_prompts(
            &json!({"parameters": {"v4_prompt": {"caption": {"char_captions": 5}}}}),
        )
        .unwrap_err();
        assert_eq!(err.type_name(), "TypeError");
        assert!(capture_prompts(&json!([1])).is_err());
    }

    #[test]
    fn thumbnail_sizes() {
        assert_eq!(thumbnail_size(768, 768), None);
        assert_eq!(thumbnail_size(100, 50), None);
        assert_eq!(thumbnail_size(769, 769), Some((768, 768)));
        assert_eq!(thumbnail_size(1000, 700), Some((768, 538)));
        assert_eq!(thumbnail_size(3000, 100), Some((768, 26)));
        assert_eq!(thumbnail_size(1, 30000), Some((1, 768)));
        assert_eq!(thumbnail_size(30000, 1), Some((768, 1)));
        assert_eq!(thumbnail_size(999, 1001), Some((766, 768)));
    }

    #[test]
    fn full_image_types_and_limits() {
        assert_eq!(full_image(b""), (Some(vec![]), "image/png".into()));
        assert_eq!(
            full_image(b"\xff\xd8\xffabc"),
            (Some(b"\xff\xd8\xffabc".to_vec()), "image/jpeg".into())
        );
        assert_eq!(full_image(b"RIFF\0\0\0\0WEBPx").1, "image/webp");
        assert_eq!(full_image(b"RIFF\0\0\0\0WAVEx").1, "image/png");
        assert_eq!(full_image(b"PKnot a zip"), (None, String::new()));
        let big = vec![0u8; MAX_IMAGE_BYTES + 1];
        assert_eq!(full_image(&big), (None, String::new()));
        assert!(full_image(&big[..MAX_IMAGE_BYTES]).0.is_some());
    }

    fn png(w: u32, h: u32, rgb: [u8; 3]) -> Vec<u8> {
        let mut out = Cursor::new(Vec::new());
        DynamicImage::ImageRgb8(image::RgbImage::from_pixel(w, h, image::Rgb(rgb)))
            .write_to(&mut out, ImageFormat::Png)
            .unwrap();
        out.into_inner()
    }

    fn jpeg(w: u32, h: u32, rgb: [u8; 3]) -> Vec<u8> {
        let mut out = Vec::new();
        DynamicImage::ImageRgb8(image::RgbImage::from_pixel(w, h, image::Rgb(rgb)))
            .write_with_encoder(JpegEncoder::new_with_quality(&mut out, 90))
            .unwrap();
        out
    }

    fn zip_of(entries: &[(&str, &[u8])]) -> Vec<u8> {
        use std::io::Write;
        let mut buf = Cursor::new(Vec::new());
        let mut z = zip::ZipWriter::new(&mut buf);
        for (name, data) in entries {
            z.start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            z.write_all(data).unwrap();
        }
        z.finish().unwrap();
        buf.into_inner()
    }

    fn thumb_size(payload: &[u8]) -> Option<(u32, u32)> {
        let t = make_thumbnail(payload)?;
        assert!(t.starts_with(b"\xff\xd8\xff"), "缩略图是 JPEG");
        let img = image::load_from_memory_with_format(&t, ImageFormat::Jpeg).unwrap();
        Some((img.width(), img.height()))
    }

    #[test]
    fn thumbnail_decisions_on_synthetic_images() {
        // 小图不放大；大图按比例缩到 768 以内
        assert_eq!(thumb_size(&png(8, 8, [200, 30, 30])), Some((8, 8)));
        assert_eq!(thumb_size(&png(1600, 800, [30, 200, 30])), Some((768, 384)));
        assert_eq!(
            thumb_size(&jpeg(900, 1800, [30, 30, 200])),
            Some((384, 768))
        );
        // zip：按后缀挑第一个图片条目（不分大小写、不看目录），按内容解码
        let (a, b) = (png(16, 16, [1, 2, 3]), png(32, 32, [4, 5, 6]));
        let z = zip_of(&[("notes.txt", b"hi"), ("sub/IMG.PNG", &a), ("b.png", &b)]);
        assert_eq!(thumb_size(&z), Some((16, 16)));
        assert_eq!(thumb_size(&zip_of(&[("a.gif", &a)])), None);
        assert_eq!(thumb_size(&zip_of(&[("a.png", b"garbage")])), None);
        assert_eq!(thumb_size(&zip_of(&[])), None);
        // JPEG：EOI 之后有多余字节照收；截断不收（zune-jpeg 本身会补灰）
        let j = jpeg(300, 200, [90, 90, 90]);
        let mut padded = j.clone();
        padded.extend_from_slice(&[0, 0, 0, 0]);
        assert_eq!(thumb_size(&padded), Some((300, 200)));
        assert_eq!(thumb_size(&j[..j.len() / 2]), None);
        assert_eq!(thumb_size(&j[..j.len() - 2]), None);
        // 损坏 / 不是图片
        let p = png(64, 64, [9, 9, 9]);
        assert_eq!(thumb_size(&p[..p.len() / 2]), None);
        for junk in [
            &b""[..],
            b"x",
            b"PK",
            b"PK\x03\x04junk",
            &[0x89, b'P', b'N', b'G'],
            b"GIF89a",
        ] {
            assert_eq!(thumb_size(junk), None);
        }
    }

    #[test]
    fn full_image_extracts_first_zip_entry_without_decoding() {
        let (a, b) = (png(16, 16, [1, 2, 3]), jpeg(16, 16, [4, 5, 6]));
        let z = zip_of(&[("readme.txt", b"hi"), ("1.JPG", &b), ("2.png", &a)]);
        assert_eq!(full_image(&z), (Some(b.clone()), "image/jpeg".into()));
        assert_eq!(
            full_image(&zip_of(&[("2.png", &a)])),
            (Some(a), "image/png".into())
        );
        // 不检查内容：后缀对就原样存
        assert_eq!(
            full_image(&zip_of(&[("a.webp", b"not really")])),
            (Some(b"not really".to_vec()), "image/png".into())
        );
        assert_eq!(
            full_image(&zip_of(&[("a.txt", b"x")])),
            (None, String::new())
        );
        assert_eq!(full_image(&zip_of(&[])), (None, String::new()));
        assert_eq!(full_image(b"PK\x03\x04junk"), (None, String::new()));
    }
}
