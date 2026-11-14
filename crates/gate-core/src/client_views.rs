//! 官方客户端的订阅视图，只使用已经解析的本地成员规则和用量。
use crate::member_policy::MemberPolicy;
use serde_json::{Value, json};

#[derive(Debug, Clone, Copy)]
pub struct SubscriptionData {
    pub month_anlas_used: f64,
    /// 这把 Key 今天用掉的 Anlas（首页「个人剩余」用它算）。
    pub daily_anlas_used: f64,
    /// 全站今天的 Anlas 池上限和已用量：工作台显示的余额要和个人剩余取较小值。
    pub global_anlas_limit: f64,
    pub global_anlas_total: f64,
    pub global_monthly_anlas_limit: f64,
    pub global_monthly_anlas_total: f64,
    pub daily_v5_used: i64,
    pub global_v5_limit: u32,
    pub global_v5_total: i64,
    pub v5_paused: bool,
    /// 今天已用的 V4.5 免费张数。没开挂费时官方客户端只看得懂 `trainingStepsLeft`，用它来显示剩余张数。
    pub daily_legacy_used: i64,
}

pub fn subscription_payload(
    policy: &MemberPolicy,
    expires: Option<f64>,
    now: f64,
    data: &SubscriptionData,
    skip_free_clamp: bool,
) -> Value {
    let allowed = policy.allows("v5");
    let open = allowed && !data.v5_paused;
    let v5_limit = if open {
        policy.daily_v5.min(data.global_v5_limit)
    } else {
        0
    };
    let v5_left = if open {
        (i64::from(policy.daily_v5) - data.daily_v5_used)
            .max(0)
            .min((i64::from(data.global_v5_limit) - data.global_v5_total).max(0))
    } else {
        0
    };
    // 开了高规格开关（跳过免费档裁剪）才把 Anlas 余额给官方客户端。
    let anlas_enabled = policy.paid_parameters || skip_free_clamp;
    let anlas_left = if anlas_enabled {
        let personal = (f64::from(policy.daily_anlas) - data.daily_anlas_used).max(0.0);
        let site = (data.global_anlas_limit - data.global_anlas_total).max(0.0);
        let personal_month = (policy.monthly_anlas - data.month_anlas_used).max(0.0);
        let site_month =
            (data.global_monthly_anlas_limit - data.global_monthly_anlas_total).max(0.0);
        personal
            .min(site)
            .min(personal_month)
            .min(site_month)
            .trunc() as i64
    } else {
        0
    };
    let steps_left = if anlas_enabled {
        anlas_left
    } else {
        (i64::from(policy.daily_images) - data.daily_legacy_used).max(0)
    };
    let expires = expires
        .filter(|e| *e != 0.0)
        .unwrap_or(now + 3650.0 * 86400.0)
        .trunc() as i64;
    json!({
        "tier":3,"active":true,"expiresAt":expires,
        "trainingStepsLeft":{"fixedTrainingStepsLeft":steps_left,"purchasedTrainingSteps":0},
        "perks":{"imageGeneration":policy.image,"unlimitedImageGeneration":false,
            "voiceGeneration":false,"contextTokens":0},
        "naiGate":{
            "v45DailyLimit":policy.daily_images,"dailyAnlasLimit":policy.daily_anlas,
            "anlasEnabled":anlas_enabled,"anlasLeft":anlas_left,
            "anlasMonthlyLimit":policy.monthly_anlas,
            "v5LeftToday":v5_left,"v5DailyLimit":v5_limit,
            "v5Paused":data.v5_paused,"v5Allowed":allowed,"account":null,
            "features":{"image":policy.image,"v5":policy.v5,"vibe":policy.vibe,"tags":policy.tags,
                "upscale":policy.upscale,"augment":policy.augment,"img2img":policy.img2img,"paid_parameters":policy.paid_parameters},
            "limits":{"dailyImages":policy.daily_images,"dailyAnlas":policy.daily_anlas,"monthlyAnlas":policy.monthly_anlas,"rpm":policy.rpm}
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn policy() -> MemberPolicy {
        MemberPolicy {
            daily_images: 150,
            daily_v5: 10,
            daily_anlas: 15,
            monthly_anlas: 100.0,
            rpm: 10,
            image: true,
            v5: true,
            vibe: true,
            tags: true,
            upscale: false,
            augment: false,
            img2img: false,
            paid_parameters: true,
            share_exempt: false,
        }
    }
    fn data() -> SubscriptionData {
        SubscriptionData {
            month_anlas_used: 20.0,
            daily_anlas_used: 8.0,
            global_anlas_limit: 300.0,
            global_anlas_total: 100.0,
            global_monthly_anlas_limit: 1000.0,
            global_monthly_anlas_total: 100.0,
            daily_v5_used: 2,
            global_v5_limit: 150,
            global_v5_total: 147,
            v5_paused: false,
            daily_legacy_used: 0,
        }
    }
    #[test]
    fn member_and_shared_limits_are_both_applied() {
        let value = subscription_payload(&policy(), None, 1000.0, &data(), false);
        assert_eq!(value["naiGate"]["v5LeftToday"], 3);
        assert_eq!(value["naiGate"]["v5DailyLimit"], 10);
        // 个人日上限 15 − 已用 8 = 7，全站池还剩 200，取较小值
        assert_eq!(value["naiGate"]["anlasLeft"], 7);
        assert!(value["naiGate"]["account"].is_null());
        assert_eq!(value["expiresAt"], 1000 + 3650 * 86400);
    }
    #[test]
    fn disabled_paused_and_zero_limits_never_mean_unlimited() {
        for (allowed, paused, global) in [(false, false, 150), (true, true, 150), (true, false, 0)]
        {
            let mut policy = policy();
            policy.v5 = allowed;
            let data = SubscriptionData {
                v5_paused: paused,
                global_v5_limit: global,
                ..data()
            };
            let value = subscription_payload(&policy, Some(1234.0), 1000.0, &data, false);
            assert_eq!(value["naiGate"]["v5DailyLimit"], 0);
            assert_eq!(value["naiGate"]["v5LeftToday"], 0);
            assert_eq!(value["expiresAt"], 1234);
        }
    }
    #[test]
    fn site_pool_exhaustion_zeroes_the_workbench_balance() {
        let data = SubscriptionData {
            daily_anlas_used: 0.0,
            global_anlas_limit: 300.0,
            global_anlas_total: 300.0,
            ..data()
        };
        let value = subscription_payload(&policy(), None, 0.0, &data, false);
        assert_eq!(value["naiGate"]["anlasLeft"], 0);
    }

    #[test]
    fn monthly_member_and_site_budgets_also_limit_the_visible_anlas() {
        let mut policy = policy();
        policy.monthly_anlas = 22.0;
        let value = subscription_payload(&policy, None, 0.0, &data(), false);
        assert_eq!(value["naiGate"]["anlasLeft"], 2);
        policy.monthly_anlas = 100.0;
        let data = SubscriptionData {
            global_monthly_anlas_limit: 101.0,
            global_monthly_anlas_total: 100.0,
            ..data()
        };
        let value = subscription_payload(&policy, None, 0.0, &data, false);
        assert_eq!(value["naiGate"]["anlasLeft"], 1);
    }

    #[test]
    fn exhausted_budgets_clamp_to_zero() {
        let mut policy = policy();
        policy.daily_v5 = 0;
        let data = SubscriptionData {
            month_anlas_used: 120.0,
            daily_anlas_used: 15.0,
            ..data()
        };
        let value = subscription_payload(&policy, None, 0.0, &data, false);
        assert_eq!(value["naiGate"]["v5LeftToday"], 0);
        assert_eq!(value["naiGate"]["anlasLeft"], 0);
    }

    #[test]
    fn free_members_see_remaining_v45_images_as_training_steps() {
        let mut policy = policy();
        policy.paid_parameters = false;
        policy.daily_images = 100;
        let data = SubscriptionData {
            daily_legacy_used: 3,
            ..data()
        };
        let value = subscription_payload(&policy, None, 1000.0, &data, false);
        assert_eq!(value["trainingStepsLeft"]["fixedTrainingStepsLeft"], 97);
        assert_eq!(value["naiGate"]["anlasLeft"], 0);
        assert_eq!(value["naiGate"]["anlasEnabled"], false);
    }

    #[test]
    fn skip_free_clamp_shows_anlas_without_the_paid_switch() {
        let mut policy = policy();
        policy.paid_parameters = false;
        let value = subscription_payload(&policy, None, 1000.0, &data(), true);
        assert_eq!(value["naiGate"]["anlasEnabled"], true);
        assert_eq!(value["naiGate"]["anlasLeft"], 7);
    }
}
