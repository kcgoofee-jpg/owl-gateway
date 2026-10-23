//! 图片工具的边界与上游结果校验。
//!
//! - `dimensions`：真正解码（不只看文件头），先查尺寸再分配内存；只认静态 PNG / JPEG / WebP；
//! - `validate_result`：上游出图结果必须是指定张数的完整图片（zip 里不许有目录、加密、路径分隔符），
//!   在内存里校验，绝不按 zip 里的路径解压；
//! - `prepare_tool`：放大 / 导演工具的参数与费用按官方前端计算，不信客户端给的尺寸或费用。
//!
//! 小图送导演工具前的放大由 image crate 完成（尺寸、费用、拒绝条件与上游网页端一致）。

use base64::Engine;
use image::{ImageFormat, ImageReader, Limits};
use serde_json::{Map, Value, json};
use std::io::{Cursor, Read};

pub const MAX_PIXELS: u64 = 3_145_728;
pub const MAX_RESPONSE_BYTES: usize = 64 * 1024 * 1024;
pub const UPSCALE_MODEL: &str = "nai-diffusion-5-curated";
pub const DIRECTOR_TOOLS: [&str; 7] = [
    "bg-removal",
    "lineart",
    "sketch",
    "colorize",
    "emotion",
    "declutter",
    "declutter-keep-bubbles",
];

const BAD_IMAGE: &str = "图片无法读取，仅支持 PNG、JPEG、WebP 静态图，未扣费。请更换图片。";
const TOO_BIG: &str = "图片过大，最多约 314 万像素，例如 2048×1536，未扣费。请缩小图片。";

fn is_animated(raw: &[u8], format: ImageFormat) -> bool {
    match format {
        ImageFormat::Png => image::codecs::png::PngDecoder::new(Cursor::new(raw))
            .is_ok_and(|d| d.is_apng().unwrap_or(false)),
        ImageFormat::WebP => {
            image::codecs::webp::WebPDecoder::new(Cursor::new(raw)).is_ok_and(|d| d.has_animation())
        }
        _ => false,
    }
}

/// 解码并返回 (宽, 高)。`output=true` 时像素上限放宽到 4 倍（上游输出可以比输入大）。
pub fn dimensions(raw: &[u8], output: bool) -> Result<(u32, u32), String> {
    let reader = ImageReader::new(Cursor::new(raw))
        .with_guessed_format()
        .map_err(|_| BAD_IMAGE.to_string())?;
    let format = reader.format().ok_or_else(|| BAD_IMAGE.to_string())?;
    if !matches!(
        format,
        ImageFormat::Png | ImageFormat::Jpeg | ImageFormat::WebP
    ) {
        return Err(TOO_BIG.into());
    }
    let (w, h) = reader
        .into_dimensions()
        .map_err(|_| BAD_IMAGE.to_string())?;
    let limit = if output { MAX_PIXELS * 4 } else { MAX_PIXELS };
    if w.min(h) < 1
        || w.max(h) > 8192
        || u64::from(w) * u64::from(h) > limit
        || is_animated(raw, format)
    {
        return Err(TOO_BIG.into());
    }
    // 完整解码一次（与 Pillow 的 img.load() 一样，截断 / 损坏的数据在这里暴露）
    let mut reader = ImageReader::with_format(Cursor::new(raw), format);
    let mut limits = Limits::default();
    limits.max_image_width = Some(8192);
    limits.max_image_height = Some(8192);
    limits.max_alloc = Some(limit * 8);
    reader.limits(limits);
    reader.decode().map_err(|_| BAD_IMAGE.to_string())?;
    Ok((w, h))
}

fn media_type(raw: &[u8]) -> &'static str {
    if raw.starts_with(b"\x89PNG") {
        "image/png"
    } else if raw.starts_with(b"\xff\xd8") {
        "image/jpeg"
    } else {
        "image/webp"
    }
}

/// 校验上游结果；返回 (媒体类型, 张数)。`expected` 为 None 时：去背景 3 张，其余 1 张。
pub fn validate_result(
    raw: &[u8],
    tool: &str,
    expected: Option<usize>,
) -> Result<(&'static str, usize), String> {
    if raw.is_empty() || raw.len() > MAX_RESPONSE_BYTES {
        return Err("上游图片结果为空或过大".into());
    }
    let expected = expected.unwrap_or(if tool == "bg-removal" { 3 } else { 1 });
    if !raw.starts_with(b"PK") {
        dimensions(raw, true)?;
        if expected != 1 {
            return Err("上游未返回完整数量的图片结果".into());
        }
        return Ok((media_type(raw), 1));
    }
    let bad = || "上游图片压缩包损坏".to_string();
    let mut archive = zip::ZipArchive::new(Cursor::new(raw)).map_err(|_| bad())?;
    let mut total: u64 = 0;
    let mut names = Vec::new();
    for i in 0..archive.len() {
        let e = archive.by_index_raw(i).map_err(|_| bad())?;
        let name = e.name().map_err(|_| bad())?.to_string();
        if e.is_dir()
            || e.encrypted()
            || name.contains('/')
            || name.contains('\\')
            || name.contains(':')
            || name == "."
            || name == ".."
        {
            return Err("上游压缩包内容或大小无效".into());
        }
        total += e.size();
        names.push(i);
    }
    if names.len() != expected || total > MAX_RESPONSE_BYTES as u64 {
        return Err("上游压缩包内容或大小无效".into());
    }
    for i in names {
        let mut e = archive.by_index(i).map_err(|_| bad())?;
        let mut buf = Vec::with_capacity(e.size().min(MAX_RESPONSE_BYTES as u64) as usize);
        (&mut e)
            .take(MAX_RESPONSE_BYTES as u64 + 1)
            .read_to_end(&mut buf)
            .map_err(|_| bad())?;
        dimensions(&buf, true)?;
    }
    Ok(("application/zip", expected))
}

fn int_field(body: &Map<String, Value>, name: &str) -> Option<Option<i64>> {
    // Some(None)：字段存在但不是 int（type(x) is not int，bool 也不算）
    body.get(name).map(gate_core::pyval::strict_int)
}

/// 放大 / 导演工具：校验参数并按官方规则算 Anlas。返回 (发给上游的请求体, 费用)。
pub fn prepare_tool(body: &Value, operation: &str) -> Result<(Value, i64), String> {
    let empty = Map::new();
    let b = body.as_object().unwrap_or(&empty);
    let image = match b.get("image") {
        Some(Value::String(s)) if !s.is_empty() && s.len() <= 24 * 1024 * 1024 => s.clone(),
        _ => {
            return Err(
                "图片无法读取，未扣费。请更换图片。给客户端作者：image 须为有效的 base64 图片"
                    .into(),
            );
        }
    };
    let raw = base64::engine::general_purpose::STANDARD
        .decode(&image)
        .map_err(|_| {
            "图片无法读取，未扣费。请更换图片。给客户端作者：image 须为有效的 base64 图片"
                .to_string()
        })?;
    let (width, height) = dimensions(&raw, false)?;
    if width.min(height) < 64 {
        return Err("图片过小，宽和高至少 64 像素，未扣费。请更换图片。".into());
    }
    for (name, actual) in [("width", width), ("height", height)] {
        if let Some(v) = int_field(b, name)
            && v != Some(i64::from(actual))
        {
            return Err("声明的图片尺寸与实际不一致，未扣费。请重新添加图片。给客户端作者：width / height 须与图片实际尺寸一致".into());
        }
    }
    let pixels = u64::from(width) * u64::from(height);
    if operation == "upscale" {
        let model_ok = b
            .get("model")
            .is_none_or(|m| m.as_str() == Some(UPSCALE_MODEL));
        let scale_ok = b
            .get("scale")
            .is_none_or(|s| gate_core::pyval::strict_int(s) == Some(2));
        let blur_ok = b
            .get("declared_blur_sigma")
            .is_none_or(|s| gate_core::pyval::strict_number(s) == Some(0.0));
        if !(model_ok && scale_ok && blur_ok) {
            return Err("放大仅支持 2 倍默认设置，未扣费。请在客户端中恢复默认值。给客户端作者：只支持官方当前放大模型、scale=2、declared_blur_sigma=0".into());
        }
        let cost = [
            (1_048_576u64, 1),
            (1_747_627, 2),
            (2_446_678, 3),
            (MAX_PIXELS, 4),
        ]
        .iter()
        .find(|(p, _)| pixels <= *p)
        .map(|(_, c)| *c)
        .unwrap_or(4);
        return Ok((
            json!({"image": image, "model": UPSCALE_MODEL, "declared_blur_sigma": 0}),
            cost,
        ));
    }
    let tool = match b.get("req_type").and_then(Value::as_str) {
        Some(t) if DIRECTOR_TOOLS.contains(&t) => t.to_string(),
        _ => return Err("本站不支持该导演工具，未扣费。请改用线稿、上色等官方工具。给客户端作者：req_type 不在支持列表里".into()),
    };
    let prompt = match b.get("prompt") {
        None => String::new(),
        Some(Value::String(s)) if s.len() <= 8192 => s.clone(),
        _ => {
            return Err(
                "导演工具提示词过长：上限 8192 字节，每个汉字计 3 字节，未扣费。请缩短提示词。"
                    .into(),
            );
        }
    };
    let mut payload = Map::new();
    payload.insert("image".into(), json!(image));
    payload.insert("width".into(), json!(width));
    payload.insert("height".into(), json!(height));
    payload.insert("req_type".into(), json!(tool));
    if tool == "colorize" || tool == "emotion" {
        let defry = match b.get("defry") {
            None => 0,
            Some(v) => match gate_core::pyval::strict_int(v) {
                Some(d) if (0..=5).contains(&d) => d,
                _ => {
                    return Err(
                        "去瑕疵 defry 须为 0 到 5 的整数，未扣费。请在客户端中修改。".into(),
                    );
                }
            },
        };
        payload.insert("prompt".into(), json!(prompt));
        payload.insert("defry".into(), json!(defry));
    }
    let (mut w, mut h) = (u64::from(width), u64::from(height));
    // 与官方页面一样：小图先放大到 Normal 再处理（这也让去背景有 65 Anlas 的最低价）
    if pixels < 1_011_712 {
        let ratio = (1_048_576f64 / pixels as f64).sqrt();
        w = (width as f64 * ratio).floor() as u64;
        h = (height as f64 * ratio).floor() as u64;
        if w.max(h) > 8192 {
            return Err("图片长宽比过大，未扣费。请裁剪后重试。".into());
        }
        let img = image::load_from_memory(&raw).map_err(|_| BAD_IMAGE.to_string())?;
        let resized = img.resize_exact(w as u32, h as u32, image::imageops::FilterType::Lanczos3);
        let mut png = Cursor::new(Vec::new());
        resized
            .write_to(&mut png, ImageFormat::Png)
            .map_err(|_| BAD_IMAGE.to_string())?;
        payload.insert(
            "image".into(),
            json!(base64::engine::general_purpose::STANDARD.encode(png.into_inner())),
        );
        payload.insert("width".into(), json!(w));
        payload.insert("height".into(), json!(h));
    }
    let base = 2i64.max(
        ((2.951823174884865e-6 + 5.753298233447344e-7 * 28.0) * w as f64 * h as f64).ceil() as i64,
    );
    let cost = if tool == "bg-removal" {
        3 * base + 5
    } else if w * h <= 1_048_576 {
        0
    } else {
        base
    };
    Ok((Value::Object(payload), cost))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(w: u32, h: u32) -> Vec<u8> {
        let img = image::RgbImage::new(w, h);
        let mut out = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(img)
            .write_to(&mut out, ImageFormat::Png)
            .unwrap();
        out.into_inner()
    }

    #[test]
    fn validates_results() {
        let zip = gate_testkit_zip(2);
        assert_eq!(
            validate_result(&zip, "", Some(2)).unwrap(),
            ("application/zip", 2)
        );
        assert!(validate_result(&zip, "", Some(1)).is_err());
        assert_eq!(
            validate_result(&png(64, 64), "", None).unwrap(),
            ("image/png", 1)
        );
        assert!(validate_result(b"PKgarbage", "", None).is_err());
        assert!(validate_result(b"", "", None).is_err());
        let mut truncated = png(64, 64);
        truncated.truncate(truncated.len() - 20);
        assert!(validate_result(&truncated, "", None).is_err());
    }

    fn gate_testkit_zip(n: usize) -> Vec<u8> {
        use std::io::Write;
        let mut buf = Cursor::new(Vec::new());
        let mut z = zip::ZipWriter::new(&mut buf);
        for i in 0..n {
            z.start_file(
                format!("image_{i}.png"),
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
            z.write_all(&png(64, 64)).unwrap();
        }
        z.finish().unwrap();
        buf.into_inner()
    }

    #[test]
    fn upscale_cost_table() {
        let b64 = base64::engine::general_purpose::STANDARD.encode(png(1024, 1024));
        let (p, cost) = prepare_tool(&json!({"image": b64}), "upscale").unwrap();
        assert_eq!(cost, 1);
        assert_eq!(p["model"], UPSCALE_MODEL);
        assert!(prepare_tool(&json!({"image": b64, "scale": 4}), "upscale").is_err());
        assert!(prepare_tool(&json!({"image": b64, "width": 1000}), "upscale").is_err());
    }

    #[test]
    fn director_small_image_expands_and_prices() {
        let b64 = base64::engine::general_purpose::STANDARD.encode(png(512, 512));
        let (p, cost) = prepare_tool(
            &json!({"image": b64, "req_type": "bg-removal"}),
            "augment-image",
        )
        .unwrap();
        assert_eq!(
            (p["width"].as_u64(), p["height"].as_u64()),
            (Some(1024), Some(1024))
        );
        // base = max(2, ceil((2.9518e-6 + 5.7533e-7*28) * 1048576)) = 20（Python 实算）→ 3*20+5
        assert_eq!(cost, 65);
        let (_, c2) = prepare_tool(
            &json!({"image": b64, "req_type": "lineart"}),
            "augment-image",
        )
        .unwrap();
        assert_eq!(c2, 0);
        assert!(
            prepare_tool(
                &json!({"image": b64, "req_type": "colorize", "defry": 9}),
                "augment-image"
            )
            .is_err()
        );
    }
}
