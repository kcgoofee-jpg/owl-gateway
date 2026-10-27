//! 额度：把站长手动定的每人额度下发给成员 Key，写首页那句「今日额度」；另有 V5 电量的记录（只显示）。
//!
//! 站长 10-23：所有自动额度算法删除，额度全是手动的一个个数（坚守初心让更多人进来用、调低个人限额，重度用户自己花钱）：
//! - V4.5 每人每天 `register_daily_images`、V5 每人每天 `register_daily_v5`、全站每日 V5 `global_daily_v5`、
//!   V5 暂停 `v5_paused`（开关）——登记见 [`crate::site_flags`]；
//! - 删除的：V4.5 每人上限自动调整（`quota_ceiling` / `quota_step` / `quota_target_avg` / `quota_ceiling_max` /
//!   `quota_base_min_fair`、water-fill）、新号爬坡（`newbie_ramp_*`）、按电量定的 V5 当天方案（`quota_v5_day_plan`、
//!   临时加量 `quota_v5_bonus`、自动暂停 `quota_v5_paused`）、动态额度总开关 `quota_auto_enabled`、每日复盘
//!   （`quota_algo_day` / `quota_algo_history` / `quota_algo_last`）、账号日上限的自动版（`quota_account_caps` /
//!   `quota_account_cap_auto`）。库里的旧行不删，只是不再读写。
//!
//! [`sync`]：额度同步任务每轮（以及后台改了额度时）调一次：成员 Key 的值和手动值不一样才写，文案变了才写。
//! [`Effect::UpdateKeys`] 的执行：`UPDATE api_keys SET daily_images=?, daily_v5=?`
//! WHERE quota_auto=1`。后台手动设过额度的 Key（`quota_auto=-1`）不动。

use crate::validation::ValidationError;
use crate::{primitives, values};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// 首页那句「今日额度」（`site_settings` 键）。
pub const NOTICE_KEY: &str = "algo_notice";
/// V5 暂停时成员看到的话（拒绝 V5 时的 402，原因码 `v5_exhausted`）。
pub const V5_PAUSED_TEXT: &str = "V5 暂停，未扣费。请使用 V4.5。";
/// Meter readings at or above this are treated as full (recovery is clipped, not measurable).
pub const V5_FULL_PERCENT: f64 = 99.5;

// ---------------------------------------------------------------------------
// 错误与副作用
// ---------------------------------------------------------------------------

/// 额度判定失败，按异常类型分类（错误文案以 [`DecideError::error_kind`] 的类型名开头）。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DecideError {
    #[error("ValueError: {0}")]
    Value(String),
    #[error("JSONDecodeError: {0}")]
    Json(String),
    #[error("TypeError: {0}")]
    Type(String),
    #[error("KeyError: {0}")]
    Key(String),
    #[error("AttributeError: {0}")]
    Attribute(String),
    #[error("OverflowError: {0}")]
    Overflow(String),
    #[error("ZeroDivisionError: {0}")]
    ZeroDivision(String),
    /// 服务层没按约定提供某组输入（内部 bug，不属于上游判定失败）。
    #[error("缺少输入: {0}")]
    MissingInput(String),
}

impl DecideError {
    pub fn error_kind(&self) -> &'static str {
        match self {
            Self::Value(_) => "invalid_value",
            Self::Json(_) => "JSONDecodeError",
            Self::Type(_) => "invalid_type",
            Self::Key(_) => "KeyError",
            Self::Attribute(_) => "invalid_structure",
            Self::Overflow(_) => "out_of_range",
            Self::ZeroDivision(_) => "division_by_zero",
            Self::MissingInput(_) => "MissingInput",
        }
    }
}

impl From<ValidationError> for DecideError {
    fn from(e: ValidationError) -> Self {
        match e {
            ValidationError::InvalidValue(m) => Self::Value(m),
            ValidationError::InvalidType(m) => Self::Type(m),
            ValidationError::InvalidStructure(m) => Self::Attribute(m),
            ValidationError::OutOfRange(m) => Self::Overflow(m),
            ValidationError::DivisionByZero(m) => Self::ZeroDivision(m),
        }
    }
}

/// 一次写入 / 动作，按发生的顺序排列（额度同步与自动驾驶共用）。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Effect {
    /// `db.set_setting(key, value)`（值已是 `str(...)` 之后的精确字符串）。
    Set { key: String, value: String },
    /// `db.set_settings_bulk({...})`（保持字典顺序）。
    Bulk { pairs: Vec<(String, String)> },
    /// `log_action(db, actor, action, target, detail)`。
    Log {
        actor: String,
        action: String,
        target: String,
        detail: String,
    },
    /// 把手动额度下发给 `quota_auto=1` 的成员 Key（见模块文档）。
    UpdateKeys { daily_images: i64, daily_v5: i64 },
    /// 自动驾驶：`guard.trip_breaker(seconds, reason, now)`（内存态）。
    TripBreaker {
        seconds: i64,
        reason: String,
        now: f64,
    },
    /// 自动驾驶：临时暂停单把 Key（落 `share_state` 表并更新内存暂停表）。
    PauseKey {
        key_id: i64,
        seconds: i64,
        reason: String,
        now: f64,
    },
}

// ---------------------------------------------------------------------------
// 手动额度的下发与首页文案
// ---------------------------------------------------------------------------

/// 站长手动定的额度（都已按登记表解析、夹好）。10-27 规范化：0 = 停用，上限最大 9999。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manual {
    /// V4.5 每人每天（0 = 停用）
    pub daily_images: i64,
    /// V5 每人每天（0 = 停用）
    pub daily_v5: i64,
    /// 全站每日 V5（0 = 停用）
    pub global_v5: i64,
    pub v5_paused: bool,
    /// Anlas 每人每天（0 = 停用）
    pub member_anlas: i64,
}

/// `quota_auto=1` 的成员 Key 当前的值。
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManagedKey {
    pub daily_images: i64,
    pub daily_v5: i64,
    pub image_model_scope: String,
}

/// 首页那句「今日额度」。
pub fn quota_notice(m: &Manual, reset_hour: i64) -> String {
    let v45 = if m.daily_images > 0 {
        format!("V4.5 每人 {} 张/天", m.daily_images)
    } else {
        "V4.5 已停用".to_string()
    };
    let v5 = if m.v5_paused {
        " · V5 暂停中".to_string()
    } else if m.daily_v5 == 0 {
        " · V5 已停用".to_string()
    } else {
        let each = format!("每人 {} 张", m.daily_v5);
        if m.global_v5 > 0 {
            format!(" · V5 {each}（全站今日共 {} 张，先到先得）", m.global_v5)
        } else {
            " · V5 已停用（全站池未开放）".to_string()
        }
    };
    let anlas = if m.member_anlas > 0 {
        format!(" · Anlas 每人 {}/天", m.member_anlas)
    } else {
        String::new()
    };
    format!("今日额度：{v45}（每日 {reset_hour} 点重置）{v5}{anlas}")
}

/// 一轮同步要做的写入：成员 Key 和手动值不一样 → [`Effect::UpdateKeys`]；文案变了 → 写 [`NOTICE_KEY`]。
/// 返回 (写入, 要改几把 Key)。
pub fn sync(
    m: &Manual,
    keys: &[ManagedKey],
    notice_now: Option<&str>,
    reset_hour: i64,
) -> (Vec<Effect>, usize) {
    let mut out = Vec::new();
    let changed = keys
        .iter()
        .filter(|k| k.daily_images != m.daily_images || k.daily_v5 != m.daily_v5)
        .count();
    if changed > 0 {
        out.push(Effect::UpdateKeys {
            daily_images: m.daily_images,
            daily_v5: m.daily_v5,
        });
    }
    let note = quota_notice(m, reset_hour);
    if notice_now != Some(note.as_str()) {
        out.push(Effect::Set {
            key: NOTICE_KEY.into(),
            value: note,
        });
    }
    (out, changed)
}

// ---------------------------------------------------------------------------
// V5 电量记录（只显示，不参与任何限额）
// ---------------------------------------------------------------------------

/// 一条上游快照（`upstream_snapshots`）。
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuietSnap {
    pub ts: f64,
    pub percent: f64,
}

/// `quiet_recovery` 的输入：按 `ts` 升序的快照 + 相邻两条之间的 V5 出图条数。
#[derive(Debug, Clone, PartialEq, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct QuietInputs {
    pub snaps: Vec<QuietSnap>,
    /// `counts[i]` = `snaps[i]` 到 `snaps[i+1]` 之间的 V5 成功日志条数（间隔 > 3 小时的不会被读，值任意）。
    pub counts: Vec<i64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Quiet {
    pub rate: f64,
    pub hours: f64,
    pub gain: f64,
}

impl Quiet {
    pub fn to_value(&self) -> Value {
        let mut m = Map::new();
        m.insert("rate".into(), values::float(self.rate));
        m.insert("hours".into(), values::float(self.hours));
        m.insert("gain".into(), values::float(self.gain));
        Value::Object(m)
    }
}

/// `quiet_recovery` 回看的窗口（72 小时）。
pub const QUIET_WINDOW_SECS: f64 = 72.0 * 3600.0;

/// 服务层查快照的下界参数：`ts > now - 72h`。
pub fn quiet_snaps_since(now: f64) -> f64 {
    now - QUIET_WINDOW_SECS
}

/// 要数 V5 条数的相邻快照对（间隔 ≤ 3 小时）的下标 `i`（即 `snaps[i]` → `snaps[i+1]`）。
/// 服务层只需要对这些下标执行 `quiet.counts[i]` 那条查询，其余位置填 0 即可。
pub fn quiet_count_pairs(snaps: &[QuietSnap]) -> Vec<usize> {
    snaps
        .windows(2)
        .enumerate()
        .filter(|(_, w)| {
            let skipped = w[1].ts - w[0].ts > 3.0 * 3600.0;
            !skipped
        })
        .map(|(i, _)| i)
        .collect()
}

/// 独立测量恢复速度：相邻两次「主动读」的快照之间一张 V5 都没出 → 剩余的上涨就是纯恢复。
/// 累计 ≥ 4 个安静小时才给结果。
pub fn quiet_recovery(q: &QuietInputs) -> Option<Quiet> {
    let mut hours = 0.0_f64;
    let mut gain = 0.0_f64;
    for (i, w) in q.snaps.windows(2).enumerate() {
        let (a, b) = (&w[0], &w[1]);
        if b.ts - a.ts > 3.0 * 3600.0 {
            continue;
        }
        // A meter pinned at 100% cannot rise: counting those hours as zero gain under-reads the rate.
        if a.percent >= V5_FULL_PERCENT || b.percent >= V5_FULL_PERCENT {
            continue;
        }
        if q.counts.get(i).copied().unwrap_or(0) == 0 {
            hours += (b.ts - a.ts) / 3600.0;
            gain += b.percent - a.percent;
        }
    }
    if hours < 4.0 {
        return None;
    }
    Some(Quiet {
        rate: primitives::round(gain / hours * 24.0, 2),
        hours: primitives::round(hours, 1),
        gain,
    })
}

/// 一个上游账号的 V5 缓存（`allowance.snapshot` 里的一项）。
/// 线上 `percent` 一定是 int（`allowance.resolve` 拒绝非 int），`recharge_per_day` 是 `round(86400/秒数, 1)` 的 float。
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AccountAllowance {
    pub percent: Option<f64>,
    pub recharge_per_day: Option<f64>,
}

/// `_allowance` 的纯部分：账号 V5 剩余 % 与恢复 %/天（多个账号取平均；没有读数返回 `(None, None)`）。
pub fn allowance_summary(accounts: &[AccountAllowance]) -> (Option<f64>, Option<f64>) {
    let rows: Vec<&AccountAllowance> = accounts.iter().filter(|a| a.percent.is_some()).collect();
    if rows.is_empty() {
        return (None, None);
    }
    let pct = primitives::sum(rows.iter().filter_map(|a| a.percent)) / rows.len() as f64;
    // `if a.get("recharge_per_day")`：0 / None 不算
    let rates: Vec<f64> = rows
        .iter()
        .filter_map(|a| a.recharge_per_day)
        .filter(|r| *r != 0.0)
        .collect();
    let rate = if rates.is_empty() {
        None
    } else {
        Some(primitives::sum(rates.iter().copied()) / rates.len() as f64)
    };
    (Some(pct), rate)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manual() -> Manual {
        Manual {
            daily_images: 150,
            daily_v5: 0,
            global_v5: 0,
            v5_paused: true,
            member_anlas: 150,
        }
    }

    fn key(images: i64, v5: i64) -> ManagedKey {
        ManagedKey {
            daily_images: images,
            daily_v5: v5,
            image_model_scope: "all".into(),
        }
    }

    #[test]
    fn notice_says_the_manual_numbers() {
        let m = manual();
        assert_eq!(
            quota_notice(&m, 7),
            "今日额度：V4.5 每人 150 张/天（每日 7 点重置） · V5 暂停中 · Anlas 每人 150/天"
        );
        let open = Manual {
            v5_paused: false,
            daily_v5: 10,
            global_v5: 300,
            member_anlas: 0,
            daily_images: 0,
        };
        assert_eq!(
            quota_notice(&open, 7),
            "今日额度：V4.5 已停用（每日 7 点重置） · V5 每人 10 张（全站今日共 300 张，先到先得）"
        );
        // 10-27：全局 V5 池停用（0）时不再显示成「不限」
        let closed = Manual {
            v5_paused: false,
            daily_v5: 10,
            global_v5: 0,
            member_anlas: 0,
            daily_images: 150,
        };
        assert_eq!(
            quota_notice(&closed, 7),
            "今日额度：V4.5 每人 150 张/天（每日 7 点重置） · V5 已停用（全站池未开放）"
        );
    }

    #[test]
    fn sync_writes_only_what_differs() {
        let m = manual();
        let note = quota_notice(&m, 7);
        // 新号老号都一样：只要和手动值不同就改（新号爬坡已删）
        let mut restricted = key(150, 0);
        restricted.image_model_scope = "legacy".into();
        let (fx, n) = sync(
            &m,
            &[key(150, 0), restricted, key(75, 0), key(175, 3)],
            None,
            7,
        );
        assert_eq!(n, 2);
        assert_eq!(
            fx,
            vec![
                Effect::UpdateKeys {
                    daily_images: 150,
                    daily_v5: 0
                },
                Effect::Set {
                    key: NOTICE_KEY.into(),
                    value: note.clone()
                }
            ]
        );
        // 第二轮什么都不写
        let (fx, n) = sync(&m, &[key(150, 0)], Some(&note), 7);
        assert_eq!((fx, n), (vec![], 0));
    }

    fn snaps(points: &[(f64, f64)]) -> QuietInputs {
        QuietInputs {
            snaps: points
                .iter()
                .map(|&(h, percent)| QuietSnap {
                    ts: h * 3600.0,
                    percent,
                })
                .collect(),
            counts: vec![0; points.len().saturating_sub(1)],
        }
    }

    #[test]
    fn quiet_recovery_ignores_hours_pinned_at_full() {
        // 0.4%/h for 6 hours, then pinned at 100% for 6 hours: 9.6%/day, not 4.8%/day.
        let mut pts: Vec<(f64, f64)> = (0..=6).map(|h| (h as f64, 97.0 + 0.4 * h as f64)).collect();
        pts.extend((7..=12).map(|h| (h as f64, 100.0)));
        let q = quiet_recovery(&snaps(&pts)).expect("enough quiet hours");
        assert_eq!(q.hours, 6.0);
        assert_eq!(q.rate, 9.6);
    }

    #[test]
    fn quiet_recovery_needs_four_unclipped_hours() {
        let pts: Vec<(f64, f64)> = (0..=10).map(|h| (h as f64, 100.0)).collect();
        assert!(quiet_recovery(&snaps(&pts)).is_none());
    }
}
