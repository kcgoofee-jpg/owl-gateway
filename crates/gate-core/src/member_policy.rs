//! 成员的统一规则：默认值加字段例外。身份组和 Key 凭据不参与规则计算。

use serde::{Deserialize, Serialize};

pub fn is_discord_id(value: &str) -> bool {
    value
        .parse::<u64>()
        .is_ok_and(|number| number > 0 && number.to_string() == value)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EligibilityRule {
    pub guild: String,
    pub role: String,
    #[serde(default)]
    pub note: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClaimRules {
    pub open: bool,
    pub max_users: u32,
    pub expires_days: u32,
    pub roles: Vec<EligibilityRule>,
}

impl ClaimRules {
    pub fn initial() -> Self {
        Self {
            open: false,
            max_users: 10,
            expires_days: 0,
            roles: vec![],
        }
    }

    pub fn validate(&self) -> Result<(), &'static str> {
        if self.max_users > 100000 || self.expires_days > 3650 || self.roles.len() > 6 {
            return Err("领取条件超出有效范围");
        }
        let mut seen = std::collections::HashSet::new();
        for role in &self.roles {
            if !is_discord_id(&role.guild)
                || !is_discord_id(&role.role)
                || role.note.chars().count() > 80
            {
                return Err("领取身份组格式无效");
            }
            if !seen.insert((&role.guild, &role.role)) {
                return Err("领取身份组不能重复");
            }
        }
        Ok(())
    }
}

/// 所有成员共同遵守的额度，不提供个人豁免。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SharedBudgets {
    pub daily_v5: u32,
    pub daily_anlas: u32,
    pub monthly_anlas: f64,
}

impl SharedBudgets {
    pub fn initial() -> Self {
        Self {
            daily_v5: 150,
            daily_anlas: 200,
            monthly_anlas: 10000.0,
        }
    }

    pub fn validate(&self) -> Result<(), &'static str> {
        if self.daily_v5 > 9999 || self.daily_anlas > 9999 {
            return Err("全站每日额度必须是 0～9999 的整数");
        }
        if !self.monthly_anlas.is_finite() || !(0.0..=99999.0).contains(&self.monthly_anlas) {
            return Err("全站每月 Anlas 必须是 0～99999 的数字");
        }
        Ok(())
    }
}

/// 完整的配置值；暂停、用量、排队和全站预算另行限制实际请求。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemberPolicy {
    pub daily_images: u32,
    pub daily_v5: u32,
    pub daily_anlas: u32,
    pub monthly_anlas: f64,
    pub rpm: u32,
    pub image: bool,
    pub v5: bool,
    pub vibe: bool,
    pub tags: bool,
    pub upscale: bool,
    pub augment: bool,
    pub img2img: bool,
    pub paid_parameters: bool,
}

/// PUT 完整替换例外；省略或 null 都表示使用默认值。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MemberOverrides {
    pub daily_images: Option<u32>,
    pub daily_v5: Option<u32>,
    pub daily_anlas: Option<u32>,
    pub monthly_anlas: Option<f64>,
    pub rpm: Option<u32>,
    pub image: Option<bool>,
    pub v5: Option<bool>,
    pub vibe: Option<bool>,
    pub tags: Option<bool>,
    pub upscale: Option<bool>,
    pub augment: Option<bool>,
    pub img2img: Option<bool>,
    pub paid_parameters: Option<bool>,
}

impl MemberPolicy {
    pub fn initial() -> Self {
        Self {
            daily_images: 150,
            daily_v5: 8,
            daily_anlas: 50,
            monthly_anlas: 99999.0,
            rpm: 10,
            image: true,
            v5: false,
            vibe: false,
            tags: true,
            upscale: false,
            augment: false,
            img2img: false,
            paid_parameters: false,
        }
    }

    pub fn allows(&self, feature: &str) -> bool {
        match feature {
            "image" => self.image,
            "v5" => self.image && self.v5,
            "vibe" => self.vibe,
            "tags" => self.tags,
            "upscale" => self.upscale,
            "augment" => self.augment,
            _ => false,
        }
    }
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.daily_images > 9999 || self.daily_v5 > 9999 || self.daily_anlas > 9999 {
            return Err("每日额度必须是 0～9999 的整数");
        }
        if !self.monthly_anlas.is_finite() || !(0.0..=99999.0).contains(&self.monthly_anlas) {
            return Err("每月 Anlas 必须是 0～99999 的数字");
        }
        if !(1..=600).contains(&self.rpm) {
            return Err("每分钟请求数必须是 1～600 的整数");
        }
        Ok(())
    }

    pub fn resolve(&self, overrides: &MemberOverrides) -> Result<Self, &'static str> {
        let resolved = Self {
            daily_images: overrides.daily_images.unwrap_or(self.daily_images),
            daily_v5: overrides.daily_v5.unwrap_or(self.daily_v5),
            daily_anlas: overrides.daily_anlas.unwrap_or(self.daily_anlas),
            monthly_anlas: overrides.monthly_anlas.unwrap_or(self.monthly_anlas),
            rpm: overrides.rpm.unwrap_or(self.rpm),
            image: overrides.image.unwrap_or(self.image),
            v5: overrides.v5.unwrap_or(self.v5),
            vibe: overrides.vibe.unwrap_or(self.vibe),
            tags: overrides.tags.unwrap_or(self.tags),
            upscale: overrides.upscale.unwrap_or(self.upscale),
            augment: overrides.augment.unwrap_or(self.augment),
            img2img: overrides.img2img.unwrap_or(self.img2img),
            paid_parameters: overrides.paid_parameters.unwrap_or(self.paid_parameters),
        };
        resolved.validate()?;
        Ok(resolved)
    }

    /// 只保存不同于默认值的字段；相同值跟随此后的默认值变更。
    pub fn differences(&self, configured: &Self) -> MemberOverrides {
        fn changed<T: Copy + PartialEq>(default: T, value: T) -> Option<T> {
            (default != value).then_some(value)
        }
        MemberOverrides {
            daily_images: changed(self.daily_images, configured.daily_images),
            daily_v5: changed(self.daily_v5, configured.daily_v5),
            daily_anlas: changed(self.daily_anlas, configured.daily_anlas),
            monthly_anlas: changed(self.monthly_anlas, configured.monthly_anlas),
            rpm: changed(self.rpm, configured.rpm),
            image: changed(self.image, configured.image),
            v5: changed(self.v5, configured.v5),
            vibe: changed(self.vibe, configured.vibe),
            tags: changed(self.tags, configured.tags),
            upscale: changed(self.upscale, configured.upscale),
            augment: changed(self.augment, configured.augment),
            img2img: changed(self.img2img, configured.img2img),
            paid_parameters: changed(self.paid_parameters, configured.paid_parameters),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn defaults() -> MemberPolicy {
        MemberPolicy {
            daily_images: 150,
            daily_v5: 8,
            daily_anlas: 15,
            monthly_anlas: 99999.0,
            rpm: 10,
            image: true,
            v5: false,
            vibe: true,
            tags: true,
            upscale: false,
            augment: false,
            img2img: false,
            paid_parameters: false,
        }
    }

    #[test]
    fn zero_and_false_are_exceptions_and_null_restores_defaults() {
        let defaults = defaults();
        let exceptions: MemberOverrides = serde_json::from_value(json!({
            "daily_images": 0, "image": false, "daily_v5": null
        }))
        .unwrap();
        let resolved = defaults.resolve(&exceptions).unwrap();
        assert_eq!(resolved.daily_images, 0);
        assert!(!resolved.image);
        assert_eq!(resolved.daily_v5, 8);
        assert_eq!(
            defaults.resolve(&MemberOverrides::default()).unwrap(),
            defaults
        );
    }

    #[test]
    fn equal_values_follow_defaults_but_other_exceptions_survive() {
        let mut defaults = defaults();
        let mut configured = defaults.clone();
        configured.v5 = true;
        let exceptions = defaults.differences(&configured);
        assert_eq!(exceptions.daily_images, None);
        defaults.daily_images = 200;
        let effective = defaults.resolve(&exceptions).unwrap();
        assert_eq!(effective.daily_images, 200);
        assert!(effective.v5);
    }

    #[test]
    fn invalid_values_and_legacy_modes_are_rejected() {
        for invalid in [
            json!({"daily_images": -1}),
            json!({"daily_images": 1.5}),
            json!({"daily_images": "10"}),
            json!({"quota_auto": 1}),
            json!({"daily_images": {"mode": "unlimited"}}),
        ] {
            assert!(serde_json::from_value::<MemberOverrides>(invalid).is_err());
        }
        for invalid in [
            json!({"daily_images":10000}),
            json!({"rpm":0}),
            json!({"monthly_anlas":100000}),
        ] {
            let exceptions = serde_json::from_value(invalid).unwrap();
            assert!(defaults().resolve(&exceptions).is_err());
        }
    }
}
