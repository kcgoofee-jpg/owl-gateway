//! 自动驾驶的两条规则（旧 `app/autopilot.py`）：全站熔断、单个 Key 限流。
//!
//! 两个纯函数 [`breaker`] 和 [`key_guard`]，各自由服务层的一个定时任务调用：输入是已经查好的数字，
//! 输出是有序的 [`Effect`] 和一句摘要（写进后台「定时任务」卡片）。判定与文案照旧版；
//! 不再写 `autopilot_last` / `autopilot_history`（旧的几行留在库里，不读不写）。
//!
//! # 服务层要实现的查询 / 内存读取
//!
//! | 字段 | 来源 | 参数 |
//! |---|---|---|
//! | [`BreakerInputs::rows`] | `SELECT SUM(CASE WHEN status='error' AND (up_status>=500 OR up_status BETWEEN 200 AND 299) THEN 1 ELSE 0 END), COUNT(*) FROM usage_log WHERE ts>=? AND kind LIKE 'image%' AND status IN ('ok','error')`（SUM 可能为 NULL）；另取这些失败里不同的 Key 数 `fail_keys` | `(since)`，见 [`breaker_since`]；`autopilot_breaker_until` 读不懂时按没有失败处理 |
//! | [`BreakerInputs::breaker_until`] | 内存里的 Guard：`breaker_until` | — |
//! | [`KeyGuardInputs::rejects`] | `SELECT key_id, SUM(CASE WHEN reason IN (<`[`KEY_GUARD_HALF_WEIGHT`]`>) THEN 1 ELSE 2 END)/2 FROM usage_log WHERE ts>=? AND status='rejected' AND key_id IS NOT NULL AND reason NOT IN (<`[`KEY_GUARD_SKIP_REASONS`]`>) GROUP BY key_id` | `(now - 3600)` |
//! | [`KeyGuardInputs::keys`] | `SELECT id, name FROM api_keys WHERE enabled=1 ORDER BY id`（决定 `decisions` 的顺序） | — |
//! | [`KeyGuardInputs::paused`] | 内存里的 Key 暂停表（`state.pauses`）：`key_id → 暂停到期时间` | — |
//!
//! # `effects` 的执行方式（按顺序）
//!
//! 同 [`crate::quota::Effect`]；这里用到：
//! - `TripBreaker`：`guard.trip_breaker(seconds, reason, now)`（内存态，`breaker_until = now + seconds`）；
//! - `Set`：熔断时写 `autopilot_breaker_until`（下一次统计从熔断结束后算起）；
//! - `PauseKey`：暂停单把 Key：写 `share_state` 表（表名为兼容旧库保留）并更新内存暂停表；
//! - `Log`：操作日志「自动驾驶：熔断」「自动驾驶：Key 限流」。
//!
//! # 已删除的规则
//!
//! - 2.x 单 Key 守护里依赖防分享证据的两条（来回切换网段、多客户端全天在用）：输入恒为空，从未触发。
//! - 「名额 +5」和每条规则的 `off` / `observe` / `enforce` 模式：熔断与单 Key 限流一律执行。

use crate::pyc;
use crate::quota::{DecideError, Effect};
use crate::reasons::{
    BODY_ABORT, BREAKER, BURST_CAP, CAP_3H, COOLDOWN, DAILY_CAP, FEATURE_OFF, HOURLY_CAP, KEY_BUSY,
    KEY_DISABLED, KEY_INVALID, KEY_PAUSED, KEY_RATE, MEMBER_QUEUE_FULL, MEMBER_QUOTA, PACED, PARAM,
    QUEUE_FULL, QUEUE_TIMEOUT, QUIET_CAP, SITE_QUOTA, TEST_REQUEST, UPSTREAM_LOCKED, V5_EXHAUSTED,
};
use std::collections::HashMap;

/// 单 Key 限流统计要排除的拒绝原因码：已暂停（`KEY_PAUSED`）+ 全站层面的拒绝（原因在站点：排队满、
/// 账号上限、熔断、冷却……，不是这个成员的客户端出了问题）。忙时自动重试的正常成员不能被误判为死循环。
pub const KEY_GUARD_SKIP_REASONS: [&str; 16] = [
    KEY_PAUSED,
    KEY_DISABLED,
    KEY_INVALID,
    QUEUE_FULL,
    HOURLY_CAP,
    QUIET_CAP,
    BURST_CAP,
    CAP_3H,
    DAILY_CAP,
    PACED,
    BREAKER,
    COOLDOWN,
    V5_EXHAUSTED,
    QUEUE_TIMEOUT,
    SITE_QUOTA,
    UPSTREAM_LOCKED,
];

/// 计入单 Key 限流时只按半次算的原因：站点拥堵时排队变长，客户端「上一张没出完就重发」
/// 一半要算到站点头上；真在死循环重发的客户端仍会在 2 倍次数时被暂停。
/// 每人排队已满（`MEMBER_QUEUE_FULL`，10-22 取代 `KEY_BUSY`）是同一件事，同样半次。
pub const KEY_GUARD_HALF_WEIGHT: [&str; 2] = [KEY_BUSY, MEMBER_QUEUE_FULL];

/// 由成员自己（客户端 / 网络）引起的拒绝：计入单 Key 限流。
///
/// 每个原因码都必须归到这里或 [`KEY_GUARD_SKIP_REASONS`] 之一（测试逐个核对 `reasons.rs` 里的常量）：
/// 新加原因码时必须先想清楚它算不算成员的问题。
pub const MEMBER_CAUSED: [&str; 8] = [
    KEY_BUSY,
    MEMBER_QUEUE_FULL,
    MEMBER_QUOTA,
    KEY_RATE,
    BODY_ABORT,
    FEATURE_OFF,
    PARAM,
    TEST_REQUEST,
];

/// 熔断设置键：上次熔断结束的时间（下次统计从这之后算）。
pub const BREAKER_UNTIL_KEY: &str = "autopilot_breaker_until";

// ---------------------------------------------------------------------------
// 纯规则
// ---------------------------------------------------------------------------

/// 熔断还要求失败来自至少这么多把不同的 Key：一个成员反复发畸形请求不算上游故障。
pub const BREAKER_MIN_KEYS: i64 = 2;
/// 熔断时全站暂停出图的秒数。
pub const BREAKER_SECS: i64 = 300;

/// 最近 15 分钟上游失败 ≥ 5 次且占比 ≥ 30% → 熔断（旧版规则；另加 Key 数门槛，见 [`breaker_rule_keys`]）。
pub fn breaker_rule(fails: i64, total: i64) -> (bool, String) {
    if fails >= 5 && total != 0 && (fails as f64 / total as f64) >= 0.3 {
        return (
            true,
            format!("最近 15 分钟上游失败 {fails}/{total}：暂停出图 5 分钟"),
        );
    }
    (false, format!("最近 15 分钟上游失败 {fails}/{total}：正常"))
}

/// [`breaker_rule`] 加上 Key 数门槛：失败只来自不到 [`BREAKER_MIN_KEYS`] 把 Key 时不熔断。
pub fn breaker_rule_keys(fails: i64, total: i64, fail_keys: i64) -> (bool, String) {
    let (trip, why) = breaker_rule(fails, total);
    if trip && fail_keys < BREAKER_MIN_KEYS {
        return (
            false,
            format!(
                "最近 15 分钟上游失败 {fails}/{total}，只来自 {fail_keys} 把 Key（多半是这把 Key 的请求有问题）：不熔断"
            ),
        );
    }
    (trip, why)
}

/// 单 Key 限流：1 小时内被拒 ≥ 这个次数，多半是客户端在死循环重试（站长 10-21 从 60 放宽到 120）。
pub const KEY_GUARD_REJECTS: i64 = 120;
/// 触发后暂停这把 Key 的秒数。
pub const KEY_GUARD_PAUSE_SECS: i64 = 3600;
/// 暂停时给成员看的说明（请求被拒时返回）。
pub const KEY_GUARD_PAUSE_REASON: &str =
    "你的 Key 短时间内被大量拒绝，疑似客户端在反复重试，已暂停 1 小时";

/// 单 Key 限流：1 小时内被拒 ≥ [`KEY_GUARD_REJECTS`] 次 → 返回暂停的原因；否则 `None`。
pub fn key_guard_rule(rejects_1h: i64) -> Option<String> {
    (rejects_1h >= KEY_GUARD_REJECTS)
        .then(|| format!("1 小时内被拒绝 {rejects_1h} 次（多半是客户端在死循环重试）：暂停 1 小时"))
}

/// 熔断统计的下界：`max(now - 900, float(autopilot_breaker_until or 0))`——只看上次熔断结束之后的数据。
/// 设置读不懂时返回错误，服务层按「没有失败」处理（不让整轮失败）。
pub fn breaker_since(now: f64, breaker_until_setting: Option<&str>) -> Result<f64, DecideError> {
    Ok(pyc::py_max(
        now - 900.0,
        setting_float(breaker_until_setting)?,
    ))
}

/// `float(get_setting(key, 0) or 0)`。
fn setting_float(raw: Option<&str>) -> Result<f64, DecideError> {
    match raw {
        None => Ok(0.0),
        Some("") => Ok(0.0),
        Some(s) => pyc::parse_float(s)
            .ok_or_else(|| DecideError::Value(format!("could not convert string to float: {s:?}"))),
    }
}

// ---------------------------------------------------------------------------
// 全站熔断
// ---------------------------------------------------------------------------

/// 熔断统计窗口里的上游失败。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BreakerRows {
    /// `SUM(CASE …)`，没有行时为 NULL。
    pub fails: Option<i64>,
    pub total: Option<i64>,
    /// 这些失败来自多少把不同的 Key。不到 [`BREAKER_MIN_KEYS`] 把不熔断。
    pub fail_keys: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BreakerInputs {
    pub now: f64,
    pub rows: BreakerRows,
    /// 内存 Guard 的 `breaker_until`：还没到期就是正在熔断中，不重复触发。
    pub breaker_until: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BreakerOutcome {
    /// 规则判定要熔断。
    pub trip: bool,
    /// 这一轮真的触发了（判定要熔断且此刻不在熔断中）。
    pub applied: bool,
    /// 判定说明（旧版 `why`，也是操作日志的前半句）。
    pub why: String,
    /// 一句摘要（后台「定时任务」卡片）。
    pub summary: String,
    pub effects: Vec<Effect>,
}

/// 全站熔断：最近 15 分钟（从上次熔断结束算起）上游失败 ≥ 5 次、占比 ≥ 30%、来自 ≥ 2 把 Key
/// → 暂停出图 5 分钟。已在熔断中不重复触发、不重复记日志。
pub fn breaker(inp: &BreakerInputs) -> BreakerOutcome {
    let now = inp.now;
    let fails = inp.rows.fails.unwrap_or(0);
    let total = inp.rows.total.unwrap_or(0);
    let (trip, why) = breaker_rule_keys(fails, total, inp.rows.fail_keys);
    let mut effects = Vec::new();
    let applied = trip && inp.breaker_until <= now;
    if applied {
        effects.push(Effect::TripBreaker {
            seconds: BREAKER_SECS,
            reason: format!("上游连续出错，已暂停出图 5 分钟（{why}）"),
            now,
        });
        effects.push(Effect::Set {
            key: BREAKER_UNTIL_KEY.to_string(),
            value: pyc::float_repr(now + BREAKER_SECS as f64),
        });
        effects.push(log(
            "自动驾驶：熔断",
            String::new(),
            format!("{why}；全站暂停出图 5 分钟"),
        ));
    }
    let summary = if trip && !applied {
        format!("{why}（已在熔断中，不重复触发）")
    } else {
        why.clone()
    };
    BreakerOutcome {
        trip,
        applied,
        why,
        summary,
        effects,
    }
}

// ---------------------------------------------------------------------------
// 单 Key 限流
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct KeyGuardInputs {
    pub now: f64,
    /// `(key_id, 被拒次数)`。
    pub rejects: Vec<(i64, i64)>,
    /// 参与判断的成员 Key `(id, name)`，按查询顺序。
    pub keys: Vec<(i64, String)>,
    /// 当前的暂停表：`key_id → 暂停到期时间`。
    pub paused: HashMap<i64, f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct KeyDecision {
    pub key: i64,
    pub name: String,
    pub why: String,
    /// 这一轮真的暂停了（此前不在暂停中）。
    pub applied: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct KeyGuardOutcome {
    /// 触发规则的 Key，顺序同 `keys`。
    pub decisions: Vec<KeyDecision>,
    pub summary: String,
    pub effects: Vec<Effect>,
}

/// 单 Key 限流：1 小时内被拒 ≥ 120 次（不算站点层面的拒绝）→ 暂停这把 Key 1 小时。已暂停的不重复暂停。
pub fn key_guard(inp: &KeyGuardInputs) -> KeyGuardOutcome {
    let now = inp.now;
    let rejects: HashMap<i64, i64> = inp.rejects.iter().copied().collect();
    // 重复的 id：位置取第一次、名字取最后一次（数据库主键不会重复，这里只是保险）
    let mut keys: Vec<(i64, &String)> = Vec::new();
    for (kid, name) in &inp.keys {
        match keys.iter_mut().find(|(k, _)| k == kid) {
            Some(slot) => slot.1 = name,
            None => keys.push((*kid, name)),
        }
    }
    let mut decisions = Vec::new();
    let mut effects = Vec::new();
    for (kid, name) in keys {
        let Some(why) = key_guard_rule(rejects.get(&kid).copied().unwrap_or(0)) else {
            continue;
        };
        let already = inp.paused.get(&kid).copied().unwrap_or(0.0) > now;
        if !already {
            effects.push(Effect::PauseKey {
                key_id: kid,
                seconds: KEY_GUARD_PAUSE_SECS,
                reason: KEY_GUARD_PAUSE_REASON.to_string(),
                now,
            });
            effects.push(log("自动驾驶：Key 限流", name.clone(), why.clone()));
        }
        decisions.push(KeyDecision {
            key: kid,
            name: name.clone(),
            why,
            applied: !already,
        });
    }
    let summary = key_guard_summary(&decisions);
    KeyGuardOutcome {
        decisions,
        summary,
        effects,
    }
}

/// 「没有 Key 触发」/「暂停 2 把：A、B」/「1 把 Key 触发，都已在暂停中」。
fn key_guard_summary(decisions: &[KeyDecision]) -> String {
    if decisions.is_empty() {
        return "没有 Key 触发".to_string();
    }
    let applied: Vec<&str> = decisions
        .iter()
        .filter(|d| d.applied)
        .map(|d| d.name.as_str())
        .collect();
    let skipped = decisions.len() - applied.len();
    if applied.is_empty() {
        return format!("{} 把 Key 触发，都已在暂停中", decisions.len());
    }
    let mut s = format!("暂停 {} 把：{}", applied.len(), applied.join("、"));
    if skipped > 0 {
        s.push_str(&format!("；另有 {skipped} 把已在暂停中"));
    }
    s
}

fn log(action: &str, target: String, detail: String) -> Effect {
    Effect::Log {
        actor: "系统".into(),
        action: action.into(),
        target,
        detail,
    }
}
