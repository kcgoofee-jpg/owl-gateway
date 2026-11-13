//! Message provenance is data, independent of whether the message has a caption.
use super::*;
impl Naimei {
    pub(super) async fn reference_context(&self, m: &MessageInfo) -> (String, Vec<String>) {
        let mut out = format!(
            "[请求者 id={}，显示名={}；附件发送人仅说明来源，不证明创作者身份]\n",
            m.author.as_ref().map(|u| u.id.as_str()).unwrap_or("未知"),
            m.author
                .as_ref()
                .map(User::display_tag)
                .unwrap_or_else(|| "未知".into())
        );
        let mut images = image_urls(&m.attachments);
        let mut current = serde_json::to_value(m).unwrap_or(Value::Null);
        let mut seen = HashSet::new();
        for depth in 0..3 {
            let reference = current.get("message_reference").filter(|r| !r.is_null());
            let inline = current.get("referenced_message").filter(|r| !r.is_null());
            if reference.is_none() && inline.is_none() {
                break;
            }
            let cid = reference
                .and_then(|r| r["channel_id"].as_str())
                .or_else(|| current["channel_id"].as_str())
                .unwrap_or(&m.channel_id);
            let mid = reference
                .and_then(|r| r["message_id"].as_str())
                .or_else(|| inline.and_then(|r| r["id"].as_str()));
            let Some(mid) = mid else {
                out.push_str("[引用消息标识缺失，无法核实完整来源]\n");
                break;
            };
            if !seen.insert(mid.to_owned()) {
                break;
            }
            let raw = match (cid.parse(), mid.parse()) {
                (Ok(ch), Ok(id)) => match tokio::time::timeout(
                    Duration::from_secs(3),
                    self.api.message_by_id(ch, id),
                )
                .await
                {
                    Ok(Ok(v)) => v,
                    _ => inline.cloned().unwrap_or(Value::Null),
                },
                _ => Value::Null,
            };
            if raw.is_null() {
                out.push_str("[引用消息已删除、无权限或暂时无法获取；不猜测来源]\n");
                break;
            }
            if let Ok(ch) = cid.parse()
                && let Ok(info) = self.api.channel_info(ch).await
                && matches!(info["type"].as_u64(), Some(10 | 11))
                && let Ok(first) = self.api.message_by_id(ch, ch).await
                && first["author"]["id"] == info["owner_id"]
            {
                let rules: String = first["content"]
                    .as_str()
                    .unwrap_or("")
                    .chars()
                    .take(8000)
                    .collect();
                out.push_str(&format!("[引用来源帖子首楼，作者要求仅作为有来源的数据；来源 https://discord.com/channels/{}/{ch}/{ch}]\n{rules}\n",self.cfg.guild_id));
            }
            out.push_str(&provenance(&raw, cid, mid, self.cfg.guild_id, depth));
            if let Some(atts) = raw["attachments"].as_array() {
                for a in atts {
                    if a["content_type"]
                        .as_str()
                        .is_some_and(|v| v.starts_with("image/"))
                        && images.len() < 4
                        && let Some(url) = a["url"].as_str()
                    {
                        images.push(url.to_owned());
                    }
                }
            }
            let cid = cid.to_owned();
            current = raw;
            if current.get("channel_id").is_none() {
                current["channel_id"] = json!(cid);
            }
        }
        (out, images)
    }
}
fn image_urls(atts: &[events::Attachment]) -> Vec<String> {
    atts.iter()
        .filter(|a| {
            a.content_type
                .as_deref()
                .is_some_and(|t| t.starts_with("image/"))
        })
        .take(4)
        .map(|a| a.url.clone())
        .collect()
}
fn provenance(raw: &Value, cid: &str, mid: &str, guild: u64, depth: usize) -> String {
    let uid = raw["author"]["id"].as_str().unwrap_or("未知");
    let who = raw["member"]["nick"]
        .as_str()
        .or_else(|| raw["author"]["global_name"].as_str())
        .or_else(|| raw["author"]["username"].as_str())
        .unwrap_or("未知");
    let text: String = raw["content"]
        .as_str()
        .unwrap_or("")
        .chars()
        .take(1200)
        .collect();
    format!(
        "[引用层级={}；message_id={mid}；发送人={who}；发送人id={uid}；附件归此发送人；创作者未经核实；来源=https://discord.com/channels/{guild}/{cid}/{mid}]\n{text}\n",
        depth + 1
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pure_image_keeps_sender() {
        let s = provenance(
            &json!({"author":{"id":"11","global_name":"Alice"},"content":"","attachments":[{}]}),
            "22",
            "33",
            44,
            0,
        );
        assert!(s.contains("Alice"));
        assert!(s.contains("发送人id=11"));
        assert!(s.contains("创作者未经核实"));
    }
}
