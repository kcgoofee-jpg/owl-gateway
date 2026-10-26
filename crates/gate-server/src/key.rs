//! 虚拟 Key 行。数据库读出来是一个 JSON 对象（列名 → 值）；
//! 这里给常用字段一组带 Python 语义的访问器（`bool(row[x])`、缺列当假 / 0）。

use gate_core::pyval;
use serde_json::Value;

#[derive(Debug, Clone)]
pub struct Key(pub Value);

impl Key {
    pub fn try_quota_limits(&self) -> Result<gate_core::quota_limits::MemberLimits, String> {
        let stored = self.get("quota_limits");
        if stored.is_null() {
            return Ok(gate_core::quota_limits::MemberLimits::legacy(
                self.int("daily_images"),
                self.int("daily_v5"),
                self.float("daily_anlas"),
                self.float("monthly_anlas"),
                self.int("quota_auto"),
            ));
        }
        let parsed = match stored {
            Value::String(raw) => serde_json::from_str::<Value>(raw)
                .map_err(|_| "invalid quota_limits JSON".to_string())?,
            Value::Object(_) => stored.clone(),
            _ => return Err("quota_limits must be an object or JSON object string".into()),
        };
        gate_core::quota_limits::MemberLimits::from_value(&parsed)
            .ok_or_else(|| "quota_limits contains invalid modes or values".to_string())
    }

    pub fn quota_limits(&self) -> gate_core::quota_limits::MemberLimits {
        self.try_quota_limits()
            .unwrap_or(gate_core::quota_limits::MemberLimits {
                images: gate_core::quota_limits::Limit::Limit(0.0),
                v5: gate_core::quota_limits::Limit::Limit(0.0),
                daily_anlas: gate_core::quota_limits::Limit::Limit(0.0),
                monthly_anlas: gate_core::quota_limits::Limit::Limit(0.0),
            })
    }

    pub fn get(&self, name: &str) -> &Value {
        self.0.get(name).unwrap_or(&Value::Null)
    }

    /// `bool(row[name])`；缺列为假。
    pub fn flag(&self, name: &str) -> bool {
        pyval::truthy(self.get(name))
    }

    pub fn int(&self, name: &str) -> i64 {
        let v = self.get(name);
        pyval::int_like(v)
            .or_else(|| pyval::number_like(v).and_then(gate_core::pyc::int_of))
            .unwrap_or(0)
    }

    pub fn float(&self, name: &str) -> f64 {
        pyval::number_like(self.get(name)).unwrap_or(0.0)
    }

    pub fn opt_float(&self, name: &str) -> Option<f64> {
        pyval::number_like(self.get(name))
    }

    pub fn str(&self, name: &str) -> &str {
        self.get(name).as_str().unwrap_or("")
    }

    pub fn id(&self) -> i64 {
        self.int("id")
    }
    pub fn name(&self) -> &str {
        self.str("name")
    }
    pub fn enabled(&self) -> bool {
        self.flag("enabled")
    }
    pub fn rpm(&self) -> i64 {
        self.int("rpm")
    }
    pub fn daily_images(&self) -> i64 {
        self.int("daily_images")
    }
    pub fn daily_v5(&self) -> i64 {
        self.int("daily_v5")
    }
    pub fn daily_anlas(&self) -> f64 {
        self.float("daily_anlas")
    }
    pub fn monthly_anlas(&self) -> f64 {
        self.float("monthly_anlas")
    }
    pub fn allow_anlas(&self) -> bool {
        self.flag("allow_anlas")
    }
    pub fn allow_img2img(&self) -> bool {
        self.flag("allow_img2img")
    }
    pub fn exclude_global_v5(&self) -> bool {
        self.flag("exclude_global_v5")
    }
    pub fn image_model_scope(&self) -> &str {
        self.str("image_model_scope")
    }
    /// `expires_at and expires_at < now`
    pub fn expired(&self, now: f64) -> bool {
        self.opt_float("expires_at")
            .is_some_and(|e| e != 0.0 && e < now)
    }
}
