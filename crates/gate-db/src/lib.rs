//! SQLite 存取层。整个项目只有这个 crate 写 SQL。
//!
//! 并发模型：
//! - **一个写线程**独占写连接，所有写操作排队执行，每个任务是一个 `BEGIN IMMEDIATE` 事务；
//!   遇到 `SQLITE_BUSY`（备份脚本同时开着库等）重试 4 次。
//! - **只读连接池**（默认 2 个）：WAL 下读不阻塞写，查询放在 `spawn_blocking` 里跑。
//!   等连接的读请求在异步信号量上排队，不占阻塞线程（同一时刻最多「池子大小」个读线程）。
//!
//! `repo` 里每个方法都有快照：`tests/repo_snapshot.rs` 对返回值、有变化的表和原图目录做快照
//!
//! 事务失败即整体回滚：由 `rusqlite::Transaction` 的 Drop 自动完成，不会出现「一条语句失败后
//! 事务挂着、之后所有写都 locked」。

mod archive_removal;
pub mod bootstrap;
pub mod policy_migration;
pub mod repo;
pub mod value;

use rusqlite::{Connection, OpenFlags, Transaction, TransactionBehavior};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::oneshot;

#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("数据库错误：{0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("写线程已退出")]
    WriterGone,
    #[error("读任务异常退出")]
    ReaderPanicked,
    /// 调用参数不合法。
    #[error("参数错误：{0}")]
    Invalid(String),
    #[error("文件操作失败：{0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, DbError>;

type Job = Box<dyn FnOnce(&mut Connection) + Send>;

/// 数据库句柄；克隆成本很低（内部是 Arc）。
#[derive(Clone)]
pub struct Db {
    inner: Arc<Inner>,
}

struct Inner {
    path: PathBuf,
    writer: std::sync::mpsc::SyncSender<Job>,
    readers: Mutex<Vec<Connection>>,
    /// 许可数 = 读连接数：拿到许可才进阻塞线程，所以进去时池子里一定有空闲连接。
    read_slots: Arc<tokio::sync::Semaphore>,
}

/// 健康检查没通过的原因（给 `/healthz` 用）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unhealthy {
    /// 只读连接在时限内查不出 `SELECT 1`。
    Read,
    /// 写线程已退出，或写队列在时限内没轮到。
    Write,
}

impl Unhealthy {
    pub fn code(self) -> &'static str {
        match self {
            Self::Read => "db_read",
            Self::Write => "db_write",
        }
    }
}

/// 写事务里遇到锁冲突时的重试次数。
const WRITE_ATTEMPTS: u32 = 4;

impl Db {
    /// 打开（不存在则创建）数据库并执行建表迁移；读连接固定 2 条。
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::open_inner(path.as_ref(), None)
    }

    /// 网关启动专用：在写线程、请求和任务启动前完成规则迁移。
    pub fn open_for_gateway(path: impl AsRef<Path>, now: f64, idle_days: i64) -> Result<Self> {
        Self::open_inner(path.as_ref(), Some((now, idle_days)))
    }

    fn open_inner(path: &Path, member_migration: Option<(f64, i64)>) -> Result<Self> {
        const READERS: usize = 2;
        let path = path.to_path_buf();
        let mut writer_conn = Connection::open(&path)?;
        if member_migration.is_none() {
            let legacy = writer_conn
                .prepare("SELECT 1 FROM pragma_table_info('api_keys') WHERE name='daily_images'")?
                .exists([])?;
            if legacy {
                return Err(DbError::Invalid(
                    "旧权限库必须使用网关启动迁移并显式提供时间与宽限期".into(),
                ));
            }
        }
        bootstrap::run(&mut writer_conn)?;
        archive_removal::run(&mut writer_conn, &path)?;
        let (now, idle_days) = member_migration.unwrap_or((0.0, 0));
        policy_migration::run(&mut writer_conn, &path, now, idle_days)?;

        let mut pool = Vec::with_capacity(READERS);
        for _ in 0..READERS {
            let c = Connection::open_with_flags(
                &path,
                OpenFlags::SQLITE_OPEN_READ_ONLY
                    | OpenFlags::SQLITE_OPEN_NO_MUTEX
                    | OpenFlags::SQLITE_OPEN_URI,
            )?;
            c.busy_timeout(Duration::from_secs(15))?;
            pool.push(c);
        }

        // 有界队列：写线程卡住时调用方会等待而不是无限堆积内存
        let (tx, rx) = std::sync::mpsc::sync_channel::<Job>(1024);
        let writer_path = path.clone();
        std::thread::Builder::new()
            .name("gate-db-writer".into())
            .spawn(move || {
                let mut conn = writer_conn;
                while let Ok(job) = rx.recv() {
                    // 某个写任务 panic 不能拖死写线程（否则之后所有记账都静默失败，直到重启）。
                    // 这个任务的事务已随 Transaction 的 Drop 回滚，调用方收到 WriterGone；
                    // panic 之后连接状态不可信，换一条新连接再接着处理队列。
                    if catch_unwind(AssertUnwindSafe(|| job(&mut conn))).is_err() {
                        match reopen_writer(&writer_path) {
                            Ok(fresh) => {
                                conn = fresh;
                                tracing::error!("数据库写任务 panic，已回滚并重开写连接");
                            }
                            Err(e) => {
                                tracing::error!(error = %e, "数据库写任务 panic，重开写连接失败，继续用原连接");
                            }
                        }
                    }
                }
                // 通道关闭（进程退出）：做一次被动 checkpoint，让 WAL 尽量并回主库
                let _ = conn.execute_batch("PRAGMA wal_checkpoint(PASSIVE);");
            })
            .expect("能创建写线程");

        Ok(Self {
            inner: Arc::new(Inner {
                path,
                writer: tx,
                read_slots: Arc::new(tokio::sync::Semaphore::new(pool.len())),
                readers: Mutex::new(pool),
            }),
        })
    }

    pub fn path(&self) -> &Path {
        &self.inner.path
    }

    /// 在写线程上执行一个事务；闭包返回 Err 或 panic 时整个事务回滚。
    pub async fn write<R, F>(&self, f: F) -> Result<R>
    where
        R: Send + 'static,
        F: Fn(&Transaction<'_>) -> rusqlite::Result<R> + Send + 'static,
    {
        let (tx, rx) = oneshot::channel();
        let job: Job = Box::new(move |conn: &mut Connection| {
            let mut attempt = 0;
            let result = loop {
                attempt += 1;
                let r = (|| {
                    let t = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
                    let out = f(&t)?;
                    t.commit()?;
                    Ok(out)
                })();
                match r {
                    Err(e) if is_busy(&e) && attempt < WRITE_ATTEMPTS => {
                        std::thread::sleep(Duration::from_millis(50 * u64::from(attempt)));
                    }
                    other => break other,
                }
            };
            let _ = tx.send(result);
        });
        self.inner
            .writer
            .send(job)
            .map_err(|_| DbError::WriterGone)?;
        rx.await
            .map_err(|_| DbError::WriterGone)?
            .map_err(DbError::from)
    }

    /// 在写连接上直接执行（**不**开事务）：`PRAGMA wal_checkpoint` 之类不能在事务里跑的语句用它。
    /// 和 [`Db::write`] 排在同一个队列里，所以不会和别的写入交错。
    pub async fn write_raw<R, F>(&self, f: F) -> Result<R>
    where
        R: Send + 'static,
        F: FnOnce(&mut Connection) -> rusqlite::Result<R> + Send + 'static,
    {
        let (tx, rx) = oneshot::channel();
        let job: Job = Box::new(move |conn: &mut Connection| {
            let _ = tx.send(f(conn));
        });
        self.inner
            .writer
            .send(job)
            .map_err(|_| DbError::WriterGone)?;
        rx.await
            .map_err(|_| DbError::WriterGone)?
            .map_err(DbError::from)
    }

    /// 在只读连接上执行查询（WAL 快照读，不阻塞写）。
    pub async fn read<R, F>(&self, f: F) -> Result<R>
    where
        R: Send + 'static,
        F: FnOnce(&Connection) -> rusqlite::Result<R> + Send + 'static,
    {
        let inner = self.inner.clone();
        // 先在异步信号量上排队，再进阻塞线程。以前是每个读请求先占一个阻塞线程、在里面每 2 毫秒
        // 轮询一次空闲连接：晚高峰网关有 37 个阻塞线程同时活着（2 核机器），白占 CPU，
        // 每个线程还各留一份分配器缓存（2026-10-18 线上实测）。
        let permit = inner
            .read_slots
            .clone()
            .acquire_owned()
            .await
            .expect("读连接信号量不会关闭");
        tokio::task::spawn_blocking(move || {
            // 许可最后释放：先把连接还回池子（`lease` 在下面声明，先于 `_permit` 被丢弃）
            let _permit = permit;
            let conn = lock_pool(&inner.readers)
                .pop()
                .expect("拿到许可时池子里一定有空闲连接");
            // 查询 panic 也要把连接还回池子：池子只有 2 条，漏两次之后所有读都会永远等下去
            let lease = Lease {
                pool: &inner.readers,
                conn: Some(conn),
            };
            f(lease.conn.as_ref().expect("连接在租约里")).map_err(DbError::from)
        })
        .await
        .map_err(|_| DbError::ReaderPanicked)?
    }

    /// 读、写两条路各跑一次 `SELECT 1`，都要在 `limit` 内完成。
    /// 写那一条排在写队列里，所以写线程退出或队列卡住都会在这里暴露。
    pub async fn health(&self, limit: Duration) -> std::result::Result<(), Unhealthy> {
        let one = |c: &Connection| c.query_row("SELECT 1", [], |r| r.get::<_, i64>(0));
        match tokio::time::timeout(limit, self.read(one)).await {
            Ok(Ok(1)) => {}
            _ => return Err(Unhealthy::Read),
        }
        match tokio::time::timeout(limit, self.write_raw(move |c| one(c))).await {
            Ok(Ok(1)) => Ok(()),
            _ => Err(Unhealthy::Write),
        }
    }
}

fn lock_pool(pool: &Mutex<Vec<Connection>>) -> std::sync::MutexGuard<'_, Vec<Connection>> {
    // 锁里只做 push / pop，不会在持锁时 panic；万一中毒也照常用里面的连接
    pool.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// 借出的只读连接；Drop 时（包括 panic 展开时）还回池子。
struct Lease<'a> {
    pool: &'a Mutex<Vec<Connection>>,
    conn: Option<Connection>,
}

impl Drop for Lease<'_> {
    fn drop(&mut self) {
        if let Some(c) = self.conn.take() {
            lock_pool(self.pool).push(c);
        }
    }
}

fn reopen_writer(path: &Path) -> rusqlite::Result<Connection> {
    let conn = Connection::open(path)?;
    bootstrap::configure(&conn)?;
    Ok(conn)
}

fn is_busy(e: &rusqlite::Error) -> bool {
    matches!(e, rusqlite::Error::SqliteFailure(f, _)
        if matches!(f.code, rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked))
}
