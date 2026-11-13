//! 每人 / 每站额度检查与估算：这一步只读，不改账。

use super::queue::Est;
use crate::error::{ApiError, GateError};
use crate::key::Key;
use crate::state::{AppState, Reservation};
use gate_core::reasons;

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
    let skip_site = key.skip_site_budget();
    if est.v5 > 0 && !skip_site && crate::ops::v5_paused(&state.db).await? {
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
                if policy.daily_images == 0 {
                    "当前未分配 V4.5 额度。".into()
                } else {
                    format!("今日 V4.5 额度已用完，{reset}重置。")
                },
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
                if policy.daily_v5 == 0 {
                    "当前未分配 V5 额度，请使用 V4.5。".into()
                } else {
                    format!("今日 V5 额度已用完，{reset}重置。请使用 V4.5。")
                },
                reasons::MEMBER_QUOTA,
            )
            .into());
        }
        let used = state.db.day_v5_total(&day).await?;
        let reserved: i64 = pending.iter().filter(|r| r.day == day).map(|r| r.v5).sum();
        if !skip_site && used + reserved + est.v5 > i64::from(shared.daily_v5) {
            return Err(GateError::with_code(
                403,
                if shared.daily_v5 == 0 {
                    "全站 V5 暂停，请使用 V4.5。".into()
                } else {
                    format!("全站今日 V5 额度已用完，{reset}重置。请使用 V4.5。")
                },
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
        if !skip_site
            && state.db.day_anlas_total(&day).await? + reserved + est.anlas
                > f64::from(shared.daily_anlas)
        {
            return Err(GateError::with_code(
                403,
                if shared.daily_anlas == 0 {
                    "全站暂未开放 Anlas 消费。".into()
                } else {
                    format!("全站今日 Anlas 额度不足，{reset}重置。")
                },
                reasons::SITE_QUOTA,
            )
            .into());
        }
        let reserved: f64 = own_day.iter().map(|r| r.anlas).sum();
        if counter.anlas + reserved + est.anlas > f64::from(policy.daily_anlas) {
            return Err(GateError::with_code(
                403,
                if policy.daily_anlas == 0 {
                    "当前未开通 Anlas 消费。".into()
                } else {
                    format!("今日 Anlas 额度不足，{reset}重置。")
                },
                reasons::MEMBER_QUOTA,
            )
            .into());
        }
        let reserved: f64 = own_month.iter().map(|r| r.anlas).sum();
        let used = state.db.month_anlas(key.id(), &month).await?;
        if used + reserved + est.anlas > policy.monthly_anlas {
            return Err(GateError::with_code(
                403,
                if policy.monthly_anlas == 0.0 {
                    "当前未分配每月 Anlas 额度。"
                } else {
                    "本月 Anlas 额度不足，下月恢复。"
                },
                reasons::MEMBER_QUOTA,
            )
            .into());
        }
        let reserved: f64 = pending
            .iter()
            .filter(|r| r.month == month)
            .map(|r| r.anlas)
            .sum();
        if !skip_site
            && state.db.month_anlas_all(&month).await? + reserved + est.anlas > shared.monthly_anlas
        {
            return Err(GateError::with_code(
                403,
                "全站本月 Anlas 预算不足，免费出图不受影响。",
                reasons::SITE_QUOTA,
            )
            .into());
        }
    }
    Ok(())
}
