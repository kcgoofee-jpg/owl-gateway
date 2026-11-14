//! 跨模块共用的站点设置登记表（对应 `app/site_flags.py`）：键名、类型、默认值、范围、归属只在这里定义一次。
//!
//! 不在这里的（服务层负责）：
//! - `get(db, spec)`：`db.get_setting(spec.key, None)` 读 `site_settings` 表的 `spec.key` 一行（TEXT 或没有），
//!   再交给 [`get_from_raw`]（= `parse(spec, raw, default_of(spec))`）；
//! - `put(db, spec, value)`：`db.set_setting(spec.key, encode(spec, value))`，即 `site_settings` 写 `spec.key`，
//!   值是 [`encode`] 的返回串。
//!
//! 格式怪癖：float 类型的设置被夹到边界时得到的是 **int**（`lo` / `hi` 在登记表里都是整数字面量，
//! 夹取返回「胜出的那个值」），`encode` 因此写出 `"0"` 而不是 `"0.0"`；没夹到边界才是 float。

use crate::{primitives, validation, values};
use serde_json::Value;

/// 设置项的类型（`Spec.kind`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// "1" / "0"
    Flag,
    Int,
    Float,
    Str,
    Choice,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Flag => "flag",
            Kind::Int => "int",
            Kind::Float => "float",
            Kind::Str => "str",
            Kind::Choice => "choice",
        }
    }
}

/// `Spec.default`。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SpecDefault {
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(&'static str),
}

impl SpecDefault {
    fn to_value(self) -> Value {
        match self {
            SpecDefault::Bool(b) => Value::Bool(b),
            SpecDefault::Int(i) => Value::from(i),
            SpecDefault::Float(f) => values::float(f),
            SpecDefault::Str(s) => Value::String(s.into()),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Spec {
    pub key: &'static str,
    pub kind: Kind,
    pub default: SpecDefault,
    pub owner: &'static str,
    pub doc: &'static str,
    /// 登记表里的上下限都是整数字面量（见模块说明里的类型怪癖）
    pub lo: Option<i64>,
    pub hi: Option<i64>,
    pub choices: &'static [&'static str],
}

const fn spec(
    key: &'static str,
    kind: Kind,
    default: SpecDefault,
    owner: &'static str,
    doc: &'static str,
) -> Spec {
    Spec {
        key,
        kind,
        default,
        owner,
        doc,
        lo: None,
        hi: None,
        choices: &[],
    }
}

const fn bounded(s: Spec, lo: i64, hi: i64) -> Spec {
    Spec {
        lo: Some(lo),
        hi: Some(hi),
        ..s
    }
}

pub static ISSUE_HOURLY_CAP: Spec = bounded(
    spec(
        "issue_hourly_cap",
        Kind::Int,
        SpecDefault::Int(12),
        "registration",
        "每小时最多发放的新 Key",
    ),
    0,
    9999,
);
/// V5 暂停（手动开关，站长 10-23）：开着时 V5 请求在本地直接拒（402，不扣 Anlas、不发上游），
/// 首页 / 成员页 / 客户端订阅信息都按暂停显示。
pub static V5_PAUSED: Spec = spec(
    "v5_paused",
    Kind::Flag,
    SpecDefault::Bool(false),
    "额度 / V5",
    "V5 暂停：开着时所有 V5 请求直接拒（不扣费）",
);
/// 人工维护模式：开启后公共网站与服务接口统一返回维护页 / 503；后台与健康检查仍可访问。
pub static MAINTENANCE_MODE: Spec = spec(
    "maintenance_mode",
    Kind::Flag,
    SpecDefault::Bool(false),
    "system",
    "维护模式：暂停公共网站与服务接口，后台和健康检查保留",
);
pub static MEMBER_SWEEP: Spec = spec(
    "member_sweep",
    Kind::Flag,
    SpecDefault::Bool(true),
    "registration",
    "退群回收：每人每天核对一次是否还在服务器，不在就删 Key",
);

/// V5 Medium 降档（手动）：`off` 不动（默认）/ `on` 一律改走 Medium（见 `v5_medium`）。
/// 认不出的值（含 10-23 删除的 `auto`）一律当 `off`（fail-closed：不会因为写坏了就悄悄改掉成员的模型）。
pub static V5_MEDIUM_MODE: Spec = Spec {
    choices: &["off", "on"],
    ..spec(
        "v5_medium_mode",
        Kind::Choice,
        SpecDefault::Str("off"),
        "出图 / 后台额度页",
        "V5 Medium 降档：off 关闭 / on 全部 V5 Full 请求改走 Medium",
    )
};

/// `default_of(spec)`：登记表里写死的默认值（不取环境变量）。
pub fn default_of(spec: &Spec) -> Value {
    spec.default.to_value()
}

/// `parse(spec, raw, fallback)`：库里的原始值（字符串 / `Settings` 属性 / 请求体里的任意值）→ 带类型的值。
pub fn parse(spec: &Spec, raw: &Value, fallback: Value) -> Value {
    if raw.is_null() {
        return fallback;
    }
    if let Value::String(s) = raw
        && primitives::strip(s).is_empty()
        && spec.kind != Kind::Str
    {
        return fallback;
    }
    let s = primitives::strip(&validation::text(raw)).to_string();
    let low = s.to_lowercase();
    match spec.kind {
        // 认不出的写法回到默认值（fail-closed），不能当成「关」
        Kind::Flag => match low.as_str() {
            "1" | "true" | "yes" | "on" => Value::Bool(true),
            "0" | "false" | "no" | "off" => Value::Bool(false),
            _ => fallback,
        },
        Kind::Choice => {
            if spec.choices.contains(&s.as_str()) {
                Value::String(s)
            } else {
                fallback
            }
        }
        Kind::Str => Value::String(validation::text(raw)),
        Kind::Int | Kind::Float => {
            let Some(f) = primitives::parse_float(&s) else {
                return fallback;
            };
            if f.is_nan() {
                return fallback;
            }
            // max(lo, num) / min(hi, num)：num 不严格胜出时返回的是边界那个 int（见模块说明）
            let mut num = Num::Float(f);
            if let Some(lo) = spec.lo
                && num.as_f64() <= lo as f64
            {
                num = Num::Int(lo);
            }
            if let Some(hi) = spec.hi
                && num.as_f64() >= hi as f64
            {
                num = Num::Int(hi);
            }
            match (spec.kind, num) {
                (Kind::Int, Num::Float(x)) => Value::from(x.trunc() as i64),
                (_, Num::Float(x)) => values::float(x),
                (_, Num::Int(i)) => Value::from(i),
            }
        }
    }
}

#[derive(Clone, Copy)]
enum Num {
    Int(i64),
    Float(f64),
}

impl Num {
    fn as_f64(self) -> f64 {
        match self {
            Num::Int(i) => i as f64,
            Num::Float(f) => f,
        }
    }
}

/// `get(db, spec)` 的纯部分：`raw` 是 `db.get_setting(spec.key, None)` 的结果（没有这一行 = None）。
pub fn get_from_raw(spec: &Spec, raw: Option<&str>) -> Value {
    let fallback = default_of(spec);
    match raw {
        None => fallback,
        Some(s) => parse(spec, &Value::String(s.to_string()), fallback),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct EncodeError(pub String);

/// `encode(spec, value)`：要写进 `site_settings` 的字符串。
pub fn encode(spec: &Spec, value: &Value) -> Result<String, EncodeError> {
    match spec.kind {
        Kind::Flag => Ok(if values::enabled(value) { "1" } else { "0" }.into()),
        Kind::Choice => {
            let ok = matches!(value, Value::String(s) if spec.choices.contains(&s.as_str()));
            if !ok {
                return Err(EncodeError(format!(
                    "{} 只能是 {}",
                    spec.key,
                    spec.choices.join(" / ")
                )));
            }
            Ok(validation::text(value))
        }
        Kind::Int | Kind::Float => {
            let v = parse(spec, value, Value::Null);
            if v.is_null() {
                return Err(EncodeError(format!("{} 必须是数字", spec.key)));
            }
            Ok(validation::text(&v))
        }
        Kind::Str => Ok(validation::text(value)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample_float() -> Spec {
        bounded(
            spec(
                "sample_float",
                Kind::Float,
                SpecDefault::Float(10000.0),
                "test",
                "float clamp sample",
            ),
            0,
            99999,
        )
    }

    #[test]
    fn clamps_with_int_edges() {
        let spec = sample_float();
        // float 设置被夹到边界时是 int，不是 float
        let v = parse(&spec, &json!("-5"), Value::Null);
        assert!(v.is_i64() && v == json!(0));
        let v = parse(&spec, &json!("12.5"), Value::Null);
        assert!(v.is_f64());
        assert_eq!(encode(&spec, &json!("5000000")).unwrap(), "99999");
    }

    #[test]
    fn v5_medium_mode_fails_closed() {
        assert_eq!(get_from_raw(&V5_MEDIUM_MODE, None), json!("off"));
        for (raw, want) in [("on", "on"), (" on ", "on")] {
            assert_eq!(get_from_raw(&V5_MEDIUM_MODE, Some(raw)), json!(want));
        }
        // 大小写不对、空串、乱写、已删除的 auto 都回到 off
        for raw in ["ON", "auto", "Auto", "", "1", "true", "medium"] {
            assert_eq!(get_from_raw(&V5_MEDIUM_MODE, Some(raw)), json!("off"));
        }
        assert!(encode(&V5_MEDIUM_MODE, &json!("auto")).is_err());
        assert!(encode(&V5_MEDIUM_MODE, &json!("ON")).is_err());
        assert!(encode(&V5_MEDIUM_MODE, &json!(true)).is_err());
    }

    #[test]
    fn flags_fail_closed() {
        assert_eq!(
            parse(&V5_PAUSED, &json!("maybe"), Value::Bool(false)),
            Value::Bool(false)
        );
        assert_eq!(
            parse(&MEMBER_SWEEP, &json!(" OFF "), Value::Bool(true)),
            Value::Bool(false)
        );
        assert_eq!(
            get_from_raw(&ISSUE_HOURLY_CAP, Some("100000")),
            json!(9999),
            "超上限按上限夹值"
        );
    }
}
