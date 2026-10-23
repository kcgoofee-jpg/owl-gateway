//! 每把上游 Token 的图片并发槽：逐行移植 `app/concurrency.py` 的 `AdjustableLimiter`。
//!
//! 为什么不用 `tokio::sync::Semaphore`：这里要能在运行中**缩容**（后台把并发从 2 调回 1 时，
//! 正在跑的不受影响、之后的按新上限放行），还要 `wait_idle`（停用 Token 时等它手上的活跑完）。
//! 语义与旧版一致：
//! - 先到先得（FIFO），有人在等时新来的不能插队；
//! - 等待中被取消：若已被授予槽位则立即归还（不会漏槽），否则从队列里移除并唤醒下一个。
//!
//! 出图排队（站长 10-22，公平排队）不再走这里的 FIFO：谁先轮到由账号的等待队列按「今天用得少的先」排
//! （见 `scheduler.rs`），轮到的那张用 [`AdjustableLimiter::try_acquire`] 直接取槽；槽位归还 / 扩容时
//! [`AdjustableLimiter::notify`] 叫醒在等的。

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use tokio::sync::{Notify, oneshot};

#[derive(Default)]
struct State {
    limit: usize,
    active: usize,
    waiters: VecDeque<(u64, oneshot::Sender<()>)>,
    idle_waiters: Vec<oneshot::Sender<()>>,
    next_id: u64,
}

impl State {
    fn release_locked(&mut self) {
        assert!(
            self.active >= 1,
            "image limiter released without acquisition"
        );
        self.active -= 1;
        if self.active == 0 {
            for tx in self.idle_waiters.drain(..) {
                let _ = tx.send(());
            }
        }
        self.wake();
    }

    fn wake(&mut self) {
        while self.active < self.limit {
            let Some((_, tx)) = self.waiters.pop_front() else {
                break;
            };
            if tx.is_closed() {
                continue; // 等待者已经走了
            }
            self.active += 1;
            if tx.send(()).is_err() {
                // 接收端已不在（等待者的 Drop 在锁内已把自己移出队列，正常走不到这里）
                self.active -= 1;
            }
        }
    }
}

#[derive(Clone)]
pub struct AdjustableLimiter {
    state: Arc<Mutex<State>>,
    /// 槽位归还 / 上限变化时通知（`notify_waiters`）。账号的等待队列变化也借它叫醒（见 `pool.rs`）。
    notify: Arc<Notify>,
}

/// 持有期间占用一个槽；Drop 时归还。
pub struct Permit {
    limiter: AdjustableLimiter,
    released: bool,
}

impl Drop for Permit {
    fn drop(&mut self) {
        if !self.released {
            self.limiter.release();
        }
    }
}

impl Permit {
    /// 显式归还（与 Drop 等价，只是让调用点更清楚）。
    pub fn release(mut self) {
        self.released = true;
        self.limiter.release();
    }
}

impl AdjustableLimiter {
    pub fn new(limit: usize) -> Self {
        Self {
            state: Arc::new(Mutex::new(State {
                limit: limit.max(1),
                ..Default::default()
            })),
            notify: Arc::new(Notify::new()),
        }
    }

    /// 槽位归还、上限变化（以及账号等待队列变化）时会被 `notify_waiters` 的通知器。
    pub fn notify(&self) -> &Notify {
        &self.notify
    }

    /// 有空槽、也没有 FIFO 等待者时立即取一个；否则 None（不排队）。
    pub fn try_acquire(&self) -> Option<Permit> {
        let mut s = self.state.lock().expect("limiter");
        if s.active < s.limit && s.waiters.is_empty() {
            s.active += 1;
            return Some(Permit {
                limiter: self.clone(),
                released: false,
            });
        }
        None
    }

    pub fn limit(&self) -> usize {
        self.state.lock().expect("limiter").limit
    }

    pub fn active(&self) -> usize {
        self.state.lock().expect("limiter").active
    }

    /// 取得一个槽。future 被丢弃（取消）时不会漏槽也不会占着队列位置。
    pub async fn acquire(&self) -> Permit {
        let (id, rx) = {
            let mut s = self.state.lock().expect("limiter");
            if s.active < s.limit && s.waiters.is_empty() {
                s.active += 1;
                return Permit {
                    limiter: self.clone(),
                    released: false,
                };
            }
            let (tx, rx) = oneshot::channel();
            let id = s.next_id;
            s.next_id += 1;
            s.waiters.push_back((id, tx));
            (id, rx)
        };
        // 取消保护：如果在等待中被丢弃，Guard 的 Drop 负责善后
        struct Waiting {
            limiter: AdjustableLimiter,
            id: u64,
            rx: Option<oneshot::Receiver<()>>,
        }
        impl Drop for Waiting {
            fn drop(&mut self) {
                let Some(mut rx) = self.rx.take() else { return };
                // 必须在锁内判断「是否已被授予」：wake() 在锁内发信号，锁外判断会有竞态漏槽
                let mut s = self.limiter.state.lock().expect("limiter");
                if matches!(rx.try_recv(), Ok(())) {
                    // 已经被授予了槽位但没来得及用：归还
                    s.release_locked();
                } else {
                    s.waiters.retain(|(i, _)| *i != self.id);
                    s.wake();
                }
            }
        }
        let mut w = Waiting {
            limiter: self.clone(),
            id,
            rx: Some(rx),
        };
        let rx = w.rx.as_mut().expect("刚放进去");
        // 发送端只会在授予时 send；若发送端被丢弃（不会发生，State 持有它）就当作授予失败重新排队
        let _ = rx.await;
        w.rx = None; // 已消费，Drop 不再处理
        Permit {
            limiter: self.clone(),
            released: false,
        }
    }

    fn release(&self) {
        self.state.lock().expect("limiter").release_locked();
        self.notify.notify_waiters();
    }

    /// 调整上限：变大立即唤醒等待者；变小只影响之后的放行。
    pub fn resize(&self, limit: usize) {
        {
            let mut s = self.state.lock().expect("limiter");
            s.limit = limit.max(1);
            s.wake();
        }
        self.notify.notify_waiters();
    }

    /// 等到没有在跑的任务（停用 Token 时用）。
    pub async fn wait_idle(&self) {
        let rx = {
            let mut s = self.state.lock().expect("limiter");
            if s.active == 0 {
                return;
            }
            let (tx, rx) = oneshot::channel();
            s.idle_waiters.push(tx);
            rx
        };
        let _ = rx.await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn fifo_and_no_leak_on_cancel() {
        let l = AdjustableLimiter::new(1);
        let p1 = l.acquire().await;
        // 第二个等待者被取消
        let l2 = l.clone();
        let waiting = tokio::spawn(async move { l2.acquire().await });
        tokio::time::sleep(Duration::from_millis(10)).await;
        waiting.abort();
        let _ = waiting.await;
        // 第三个等待者能在 p1 释放后拿到槽
        let l3 = l.clone();
        let third = tokio::spawn(async move { l3.acquire().await });
        tokio::time::sleep(Duration::from_millis(10)).await;
        drop(p1);
        let p3 = tokio::time::timeout(Duration::from_secs(1), third)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(l.active(), 1);
        drop(p3);
        assert_eq!(l.active(), 0);
    }

    #[tokio::test]
    async fn try_acquire_never_queues_and_release_notifies() {
        let l = AdjustableLimiter::new(1);
        let p = l.try_acquire().expect("空槽");
        assert!(l.try_acquire().is_none(), "满了不排队");
        let woke = l.notify().notified();
        tokio::pin!(woke);
        woke.as_mut().enable();
        drop(p);
        tokio::time::timeout(Duration::from_secs(1), woke)
            .await
            .expect("归还时通知");
        assert!(l.try_acquire().is_some());
    }

    #[tokio::test]
    async fn resize_and_wait_idle() {
        let l = AdjustableLimiter::new(1);
        let a = l.acquire().await;
        let l2 = l.clone();
        let b = tokio::spawn(async move { l2.acquire().await });
        tokio::time::sleep(Duration::from_millis(10)).await;
        l.resize(2); // 扩容后等待者立即放行
        let b = tokio::time::timeout(Duration::from_secs(1), b)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(l.active(), 2);
        l.resize(1); // 缩容不影响正在跑的
        let l3 = l.clone();
        let idle = tokio::spawn(async move { l3.wait_idle().await });
        drop(a);
        assert!(!idle.is_finished());
        drop(b);
        tokio::time::timeout(Duration::from_secs(1), idle)
            .await
            .unwrap()
            .unwrap();
    }
}
