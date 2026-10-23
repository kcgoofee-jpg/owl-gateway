//! 墙钟时间（Unix 秒，f64）。集中在这里，测试可以替换。

use std::sync::atomic::{AtomicU64, Ordering};

/// 测试用：非零时固定返回这个时间（f64 的位表示）。
static FROZEN: AtomicU64 = AtomicU64::new(0);

pub fn now() -> f64 {
    let f = FROZEN.load(Ordering::Relaxed);
    if f != 0 {
        return f64::from_bits(f);
    }
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

/// 冻结时钟（仅测试）。传 None 恢复真实时间。
pub fn freeze(at: Option<f64>) {
    FROZEN.store(at.map(f64::to_bits).unwrap_or(0), Ordering::Relaxed);
}
