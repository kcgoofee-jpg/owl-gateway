//! 客户端兼容视图（`app/client_views.py`）：只由已认证的虚拟 Key 推出，供官方风格的客户端读订阅信息。
//!
//! 订阅数字来自四个单行库查询，计算本身是纯的，所以「读库」留给调用方、这里只做计算：
//! 调用方按 [`SubscriptionData`] 把数字查好传进来
//! （都是便宜的单行查询；用不上的会被忽略，调用方也可以跳过）。
//!
//! 各入参的来源：
//! - `month_anlas_used`：该 Key 当月的 Anlas 用量（`month = state.month(now)`）；
//! - `daily_v5_used`：该 Key 当天已用的 V5（`day = state.day(now)`）；
//! - `global_v5_limit`：全站每日 V5 上限（`site_flags` 的 `GLOBAL_DAILY_V5`，0 = 不限）；
//! - `global_v5_total`：全站当天的 V5 总用量；
//! - 站长的 V5 暂停开关（`v5_paused`，10-23 起）→ `v5_paused`：暂停时 V5 显示 0 / 0、不是「不限」。

use serde_json::{Value, json};

use crate::values;

/// 调用方从库里查好的数字。
#[derive(Debug, Clone, Copy)]
pub struct SubscriptionData {
    pub month_anlas_used: f64,
    pub daily_v5_used: i64,
    pub global_v5_limit: i64,
    pub global_v5_total: i64,
    pub v5_paused: bool,
    pub v5_access_allowed: bool,
    pub global_quota_modes: crate::quota_limits::GlobalLimits,
}

/// Key 行缺字段或类型不对（线上 Key 行总是完整的，这只在数据损坏时出现）。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SubscriptionError {
    #[error("Key 缺少字段 {0}")]
    MissingKey(&'static str),
    #[error("Key 字段 {0} 的类型不是数字")]
    BadNumber(&'static str),
    #[error("Key 额度模式无效")]
    BadQuotaModes,
}

fn field<'a>(key: &'a Value, name: &'static str) -> Result<&'a Value, SubscriptionError> {
    key.get(name).ok_or(SubscriptionError::MissingKey(name))
}

/// `float(x or 0)`：数字（含 bool）或 None / 0。
fn float_or_zero(v: &Value, name: &'static str) -> Result<f64, SubscriptionError> {
    if !values::enabled(v) {
        return Ok(0.0);
    }
    values::number(v).ok_or(SubscriptionError::BadNumber(name))
}

/// `int(x or 0)`：浮点向零截断。
fn int_or_zero(v: &Value, name: &'static str) -> Result<i64, SubscriptionError> {
    if !values::enabled(v) {
        return Ok(0);
    }
    if let Some(i) = values::int_like(v) {
        return Ok(i);
    }
    match values::number(v) {
        Some(f) if f.is_finite() => Ok(f.trunc() as i64),
        _ => Err(SubscriptionError::BadNumber(name)),
    }
}

/// `subscription_payload(state, key, now=)`
pub fn subscription_payload(
    key: &Value,
    now: f64,
    data: &SubscriptionData,
) -> Result<Value, SubscriptionError> {
    let quota_limits = {
        let stored = key.get("quota_limits");
        let value = match stored {
            None | Some(Value::Null) => None,
            Some(Value::String(raw)) => Some(
                serde_json::from_str::<Value>(raw).map_err(|_| SubscriptionError::BadQuotaModes)?,
            ),
            Some(Value::Object(_)) => Some(stored.cloned().unwrap_or(Value::Null)),
            _ => return Err(SubscriptionError::BadQuotaModes),
        };
        match value {
            Some(value) => crate::quota_limits::MemberLimits::from_value(&value)
                .ok_or(SubscriptionError::BadQuotaModes)?,
            None => {
                // Keep the legacy projection's required-field checks and their precedence for
                // pre-migration Key rows. Optional quota metadata was never required by this view.
                let _ = field(key, "allow_anlas")?;
                let monthly_anlas = float_or_zero(field(key, "monthly_anlas")?, "monthly_anlas")?;
                let daily_v5 = int_or_zero(field(key, "daily_v5")?, "daily_v5")?;
                let _ = field(key, "expires_at")?;
                let _ = field(key, "image_model_scope")?;
                let null = Value::Null;
                let get = |name| key.get(name).unwrap_or(&null);
                crate::quota_limits::MemberLimits::legacy(
                    int_or_zero(get("daily_images"), "daily_images")?,
                    daily_v5,
                    float_or_zero(get("daily_anlas"), "daily_anlas")?,
                    monthly_anlas,
                    int_or_zero(get("quota_auto"), "quota_auto")?,
                )
            }
        }
    };
    let images_limit = match quota_limits.images {
        crate::quota_limits::Limit::Inherit => {
            data.global_quota_modes.register_daily_images.resolve(None)
        }
        mode => mode.resolve(None),
    };
    let daily_anlas_limit = match quota_limits.daily_anlas {
        crate::quota_limits::Limit::Inherit => {
            data.global_quota_modes.member_daily_anlas.resolve(None)
        }
        mode => mode.resolve(None),
    };
    let paid = values::enabled(field(key, "allow_anlas")?);
    let monthly_limit = quota_limits.monthly_anlas.resolve(None);
    let anlas_left = if paid && monthly_limit.is_some() {
        ((monthly_limit.unwrap_or_default() - data.month_anlas_used).trunc() as i64).max(0)
    } else {
        0
    };

    let mut limits: Vec<i64> = Vec::new();
    let mut remaining: Vec<i64> = Vec::new();
    let daily_limit = match quota_limits.v5 {
        crate::quota_limits::Limit::Inherit => {
            data.global_quota_modes.register_daily_v5.resolve(None)
        }
        mode => mode.resolve(None),
    };
    if let Some(daily_limit) = daily_limit {
        limits.push(daily_limit.trunc() as i64);
        remaining.push(((daily_limit - data.daily_v5_used as f64).trunc() as i64).max(0));
    }
    if !key.get("exclude_global_v5").is_some_and(values::enabled) {
        let global_limit = data.global_quota_modes.global_daily_v5.resolve(None);
        if let Some(global_limit) = global_limit {
            limits.push(global_limit.trunc() as i64);
            remaining.push(((global_limit - data.global_v5_total as f64).trunc() as i64).max(0));
        }
    }
    // V5 暂停：网关会在本地拒掉 V5，客户端不能看到「不限」
    let paused = data.v5_paused;
    let access_closed = !data.v5_access_allowed;
    let v5_limit = if paused || access_closed {
        0
    } else {
        limits.iter().copied().min().unwrap_or(0)
    };
    let v5_left = if paused || access_closed {
        0
    } else {
        remaining.iter().copied().min().unwrap_or(0)
    };

    // int(key["expires_at"] or now + 3650 * 86400)
    let expires = field(key, "expires_at")?;
    let expires_at = if values::enabled(expires) {
        match values::int_like(expires) {
            Some(i) => i,
            None => match values::number(expires) {
                Some(f) if f.is_finite() => f.trunc() as i64,
                _ => return Err(SubscriptionError::BadNumber("expires_at")),
            },
        }
    } else {
        (now + (3650 * 86400) as f64).trunc() as i64
    };

    let scope = field(key, "image_model_scope")?.clone();

    Ok(json!({
        "tier": 3,
        "active": true,
        "expiresAt": expires_at,
        "trainingStepsLeft": {
            "fixedTrainingStepsLeft": anlas_left,
            "purchasedTrainingSteps": 0,
        },
        "perks": {
            "imageGeneration": true, "unlimitedImageGeneration": false,
            "voiceGeneration": false, "contextTokens": 0,
        },
        // 本地每日额度走 naiGate；官方电池用量不可用。
        "naiGate": {
            "imageModelScope": scope,
            "quotaLimits": quota_limits,
            "v45DailyLimit": images_limit.unwrap_or(0.0).trunc() as i64,
            "v45Unlimited": images_limit.is_none(),
            "dailyAnlasLimit": daily_anlas_limit.unwrap_or(0.0),
            "dailyAnlasUnlimited": daily_anlas_limit.is_none(),
            "anlasEnabled": paid,
            "anlasLeft": anlas_left,
            "anlasMonthlyLimit": monthly_limit.unwrap_or(0.0).trunc() as i64,
            "anlasUnlimited": monthly_limit.is_none(),
            "quotaLimits": quota_limits,
            "v5LeftToday": v5_left,
            "v5DailyLimit": v5_limit,
            "v5Unlimited": limits.is_empty() && !paused && !access_closed,
            "v5Paused": paused,
            "v5Allowed": data.v5_access_allowed,
            // `account` 恒为 null：客户端面板接受 null 并隐藏共享账号信息。
            "account": Value::Null,
        },
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> Value {
        json!({"id": 1, "expires_at": 0, "monthly_anlas": 100, "daily_v5": 10, "allow_anlas": true,
               "image_model_scope": "all", "exclude_global_v5": false})
    }

    #[test]
    fn limits_by_key_and_shared_v5() {
        let data = SubscriptionData {
            month_anlas_used: 20.0,
            daily_v5_used: 2,
            global_v5_limit: 150,
            global_v5_total: 147,
            v5_paused: false,
            v5_access_allowed: true,
            global_quota_modes: crate::quota_limits::GlobalLimits::from_legacy([
                150.0, 0.0, 150.0, 200.0, 50.0, 10000.0,
            ]),
        };
        let v = subscription_payload(&key(), 1.0e9, &data).unwrap();
        assert_eq!(v["naiGate"]["anlasLeft"], 80);
        assert_eq!(v["naiGate"]["v5LeftToday"], 3);
        assert_eq!(v["naiGate"]["v5DailyLimit"], 10);
        assert_eq!(v["naiGate"]["account"], Value::Null);
    }

    #[test]
    fn paused_v5_is_zero_not_unlimited() {
        // 10-27 规范化：上限是数字（9999 = 放开），暂停时必须是 0 / 0
        let mut k = key();
        k["daily_v5"] = json!(9999);
        let data = SubscriptionData {
            month_anlas_used: 0.0,
            daily_v5_used: 0,
            global_v5_limit: 0,
            global_v5_total: 0,
            v5_paused: false,
            v5_access_allowed: true,
            global_quota_modes: crate::quota_limits::GlobalLimits::from_legacy([
                150.0, 9999.0, 9999.0, 200.0, 50.0, 10000.0,
            ]),
        };
        let open = subscription_payload(&k, 0.0, &data).unwrap();
        assert_eq!(open["naiGate"]["v5DailyLimit"], 9999);
        assert_eq!(open["naiGate"]["v5Unlimited"], false);
        let paused = SubscriptionData {
            v5_paused: true,
            ..data
        };
        let v = subscription_payload(&k, 0.0, &paused).unwrap();
        assert_eq!(v["naiGate"]["v5Unlimited"], false);
        assert_eq!(v["naiGate"]["v5LeftToday"], 0);
        assert_eq!(v["naiGate"]["v5DailyLimit"], 0);
        assert_eq!(v["naiGate"]["v5Paused"], true);
        // 有限额时也一样
        let v = subscription_payload(
            &key(),
            0.0,
            &SubscriptionData {
                global_v5_limit: 150,
                ..paused
            },
        )
        .unwrap();
        assert_eq!(
            (
                v["naiGate"]["v5LeftToday"].clone(),
                v["naiGate"]["v5DailyLimit"].clone()
            ),
            (json!(0), json!(0))
        );
    }

    #[test]
    fn denied_v5_is_zero_not_unlimited() {
        let mut k = key();
        k["daily_v5"] = json!(9999);
        let d = SubscriptionData {
            month_anlas_used: 0.0,
            daily_v5_used: 0,
            global_v5_limit: 0,
            global_v5_total: 0,
            v5_paused: false,
            v5_access_allowed: false,
            global_quota_modes: crate::quota_limits::GlobalLimits::from_legacy([
                150.0, 9999.0, 9999.0, 200.0, 50.0, 10000.0,
            ]),
        };
        let result = subscription_payload(&k, 0.0, &d).unwrap();
        assert_eq!(result["naiGate"]["v5Unlimited"], false);
        assert_eq!(result["naiGate"]["v5LeftToday"], 0);
        assert_eq!(result["naiGate"]["v5DailyLimit"], 0);
    }
}
