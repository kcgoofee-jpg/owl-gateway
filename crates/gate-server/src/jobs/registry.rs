//! 定时任务的登记表、运行记录与循环。
//!
//! 每个任务一个循环（[`spawn_job`]）：启动后立即跑第一轮，之后按周期（加抖动）再跑。一轮跑完记一次结果
//! （[`JobsState::record`]）：内存里一份，整体写进设置 `job_last`（serde_json）。任务出错或 panic 都只记成
//! 这一轮失败（`ok=false`，摘要写错误，另记 Bug，来源 `job:<名字>`），循环照常继续。

use super::{
    autopilot, capacity, idle_recycle_job, jittered, maintenance_job, quota, sleep_or_shutdown,
};
use crate::error::{ApiError, InternalError};
use crate::state::AppState;
use futures::FutureExt;
use futures::future::BoxFuture;
use gate_core::pyc::take_chars;
use serde_json::{Map, Value, json};
use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::sync::RwLock;
use std::time::Instant;

/// 设置键：每个任务最近一轮的结果 `{"<名字>": {"at", "ok", "ms", "summary"}}`。
pub const STATE_KEY: &str = "job_last";
/// 摘要（含错误）最多这么多个字符。
pub const SUMMARY_CHARS: usize = 300;

// ---------------------------------------------------------------------------
// 登记表
// ---------------------------------------------------------------------------

/// 一轮任务：返回一句摘要（写进后台「系统」页的「定时任务」卡片）。
pub type RunFn = for<'a> fn(&'a AppState) -> BoxFuture<'a, Result<String, JobError>>;

pub struct Job {
    pub name: &'static str,
    pub title: &'static str,
    /// 给人看的周期（「每 10 分钟」）。
    pub every: &'static str,
    pub every_secs: f64,
    /// 周期的 ±抖动比例（0 = 固定间隔）。固定网格是最容易被上游识别的机器特征之一。
    pub jitter: f64,
    pub run: RunFn,
}

fn run_quota(s: &AppState) -> BoxFuture<'_, Result<String, JobError>> {
    Box::pin(quota::tick(s))
}
fn run_upstream_snapshot(s: &AppState) -> BoxFuture<'_, Result<String, JobError>> {
    Box::pin(capacity::upstream_snapshot(s))
}
fn run_breaker(s: &AppState) -> BoxFuture<'_, Result<String, JobError>> {
    Box::pin(autopilot::breaker(s))
}
fn run_key_guard(s: &AppState) -> BoxFuture<'_, Result<String, JobError>> {
    Box::pin(autopilot::key_guard(s))
}
fn run_maintenance(s: &AppState) -> BoxFuture<'_, Result<String, JobError>> {
    Box::pin(maintenance_job(s))
}
fn run_abuse_scan(s: &AppState) -> BoxFuture<'_, Result<String, JobError>> {
    Box::pin(super::abuse_scan::run(s))
}
fn run_anlas_reconcile(s: &AppState) -> BoxFuture<'_, Result<String, JobError>> {
    Box::pin(super::reconcile::run(s))
}
fn run_idle_recycle(s: &AppState) -> BoxFuture<'_, Result<String, JobError>> {
    Box::pin(idle_recycle_job(s))
}

/// 全部定时任务，按后台显示顺序。周期和抖动沿用内核时代的值。
pub const JOBS: [Job; 8] = [
    Job {
        name: "quota",
        title: "额度同步",
        every: "每 10 分钟",
        every_secs: 600.0,
        jitter: 0.2,
        run: run_quota,
    },
    Job {
        name: "upstream_snapshot",
        title: "上游 V5 快照",
        every: "每 10 分钟",
        every_secs: 600.0,
        jitter: 0.2,
        run: run_upstream_snapshot,
    },
    Job {
        name: "breaker",
        title: "全站熔断",
        every: "每 10 分钟",
        every_secs: 600.0,
        jitter: 0.2,
        run: run_breaker,
    },
    Job {
        name: "key_guard",
        title: "单 Key 限流",
        every: "每 10 分钟",
        every_secs: 600.0,
        jitter: 0.2,
        run: run_key_guard,
    },
    Job {
        name: "maintenance",
        title: "维护",
        every: "每 5 分钟",
        every_secs: 300.0,
        jitter: 0.2,
        run: run_maintenance,
    },
    Job {
        name: "abuse_scan",
        title: "小号扫描",
        every: "每 6 小时",
        every_secs: 21600.0,
        jitter: 0.1,
        run: run_abuse_scan,
    },
    Job {
        name: "anlas_reconcile",
        title: "Anlas 自动核对",
        every: "每小时",
        every_secs: 3600.0,
        jitter: 0.1,
        run: run_anlas_reconcile,
    },
    Job {
        name: "idle_recycle",
        title: "闲置回收",
        every: "每小时",
        every_secs: 3600.0,
        jitter: 0.0,
        run: run_idle_recycle,
    },
];

/// 按名字找任务。
pub fn find(name: &str) -> Option<&'static Job> {
    JOBS.iter().find(|j| j.name == name)
}

// ---------------------------------------------------------------------------
// 错误
// ---------------------------------------------------------------------------

/// 任务里一步出错：错误类型名 + 消息（写进摘要与 Bug 追踪，格式 `类型: 消息`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobError {
    pub ty: String,
    pub msg: String,
    /// 各步骤已经自己记过 Bug（维护任务）：这一轮只记失败，不再另记一条。
    pub reported: bool,
}

impl JobError {
    pub fn new(ty: impl Into<String>, msg: impl Into<String>) -> Self {
        Self {
            ty: ty.into(),
            msg: msg.into(),
            reported: false,
        }
    }

    /// 已经各自记过 Bug 的失败（只用来显示）。
    pub fn reported(msg: impl Into<String>) -> Self {
        Self {
            ty: String::new(),
            msg: msg.into(),
            reported: true,
        }
    }
}

impl std::fmt::Display for JobError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.ty.is_empty() {
            f.write_str(&self.msg)
        } else {
            write!(f, "{}: {}", self.ty, self.msg)
        }
    }
}

impl std::error::Error for JobError {}

impl From<gate_core::quota::DecideError> for JobError {
    fn from(e: gate_core::quota::DecideError) -> Self {
        let ty = e.py_type();
        let text = e.to_string();
        let msg = text
            .strip_prefix(&format!("{ty}: "))
            .unwrap_or(&text)
            .to_string();
        Self::new(ty, msg)
    }
}

impl From<gate_db::DbError> for JobError {
    fn from(e: gate_db::DbError) -> Self {
        Self::new("DbError", e.to_string())
    }
}

impl From<crate::guard::GuardIoError> for JobError {
    fn from(e: crate::guard::GuardIoError) -> Self {
        Self::new("GuardError", e.to_string())
    }
}

impl From<JobError> for ApiError {
    fn from(e: JobError) -> Self {
        InternalError(e.to_string()).into()
    }
}

// ---------------------------------------------------------------------------
// 运行记录
// ---------------------------------------------------------------------------

/// 一轮结果从哪来（决定记 Bug 和写库的方式）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// 定时循环（以及 [`run_once`]）：失败记 Bug（来源 `job:<名字>`），整份记录立即写库。
    Loop,
    /// 后台触发的立即重算（[`super::quota::run_now`]）：只更新内存里的记录（接口马上能看到）；
    /// 出错由调用方的接口自己记 Bug；落库跟着下一次任何定时任务记结果一起写（最多约一个维护周期）。
    /// 这样后台改 Key、改参数的接口不会顺带多写一行设置。
    Manual,
}

/// 定时任务的共享状态，放在 [`AppState`] 里。
pub struct JobsState {
    /// 每个任务最近一轮的结果（也就是设置 `job_last` 的内容）。写库时一直持锁，保证库里的是最新的一份。
    last: tokio::sync::Mutex<Map<String, Value>>,
    /// 动态额度重算串行化：定时任务、后台改参数、Key 交回算法都会调它，不能两份同时写。
    quota_lock: tokio::sync::Mutex<()>,
    /// 官方状态接口地址的覆盖（测试 / 本地开发用，保证不出本机）；None 用状态页的 `OFFICIAL_API`。
    official_api: RwLock<Option<String>>,
}

impl Default for JobsState {
    fn default() -> Self {
        Self::new()
    }
}

impl JobsState {
    pub fn new() -> Self {
        Self {
            last: tokio::sync::Mutex::new(Map::new()),
            quota_lock: tokio::sync::Mutex::new(()),
            official_api: RwLock::new(None),
        }
    }

    /// 启动时读回上次的运行记录（重启后卡片不会清空）。读不懂就当没有。
    pub async fn load(&self, db: &gate_db::Db) -> Result<(), JobError> {
        let raw = db.get_setting(STATE_KEY).await?;
        let parsed = raw
            .and_then(|s| serde_json::from_str::<Value>(&s).ok())
            .and_then(|v| match v {
                Value::Object(m) => Some(m),
                _ => None,
            })
            .unwrap_or_default();
        *self.last.lock().await = parsed;
        Ok(())
    }

    /// 运行记录里最近一次有任务跑完的时间；`raw` 是库里的 `job_last`，读不懂或为空时 None。
    /// 启动时的停机检测用：维护任务每 5 分钟跑一次，比请求日志更能说明网关是不是一直开着。
    pub fn latest_run_at(raw: Option<&str>) -> Option<f64> {
        let v: Value = serde_json::from_str(raw?).ok()?;
        v.as_object()?
            .values()
            .filter_map(|j| j.get("at")?.as_f64())
            .reduce(f64::max)
    }

    /// 当前的运行记录（`job_last`）。
    pub async fn last(&self) -> Map<String, Value> {
        self.last.lock().await.clone()
    }

    /// 记一轮结果，见 [`Origin`]。
    pub async fn record(
        &self,
        state: &AppState,
        name: &str,
        at: f64,
        ms: i64,
        result: &Result<String, JobError>,
        origin: Origin,
    ) {
        let (ok, summary) = match result {
            Ok(s) => (true, s.clone()),
            Err(e) => (false, e.to_string()),
        };
        let entry = json!({
            "at": at,
            "ok": ok,
            "ms": ms,
            "summary": take_chars(&summary, SUMMARY_CHARS),
        });
        if let Err(e) = result
            && origin == Origin::Loop
            && !e.reported
        {
            let source = format!("job:{name}");
            state.bugs.capture(crate::bugs::Report {
                source: &source,
                error: Some((&e.ty, &e.msg)),
                ..Default::default()
            });
        }
        let mut last = self.last.lock().await;
        last.insert(name.to_string(), entry);
        if origin == Origin::Manual {
            return;
        }
        let text = Value::Object(last.clone()).to_string();
        if let Err(e) = state.db.set_setting(STATE_KEY, &text).await {
            tracing::warn!(job = name, "定时任务运行记录写库失败：{e}");
        }
    }

    /// 把「取官方状态」指到别的地址（测试 / 本地开发用，保证不出本机）；`None` 恢复官方接口。
    pub fn set_official_api(&self, url: Option<String>) {
        *self.official_api.write().expect("official_api") = url;
    }

    pub fn official_api(&self) -> Option<String> {
        self.official_api.read().expect("official_api").clone()
    }

    pub(crate) async fn quota_guard(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.quota_lock.lock().await
    }
}

// ---------------------------------------------------------------------------
// 运行
// ---------------------------------------------------------------------------

/// 跑一个 future，把 panic 也变成 [`JobError`]（类型 `Panic`），保证调用方（循环）不会因此退出。
pub async fn guarded<F, T>(fut: F) -> Result<T, JobError>
where
    F: Future<Output = Result<T, JobError>>,
{
    match AssertUnwindSafe(fut).catch_unwind().await {
        Ok(r) => r,
        Err(payload) => {
            let msg = payload
                .downcast_ref::<&str>()
                .map(|s| (*s).to_string())
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "任务 panic".to_string());
            Err(JobError::new("Panic", msg))
        }
    }
}

/// 立即跑一轮并记结果（出错时记 Bug）。循环和测试都走这里。
pub async fn run_once(state: &AppState, job: &Job) -> Result<String, JobError> {
    let at = state.now();
    let t0 = Instant::now();
    let r = guarded((job.run)(state)).await;
    let ms = i64::try_from(t0.elapsed().as_millis()).unwrap_or(i64::MAX);
    state
        .jobs
        .record(state, job.name, at, ms, &r, Origin::Loop)
        .await;
    r
}

/// 拉起一个任务的循环（挂在 `state.tasks` 上）：先跑一轮，再睡一个周期；睡觉时收到关停就退出。
/// 一轮工作做到底才响应关停：额度重算这类多步写入不能被拦腰切断。
pub fn spawn_job(state: &AppState, job: &'static Job) {
    let st = state.clone();
    state.tasks.spawn(async move {
        loop {
            let _ = run_once(&st, job).await;
            let secs = if job.jitter > 0.0 {
                jittered(job.every_secs, job.jitter)
            } else {
                job.every_secs
            };
            if !sleep_or_shutdown(&st, secs).await {
                break;
            }
        }
        tracing::info!(task = job.name, "定时任务已停止");
    });
}
