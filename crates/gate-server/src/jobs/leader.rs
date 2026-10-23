//! 定时任务只在一个网关进程里跑：拿到数据目录里 `jobs.lock` 排他锁（flock）的那个。
//!
//! 部署时新旧网关可能短暂同时在线：旧的还在把在途的图收尾，新的已经在接请求。
//! 两边共用同一个库，动态额度、自动驾驶、回收这类多步写入不能两边各跑一份，所以后起的进程先只接请求，
//! 每 2 秒试一次锁，等旧进程退出（内核随进程释放锁，崩溃也一样）后再拉起定时任务。
//!
//! 锁一直拿到进程退出：一轮任务做到底才响应关停，提前放锁会让新进程的第一轮和旧进程的最后一轮重叠。

use super::{JobError, sleep_or_shutdown, spawn_all};
use crate::state::AppState;
use std::fs::{File, OpenOptions, TryLockError};
use std::path::Path;
use std::sync::Mutex;

/// 数据目录里的锁文件。宿主机上可以用 `flock -n <data>/jobs.lock true` 看有没有进程拿着它。
pub const LOCK_FILE: &str = "jobs.lock";
/// 没拿到锁时多久再试一次。
const RETRY_SECONDS: f64 = 2.0;

/// 拿到的锁放这里，进程退出时由内核释放。
static HELD: Mutex<Vec<File>> = Mutex::new(Vec::new());

/// 拿到锁就立即拉起全部定时任务；拿不到就挂一个等锁的循环（关停时退出）。
pub(super) fn start(state: &AppState) -> Result<(), JobError> {
    let path = state.env.data_dir.join(LOCK_FILE);
    let file = open(&path).map_err(|e| JobError::new("LockError", format!("{LOCK_FILE}：{e}")))?;
    if take(&file) {
        hold(file);
        spawn_all(state);
        return Ok(());
    }
    tracing::info!("另一个网关进程正在跑定时任务（部署交接中），等它退出后接手");
    let st = state.clone();
    state.tasks.spawn(async move {
        while sleep_or_shutdown(&st, RETRY_SECONDS).await {
            if take(&file) {
                tracing::info!("旧进程已退出，接手定时任务");
                hold(file);
                spawn_all(&st);
                return;
            }
        }
    });
    Ok(())
}

/// 宿主机上用 root 建出来的锁文件网关写不了：只读打开也能加 flock。
fn open(path: &Path) -> std::io::Result<File> {
    match OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
    {
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => File::open(path),
        r => r,
    }
}

fn take(file: &File) -> bool {
    match file.try_lock() {
        Ok(()) => true,
        Err(TryLockError::WouldBlock) => false,
        Err(TryLockError::Error(e)) => {
            // 文件系统不支持锁（不该出现在线上）：不能因此永远不跑定时任务
            tracing::warn!("{LOCK_FILE} 加锁失败，按独占处理：{e}");
            true
        }
    }
}

fn hold(file: File) {
    HELD.lock().expect("jobs lock").push(file);
}
