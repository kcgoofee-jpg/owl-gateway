//! 出图节奏：「这个上游账号的下一张什么时候能发」只在这里算。
//!
//! - 出完后歇一下（站长 10-23：出图节奏只剩这一个手动旋钮）：出图结束（成功或失败，只要真的发往了上游）后，
//!   这个账号的下一张最早 = max(原定, 现在 + 歇的秒数)，秒数在 [最少, 最多]（账号保护 `post_jitter_min` /
//!   `post_jitter_max`，下限 1 秒，经 [`Hooks::post_jitter`]）里均匀随机。下限 1 秒同时是「上游并发锁比响应晚一点
//!   释放」要的那一下。开始到开始的出图间隔与间隔抖动已删除。测试环境（[`crate::client::Config::pace`] = false）不歇；
//! - 等槽位（公平排队，站长 10-22）：等这个账号的图里只有排在最前的那张（[`crate::pace::fair_cmp`]：今天出得少的先，
//!   等久了的按先来后到）能开始；它在歇完、并发槽有空时**同时**占槽和记下开始时刻，所以账号空出来那一刻才决定谁上，
//!   后来的轻度用户能排到前面。整体的排队超时与换号由调用方（`dispatch.rs`）管；
//! - 上游回了并发锁时，这个账号的下一张定在退避秒数之后（[`Scheduler::hold_for_lock`]，不算 429）；
//! - 占了槽却没发出去（排完队被拒、换号、客户端断开）：[`Scheduler::refund`] 把占位还回去；
//! - 估算：[`Scheduler::est_start`] 给出「再来一张要多久才能开始」，选号和排队上限都按它。
//!
//! 时钟是单调时钟（客户端启动以来的秒），[`crate::pool::TokenFields::image_next_at`] 用的就是它。
//! `image_next_at` 的读改写都在调度器自己的锁下串行（不跨 await）。

use crate::hooks::Hooks;
use crate::limiter::Permit;
use crate::pace;
use crate::pool::Token;
use std::sync::Mutex;
use std::time::Duration;

/// 等轮到时最多隔多久自己再看一次（位次会随时间变：等久了的会排到最前）。
const RECHECK: f64 = 0.5;

pub(crate) struct Scheduler {
    base: tokio::time::Instant,
    /// 出完后歇不歇（测试环境不歇）。
    pace: bool,
    /// 串行化所有 `image_next_at` 的读改写。
    lock: Mutex<()>,
}

/// 一次占到的开始时刻：发出去之前被放弃时用它退还（见 [`Scheduler::refund`]）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Turn {
    prev: f64,
    reserved: f64,
}

impl Scheduler {
    pub(crate) fn new(pace: bool) -> Self {
        Self {
            base: tokio::time::Instant::now(),
            pace,
            lock: Mutex::new(()),
        }
    }

    /// 客户端启动以来的单调秒数。
    pub(crate) fn mono(&self) -> f64 {
        self.base.elapsed().as_secs_f64()
    }

    /// 上游对这个账号回了 429：只记下时刻（后台显示用）；间隔不变，冷却由调用方处理。
    pub(crate) fn on_429(&self, token: &Token, hooks: &dyn Hooks) {
        let now = hooks.now();
        token.with(|f| f.last_429 = now);
    }

    /// 出完后歇的 [最少, 最多] 秒数（`最多 < 最少` 按最少）；测试环境 (0, 0)。
    fn rest_range(&self, hooks: &dyn Hooks) -> (f64, f64) {
        if !self.pace {
            return (0.0, 0.0);
        }
        let (min, max) = hooks.post_jitter();
        let lo = min.max(0.0);
        (lo, max.max(lo))
    }

    /// 出完后平均歇几秒。
    fn post_mean(&self, hooks: &dyn Hooks) -> f64 {
        let (lo, hi) = self.rest_range(hooks);
        (lo + hi) / 2.0
    }

    /// 这个账号两次开始出图的平均间距（秒），见 [`pace::cycle`]。
    pub(crate) fn cycle(&self, token: &Token, hooks: &dyn Hooks) -> f64 {
        let gen_secs = token.with(|f| f.gen_secs);
        pace::cycle(gen_secs, self.post_mean(hooks), token.image_slots.limit())
    }

    /// 前面还有 `ahead` 张在等这个账号时，再排一张要多少秒才能开始（估算）：
    /// 等到歇完（正在生成的占满并发时，还要等它大约出完再歇完），再加上前面每张一个周期。
    pub(crate) fn est_start(&self, token: &Token, hooks: &dyn Hooks, ahead: usize) -> f64 {
        let now = self.mono();
        let cycle = self.cycle(token, hooks);
        let (next_at, last_start, gen_secs) =
            token.with(|f| (f.image_next_at, f.last_start, f.gen_secs));
        let mut base = (next_at - now).max(0.0);
        if token.image_slots.active() >= token.image_slots.limit() && last_start > 0.0 {
            let busy_until = last_start + gen_secs + self.post_mean(hooks);
            base = base.max(busy_until - now);
        }
        base + ahead as f64 * cycle
    }

    /// 记下开始时刻：下一张最早 = 现在（真正的「歇」在出完后由 [`Scheduler::after_request`] 定）。
    /// 调用方持 `lock`、且已经歇完。
    fn reserve(now: f64, token: &Token) -> Turn {
        token.with(|f| {
            let next = now;
            let prev = f.image_next_at;
            f.image_next_at = next;
            f.last_start = now;
            Turn {
                prev,
                reserved: next,
            }
        })
    }

    /// 出图槽位：等到这张（票号 `ticket`）在这个账号的公平排队里排第一、上一张出完后歇够了、并发槽有空，
    /// 然后一次占下并发槽（放进 `permit`，由调用方结算后归还；已经占着就不再占）和开始时刻（放进 `turn`），
    /// 并把票移出等待队列。`ticket` 为 None（并发锁后重试，手上已经占着槽）时不看排位。
    /// 不设超时：排队超时与换号由调用方在外面 `select!`。取消安全：占槽、记开始时刻在同一次加锁里完成，中间没有 await。
    pub(crate) async fn acquire(
        &self,
        token: &Token,
        permit: &mut Option<Permit>,
        turn: &mut Option<Turn>,
        ticket: Option<u64>,
    ) {
        loop {
            let changed = token.image_slots.notify().notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            let wait = {
                let guard = self.lock.lock().expect("scheduler");
                let now = self.mono();
                let first = ticket.is_none_or(|id| token.ahead_of(id, now) == 0);
                let due = (token.with(|f| f.image_next_at) - now).max(0.0);
                if !first {
                    RECHECK
                } else if due > 0.0 {
                    due.min(RECHECK)
                } else {
                    if permit.is_none() {
                        *permit = token.image_slots.try_acquire();
                    }
                    if permit.is_some() {
                        *turn = Some(Self::reserve(now, token));
                        drop(guard);
                        if let Some(id) = ticket {
                            token.leave_queue(id);
                        }
                        return;
                    }
                    RECHECK // 槽位归还时会被叫醒
                }
            };
            tokio::select! {
                () = &mut changed => {}
                () = tokio::time::sleep(Duration::from_secs_f64(wait)) => {}
            }
        }
    }

    /// 上游回了「并发锁」：这次不算一次真正的出图，这个账号的下一张定在 `wait` 秒后
    /// （这期间别人也不会发往这个账号）。别人占过的更晚的时刻不动。
    pub(crate) fn hold_for_lock(&self, token: &Token, turn: Option<Turn>, wait: f64) {
        let _guard = self.lock.lock().expect("scheduler");
        let next = self.mono() + wait.max(0.0);
        token.with(|f| {
            if turn.is_some_and(|t| f.image_next_at == t.reserved) {
                f.image_next_at = next;
            } else {
                f.image_next_at = f.image_next_at.max(next);
            }
        });
    }

    /// 占了槽却没发往上游：把占位还回去（只在这期间没人再改过时才还，不会抹掉别人的占位）。
    pub(crate) fn refund(&self, token: &Token, turn: Turn) {
        {
            let _guard = self.lock.lock().expect("scheduler");
            token.with(|f| {
                if f.image_next_at == turn.reserved {
                    f.image_next_at = turn.prev;
                }
            });
        }
        token.image_slots.notify().notify_waiters();
    }

    /// 出完后歇一下：这个账号的下一张最早 = max(原定, 现在 + [最少, 最多] 里均匀随机的秒数)。
    /// 既拟人化（消除「上一张一结束下一张立刻开始」的贴脸节奏），也给上游并发锁留出释放的时间（下限 1 秒）。
    /// 调用方只对真正发往上游的请求调用；测试环境不歇。
    pub(crate) fn after_request(&self, token: &Token, hooks: &dyn Hooks) {
        let (lo, hi) = self.rest_range(hooks);
        if hi <= 0.0 {
            return;
        }
        let delay = uniform(lo, hi);
        let now = self.mono();
        let _guard = self.lock.lock().expect("scheduler");
        token.with(|f| f.image_next_at = f.image_next_at.max(now + delay));
    }
}

/// Python `random.uniform(a, b)` = `a + (b - a) * random()`。
fn uniform(a: f64, b: f64) -> f64 {
    a + (b - a) * rand::random::<f64>()
}
