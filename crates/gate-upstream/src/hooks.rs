//! 网关注入给上游客户端的能力（全部由 gate-server 实现；测试里用内存版）。

use crate::pool::Token;
use bytes::Bytes;
use std::future::Future;
use std::pin::Pin;

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;
pub type HookResult<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// 时间、事件告警、账号保护（guard）、上游计数（数据库）、结果校验。
pub trait Hooks: Send + Sync + 'static {
    /// 当前服务日（`state.day()`）。
    fn day(&self) -> String;
    /// 墙钟时间（Unix 秒）。
    fn now(&self) -> f64;
    /// 告警事件（同类有冷却，由实现方处理）。
    fn event(&self, kind: &str, message: &str, cooldown: f64);
    /// 某个账号的冷却状态变了（429 / 403）：实现方存起来，重启后用 [`Token::restore_cooldown`] 恢复。
    fn cooldown_changed(&self, _token_id: &str, _cooldown: crate::pool::Cooldown) {}
    /// 上游明确返回 401 时持久化该凭据指纹的隔离状态。调用方等待完成后才结束请求。
    fn unauthorized<'a>(&'a self, _token_id: &'a str) -> BoxFuture<'a, HookResult<()>> {
        Box::pin(async { Ok(()) })
    }
    /// 已验证凭据替换后清除原指纹隔离，和 401 持久化使用同一序列化边界。
    fn clear_unauthorized<'a>(&'a self, _token_id: &'a str) -> BoxFuture<'a, HookResult<()>> {
        Box::pin(async { Ok(()) })
    }

    /// 是否接了账号保护（旧版 `self.guard is not None`）。
    fn has_guard(&self) -> bool;
    /// 账号保护：这把 Token 现在能不能出图；不能时返回 (给成员的原因, 原因码)。
    fn token_block_reason<'a>(
        &'a self,
        token_id: &'a str,
        day: &'a str,
    ) -> BoxFuture<'a, HookResult<Option<(String, String)>>>;
    /// 账号保护：每张出完后歇的 (最少, 最多) 秒数（`guard_post_jitter_min` / `_max`）。
    fn post_jitter(&self) -> (f64, f64);

    fn upstream_v5_counter<'a>(
        &'a self,
        token_id: &'a str,
        day: &'a str,
    ) -> BoxFuture<'a, HookResult<i64>>;
    fn bump_upstream_v5<'a>(
        &'a self,
        token_id: &'a str,
        day: &'a str,
    ) -> BoxFuture<'a, HookResult<()>>;
    fn bump_upstream_images<'a>(
        &'a self,
        token_id: &'a str,
        day: &'a str,
        n: i64,
    ) -> BoxFuture<'a, HookResult<()>>;

    /// 校验上游返回的出图结果是否完整（`image_tools.validate_result`），在阻塞线程里跑。
    fn validate_image_result(
        &self,
        body: Bytes,
        expected_images: usize,
    ) -> BoxFuture<'static, bool>;
}

/// 单次请求的观察者（旧版 `request_timing` 的 contextvars）：记录「已发往上游」与上游状态码。
pub trait Observer: Send + Sync {
    /// 选中了上游账号（`token_id`）：在排队等出图槽位之前调用，所以选号之后才失败的请求也知道是哪个账号。
    /// 重试换号会再调用一次，以最后一次为准。
    fn picked(&self, _token_id: &str) {}
    /// 正在等 `token_id` 这个账号：估计还要 `wait` 秒才能开始（排队期间会更新；换号后报新账号）。
    fn queued(&self, _token_id: &str, _wait: f64) {}
    fn sent(&self) {}
    fn status(&self, _code: u16) {}
    /// 这个成员今天已经出了几张：公平排队按它排（少的先，见 [`crate::pace::fair_cmp`]）。默认 0。
    fn share(&self) -> i64 {
        0
    }
    /// 上游对 `token_id` 回了「并发锁」（上一张还没释放；不是限流，网关会在同一账号上稍后重试）。
    fn locked(&self, _token_id: &str) {}
}

pub struct NoObserver;
impl Observer for NoObserver {}

/// V5 额度确认（`AllowanceCache.resolve`）：由 gate-server 实现（需要库里的阈值与周期样本）。
pub trait Allowance: Send + Sync {
    /// 返回官方免费额度是否已耗尽；查询失败返回 Err(给成员的文案)。
    fn resolve<'a>(&'a self, token: &'a Token) -> BoxFuture<'a, Result<bool, String>>;

    /// 最近一次读到的这个账号的 Anlas 余额（订阅接口里的读数）；没读到为 None。
    fn anlas_balance(&self, _token_id: &str) -> Option<i64> {
        None
    }

    /// 这次请求如果按 Anlas 付，估计要花多少 Anlas（0 = 不知道）。
    fn paid_cost(&self) -> f64 {
        0.0
    }

    /// 这个账号最近读到的余额不够这次按 Anlas 付（至少 1 Anlas）。读不到余额不算不够：
    /// 没读数时照常发，真不够上游会回 402（见 dispatch 的 402 处理）。
    fn anlas_short(&self, token_id: &str) -> bool {
        self.anlas_balance(token_id)
            .is_some_and(|b| (b as f64) < self.paid_cost().max(1.0))
    }
}
