//! 排队与准入：每把 Key 同时只发 1 张、每人最多排 `queue_per_member` 张，以及硬熔断与并发槽。

use crate::error::{ApiError, GateError, InternalError};
use crate::key::Key;
use crate::reqctx::ReqCtx;
use crate::state::AppState;
use gate_core::reasons::{self, Reason};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

/// 一次出图的估算消耗。
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Est {
    pub anlas: f64,
    pub v5: i64,
    /// 带参考图 / Vibe：没开 Anlas 的 Key 也能抢全站 Vibe 池。
    pub is_vibe: bool,
}

impl Est {
    pub fn from_cost(c: gate_core::policy::ImageCost) -> Self {
        Self {
            anlas: c.anlas as f64,
            v5: c.v5,
            is_vibe: false,
        }
    }
}

fn gate(status: u16, r: Reason) -> ApiError {
    GateError::with_code(status, r.text().to_string(), r.code().to_string()).into()
}

fn guard_io(e: crate::guard::GuardIoError) -> ApiError {
    InternalError(e.to_string()).into()
}

/// 当前可用的上游账号（`[t for t in pool if t.usable]`）。
pub async fn usable_token_ids(state: &AppState) -> Vec<String> {
    let now = state.now();
    state
        .nai
        .tokens()
        .await
        .into_iter()
        .filter(|t| t.usable(now))
        .map(|t| t.token_id.clone())
        .collect()
}

async fn block_reasons(state: &AppState, pool: &[String]) -> Result<Vec<Option<Reason>>, ApiError> {
    let day = state.day();
    let mut out = Vec::with_capacity(pool.len());
    for t in pool {
        out.push(
            state
                .guard
                .token_block_reason(t, &day, state.now())
                .await
                .map_err(guard_io)?,
        );
    }
    Ok(out)
}

/// 所有可用账号都被账号保护拦下（熔断 / 每日上限——都是「等也不会好」的原因）时直接 429 并说明原因。
/// 每分钟次数检查之前先看一次：这时客户端的自动重试应该拿到真正的原因（例如 daily_cap），
/// 而不是被算成这把 Key 请求过快（key_rate）。出图准入（[`image_admission`]）再看一次；
/// 这里不替成员排队等，直接返回原因。
pub async fn guard_hard_block(state: &AppState) -> Result<(), ApiError> {
    match all_blocked(state).await? {
        Some(r) => Err(gate(hard_block_status(r.code()), r)),
        None => Ok(()),
    }
}

/// Daily account cap will not clear before the reset: 403 so clients don't auto-retry; breaker etc. are transient (429).
fn hard_block_status(code: &str) -> u16 {
    if code == reasons::DAILY_CAP { 403 } else { 429 }
}

/// 所有可用账号都被拦时返回第一个原因；没有可用账号、或任一账号没被拦，都返回 `None`（放行）。
async fn all_blocked(state: &AppState) -> Result<Option<Reason>, ApiError> {
    let pool = usable_token_ids(state).await;
    if pool.is_empty() {
        return Ok(None);
    }
    let rs = block_reasons(state, &pool).await?;
    if rs.iter().any(Option::is_none) {
        return Ok(None);
    }
    Ok(rs.into_iter().next().flatten())
}

/// 全站排队上限（排队 + 正在生成）：每个可用账号取 min(每账号排队数, 排队超时内它真能开始的张数) 相加
/// （见 `GuardCore::queue_cap`）。账号被 429 拖慢了间隔，它能接的就跟着变少，不会排进来干等到超时。
pub async fn queue_cap(state: &AppState) -> Option<i64> {
    let caps = state.nai.queue_caps(state.limits().queue_timeout).await;
    state.guard.with(|g| g.queue_cap(&caps))
}

/// 排队登记：每把 Key 同时生成 1 张、每人最多同时排 `queue_per_member` 张；全站排队总数有上限。
pub struct Admission {
    guard: Arc<crate::guard::Guard>,
    key_id: i64,
    entry_id: i64,
}

impl Drop for Admission {
    fn drop(&mut self) {
        self.guard.release_image(self.key_id, Some(self.entry_id));
    }
}

/// 正在排队的计数：创建前先 `fetch_add(1)`，离开（含超时 / 客户端断开）时自动减一。
struct Waiting<'a>(&'a AppState);

impl Drop for Waiting<'_> {
    fn drop(&mut self) {
        self.0.global_waiting.fetch_sub(1, Ordering::Relaxed);
    }
}

fn queue_timeout() -> GateError {
    GateError::with_code(
        429,
        "站点繁忙，排队超时，未扣费。请稍后重试。",
        reasons::QUEUE_TIMEOUT,
    )
}

/// 排进队以后等轮到这把 Key 的这一张：前一张出完（或被拒 / 取消）才往下走。超过排队超时回 429。
async fn wait_key_turn(state: &AppState, key_id: i64, entry_id: i64) -> Result<(), GateError> {
    if state.guard.with(|g| g.key_turn(key_id, entry_id)) {
        return Ok(());
    }
    state.global_waiting.fetch_add(1, Ordering::Relaxed);
    let _w = Waiting(state);
    let turn = async {
        loop {
            let released = state.guard.released.notified();
            tokio::pin!(released);
            released.as_mut().enable();
            if state.guard.with(|g| g.key_turn(key_id, entry_id)) {
                return;
            }
            released.await;
        }
    };
    let t = state.limits().queue_timeout;
    tokio::time::timeout(Duration::from_secs_f64(t.max(0.0)), turn)
        .await
        .map_err(|_| queue_timeout())
}

pub async fn image_admission(
    state: &AppState,
    ctx: &ReqCtx,
    key: &Key,
) -> Result<Admission, ApiError> {
    guard_hard_block(state).await?;
    let cap = queue_cap(state).await;
    let key_id = key.id();
    let now = state.now();
    // 登记与取编号在同一次加锁里完成，不会被别人插队
    let admitted = state.guard.with(|g| match g.admit_image(key_id, cap, now) {
        Some(r) => Err(r),
        None => Ok(g.entries.last().map_or(0, |e| e.id)),
    });
    let entry_id = admitted.map_err(|r| gate(429, r))?;
    // 先拿到登记：等待时超时 / 客户端断开，登记照样归还
    let admission = Admission {
        guard: state.guard.clone(),
        key_id,
        entry_id,
    };
    wait_key_turn(state, key_id, entry_id).await?;
    // 公平排队（站长 10-22）：上游账号空出来时先给今天出得少的成员；轮到这一张时再取，前一张刚出完也算进去
    let images_today = state.db.get_counter(key_id, &state.day()).await?.images;
    ctx.set_share(images_today);
    let guard = state.guard.clone();
    ctx.on_sent(move || guard.with(|g| g.mark_running(key_id, Some(entry_id))));
    // 上游客户端排到 / 换到账号后报「还要等多久」：成员看到的排队位次按它排（两个账号各排各的）
    let guard = state.guard.clone();
    ctx.on_queued(move |eta| guard.with(|g| g.set_eta(entry_id, eta)));
    Ok(admission)
}

/// 「正在处理」计数（`/public/status` 的 active）：持有期间 +1，丢弃时 -1。
/// 不再占并发槽：出图由 [`image_admission`] 的排队保证同一把 Key 同时只生成 1 张，
/// 标签补全由补全队列保证全站一次一条。
pub struct Slots {
    state: AppState,
}

impl Drop for Slots {
    fn drop(&mut self) {
        self.state.global_active.fetch_sub(1, Ordering::Relaxed);
    }
}

pub fn acquire_slots(state: &AppState) -> Slots {
    state.global_active.fetch_add(1, Ordering::Relaxed);
    Slots {
        state: state.clone(),
    }
}
