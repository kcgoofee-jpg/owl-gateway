//! 预算预留与释放：`Reserved` 是唯一改 `state.budget` 的地方，其余模块只读快照。

use super::queue::Est;
use super::quota::quota_image_check;
use crate::error::{ApiError, GateError, InternalError};
use crate::key::Key;
use crate::reqctx::ReqCtx;
use crate::state::{AppState, Reservation};
use gate_core::reasons;
use std::time::Duration;

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
/// 预算快照（只读）：额度检查用。复制一份，调用方不持锁做别的事。
pub async fn budget_snapshot(state: &AppState) -> crate::state::Budget {
    state.budget.lock().await.clone()
}

/// 释放某个请求留下的全部预留（后台「人工释放」用）；返回是否已经没有进行中的预留。
/// 是否唤醒排队由调用方决定，这里只做状态变更。
pub async fn release_reservations(state: &AppState, request_id: &str) -> bool {
    let mut budget = state.budget.lock().await;
    let ids: Vec<u64> = budget
        .reservations
        .iter()
        .filter_map(|(id, r)| (r.request_id == request_id).then_some(*id))
        .collect();
    for id in ids {
        budget.reservations.remove(&id);
    }
    let idle = !budget.reservations.values().any(|r| r.active);
    drop(budget);
    if idle {
        state.budget_idle.notify_waiters();
    }
    idle
}

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
