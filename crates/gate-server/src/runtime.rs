//! 运行期共享状态：
//! - 一个通用滑动窗口 [`SlidingWindow`]（RPM / 无效 Key / 后台登录 / 前端报错共用）；
//! - 防爆破（IPv6 聚合到 /64）；
//! - 全站图片冷却（上游 429 后）；
//! - Key 暂停闸门（自动驾驶限流等）。
//!
//! 时间：墙钟用 `crate::time::now()`，间隔类用 tokio 单调时钟（测试可暂停）。
//!
//! 每把 Key 的限速只有这两层：出图排队（同时生成 1 张，每人最多排 `queue_per_member` 张，见 `admission::image_admission`）
//! 和每分钟次数（RPM）。

use std::collections::{HashMap, VecDeque};
use std::hash::Hash;
use std::net::IpAddr;
use std::sync::Mutex;
use tokio::time::{Duration, Instant};

/// 滑动窗口计数器。
pub struct SlidingWindow<K: Eq + Hash + Clone> {
    inner: Mutex<HashMap<K, VecDeque<f64>>>,
    /// 键太多时顺手清理的上限（RPM 1024、登录 / 防爆破 2048）。
    gc_over: usize,
}

impl<K: Eq + Hash + Clone> SlidingWindow<K> {
    pub fn new(gc_over: usize) -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
            gc_over,
        }
    }

    /// 先查后记（RPM、后台登录）：窗口内已有 `limit` 次则拒绝且不记；否则记一次放行。
    /// 过期判定：`now - t > window` 才算出窗（边界上的那一秒不算过期）。
    pub fn check_then_push(&self, key: &K, limit: usize, window: f64, now: f64) -> bool {
        let mut map = self.inner.lock().expect("window");
        let win = map.entry(key.clone()).or_default();
        while win.front().is_some_and(|t| now - t > window) {
            win.pop_front();
        }
        if win.len() >= limit.max(1) {
            return false;
        }
        win.push_back(now);
        if map.len() > self.gc_over {
            map.retain(|k, w| k == key || w.back().is_some_and(|t| now - t <= window));
        }
        true
    }

    /// 先记后查（无效 Key 计数）：记一次，返回窗口内的次数。
    pub fn push_then_count(&self, key: &K, window: f64, now: f64) -> usize {
        let mut map = self.inner.lock().expect("window");
        let win = map.entry(key.clone()).or_default();
        win.push_back(now);
        while win.front().is_some_and(|t| now - t > window) {
            win.pop_front();
        }
        let n = win.len();
        if map.len() > self.gc_over {
            map.retain(|k, w| k == key || w.back().is_some_and(|t| now - t <= window));
        }
        n
    }

    pub fn clear(&self, key: &K) {
        if let Some(w) = self.inner.lock().expect("window").get_mut(key) {
            w.clear();
        }
    }
}

/// 防爆破桶的键：IPv6 聚合到 /64（同一用户的临时地址都在一个 /64 里轮换）；IPv4 / 无法解析的原样。
pub fn bruteforce_bucket(client_id: &str) -> String {
    match client_id.parse::<IpAddr>() {
        Ok(IpAddr::V4(_)) | Err(_) => client_id.to_string(),
        Ok(IpAddr::V6(v6)) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return v4.to_string();
            }
            let seg = v6.segments();
            std::net::Ipv6Addr::new(seg[0], seg[1], seg[2], seg[3], 0, 0, 0, 0).to_string()
        }
    }
}

/// IP 打码：IPv4 只留后两段，其余只留最后 6 个字符（告警用）。
pub fn mask_ip(value: &str) -> String {
    let parts: Vec<&str> = value.split('.').collect();
    if parts.len() == 4 {
        format!("*.*.{}.{}", parts[2], parts[3])
    } else {
        format!("…{}", gate_core::primitives::last_chars(value, 6))
    }
}

/// 无效 Key 防爆破。
pub struct AuthGuard {
    fails: SlidingWindow<String>,
    blocked_until: Mutex<HashMap<String, f64>>,
}

impl Default for AuthGuard {
    fn default() -> Self {
        Self {
            fails: SlidingWindow::new(2048),
            blocked_until: Mutex::new(HashMap::new()),
        }
    }
}

impl AuthGuard {
    /// 该 IP 因多次无效 Key 被临时拦截时的剩余秒数；否则 0。
    pub fn blocked(&self, client_id: &str, now: f64) -> i64 {
        let until = *self
            .blocked_until
            .lock()
            .expect("auth")
            .get(&bruteforce_bucket(client_id))
            .unwrap_or(&0.0);
        ((until - now) as i64).max(0)
    }

    /// 记一次无效 Key；达到阈值时拦截并返回 true（调用方发告警）。
    pub fn record_failure(
        &self,
        client_id: &str,
        max: i64,
        window: i64,
        block_seconds: i64,
        now: f64,
    ) -> bool {
        let bucket = bruteforce_bucket(client_id);
        let window = window.max(1) as f64;
        let n = self.fails.push_then_count(&bucket, window, now);
        if n as i64 >= max.max(1) {
            let mut b = self.blocked_until.lock().expect("auth");
            b.insert(bucket.clone(), now + block_seconds.max(1) as f64);
            if b.len() > 2048 {
                b.retain(|_, t| *t >= now);
            }
            drop(b);
            self.fails.clear(&bucket);
            return true;
        }
        false
    }
}

/// 全站图片冷却（上游 429 后）。
#[derive(Default)]
pub struct ImageCooldown {
    until: Mutex<f64>,
}

impl ImageCooldown {
    pub fn remaining(&self, now: f64) -> i64 {
        ((*self.until.lock().expect("cooldown") - now) as i64).max(0)
    }

    /// 从库里恢复（`image_cooldown_until`）：非数 / 超过 1 小时的值视为无效。
    pub fn load(&self, raw: Option<&str>, now: f64) {
        let v = raw
            .and_then(gate_core::primitives::parse_float)
            .unwrap_or(0.0);
        let v = if v.is_nan() || v > now + 3600.0 {
            0.0
        } else {
            v.max(0.0)
        };
        *self.until.lock().expect("cooldown") = v;
    }

    /// 延长冷却，返回新的截止时间（调用方写库 + 告警）。
    pub fn block(&self, retry_after: f64, now: f64) -> f64 {
        let mut u = self.until.lock().expect("cooldown");
        *u = u.max(now + gate_core::upstream_policy::clamp_retry_after_f64(retry_after));
        *u
    }
}

/// Key 暂停闸门。
#[derive(Default)]
pub struct KeyPauses {
    inner: Mutex<HashMap<i64, (f64, String)>>,
}

impl KeyPauses {
    pub fn load(&self, rows: Vec<(i64, f64, String)>) {
        *self.inner.lock().expect("pauses") =
            rows.into_iter().map(|(k, u, r)| (k, (u, r))).collect();
    }

    /// 仍在暂停中：返回 (截止时间, 原因)。
    pub fn paused(&self, key_id: i64, now: f64) -> Option<(f64, String)> {
        self.inner
            .lock()
            .expect("pauses")
            .get(&key_id)
            .filter(|(u, _)| *u > now)
            .cloned()
    }

    pub fn set(&self, key_id: i64, until: f64, reason: String) {
        self.inner
            .lock()
            .expect("pauses")
            .insert(key_id, (until, reason));
    }

    pub fn remove(&self, key_id: i64) {
        self.inner.lock().expect("pauses").remove(&key_id);
    }

    pub fn snapshot(&self) -> HashMap<i64, (f64, String)> {
        self.inner.lock().expect("pauses").clone()
    }
}

/// 同一把 Key 两次标签补全至少间隔 2 秒（补全很轻，旧查询会被新查询取代）。
pub const TAG_MIN_INTERVAL: f64 = 2.0;

/// 标签补全排队。
/// 客户端每输入一个字就查一次补全，只有最新那次有意义：新查询到来时，同一把 Key 还在排队的旧查询作废。
#[derive(Default)]
pub struct TagQueue {
    inner: Mutex<TagState>,
    changed: tokio::sync::Notify,
}

#[derive(Default)]
struct TagState {
    active: std::collections::HashSet<i64>,
    next_at: HashMap<i64, Instant>,
    waiting: usize,
    waiting_by_key: HashMap<i64, usize>,
    latest: HashMap<i64, u64>,
}

/// 标签补全同时打向 NovelAI 的全站并发数（单账号建议保持 1，太高会触发 429 甚至风控）。
/// 出图不受它影响：出图由每把上游 Token 的出图槽位和最小间隔控制。
/// 原来是环境变量 `GLOBAL_CONCURRENCY`（旧说明写成了「全站并发」，其实一直只管标签补全），现在是常量。
pub const TAG_CONCURRENCY: i64 = 1;

/// 排队结果：放行 / 全站排队已满 / 被同一 Key 更新的查询取代。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TagAdmit {
    Admitted,
    Full,
    Superseded,
}

impl TagQueue {
    /// `global_concurrency` 传 [`TAG_CONCURRENCY`]。被取消（超时 / 断开）时排队计数照样归还。
    /// 同一把 Key 两次补全至少隔 [`TAG_MIN_INTERVAL`] 秒。
    pub async fn wait(&self, key_id: i64, global_concurrency: i64) -> TagAdmit {
        struct Waiting<'a>(&'a TagQueue, i64);
        impl Drop for Waiting<'_> {
            fn drop(&mut self) {
                let mut s = self.0.inner.lock().expect("tags");
                s.waiting -= 1;
                match s.waiting_by_key.get_mut(&self.1) {
                    Some(n) if *n > 1 => *n -= 1,
                    _ => {
                        s.waiting_by_key.remove(&self.1);
                    }
                }
            }
        }
        let ticket = {
            let mut s = self.inner.lock().expect("tags");
            if s.waiting as i64 >= (global_concurrency * 16).max(16) {
                return TagAdmit::Full;
            }
            let t = s.latest.get(&key_id).copied().unwrap_or(0) + 1;
            s.latest.insert(key_id, t);
            s.waiting += 1;
            *s.waiting_by_key.entry(key_id).or_insert(0) += 1;
            t
        };
        let _w = Waiting(self, key_id);
        self.changed.notify_waiters(); // 叫醒同一 Key 的旧查询，让它们发现自己已作废
        let capacity = global_concurrency.clamp(1, 8) as usize;
        loop {
            let notified = self.changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let delay = {
                let mut s = self.inner.lock().expect("tags");
                if s.latest.get(&key_id) != Some(&ticket) {
                    return TagAdmit::Superseded;
                }
                let now = Instant::now();
                let delay = s
                    .next_at
                    .get(&key_id)
                    .map_or(Duration::ZERO, |n| n.saturating_duration_since(now));
                if !s.active.contains(&key_id) && s.active.len() < capacity {
                    if delay.is_zero() {
                        s.active.insert(key_id);
                        s.next_at
                            .insert(key_id, now + Duration::from_secs_f64(TAG_MIN_INTERVAL));
                        return TagAdmit::Admitted;
                    }
                    Some(delay)
                } else {
                    None
                }
            };
            match delay {
                Some(d) => {
                    tokio::select! {
                        _ = &mut notified => {}
                        _ = tokio::time::sleep(d) => {}
                    }
                }
                None => notified.await,
            }
        }
    }

    pub fn finish(&self, key_id: i64) {
        self.inner.lock().expect("tags").active.remove(&key_id);
        self.changed.notify_waiters();
    }
}

/// 最近 10 分钟上游调用结果（状态页判断「上游不稳定」）。
#[derive(Default)]
pub struct UpstreamHealth {
    events: Mutex<VecDeque<(f64, bool)>>,
}

impl UpstreamHealth {
    /// 记一次结果（只留最近 40 次）。
    pub fn record(&self, ok: bool, now: f64) {
        let mut e = self.events.lock().expect("health");
        e.push_back((now, ok));
        while e.len() > 40 {
            e.pop_front();
        }
    }

    /// (status, recent, failed)：近 10 分钟 ≥ 4 次且失败过半为 degraded；没有记录为 idle。
    pub fn status(&self, now: f64) -> (&'static str, usize, usize) {
        let e = self.events.lock().expect("health");
        let recent: Vec<bool> = e
            .iter()
            .filter(|(t, _)| *t >= now - 600.0)
            .map(|(_, ok)| *ok)
            .collect();
        let failed = recent.iter().filter(|ok| !**ok).count();
        let status = if recent.len() >= 4 && failed as f64 / recent.len() as f64 >= 0.5 {
            "degraded"
        } else if recent.is_empty() {
            "idle"
        } else {
            "ok"
        };
        (status, recent.len(), failed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_semantics() {
        let w: SlidingWindow<i64> = SlidingWindow::new(1024);
        assert!(w.check_then_push(&1, 2, 60.0, 0.0));
        assert!(w.check_then_push(&1, 2, 60.0, 1.0));
        assert!(!w.check_then_push(&1, 2, 60.0, 2.0));
        // 恰好 60 秒不出窗（边界用 > 判定）
        assert!(!w.check_then_push(&1, 2, 60.0, 60.0));
        assert!(w.check_then_push(&1, 2, 60.0, 60.5));
    }

    #[test]
    fn buckets() {
        assert_eq!(bruteforce_bucket("203.0.113.9"), "203.0.113.9");
        assert_eq!(bruteforce_bucket("2001:db8:1:2:3:4:5:6"), "2001:db8:1:2::");
        assert_eq!(bruteforce_bucket("::ffff:203.0.113.9"), "203.0.113.9");
        assert_eq!(bruteforce_bucket("unknown"), "unknown");
        assert_eq!(mask_ip("203.0.113.9"), "*.*.113.9");
    }

    #[test]
    fn auth_guard_blocks_after_max() {
        let g = AuthGuard::default();
        for i in 0..19 {
            assert!(!g.record_failure("203.0.113.9", 20, 600, 900, i as f64));
        }
        assert!(g.record_failure("203.0.113.9", 20, 600, 900, 19.0));
        assert_eq!(g.blocked("203.0.113.9", 20.0), 899);
        assert_eq!(g.blocked("203.0.113.10", 20.0), 0);
    }
}
