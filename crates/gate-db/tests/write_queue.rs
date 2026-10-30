//! 写队列的背压（审查 2026-10-31 ①）：队列满以后调用方必须在**异步**信号量上排队。
//!
//! 以前 `Db::write` 在 async 上下文里直接 `SyncSender::send`：队列一满就把 tokio worker 卡死，
//! 同一时刻的 `/healthz`、SSE 心跳、其他请求一起停。这个用例用「单 worker 运行时 + 长度为 1 的写队列」
//! 把这个行为钉住：写线程被占住时，运行时必须还能跑别的任务。

use gate_db::Db;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

fn open(queue: usize) -> (tempfile::TempDir, Db) {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open_with_queue(&dir.path().join("q.db"), None, queue).unwrap();
    (dir, db)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn a_full_write_queue_waits_without_blocking_the_runtime() {
    let (_dir, db) = open(1);
    db.write(|t| t.execute("CREATE TABLE q (v INTEGER)", []))
        .await
        .unwrap();
    let running = Arc::new(AtomicBool::new(false));
    // 唯一的写队列位置被一个 300ms 的任务占住
    let slow = {
        let db = db.clone();
        let running = running.clone();
        tokio::spawn(async move {
            db.write(move |t| -> rusqlite::Result<()> {
                running.store(true, Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(300));
                t.execute("INSERT INTO q VALUES (1)", []).map(|_| ())
            })
            .await
        })
    };
    for _ in 0..500 {
        if running.load(Ordering::SeqCst) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    assert!(running.load(Ordering::SeqCst), "慢写任务没起来");

    // 另外两个写请求：队列长度只有 1，它们必须挂起等待，而且不能占死唯一的 worker
    let waiting: Vec<_> = (0..2)
        .map(|_| {
            let db = db.clone();
            tokio::spawn(async move {
                db.write(|t| t.execute("INSERT INTO q VALUES (2)", []))
                    .await
            })
        })
        .collect();

    // 心跳：如果 send 还是阻塞的，唯一的 worker 会被卡住，这个 sleep 不会按时完成
    let beat = tokio::spawn(async {
        tokio::time::sleep(Duration::from_millis(120)).await;
        true
    });
    let ticked = tokio::time::timeout(Duration::from_millis(250), beat).await;
    assert!(
        matches!(ticked, Ok(Ok(true))),
        "写队列满时 tokio worker 被阻塞：异步背压没生效"
    );

    slow.await.unwrap().unwrap();
    for w in waiting {
        w.await.unwrap().unwrap();
    }
    let n: i64 = db
        .read(|c| c.query_row("SELECT count(*) FROM q", [], |r| r.get(0)))
        .await
        .unwrap();
    assert_eq!(n, 3);
}
