//! 每个上游账号的出图节奏估算（纯函数，便于单测）。
//!
//! 出图节奏只有一个手动旋钮（站长 10-23）：每张出完后这个账号歇 N～M 秒（均匀随机，账号保护
//! `post_jitter_min` / `post_jitter_max`，下限 1 秒）。开始到开始的「出图间隔」和「间隔抖动」已删除——
//! 并发 1 时出图本身就要七八秒，间隔从来不起作用。上游 429 只让这个账号冷却（安全反应）。
//!
//! - **周期**：一个账号两次开始出图的平均间距 = (实测出图耗时 + 平均歇的秒数) ÷ 并发。
//!   选号（谁能最早开始）和排队上限（排队超时内真能出完几张）都按它估算。
//! - **公平排队**（站长 10-22，参照 AI Horde）：一个账号空出来时，等它的图里先轮到「这个成员今天出得最少」的，
//!   一样多再按先来后到；重度用户自然沉底。防饿死：等了一半排队超时还没开始的，不再看用量，按先来后到排在最前
//!   （见 [`Arrival`] / [`fair_cmp`]）。
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
    /// 这个成员今天已经出了几张（少的先）。
    pub share: i64,
    /// 开始排队的时刻（上游客户端的单调秒）。
    pub since: f64,
    /// 到这个时刻（单调秒）还没开始就不再看用量，按先来后到排在最前（防饿死：开始排队 + 排队超时的一半）。
    pub patient_at: f64,
}

impl Arrival {
    pub fn new(share: i64, now: f64, queue_timeout: f64) -> Self {
        Self {
            share: share.max(0),
            since: now,
            patient_at: now + queue_timeout.max(0.0) / 2.0,
        }
    }
}

/// 公平排队的先后（`Less` = `a` 先轮到）：等久了的（过了 `patient_at`）在最前、按先来后到；
/// 其余按今天出的张数从少到多，一样多按先来后到；最后按票号（同一时刻到的先登记的先）。
pub fn fair_cmp(a: (&Arrival, u64), b: (&Arrival, u64), now: f64) -> std::cmp::Ordering {
    let key = |(x, id): (&Arrival, u64)| {
        let aged = now >= x.patient_at;
        (!aged, if aged { 0 } else { x.share }, x.since, id)
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
    fn light_users_first_then_arrival_and_the_aged_jump_the_line() {
        use std::cmp::Ordering::*;
        let heavy = Arrival::new(150, 0.0, 90.0);
        let light = Arrival::new(3, 10.0, 90.0);
        // 后来的轻度用户排在先来的重度用户前面
        assert_eq!(fair_cmp((&light, 2), (&heavy, 1), 20.0), Less);
        // 一样多：先来的先
        let light2 = Arrival::new(3, 12.0, 90.0);
        assert_eq!(fair_cmp((&light, 2), (&light2, 3), 20.0), Less);
        // 重度用户等过了排队超时的一半（45 秒）：不再看用量，排到最前
        assert_eq!(fair_cmp((&heavy, 1), (&light, 2), 45.0), Less);
        // 都等久了：先来后到
        assert_eq!(fair_cmp((&light, 2), (&heavy, 1), 60.0), Greater);
        // 负数用量按 0
        assert_eq!(Arrival::new(-5, 0.0, 90.0).share, 0);
    }

    #[test]
    fn ewma_starts_from_the_first_sample() {
        assert_eq!(ewma(0.0, 10.0), 10.0);
        assert_eq!(ewma(10.0, 20.0), 12.0);
        assert_eq!(ewma(10.0, f64::NAN), 10.0);
    }
}
