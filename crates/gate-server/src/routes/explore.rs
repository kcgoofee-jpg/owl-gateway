//! 「上传到 Explore」的落点。
//!
//! 官网工作台里那个按钮本身只是 `<a href="/explore/register" target="_blank">`（没登录 Explore
//! 就跳注册页，登录了才走 Explore 上传弹窗）。我们的镜像没有 Explore 站，所以桥接层（`bridge.js`）
//! 把它改成：取出画布上这张**原图**（webp/png 原始字节，官方 EXIF 元数据都在里面），
//! POST 到这里的 `/owl/explore`，由机器人把图和提交者一起发到站长指定的 Discord 频道。
//!
//! 约定：
//! - 必须是网页登录的成员（成员会话 Cookie），游客一律 401；
//! - 每位成员 20 秒内最多一次，避免刷屏；
//! - 只接受 webp / png / jpeg，按文件头判断，不认前端给的 MIME；
//! - 图片按原始字节转发（multipart 附件），外加一条 embed：提交者 / 时间 / 尺寸 / 提示词。

use crate::error::{ApiError, ApiResult, HttpError, InternalError, ok_json};
use crate::multipart::read_bounded_body;
use crate::state::AppState;
use axum::Router;
use axum::extract::{Request, State};
use axum::http::HeaderMap;
use axum::routing::post;
use bytes::Bytes;
use serde_json::{Value, json};

/// 一张原图最多 32MB（4.5 / V5 的 1664×2432 PNG 也在 10MB 量级）。
const MAX_UPLOAD_BYTES: usize = 32 * 1024 * 1024;
/// 同一位成员的冷却（秒）。
const COOLDOWN_SECONDS: f64 = 20.0;
const MAX_FIELDS: usize = 8;
const MAX_FIELD_BYTES: usize = 8 * 1024;

pub fn router() -> Router<AppState> {
    Router::new().route("/owl/explore", post(upload))
}

/// 收到的图片：原始字节 + 供 Discord 用的文件名与 MIME。
struct Upload {
    bytes: Bytes,
    file_name: String,
    mime: &'static str,
}

/// 按文件头认格式，只认这三种。
fn sniff(bytes: &[u8]) -> Option<&'static str> {
    if bytes.len() > 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return Some("image/webp");
    }
    if bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]) {
        return Some("image/png");
    }
    if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        return Some("image/jpeg");
    }
    None
}

fn ext_of(mime: &str) -> &'static str {
    match mime {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        _ => "webp",
    }
}

fn clean_file_name(raw: Option<&str>, mime: &str) -> String {
    let fallback = format!("owl-explore.{}", ext_of(mime));
    let Some(raw) = raw else {
        return fallback;
    };
    let trimmed: String = raw
        .chars()
        .filter(|c| !matches!(c, '/' | '\\' | '"' | '\n' | '\r' | '\0'))
        .take(80)
        .collect();
    let trimmed = trimmed.trim();
    if trimmed.is_empty() {
        fallback
    } else if trimmed.contains('.') {
        trimmed.to_string()
    } else {
        format!("{trimmed}.{}", ext_of(mime))
    }
}

/// 从原图里读生成参数：官网出的 PNG 带 `tEXt` 元数据（`Comment` 是完整参数 JSON，`Software` 是模型名）。
/// 读到了就用图里的值——比前端从 DOM 里抓的尺寸/种子可信。
fn png_meta(bytes: &[u8]) -> Option<Value> {
    if bytes.len() < 8 || &bytes[0..8] != b"\x89PNG\r\n\x1a\n" {
        return None;
    }
    let mut pos = 8usize;
    let mut comment: Option<String> = None;
    let mut software: Option<String> = None;
    while pos + 8 <= bytes.len() {
        let len = u32::from_be_bytes(bytes[pos..pos + 4].try_into().ok()?) as usize;
        let kind = &bytes[pos + 4..pos + 8];
        let start = pos + 8;
        let end = start.checked_add(len)?;
        if end + 4 > bytes.len() {
            break;
        }
        let data = &bytes[start..end];
        match kind {
            b"tEXt" | b"iTXt" => {
                let mut parts = data.splitn(3, |b| *b == 0);
                let key = parts.next().unwrap_or_default();
                let value = if kind == b"tEXt" {
                    parts.next().unwrap_or_default()
                } else {
                    // iTXt：压缩标志 + 压缩方法 + 语言 + 译文 + 正文，NAI 写的是不压缩
                    let rest = parts.next().unwrap_or_default();
                    rest.iter()
                        .position(|b| *b == 0)
                        .and_then(|_| rest.get(2..))
                        .map(|tail| {
                            let mut it = tail.splitn(3, |b| *b == 0);
                            it.nth(2).unwrap_or_default()
                        })
                        .unwrap_or_default()
                };
                let text = String::from_utf8_lossy(value).to_string();
                match key {
                    b"Comment" if comment.is_none() => comment = Some(text),
                    b"Software" if software.is_none() => software = Some(text),
                    _ => {}
                }
            }
            b"IEND" => break,
            _ => {}
        }
        pos = end + 4;
    }
    let parsed: Option<Value> = comment
        .as_deref()
        .and_then(|text| serde_json::from_str::<Value>(text).ok());
    if parsed.is_none() && software.is_none() {
        return None;
    }
    let mut out = parsed.unwrap_or_else(|| json!({}));
    if let Some(sw) = software
        && out.get("software").is_none()
    {
        out["software"] = json!(sw);
    }
    Some(out)
}

async fn upload(State(st): State<AppState>, headers: HeaderMap, req: Request) -> ApiResult {
    let Some((discord_id, key)) = crate::session::member_state(&st, &headers).await? else {
        return Err(HttpError::new(401, "请先在网页登录，再上传到 Explore。").into());
    };
    if st.env.explore_channel_id.trim().is_empty() || st.env.discord_bot_token.trim().is_empty() {
        return Err(InternalError("站点还没配置上传频道，暂时无法提交。".into()).into());
    }
    let bucket = key.as_ref().map(crate::key::Key::id).unwrap_or_default();
    if !st
        .rpm
        .check_then_push(&bucket, 1, COOLDOWN_SECONDS, st.now())
    {
        return Err(HttpError::new(
            429,
            format!("提交太快了，{COOLDOWN_SECONDS:.0} 秒后再试一次。"),
        )
        .into());
    }

    let (parts, body) = req.into_parts();
    let content_type = parts
        .headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let raw = read_bounded_body(&parts.headers, body, MAX_UPLOAD_BYTES).await?;
    let (image, meta) = parse_form(&content_type, raw).await?;
    let mime = sniff(&image.bytes)
        .ok_or_else(|| HttpError::new(400, "只支持 webp / png / jpg 图片，未提交。"))?;

    let upload = Upload {
        bytes: image.bytes,
        file_name: clean_file_name(image.file_name.as_deref(), mime),
        mime,
    };
    let mut meta = meta;
    if let Some(inner) = png_meta(&upload.bytes) {
        let mut put = |key: &str, value: String| {
            if !value.trim().is_empty() {
                meta.insert(key.to_string(), value);
            }
        };
        put(
            "prompt",
            inner
                .get("prompt")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        );
        let (w, h) = (
            inner.get("width").and_then(Value::as_u64),
            inner.get("height").and_then(Value::as_u64),
        );
        if let (Some(w), Some(h)) = (w, h) {
            put("size", format!("{w} × {h}"));
        }
        if let Some(seed) = inner.get("seed").and_then(Value::as_u64) {
            put("seed", seed.to_string());
        }
        put(
            "model",
            inner
                .get("software")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        );
    }
    let uploader = st
        .db
        .discord_profile_row(&discord_id)
        .await
        .ok()
        .flatten()
        .map(|(username, display_name, _)| {
            display_name
                .filter(|d| !d.trim().is_empty())
                .or(username)
                .unwrap_or_default()
        })
        .unwrap_or_default();
    let message_id = post_to_discord(&st, &discord_id, &uploader, &upload, &meta).await?;
    tracing::info!(
        target: "explore",
        "Explore 上传：{}（{}）{} 字节，msg={}",
        &discord_id,
        upload.mime,
        upload.bytes.len(),
        message_id
    );
    Ok(ok_json(&json!({ "ok": true, "message_id": message_id })))
}

/// multipart 里的一项。
struct Part {
    file_name: Option<String>,
    bytes: Bytes,
}

/// 极简 multipart：一个图片文件 + 若干短文本字段。
async fn parse_form(
    content_type: &str,
    raw: Bytes,
) -> Result<(Part, std::collections::HashMap<String, String>), HttpError> {
    let boundary = multer::parse_boundary(content_type)
        .map_err(|_| HttpError::new(400, "上传格式错误，未提交。"))?;
    let stream = futures::stream::once(std::future::ready(Ok::<Bytes, std::convert::Infallible>(
        raw,
    )));
    let mut multipart = multer::Multipart::new(stream, boundary);
    let mut image: Option<Part> = None;
    let mut meta = std::collections::HashMap::new();
    while let Some(mut field) = multipart
        .next_field()
        .await
        .map_err(|_| HttpError::new(400, "上传格式错误，未提交。"))?
    {
        let name = field.name().unwrap_or_default().to_string();
        let file_name = field.file_name().map(str::to_string);
        let mut buf = Vec::new();
        while let Some(chunk) = field
            .chunk()
            .await
            .map_err(|_| HttpError::new(400, "上传中断，未提交。"))?
        {
            if buf.len() + chunk.len() > MAX_UPLOAD_BYTES {
                return Err(HttpError::new(413, "图片太大，未提交。"));
            }
            buf.extend_from_slice(&chunk);
        }
        if file_name.is_some() || name == "image" {
            if image.is_some() {
                return Err(HttpError::new(400, "一次只能提交一张图片，未提交。"));
            }
            image = Some(Part {
                file_name,
                bytes: Bytes::from(buf),
            });
            continue;
        }
        if meta.len() >= MAX_FIELDS || buf.len() > MAX_FIELD_BYTES {
            continue;
        }
        if let Ok(text) = String::from_utf8(buf) {
            meta.insert(name, text);
        }
    }
    let image = image.ok_or_else(|| HttpError::new(400, "没有收到图片，未提交。"))?;
    Ok((image, meta))
}

fn clip(value: &str, max: usize) -> String {
    let trimmed = value.trim();
    if trimmed.chars().count() <= max {
        return trimmed.to_string();
    }
    let mut out: String = trimmed.chars().take(max).collect();
    out.push('…');
    out
}

/// 采样器内部名 → 界面上的叫法（只覆盖常见的，其余原样显示）。
fn sampler_label(raw: &str) -> String {
    match raw.trim() {
        "k_euler_ancestral" => "Euler Ancestral".into(),
        "k_euler" => "Euler".into(),
        "k_dpmpp_2m" => "DPM++ 2M".into(),
        "k_dpmpp_2s_ancestral" => "DPM++ 2S Ancestral".into(),
        "k_dpmpp_sde" => "DPM++ SDE".into(),
        "ddim" => "DDIM".into(),
        other => other.to_string(),
    }
}

/// 卡片上的模型名用短名：优先前端报的界面标签（`V4.5 Full`），拿不到就从元数据里认出世代。
fn model_label(meta: &std::collections::HashMap<String, String>) -> String {
    let ui = meta.get("model").map(|s| s.trim()).unwrap_or_default();
    if !ui.is_empty() {
        return ui.to_string();
    }
    let software = meta
        .get("software")
        .map(|s| s.to_lowercase())
        .unwrap_or_default();
    if software.contains("v4.5") {
        "V4.5".into()
    } else if software.contains("v5") {
        "V5".into()
    } else if software.contains("v4") {
        "V4".into()
    } else if software.contains("v3") {
        "V3".into()
    } else {
        String::new()
    }
}

fn human_size(bytes: usize) -> String {
    let kb = bytes as f64 / 1024.0;
    if kb >= 1024.0 {
        format!("{:.1} MB", kb / 1024.0)
    } else if kb >= 10.0 {
        format!("{kb:.0} KB")
    } else {
        format!("{kb:.1} KB")
    }
}

async fn post_to_discord(
    st: &AppState,
    discord_id: &str,
    uploader: &str,
    upload: &Upload,
    meta: &std::collections::HashMap<String, String>,
) -> Result<String, ApiError> {
    let channel = st.env.explore_channel_id.trim();
    let url = format!(
        "{}/channels/{channel}/messages",
        st.env.discord_api_base.trim_end_matches('/')
    );
    let get = |key: &str| {
        meta.get(key)
            .map(|s| s.trim().to_string())
            .unwrap_or_default()
    };

    // 提交者：@名字（拿不到名字才退回 id），单独占一行
    let who = if uploader.trim().is_empty() {
        format!("@{discord_id}")
    } else {
        format!("@{}", clip(uploader, 40))
    };
    let mut fields = vec![json!({"name": "提交者", "value": who, "inline": false})];

    let model = model_label(meta);
    if !model.is_empty() {
        fields.push(json!({"name": "模型", "value": clip(&model, 60), "inline": true}));
    }

    let size = get("size");
    if !size.is_empty() {
        fields.push(json!({"name": "尺寸", "value": clip(&size, 40), "inline": true}));
    }

    // 生成参数：步数 · 采样器 · 引导 · 噪声表
    let mut params: Vec<String> = Vec::new();
    let steps = get("steps");
    if !steps.is_empty() {
        params.push(format!("{steps} 步"));
    }
    let sampler = get("sampler");
    if !sampler.is_empty() {
        params.push(sampler_label(&sampler));
    }
    let scale = get("scale");
    if !scale.is_empty() {
        params.push(format!("Guidance {scale}"));
    }
    let noise = get("noise_schedule");
    if !noise.is_empty() {
        params.push(noise);
    }
    if !params.is_empty() {
        fields.push(
            json!({"name": "参数", "value": clip(&params.join(" · "), 200), "inline": false}),
        );
    }

    let seed = get("seed");
    if !seed.is_empty() {
        fields.push(json!({"name": "种子", "value": format!("`{seed}`"), "inline": true}));
    }

    let format = match upload.mime {
        "image/png" => "PNG",
        "image/jpeg" => "JPG",
        _ => "WebP",
    };
    fields.push(json!({
        "name": "原图",
        "value": format!("{format} · {}", human_size(upload.bytes.len())),
        "inline": true
    }));

    // 今天全站出到第几张：收进底部那行小字（读不到就只留站名）
    let footer = match st.db.day_images_total(&st.day()).await {
        Ok(total) if total > 0 => format!("猫头鹰公益站 · 今日全站第 {total} 张"),
        _ => "猫头鹰公益站".to_string(),
    };

    let embed = json!({
        "color": 0x6b4bd1u32,
        "fields": fields,
        "image": {"url": format!("attachment://{}", upload.file_name)},
        "footer": {"text": footer},
    });
    let payload = json!({
        "embeds": [embed],
        "allowed_mentions": {"parse": []},
    });

    let part = reqwest::multipart::Part::bytes(upload.bytes.to_vec())
        .file_name(upload.file_name.clone())
        .mime_str(upload.mime)
        .map_err(|e| InternalError(format!("构造附件失败：{e}")))?;
    let form = reqwest::multipart::Form::new()
        .text("payload_json", payload.to_string())
        .part("files[0]", part);

    let http = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .user_agent(gate_core::discord_ua::DISCORD_USER_AGENT)
        .build()
        .map_err(|e| InternalError(format!("初始化上传客户端失败：{e}")))?;
    let resp = http
        .post(&url)
        .header(
            reqwest::header::AUTHORIZATION,
            format!("Bot {}", st.env.discord_bot_token.trim()),
        )
        .multipart(form)
        .send()
        .await
        .map_err(|e| InternalError(format!("提交到 Discord 失败：{e}")))?;
    let status = resp.status();
    let body: Value = resp.json().await.unwrap_or(Value::Null);
    if !status.is_success() {
        tracing::error!(target: "explore", "Discord 上传失败 {status}: {}", clip(&body.to_string(), 400));
        return Err(InternalError(format!(
            "提交到 Discord 失败（HTTP {}），请稍后重试。",
            status.as_u16()
        ))
        .into());
    }
    Ok(body
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sniffs_supported_formats_only() {
        assert_eq!(sniff(b"RIFF\0\0\0\0WEBPVP8 "), Some("image/webp"));
        assert_eq!(
            sniff(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0, 0]),
            Some("image/png")
        );
        assert_eq!(sniff(&[0xff, 0xd8, 0xff, 0xe0]), Some("image/jpeg"));
        assert_eq!(sniff(b"GIF89a"), None);
    }

    #[test]
    fn model_label_stays_short() {
        let mut meta = std::collections::HashMap::new();
        meta.insert("software".into(), "NovelAI Diffusion V4.5 4BDE2A90".into());
        meta.insert("model".into(), "V4.5 Full".into());
        // 前端报了标签就用标签
        assert_eq!(model_label(&meta), "V4.5 Full");

        // 只有元数据时按世代取短名，不把哈希带上
        meta.remove("model");
        assert_eq!(model_label(&meta), "V4.5");

        meta.clear();
        meta.insert("software".into(), "NovelAI Diffusion V5 0ADF9AB7".into());
        assert_eq!(model_label(&meta), "V5");

        meta.clear();
        assert_eq!(model_label(&meta), "");
    }

    #[test]
    fn file_sizes_read_naturally() {
        assert_eq!(human_size(486), "0.5 KB");
        assert_eq!(human_size(51_200), "50 KB");
        assert_eq!(human_size(1_600_000), "1.5 MB");
    }

    #[test]
    fn sampler_names_are_pretty_printed() {
        assert_eq!(sampler_label("k_euler_ancestral"), "Euler Ancestral");
        assert_eq!(sampler_label("k_dpmpp_2m"), "DPM++ 2M");
        assert_eq!(sampler_label("something_new"), "something_new");
    }

    #[test]
    fn file_names_keep_extensions_and_drop_paths() {
        assert_eq!(clean_file_name(None, "image/webp"), "owl-explore.webp");
        assert_eq!(clean_file_name(Some("a/b.webp"), "image/webp"), "ab.webp");
        assert_eq!(
            clean_file_name(Some("explore-upload"), "image/png"),
            "explore-upload.png"
        );
    }

    /// 拼一个只带 tEXt 的最小 PNG（不追求能解码，只验证元数据解析）。
    fn png_with_text(chunks: &[(&str, &str)]) -> Vec<u8> {
        let mut out = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
        for (key, value) in chunks {
            let mut data = Vec::new();
            data.extend_from_slice(key.as_bytes());
            data.push(0);
            data.extend_from_slice(value.as_bytes());
            out.extend_from_slice(&(data.len() as u32).to_be_bytes());
            out.extend_from_slice(b"tEXt");
            out.extend_from_slice(&data);
            out.extend_from_slice(&[0, 0, 0, 0]);
        }
        out.extend_from_slice(&0u32.to_be_bytes());
        out.extend_from_slice(b"IEND");
        out.extend_from_slice(&[0, 0, 0, 0]);
        out
    }

    #[test]
    fn reads_generation_params_from_png_metadata() {
        let comment = r#"{"prompt":"1girl, bakery","width":832,"height":1216,"seed":4048844816}"#;
        let png = png_with_text(&[("Comment", comment), ("Software", "NovelAI")]);
        let meta = png_meta(&png).expect("metadata");
        assert_eq!(meta["prompt"], "1girl, bakery");
        assert_eq!(meta["width"], 832);
        assert_eq!(meta["height"], 1216);
        assert_eq!(meta["seed"], 4048844816u64);
        assert_eq!(meta["software"], "NovelAI");
    }

    #[test]
    fn ignores_images_without_metadata() {
        assert!(png_meta(b"RIFF    WEBP").is_none());
        assert!(png_meta(&png_with_text(&[("Author", "someone")])).is_none());
    }
}
