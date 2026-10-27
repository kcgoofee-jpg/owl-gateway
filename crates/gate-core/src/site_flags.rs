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
    500,
);
pub static REGISTER_OPEN: Spec = spec(
    "register_open",
    Kind::Flag,
    SpecDefault::Bool(false),
    "registration",
    "开放 /register 领取",
);
/// 全站每日 V5（手动，站长 10-23；以前由动态额度每轮写）。没设过按 150。
pub static GLOBAL_DAILY_V5: Spec = bounded(
    spec(
        "global_daily_v5",
        Kind::Int,
        SpecDefault::Int(150),
        "额度 / 首页 / 客户端",
        "全站每日 V5 张数（0 = 停用）",
    ),
    0,
    9999,
);
/// 全站每日 Anlas 池（Vibe PVP）。没设过按 200；2.x / 3.0 早期的默认值取环境变量 `GLOBAL_DAILY_ANLAS`，
/// 现已删除（默认值直接写在登记表里）。
pub static GLOBAL_DAILY_ANLAS: Spec = bounded(
    spec(
        "global_daily_anlas",
        Kind::Int,
        SpecDefault::Int(200),
        "额度 / Vibe PVP",
        "全站每日 Anlas 额度（0 = 停用，Vibe 对公不开放；Vibe PVP 抢额度）",
    ),
    0,
    9999,
);
/// 全站每月 Anlas 预算。没设过按 10000；环境变量 `GLOBAL_MONTHLY_ANLAS` 已删除（默认值直接写在这里）。
pub static GLOBAL_MONTHLY_ANLAS: Spec = bounded(
    spec(
        "global_monthly_anlas",
        Kind::Float,
        SpecDefault::Float(10000.0),
        "额度 / Anlas 池",
        "全站每月 Anlas 预算（0 = 停用）",
    ),
    0,
    99999,
);
pub static MEMBER_DAILY_ANLAS: Spec = bounded(
    spec(
        "member_daily_anlas",
        Kind::Int,
        SpecDefault::Int(50),
        "额度 / Anlas 池",
        "每人每日 Anlas 上限（仅对未手动设置 Anlas 日额度的 Key 生效；0 = 停用）。Vibe 编码一张参考图约 2 点，50 点 ≈ 25 次/天，防单人吃光全站 PVP 池",
    ),
    0,
    9999,
);
/// V4.5 每人每天张数（手动，站长 10-23：新号老号一样，算法不改；新号爬坡 `newbie_ramp_*` 已删除）。
/// 额度同步任务把它下发给 `quota_auto=1` 的成员 Key，新领的 Key 也用它。没设过按 150，0 = 停用。
pub static REGISTER_DAILY_IMAGES: Spec = bounded(
    spec(
        "register_daily_images",
        Kind::Int,
        SpecDefault::Int(150),
        "额度 / V4.5",
        "V4.5 每人每天张数（手动；0 = 停用）",
    ),
    0,
    9999,
);
/// V5 每人每天张数（手动，站长 10-23：按电量自动定的 V5 方案已删除）。同步方式同 [`REGISTER_DAILY_IMAGES`]。
/// 没设过按 9999（等于放开，只受全站每日 V5 管）。
pub static REGISTER_DAILY_V5: Spec = bounded(
    spec(
        "register_daily_v5",
        Kind::Int,
        SpecDefault::Int(9999),
        "额度 / V5",
        "V5 每人每天张数（手动；0 = 停用）",
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
pub static ALGO_NOTICE: Spec = spec(
    "algo_notice",
    Kind::Str,
    SpecDefault::Str(""),
    "quota_algo → 首页",
    "首页显示的今日额度说明",
);
/// 请求日志里来源 IP 和客户端（User-Agent）保留几天，到期由维护任务清成空串；0 = 永久（站长 10-22：后台记录一律留存）。
pub static ORIGIN_KEEP_DAYS: Spec = bounded(
    spec(
        "usage_origin_keep_days",
        Kind::Int,
        SpecDefault::Int(0),
        "audit",
        "请求日志里的来源 IP 和客户端保留几天，到期清除；0 = 永久",
    ),
    0,
    365,
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

/// 登记表（已删除的设置不在里面；新增的追加在末尾）。
pub static ALL: [&Spec; 9] = [
    &MEMBER_SWEEP,
    &ISSUE_HOURLY_CAP,
    &REGISTER_OPEN,
    &GLOBAL_DAILY_V5,
    &GLOBAL_DAILY_ANLAS,
    &GLOBAL_MONTHLY_ANLAS,
    &ALGO_NOTICE,
    &V5_MEDIUM_MODE,
    &ORIGIN_KEEP_DAYS,
];

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

    #[test]
    fn clamps_with_int_edges() {
        // float 设置被夹到边界时是 int，不是 float
        let v = parse(&GLOBAL_MONTHLY_ANLAS, &json!("-5"), Value::Null);
        assert!(v.is_i64() && v == json!(0));
        let v = parse(&GLOBAL_MONTHLY_ANLAS, &json!("12.5"), Value::Null);
        assert!(v.is_f64());
        assert_eq!(
            encode(&GLOBAL_MONTHLY_ANLAS, &json!("5000000")).unwrap(),
            "99999"
        );
    }

    #[test]
    fn anlas_budgets_have_fixed_defaults() {
        // 默认值在登记表里，不再取环境变量
        assert_eq!(get_from_raw(&GLOBAL_DAILY_ANLAS, None), json!(200));
        assert_eq!(get_from_raw(&GLOBAL_MONTHLY_ANLAS, None), json!(10000.0));
        assert_eq!(get_from_raw(&GLOBAL_DAILY_ANLAS, Some("abc")), json!(200));
        assert_eq!(
            encode(&GLOBAL_DAILY_ANLAS, &json!("100001")).unwrap(),
            "9999"
        );
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
            parse(&REGISTER_OPEN, &json!("maybe"), Value::Bool(false)),
            Value::Bool(false)
        );
        assert_eq!(
            parse(&MEMBER_SWEEP, &json!(" OFF "), Value::Bool(true)),
            Value::Bool(false)
        );
        assert_eq!(get_from_raw(&ISSUE_HOURLY_CAP, Some("9999")), json!(500));
    }
}
