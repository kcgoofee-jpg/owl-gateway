//! 每个上游账号的出图节奏估算（纯函数，便于单测）。
//!
//! 出图节奏只有一个手动旋钮（站长 10-23）：每张出完后这个账号歇 N～M 秒（均匀随机，账号保护
//! `post_jitter_min` / `post_jitter_max`，下限 1 秒）。开始到开始的「出图间隔」和「间隔抖动」已删除——
//! 并发 1 时出图本身就要七八秒，间隔从来不起作用。上游 429 只让这个账号冷却（安全反应）。
//!
//! - **周期**：一个账号两次开始出图的平均间距 = (实测出图耗时 + 平均歇的秒数) ÷ 并发。
//!   选号（谁能最早开始）和排队上限（排队超时内真能出完几张）都按它估算。
//! - **排队顺序**（[`Arrival`] / [`fair_cmp`]）：一个账号空出来时，等它的图里先轮到**插队**（站长号 / 赞助席），
//!   再轮到**短作业车道**（V5），再普通。同车道按先来后到。防饿死：等了一半排队超时还没开始的，在本车道内直接排到最前；
//!   等满整个超时的才压过 V5 车道（不能压过插队）。11-07 删掉了「这个成员今天出得最少」那一层 —— 它让所有人都要多等，而防饿死靠的是老化。
//! - **上游并发锁**（NovelAI 429 / 409 + `Concurrent generation is locked`）：这个账号上一张还没释放，
//!   不是限流——不冷却、不算 429；在同一个账号上等 [`lock_wait`] 秒再试，最多 [`LOCK_RETRIES`] 次
//!   （不超过排队超时）。锁在响应返回后稍晚才释放，所以出完后歇的秒数下限是 1。

/// 上游并发锁后第一次重试前等几秒（之后每次翻倍：2、4、8）。
pub const LOCK_RETRY_BASE: f64 = 2.0;
/// 上游并发锁最多重试几次（还受排队超时限制）。
pub const LOCK_RETRIES: u32 = 3;

/// 第 `retry` 次（0 起）并发锁重试前等几秒；次数用完返回 None。
pub fn lock_wait(retry: u32) -> Option<f64> {
    (retry < LOCK_RETRIES).then(|| LOCK_RETRY_BASE * f64::from(1u32 << retry.min(16)))
}

/// 一张图排队时的公平信息：选号、换号、轮到谁、成员看到的位次都按它（换号时不变）。
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Arrival {
    /// 开始排队的时刻（上游客户端的单调秒）。
    pub since: f64,
    /// 到这个时刻（单调秒）还没开始就不再看用量，按先来后到排在最前（防饿死：开始排队 + 排队超时的一半）。
    /// 只在同一条车道内生效——跨车道的抢先看 [`Self::hard_at`]。
    pub patient_at: f64,
    /// 到这个时刻（单调秒）还没开始就压过 V5 短作业车道（防饿死：开始排队 + 整个排队超时）。
    /// 半个超时只在车道内抢先，是因为拥塞时几乎每张普通请求都过 45 秒，用两条不同的阈值才能既让 V5 真的先出、
    /// 又不把普通请求饿死。硬老化不能压过插队：赞助 / 站长号始终排在等待中的生图前面。
    pub hard_at: f64,
    /// 短作业车道：V5（`guard_v5_priority`）。实测 V5 上游只占 4 秒左右、且没有长尾，让它先于同条件的普通请求开始。
    /// 客户端指定无效。站长号 / 赞助席不走这条，见 [`Self::jump_lane`]。
    pub fast_lane: bool,
    /// 插队车道：站长号（`owner_image_full`）和赞助席（`queue_jump`）。同一个账号空出来时排在所有等待中的
    /// V5 / V4.5 前面；不抢占已经发往上游的那张。多名插队按先来后到。
    pub jump_lane: bool,
}

impl Arrival {
    pub fn new(now: f64, queue_timeout: f64) -> Self {
        Self {
            since: now,
            patient_at: now + queue_timeout.max(0.0) / 2.0,
            hard_at: now + queue_timeout.max(0.0),
            fast_lane: false,
            jump_lane: false,
        }
    }

    /// 标记成短作业车道（[`Self::fast_lane`]）。
    pub fn fast_lane(mut self, fast: bool) -> Self {
        self.fast_lane = fast;
        self
    }

    /// 标记成插队车道（[`Self::jump_lane`]）。
    pub fn jump_lane(mut self, jump: bool) -> Self {
        self.jump_lane = jump;
        self
    }
}

/// 排队的先后（`Less` = `a` 先轮到）：
/// 1. **车道**：插队（站长 / 赞助）先于短作业（V5）先于普通；等满整个排队超时的普通 / V5（`hard_at`）只压过 V5 车道，压不过插队；
/// 2. **老化**：等满半个超时的（`patient_at`）在车道内不再排队、直接排到本车道最前；
/// 3. **先来后到**：其余按开始排队的时刻；
/// 4. 同一时刻的按票号。
///
/// 11-07 删掉了「今天出得少的人先」：那个比较让所有人都要多等（重度用户沉底、轻度用户被后来的轻度用户插队），
/// 而真正防饿死的是老化；删掉之后排队就是「车道 + 老化 + 先来后到」，谁都能算出自己还要等几张。
///
/// 为什么老化分两档：11-07 上线实测发现，拥塞时几乎每张普通请求都过了 45 秒，若「等半个超时」就能越过车道，
/// V5 一次也插不进去（18:05 那批 V5 等了 52 秒、79 秒）。半个超时留在车道内防饿死，整个超时才允许跨过 V5。
/// 11-12 起插队单独一条车道：V5 仍是模型车道，赞助 / 站长排在所有等待中的生图前面，90 秒老化也不能越过他们。
pub fn fair_cmp(a: (&Arrival, u64), b: (&Arrival, u64), now: f64) -> std::cmp::Ordering {
    let key = |(x, id): (&Arrival, u64)| {
        let aged = now >= x.patient_at;
        let hard = now >= x.hard_at;
        let lane = if x.jump_lane {
            0u8
        } else if x.fast_lane || hard {
            1
        } else {
            2
        };
        (lane, !aged, x.since, id)
    };
    let (ka, kb) = (key(a), key(b));
    ka.0.cmp(&kb.0)
        .then(ka.1.cmp(&kb.1))
        .then(ka.2.total_cmp(&kb.2))
        .then(ka.3.cmp(&kb.3))
}

/// 两次开始出图的平均间距（秒）。`gen_secs` = 实测出图耗时（没有样本为 0），`post_mean` = 平均歇的秒数，
/// `concurrency` ≥ 1。
pub fn cycle(gen_secs: f64, post_mean: f64, concurrency: usize) -> f64 {
    (gen_secs.max(0.0) + post_mean.max(0.0)) / concurrency.max(1) as f64
}

/// 排队超时之内这个账号真能开始几张（含正在生成的那张）：⌈超时 ÷ 周期⌉，至少 1。周期为 0（测试环境）= 不限。
pub fn realistic_queue(queue_timeout: f64, cycle: f64) -> Option<i64> {
    if cycle.is_nan() || cycle <= 0.0 {
        return None;
    }
    let n = (queue_timeout.max(0.0) / cycle).ceil();
    Some(if n.is_finite() {
        (n as i64).max(1)
    } else {
        i64::MAX
    })
}

/// 出图耗时的滑动平均（新样本占 20%；第一个样本直接用）。
pub fn ewma(old: f64, sample: f64) -> f64 {
    if !(sample.is_finite() && sample >= 0.0) {
        return old;
    }
    if old <= 0.0 {
        sample
    } else {
        old * 0.8 + sample * 0.2
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cycle_and_realistic_queue() {
        // 出图 7.6 秒 + 平均歇 2 秒；并发 2 时减半；还没有样本时只剩歇的秒数
        assert_eq!(cycle(7.5, 2.0, 1), 9.5);
        assert_eq!(cycle(25.0, 3.0, 1), 28.0);
        assert_eq!(cycle(25.0, 10.0, 2), 17.5);
        assert_eq!(cycle(0.0, 0.0, 1), 0.0);
        assert_eq!(realistic_queue(90.0, 17.5), Some(6));
        assert_eq!(realistic_queue(90.0, 30.0), Some(3));
        assert_eq!(realistic_queue(10.0, 120.0), Some(1));
        assert_eq!(realistic_queue(90.0, 0.0), None);
    }

    #[test]
    fn lock_retries_back_off_and_stop() {
        assert_eq!(lock_wait(0), Some(2.0));
        assert_eq!(lock_wait(1), Some(4.0));
        assert_eq!(lock_wait(2), Some(8.0));
        assert_eq!(lock_wait(3), None);
    }

    #[test]
    fn arrival_order_then_the_aged_jump_the_line() {
        use std::cmp::Ordering::*;
        let first = Arrival::new(0.0, 90.0);
        let second = Arrival::new(10.0, 90.0);
        // 先来的先跑（11-07 起不再按今天出了几张排）
        assert_eq!(fair_cmp((&first, 1), (&second, 2), 20.0), Less);
        // 同一个时刻按票号
        let same = Arrival::new(10.0, 90.0);
        assert_eq!(fair_cmp((&second, 2), (&same, 3), 20.0), Less);
        // 后来的等过一半超时（45 秒）就排到本车道最前：first 在 0 秒到达、45 秒时变「等久了」
        let late = Arrival::new(40.0, 90.0);
        assert_eq!(fair_cmp((&first, 1), (&late, 3), 50.0), Less);
    }

    #[test]
    fn fast_lane_wins_until_the_other_side_has_waited_the_whole_timeout() {
        use std::cmp::Ordering::*;
        // 普通先到、V5 后到：V5 先上
        let plain = Arrival::new(0.0, 90.0);
        let v5 = Arrival::new(5.0, 90.0).fast_lane(true);
        assert_eq!(fair_cmp((&v5, 2), (&plain, 1), 10.0), Less);
        // 普通等过 45 秒（半个超时）：只在普通车道内抢先，压不过车道
        assert_eq!(fair_cmp((&plain, 1), (&v5, 2), 45.0), Greater);
        // 等满整个超时（90 秒）：硬老化压过车道，最后一定能跑到
        assert_eq!(fair_cmp((&plain, 1), (&v5, 2), 91.0), Less);
        // V5 自己等满整个超时后，两条都按先来后到
        assert_eq!(fair_cmp((&v5, 2), (&plain, 1), 96.0), Greater);
    }

    #[test]
    fn fast_lane_keeps_the_arrival_order_inside_the_lane() {
        use std::cmp::Ordering::*;
        let a = Arrival::new(0.0, 90.0).fast_lane(true);
        let b = Arrival::new(1.0, 90.0).fast_lane(true);
        // 同车道：先来后到，车道不改变这一点
        assert_eq!(fair_cmp((&a, 1), (&b, 2), 10.0), Less);
        assert_eq!(fair_cmp((&b, 2), (&a, 1), 10.0), Greater);
    }

    #[test]
    fn jump_lane_beats_v5_and_plain_even_after_hard_aging() {
        use std::cmp::Ordering::*;
        let plain = Arrival::new(0.0, 90.0);
        let v5 = Arrival::new(1.0, 90.0).fast_lane(true);
        let jump = Arrival::new(5.0, 90.0).jump_lane(true);
        assert_eq!(fair_cmp((&jump, 3), (&v5, 2), 10.0), Less);
        assert_eq!(fair_cmp((&jump, 3), (&plain, 1), 10.0), Less);
        // 等满 90 秒的 V4.5 / V5 仍压不过刚来的插队
        assert_eq!(fair_cmp((&plain, 1), (&jump, 3), 91.0), Greater);
        assert_eq!(fair_cmp((&v5, 2), (&jump, 3), 92.0), Greater);
        // 两名插队：先来后到
        let first = Arrival::new(0.0, 90.0).jump_lane(true);
        let second = Arrival::new(1.0, 90.0).jump_lane(true);
        assert_eq!(fair_cmp((&first, 1), (&second, 2), 10.0), Less);
        assert_eq!(fair_cmp((&second, 2), (&first, 1), 10.0), Greater);
    }

    #[test]
    fn ewma_starts_from_the_first_sample() {
        assert_eq!(ewma(0.0, 10.0), 10.0);
        assert_eq!(ewma(10.0, 20.0), 12.0);
        assert_eq!(ewma(10.0, f64::NAN), 10.0);
    }
}
