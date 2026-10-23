//! 状态快照（旧版 `status_page.snapshot`）：每个组件 90 天的色条、可用率、当前状态，以及事件列表。
//!
//! 「天」按网关的时区（常量 Asia/Shanghai，见 `gate_core::clock::TZ`）划分，与旧版 `datetime.fromtimestamp` 的本地时间一致。
//! 数值运算的顺序照抄旧版（浮点结果要逐位相同，`/status.json` 里的 `uptime` 会原样输出）。

use gate_core::clock::Clock;
use gate_db::repo::status_page::{Incident, UpstreamRecent};
use jiff::ToSpan;
use jiff::civil::Date;
use serde_json::Value;
use std::collections::HashMap;

/// 事件可以标的组件（老事件里有 web / bot）。
pub const COMPONENTS: [(&str, &str); 4] = [
    ("upstream", "NovelAI 上游（生图）"),
    ("api", "出图接口（gate.davidzhao.top）"),
    ("web", "官网与后台"),
    ("bot", "Discord 机器人（奶妹）"),
];
/// 状态页只展示这两个组件；只影响官网 / Discord 的事件不上状态页。
pub const SHOWN: [&str; 2] = ["upstream", "api"];
/// 维护循环每 5 分钟从 NovelAI 官方状态页取一次，存在这个设置里。
pub const OFFICIAL_KEY: &str = "novelai_official_status";
/// usage_log 第一条记录的日期：更早的日子是灰色「无数据」，不编造。
pub const TRACKING_START: &str = "2026-10-09";
pub const DAYS: usize = 90;
pub const HISTORY_DAYS: usize = 15;
/// 官方状态超过这么久没取到新的，就不再参考。
const OFFICIAL_MAX_AGE: f64 = 3600.0;

/// Statuspage 的组件状态措辞，下标 = 级别（0 正常 … 3 严重）。
pub const IMPACT_COMPONENT: [&str; 4] = ["运行正常", "性能下降", "部分中断", "严重中断"];

/// `IMPACT_RANK.get(impact, 1)`：未知的严重度按 minor 算。
pub fn impact_rank(impact: &str) -> usize {
    match impact {
        "none" => 0,
        "minor" => 1,
        "major" => 2,
        "critical" => 3,
        _ => 1,
    }
}

/// 事件是否标了某个组件（`key in inc["components"]`）。
pub fn has_component(inc: &Incident, key: &str) -> bool {
    inc.components
        .as_array()
        .is_some_and(|a| a.iter().any(|c| c.as_str() == Some(key)))
}

#[derive(Debug, Clone, PartialEq)]
pub struct Day {
    /// `YYYY-MM-DD`
    pub day: String,
    /// None = 开始记录之前，无数据。
    pub level: Option<usize>,
    pub titles: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct Component {
    pub key: &'static str,
    pub name: &'static str,
    pub days: Vec<Day>,
    pub uptime: Option<f64>,
    pub status: &'static str,
    pub level: usize,
}

#[derive(Debug, Clone)]
pub struct Snapshot {
    pub now: f64,
    pub components: Vec<Component>,
    /// 只含标了展示组件的事件，开始时间新的在前。
    pub incidents: Vec<Incident>,
    /// `incidents` 里未解决的下标。
    pub active: Vec<usize>,
}

/// 组装快照需要的、已经从库里读出来的原始数据。
pub struct Inputs {
    pub incidents: Vec<Incident>,
    /// 本地日 → 出图接口当天 5xx 占比（`status_hourly`）。
    pub api_error_days: HashMap<String, f64>,
    /// 本地日 → 当天上游失败占比（`usage_log`）。
    pub upstream_days: HashMap<String, f64>,
    pub upstream_recent: UpstreamRecent,
    /// NovelAI 官方状态里「Image Generation」的状态码（没取到 / 过期为 None）。
    pub official_image_code: Option<f64>,
}

/// 占比 → 级别：> 10% 部分中断，> 1% 性能下降。
pub fn level_of(ratio: f64) -> usize {
    if ratio > 0.10 {
        2
    } else if ratio > 0.01 {
        1
    } else {
        0
    }
}

/// 「现在」（最近一小时）上游的失败占比，决定当前状态。
///
/// 旧版：不到 10 次请求不下结论，否则 `失败 / 总数`。3.0 追加（站长批准）：
/// 失败至少 3 次且来自至少 2 把不同的 Key 才算——一个成员自己的畸形请求（被 NovelAI 返回 500）
/// 不该让全站的上游显示「性能下降」。每天的色条（[`level_of`] 按日占比）不受影响。
pub fn recent_ratio(r: UpstreamRecent) -> f64 {
    if r.total >= 10 && r.bad >= 3 && r.bad_keys >= 2 {
        r.bad as f64 / r.total as f64
    } else {
        0.0
    }
}

/// 本地日期（`datetime.fromtimestamp(ts).date()`）。
pub fn local_date(clock: &Clock, ts: f64) -> Date {
    clock
        .format(ts, "%Y-%m-%d")
        .parse()
        .unwrap_or(Date::constant(1970, 1, 1))
}

/// 某天本地 0 点的时间戳（`datetime.combine(d, time.min).timestamp()`）。
pub fn midnight(clock: &Clock, d: Date) -> f64 {
    clock.local_midnight(&d.to_string()).unwrap_or(0.0)
}

pub fn days_before(d: Date, n: usize) -> Date {
    d.checked_sub((n as i64).days()).unwrap_or(d)
}

pub fn days_after(d: Date, n: usize) -> Date {
    d.checked_add((n as i64).days()).unwrap_or(d)
}

/// 官方状态：`{"image_code": ..., "fetched_at": ...}` 的设置串 → 仍然有效时的图片生成状态码。
/// 解析失败 / 不是对象 / 超过 1 小时没更新 / 没有这个字段 → None。
pub fn official_image_code(raw: Option<&str>, now: f64) -> Option<f64> {
    let v: Value = serde_json::from_str(raw.filter(|s| !s.is_empty()).unwrap_or("{}")).ok()?;
    let obj = v.as_object()?;
    let fetched = match obj.get("fetched_at") {
        Some(Value::Number(n)) => n.as_f64().unwrap_or(0.0),
        Some(Value::String(s)) => s.trim().parse().unwrap_or(0.0),
        _ => 0.0,
    };
    if now - fetched > OFFICIAL_MAX_AGE {
        return None;
    }
    obj.get("image_code").and_then(Value::as_f64)
}

/// 出图接口每天的 5xx 占比（`_api_error_days`）：`hour` 是整点的秒数。
pub fn api_error_days(clock: &Clock, rows: &[(i64, i64, i64)]) -> HashMap<String, f64> {
    let mut by_day: HashMap<String, (i64, i64)> = HashMap::new();
    for &(hour, code, n) in rows {
        let tot = by_day
            .entry(clock.format(hour as f64, "%Y-%m-%d"))
            .or_default();
        tot.0 += n;
        if code >= 500 {
            tot.1 += n;
        }
    }
    by_day
        .into_iter()
        .map(|(d, (total, bad))| {
            (
                d,
                if total != 0 {
                    bad as f64 / total as f64
                } else {
                    0.0
                },
            )
        })
        .collect()
}

/// NovelAI 每天真实返回的失败占比（`_upstream_days`）：15 分钟一桶的统计按本地日归并。
pub fn upstream_days(clock: &Clock, buckets: &[(i64, i64, i64)]) -> HashMap<String, f64> {
    let mut by_day: HashMap<String, (i64, i64)> = HashMap::new();
    for &(start, total, bad) in buckets {
        let t = by_day
            .entry(clock.format(start as f64, "%Y-%m-%d"))
            .or_default();
        t.0 += total;
        t.1 += bad;
    }
    by_day
        .into_iter()
        .filter(|(_, (total, _))| *total != 0)
        .map(|(d, (total, bad))| (d, bad as f64 / total as f64))
        .collect()
}

/// 旧版 `snapshot` 的主体（纯函数，数据已读好）。
pub fn build(clock: &Clock, now: f64, inputs: Inputs) -> Snapshot {
    let today = local_date(clock, now);
    let first = days_before(today, DAYS - 1);
    let incidents: Vec<Incident> = inputs
        .incidents
        .into_iter()
        .filter(|i| SHOWN.iter().any(|c| has_component(i, c)))
        .collect();
    let up_now = recent_ratio(inputs.upstream_recent);

    let mut comps = Vec::new();
    for (key, name) in COMPONENTS.iter().filter(|c| SHOWN.contains(&c.0)) {
        let (mut days, mut down_minutes, mut tracked_minutes) = (Vec::new(), 0.0_f64, 0.0_f64);
        for i in 0..DAYS {
            let d = days_after(first, i);
            let ds = d.to_string();
            let d0 = midnight(clock, d);
            let d1 = (d0 + 86400.0).min(now);
            if ds.as_str() < TRACKING_START {
                days.push(Day {
                    day: ds,
                    level: None,
                    titles: Vec::new(),
                });
                continue;
            }
            let (mut level, mut titles) = (0usize, Vec::new());
            for inc in &incidents {
                if !has_component(inc, key) {
                    continue;
                }
                // `inc["resolved_at"] or now`：0.0 也当作没有
                let s = inc.started_at;
                let e = inc.resolved_at.filter(|v| *v != 0.0).unwrap_or(now);
                if s < d1 && e > d0 {
                    let rank = impact_rank(&inc.impact);
                    level = level.max(rank);
                    titles.push(inc.title.clone());
                    if rank >= 2 {
                        down_minutes += (e.min(d1) - s.max(d0)) / 60.0;
                    }
                }
            }
            if *key == "api" && inputs.api_error_days.get(&ds).copied().unwrap_or(0.0) > 0.01 {
                level = level.max(1);
            }
            if *key == "upstream"
                && let Some(ratio) = inputs.upstream_days.get(&ds)
            {
                level = level.max(level_of(*ratio));
            }
            tracked_minutes += (d1 - d0).max(0.0) / 60.0;
            days.push(Day {
                day: ds,
                level: Some(level),
                titles,
            });
        }
        let mut now_level = incidents
            .iter()
            .filter(|i| i.resolved_at.is_none() && has_component(i, key))
            .map(|i| impact_rank(&i.impact))
            .max()
            .unwrap_or(0);
        if *key == "upstream" {
            now_level = now_level.max(level_of(up_now));
            // 官方自己标了故障
            if inputs.official_image_code.unwrap_or(100.0) >= 300.0 {
                now_level = now_level.max(1);
            }
        }
        let uptime =
            (tracked_minutes != 0.0).then(|| 100.0 * (1.0 - down_minutes / tracked_minutes));
        comps.push(Component {
            key,
            name,
            days,
            uptime,
            status: IMPACT_COMPONENT[now_level],
            level: now_level,
        });
    }
    let active = incidents
        .iter()
        .enumerate()
        .filter(|(_, i)| i.resolved_at.is_none())
        .map(|(n, _)| n)
        .collect();
    Snapshot {
        now,
        components: comps,
        incidents,
        active,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gate_db::repo::status_page::IncidentUpdate;
    use serde_json::json;

    fn clock() -> Clock {
        Clock::new("Asia/Shanghai", 7)
    }

    // 2026-10-14 12:00:00 +08:00
    const NOW: f64 = 1_791_950_400.0;

    fn incident(
        id: i64,
        impact: &str,
        comps: Value,
        started: f64,
        resolved: Option<f64>,
    ) -> Incident {
        Incident {
            id,
            title: format!("合成事件 {id}"),
            impact: impact.into(),
            components: comps,
            started_at: started,
            resolved_at: resolved,
            updates: vec![IncidentUpdate {
                status: "investigating".into(),
                body: "我们正在调查此问题。".into(),
                at: started,
            }],
        }
    }

    fn inputs(incidents: Vec<Incident>, recent: UpstreamRecent) -> Inputs {
        Inputs {
            incidents,
            api_error_days: HashMap::new(),
            upstream_days: HashMap::new(),
            upstream_recent: recent,
            official_image_code: None,
        }
    }

    fn upstream(s: &Snapshot) -> &Component {
        s.components.iter().find(|c| c.key == "upstream").unwrap()
    }

    #[test]
    fn recent_failures_need_three_requests_from_two_keys() {
        let r = |total, bad, bad_keys| {
            recent_ratio(UpstreamRecent {
                total,
                bad,
                bad_keys,
            })
        };
        // 不到 10 次不下结论（与旧版一致）
        assert_eq!(r(9, 9, 5), 0.0);
        // 同一把 Key 反复失败：不算
        assert_eq!(r(105, 5, 1), 0.0);
        // 失败不到 3 次：不算
        assert_eq!(r(100, 2, 2), 0.0);
        // 两把 Key、3 次：按占比
        assert_eq!(r(103, 3, 2), 3.0 / 103.0);
        assert_eq!(level_of(r(103, 3, 2)), 1);
        assert_eq!(level_of(r(20, 10, 3)), 2);
    }

    #[test]
    fn single_key_failures_keep_upstream_healthy() {
        let one_key = UpstreamRecent {
            total: 105,
            bad: 5,
            bad_keys: 1,
        };
        let s = build(&clock(), NOW, inputs(vec![], one_key));
        assert_eq!(upstream(&s).status, "运行正常");
        let two_keys = UpstreamRecent {
            total: 105,
            bad: 5,
            bad_keys: 2,
        };
        let s = build(&clock(), NOW, inputs(vec![], two_keys));
        assert_eq!(upstream(&s).status, "性能下降");
    }

    #[test]
    fn official_and_incidents_still_apply() {
        let mut i = inputs(vec![], UpstreamRecent::default());
        i.official_image_code = Some(300.0);
        assert_eq!(upstream(&build(&clock(), NOW, i)).level, 1);
        let live = incident(1, "major", json!(["upstream"]), NOW - 600.0, None);
        let s = build(&clock(), NOW, inputs(vec![live], UpstreamRecent::default()));
        assert_eq!(upstream(&s).level, 2);
        assert_eq!(s.active, vec![0]);
    }

    #[test]
    fn web_only_incidents_are_hidden() {
        let web = incident(1, "major", json!(["web", "bot"]), NOW - 600.0, None);
        let api = incident(2, "minor", json!(["api"]), NOW - 300.0, None);
        let s = build(
            &clock(),
            NOW,
            inputs(vec![web, api], UpstreamRecent::default()),
        );
        assert_eq!(s.incidents.len(), 1);
        assert_eq!(s.incidents[0].id, 2);
        assert_eq!(
            s.components.iter().map(|c| c.key).collect::<Vec<_>>(),
            ["upstream", "api"]
        );
    }

    #[test]
    fn days_before_tracking_start_have_no_data() {
        let s = build(&clock(), NOW, inputs(vec![], UpstreamRecent::default()));
        let api = &s.components[1];
        assert_eq!(api.days.len(), DAYS);
        assert_eq!(api.days[0].level, None);
        assert_eq!(api.days[DAYS - 1].day, "2026-10-14");
        assert_eq!(api.days[DAYS - 1].level, Some(0));
        // 开始记录当天是第一个有数据的日子：2026-10-09
        let first = api.days.iter().find(|d| d.level.is_some()).unwrap();
        assert_eq!(first.day, TRACKING_START);
        assert!(api.uptime.is_some());
    }

    #[test]
    fn official_status_parsing() {
        let fresh = format!(r#"{{"image_code": 400, "fetched_at": {}}}"#, NOW - 60.0);
        assert_eq!(official_image_code(Some(&fresh), NOW), Some(400.0));
        let stale = format!(r#"{{"image_code": 400, "fetched_at": {}}}"#, NOW - 7200.0);
        assert_eq!(official_image_code(Some(&stale), NOW), None);
        assert_eq!(official_image_code(Some("not json"), NOW), None);
        assert_eq!(official_image_code(Some("[1]"), NOW), None);
        assert_eq!(official_image_code(None, NOW), None);
        // 没有 fetched_at 按 0 算 = 早就过期
        assert_eq!(
            official_image_code(Some(r#"{"image_code": 400}"#), NOW),
            None
        );
    }
}
