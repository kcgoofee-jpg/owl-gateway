//! 存图频道里算不算「发了一张图」。
//!
//! 新开的帖子、以及已有帖子里后补的内容，都看同一件事：这条消息（或它转发来的原消息）
//! 带没带图片。纯文字、机器人自己发的，都不算。

use crate::events::{Attachment, MessageInfo};

/// 「存图备份专用」。
pub const FORUM_CHANNEL_ID: &str = "1558068804934705222";

const IMAGE_EXT: &[&str] = &["png", "jpg", "jpeg", "webp", "gif", "avif"];

pub fn has_image(m: &MessageInfo) -> bool {
    m.attachments.iter().any(attachment_is_image)
        || m.message_snapshots.iter().any(|snap| {
            snap.message.as_ref().is_some_and(|body| {
                body.attachments.iter().any(attachment_is_image)
                    || body.embeds.iter().any(embed_is_image)
            })
        })
}

fn attachment_is_image(a: &Attachment) -> bool {
    if a.content_type.as_deref().is_some_and(|raw| {
        let mime = raw.split(';').next().unwrap_or(raw).trim();
        mime.starts_with("image/")
    }) {
        return true;
    }
    let ext = a.filename.rsplit('.').next().unwrap_or("");
    IMAGE_EXT.iter().any(|e| ext.eq_ignore_ascii_case(e))
}

/// Discord 自己的图片 embed，或转发里指向 Discord 图床的图。外链预览不算。
fn embed_is_image(v: &serde_json::Value) -> bool {
    if v.get("type").and_then(|t| t.as_str()) == Some("image") {
        return true;
    }
    v.get("image")
        .and_then(|image| image.get("url"))
        .and_then(|url| url.as_str())
        .is_some_and(|url| {
            url.contains("cdn.discordapp.com/") || url.contains("media.discordapp.net/")
        })
}

/// Discord 雪花 id 里的创建时间（Unix 秒）。纪元是 2015-01-01T00:00:00Z。
pub fn snowflake_unix(id: &str) -> Option<f64> {
    let id: u64 = id.parse().ok()?;
    let ms = (id >> 22) + 1_420_070_400_000;
    Some(ms as f64 / 1000.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn message(extra: serde_json::Value) -> MessageInfo {
        let mut base = json!({
            "id": "900000000000000081",
            "channel_id": "900000000000000082",
            "author": {"id": "900000000000000005", "username": "alice"},
        });
        let obj = base.as_object_mut().unwrap();
        for (k, v) in extra.as_object().unwrap() {
            obj.insert(k.clone(), v.clone());
        }
        serde_json::from_value(base).unwrap()
    }

    #[test]
    fn image_attachment_and_forward_count_text_does_not() {
        assert!(!has_image(&message(json!({}))));
        assert!(!has_image(&message(json!({"content": "只有文字"}))));
        assert!(has_image(&message(json!({
            "attachments": [{"id": "1", "filename": "a.png", "content_type": "image/png", "url": "https://cdn.example/a.png"}]
        }))));
        assert!(has_image(&message(json!({
            "attachments": [{"id": "1", "filename": "shot.JPG", "url": "https://cdn.example/a.jpg"}]
        }))));
        assert!(!has_image(&message(json!({
            "attachments": [{"id": "1", "filename": "note.txt", "content_type": "text/plain", "url": "https://cdn.example/n.txt"}]
        }))));
        assert!(has_image(&message(json!({
            "message_snapshots": [{"message": {"attachments": [
                {"id": "2", "filename": "fwd.webp", "content_type": "image/webp", "url": "https://cdn.example/f.webp"}
            ]}}]
        }))));
        assert!(has_image(&message(json!({
            "message_snapshots": [{"message": {"embeds": [
                {"type": "image", "url": "https://cdn.discordapp.com/attachments/1/2/a.png"}
            ]}}]
        }))));
        assert!(!has_image(&message(json!({
            "message_snapshots": [{"message": {"embeds": [
                {"type": "article", "thumbnail": {"url": "https://example.com/t.jpg"}}
            ]}}]
        }))));
    }

    #[test]
    fn snowflake_time_uses_the_discord_epoch() {
        // 早一小时会让刚发出的图被网关当成「未来时间」丢掉。
        let at = snowflake_unix("175928847299117063").unwrap();
        assert!((at - 1_462_015_105.796).abs() < 0.001);
        assert!(snowflake_unix("nope").is_none());
    }
}
