//! 账号保护与排队的纯逻辑：每个上游账号的每日出图上限（手动）、全站熔断、出图间隔与出图后静默的随机抖动、
//! 每人排队数、全站排队上限。
//!
//! [`GuardCore`] 持有全部内存状态；所有依赖时间的方法都显式传 `now`（Unix 秒），
//! 抖动用的随机数由调用方注入（`random.uniform(0, n)` = `0 + n * r`，r 为 [0,1) 的均匀随机数）。
//! 「一天」从北京时间 7 点开始（[`crate::clock::DAY_RESET_HOUR`]）。
//!
//! 全部设置都是**手动**的。已删除、库里的旧行只会被忽略的：每小时上限（`guard_account_hourly_cap` /
//! `guard_hourly_*` / `guard_daily_burst` / `guard_429_cap_at`，10-22）；每日上限的自动版（`quota_account_caps` /
//! `quota_account_cap_auto`）、出图间隔 AIMD 的四个 `guard_interval_*`、安静时段（`guard_quiet_start` /
//! `guard_quiet_end` / `guard_quiet_hourly_cap`，连同按账号记最近 3 小时出图的窗口）（10-23）。
//! 原因码 `hourly_cap` / `paced` / `quiet_cap` 只剩历史日志里有。
//!
//! 不在这里的 IO（服务层负责，键名都在 `site_settings` 表，`guard_<字段>`）：
//! - `load()`：对每个字段 `get_setting("guard_<字段>", None)`，读到的键值交给 [`GuardCore::load_settings`]；
//!   `save(body)`：先 [`GuardCore::validate_save`]，再 `set_settings_bulk(guard_<字段> → str(int))`，
//!   写成功之后才 [`GuardCore::apply_updates`]（先写库后改内存）；`to_settings` 给出全部字段的当前串；
//! - `token_block_reason(db, token_id, day)` 里的 `db.get_upstream_counter(token_id, day)`
//!   （`upstream_token_counters` 表的 `images`，没有记录是 0）：服务层取出 `used` 传进来。

use crate::clock::Clock;
use crate::pyc;
use crate::pyext::PyErr;
use crate::reasons::{self, Reason};
use serde_json::{Map, Value};
use std::collections::BTreeMap;

/// 一个设置字段：名称、默认值、最小、最大、说明。
#[derive(Debug, Clone, Copy)]
pub struct Field {
    pub name: &'static str,
    pub default: i64,
    pub min: i64,
    pub max: i64,
    pub label: &'static str,
}

const fn field(name: &'static str, default: i64, min: i64, max: i64, label: &'static str) -> Field {
    Field {
        name,
        default,
        min,
        max,
        label,
    }
}

/// 每个上游账号每日出图上限的默认值（手动；站长 10-22 定 2400）。
pub const ACCOUNT_DAILY_CAP_DEFAULT: i64 = 2400;

/// 每个成员最多同时排几张（`queue_per_member` 的默认值）。
pub const QUEUE_PER_MEMBER_DEFAULT: i64 = 2;

/// 所有设置（存 `site_settings` 的键是 `guard_<名称>`）；0 表示关闭该项。
pub const FIELDS: [Field; 5] = [
    field(
        "account_daily_cap",
        ACCOUNT_DAILY_CAP_DEFAULT,
        0,
        20000,
        "每个上游账号每天最多出图张数",
    ),
    // 出图节奏只剩这一对（站长 10-23）：每张出完后这个账号歇 N～M 秒（均匀随机）再开始下一张。
    // 开始到开始的「出图间隔」和它的「间隔抖动」已删除（并发 1 时出图本身就要七八秒，间隔从来不起作用）；
    // 原来写死的「响应后至少歇 1 秒」就是这里的下限 1（上游并发锁比响应晚一点释放）。旧的 guard_interval_jitter 行读进来也被忽略
    field(
        "post_jitter_min",
        1,
        1,
        30,
        "每张出完后至少歇几秒（实际在最少～最多之间均匀随机）",
    ),
    field(
        "post_jitter_max",
        3,
        1,
        30,
        "每张出完后最多歇几秒（小于最少时按最少）",
    ),
    // 每人排队上限（站长 10-22，手动）：取代旧的 `key_image_queue`（生成中那张之外再排几张，默认 1），
    // 旧键留在库里也只会被忽略。数的是还没发往上游的那几张（正在生成的不算）
    field(
        "queue_per_member",
        QUEUE_PER_MEMBER_DEFAULT,
        1,
        5,
        "每个成员最多同时排几张图（正在生成的那张不算）",
    ),
    // 只是上限：实际还会收紧到排队超时内这个账号真能开始的张数（见 queue_cap）；0 = 只按后者
    field(
        "queue_per_account",
        5,
        0,
        100,
        "每个上游账号全站最多同时排几张图",
    ),
];

/// `site_settings` 里的键名。
pub fn setting_key(name: &str) -> String {
    format!("guard_{name}")
}

/// 设置表里 `float(await get_setting(k, 0) or 0)` 的读法：没有 / 空串 → 0.0，读不懂 → ValueError。
pub fn parse_setting_float(raw: Option<&str>) -> Result<f64, PyErr> {
    match raw {
        None | Some("") => Ok(0.0),
        Some(s) => pyc::parse_float(s)
            .ok_or_else(|| PyErr::Value(format!("could not convert string to float: {s:?}"))),
    }
}

/// 进行中的出图（排队 / 生成），按进入时间排序。
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub id: i64,
    pub key: i64,
    pub since: f64,
    pub running: bool,
    /// 上游客户端估计的开始时刻（墙钟）：已经排到某个账号上才有。排队位次按它排（见 [`GuardCore::queue_view`]）。
    pub eta: Option<f64>,
}

/// `save(body)` 的结果。
#[derive(Debug, Clone, PartialEq)]
pub struct Saved {
    pub updates: Vec<(String, i64)>,
    /// `set_settings_bulk` 要写的键值；`updates` 为空时旧版不写库
    pub writes: Vec<(String, String)>,
}

#[derive(Debug, Clone)]
pub struct GuardCore {
    pub clock: Clock,
    /// 服务日起点（与 `state.day()` 同一口径）
    pub reset_hour: i64,
    pub values: BTreeMap<String, i64>,
    pub image_inflight: BTreeMap<i64, i64>,
    pub entries: Vec<Entry>,
    pub seq: i64,
    /// 全站熔断到期时间（内存态，重启即清）
    pub breaker_until: f64,
    pub breaker_reason: String,
    /// Temporary per-account daily caps: token_id → (cap, until epoch). Expired entries fall back to
    /// `account_daily_cap`. Loaded from the `guard_token_daily_caps` setting.
    pub token_daily_caps: BTreeMap<String, (i64, f64)>,
}

/// Setting that holds [`GuardCore::token_daily_caps`] as `{"token-id": {"cap": 1800, "until": 1792900000}}`.
pub const TOKEN_DAILY_CAPS_KEY: &str = "guard_token_daily_caps";

/// Parse the `guard_token_daily_caps` setting; anything malformed is skipped.
pub fn parse_token_daily_caps(raw: Option<&str>) -> BTreeMap<String, (i64, f64)> {
    let Some(serde_json::Value::Object(m)) = raw.and_then(|r| serde_json::from_str(r).ok()) else {
        return BTreeMap::new();
    };
    m.into_iter()
        .filter_map(|(k, v)| {
            let cap = v.get("cap")?.as_i64().filter(|c| *c >= 0)?;
            let until = v.get("until")?.as_f64()?;
            Some((k, (cap, until)))
        })
        .collect()
}

impl GuardCore {
    /// `Guard(db, tz, reset_hour)`（旧版默认 `reset_hour=0`；线上用 [`crate::clock::TZ`] / [`crate::clock::DAY_RESET_HOUR`]）。
    pub fn new(tz: &str, reset_hour: i64) -> Self {
        let reset_hour = reset_hour.clamp(0, 23);
        Self {
            clock: Clock::new(tz, reset_hour),
            reset_hour,
            values: FIELDS
                .iter()
                .map(|f| (f.name.to_string(), f.default))
                .collect(),
            image_inflight: BTreeMap::new(),
            entries: Vec::new(),
            seq: 0,
            breaker_until: 0.0,
            breaker_reason: String::new(),
            token_daily_caps: BTreeMap::new(),
        }
    }

    /// Daily cap for one account right now: a live temporary override, else `account_daily_cap`
    /// (manual). `account_daily_cap` = 0 is unlimited.
    pub fn daily_cap_for(&self, token_id: &str, now: f64) -> i64 {
        match self.token_daily_caps.get(token_id) {
            Some(&(cap, until)) if now < until => cap,
            _ => self.v("account_daily_cap"),
        }
    }

    /// 这个账号今天出满了吗（`used` ≥ 今天的上限；上限 0 = 不限，永远不满）。与 [`Self::token_block_reason`] 同一判断。
    pub fn at_daily_cap(&self, token_id: &str, used: i64, now: f64) -> bool {
        let daily = self.daily_cap_for(token_id, now);
        daily != 0 && used >= daily
    }

    /// Today's daily caps summed over these accounts (0 when `account_daily_cap` is 0 = unlimited).
    pub fn total_daily_cap(&self, token_ids: &[String], now: f64) -> i64 {
        if self.v("account_daily_cap") == 0 {
            return 0;
        }
        token_ids.iter().map(|t| self.daily_cap_for(t, now)).sum()
    }

    fn v(&self, name: &str) -> i64 {
        self.values[name]
    }

    // ---------- 设置 ----------

    /// `load()` 的解析：`stored` 是库里 `guard_*` 键的原文（没有的键 = 没存，保持现值）。
    /// 每个字段 `max(low, min(high, int(float(raw))))`；读不懂（ValueError / TypeError）回到字段默认值；
    /// `inf` 在旧版是 OverflowError 且没被捕获——这里同样返回 `Err`，前面已处理的字段保持已更新（与旧版一致）。
    pub fn load_settings(&mut self, stored: &BTreeMap<String, String>) -> Result<(), PyErr> {
        for f in FIELDS {
            let Some(raw) = stored.get(&setting_key(f.name)) else {
                continue;
            };
            let v = match pyc::parse_float(raw) {
                None => f.default,
                Some(x) if x.is_nan() => f.default,
                Some(x) => pyc::int_checked(x)?.clamp(f.min, f.max),
            };
            self.values.insert(f.name.to_string(), v);
        }
        Ok(())
    }

    /// 所有字段的当前值，写库用的串（`str(int)`，不带 `.0`），顺序同 `FIELDS`。
    pub fn to_settings(&self) -> Vec<(String, String)> {
        FIELDS
            .iter()
            .map(|f| (setting_key(f.name), self.values[f.name].to_string()))
            .collect()
    }

    /// `dict(self.values)`，顺序同 `FIELDS`。
    pub fn values_map(&self) -> Map<String, Value> {
        FIELDS
            .iter()
            .map(|f| (f.name.to_string(), Value::from(self.values[f.name])))
            .collect()
    }

    /// `save(body)` 的校验：不认识的字段忽略；每个值必须是整数（bool、字符串、小数都不行）且在范围内。
    /// 返回按请求体顺序的更新列表；不改内存。
    pub fn validate_save(body: &Map<String, Value>) -> Result<Vec<(String, i64)>, PyErr> {
        let mut updates = Vec::new();
        for (name, value) in body {
            let Some(f) = FIELDS.iter().find(|f| f.name == name) else {
                continue;
            };
            let int: Option<i128> = match value {
                Value::Number(n) if !n.is_f64() => n
                    .as_i64()
                    .map(i128::from)
                    .or_else(|| n.as_u64().map(i128::from)),
                // value != int(value)：有小数部分的浮点不是整数
                Value::Number(n) => n
                    .as_f64()
                    .filter(|x| x.is_finite() && *x == x.trunc())
                    .map(|x| x as i128),
                _ => None,
            };
            let Some(int) = int else {
                return Err(PyErr::Value(format!("{} 必须是整数", f.label)));
            };
            if !(i128::from(f.min) <= int && int <= i128::from(f.max)) {
                return Err(PyErr::Value(format!(
                    "{} 必须在 {}～{} 之间",
                    f.label, f.min, f.max
                )));
            }
            updates.push((name.clone(), int as i64));
        }
        Ok(updates)
    }

    /// `self.values.update(updates)`。
    pub fn apply_updates(&mut self, updates: &[(String, i64)]) {
        for (k, v) in updates {
            self.values.insert(k.clone(), *v);
        }
    }

    /// `save(body)` 整体：校验 + 改内存，并给出要写库的键值。
    /// 想保持旧版「先写库后改内存」的顺序时，改用 `validate_save` + 写库 + `apply_updates`。
    pub fn save(&mut self, body: &Map<String, Value>) -> Result<Saved, PyErr> {
        let updates = Self::validate_save(body)?;
        let writes = updates
            .iter()
            .map(|(k, v)| (setting_key(k), v.to_string()))
            .collect();
        self.apply_updates(&updates);
        Ok(Saved { updates, writes })
    }

    /// `describe()`：后台「账号保护」页的设置。
    pub fn describe(&self) -> Value {
        let fields: Map<String, Value> = FIELDS
            .iter()
            .map(|f| {
                let mut m = Map::new();
                m.insert("default".into(), Value::from(f.default));
                m.insert("min".into(), Value::from(f.min));
                m.insert("max".into(), Value::from(f.max));
                m.insert("label".into(), Value::String(f.label.into()));
                (f.name.to_string(), Value::Object(m))
            })
            .collect();
        let mut out = Map::new();
        out.insert("values".into(), Value::Object(self.values_map()));
        out.insert("fields".into(), Value::Object(fields));
        Value::Object(out)
    }

    /// 每张出完后歇的 (最少, 最多) 秒数（原样给出；`最多 < 最少` 按最少，由调度器处理）。
    pub fn post_jitter(&self) -> (i64, i64) {
        (self.v("post_jitter_min"), self.v("post_jitter_max"))
    }

    /// 全站熔断：在 `seconds` 秒内，所有上游账号都拒接新的出图任务。到点自动恢复。
    pub fn trip_breaker(&mut self, seconds: f64, reason: &str, now: f64) -> Result<(), PyErr> {
        self.breaker_until = now + if seconds > 0.0 { seconds } else { 0.0 };
        self.breaker_reason = if reason.is_empty() {
            format!(
                "NovelAI 连续出错，全站暂停出图 {} 秒，未扣费。到时自动恢复。",
                pyc::int_checked(seconds)?
            )
        } else {
            reason.to_string()
        };
        Ok(())
    }

    /// 这个上游账号现在不能再接新的出图任务时，返回给成员看的原因。
    /// `used` = 该账号今天已出的张数（`upstream_token_counters` 的 `images`）。
    pub fn token_block_reason(&self, token_id: &str, used: f64, now: f64) -> Option<Reason> {
        if self.breaker_until > now {
            let text = if self.breaker_reason.is_empty() {
                format!(
                    "NovelAI 连续出错，全站暂停出图，未扣费。约 {} 秒后自动恢复。",
                    (self.breaker_until - now) as i64
                )
            } else {
                self.breaker_reason.clone()
            };
            return Some(Reason::new(text, reasons::BREAKER));
        }
        let daily = self.daily_cap_for(token_id, now);
        if daily != 0 && used >= daily as f64 {
            return Some(Reason::new(
                format!(
                    "本站今日出图总量已达上限，每个账号每天 {daily} 张，未扣费。{}恢复。",
                    self.clock.next_reset_phrase(now)
                ),
                reasons::DAILY_CAP,
            ));
        }
        None
    }

    // ---------- 排队（P1） ----------

    /// 全站排队上限（排队 + 正在生成）：每个可用账号取 min(`queue_per_account`, 排队超时内它真能开始的张数)
    /// 再相加。`realistic` 每个可用账号一项（`gate_upstream` 的 `queue_caps`；None = 估不出 / 不限）；
    /// 一个可用账号都没有时按一个账号算。返回 None = 不限。
    pub fn queue_cap(&self, realistic: &[Option<i64>]) -> Option<i64> {
        let per = self.v("queue_per_account");
        let one = |r: Option<i64>| match (per, r) {
            (0, r) => r,
            (p, Some(r)) => Some(p.min(r)),
            (p, None) => Some(p),
        };
        if realistic.is_empty() {
            return one(None);
        }
        realistic
            .iter()
            .map(|r| one(*r))
            .try_fold(0i64, |sum, c| c.map(|c| sum.saturating_add(c)))
    }

    /// 全站排队满了吗（排队 + 正在生成 ≥ [`queue_cap`](Self::queue_cap)）：入队判断和 /public/live 的 `queue.full` 共用这一处。
    pub fn queue_full(&self, cap: Option<i64>) -> bool {
        let total: i64 = self.image_inflight.values().sum();
        cap.is_some_and(|c| total >= c)
    }

    /// 每个成员最多同时排 `queue_per_member` 张（还没发往上游的；同一把 Key 仍然一次只生成 1 张，见 [`key_turn`](Self::key_turn)）；
    /// 全站排队总数有上限（`cap`，见 [`queue_cap`](Self::queue_cap)）。
    /// 返回拒绝原因或 None。通过时登记一个排队条目（编号 = `self.seq`，取 `entries.last()` 可得）；`now` 是 `time.time()`。
    pub fn admit_image(&mut self, key_id: i64, cap: Option<i64>, now: f64) -> Option<Reason> {
        let mine = self.image_inflight.get(&key_id).copied().unwrap_or(0);
        let per = self.v("queue_per_member").max(1);
        let waiting = self
            .entries
            .iter()
            .filter(|e| e.key == key_id && !e.running)
            .count() as i64;
        if waiting >= per {
            return Some(Reason::new(
                format!("已有 {per} 张在排队，请等前面完成后再发。"),
                reasons::MEMBER_QUEUE_FULL,
            ));
        }
        if let Some(cap) = cap.filter(|_| self.queue_full(cap)) {
            return Some(Reason::new(
                format!("全站排队已满，最多同时排 {cap} 张，未扣费。请稍后重试。"),
                reasons::QUEUE_FULL,
            ));
        }
        self.image_inflight.insert(key_id, mine + 1);
        self.seq += 1;
        self.entries.push(Entry {
            id: self.seq,
            key: key_id,
            since: now,
            running: false,
            eta: None,
        });
        None
    }

    /// 上游客户端估计这一张 `eta`（墙钟）开始（排到 / 换到某个账号上之后，排队期间会更新）。
    pub fn set_eta(&mut self, entry_id: i64, eta: f64) {
        if let Some(e) = self.entries.iter_mut().find(|e| e.id == entry_id) {
            e.eta = Some(eta);
        }
    }

    /// 轮到这把 Key 的这一张了吗：它是这把 Key 还在的条目里最早登记的（前面那张已出完或被取消）。
    /// 每把 Key 同时只生成 1 张，排在后面的那张等它——取代旧版每把 Key 一个的信号量。
    pub fn key_turn(&self, key_id: i64, entry_id: i64) -> bool {
        !self
            .entries
            .iter()
            .any(|e| e.key == key_id && e.id < entry_id)
    }

    /// 这把 Key 的一张排队中的图开始发往上游；给了 `entry_id` 就标记那一张。
    pub fn mark_running(&mut self, key_id: i64, entry_id: Option<i64>) {
        for e in &mut self.entries {
            if e.key == key_id && !e.running && entry_id.is_none_or(|id| e.id == id) {
                e.running = true;
                return;
            }
        }
    }

    /// 排队中的条目，按预计开始的先后：已经排到账号上的按上游估计的开始时刻（两个账号各排各的，
    /// 先来的不一定先出），还没排到账号的（在等同一把 Key 的前一张）排在后面；再按进入时间。稳定排序。
    fn waiting_sorted(&self) -> Vec<&Entry> {
        let mut w: Vec<&Entry> = self.entries.iter().filter(|e| !e.running).collect();
        w.sort_by(|a, b| {
            let (ea, eb) = (
                a.eta.unwrap_or(f64::INFINITY),
                b.eta.unwrap_or(f64::INFINITY),
            );
            ea.total_cmp(&eb).then(a.since.total_cmp(&b.since))
        });
        w
    }

    pub fn release_image(&mut self, key_id: i64, entry_id: Option<i64>) {
        let n = self.image_inflight.get(&key_id).copied().unwrap_or(0) - 1;
        if n > 0 {
            self.image_inflight.insert(key_id, n);
        } else {
            self.image_inflight.remove(&key_id);
        }
        let mine: Vec<usize> = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| e.key == key_id)
            .map(|(i, _)| i)
            .collect();
        let done = match entry_id {
            // 按条目编号移除：排队中的那张被取消时，不能误删正在生成的那张
            Some(id) => mine.iter().copied().find(|&i| self.entries[i].id == id),
            // 旧调用：先移除正在生成的那张，否则移除最早的
            None => mine
                .iter()
                .copied()
                .find(|&i| self.entries[i].running)
                .or_else(|| mine.first().copied()),
        };
        if let Some(i) = done {
            self.entries.remove(i);
        }
    }

    /// 匿名的排队视图：waiting / running 数量；给定 key_id 时附上这把 Key 的图所处位置（1 起，按预计开始先后），
    /// 上游给过估计的再附 `start_at`（预计开始的墙钟时刻）。
    pub fn queue_view(&self, key_id: Option<i64>) -> Value {
        let waiting = self.waiting_sorted();
        let running: Vec<&Entry> = self.entries.iter().filter(|e| e.running).collect();
        let mut out = Map::new();
        out.insert("waiting".into(), Value::from(waiting.len()));
        out.insert("running".into(), Value::from(running.len()));
        if let Some(k) = key_id {
            let mut mine = Vec::new();
            for _ in running.iter().filter(|e| e.key == k) {
                let mut m = Map::new();
                m.insert("state".into(), Value::String("running".into()));
                mine.push(Value::Object(m));
            }
            for (i, e) in waiting.iter().enumerate() {
                if e.key == k {
                    let mut m = Map::new();
                    m.insert("state".into(), Value::String("waiting".into()));
                    m.insert("position".into(), Value::from(i + 1));
                    if let Some(at) = e.eta {
                        m.insert("start_at".into(), Value::from(at));
                    }
                    mine.push(Value::Object(m));
                }
            }
            out.insert("mine".into(), Value::Array(mine));
        }
        Value::Object(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // 2026-10-14 12:00:00 +08:00
    const NOON: f64 = 1_791_950_400.0;

    fn guard() -> GuardCore {
        GuardCore::new("Asia/Shanghai", 7)
    }

    #[test]
    fn temporary_per_account_daily_cap() {
        let mut g = guard();
        g.values.insert("account_daily_cap".into(), 1400);
        g.token_daily_caps = parse_token_daily_caps(Some(
            r#"{"t1": {"cap": 1800, "until": 1791960000}, "bad": {"cap": "x"}, "old": {"cap": 9000, "until": 1}}"#,
        ));
        assert_eq!(g.token_daily_caps.len(), 2);
        assert_eq!(g.daily_cap_for("t1", NOON), 1800);
        assert_eq!(g.daily_cap_for("t2", NOON), 1400);
        assert_eq!(g.daily_cap_for("old", NOON), 1400); // expired
        // t1 keeps going past the base cap, t2 stops at it
        assert!(g.token_block_reason("t1", 1500.0, NOON).is_none());
        let r = g
            .token_block_reason("t2", 1500.0, NOON)
            .expect("t2 at its cap");
        assert_eq!(r.code(), reasons::DAILY_CAP);
        let r = g
            .token_block_reason("t1", 1800.0, NOON)
            .expect("t1 at its raised cap");
        assert!(r.text().contains("每天 1800 张"), "{}", r.text());
        // after `until` the raise is gone
        assert_eq!(g.daily_cap_for("t1", 1_791_960_000.0), 1400);
    }

    #[test]
    fn no_paced_share_before_the_daily_cap() {
        // 7 点刚过就用了 2000 张（以前「7 点先放 150 张、每小时补 100 张」早拦了）：每日上限之前都放行
        let g = guard();
        let morning = NOON - 5.0 * 3600.0 + 60.0;
        assert_eq!(g.token_block_reason("t", 2000.0, morning), None);
    }

    #[test]
    fn daily_cap_counts_real_images() {
        // 节约模式已删除：按实际张数比，文案不再写「按算力折算」
        let g = guard();
        assert_eq!(g.token_block_reason("t", 2399.0, NOON), None);
        let r = g.token_block_reason("t", 2400.0, NOON).unwrap();
        assert_eq!(r.code(), reasons::DAILY_CAP);
        assert!(!r.text().contains("折算"), "{}", r.text());
    }

    #[test]
    fn queue_full_is_the_same_rule_admit_image_refuses_with() {
        let mut g = guard();
        g.values.insert("queue_per_account".into(), 2);
        g.values.insert("queue_per_member".into(), 5);
        // 2 个账号 → 全站最多 4 张（排队 + 生成中）
        let two = g.queue_cap(&[None, None]);
        assert_eq!(two, Some(4));
        for (i, key) in [1, 2, 3, 4].into_iter().enumerate() {
            assert!(!g.queue_full(two), "第 {i} 张之前还没满");
            assert_eq!(g.admit_image(key, two, NOON), None);
        }
        assert!(g.queue_full(two));
        let r = g.admit_image(5, two, NOON).unwrap();
        assert_eq!(r.code(), reasons::QUEUE_FULL);
        assert!(r.text().contains("最多同时排 4 张"), "{}", r.text());
        // 账号多了，上限跟着变；一个可用账号都没有按一个算；None = 不限
        assert!(!g.queue_full(g.queue_cap(&[None, None, None])));
        assert_eq!(g.queue_cap(&[]), Some(2));
        assert!(!g.queue_full(None));
    }

    #[test]
    fn queue_cap_is_what_the_accounts_can_start_before_the_timeout() {
        let mut g = guard();
        g.values.insert("queue_per_account".into(), 10);
        // 排队超时 90 秒、周期 17.5 秒 → 每个账号 6 张；设置的 10 只是上限
        assert_eq!(g.queue_cap(&[Some(6), Some(6)]), Some(12));
        // 一个账号被 429 拖慢（周期 35 秒 → 3 张）
        assert_eq!(g.queue_cap(&[Some(6), Some(3)]), Some(9));
        // 设置更小时以设置为准
        g.values.insert("queue_per_account".into(), 4);
        assert_eq!(g.queue_cap(&[Some(6), Some(3)]), Some(7));
        // 0 = 只按估算；估算不出（测试环境间隔 0）也不限
        g.values.insert("queue_per_account".into(), 0);
        assert_eq!(g.queue_cap(&[Some(6), Some(3)]), Some(9));
        assert_eq!(g.queue_cap(&[Some(6), None]), None);
    }

    #[test]
    fn queue_position_follows_the_upstream_estimate() {
        let mut g = guard();
        g.values.insert("queue_per_member".into(), 3);
        // 1、2、3 号 Key 依次来；2 号排到了空着的账号上，估计比 1 号先开始
        for (key, at) in [(1, 0.0), (2, 1.0), (3, 2.0)] {
            assert_eq!(g.admit_image(key, None, NOON + at), None);
        }
        let ids: Vec<i64> = g.entries.iter().map(|e| e.id).collect();
        g.set_eta(ids[0], NOON + 20.0);
        g.set_eta(ids[1], NOON + 5.0);
        let pos = |g: &GuardCore, key| g.queue_view(Some(key))["mine"][0]["position"].clone();
        assert_eq!(pos(&g, 2), 1);
        assert_eq!(pos(&g, 1), 2);
        // 还没排到账号的（没有估计）在后面
        assert_eq!(pos(&g, 3), 3);
        assert_eq!(g.queue_view(Some(2))["mine"][0]["start_at"], NOON + 5.0);
        assert!(g.queue_view(Some(3))["mine"][0].get("start_at").is_none());
    }

    #[test]
    fn key_turn_is_first_come_first_served() {
        let mut g = guard();
        let one = g.queue_cap(&[None]);
        assert_eq!(g.admit_image(7, one, NOON), None);
        let first = g.entries.last().unwrap().id;
        assert_eq!(g.admit_image(7, one, NOON + 1.0), None);
        let second = g.entries.last().unwrap().id;
        assert_eq!(g.admit_image(8, one, NOON + 2.0), None);
        let other = g.entries.last().unwrap().id;
        // 同一把 Key：先来的先生成，后来的等；别的 Key 不受影响
        assert!(g.key_turn(7, first));
        assert!(!g.key_turn(7, second));
        assert!(g.key_turn(8, other));
        // 第三张排不进来（每人最多同时排 2 张，两张都还没发出）
        assert_eq!(
            g.admit_image(7, one, NOON + 3.0)
                .map(|r| r.code().to_string()),
            Some(reasons::MEMBER_QUEUE_FULL.to_string())
        );
        // 前一张出完（或被取消）：轮到后一张
        g.release_image(7, Some(first));
        assert!(g.key_turn(7, second));
    }

    #[test]
    fn member_queue_cap_counts_only_images_not_yet_sent() {
        let mut g = guard();
        assert_eq!(g.v("queue_per_member"), QUEUE_PER_MEMBER_DEFAULT);
        let admit = |g: &mut GuardCore, at: f64| g.admit_image(9, None, NOON + at);
        assert_eq!(admit(&mut g, 0.0), None);
        let first = g.entries.last().unwrap().id;
        assert_eq!(admit(&mut g, 1.0), None);
        // 两张都在排队：第三张当场拒绝，文案带张数，原因码是 member_queue_full
        let r = admit(&mut g, 2.0).unwrap();
        assert_eq!(r.code(), reasons::MEMBER_QUEUE_FULL);
        assert!(
            r.text().contains("已有 2 张在排队，请等前面完成后再发"),
            "{}",
            r.text()
        );
        assert_eq!(g.image_inflight[&9], 2, "被拒的不登记");
        // 第一张发往上游以后不算排队：可以再排一张（生成 1 + 排队 2）
        g.mark_running(9, Some(first));
        assert_eq!(admit(&mut g, 3.0), None);
        assert_eq!(
            admit(&mut g, 4.0).map(|r| r.code().to_string()),
            Some(reasons::MEMBER_QUEUE_FULL.to_string())
        );
        // 设置是手动的：调成 1
        g.values.insert("queue_per_member".into(), 1);
        assert_eq!(
            g.admit_image(10, None, NOON).map(|_| ()),
            None,
            "别人不受影响"
        );
        assert!(g.admit_image(10, None, NOON).is_some());
    }

    #[test]
    fn removed_settings_are_ignored() {
        // 旧库里还留着每小时上限 / 按时段放行的设置：读进来不报错、不生效；后台再提交也只是忽略
        let mut g = guard();
        let stored: BTreeMap<String, String> = [
            ("guard_account_hourly_cap", "100"),
            ("guard_daily_burst", "150"),
            ("guard_hourly_min", "100"),
            ("guard_key_image_queue", "0"),
            ("guard_quiet_start", "3"),
            ("guard_quiet_hourly_cap", "0"),
            ("guard_interval_cap", "60"),
            ("guard_account_daily_cap", "1800"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        g.load_settings(&stored).unwrap();
        // 安静时段、出图间隔 AIMD 10-23 删除：旧行读进来也不生效；单号每日上限（手动）照常
        assert!(!g.values.contains_key("quiet_start"));
        assert!(!g.values.contains_key("interval_cap"));
        assert_eq!(g.v("account_daily_cap"), 1800);
        assert_eq!(g.token_block_reason("t", 0.0, NOON), None);
        assert!(!g.values.contains_key("account_hourly_cap"));
        // 旧的每把 Key 排队数被每人排队上限取代：旧值不影响新设置
        assert!(!g.values.contains_key("key_image_queue"));
        assert_eq!(g.v("queue_per_member"), QUEUE_PER_MEMBER_DEFAULT);
        let body = serde_json::json!({"account_hourly_cap": 80, "daily_burst": 0});
        assert_eq!(g.save(body.as_object().unwrap()).unwrap().updates, vec![]);
        assert_eq!(g.values_map().len(), FIELDS.len());
    }

    #[test]
    fn save_rejects_non_integers() {
        let body = serde_json::json!({"post_jitter_max": 1.5});
        assert!(GuardCore::validate_save(body.as_object().unwrap()).is_err());
        let body = serde_json::json!({"post_jitter_max": 3.0, "nope": "x", "interval_jitter": 3});
        assert_eq!(
            GuardCore::validate_save(body.as_object().unwrap()).unwrap(),
            vec![("post_jitter_max".to_string(), 3)]
        );
        // 歇的下限是 1 秒（上游并发锁比响应晚一点释放）：0 不收
        let body = serde_json::json!({"post_jitter_min": 0});
        assert!(GuardCore::validate_save(body.as_object().unwrap()).is_err());
    }
}
