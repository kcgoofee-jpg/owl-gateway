//! 出图准入与额度预留。
//!
//! 每一步都是一个 RAII 守卫：被拒或客户端断开时自动归还。真正发往上游之后，守卫随上游操作一起
//! 移进后台任务（见 `routes::image::shielded`），保证「先记账、再释放额度预留和并发槽」。
//!
//! 每把 Key 的限速只有两层：这里的排队（同时生成 1 张，每人最多同时排 `queue_per_member` 张，默认 2）
//! 和每分钟次数（RPM，`auth::check_rpm`）。

use crate::error::{ApiError, GateError, InternalError};
use crate::key::Key;
use crate::reqctx::ReqCtx;
use crate::state::{AppState, Reservation};
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

/// 所有入口与界面使用同一份成员规则。
pub async fn member_anlas_cap_f64(state: &AppState, key: &Key) -> Result<f64, gate_db::DbError> {
    Ok(f64::from(key.refresh_policy(&state.db).await?.daily_anlas))
}

/// 必须持有预算锁；派发前再次读取当前规则，算上其他请求的额度预留。
pub async fn quota_image_check(
    state: &AppState,
    budget: &crate::state::Budget,
    key: &Key,
    est: &Est,
    legacy_free_images: i64,
    exclude: Option<u64>,
) -> Result<(), ApiError> {
    let policy = key.refresh_policy(&state.db).await?;
    let shared = state.db.shared_budgets().await?;
    if est.v5 > 0 && crate::ops::v5_paused(&state.db).await? {
        return Err(GateError::with_code(
            403,
            gate_core::quota::V5_PAUSED_TEXT,
            reasons::V5_EXHAUSTED,
        )
        .into());
    }
    if est.v5 > 0 && !policy.allows("v5") {
        return Err(
            GateError::with_code(403, "此成员未开放 V5，未扣费。", reasons::FEATURE_OFF).into(),
        );
    }
    if est.is_vibe && !policy.vibe {
        return Err(GateError::with_code(
            403,
            "此成员未开放 Vibe 和参考图，未扣费。",
            reasons::FEATURE_OFF,
        )
        .into());
    }
    let day = state.day();
    let month = state.month();
    let reset = state.clock.next_reset_phrase(state.now());
    let counter = state.db.get_counter(key.id(), &day).await?;
    let pending: Vec<&Reservation> = budget
        .reservations
        .iter()
        .filter(|(id, _)| Some(**id) != exclude)
        .map(|(_, r)| r)
        .collect();
    let own_day: Vec<_> = pending
        .iter()
        .filter(|r| r.key_id == key.id() && r.day == day)
        .collect();
    let own_month: Vec<_> = pending
        .iter()
        .filter(|r| r.key_id == key.id() && r.month == month)
        .collect();
    if legacy_free_images > 0 {
        let reserved: i64 = own_day.iter().map(|r| r.legacy).sum();
        if counter.legacy_free_images + reserved + legacy_free_images
            > i64::from(policy.daily_images)
        {
            return Err(GateError::with_code(
                403,
                format!(
                    "今日 V4.5 额度已用完，每天 {} 张，未扣费。{reset}重置。",
                    policy.daily_images
                ),
                reasons::MEMBER_QUOTA,
            )
            .into());
        }
    }
    if est.v5 > 0 {
        let reserved: i64 = own_day.iter().map(|r| r.v5).sum();
        if counter.v5 + reserved + est.v5 > i64::from(policy.daily_v5) {
            return Err(GateError::with_code(
                403,
                format!(
                    "今日 V5 额度已用完，每天 {} 张，未扣费。请使用 V4.5，{reset}重置。",
                    policy.daily_v5
                ),
                reasons::MEMBER_QUOTA,
            )
            .into());
        }
        let used = state.db.day_v5_total(&day).await?;
        let reserved: i64 = pending.iter().filter(|r| r.day == day).map(|r| r.v5).sum();
        if used + reserved + est.v5 > i64::from(shared.daily_v5) {
            return Err(GateError::with_code(
                403,
                format!(
                    "全站今日 V5 额度已用完，每天 {} 张，未扣费。请使用 V4.5，{reset}重置。",
                    shared.daily_v5
                ),
                reasons::SITE_QUOTA,
            )
            .into());
        }
    }
    if est.anlas > 0.0 {
        let reserved: f64 = pending
            .iter()
            .filter(|r| r.day == day)
            .map(|r| r.anlas)
            .sum();
        if state.db.day_anlas_total(&day).await? + reserved + est.anlas
            > f64::from(shared.daily_anlas)
        {
            return Err(GateError::with_code(
                403,
                format!(
                    "全站今日 Anlas 额度已用完，每天 {} Anlas，未扣费。{reset}重置。",
                    shared.daily_anlas
                ),
                reasons::SITE_QUOTA,
            )
            .into());
        }
        let reserved: f64 = own_day.iter().map(|r| r.anlas).sum();
        if counter.anlas + reserved + est.anlas > f64::from(policy.daily_anlas) {
            return Err(GateError::with_code(
                403,
                format!(
                    "今日 Anlas 额度不足：已用 {:.0}，上限 {}，未扣费。{reset}重置。",
                    counter.anlas, policy.daily_anlas
                ),
                reasons::MEMBER_QUOTA,
            )
            .into());
        }
        let reserved: f64 = own_month.iter().map(|r| r.anlas).sum();
        let used = state.db.month_anlas(key.id(), &month).await?;
        if used + reserved + est.anlas > policy.monthly_anlas {
            return Err(GateError::with_code(
                403,
                format!(
                    "本月 Anlas 额度不足：已用 {used:.0}，上限 {}，未扣费。下月恢复。",
                    policy.monthly_anlas
                ),
                reasons::MEMBER_QUOTA,
            )
            .into());
        }
        let reserved: f64 = pending
            .iter()
            .filter(|r| r.month == month)
            .map(|r| r.anlas)
            .sum();
        if state.db.month_anlas_all(&month).await? + reserved + est.anlas > shared.monthly_anlas {
            return Err(GateError::with_code(
                403,
                "全站本月 Anlas 预算已用完，未扣费。免费出图不受影响，下月恢复。",
                reasons::SITE_QUOTA,
            )
            .into());
        }
    }
    Ok(())
}

/// 已登记的额度预留；释放时在预算锁下移除（不和进行中的额度检查交错）。
#[derive(Clone, Copy)]
pub struct DispatchRequirements {
    feature: &'static str,
    img2img: bool,
    paid_parameters: bool,
}

impl DispatchRequirements {
    pub fn feature(feature: &'static str) -> Self {
        Self {
            feature,
            img2img: false,
            paid_parameters: false,
        }
    }

    pub fn image(body: &serde_json::Value) -> Result<Self, InternalError> {
        let (_, notes, problem) = gate_core::policy::clamp_image_params(
            body,
            gate_core::policy::FREE_MAX_PIXELS,
            gate_core::policy::FREE_MAX_STEPS,
            true,
        )
        .map_err(|e| InternalError(e.to_string()))?;
        let img2img = ["image", "mask"].iter().any(|name| {
            body.get("parameters")
                .and_then(|p| p.get(*name))
                .and_then(serde_json::Value::as_str)
                .is_some_and(|s| !s.is_empty())
        });
        Ok(Self {
            feature: "image",
            img2img,
            paid_parameters: !notes.is_empty() || problem.is_some(),
        })
    }
}

pub struct Reserved {
    state: AppState,
    id: u64,
    key: Key,
    request_id: String,
    requirements: DispatchRequirements,
}

impl Reserved {
    pub fn id(&self) -> u64 {
        self.id
    }

    /// V5 额度确认后改按 Anlas 估算：在预算锁下复核（不算自己），通过才改预留。
    pub async fn update(&self, est: &Est, legacy: i64) -> Result<(), ApiError> {
        let mut b = self.state.budget.lock().await;
        quota_image_check(&self.state, &b, &self.key, est, legacy, Some(self.id)).await?;
        let (day, month) = (self.state.day(), self.state.month());
        if !self
            .state
            .db
            .update_request_reservation(
                &self.request_id,
                &day,
                &month,
                est.is_vibe,
                est.anlas,
                est.v5,
                legacy,
            )
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "request reservation persistence failed");
                InternalError("保存额度预留失败，请稍后重试。".into())
            })?
        {
            return Err(InternalError("持久化额度预留已不在待派发状态".into()).into());
        }
        if let Some(r) = b.reservations.get_mut(&self.id) {
            r.anlas = est.anlas;
            r.v5 = est.v5;
            r.legacy = legacy;
            r.day = day;
            r.month = month;
            r.is_vibe = est.is_vibe;
        }
        Ok(())
    }

    /// Recheck current quota and atomically move the reservation into the dispatch period.
    pub async fn prepare_dispatch(
        &self,
        correlation_id: &str,
    ) -> Result<(String, String), ApiError> {
        let row = self.state.db.get_key(self.key.id()).await?.ok_or_else(|| {
            GateError::with_code(403, "这把 Key 已失效，未扣费。", reasons::KEY_DISABLED)
        })?;
        let current = Key::load(&self.state.db, row).await?;
        if current.subject() != self.key.subject() {
            return Err(GateError::with_code(
                403,
                "这把 Key 的归属已变更，未扣费。",
                reasons::KEY_DISABLED,
            )
            .into());
        }
        if let Some(error) = crate::auth::key_status_error(&current, self.state.now()) {
            return Err(error.into());
        }
        crate::auth::require_feature(&self.state, &current, self.requirements.feature).await?;
        if (self.requirements.img2img && !current.policy.img2img)
            || (self.requirements.paid_parameters && !current.policy.paid_parameters)
        {
            return Err(GateError::with_code(
                403,
                "成员规则已变更，本次参数不再开放，未扣费。",
                reasons::FEATURE_OFF,
            )
            .into());
        }
        let mut b = self.state.budget.lock().await;
        let Some(r) = b.reservations.get(&self.id).cloned() else {
            return Err(InternalError("请求额度预留已丢失，未发送上游请求。".into()).into());
        };
        let est = Est {
            anlas: r.anlas,
            v5: r.v5,
            is_vibe: r.is_vibe,
        };
        quota_image_check(&self.state, &b, &self.key, &est, r.legacy, Some(self.id)).await?;
        let (day, month) = (self.state.day(), self.state.month());
        let ok = self
            .state
            .db
            .dispatch_request_settlement(
                &self.request_id,
                correlation_id,
                &day,
                &month,
                r.is_vibe,
                r.anlas,
                r.v5,
                r.legacy,
                self.state.now(),
            )
            .await
            .map_err(|e| { tracing::error!(error = %e, request_id = %self.request_id, "request dispatch persistence failed"); InternalError("保存请求派发状态失败，未发送上游请求。".into()) })?;
        if !ok {
            return Err(InternalError("请求派发状态无效，未发送上游请求。".into()).into());
        }
        if let Some(r) = b.reservations.get_mut(&self.id) {
            r.day = day.clone();
            r.month = month.clone();
        }
        Ok((day, month))
    }

    pub async fn set_remaining_budget(&self, anlas: f64, v5: i64, legacy: i64) {
        let mut b = self.state.budget.lock().await;
        if let Some(r) = b.reservations.get_mut(&self.id) {
            r.anlas = anlas.max(0.0);
            r.v5 = v5.max(0);
            r.legacy = legacy.max(0);
        }
    }
}

fn remove_reservation(b: &mut crate::state::Budget, id: u64, state: &AppState) {
    b.reservations.remove(&id);
    if !b.reservations.values().any(|r| r.active) {
        state.budget_idle.notify_waiters();
    }
}

impl Drop for Reserved {
    fn drop(&mut self) {
        let (state, id, request_id) = (self.state.clone(), self.id, self.request_id.clone());
        let db_state = state.clone();
        state.tasks.spawn(async move {
            let status = match db_state
                .db
                .close_request_reservation(&request_id, db_state.now())
                .await
            {
                Ok(s) => s,
                Err(e) => {
                    tracing::error!(error = %e, "request settlement finalization failed");
                    return;
                }
            };
            let mut b = db_state.budget.lock().await;
            if matches!(
                status.as_str(),
                "released" | "committed" | "released_manual"
            ) {
                remove_reservation(&mut b, id, &db_state);
            } else if let Some(reservation) = b.reservations.get_mut(&id) {
                reservation.active = false;
                if !b.reservations.values().any(|r| r.active) {
                    db_state.budget_idle.notify_waiters();
                }
            }
        });
    }
}

/// 核对额度并登记预留（排队等待期间就占住，防止并发请求一起超额）。
pub async fn reserve_image_budget(
    state: &AppState,
    ctx: &ReqCtx,
    key: &Key,
    est: &Est,
    legacy_free_images: i64,
    requirements: DispatchRequirements,
) -> Result<Reserved, ApiError> {
    let t = state.limits().queue_timeout;
    let res = tokio::time::timeout(Duration::from_secs_f64(t.max(0.0)), async {
        let mut b = state.budget.lock().await;
        quota_image_check(state, &b, key, est, legacy_free_images, None).await?;
        let (day, month) = (state.day(), state.month());
        state
            .db
            .reserve_request_settlement(
                &ctx.internal_id(),
                &ctx.rid(),
                key.id(),
                &day,
                &month,
                est.is_vibe,
                est.anlas,
                est.v5,
                legacy_free_images,
                state.now(),
            )
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "request reservation persistence failed");
                InternalError("保存额度预留失败，请稍后重试。".into())
            })?;
        ctx.mark_settlement_reserved();
        b.next_id += 1;
        let id = b.next_id;
        b.reservations.insert(
            id,
            Reservation {
                request_id: ctx.internal_id(),
                active: true,
                key_id: key.id(),
                is_vibe: est.is_vibe,
                day,
                month,
                anlas: est.anlas,
                v5: est.v5,
                legacy: legacy_free_images,
            },
        );
        Ok::<u64, ApiError>(id)
    })
    .await;
    match res {
        Ok(Ok(id)) => Ok(Reserved {
            state: state.clone(),
            id,
            key: key.clone(),
            request_id: ctx.internal_id(),
            requirements,
        }),
        Ok(Err(e)) => Err(e),
        Err(_) => Err(GateError::with_code(
            429,
            "站点繁忙，额度核对超时，未扣费。请稍后重试。",
            reasons::QUEUE_TIMEOUT,
        )
        .into()),
    }
}
