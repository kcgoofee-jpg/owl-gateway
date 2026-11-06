//! 定时任务的循环：维护一轮、启动 / 关停序列，以及「任务出错或 panic 时循环不退出」。
//! 各任务本身的规则见 `jobs_quota.rs` / `jobs_capacity.rs`。
//! 公开仓库：全部是合成数据；Discord / NovelAI都不出本机。

mod common;
use common::*;
use futures::future::BoxFuture;
use gate_server::AppState;
use gate_server::jobs::{self, JOBS, Job, JobError};
use serde_json::{Value, json};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

// ------------------------------------------------------------------ 启动 / 关停

#[tokio::test]
async fn startup_runs_every_job_and_shutdown_finishes_promptly() {
    let e = setup(&[]).await;
    let s = &e.state;
    // 上次的运行记录在启动时读回来；已经不存在的任务记录直接丢掉，不让它跟着写回落库
    s.db.set_setting(
        "job_last",
        &json!({"retired_job": {"at": 1.0, "ok": true, "ms": 0, "summary": "旧"}}).to_string(),
    )
    .await
    .unwrap();
    jobs::startup(s).await.unwrap();
    assert!(!s.jobs.last().await.contains_key("retired_job"));
    // 每个任务的第一轮都立即执行（不等一个周期）
    let start = Instant::now();
    loop {
        let raw = setting(s, "job_last").await.unwrap_or_default();
        let saved: Value = serde_json::from_str(&raw).unwrap_or(Value::Null);
        if JOBS.iter().all(|j| saved.get(j.name).is_some()) {
            assert!(
                saved.get("retired_job").is_none(),
                "已删任务的记录不该留在库里"
            );
            break;
        }
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "定时任务没有在启动后立即跑第一轮：{raw}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    // 7 个循环都还挂着（跑完第一轮在睡觉，没有退出）
    assert!(s.tasks.len() >= JOBS.len(), "{}", s.tasks.len());
    let t = Instant::now();
    jobs::shutdown(s).await;
    assert!(
        t.elapsed() < Duration::from_secs(5),
        "关停太慢：{:?}",
        t.elapsed()
    );
    assert!(s.shutdown.is_cancelled() && s.tasks.is_closed() && s.tasks.is_empty());
}

// ------------------------------------------------------------------ 出错 / panic 不让循环退出

static FLAKY_CALLS: AtomicUsize = AtomicUsize::new(0);

/// 第一轮 panic，第二轮报错，之后正常。
fn flaky(_: &AppState) -> BoxFuture<'_, Result<String, JobError>> {
    Box::pin(async {
        match FLAKY_CALLS.fetch_add(1, Ordering::SeqCst) {
            0 => panic!("合成的 panic"),
            1 => Err(JobError::new("ValueError", "合成的错误")),
            n => Ok(format!("第 {} 轮", n + 1)),
        }
    })
}

static FLAKY: Job = Job {
    name: "flaky_test",
    title: "测试用",
    every: "每 0.05 秒",
    every_secs: 0.05,
    jitter: 0.0,
    run: flaky,
};

#[tokio::test]
async fn a_failing_or_panicking_job_keeps_its_loop_alive() {
    let e = setup(&[]).await;
    jobs::spawn_job(&e.state, &FLAKY);
    let start = Instant::now();
    loop {
        let last = Value::Object(e.state.jobs.last().await);
        if last["flaky_test"]["ok"] == true {
            break;
        }
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "循环在出错后退出了：{last}"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(FLAKY_CALLS.load(Ordering::SeqCst) >= 3);
    jobs::shutdown(&e.state).await;
    assert!(e.state.tasks.is_empty(), "关停后循环退出");
}

#[tokio::test]
async fn a_panic_becomes_a_job_error() {
    let r: Result<String, JobError> = jobs::guarded(async { panic!("合成的 panic") }).await;
    let err = r.unwrap_err();
    assert_eq!(
        (err.ty.as_str(), err.msg.as_str()),
        ("Panic", "合成的 panic")
    );
    let r = jobs::guarded(async { Ok::<_, JobError>("好".to_string()) }).await;
    assert_eq!(r.unwrap(), "好");
}
