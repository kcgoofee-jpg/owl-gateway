//! Explicit quota modes. Access permissions are intentionally modeled elsewhere.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemberLimits {
    pub images: Limit,
    pub v5: Limit,
    pub daily_anlas: Limit,
    pub monthly_anlas: Limit,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GlobalLimits {
    pub register_daily_images: Limit,
    pub register_daily_v5: Limit,
    pub global_daily_v5: Limit,
    pub global_daily_anlas: Limit,
    pub member_daily_anlas: Limit,
    pub global_monthly_anlas: Limit,
    pub public_vibe_enabled: bool,
}

impl GlobalLimits {
    pub fn from_value(value: &Value) -> Option<Self> {
        let limits: Self = serde_json::from_value(value.clone()).ok()?;
        [
            (limits.register_daily_images, 100_000.0, true),
            (limits.register_daily_v5, 100_000.0, true),
            (limits.global_daily_v5, 100_000.0, true),
            (limits.global_daily_anlas, 100_000.0, true),
            (limits.member_daily_anlas, 100_000.0, true),
            (limits.global_monthly_anlas, 1_000_000.0, false),
        ]
        .iter()
        .all(|(mode, max, integer)| match mode {
            Limit::Unlimited => true,
            Limit::Limit(n) => {
                n.is_finite() && *n >= 0.0 && *n <= *max && (!integer || n.fract() == 0.0)
            }
            Limit::Inherit => false,
        })
        .then_some(limits)
    }

    pub fn from_legacy(values: [f64; 6]) -> Self {
        let mode = |n: f64| {
            if n <= 0.0 {
                Limit::Unlimited
            } else {
                Limit::Limit(n)
            }
        };
        Self {
            register_daily_images: mode(values[0]),
            register_daily_v5: mode(values[1]),
            global_daily_v5: mode(values[2]),
            global_daily_anlas: mode(values[3]),
            member_daily_anlas: mode(values[4]),
            global_monthly_anlas: mode(values[5]),
            // Historical global_daily_anlas=0 disabled free Vibe while paid keys bypassed the pool.
            public_vibe_enabled: values[3] > 0.0,
        }
    }
}

impl MemberLimits {
    pub fn from_value(value: &Value) -> Option<Self> {
        let limits: Self = serde_json::from_value(value.clone()).ok()?;
        limits.validate().then_some(limits)
    }

    pub fn validate(self) -> bool {
        fn valid(mode: Limit, allow_inherit: bool, integer: bool, max: f64) -> bool {
            match mode {
                Limit::Inherit => allow_inherit,
                Limit::Unlimited => true,
                Limit::Limit(n) => {
                    n.is_finite() && n >= 0.0 && n <= max && (!integer || n.fract() == 0.0)
                }
            }
        }
        valid(self.images, true, true, 1_000_000.0)
            && valid(self.v5, true, true, 100_000.0)
            && valid(self.daily_anlas, true, false, 100_000.0)
            && valid(self.monthly_anlas, false, false, 100_000.0)
    }

    pub fn legacy(images: i64, v5: i64, daily_anlas: f64, monthly_anlas: f64, auto: i64) -> Self {
        Self {
            images: if auto == 1 {
                Limit::Inherit
            } else {
                Limit::from_legacy_manual(images as f64)
            },
            v5: if auto == 1 {
                Limit::Inherit
            } else {
                Limit::from_legacy_manual(v5 as f64)
            },
            daily_anlas: if daily_anlas <= 0.0 {
                Limit::Inherit
            } else {
                Limit::Limit(daily_anlas)
            },
            monthly_anlas: Limit::from_legacy_manual(monthly_anlas),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[serde(tag = "mode", content = "value", rename_all = "snake_case")]
pub enum Limit {
    Inherit,
    Unlimited,
    Limit(f64),
}

impl Limit {
    pub fn resolve(self, inherited: Option<f64>) -> Option<f64> {
        match self {
            Self::Inherit => inherited,
            Self::Unlimited => None,
            Self::Limit(n) => Some(n.max(0.0)),
        }
    }

    /// Legacy zero represented unlimited for these member limits.
    pub fn from_legacy_manual(value: f64) -> Self {
        if value <= 0.0 {
            Self::Unlimited
        } else {
            Self::Limit(value)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn explicit_zero_is_distinct_from_legacy_unlimited_zero() {
        assert_eq!(Limit::from_legacy_manual(0.0), Limit::Unlimited);
        assert_eq!(Limit::Limit(0.0).resolve(None), Some(0.0));
        assert_eq!(Limit::Inherit.resolve(Some(9.0)), Some(9.0));
        assert_eq!(Limit::Unlimited.resolve(Some(9.0)), None);
        assert_eq!(
            serde_json::to_value(Limit::Limit(0.0)).unwrap(),
            json!({"mode":"limit","value":0.0})
        );
    }

    #[test]
    fn managed_and_manual_legacy_zero_have_different_modes() {
        let managed = MemberLimits::legacy(0, 0, 0.0, 100.0, 1);
        assert_eq!(managed.images, Limit::Inherit);
        assert_eq!(managed.v5, Limit::Inherit);
        let manual = MemberLimits::legacy(0, 0, 0.0, 0.0, -1);
        assert_eq!(manual.images, Limit::Unlimited);
        assert_eq!(manual.v5, Limit::Unlimited);
        assert_eq!(manual.monthly_anlas, Limit::Unlimited);
    }

    #[test]
    fn canonical_parser_rejects_invalid_or_ambiguous_modes() {
        let valid = json!({
            "images":{"mode":"limit","value":0}, "v5":{"mode":"unlimited"},
            "daily_anlas":{"mode":"inherit"}, "monthly_anlas":{"mode":"unlimited"}
        });
        assert_eq!(
            MemberLimits::from_value(&valid).unwrap().images,
            Limit::Limit(0.0)
        );
        let mut bad = valid.clone();
        bad["images"]["value"] = json!(-1);
        assert!(MemberLimits::from_value(&bad).is_none());
        bad = valid.clone();
        bad["images"]["value"] = json!(1.5);
        assert!(MemberLimits::from_value(&bad).is_none());
        bad = valid.clone();
        bad["monthly_anlas"] = json!({"mode":"inherit"});
        assert!(MemberLimits::from_value(&bad).is_none());
        bad = valid.clone();
        bad["images"]["unexpected"] = json!(1);
        assert!(MemberLimits::from_value(&bad).is_none());
    }

    #[test]
    fn global_anlas_zero_legacy_splits_public_access_and_paid_pool() {
        let zero = GlobalLimits::from_legacy([100.0, 0.0, 20.0, 0.0, 50.0, 1000.0]);
        assert_eq!(zero.global_daily_anlas, Limit::Unlimited);
        assert!(!zero.public_vibe_enabled);
        let positive = GlobalLimits::from_legacy([100.0, 0.0, 20.0, 10.0, 50.0, 1000.0]);
        assert_eq!(positive.global_daily_anlas, Limit::Limit(10.0));
        assert!(positive.public_vibe_enabled);
    }

    #[test]
    fn global_parser_rejects_unknown_fields() {
        let mut value = serde_json::to_value(GlobalLimits::from_legacy([0.0; 6])).unwrap();
        value["unexpected"] = json!(true);
        assert!(GlobalLimits::from_value(&value).is_none());
    }
}
