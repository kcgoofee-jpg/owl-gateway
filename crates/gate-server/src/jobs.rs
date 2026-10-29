//! 定时任务与启动 / 关停序列。
//!
//! 每件事一个循环，全部挂在 `state.tasks` 上，启动后立即跑第一轮，睡觉时听 `state.shutdown`：
//!
//! | 任务 | 周期 | 做什么 |
//! |---|---|---|
//! | `upstream_snapshot` 上游 V5 快照 | 10 分钟 ±20%，满 1 小时才记 | 主动读一次订阅，写 `upstream_snapshots`（后台「额度记录」页估算 V5 实测恢复速度用） |
//! | `breaker` 全站熔断 | 10 分钟 ±20% | 15 分钟内上游失败 ≥ 5 次、≥ 30%、来自 ≥ 2 把 Key → 停 5 分钟 |
//! | `key_guard` 单 Key 限流 | 10 分钟 ±20% | 1 小时被拒 ≥ 120 次 → 暂停这把 Key 1 小时 |
//! | `maintenance` 维护 | 5 分钟 ±20% | 设置了保留期的记录才清（线上全部永久保留）/ 注册服务的三个同步（`registration::registrar_jobs`）/ 磁盘自检 |
//! | `abuse_scan` 小号扫描 | 6 小时 ±10% | 行为相似 / 同设备 / 设备相近 / 指纹异常 / 新号 / 共享 Key，以及关联扫描（同 IP 换号 / 同时异地 / 同参考图 / 同外部账号 / 服务器重合 / 电量相同）写进待审队列（[`abuse_scan`]），不自动封号 |
//! | `anlas_reconcile` Anlas 自动核对 | 1 小时 ±10% | 和后台「Anlas 消耗核对」同一套（[`reconcile`]）；忙时等不到空档就跳过，差距偏大记一条「注意」Bug |
//! | `idle_recycle` 闲置回收 | 1 小时 | 长期不用的 Key 删除（不拉黑，之后还能再 /register；站点出不了图的服务日不算，见 [`idle_recycle_window`]）；`KEY_INACTIVITY_DELETE_DAYS=0` 时**什么都不做** |
//!
//! 每轮的结果记进设置 `job_last`，后台「系统」页的「定时任务」卡片显示（[`registry`]）。
//! 同一个库同一时刻只有一个进程跑这些任务（部署交接时新旧网关会同时在线，见 [`leader`]）。
//! 一轮工作做到底才响应关停（只在睡觉时被打断）：额度重算这类多步写入不能被拦腰切断。

mod abuse_scan;
mod autopilot;
mod capacity;
pub mod leader;
mod reconcile;
pub mod registry;
mod support;

pub use registry::{JOBS, Job, JobError, JobsState, Origin, find, guarded, run_once, spawn_job};

use crate::action_log::log_action;
use crate::bugs::Report;
use crate::state::AppState;
use gate_core::idle;
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
    step!("purge", "清理过期记录", purge(state));
    // 同步身份组 / 补全资料 / 退群回收（注册服务自己逐步记错；没配齐就什么都不做）
    if state.shutdown.is_cancelled() {
        return failed;
    }
    crate::registration::registrar_jobs(state).await;
    step!("disk", "磁盘检查", disk_check(state));
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

/// Apply request log retention.
pub async fn purge(state: &AppState) -> Result<(), JobError> {
    let db = &state.db;
    let keep = state.env.usage_log_retention_days;
    if keep > 0 {
        db.purge_usage_log(state.now() - std::cmp::max(7, keep) as f64 * 86400.0)
            .await?;
    }
    // 请求日志里的 UA / IP：后台设置了天数才清（0 = 永久）
    let origin_days = crate::settings::usage_origin_keep_days(db).await?;
    if origin_days > 0 {
        let cleared = db
            .purge_usage_origin(state.now() - origin_days as f64 * 86400.0)
            .await?;
        if cleared > 0 {
            tracing::info!("[purge] 已清掉 {cleared} 条日志里超过 {origin_days} 天的 UA / IP");
        }
    }
    Ok(())
}

/// 磁盘剩余不足 10% 提醒站长（同一条 6 小时一次）。
async fn disk_check(state: &AppState) -> Result<(), JobError> {
    let st = rustix::fs::statvfs(&state.env.data_dir)
        .map_err(|e| JobError::new("OSError", e.to_string()))?;
    // shutil.disk_usage：free = f_bavail * f_frsize，total = f_blocks * f_frsize
    let free = st.f_bavail.saturating_mul(st.f_frsize);
    let total = st.f_blocks.saturating_mul(st.f_frsize);
    if total == 0 {
        return Err(JobError::new("division_by_zero", "division by zero"));
    }
    if (free as f64) / (total as f64) < 0.10 {
        state.alerter.notify(
            "disk_low",
            &format!(
                "服务器磁盘剩余不足 10%（剩 {} MB），请清理或扩容。",
                free / (1 << 20)
            ),
            6.0 * 3600.0,
        );
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// 闲置 Key 回收
// ---------------------------------------------------------------------------

/// 定时任务「闲置回收」。2026-10-13 站长：取消闲置提醒私信，到期直接删 Key，不提前通知。
/// 10-23 站长：只删 Key，**不拉黑** Discord 账号——成员之后还能再 /register 领新 Key。
pub(crate) async fn idle_recycle_job(state: &AppState) -> Result<String, JobError> {
    Ok(match recycle_inactive_keys(state).await? {
        Recycle::Off => "关闭（天数=0）".to_string(),
        Recycle::Skipped => "最近 60 分钟没出图，本轮跳过".to_string(),
        Recycle::Done {
            deleted,
            skipped_days,
        } => {
            if deleted > 0 {
                tracing::info!("deleted {deleted} inactive API key(s)");
            }
            if skipped_days > 0 {
                format!("回收 {deleted} 把（不算站点没在正常出图的 {skipped_days} 天）")
            } else {
                format!("回收 {deleted} 把")
            }
        }
    })
}

/// 一轮闲置回收的结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Recycle {
    /// `KEY_INACTIVITY_DELETE_DAYS=0`：关闭。
    Off,
    /// 最近 60 分钟全站没有成功出图：本轮跳过。
    Skipped,
    /// 删掉的 Key 数；`skipped_days` = 往回多数的、站点没在正常出图的服务日数。
    Done { deleted: usize, skipped_days: i64 },
}

/// 本轮闲置回收的截止时间；站点眼下就出不了图时返回 `None`（本轮不回收）。
///
/// 站点自己出不了图的服务日不算成员闲置：
/// - 已完结的服务日全站成功出图不到 [`idle::HEALTHY_DAY_MIN_IMAGES`] 张 → 跳过，往回多数一天（最多回看 30 天）；
/// - 当前服务日：最近 60 分钟全站一张都没出成功 → 本轮直接跳过。
///
/// 平时（近几天都在正常出图）截止时间就是「现在 − N 天」，与原来完全相同。
pub async fn idle_recycle_window(
    state: &AppState,
    days: i64,
    now: f64,
) -> Result<Option<idle::IdleWindow>, JobError> {
    let recent = state
        .db
        .ok_image_counts(vec![(now - idle::RECENT_OK_WINDOW, f64::MAX)])
        .await?;
    if recent.first().copied().unwrap_or(0) == 0 {
        return Ok(None);
    }
    // 已完结的服务日：用服务日工具取日期与起点，不自己做时区换算
    let lookback = idle::max_lookback_days(days) as usize;
    let names = state.clock.last_days(now, lookback + 1);
    let mut ranges = Vec::with_capacity(lookback);
    for pair in names.windows(2) {
        if let (Some(start), Some(end)) = (
            state.clock.service_day_start(&pair[0]),
            state.clock.service_day_start(&pair[1]),
        ) {
            ranges.push((start, end));
        }
    }
    let counts = state.db.ok_image_counts(ranges.clone()).await?;
    let served: Vec<idle::ServedDay> = ranges
        .into_iter()
        .zip(counts)
        .map(|((start, end), ok_images)| idle::ServedDay {
            start,
            end,
            ok_images,
        })
        .collect();
    Ok(Some(idle::idle_window(now, days, &served)))
}

/// 回收长期未使用的普通 Key（只删 Key、释放名额和身份组，不拉黑）。返回删掉的 Key 数，见 [`recycle_inactive_keys`]。
pub async fn delete_inactive_keys(state: &AppState) -> Result<usize, JobError> {
    Ok(match recycle_inactive_keys(state).await? {
        Recycle::Done { deleted, .. } => deleted,
        Recycle::Off | Recycle::Skipped => 0,
    })
}

/// 闲置回收的一轮。
///
/// **`KEY_INACTIVITY_DELETE_DAYS=0` 表示关闭：这时无论库里有什么都不会删任何 Key**（线上现在是 3，10-13 起开着）。
/// 「连续 N 天」不算站点自己出不了图的服务日，见 [`idle_recycle_window`]。
pub async fn recycle_inactive_keys(state: &AppState) -> Result<Recycle, JobError> {
    let days = std::cmp::max(0, state.env.key_inactivity_delete_days);
    if days == 0 {
        return Ok(Recycle::Off);
    }
    let now = state.now();
    let Some(window) = idle_recycle_window(state, days, now).await? else {
        tracing::info!("最近 60 分钟全站没有成功出图，本轮不做闲置回收");
        return Ok(Recycle::Skipped);
    };
    if window.skipped_days > 0 {
        tracing::info!(
            skipped_days = window.skipped_days,
            "闲置回收跳过站点没在正常出图的服务日"
        );
    }
    let key_ids = state.db.inactive_key_ids(window.cutoff, now).await?;
    let detail = if window.skipped_days > 0 {
        format!(
            "连续 {days} 天未活动自动回收（不算站点没在正常出图的 {} 天）",
            window.skipped_days
        )
    } else {
        format!("连续 {days} 天未活动自动回收")
    };
    let registrar = state.registration.clone();
    for &key_id in &key_ids {
        let key_name = state
            .db
            .get_key(key_id)
            .await?
            .and_then(|k| k.get("name").map(gate_core::values::text))
            .unwrap_or_else(|| format!("Key #{key_id}"));
        // 领取记录会随 Key 一起删掉：先记下是谁，删完摘身份组
        let reg = state.db.registration_for_key(key_id).await?;
        state.db.delete_key(key_id, state.now()).await?;
        log_action(
            &state.db,
            "系统",
            "闲置回收 Key",
            &format!("Key #{key_id} {key_name}"),
            &detail,
            true,
            state.now(),
        )
        .await;
        // 不拉黑（站长 10-23）：之后还能再 /register 领新 Key；只摘身份组
        if let (Some(reg), Some(r)) = (&reg, &registrar) {
            let _ = r.release_role(&reg.discord_id).await;
        }
        // 释放领取记录
        for discord_id in state.db.forget_registration_for_key(key_id).await? {
            if let Some(r) = &registrar {
                r.release_role(&discord_id).await?;
            }
        }
    }
    Ok(Recycle::Done {
        deleted: key_ids.len(),
        skipped_days: window.skipped_days,
    })
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
