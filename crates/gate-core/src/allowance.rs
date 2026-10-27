//! V5 订阅额度缓存的纯逻辑部分（对应 `app/allowance.py` 的 `AllowanceCache`）。
//!
//! 判定逻辑拆成「服务层做 IO、这里做判定」的几步：
//!
//! 1. 服务层持有每个 token 的锁，取单调时钟 `now_mono`，读阈值（见下），调 [`AllowanceCore::check_cache`]：
//!    `Unavailable` → 抛 [`UNAVAILABLE_MSG`]；`Cached` → 返回 `false`（不是耗尽）；`Fetch` → 去请求上游；
//! 2. 请求 `GET {host}/user/subscription`（和生图同一套浏览器请求头，8 秒总超时，不跟随重定向，
//!    响应体最多读 65536 字节），把结果装成 [`Fetched`]；
//! 3. 调 [`AllowanceCore::finish_resolve`]（传请求结束后的墙钟 / 单调时钟）：返回 [`Resolved`]
//!    （`Err(Unavailable)` 或 `Ok(是否耗尽)` 加上要做的副作用）；
//! 4. 服务层按 [`Resolved::note_period`] 调 [`AllowanceCore::note_period`] 并把返回的文本写回库，
//!    [`Resolved::low`] 为真（额度偏低）时写一行日志。
//!
//! 不在这里的 IO（服务层负责）：
//! - `read_alert_threshold(db)`：`db.get_setting("v5_alert_threshold", 20)`，取到的值交给 [`normalize_alert_threshold`]；
//! - `_note_period` 的读写：第一次用时 `db.get_setting("v5_period_samples", "{}")` 交给 [`AllowanceCore::load_periods`]，
//!   之后每次 `db.set_setting("v5_period_samples", <note_period 返回的文本>)`，写失败只忽略；
//! - HTTP 请求与响应体读取本身；按 token 串行的锁；日志。
//!
//! 已知边界（只在库里数据被手改坏、或上游返回非 UTF-8 JSON 时才碰到）：
//! - `v5_period_samples` 不是 `{token: [[ts, 秒], ...]}` 的形状时，不合法的部分当作没有；
//! - 订阅响应只按 UTF-8 解析；

use crate::primitives;
use serde_json::{Map, Value};
use std::collections::BTreeMap;

pub const SETTING: &str = "v5_alert_threshold";
pub const DEFAULT_THRESHOLD: i64 = 20;
/// 每个上游账号最近 48 小时「再恢复 1% 还要多少秒」的读数
pub const PERIOD_KEY: &str = "v5_period_samples";
pub const PERIOD_WINDOW: i64 = 48 * 3600;
/// 每个账号最多留这么多条读数
pub const PERIOD_KEEP: usize = 500;
pub const UNAVAILABLE_MSG: &str =
    "暂时无法查询 V5 剩余免费额度，请求未发出，未扣费。请稍后重试或使用 V4.5。";
pub const ERROR_MSG: &str = "额度查询失败，当前状态未确认";
/// 订阅响应体上限
pub const MAX_BODY: usize = 65536;

/// `read_alert_threshold` 的判定：库里是 TEXT，`"20"` 这样的 1～3 位 ASCII 数字串转成整数；
/// 取值必须是 1～100 的整数（bool 不算），否则回到默认值 20。
/// 没有这一行时用的就是默认值（整数 20），传 `Value::Null` 也得到 20。
pub fn normalize_alert_threshold(value: &Value) -> i64 {
    let v = match value {
        Value::String(s)
            if s.is_ascii()
                && !s.is_empty()
                && s.bytes().all(|b| b.is_ascii_digit())
                && s.len() <= 3 =>
        {
            s.parse::<i64>().ok()
        }
        Value::Number(n) if !n.is_f64() => n.as_i64(),
        _ => None,
    };
    match v {
        Some(i) if (1..=100).contains(&i) => i,
        _ => DEFAULT_THRESHOLD,
    }
}

/// 缓存里一个上游账号的最近一次读数（没读过的账号 = `None` / 0）。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Row {
    pub percent: Option<i64>,
    pub is_negative: Option<bool>,
    /// 墙钟（`time.time()`）
    pub checked_at: Option<f64>,
    /// 单调时钟（`time.monotonic()`），缓存新鲜度按它算
    pub at: Option<f64>,
    pub error: Option<String>,
    /// 单调时钟：在这之前不再请求
    pub retry_at: f64,
    pub next_percent_seconds: Option<i64>,
    /// 账号 Anlas 余额（订阅里的 fixed + purchased）；读不懂为 None
    pub anlas: Option<i64>,
    pub fixed_anlas: Option<i64>,
    pub purchased_anlas: Option<i64>,
    pub active: Option<bool>,
    pub tier: Option<i64>,
    pub expires_at: Option<i64>,
    pub is_grace_period: Option<bool>,
    pub account_type: Option<i64>,
    pub purchased_percent: Option<i64>,
}

/// 每个账号最近 48 小时的「再恢复 1% 还要多少秒」读数，保持账号的插入顺序（回写库时同序）。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Periods {
    pub tokens: Vec<(String, Vec<(i64, i64)>)>,
}

impl Periods {
    /// 解析 `v5_period_samples`；读不懂就是空。
    pub fn parse(raw: Option<&str>) -> Self {
        let text = match raw {
            Some(s) if !s.is_empty() => s,
            _ => "{}",
        };
        let Ok(Value::Object(m)) = serde_json::from_str::<Value>(text) else {
            return Self::default();
        };
        let tokens = m
            .into_iter()
            .map(|(k, v)| {
                let rows = v
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|r| {
                                let r = r.as_array()?;
                                Some((r.first()?.as_i64()?, r.get(1)?.as_i64()?))
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                (k, rows)
            })
            .collect();
        Self { tokens }
    }

    fn rows(&self, token_id: &str) -> &[(i64, i64)] {
        self.tokens
            .iter()
            .find(|(k, _)| k == token_id)
            .map_or(&[], |(_, r)| r.as_slice())
    }

    /// `json.dumps(self._periods, ensure_ascii=False)`（`", "` / `": "` 分隔）。
    pub fn to_json(&self) -> String {
        let obj: Map<String, Value> = self
            .tokens
            .iter()
            .map(|(k, rows)| {
                let arr = rows
                    .iter()
                    .map(|(t, s)| Value::Array(vec![Value::from(*t), Value::from(*s)]))
                    .collect();
                (k.clone(), Value::Array(arr))
            })
            .collect();
        crate::json::dumps(&Value::Object(obj))
    }
}

/// 订阅接口请求的结果，由服务层装好交给 [`AllowanceCore::finish_resolve`]。
#[derive(Debug, Clone)]
pub enum Fetched<'a> {
    /// 没拿到响应：连接失败、协议错误、8 秒总超时
    Failed,
    /// 拿到了状态码。`retry_after` 是响应头 `retry-after` 原值（没有 = ""）；
    /// `body` 是状态码 200 时读到的完整响应体，`None` 表示超过 [`MAX_BODY`] 或读取中途出错。
    Response {
        status: u16,
        retry_after: &'a str,
        body: Option<&'a [u8]>,
    },
}

/// 订阅响应里有用的几个字段。
#[derive(Debug, Clone, PartialEq)]
pub struct Subscription {
    pub percent: Option<i64>,
    pub is_negative: Option<bool>,
    /// `timeUntilNextPercent`：0 表示暂停；缺失或非法时为 None。
    pub next_percent_seconds: Option<i64>,
    /// `trainingStepsLeft` 的 fixed + purchased（Anlas 余额）：两项都是 0～10^12 的有限数才算，否则 None。
    /// 只是附带的读数，读不懂不影响订阅解析。
    pub anlas: Option<i64>,
    pub fixed_anlas: Option<i64>,
    pub purchased_anlas: Option<i64>,
    pub active: bool,
    pub tier: i64,
    pub expires_at: Option<i64>,
    pub is_grace_period: Option<bool>,
    pub account_type: Option<i64>,
    pub purchased_percent: Option<i64>,
}

/// 解析订阅运维投影。账号有效性、tier 和 V5 可用性独立保留；0% 不等同于无效 Token。
pub fn parse_subscription(body: &[u8]) -> Option<Subscription> {
    let body = body.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(body);
    let data: Value = serde_json::from_slice(body).ok()?;
    let usage = data.get("usage").filter(|u| u.is_object());
    // type(x) is int：bool 和 float 都不算
    let int_of = |v: Option<&Value>| match v {
        Some(Value::Number(n)) if !n.is_f64() => n
            .as_i64()
            .or_else(|| n.as_u64().map(|u| i64::try_from(u).unwrap_or(i64::MAX))),
        _ => None,
    };
    let active = data.get("active").and_then(Value::as_bool)?;
    let tier = int_of(data.get("tier"))?;
    let percent = usage
        .and_then(|u| int_of(u.get("percent")))
        .filter(|p| *p >= 0);
    let is_negative = usage
        .and_then(|u| u.get("isNegative"))
        .and_then(Value::as_bool);
    let next_percent_seconds = usage
        .and_then(|u| int_of(u.get("timeUntilNextPercent")))
        .filter(|n| (0..10_000_000).contains(n));
    let step = |k: &str| {
        data.get("trainingStepsLeft")?
            .get(k)?
            .as_f64()
            .filter(|f| f.is_finite() && (0.0..=1e12).contains(f))
    };
    let fixed_anlas = step("fixedTrainingStepsLeft").map(|v| v as i64);
    let purchased_anlas = step("purchasedTrainingSteps").map(|v| v as i64);
    let anlas = match (fixed_anlas, purchased_anlas) {
        (Some(a), Some(b)) => a.checked_add(b),
        _ => None,
    };
    let optional_bool = |key: &str| data.get(key).and_then(Value::as_bool);
    Some(Subscription {
        percent,
        is_negative,
        next_percent_seconds,
        anlas,
        fixed_anlas,
        purchased_anlas,
        active,
        tier,
        expires_at: int_of(data.get("expiresAt")),
        is_grace_period: optional_bool("isGracePeriod"),
        account_type: int_of(data.get("accountType")),
        purchased_percent: usage
            .and_then(|u| int_of(u.get("purchasedPercent")))
            .filter(|p| *p >= 0),
    })
}

/// 请求失败后多久不再请求（秒）：默认 60；上游 429 时至少 300，`retry-after` 是 1～6 位十进制数字时取
/// `max(300, min(值, 86400))`。
pub fn retry_seconds(status: Option<u16>, retry_after: &str) -> i64 {
    if status != Some(429) {
        return 60;
    }
    let mut secs = 300;
    if crate::validation::is_decimal(retry_after)
        && primitives::len_chars(retry_after) < 7
        && let Some(v) = primitives::parse_int(retry_after)
    {
        secs = secs.max(v.min(86400));
    }
    secs
}

/// 缓存有效期（秒）：剩 ≤2%（约 30 张）只用 15 秒（≈ 一次出图间隔），低于告警阈值 60 秒，其余 300 秒。
pub fn cache_ttl(percent: i64, threshold: i64) -> i64 {
    if percent <= 2 {
        15
    } else if percent < threshold {
        60
    } else {
        300
    }
}

/// [`AllowanceCore::check_cache`] 的结论。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Begin {
    /// 还在退避期：抛 [`UNAVAILABLE_MSG`]（未发送生图、未扣积分）
    Unavailable,
    /// 缓存足够新且没耗尽：直接返回 `false`
    Cached,
    /// 需要去上游读一次
    Fetch,
}

/// 一次 `resolve` 的结果。
#[derive(Debug, Clone, PartialEq)]
pub struct Resolved {
    /// `Ok(is_negative)`：额度是否已耗尽；`Err(())`：抛 [`UNAVAILABLE_MSG`]
    pub result: Result<bool, ()>,
    /// 读数里带了「再恢复 1% 的秒数」：服务层接着调 [`AllowanceCore::note_period`]
    pub note_period: Option<i64>,
    /// 读数偏低（已耗尽或低于阈值）：服务层写一行日志
    pub low: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("periods 还没从库里读入：先调 load_periods")]
pub struct PeriodsNotLoaded;

#[derive(Debug, Default)]
pub struct AllowanceCore {
    pub rows: BTreeMap<String, Row>,
    /// 第一次用时才从库里读（None = 还没读）
    pub periods: Option<Periods>,
}

impl AllowanceCore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn periods_loaded(&self) -> bool {
        self.periods.is_some()
    }

    /// `_note_period` 里「第一次用时从库里读」的那一步；已经读过就什么都不做。
    pub fn load_periods(&mut self, raw: Option<&str>) {
        if self.periods.is_none() {
            self.periods = Some(Periods::parse(raw));
        }
    }

    /// Historical measured samples of the API's relative time-to-next-1% reading.
    /// 返回要写回 `v5_period_samples` 的 JSON 文本。`now` 是墙钟 `time.time()`。
    pub fn note_period(
        &mut self,
        token_id: &str,
        seconds: i64,
        now: f64,
    ) -> Result<String, PeriodsNotLoaded> {
        let periods = self.periods.as_mut().ok_or(PeriodsNotLoaded)?;
        let mut rows: Vec<(i64, i64)> = periods
            .rows(token_id)
            .iter()
            .filter(|(ts, _)| now - (*ts as f64) < PERIOD_WINDOW as f64)
            .copied()
            .collect();
        rows.push((primitives::round0(now), seconds));
        let start = rows.len().saturating_sub(PERIOD_KEEP);
        let rows = rows[start..].to_vec();
        match periods.tokens.iter_mut().find(|(k, _)| k == token_id) {
            Some((_, r)) => *r = rows,
            None => periods.tokens.push((token_id.to_string(), rows)),
        }
        Ok(periods.to_json())
    }

    /// Historical measured maximum of observed relative time-to-next-1% values; not an API field.
    pub fn period(&self, token_id: &str, now: f64) -> Option<i64> {
        self.periods
            .as_ref()?
            .rows(token_id)
            .iter()
            .filter(|(ts, _)| now - (*ts as f64) < PERIOD_WINDOW as f64)
            .map(|(_, s)| *s)
            .max()
    }

    /// `resolve` 的开头：退避期检查 + 缓存命中判定。`now_mono` 是 `time.monotonic()`，`threshold` 是
    /// [`normalize_alert_threshold`] 的结果。
    pub fn check_cache(&self, token_id: &str, now_mono: f64, threshold: i64) -> Begin {
        let default = Row::default();
        let row = self.rows.get(token_id).unwrap_or(&default);
        if now_mono < row.retry_at {
            return Begin::Unavailable;
        }
        let pct = row.percent.unwrap_or(0);
        let ttl = cache_ttl(pct, threshold);
        // 不拿旧的「已耗尽」结果去把新的一张图切成付费
        if row.error.as_deref().is_none_or(str::is_empty)
            && row.is_negative == Some(false)
            && row.percent.unwrap_or(0) > 0
            && now_mono - row.at.unwrap_or(-1e9) < ttl as f64
        {
            return Begin::Cached;
        }
        Begin::Fetch
    }

    /// `resolve` 的结尾：根据请求结果更新缓存。`wall` / `mono_after` 是请求结束后的 `time.time()` /
    /// `time.monotonic()`；`threshold` 与 [`check_cache`](Self::check_cache) 用的是同一个值。
    pub fn finish_resolve(
        &mut self,
        token_id: &str,
        fetched: &Fetched<'_>,
        wall: f64,
        mono_after: f64,
        threshold: i64,
    ) -> Resolved {
        let (status, retry_after) = match fetched {
            Fetched::Failed => (None, ""),
            Fetched::Response {
                status,
                retry_after,
                ..
            } => (Some(*status), *retry_after),
        };
        let ok = match fetched {
            Fetched::Response {
                status: 200,
                body: Some(body),
                ..
            } => parse_subscription(body),
            _ => None,
        };
        let Some(sub) = ok else {
            let mut row = self.rows.get(token_id).cloned().unwrap_or_default();
            row.error = Some(ERROR_MSG.to_string());
            row.retry_at = mono_after + retry_seconds(status, retry_after) as f64;
            self.rows.insert(token_id.to_string(), row);
            return Resolved {
                result: Err(()),
                note_period: None,
                low: false,
            };
        };
        let eligible =
            sub.active && sub.tier == 3 && sub.percent.is_some() && sub.is_negative.is_some();
        let denial = if !sub.active {
            Some("上游订阅 inactive")
        } else if sub.tier != 3 {
            Some("上游订阅非 Opus tier")
        } else if sub.percent.is_none() || sub.is_negative.is_none() {
            Some("上游 V5 额度字段未知")
        } else {
            None
        };
        self.rows.insert(
            token_id.to_string(),
            Row {
                percent: sub.percent,
                is_negative: sub.is_negative,
                checked_at: Some(wall),
                at: Some(mono_after),
                error: denial.map(str::to_string),
                retry_at: if eligible { 0.0 } else { mono_after + 300.0 },
                // Raw API reading: relative time to the next 1% point.
                next_percent_seconds: sub.next_percent_seconds,
                anlas: sub.anlas,
                fixed_anlas: sub.fixed_anlas,
                purchased_anlas: sub.purchased_anlas,
                active: Some(sub.active),
                tier: Some(sub.tier),
                expires_at: sub.expires_at,
                is_grace_period: sub.is_grace_period,
                account_type: sub.account_type,
                purchased_percent: sub.purchased_percent,
            },
        );
        Resolved {
            result: if eligible {
                Ok(sub.is_negative.expect("eligibility requires usage state"))
            } else {
                Err(())
            },
            note_period: sub.next_percent_seconds,
            low: sub.is_negative == Some(true) || sub.percent.is_some_and(|p| p < threshold),
        }
    }

    /// `snapshot(pool)` 里一个账号的那一项。`index` 从 0 起，`now_wall` / `now_mono` 是两种时钟。
    pub fn account_snapshot(
        &self,
        index: usize,
        token_id: &str,
        threshold: i64,
        now_wall: f64,
        now_mono: f64,
    ) -> Value {
        let default = Row::default();
        let row = self.rows.get(token_id).unwrap_or(&default);
        let stale = now_mono - row.at.unwrap_or(-1e9) >= 300.0;
        let low = row.is_negative == Some(true) || row.percent.is_some_and(|p| p < threshold);
        // Historical samples are separate from the raw current API value (including zero).
        let nps = self
            .period(token_id, now_wall)
            .filter(|n| *n != 0)
            .or(row.next_percent_seconds.filter(|n| *n != 0));
        let error = row.error.clone().filter(|e| !e.is_empty());
        let mut out = Map::new();
        out.insert("account".into(), Value::from(index + 1));
        out.insert(
            "percent".into(),
            row.percent.map_or(Value::Null, Value::from),
        );
        out.insert(
            "time_until_next_percent_seconds".into(),
            row.next_percent_seconds.map_or(Value::Null, Value::from),
        );
        out.insert(
            "current_response_recharge_per_day_estimate".into(),
            row.next_percent_seconds
                .filter(|n| *n > 0)
                .map_or(Value::Null, |n| {
                    crate::values::float(primitives::round(86400.0 / n as f64, 1))
                }),
        );
        out.insert(
            "historical_sample_based_recharge_per_day_estimate".into(),
            nps.map_or(Value::Null, |n| {
                crate::values::float(primitives::round(86400.0 / n as f64, 1))
            }),
        );
        // Backward-compatible capacity/job consumer; explicitly an estimate derived from observed timings.
        out.insert(
            "recharge_per_day".into(),
            nps.map_or(Value::Null, |n| {
                crate::values::float(primitives::round(86400.0 / n as f64, 1))
            }),
        );
        out.insert("active".into(), row.active.map_or(Value::Null, Value::Bool));
        out.insert("tier".into(), row.tier.map_or(Value::Null, Value::from));
        out.insert(
            "expires_at".into(),
            row.expires_at.map_or(Value::Null, Value::from),
        );
        out.insert(
            "is_grace_period".into(),
            row.is_grace_period.map_or(Value::Null, Value::Bool),
        );
        out.insert(
            "account_type".into(),
            row.account_type.map_or(Value::Null, Value::from),
        );
        out.insert(
            "purchased_percent".into(),
            row.purchased_percent.map_or(Value::Null, Value::from),
        );
        out.insert(
            "fixed_anlas".into(),
            row.fixed_anlas.map_or(Value::Null, Value::from),
        );
        out.insert(
            "purchased_anlas".into(),
            row.purchased_anlas.map_or(Value::Null, Value::from),
        );
        out.insert(
            "is_negative".into(),
            row.is_negative.map_or(Value::Null, Value::Bool),
        );
        out.insert(
            "checked_at".into(),
            row.checked_at.map_or(Value::Null, crate::values::float),
        );
        out.insert("low".into(), Value::Bool(low));
        out.insert("uncertain".into(), Value::Bool(stale || error.is_some()));
        out.insert("error".into(), error.map_or(Value::Null, Value::String));
        out.insert("stale".into(), Value::Bool(stale));
        Value::Object(out)
    }

    /// 这些账号最近一次读到的 Anlas 余额之和；一个都没读到返回 None（快照里写 NULL，而不是 0）。
    pub fn anlas_total<'a>(&self, tokens: impl IntoIterator<Item = &'a str>) -> Option<i64> {
        tokens
            .into_iter()
            .filter_map(|t| self.rows.get(t)?.anlas)
            .reduce(|a, b| a + b)
    }

    /// `snapshot(pool)`：`tokens` 按池里的顺序给出 `token_id`。
    pub fn snapshot<'a>(
        &self,
        tokens: impl IntoIterator<Item = &'a str>,
        threshold: i64,
        now_wall: f64,
        now_mono: f64,
    ) -> Value {
        let accounts: Vec<Value> = tokens
            .into_iter()
            .enumerate()
            .map(|(i, t)| self.account_snapshot(i, t, threshold, now_wall, now_mono))
            .collect();
        let mut out = Map::new();
        out.insert("threshold".into(), Value::from(threshold));
        out.insert("accounts".into(), Value::Array(accounts));
        out.insert("mode".into(), Value::String("on_demand".into()));
        Value::Object(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn anlas_balance_is_read_alongside_the_subscription() {
        let body = |steps: Value| {
            serde_json::to_vec(&json!({"active": true, "tier": 3,
                "usage": {"percent": 40, "isNegative": false},
                "trainingStepsLeft": steps}))
            .unwrap()
        };
        let sub = parse_subscription(&body(
            json!({"fixedTrainingStepsLeft": 9000, "purchasedTrainingSteps": 512}),
        ))
        .unwrap();
        assert_eq!((sub.percent, sub.anlas), (Some(40), Some(9512)));
        // 读不懂余额不影响订阅本身
        let sub = parse_subscription(&body(json!({"fixedTrainingStepsLeft": "x"}))).unwrap();
        assert_eq!(sub.anlas, None);

        let mut core = AllowanceCore::new();
        assert_eq!(core.anlas_total(["a", "b"]), None);
        let ok = body(json!({"fixedTrainingStepsLeft": 100, "purchasedTrainingSteps": 0}));
        let f = Fetched::Response {
            status: 200,
            retry_after: "",
            body: Some(&ok),
        };
        core.finish_resolve("a", &f, 1.0, 1.0, 20);
        assert_eq!(core.anlas_total(["a", "b"]), Some(100));
    }

    #[test]
    fn subscription_keeps_unknowns_and_zero_percent_distinct_from_invalidity() {
        let body = serde_json::to_vec(&json!({
            "active": true, "tier": 3, "expiresAt": 1234,
            "isGracePeriod": false, "accountType": 0,
            "usage": {"percent": 0, "purchasedPercent": 0, "isNegative": false, "timeUntilNextPercent": 6048},
            "trainingStepsLeft": {"fixedTrainingStepsLeft": 1, "purchasedTrainingSteps": 0}
        })).unwrap();
        let sub = parse_subscription(&body).unwrap();
        assert!(sub.active);
        assert_eq!(sub.tier, 3);
        assert_eq!(sub.percent, Some(0));
        assert_eq!(sub.is_negative, Some(false));
        assert_eq!(sub.purchased_percent, Some(0));
        assert_eq!(
            (sub.fixed_anlas, sub.purchased_anlas, sub.anlas),
            (Some(1), Some(0), Some(1))
        );
        let mut core = AllowanceCore::new();
        let fetched = Fetched::Response {
            status: 200,
            retry_after: "",
            body: Some(&body),
        };
        assert_eq!(
            core.finish_resolve("synthetic", &fetched, 10.0, 10.0, 20)
                .result,
            Ok(false)
        );
        let snap = core.account_snapshot(0, "synthetic", 20, 10.0, 10.0);
        assert_eq!(snap["active"], true);
        assert_eq!(snap["percent"], 0);
        assert_eq!(snap["error"], Value::Null);
    }

    #[test]
    fn inactive_and_non_opus_statuses_are_visible_but_not_generation_eligible() {
        for (active, tier) in [(false, 3), (true, 2)] {
            let body = serde_json::to_vec(&json!({"active": active, "tier": tier,
                "usage": {"percent": 0, "isNegative": false}}))
            .unwrap();
            let mut core = AllowanceCore::new();
            let fetched = Fetched::Response {
                status: 200,
                retry_after: "",
                body: Some(&body),
            };
            assert_eq!(
                core.finish_resolve("synthetic", &fetched, 10.0, 10.0, 20)
                    .result,
                Err(())
            );
            let snap = core.account_snapshot(0, "synthetic", 20, 10.0, 10.0);
            assert_eq!(snap["active"], active);
            assert!(snap["error"].is_string());
        }
    }

    #[test]
    fn metadata_survives_missing_usage_and_blocks_generation_as_unknown() {
        let body =
            br#"{"active":false,"tier":1,"expiresAt":123,"isGracePeriod":true,"accountType":0}"#;
        let sub = parse_subscription(body).unwrap();
        assert!(!sub.active);
        assert_eq!(sub.tier, 1);
        assert_eq!(sub.percent, None);
        let mut core = AllowanceCore::new();
        let fetched = Fetched::Response {
            status: 200,
            retry_after: "",
            body: Some(body),
        };
        assert_eq!(
            core.finish_resolve("synthetic", &fetched, 10.0, 20.0, 20)
                .result,
            Err(())
        );
        let snapshot = core.account_snapshot(0, "synthetic", 20, 10.0, 20.0);
        assert_eq!(snapshot["active"], false);
        assert_eq!(snapshot["tier"], 1);
        assert_eq!(snapshot["percent"], Value::Null);
        assert_eq!(core.check_cache("synthetic", 21.0, 20), Begin::Unavailable);
    }

    #[test]
    fn raw_zero_is_preserved_and_invalid_or_unknown_times_do_not_create_estimates() {
        for (raw, expected) in [("0", Some(0)), ("-1", None), ("null", None)] {
            let body = format!(
                r#"{{"active":true,"tier":3,"usage":{{"percent":10,"isNegative":false,"timeUntilNextPercent":{raw}}}}}"#
            );
            let mut core = AllowanceCore::new();
            let fetched = Fetched::Response {
                status: 200,
                retry_after: "",
                body: Some(body.as_bytes()),
            };
            core.finish_resolve("synthetic", &fetched, 10.0, 20.0, 20);
            let snapshot = core.account_snapshot(0, "synthetic", 20, 10.0, 20.0);
            assert_eq!(
                snapshot["time_until_next_percent_seconds"].as_i64(),
                expected
            );
            assert_eq!(
                snapshot["current_response_recharge_per_day_estimate"],
                Value::Null
            );
        }
    }

    #[test]
    fn ineligible_and_failed_queries_back_off_instead_of_fetching_on_every_image() {
        let inactive = br#"{"active":false,"tier":3,"usage":{"percent":50,"isNegative":false}}"#;
        let mut core = AllowanceCore::new();
        let fetched = Fetched::Response {
            status: 200,
            retry_after: "",
            body: Some(inactive),
        };
        core.finish_resolve("inactive", &fetched, 10.0, 20.0, 20);
        assert_eq!(core.check_cache("inactive", 21.0, 20), Begin::Unavailable);
        assert_eq!(core.check_cache("inactive", 321.0, 20), Begin::Fetch);

        let failed = Fetched::Failed;
        core.finish_resolve("failed", &failed, 10.0, 50.0, 20);
        assert_eq!(core.check_cache("failed", 51.0, 20), Begin::Unavailable);
        assert_eq!(core.check_cache("failed", 110.0, 20), Begin::Fetch);
    }

    #[test]
    fn threshold_parsing() {
        assert_eq!(normalize_alert_threshold(&json!("30")), 30);
        assert_eq!(normalize_alert_threshold(&json!("0")), 20);
        assert_eq!(normalize_alert_threshold(&json!("1000")), 20);
        assert_eq!(normalize_alert_threshold(&json!(true)), 20);
        assert_eq!(normalize_alert_threshold(&json!(55)), 55);
    }

    #[test]
    fn period_keeps_longest_reading() {
        let mut c = AllowanceCore::new();
        c.load_periods(None);
        for (i, s) in [16000, 7800, 1200].into_iter().enumerate() {
            c.note_period("t1", s, 1_790_000_000.0 + i as f64).unwrap();
        }
        assert_eq!(c.period("t1", 1_790_000_100.0), Some(16000));
        assert_eq!(c.period("zz", 1_790_000_100.0), None);
        // 48 小时后全部过期
        assert_eq!(c.period("t1", 1_790_000_000.0 + 48.0 * 3600.0 + 10.0), None);
    }
}
