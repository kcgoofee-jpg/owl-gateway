//! Bug 追踪：把所有「不该发生」的错误按特征归并记下来（旧版 `app/errors.py` 的 Tracker）。
//!
//! 来源：接口未捕获的错误（500）、上游 5xx、后台任务失败、记账写入失败、成员在图片返回前断开、网页脚本报错。
//! 同一个 bug 只占一行（count 累加），每次都写一行日志；不私信站长（后台「Bug 追踪」能看）。不记录 Key、提示词或 IP。
//! 永远不会让调用方出错或阻塞：入库在后台任务里做。
//!
//! 3.0 起不再记「不是本站缺陷」的噪音（浏览器网络失败、上游故障、非本站脚本），已记下的由
//! [`Bugs::auto_triage`] 定期自动关闭；注意级 24 小时、错误级 7 天没再出现的也一并关闭，再出现按「复发」重新打开。

use gate_core::errors::{
    AutoReason, Capture, ExcInfo, OpenBug, TRIAGE_INTERVAL, auto_resolve_reason, build_event,
    should_print_stack,
};
use gate_db::Db;
use gate_db::repo::errors::{AutoResolve, ErrorRow};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

pub struct Bugs {
    db: Db,
    printed: Mutex<HashMap<String, f64>>,
    recent: Mutex<Vec<gate_core::errors::ErrorEvent>>,
    /// 同一个特征码的读-改-写串行（旧版用一把全局锁）。
    store_lock: Arc<tokio::sync::Mutex<()>>,
    tasks: tokio_util::task::TaskTracker,
    /// 还没写进库的记录数（入库是后台任务；`idle()` 等它归零）。
    pending: Arc<std::sync::atomic::AtomicUsize>,
    /// 上一次自动处理的时间（Unix 秒）；0 = 还没跑过（启动后的第一轮维护就会跑）。
    triage_at: Mutex<f64>,
}

/// 一次记录的参数（旧版 capture 的关键字参数）。
#[derive(Default, Clone)]
pub struct Report<'a> {
    pub source: &'a str,
    /// (错误类型名, 消息)；Rust 里没有 Python 的堆栈帧，位置用错误类型所在模块代替。
    pub error: Option<(&'a str, &'a str)>,
    pub title: &'a str,
    pub detail: &'a str,
    pub path: &'a str,
    pub key_id: Option<i64>,
    pub rid: &'a str,
    /// "error"（默认）或 "warn"
    pub level: &'a str,
}

impl Bugs {
    pub fn new(db: Db, tasks: tokio_util::task::TaskTracker) -> Self {
        Self {
            db,
            printed: Mutex::new(HashMap::new()),
            recent: Mutex::new(Vec::new()),
            store_lock: Arc::new(tokio::sync::Mutex::new(())),
            tasks,
            pending: Arc::default(),
            triage_at: Mutex::new(0.0),
        }
    }

    /// 等已经记下的 Bug 全部写进库（最多 5 秒）。测试用：读库之前调用，不靠固定的睡眠时间。
    pub async fn idle(&self) {
        for _ in 0..500 {
            if self.pending.load(std::sync::atomic::Ordering::Acquire) == 0 {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }

    /// 自动处理到点了吗：距上次不到 [`TRIAGE_INTERVAL`] 就不跑；到点就记下这次的时间。
    pub fn triage_due(&self, now: f64) -> bool {
        let mut last = self.triage_at.lock().expect("bugs");
        if now - *last < TRIAGE_INTERVAL {
            return false;
        }
        *last = now;
        true
    }

    /// 自动关闭不需要人处理的未处理 Bug，返回关掉的条数：
    /// 噪音（网络断开 / 上游故障 / 非本站脚本）立刻关；注意级 24 小时、错误级 7 天没再出现的关。
    /// 关掉的行带原因码（`error_events.auto_resolved`），再出现时按「复发」重新打开。
    pub async fn auto_triage(&self, now: f64) -> Result<usize, gate_db::DbError> {
        let rows = self.db.open_errors_for_triage().await?;
        let mut items = Vec::new();
        let mut by_reason: Vec<(AutoReason, usize)> = Vec::new();
        for r in &rows {
            let bug = OpenBug {
                source: &r.source,
                level: &r.level,
                title: &r.title,
                detail: &r.detail,
                last_seen: r.last_seen,
            };
            let Some(reason) = auto_resolve_reason(&bug, now) else {
                continue;
            };
            items.push(AutoResolve {
                sig: r.sig.clone(),
                last_seen: r.last_seen,
                reason: reason.code(),
            });
            match by_reason.iter_mut().find(|(x, _)| *x == reason) {
                Some((_, n)) => *n += 1,
                None => by_reason.push((reason, 1)),
            }
        }
        if items.is_empty() {
            return Ok(0);
        }
        let n = self.db.auto_resolve_errors(&items, now).await?;
        let why: Vec<String> = by_reason
            .iter()
            .map(|(r, n)| format!("{}={n}", r.code()))
            .collect();
        tracing::info!(target: "bug", "[bug] 自动关闭 {n} 个无需处理的 Bug（{}）", why.join(" "));
        Ok(n)
    }

    /// 记录一次错误，返回特征码。
    pub fn capture(&self, r: Report<'_>) -> String {
        let now = crate::time::now();
        let level = if r.level.is_empty() { "error" } else { r.level };
        let exc = r.error.map(|(ty, msg)| ExcInfo {
            type_name: ty,
            message: msg,
            app_frame: r.source,
            stack: msg,
        });
        let c = build_event(
            &Capture {
                source: r.source,
                exc,
                title: r.title,
                detail: r.detail,
                path: r.path,
                key_id: r.key_id,
                rid: r.rid,
                level,
            },
            now,
        );
        tracing::warn!(target: "bug", "{}", c.log_line);
        if r.error.is_some() && !c.stack_text.is_empty() {
            let mut p = self.printed.lock().expect("bugs");
            let last = p.get(&c.event.sig).copied();
            if should_print_stack(now, last) {
                p.insert(c.event.sig.clone(), now);
                tracing::warn!(target: "bug", "{}", c.stack_text);
            }
        }
        {
            let mut recent = self.recent.lock().expect("bugs");
            recent.push(c.event.clone());
            let n = recent.len();
            if n > 50 {
                recent.drain(..n - 50);
            }
        }
        let e = c.event.clone();
        let db = self.db.clone();
        let lock = self.store_lock.clone();
        let pending = self.pending.clone();
        pending.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        self.tasks.spawn(async move {
            // 不管成功失败，任务结束就减一
            struct Done(Arc<std::sync::atomic::AtomicUsize>);
            impl Drop for Done {
                fn drop(&mut self) {
                    self.0.fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
                }
            }
            let _done = Done(pending);
            let _g = lock.lock().await;
            let row = ErrorRow {
                sig: e.sig.clone(),
                source: e.source.clone(),
                level: e.level.clone(),
                title: e.title.clone(),
                detail: e.detail.clone(),
                path: e.path.clone(),
                key_id: e.key_id,
                rid: e.rid.clone(),
                ts: e.ts,
            };
            if db.store_error_event(&row).await.is_err() {
                tracing::warn!(target: "bug", "store failed");
            }
        });
        c.event.sig
    }

    /// 写库失败时也能在内存里看到最近 50 条。
    pub fn recent(&self) -> Vec<gate_core::errors::ErrorEvent> {
        self.recent.lock().expect("bugs").clone()
    }
}
