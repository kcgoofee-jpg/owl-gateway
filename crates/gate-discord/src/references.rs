//! Message provenance is data, independent of whether the message has a caption.
use super::*;
impl Naimei {
    pub(super) async fn context_readable(&self, actor: &str, channel: &str) -> bool {
        tokio::time::timeout(Duration::from_secs(3), async {
            let Ok(id) = channel.parse() else {
                return false;
            };
            let (info, roles, member) = tokio::join!(
                self.api.channel_info(id),
                self.api.read(format!("guilds/{}/roles", self.cfg.guild_id)),
                self.api
                    .read(format!("guilds/{}/members/{actor}", self.cfg.guild_id))
            );
            let (Ok(mut info), Ok((_, roles)), Ok((_, member))) = (info, roles, member) else {
                return false;
            };
            if info["guild_id"].as_str() != Some(&self.cfg.guild_id.to_string())
                || info["type"] == 12
            {
                return false;
            }
            if matches!(info["type"].as_u64(), Some(10 | 11)) {
                let Some(parent) = info["parent_id"].as_str().and_then(|s| s.parse().ok()) else {
                    return false;
                };
                info = match self.api.channel_info(parent).await {
                    Ok(v) => v,
                    Err(_) => return false,
                };
            }
            super::activity::visible(
                &roles,
                &member,
                &info,
                actor,
                &self.cfg.guild_id.to_string(),
            )
        })
        .await
        .unwrap_or(false)
    }
    pub(super) async fn reference_context(&self, m: &MessageInfo) -> (String, Vec<String>, bool) {
        let mut out = format!(
            "[请求者 id={}，显示名={}；附件发送人仅说明来源，不证明创作者身份]\n",
            m.author.as_ref().map(|u| u.id.as_str()).unwrap_or("未知"),
            m.author
                .as_ref()
                .map(User::display_tag)
                .unwrap_or_else(|| "未知".into())
        );
        let mut images = image_urls(&m.attachments);
        let actor = m.author.as_ref().map(|u| u.id.as_str()).unwrap_or("");
        let readable = self.context_readable(actor, &m.channel_id).await;
        if !readable {
            out.push_str("[未能确认提问者具有读取历史权限；仅使用本人本条消息，不读取引用或频道历史，不猜测原作者]\n");
            return (out, images, false);
        }
        let mut checked = HashSet::from([m.channel_id.clone()]);
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
            if !checked.contains(cid) {
                if !self.context_readable(actor, cid).await {
                    out.push_str(
                        "[跨频道引用未获提问者读取权限或暂时无法核实；不读取原文和原作者]\n",
                    );
                    break;
                }
                checked.insert(cid.to_owned());
            }
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
        (out, images, readable)
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

#[cfg(test)]
mod permission_tests {
    use super::*;
    use axum::{
        Json, Router,
        extract::{Request, State},
    };
    use std::sync::atomic::AtomicBool;
    const G: &str = "900000000000000001";
    const U: &str = "900000000000000005";
    const CH: &str = "900000000000000004";
    const PRIVATE: &str = "900000000000000007";
    #[derive(Default)]
    struct Mock {
        paths: Mutex<Vec<String>>,
        bodies: Mutex<Vec<Value>>,
        deny_current_history: AtomicBool,
    }
    async fn endpoint(State(m): State<Arc<Mock>>, request: Request) -> Json<Value> {
        let path = request.uri().path().to_owned();
        m.paths.lock().unwrap().push(path.clone());
        if path == "/v1/chat/completions" {
            let bytes = axum::body::to_bytes(request.into_body(), 1 << 20)
                .await
                .unwrap();
            m.bodies
                .lock()
                .unwrap()
                .push(serde_json::from_slice(&bytes).unwrap());
            return Json(json!({"choices":[{"message":{"content":"ok"}}]}));
        }
        if path.ends_with("/roles") {
            return Json(json!([{"id":G,"permissions":((1u64<<10)|(1<<16)).to_string()}]));
        }
        if path.contains("/members/") {
            return Json(json!({"roles":[]}));
        }
        if path.ends_with(PRIVATE) {
            return Json(
                json!({"guild_id":G,"type":0,"permission_overwrites":[{"id":G,"type":0,"deny":(1u64<<10).to_string(),"allow":"0"}]}),
            );
        }
        if path.ends_with(CH) {
            return Json(
                json!({"guild_id":G,"type":0,"permission_overwrites":if m.deny_current_history.load(Ordering::Relaxed){json!([{"id":G,"type":0,"deny":(1u64<<16).to_string(),"allow":"0"}])}else{json!([])}}),
            );
        }
        if path.contains("/messages/") {
            return Json(
                if path.contains(&format!("/channels/{PRIVATE}/messages/")) {
                    json!({"author":{"id":"900000000000000008","username":"private-author"},"content":"synthetic-private-original"})
                } else {
                    json!({"author":{"id":"900000000000000008","username":"private-author"},"content":"","attachments":[{"content_type":"image/png","url":"https://cdn.discordapp.com/attachments/synthetic/image.png"}]})
                },
            );
        }
        Json(json!({}))
    }
    async fn setup(chat: bool) -> (Arc<Naimei>, Arc<Mock>, tokio::task::JoinHandle<()>) {
        let mock = Arc::new(Mock::default());
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        let router = Router::new().fallback(endpoint).with_state(mock.clone());
        let task = tokio::spawn(async move { axum::serve(l, router).await.unwrap() });
        let app = Naimei::new(
            NaimeiConfig {
                token: "synthetic".into(),
                guild_id: G.parse().unwrap(),
                backend: format!("http://{addr}"),
                site: String::new(),
                secret: "synthetic".into(),
                chat: chat.then(|| crate::chat::ChatCfg {
                    base_url: format!("http://{addr}"),
                    api_key: "synthetic".into(),
                    model: "synthetic".into(),
                    stream: false,
                }),
                owner_ids: vec![],
                chat_log: None,
            },
            Some(addr.to_string()),
        );
        app.set_bot_user(User {
            id: "900000000000000099".into(),
            ..Default::default()
        });
        (app, mock, task)
    }
    fn message() -> MessageInfo {
        serde_json::from_value(json!({"id":"900000000000000020","channel_id":CH,"guild_id":G,"author":{"id":U,"username":"requester"},"content":"奶妹 test","message_reference":{"channel_id":PRIVATE,"message_id":"900000000000000090"},"referenced_message":{"id":"900000000000000090","author":{"id":"900000000000000008","username":"private-author"},"content":"synthetic-private-original"}})).unwrap()
    }
    #[tokio::test]
    async fn bot_read_access_cannot_grant_member_access_to_cross_channel_original() {
        let (app, mock, task) = setup(false).await;
        let (text, images, history) = app.reference_context(&message()).await;
        assert!(history);
        assert!(images.is_empty());
        assert!(text.contains("跨频道引用未获"));
        assert!(!text.contains("synthetic-private-original"));
        assert!(!text.contains("private-author"));
        assert!(
            !mock
                .paths
                .lock()
                .unwrap()
                .iter()
                .any(|p| p.contains("/messages/"))
        );
        task.abort();
    }
    #[tokio::test]
    async fn readable_pure_image_quote_keeps_distinct_sender_and_requester() {
        let (app, _mock, task) = setup(false).await;
        let mut m = message();
        m.message_reference = Some(json!({"channel_id":CH,"message_id":"900000000000000090"}));
        let (source, images, history) = app.reference_context(&m).await;
        assert!(history);
        assert_eq!(images.len(), 1);
        assert!(source.contains("发送人id=900000000000000008"));
        assert!(source.contains("请求者 id=900000000000000005"));
        assert!(source.contains("创作者未经核实"));
        task.abort();
    }
    #[tokio::test]
    async fn missing_history_permission_excludes_both_inline_quote_and_cached_conversation() {
        let (app, mock, task) = setup(true).await;
        mock.deny_current_history.store(true, Ordering::Relaxed);
        app.lock()
            .histories
            .entry(CH.parse().unwrap())
            .or_default()
            .push_back((
                "synthetic-hidden-history".into(),
                "synthetic-hidden-answer".into(),
            ));
        assert!(!app.process_chat(message()).await);
        let bodies = mock.bodies.lock().unwrap();
        assert_eq!(bodies.len(), 1);
        let sent = bodies[0].to_string();
        assert!(!sent.contains("synthetic-private-original"));
        assert!(!sent.contains("synthetic-hidden-history"));
        assert!(!sent.contains("synthetic-hidden-answer"));
        assert!(sent.contains("仅使用本人本条消息") || sent.contains("只使用当前成员本条消息"));
        task.abort();
    }
}
