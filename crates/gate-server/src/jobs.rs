//! 定时任务与启动 / 关停序列。
//!
//! 只剩三件事，全部挂在 `state.tasks` 上，启动后立即跑第一轮，睡觉时听 `state.shutdown`：
//!
//! | 任务 | 周期 | 做什么 |
//! |---|---|---|
//! | `upstream_snapshot` 上游 V5 快照 | 10 分钟 ±20%，满 1 小时才记 | 主动读一次订阅，写 `upstream_snapshots`（后台「额度记录」页估算 V5 实测恢复速度用） |
//! | `breaker` 全站熔断 | 10 分钟 ±20% | 15 分钟内上游失败 ≥ 5 次、占比 ≥ 30%、来自 ≥ 2 把 Key → 全站暂停出图 5 分钟（阈值是常量，不是自适应） |
//! | `maintenance` 维护 | 5 分钟 ±20% | 注册服务的三个同步（`registration::registrar_jobs`）/ 结算账本只留未决行 / 磁盘自检 |
//!
//! 单 Key 限流（按 1 小时被拒次数推断「客户端在死循环」）、Anlas 自动核对、日志保留清理都已删除。
//! 熔断保留：它的阈值写死在 `gate_core::breaker` 里，作用是不让上游账号被打死。
//!
//! 每轮的结果记进设置 `job_last`，启动时据此判断最近一次任务完成时间（[`registry`]）。
//! 同一个库同一时刻只有一个进程跑这些任务（部署交接时新旧网关会同时在线，见 [`leader`]）。
//! 一轮工作做到底才响应关停（只在睡觉时被打断）：额度重算这类多步写入不能被拦腰切断。

mod breaker;
mod capacity;
pub mod leader;
pub mod registry;
mod support;

pub use registry::{JOBS, Job, JobError, JobsState, find, guarded, run_once, spawn_job};

use crate::bugs::Report;
use crate::state::AppState;
use std::time::Duration;

/// 关停时等待记账 / 写库任务的最长时间（docker `stop_grace_period` 是 120 秒）。
pub const SHUTDOWN_WAIT_SECONDS: u64 = 110;

/// 周期任务的固定节拍（每 300 / 600 秒整）是最容易被识别的机器特征之一：加 ±ratio 抖动。
/// 上游看到的订阅查询、状态轮询因此不再是「每 600.0 秒一次」的精确网格。
pub fn jittered(seconds: f64, ratio: f64) -> f64 {
    jittered_with(seconds, ratio, rand::random::<f64>())
}

/// `jittered` 的纯函数形式：`u` 是 [0, 1) 的随机数（`random.uniform(-ratio, ratio)` = `-ratio + 2*ratio*u`）。
pub fn jittered_with(seconds: f64, ratio: f64, u: f64) -> f64 {
    let uniform = -ratio + (ratio - -ratio) * u;
    (seconds * (1.0 + uniform)).max(30.0)
}

/// 睡 `seconds` 秒；睡眠中收到关停信号返回 false（调用方就该退出循环了）。
async fn sleep_or_shutdown(state: &AppState, seconds: f64) -> bool {
    if state.shutdown.is_cancelled() {
        return false;
    }
    tokio::select! {
        _ = state.shutdown.cancelled() => false,
        _ = tokio::time::sleep(Duration::from_secs_f64(seconds.max(0.0))) => true,
    }
}

fn capture(state: &AppState, source: &str, e: &JobError) {
    state.bugs.capture(Report {
        source,
        error: Some((&e.ty, &e.msg)),
        ..Default::default()
    });
}

// ---------------------------------------------------------------------------
// 启动 / 关停
// ---------------------------------------------------------------------------

/// 启动序列（在 `AppState::build` 之后调用）：读回定时任务的运行记录 → 拿到 `jobs.lock` 后拉起所有循环
/// （另一个进程拿着锁时先只接请求，等它退出再接手，见 [`leader`]）。
pub async fn startup(state: &AppState) -> Result<(), JobError> {
    state.jobs.load(&state.db).await?;
    leader::start(state)
}

/// 拉起所有定时任务的循环。每个循环都在 `state.shutdown` 取消后尽快退出。
pub fn spawn_all(state: &AppState) {
    for job in JOBS.iter() {
        spawn_job(state, job);
    }
}

/// 优雅关停的后半段（HTTP 服务已经停止接收新连接并等完在途请求之后）：
/// 取消循环 → 等 `state.tasks`（记账 / Bug 写库 / 循环收尾，最多 110 秒）→ 状态码统计落库 → WAL 并回主库。
pub async fn shutdown(state: &AppState) {
    state.shutdown.cancel();
    state.tasks.close();
    if tokio::time::timeout(
        Duration::from_secs(SHUTDOWN_WAIT_SECONDS),
        state.tasks.wait(),
    )
    .await
    .is_err()
    {
        tracing::warn!("等待后台任务超过 {SHUTDOWN_WAIT_SECONDS} 秒，放弃等待");
    }
    match state.db.wal_checkpoint().await {
        Ok((busy, log, done)) => tracing::info!(busy, log, done, "WAL 已并回主库"),
        Err(e) => tracing::warn!("关停时 WAL checkpoint 失败：{e}"),
    }
}

// ---------------------------------------------------------------------------
// 维护
// ---------------------------------------------------------------------------

/// 维护的一轮：每一步各自出错、各自记 Bug（来源 `maintenance:<步骤>`），不影响其他步骤。
/// 返回出错的步骤（显示名, 错误）。
pub async fn maintenance_once(state: &AppState) -> Vec<(&'static str, JobError)> {
    let mut failed = Vec::new();
    macro_rules! step {
        ($name:literal, $label:literal, $fut:expr) => {
            if state.shutdown.is_cancelled() {
                return failed;
            }
            if let Err(e) = $fut.await {
                capture(state, concat!("maintenance:", $name), &e);
                failed.push(($label, e));
            }
        };
    }
    // 同步身份组 / 补全资料 / 退群回收（注册服务自己逐步记错；没配齐就什么都不做）
    if state.shutdown.is_cancelled() {
        return failed;
    }
    crate::registration::registrar_jobs(state).await;
    step!("settlements", "结算账本清理", prune_settled(state));
    failed
}

/// 定时任务「维护」：跑一轮 [`maintenance_once`]，出错的步骤合成一句（各步骤已经各自记过 Bug）。
pub(crate) async fn maintenance_job(state: &AppState) -> Result<String, JobError> {
    let failed = maintenance_once(state).await;
    if !failed.is_empty() {
        let text: Vec<String> = failed
            .iter()
            .map(|(label, e)| format!("{label}出错：{e}"))
            .collect();
        return Err(JobError::reported(text.join("；")));
    }
    if state.shutdown.is_cancelled() {
        return Ok("正在关停，本轮没做完".to_string());
    }
    Ok("全部完成".to_string())
}

/// 结算账本只留未决行：已终结的行与用量日志重复，留着等于同一件事存两份。
async fn prune_settled(state: &AppState) -> Result<(), JobError> {
    let before = state.now() - gate_db::repo::settlement::SETTLED_KEEP_SECS;
    let n = state
        .db
        .prune_settled_requests(before)
        .await
        .map_err(|e| JobError::new("DbError", e.to_string()))?;
    if n > 0 {
        tracing::info!(deleted = n, "pruned settled request rows");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jitter_matches_uniform_formula() {
        // random.uniform(-0.2, 0.2) = -0.2 + 0.4 * u
        assert_eq!(jittered_with(600.0, 0.2, 0.0), 480.0);
        assert!((jittered_with(600.0, 0.2, 0.5) - 600.0).abs() < 1e-9);
        assert!((jittered_with(600.0, 0.2, 1.0) - 720.0).abs() < 1e-9);
        // 下限 30 秒
        assert_eq!(jittered_with(10.0, 0.2, 0.5), 30.0);
    }
}
