//! Real-time delivery and recoverable, paginated reconciliation. Only metadata is queued.
use super::*;
use crate::bridge::ShareDelivery;
use gate_core::{clock::Clock as ServiceClock, share_detection::Report};
use twilight_model::util::Timestamp;

#[derive(Clone)]
struct Pending {
    payload: Value,
    failed_at: f64,
    next_at: f64,
    attempts: u32,
}

#[derive(Default)]
pub(super) struct State {
    report: Report,
    pending: HashMap<String, Pending>,
    inflight: HashSet<String>,
    accepted: HashSet<String>,
    cursors: HashMap<String, u64>,
    day_start: f64,
}

struct Inflight<'a> {
    app: &'a Naimei,
    id: String,
}
impl Drop for Inflight<'_> {
    fn drop(&mut self) {
        self.app.shares().inflight.remove(&self.id);
    }
}

fn snowflake_at(at: f64) -> u64 {
    ((at * 1000.0) as u64).saturating_sub(1_420_070_400_000) << 22
}

impl Naimei {
    fn shares(&self) -> std::sync::MutexGuard<'_, State> {
        self.share_sync
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn set_gateway_connected(&self, connected: bool) {
        self.gateway_connected.store(connected, Ordering::Relaxed);
        if connected {
            // READY and RESUMED must reconcile edits missed during a disconnection too.
            self.shares().cursors.clear();
            self.gateway_seen_at
                .store(self.now() as u64, Ordering::Relaxed);
        }
    }

    fn connected(&self) -> bool {
        self.gateway_connected.load(Ordering::Relaxed)
            && self.now() - (self.gateway_seen_at.load(Ordering::Relaxed) as f64) < 120.0
    }

    pub fn share_detection_report(&self) -> Report {
        let state = self.shares();
        let mut report = state.report.clone();
        report.connected = self.connected();
        report.pending = state.pending.len();
        report.oldest_failure_at = state
            .pending
            .values()
            .map(|p| p.failed_at)
            .chain(report.oldest_failure_at)
            .reduce(f64::min);
        report
    }

    fn share_error(&self, error: &str) {
        let mut state = self.shares();
        state.report.oldest_failure_at.get_or_insert(self.now());
        state.report.error = Some(error.to_owned());
    }

    /// Starts from the service-day boundary after every process restart and reset.
    fn share_day(&self) -> f64 {
        let day = ServiceClock::service().day_start(self.now());
        let mut state = self.shares();
        if state.day_start != day {
            state.cursors.clear();
            state.accepted.clear();
            state
                .pending
                .retain(|_, p| p.payload["at"].as_f64().is_some_and(|at| at >= day));
            state.day_start = day;
        }
        day
    }

    pub(super) async fn record_forum_share(&self, m: &MessageInfo) -> bool {
        let Some(author) = m.author.as_ref() else {
            return true;
        };
        if author.bot
            || m.webhook_id.is_some()
            || author.id.is_empty()
            || m.id.is_empty()
            || m.guild_id
                .as_deref()
                .is_some_and(|g| g.parse::<u64>().ok() != Some(self.cfg.guild_id))
            || !share::has_image(m)
        {
            return true;
        }
        let Some(at) = share::snowflake_unix(&m.id) else {
            return true;
        };
        if at < self.share_day() {
            return true;
        }
        let Some(parent) = self.share_parent(&m.channel_id).await else {
            self.share_error("无法核实帖子所属频道；等待补扫");
            return false;
        };
        if parent != share::FORUM_CHANNEL_ID && m.channel_id != share::FORUM_CHANNEL_ID {
            return true;
        }
        let payload = json!({"discord_id": author.id, "guild_id": self.cfg.guild_id.to_string(),
            "message_id": m.id, "channel_id": m.channel_id, "parent_id": parent, "at": at});
        {
            let mut state = self.shares();
            if state.accepted.contains(&m.id) {
                return true;
            }
            state.pending.entry(m.id.clone()).or_insert(Pending {
                payload,
                failed_at: self.now(),
                next_at: self.now(),
                attempts: 0,
            });
        }
        self.deliver_share(&m.id).await
    }

    async fn deliver_share(&self, id: &str) -> bool {
        let now = self.now();
        let payload = {
            let mut state = self.shares();
            let Some(p) = state.pending.get(id) else {
                return state.accepted.contains(id);
            };
            if p.next_at > now || state.inflight.contains(id) {
                return false;
            }
            let payload = p.payload.clone();
            state.inflight.insert(id.to_owned());
            payload
        };
        let inflight = Inflight {
            app: self,
            id: id.to_owned(),
        };
        let result = self.bridge.report_share(&payload).await;
        drop(inflight);
        let mut state = self.shares();
        match result {
            ShareDelivery::Committed => {
                state.pending.remove(id);
                state.accepted.insert(id.to_owned());
                true
            }
            ShareDelivery::Retry => {
                if let Some(p) = state.pending.get_mut(id) {
                    p.attempts = p.attempts.saturating_add(1);
                    p.next_at = now + f64::from((1u32 << p.attempts.min(6)).min(60));
                }
                state.report.oldest_failure_at.get_or_insert(now);
                state.report.error = Some("分享上报暂未确认，正在重试".into());
                false
            }
            ShareDelivery::Rejected(code) => {
                state.pending.remove(id);
                state.report.oldest_failure_at.get_or_insert(now);
                state.report.error =
                    Some(format!("网关拒绝分享记录（HTTP {code}），请检查桥接配置"));
                false
            }
        }
    }

    pub async fn retry_shares_once(&self) {
        let ids: Vec<_> = self.shares().pending.keys().cloned().collect();
        futures::stream::iter(ids)
            .for_each_concurrent(4, |id| async move {
                self.deliver_share(&id).await;
            })
            .await;
    }

    pub(super) async fn share_retry_loop(self: Arc<Self>) {
        loop {
            self.retry_shares_once().await;
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }

    pub(super) async fn forum_loop(self: Arc<Self>) {
        loop {
            self.catch_up_forum().await;
            tokio::select! {
                () = tokio::time::sleep(SYNC_INTERVAL) => {},
                () = self.forum_wake.notified() => {},
            }
        }
    }

    /// Complete means every page was read and every relevant message was acknowledged.
    pub async fn catch_up_forum(&self) -> bool {
        let _scan = self.forum_scan.lock().await;
        let day = self.share_day();
        let result = tokio::time::timeout(Duration::from_secs(120), self.sweep_forum(day)).await;
        let complete = matches!(result, Ok(Ok(())))
            && self.connected()
            && self.shares().pending.is_empty()
            && self.share_day() == day;
        if complete {
            let mut state = self.shares();
            state.report.last_complete_at = self.now();
            state.report.complete_day_start = day;
            state.report.error = None;
            state.report.oldest_failure_at = None;
        } else {
            let reason = match result {
                Ok(Err(reason)) => reason,
                Err(_) => "论坛补扫超时，等待下一轮恢复",
                _ => "等待 Discord 连接或分享记录确认",
            };
            let has_delivery_error = self.shares().report.error.is_some();
            if reason != "分享记录未确认，保留游标等待重试" || !has_delivery_error {
                self.share_error(reason);
            }
        }
        // Publish completion immediately rather than waiting for the heartbeat interval.
        self.sync_once(0.0).await;
        complete
    }

    async fn sweep_forum(&self, day: f64) -> Result<(), &'static str> {
        let channel = share::FORUM_CHANNEL_ID
            .parse()
            .map_err(|_| "论坛频道配置不正确")?;
        let info = self
            .api
            .channel_info(channel)
            .await
            .map_err(|_| "论坛不可见，请检查频道权限")?;
        if info["guild_id"]
            .as_str()
            .and_then(|g| g.parse::<u64>().ok())
            != Some(self.cfg.guild_id)
            || info["type"].as_u64() != Some(15)
        {
            return Err("论坛不属于配置的服务器或频道类型错误");
        }
        let mut threads = self
            .api
            .active_threads(self.cfg.guild_id)
            .await
            .map_err(|_| "无法列出活跃帖子，请检查 Discord 连接及权限")?;
        let mut before: Option<String> = None;
        loop {
            let page = self
                .api
                .archived_threads(channel, before.as_deref())
                .await
                .map_err(|_| "无法读取归档帖子，请检查读取历史权限")?;
            let rows = page["threads"].as_array().ok_or("归档帖子分页格式不正确")?;
            let mut oldest: Option<(String, f64)> = None;
            for row in rows {
                let stamp = row["thread_metadata"]["archive_timestamp"]
                    .as_str()
                    .ok_or("归档帖子缺少归档时间")?;
                let at = Timestamp::parse(stamp)
                    .map_err(|_| "归档时间格式不正确")?
                    .as_secs() as f64;
                oldest = Some((stamp.to_owned(), at));
                if at >= day {
                    threads.push(row.clone());
                }
            }
            if !page["has_more"].as_bool().ok_or("归档分页缺少 has_more")? {
                break;
            }
            let (stamp, at) = oldest.ok_or("归档分页没有前进")?;
            if at < day {
                break;
            }
            if before.as_deref() == Some(&stamp) {
                return Err("归档分页没有前进");
            }
            before = Some(stamp);
        }
        let mut ids = HashSet::new();
        let mut failed = None;
        for thread in threads {
            if thread["parent_id"].as_str() != Some(share::FORUM_CHANNEL_ID) {
                continue;
            }
            let id = thread["id"].as_str().ok_or("帖子缺少 ID")?;
            if !ids.insert(id.to_owned()) {
                continue;
            }
            self.remember_parent(id, share::FORUM_CHANNEL_ID);
            // Joining is best-effort: history access, not joining, proves reconciliation.
            if thread["thread_metadata"]["archived"] != true {
                self.join_thread(id).await;
            }
            if let Err(reason) = self.scan_forum_thread(id, day).await {
                failed = Some(reason);
            }
        }
        if let Some(reason) = failed {
            Err(reason)
        } else {
            Ok(())
        }
    }

    async fn scan_forum_thread(&self, id: &str, day: f64) -> Result<(), &'static str> {
        let channel = id.parse().map_err(|_| "帖子 ID 格式不正确")?;
        let mut cursor = self
            .shares()
            .cursors
            .get(id)
            .copied()
            .unwrap_or_else(|| snowflake_at(day).saturating_sub(1));
        loop {
            let rows = self
                .api
                .messages_after(channel, cursor)
                .await
                .map_err(|_| "无法读取帖子消息，请检查读取历史权限")?;
            let n = rows.len();
            let mut next = cursor;
            for raw in rows {
                let message: MessageInfo =
                    serde_json::from_value(raw).map_err(|_| "帖子消息格式不正确")?;
                let mid = message
                    .id
                    .parse::<u64>()
                    .map_err(|_| "消息 ID 格式不正确")?;
                next = next.max(mid);
                if !self.record_forum_share(&message).await {
                    return Err("分享记录未确认，保留游标等待重试");
                }
            }
            if n > 0 && next <= cursor {
                return Err("消息分页没有前进");
            }
            // Never advance a cursor across a failed page or unacknowledged delivery.
            self.shares().cursors.insert(id.to_owned(), next);
            cursor = next;
            if n < 100 {
                break;
            }
        }
        Ok(())
    }

    pub(super) async fn on_thread(&self, t: ThreadInfo, created: bool) {
        if t.parent_id.as_deref() != Some(share::FORUM_CHANNEL_ID) {
            return;
        }
        self.remember_parent(&t.id, share::FORUM_CHANNEL_ID);
        if created || !t.archived {
            self.join_thread(&t.id).await;
        }
        if let Some(raw) = t.message {
            if let Ok(m) = serde_json::from_value(raw) {
                self.record_forum_share(&m).await;
            }
        } else if created
            && let Ok(id) = t.id.parse()
            && let Ok(raw) = self.api.message_by_id(id, id).await
            && let Ok(m) = serde_json::from_value(raw)
        {
            self.record_forum_share(&m).await;
        }
        self.forum_wake.notify_one();
    }

    async fn join_thread(&self, thread_id: &str) {
        if let Ok(id) = thread_id.parse()
            && self.api.join_thread(id).await.is_err()
        {
            debug!("[bot] 加入分享帖子失败，将通过历史补扫核对");
        }
    }
}
