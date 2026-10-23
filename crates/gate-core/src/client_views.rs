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

use crate::pyval;

/// 调用方从库里查好的数字。
#[derive(Debug, Clone, Copy)]
pub struct SubscriptionData {
    pub month_anlas_used: f64,
    pub daily_v5_used: i64,
    pub global_v5_limit: i64,
    pub global_v5_total: i64,
    pub v5_paused: bool,
}

/// Key 行缺字段或类型不对（线上 Key 行总是完整的，这只在数据损坏时出现）。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SubscriptionError {
    #[error("Key 缺少字段 {0}")]
    MissingKey(&'static str),
    #[error("Key 字段 {0} 的类型不是数字")]
    BadNumber(&'static str),
}

fn field<'a>(key: &'a Value, name: &'static str) -> Result<&'a Value, SubscriptionError> {
    key.get(name).ok_or(SubscriptionError::MissingKey(name))
}

/// `float(x or 0)`：数字（含 bool）或 None / 0。
fn float_or_zero(v: &Value, name: &'static str) -> Result<f64, SubscriptionError> {
    if !pyval::truthy(v) {
        return Ok(0.0);
    }
    pyval::number_like(v).ok_or(SubscriptionError::BadNumber(name))
}

/// `int(x or 0)`：浮点向零截断。
fn int_or_zero(v: &Value, name: &'static str) -> Result<i64, SubscriptionError> {
    if !pyval::truthy(v) {
        return Ok(0);
    }
    if let Some(i) = pyval::int_like(v) {
        return Ok(i);
    }
    match pyval::number_like(v) {
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
    let paid = pyval::truthy(field(key, "allow_anlas")?);
    let monthly_limit = float_or_zero(field(key, "monthly_anlas")?, "monthly_anlas")?;
    let anlas_left = if paid && monthly_limit > 0.0 {
        ((monthly_limit - data.month_anlas_used).trunc() as i64).max(0)
    } else {
        0
    };

    let mut limits: Vec<i64> = Vec::new();
    let mut remaining: Vec<i64> = Vec::new();
    let daily_limit = int_or_zero(field(key, "daily_v5")?, "daily_v5")?;
    if daily_limit > 0 {
        limits.push(daily_limit);
        remaining.push((daily_limit - data.daily_v5_used).max(0));
    }
    if !key.get("exclude_global_v5").is_some_and(pyval::truthy) {
        let global_limit = data.global_v5_limit;
        if global_limit > 0 {
            limits.push(global_limit);
            remaining.push((global_limit - data.global_v5_total).max(0));
        }
    }
    // V5 暂停：网关会在本地拒掉 V5，客户端不能看到「不限」
    let paused = data.v5_paused;
    let v5_limit = if paused {
        0
    } else {
        limits.iter().copied().min().unwrap_or(0)
    };
    let v5_left = if paused {
        0
    } else {
        remaining.iter().copied().min().unwrap_or(0)
    };

    // int(key["expires_at"] or now + 3650 * 86400)
    let expires = field(key, "expires_at")?;
    let expires_at = if pyval::truthy(expires) {
        match pyval::int_like(expires) {
            Some(i) => i,
            None => match pyval::number_like(expires) {
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
            "anlasEnabled": paid,
            "anlasLeft": anlas_left,
            "anlasMonthlyLimit": monthly_limit.trunc() as i64,
            "v5LeftToday": v5_left,
            "v5DailyLimit": v5_limit,
            "v5Unlimited": limits.is_empty() && !paused,
            "v5Paused": paused,
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
        };
        let v = subscription_payload(&key(), 1.0e9, &data).unwrap();
        assert_eq!(v["naiGate"]["anlasLeft"], 80);
        assert_eq!(v["naiGate"]["v5LeftToday"], 3);
        assert_eq!(v["naiGate"]["v5DailyLimit"], 10);
        assert_eq!(v["naiGate"]["account"], Value::Null);
    }

    #[test]
    fn paused_v5_is_zero_not_unlimited() {
        // 全站和个人都不限时，平常是「不限」；暂停时必须是 0 / 0
        let mut k = key();
        k["daily_v5"] = json!(0);
        let data = SubscriptionData {
            month_anlas_used: 0.0,
            daily_v5_used: 0,
            global_v5_limit: 0,
            global_v5_total: 0,
            v5_paused: false,
        };
        let open = subscription_payload(&k, 0.0, &data).unwrap();
        assert_eq!(open["naiGate"]["v5Unlimited"], true);
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
}
