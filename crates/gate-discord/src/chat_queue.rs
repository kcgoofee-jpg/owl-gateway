//! Bounded durable admission, fair across channels. Dispatched work is never replayed.
use super::*;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Serialize, Deserialize)]
struct Job {
    message: MessageInfo,
    at: f64,
    running: bool,
}
#[derive(Default, Serialize, Deserialize)]
pub(super) struct Queue {
    jobs: VecDeque<Job>,
    #[serde(default)]
    settled: VecDeque<String>,
    #[serde(skip)]
    error: Option<String>,
    #[serde(skip)]
    last_queue_wait_ms: u64,
    #[serde(skip)]
    path: Option<PathBuf>,
    #[serde(skip)]
    failures: VecDeque<f64>,
    #[serde(skip)]
    open_until: f64,
    #[serde(skip)]
    probe: bool,
    #[serde(skip)]
    aborted: HashSet<String>,
}
impl Queue {
    pub fn load(path: Option<PathBuf>) -> Self {
        let mut q = Self::default();
        if let Some(p) = &path {
            match std::fs::read(p) {
                Ok(bytes) => match serde_json::from_slice::<Self>(&bytes) {
                    Ok(saved) => q = saved,
                    Err(_) => q.error = Some("聊天队列文件损坏，已暂停接收，未重放任何任务".into()),
                },
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => q.error = Some("聊天队列文件无法读取，已暂停接收".into()),
            }
        }
        q.path = path;
        q
    }
    fn remember(&mut self, id: String) {
        if !self.settled.contains(&id) {
            self.settled.push_back(id);
        }
        while self.settled.len() > 4096 {
            self.settled.pop_front();
        }
    }
    fn recover(&mut self) -> Vec<MessageInfo> {
        if self.error.is_some() {
            return Vec::new();
        }
        let rows: Vec<_> = self
            .jobs
            .iter()
            .filter(|j| j.running)
            .map(|j| j.message.clone())
            .collect();
        for m in &rows {
            self.remember(m.id.clone());
        }
        self.jobs.retain(|j| !j.running);
        let _ = self.save();
        rows
    }
    fn save(&self) -> std::io::Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let temp = path.with_extension("tmp");
        use std::io::Write;
        let mut options = std::fs::OpenOptions::new();
        options.create(true).truncate(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temp)?;
        file.write_all(&serde_json::to_vec(self)?)?;
        file.sync_all()?;
        std::fs::rename(temp, path)?;
        if let Some(dir) = path.parent() {
            std::fs::File::open(dir)?.sync_all()?;
        }
        Ok(())
    }
    fn admit(&mut self, m: MessageInfo, now: f64) -> Result<usize, &'static str> {
        if self.error.is_some() {
            return Err("聊天队列存储异常，请联系管理员；分享检测不受影响。");
        }
        if self.open_until > now {
            return Err("聊天上游暂不可用，正在冷却；分享和状态查询不受影响。");
        }
        if self.settled.contains(&m.id) || self.jobs.iter().any(|j| j.message.id == m.id) {
            return Err("这条请求已经接收，请勿重复提交。");
        }
        let uid = m.author.as_ref().map(|u| u.id.as_str()).unwrap_or("");
        let mine: Vec<_> = self
            .jobs
            .iter()
            .filter(|j| j.message.author.as_ref().is_some_and(|u| u.id == uid))
            .collect();
        if mine.len() >= 2 || mine.iter().any(|j| !j.running) {
            return Err("你已有一条等待中的请求，请等待或使用 /取消。");
        }
        if self.jobs.iter().filter(|j| !j.running).count() >= 32 {
            return Err("聊天队列已满，请稍后重试。");
        }
        self.jobs.push_back(Job {
            message: m,
            at: now,
            running: false,
        });
        if self.save().is_err() {
            self.jobs.pop_back();
            return Err("聊天队列暂时无法保存，请稍后重试；分享检测不受影响。");
        }
        Ok(self.jobs.iter().filter(|j| !j.running).count())
    }
    fn next(&mut self, now: f64) -> Option<MessageInfo> {
        if self.error.is_some() || self.open_until > now {
            return None;
        }
        let running: Vec<_> = self.jobs.iter().filter(|j| j.running).collect();
        if running.len() >= 4 || (self.open_until > 0.0 && (self.probe || !running.is_empty())) {
            return None;
        }
        let channels: HashSet<_> = running
            .iter()
            .map(|j| j.message.channel_id.clone())
            .collect();
        let users: HashSet<_> = running
            .iter()
            .filter_map(|j| j.message.author.as_ref().map(|u| u.id.clone()))
            .collect();
        let images = running.iter().filter(|j| has_images(&j.message)).count();
        let pos = self.jobs.iter().position(|j| {
            !j.running
                && now - j.at < 180.0
                && !channels.contains(&j.message.channel_id)
                && !j
                    .message
                    .author
                    .as_ref()
                    .is_some_and(|u| users.contains(&u.id))
                && (!has_images(&j.message) || images < 2)
        })?;
        self.jobs[pos].running = true;
        if self.save().is_err() {
            self.jobs[pos].running = false;
            return None;
        }
        self.last_queue_wait_ms = ((now - self.jobs[pos].at).max(0.0) * 1000.0) as u64;
        self.probe = self.open_until > 0.0;
        Some(self.jobs[pos].message.clone())
    }
    fn finish(&mut self, id: &str, failed: bool, now: f64) {
        self.remember(id.to_owned());
        self.jobs.retain(|j| j.message.id != id);
        self.aborted.remove(id);
        self.failures.retain(|at| now - *at < 60.0);
        if failed {
            self.failures.push_back(now);
            if self.probe || self.failures.len() >= 3 {
                self.open_until = now + 60.0;
            }
        } else if self.probe {
            self.open_until = 0.0;
            self.failures.clear();
        }
        self.probe = false;
        if self.save().is_err() {
            warn!("[chat] 无法保存队列完成状态；重启不会重放已派发任务");
        }
    }
}
fn has_images(m: &MessageInfo) -> bool {
    share::has_image(m)
        || m.referenced_message.as_ref().is_some_and(|r| {
            r.attachments.iter().any(|a| {
                a.content_type
                    .as_deref()
                    .is_some_and(|t| t.starts_with("image/"))
            })
        })
}

impl Naimei {
    fn queue(&self) -> std::sync::MutexGuard<'_, Queue> {
        self.chat_queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
    pub fn chat_status(&self) -> Value {
        let q = self.queue();
        json!({"error":q.error,"last_queue_wait_ms":q.last_queue_wait_ms,"oldest_wait_seconds":q.jobs.iter().filter(|j|!j.running).map(|j|(self.now()-j.at).max(0.0)).reduce(f64::max),"running":q.jobs.iter().filter(|j|j.running).count(),"waiting":q.jobs.iter().filter(|j|!j.running).count(),"max_concurrency":4,"max_images":2,"max_waiting":32,"circuit_open":q.open_until>self.now(),"retry_at":q.open_until,"configured":self.chat.is_some(),"worker_connected":self.worker_connected.load(Ordering::Relaxed),"last_model_result":self.model_status.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()})
    }
    pub(super) async fn worker_health_loop(self: Arc<Self>) {
        let Ok(url) = std::env::var("NAIMEI_CHAT_WORKER_URL") else {
            return;
        };
        let http = reqwest::Client::new();
        loop {
            let healthy = http
                .get(format!("{url}/healthz"))
                .timeout(Duration::from_secs(2))
                .send()
                .await
                .is_ok_and(|r| r.status().is_success());
            self.worker_connected.store(healthy, Ordering::Relaxed);
            tokio::time::sleep(Duration::from_secs(30)).await;
        }
    }
    pub(super) async fn enqueue_chat(&self, m: MessageInfo) {
        let Err(text) = self.queue().admit(m.clone(), self.now()) else {
            return; // Normal admission is silent; typing and the answer are sufficient.
        };
        if let Ok(channel) = m.channel_id.parse() {
            self.send_chat_message(channel, &m.id, text).await;
        }
    }
    pub(super) fn cancel_chat(&self, uid: &str) -> Result<usize, &'static str> {
        let mut q = self.queue();
        let prior = q.jobs.clone();
        let prior_settled = q.settled.clone();
        let ids: Vec<_> = q
            .jobs
            .iter()
            .filter(|j| j.message.author.as_ref().is_some_and(|u| u.id == uid))
            .map(|j| (j.message.id.clone(), j.running))
            .collect();
        q.jobs
            .retain(|j| j.running || !ids.iter().any(|(id, _)| *id == j.message.id));
        for (id, running) in &ids {
            if !*running {
                q.remember(id.clone());
            }
        }
        if q.save().is_err() {
            q.jobs = prior;
            q.settled = prior_settled;
            return Err("取消尚未可靠保存，请稍后再试。");
        }
        for (id, running) in &ids {
            if *running {
                q.aborted.insert(id.clone());
            }
        }
        Ok(ids.len())
    }
    pub(super) fn chat_cancelled(&self, id: &str) -> bool {
        self.queue().aborted.contains(id)
    }
    pub(super) async fn chat_queue_loop(self: Arc<Self>) {
        // Running entries from a previous core process have an unknown outcome.
        let interrupted = self.queue().recover();
        for m in interrupted {
            if let Ok(ch) = m.channel_id.parse() {
                self.send_chat_message(
                    ch,
                    &m.id,
                    "服务重启：这条聊天此前已派发，结果未知，未自动重发。需要时请重新 @ 我。",
                )
                .await;
            }
        }
        loop {
            let expired = {
                let mut q = self.queue();
                let rows: Vec<_> = q
                    .jobs
                    .iter()
                    .filter(|j| !j.running && self.now() - j.at >= 180.0)
                    .map(|j| j.message.clone())
                    .collect();
                for m in &rows {
                    q.remember(m.id.clone());
                }
                q.jobs.retain(|j| j.running || self.now() - j.at < 180.0);
                if !rows.is_empty() {
                    let _ = q.save();
                }
                rows
            };
            for m in expired {
                if let Ok(ch) = m.channel_id.parse() {
                    self.send_chat_message(
                        ch,
                        &m.id,
                        "这条聊天等待超过 3 分钟，已结束。可以稍后重新 @ 我；分享与查询不受影响。",
                    )
                    .await;
                }
            }
            let next = if self.chat.is_some() {
                self.queue().next(self.now())
            } else {
                None
            };
            if let Some(m) = next {
                let app = self.clone();
                tokio::spawn(async move {
                    let id = m.id.clone();
                    let worker = app.clone();
                    let task = tokio::spawn(async move { worker.process_chat(m).await });
                    // Cancellation suppresses the reply, but keeps the upstream reservation
                    // until its bounded call finishes; it must not oversubscribe the provider.
                    let failed = task.await.unwrap_or(true);
                    app.queue().finish(&id, failed, app.now());
                });
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn m(id: u64, ch: u64, user: u64, image: bool) -> MessageInfo {
        serde_json::from_value(json!({"id":id.to_string(),"channel_id":ch.to_string(),"author":{"id":user.to_string()},"attachments":if image{json!([{"url":"image","filename":"synthetic.png","content_type":"image/png"}])}else{json!([])}})).unwrap()
    }
    #[test]
    fn fair_limits_and_no_same_channel_overlap() {
        let mut q = Queue::default();
        for i in 0..8 {
            q.admit(m(i, i / 2, i, false), 1.0).unwrap();
        }
        let a = q.next(2.0).unwrap();
        let b = q.next(2.0).unwrap();
        assert_ne!(a.channel_id, b.channel_id);
        q.next(2.0).unwrap();
        q.next(2.0).unwrap();
        assert!(q.next(2.0).is_none());
    }
    #[test]
    fn images_and_user_limits() {
        let mut q = Queue::default();
        for i in 0..3 {
            q.admit(m(i, i, i, true), 1.0).unwrap();
        }
        q.next(2.0).unwrap();
        q.next(2.0).unwrap();
        assert!(q.next(2.0).is_none());
        assert!(q.admit(m(9, 9, 2, false), 3.0).is_err());
    }
    #[test]
    fn circuit_and_expiry() {
        let mut q = Queue::default();
        for i in 0..3 {
            q.finish(&i.to_string(), true, 10.0);
        }
        assert!(q.admit(m(4, 4, 4, false), 11.0).is_err());
        q.admit(m(4, 4, 4, false), 71.0).unwrap();
        q.next(72.0).unwrap();
        q.admit(m(5, 5, 5, false), 72.0).unwrap();
        assert!(q.next(72.0).is_none());
        q.finish("4", false, 73.0);
        assert!(q.next(300.0).is_none());
    }
    #[test]
    fn restart_retains_waiting_and_never_replays_dispatched() {
        let dir = std::env::temp_dir().join(format!("owl-queue-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("queue.json");
        let mut q = Queue::load(Some(path.clone()));
        q.admit(m(1, 1, 1, false), 100.0).unwrap();
        q.admit(m(2, 2, 2, false), 100.0).unwrap();
        q.next(101.0).unwrap();
        drop(q);
        let mut restored = Queue::load(Some(path));
        let unknown = restored.recover();
        assert_eq!(unknown.len(), 1);
        assert_eq!(unknown[0].id, "1");
        assert!(restored.admit(m(1, 1, 1, false), 102.0).is_err());
        assert_eq!(restored.next(102.0).unwrap().id, "2");
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn queue_capacity_and_waiting_cancellation_are_bounded() {
        let mut q = Queue::default();
        for i in 0..32 {
            q.admit(m(i, i, i, false), 10.0).unwrap();
        }
        assert!(q.admit(m(33, 33, 33, false), 10.0).is_err());
        assert!(q.next(191.0).is_none());
    }
    #[test]
    fn text_replies_do_not_consume_image_slots() {
        let mut q = Queue::default();
        for i in 0..4 {
            let mut message = m(i, i, i, false);
            message.message_reference = Some(json!({"message_id":"synthetic"}));
            message.referenced_message = Some(events::ReferencedMessage {
                content: "plain reply".into(),
                ..Default::default()
            });
            q.admit(message, 1.0).unwrap();
        }
        for _ in 0..4 {
            assert!(q.next(2.0).is_some());
        }
    }
}
