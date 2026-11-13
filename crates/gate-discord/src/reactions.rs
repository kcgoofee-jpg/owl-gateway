//! Acknowledgement is optional and never part of the share commit transaction.
use super::*;
use serde::{Deserialize, Serialize};
#[derive(Clone, Serialize, Deserialize)]
struct Pending {
    channel: String,
    next: f64,
    attempt: u32,
}
#[derive(Default, Serialize, Deserialize)]
pub(super) struct Reactions {
    since: f64,
    pending: HashMap<String, Pending>,
    #[serde(default)]
    done: HashSet<String>,
    #[serde(default)]
    permanently_failed: usize,
    error: Option<String>,
    #[serde(default)]
    pub enabled: bool,
    #[serde(skip)]
    path: Option<std::path::PathBuf>,
}
impl Reactions {
    pub fn load() -> Self {
        let path = std::env::var_os("NAIMEI_QUEUE_FILE")
            .map(std::path::PathBuf::from)
            .map(|p| p.with_file_name("reactions.json"));
        let mut r = path
            .as_ref()
            .and_then(|p| std::fs::read(p).ok())
            .and_then(|v| serde_json::from_slice::<Self>(&v).ok())
            .unwrap_or_default();
        r.path = path;
        let enabled = std::env::var("NAIMEI_REACTIONS").as_deref() == Ok("1");
        if enabled && !r.enabled {
            r.since = unix_now();
        }
        r.enabled = enabled;
        if r.since == 0.0 {
            r.since = unix_now();
        }
        r.done.retain(|id| {
            share::snowflake_unix(id).is_some_and(|at| at >= unix_now() - 2.0 * 86400.0)
        });
        r.save();
        r
    }
    fn save(&mut self) {
        if let Some(p) = &self.path {
            let result = (|| -> std::io::Result<()> {
                if let Some(d) = p.parent() {
                    std::fs::create_dir_all(d)?;
                }
                let tmp = p.with_extension("tmp");
                let mut opts = std::fs::OpenOptions::new();
                opts.create(true).truncate(true).write(true);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::OpenOptionsExt;
                    opts.mode(0o600);
                }
                use std::io::Write;
                let mut f = opts.open(&tmp)?;
                f.write_all(&serde_json::to_vec(self)?)?;
                f.sync_all()?;
                std::fs::rename(tmp, p)?;
                Ok(())
            })();
            if result.is_err() {
                self.error = Some("反应队列保存失败".into());
            }
        }
    }
}
impl Naimei {
    pub(super) fn reaction_committed(&self, p: &Value) {
        let mut r = self
            .reactions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !r.enabled || p["at"].as_f64().unwrap_or(0.0) < r.since {
            return;
        }
        if let (Some(id), Some(ch)) = (p["message_id"].as_str(), p["channel_id"].as_str()) {
            if r.done.contains(id) {
                return;
            }
            r.pending.entry(id.into()).or_insert(Pending {
                channel: ch.into(),
                next: 0.0,
                attempt: 0,
            });
            r.save();
        }
    }
    pub fn reaction_status(&self) -> Value {
        let r = self
            .reactions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        json!({"enabled":r.enabled,"pending":r.pending.len(),"error":r.error,"since":r.since,"permanently_failed":r.permanently_failed})
    }
    pub(super) async fn reaction_loop(self: Arc<Self>) {
        loop {
            let next = {
                let r = self
                    .reactions
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if r.enabled {
                    r.pending
                        .iter()
                        .find(|(_, p)| p.next <= self.now())
                        .map(|(id, p)| (id.clone(), p.clone()))
                } else {
                    None
                }
            };
            if let Some((id, p)) = next {
                let result =
                    tokio::time::timeout(Duration::from_secs(8), self.api.react(&p.channel, &id))
                        .await;
                let mut r = self
                    .reactions
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                match result {
                    Ok(Ok(())) => {
                        r.pending.remove(&id);
                        r.done.insert(id.clone());
                        if r.pending.is_empty() && r.permanently_failed == 0 {
                            r.error = None;
                        }
                    }
                    Ok(Err(e)) if matches!(e.status(), Some(401 | 403 | 404)) => {
                        r.pending.remove(&id);
                        r.done.insert(id.clone());
                        r.permanently_failed += 1;
                        r.error=Some("部分确认反应无法发送（权限、鉴权或消息已删除），停止重复请求；分享和解锁不受影响".into());
                    }
                    _ => {
                        r.error = Some("反应发送失败，分享记录与解锁不受影响".into());
                        if let Some(p) = r.pending.get_mut(&id) {
                            p.attempt = p.attempt.saturating_add(1);
                            p.next = self.now() + f64::from((1u32 << p.attempt.min(8)).min(300));
                        }
                    }
                }
                r.save();
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        Json, Router,
        extract::{Request, State},
        http::StatusCode,
        response::IntoResponse,
    };
    use std::sync::atomic::{AtomicU16, AtomicUsize};
    struct Mock {
        shares: AtomicUsize,
        reactions: AtomicUsize,
        status: AtomicU16,
    }
    async fn endpoint(State(m): State<Arc<Mock>>, r: Request) -> impl IntoResponse {
        let path = r.uri().path();
        if path == "/self-register/bot/share" {
            m.shares.fetch_add(1, Ordering::Relaxed);
            return (
                StatusCode::from_u16(m.status.load(Ordering::Relaxed)).unwrap(),
                Json(json!({"ok":true})),
            );
        }
        if path.contains("/reactions/") {
            m.reactions.fetch_add(1, Ordering::Relaxed);
            return (
                StatusCode::FORBIDDEN,
                Json(json!({"code":50013,"message":"Missing Permissions"})),
            );
        }
        (
            StatusCode::OK,
            Json(json!({"parent_id":share::FORUM_CHANNEL_ID})),
        )
    }
    #[tokio::test]
    async fn confirmation_failure_cannot_undo_commit_or_poison_share_health() {
        let mock = Arc::new(Mock {
            shares: AtomicUsize::new(0),
            reactions: AtomicUsize::new(0),
            status: AtomicU16::new(200),
        });
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        let router = Router::new().fallback(endpoint).with_state(mock.clone());
        let server = tokio::spawn(async move { axum::serve(l, router).await.unwrap() });
        const NOW: f64 = 1_800_000_000.0;
        let cfg = NaimeiConfig {
            token: "synthetic".into(),
            guild_id: 900000000000000001,
            backend: format!("http://{addr}"),
            site: String::new(),
            secret: "synthetic".into(),
            chat: None,
            owner_ids: vec![],
            chat_log: None,
        };
        let app = Naimei::with_clock(cfg, Some(addr.to_string()), Box::new(|| NOW));
        {
            let mut r = app.reactions.lock().unwrap();
            r.enabled = true;
            r.since = NOW - 1.0;
        }
        let mid = (((NOW * 1000.0) as u64 - 1420070400000) << 22).to_string();
        let message:MessageInfo=serde_json::from_value(json!({"id":mid,"channel_id":"900000000000000082","guild_id":"900000000000000001","author":{"id":"900000000000000005"},"attachments":[{"filename":"a.png","content_type":"image/png"}]})).unwrap();
        app.handle_message(message.clone()).await;
        app.handle_message(message).await;
        assert_eq!(mock.shares.load(Ordering::Relaxed), 1);
        assert_eq!(app.reaction_status()["pending"], 1);
        let task = tokio::spawn(app.clone().reaction_loop());
        tokio::time::timeout(Duration::from_secs(2), async {
            while app.reaction_status()["permanently_failed"] != 1 {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(app.reaction_status()["pending"], 0);
        assert_eq!(app.share_detection_report().pending, 0);
        assert_eq!(mock.reactions.load(Ordering::Relaxed), 1);
        task.abort();
        server.abort();
    }
    #[tokio::test]
    async fn old_and_already_confirmed_messages_do_not_create_new_reactions() {
        let cfg = NaimeiConfig {
            token: "synthetic".into(),
            guild_id: 900000000000000001,
            backend: "http://127.0.0.1:1".into(),
            site: String::new(),
            secret: "synthetic".into(),
            chat: None,
            owner_ids: vec![],
            chat_log: None,
        };
        let app = Naimei::new(cfg, None);
        {
            let mut r = app.reactions.lock().unwrap();
            r.enabled = true;
            r.since = 10.0;
            r.done.insert("new".into());
        }
        app.reaction_committed(&json!({"message_id":"old","channel_id":"synthetic","at":9.0}));
        app.reaction_committed(&json!({"message_id":"new","channel_id":"synthetic","at":11.0}));
        assert_eq!(app.reaction_status()["pending"], 0);
    }
}
