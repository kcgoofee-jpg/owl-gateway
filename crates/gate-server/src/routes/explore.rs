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

/// 北京时间（站点时区固定 UTC+8，和后台日志一致）。
fn beijing_now() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_default();
    let local = secs + 8 * 3600;
    let (days, rem) = (local.div_euclid(86_400), local.rem_euclid(86_400));
    let (h, m, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    // 1970-01-01 起的天数 → 年月日（Howard Hinnant 的 civil_from_days）
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m2 = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m2 <= 2 { y + 1 } else { y };
    format!("{y:04}-{m2:02}-{d:02} {h:02}:{m:02}:{s:02}（北京时间）")
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
    let prompt = meta.get("prompt").map(String::as_str).unwrap_or_default();
    let size = meta.get("size").map(String::as_str).unwrap_or_default();
    let model = meta.get("model").map(String::as_str).unwrap_or_default();
    let seed = meta.get("seed").map(String::as_str).unwrap_or_default();

    let who = if uploader.trim().is_empty() {
        format!("<@{discord_id}>（`{discord_id}`）")
    } else {
        format!("<@{discord_id}>（`{discord_id}` · {}）", clip(uploader, 40))
    };
    let mut fields = vec![json!({"name": "提交者", "value": who, "inline": true})];
    fields.push(json!({"name": "时间", "value": beijing_now(), "inline": true}));
    if !size.trim().is_empty() {
        fields.push(json!({"name": "尺寸", "value": clip(size, 40), "inline": true}));
    }
    if !model.trim().is_empty() || !seed.trim().is_empty() {
        let value = [clip(model, 40), seed.trim().to_string()]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(" · seed ");
        fields.push(json!({"name": "模型 / 种子", "value": value, "inline": true}));
    }

    let embed = json!({
        "title": "生图工作台 · 新作品",
        "description": if prompt.trim().is_empty() {
            "（没有取到提示词，原图元数据里还有）".to_string()
        } else {
            clip(prompt, 1500)
        },
        "color": 0x6b4bd1u32,
        "fields": fields,
        "image": {"url": format!("attachment://{}", upload.file_name)},
        "footer": {"text": "猫头鹰公益站 · 上传到 Explore"},
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
    fn file_names_keep_extensions_and_drop_paths() {
        assert_eq!(clean_file_name(None, "image/webp"), "owl-explore.webp");
        assert_eq!(clean_file_name(Some("a/b.webp"), "image/webp"), "ab.webp");
        assert_eq!(
            clean_file_name(Some("explore-upload"), "image/png"),
            "explore-upload.png"
        );
    }

    #[test]
    fn beijing_timestamp_is_formatted() {
        let now = beijing_now();
        assert!(now.contains("北京时间"), "{now}");
        assert_eq!(now.chars().filter(|c| *c == '-').count(), 2, "{now}");
    }
}
