//! 部署交接：另一个进程拿着 `jobs.lock` 时，新进程照常起来但不跑定时任务，等锁释放后再接手。
//! 公开仓库：全部是合成数据；Discord / NovelAI / 官方状态页都不出本机。

mod common;
use common::*;
use gate_server::jobs::{self, leader::LOCK_FILE};
use serde_json::Value;
use std::fs::File;
use std::time::{Duration, Instant};

async fn ran(e: &Env) -> bool {
    let raw = setting(&e.state, "job_last").await.unwrap_or_default();
    serde_json::from_str::<Value>(&raw)
        .ok()
        .and_then(|v| v.get("idle_recycle").cloned())
        .is_some()
}

#[tokio::test]
async fn jobs_wait_for_the_previous_process_to_release_the_lock() {
    let e = setup(&[]).await;
    let s = &e.state;
    s.jobs
        .set_official_api(Some("http://127.0.0.1:9/never".into()));
    // 「旧进程」：同一个锁文件另开一份并拿着锁（flock 按打开的文件算，同进程里也互斥）
    let old = File::create(s.env.data_dir.join(LOCK_FILE)).unwrap();
    old.try_lock().unwrap();

    jobs::startup(s).await.unwrap();
    tokio::time::sleep(Duration::from_millis(2500)).await;
    assert!(!ran(&e).await, "旧进程还拿着锁，定时任务不该跑");

    drop(old);
    let start = Instant::now();
    while !ran(&e).await {
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "锁释放后没有接手定时任务"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    // 接手之后锁一直拿着：第三个进程拿不到
    let third = File::open(s.env.data_dir.join(LOCK_FILE)).unwrap();
    assert!(third.try_lock().is_err());
    jobs::shutdown(s).await;
}

#[tokio::test]
async fn shutdown_while_waiting_for_the_lock_is_prompt() {
    let e = setup(&[]).await;
    let s = &e.state;
    let old = File::create(s.env.data_dir.join(LOCK_FILE)).unwrap();
    old.try_lock().unwrap();
    jobs::startup(s).await.unwrap();
    let t = Instant::now();
    jobs::shutdown(s).await;
    assert!(t.elapsed() < Duration::from_secs(3), "{:?}", t.elapsed());
    assert!(!ran(&e).await);
}
