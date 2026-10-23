//! 写任务 / 读查询 panic 后数据库仍然可用（审查 2026-10-17 R2：写线程 panic 后永久只读、读池漏连接）。

use gate_db::{Db, DbError, Unhealthy};
use std::time::Duration;

fn open() -> (tempfile::TempDir, Db) {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(dir.path().join("t.db")).unwrap();
    (dir, db)
}

async fn count(db: &Db) -> i64 {
    db.read(|c| c.query_row("SELECT count(*) FROM t", [], |r| r.get(0)))
        .await
        .unwrap()
}

#[tokio::test]
async fn writer_survives_a_panicking_job() {
    let (_dir, db) = open();
    db.write(|t| t.execute("CREATE TABLE t (v INTEGER)", []))
        .await
        .unwrap();

    let r = db
        .write(|t| -> rusqlite::Result<()> {
            t.execute("INSERT INTO t VALUES (1)", [])?;
            panic!("模拟写任务 panic");
        })
        .await;
    assert!(matches!(r, Err(DbError::WriterGone)), "{r:?}");
    // panic 的那次事务已回滚
    assert_eq!(count(&db).await, 0);

    // 下一个写任务照常执行（它排在重开连接之后）
    db.write(|t| t.execute("INSERT INTO t VALUES (2)", []))
        .await
        .unwrap();
    assert_eq!(count(&db).await, 1);
    assert_eq!(db.health(Duration::from_secs(2)).await, Ok(()));
}

#[tokio::test]
async fn reader_pool_survives_panicking_queries() {
    let (_dir, db) = open();
    db.write(|t| t.execute("CREATE TABLE t (v INTEGER)", []))
        .await
        .unwrap();
    // 池子默认 2 条；panic 三次后如果连接漏了，下面的读会一直等
    for _ in 0..3 {
        let r = db
            .read(|_| -> rusqlite::Result<()> { panic!("模拟读查询 panic") })
            .await;
        assert!(matches!(r, Err(DbError::ReaderPanicked)), "{r:?}");
    }
    let n = tokio::time::timeout(Duration::from_secs(2), count(&db))
        .await
        .expect("读连接都还回池子了");
    assert_eq!(n, 0);
}

#[tokio::test]
async fn health_reports_write_when_the_queue_is_stuck() {
    let (_dir, db) = open();
    assert_eq!(db.health(Duration::from_secs(2)).await, Ok(()));

    // 写线程被一个慢任务占住：读正常，写那一条在时限内轮不到
    let slow = db.clone();
    let blocker = tokio::spawn(async move {
        slow.write_raw(|_| {
            std::thread::sleep(Duration::from_millis(800));
            Ok(())
        })
        .await
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(
        db.health(Duration::from_millis(200)).await,
        Err(Unhealthy::Write)
    );
    blocker.await.unwrap().unwrap();
    assert_eq!(db.health(Duration::from_secs(2)).await, Ok(()));
}

/// 读请求在异步信号量上排队，不占阻塞线程：64 个并发读、池子 2 条连接，真正执行查询的线程不超过几个。
/// （以前每个读请求先占一个阻塞线程再轮询空闲连接，这里会用到几十个线程。）
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn waiting_readers_do_not_hold_blocking_threads() {
    let (_dir, db) = open();
    let threads = std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashSet::new()));
    let mut tasks = Vec::new();
    for _ in 0..64 {
        let (db, threads) = (db.clone(), threads.clone());
        tasks.push(tokio::spawn(async move {
            db.read(move |c| {
                threads.lock().unwrap().insert(std::thread::current().id());
                std::thread::sleep(Duration::from_millis(5));
                c.query_row("SELECT 1", [], |r| r.get::<_, i64>(0))
            })
            .await
        }));
    }
    for t in tasks {
        assert_eq!(t.await.unwrap().unwrap(), 1);
    }
    let n = threads.lock().unwrap().len();
    assert!(n <= 6, "执行读查询的阻塞线程有 {n} 个（池子只有 2 条连接）");
}
