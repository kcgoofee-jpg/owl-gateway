//! `/status` 页面：和首页同一套顶栏、设计令牌、字体和卡片（2026-10 起不再照搬 Statuspage 模板）。
//!
//! - 样式是 `web/src/ui/tokens.css` + `base.css` + `chrome.css` + `status-page.css` 拼成的一份同源样式表
//!   （见 `routes::status`），页面上没有 style 属性（CSP 是 `style-src 'self'`）；
//! - 90 天色条只画「有数据」的日子（从 `TRACKING_START` 起）：更早的日子不画一排灰条（看起来像故障）；
//!   颜色走类名，按天从左往右长出来的动画在 `status-page.js` 里（系统开了「减少动态效果」时不播）；
//! - 过去的事件按天分组，同一天里标题相同、严重度不高的自动维护事件合成一条（显示次数与起止）；
//!   最近 7 天默认展开，更早的收在 `<details>` 里（原生折叠，没有 JS 也能看）；
//! - 页面不用 `.reveal`（滚动显影带 `filter: blur(10px)` + `opacity: .4`，会让事件文字进入视口前不可读，
//!   也会把 axe 的对比度算成 1.7–2.5:1）；
//! - 文案（横幅、组件状态、事件进展）和 Statuspage 的标准说法一致，时间统一写「（北京时间）」。

use super::feed::{status_label, worst_active_impact};
use super::snapshot::{
    Day, HISTORY_DAYS, IMPACT_COMPONENT, RECENT_DAYS, Snapshot, days_before, impact_rank,
    local_date, midnight,
};
use gate_core::clock::Clock;
use gate_db::repo::status_page::{Incident, IncidentUpdate};
use jiff::civil::Date;
use maud::{DOCTYPE, Markup, PreEscaped, html};

/// 色条的类名（颜色在样式表里）：无数据 / 正常 / 性能下降 / 部分中断 / 严重中断。
fn bar_class(level: Option<usize>) -> &'static str {
    match level {
        None => "lv-n",
        Some(0) => "lv-0",
        Some(1) => "lv-1",
        Some(2) => "lv-2",
        _ => "lv-3",
    }
}

/// 严重度 → 颜色档：0 正常（ok）、1 轻微（warn）、2+ 严重（danger）。
fn tone_of(level: usize) -> &'static str {
    match level {
        0 => "ok",
        1 => "warn",
        _ => "danger",
    }
}

/// 时间戳拆成 (年, 月, 日, 时, 分)。`datetime` 只认公元 1–9999 年，超出范围返回 None。
fn parts(clock: &Clock, ts: f64) -> Option<(i64, i64, i64, i64, i64)> {
    if !ts.is_finite() || !(-62_135_596_800.0..=253_402_300_799.0).contains(&ts) {
        return None;
    }
    let p: Vec<i64> = clock
        .format(ts, "%Y|%m|%d|%H|%M")
        .split('|')
        .filter_map(|p| p.parse().ok())
        .collect();
    let [y, mo, d, h, mi] = p[..] else {
        return None;
    };
    Some((y, mo, d, h, mi))
}

/// `10月22日 22:21（北京时间）`；带年时 `2026年10月22日 22:21（北京时间）`。时间无效时写「时间无效」。
fn stamp(clock: &Clock, ts: f64, with_year: bool) -> String {
    let Some((y, mo, d, h, mi)) = parts(clock, ts) else {
        return "时间无效".into();
    };
    format!(
        "{}{mo}月{d}日 {h:02}:{mi:02}（北京时间）",
        if with_year {
            format!("{y}年")
        } else {
            String::new()
        }
    )
}

/// 只要时分（合并事件的起止在同一行里用）。
fn hm(clock: &Clock, ts: f64) -> String {
    parts(clock, ts).map_or_else(
        || "时间无效".into(),
        |(_, _, _, h, mi)| format!("{h:02}:{mi:02}"),
    )
}

/// `YYYY-MM-DD` → `10月9日`（色条标注 / 事件按天分组用小标题时用）。
fn short_day(day: &str) -> String {
    let mut it = day.split('-');
    let (Some(y), Some(mo), Some(d)) = (it.next(), it.next(), it.next()) else {
        return day.to_string();
    };
    let n = |s: &str| s.trim_start_matches('0').parse::<u32>().ok();
    match (n(mo), n(d)) {
        (Some(mo), Some(d)) => format!("{mo}月{d}日"),
        _ => format!("{y}年{mo}月{d}日"),
    }
}

/// 一条进展：前面一个小标签标出状态（已解决是绿色，其余灰色）。
fn update_html(clock: &Clock, u: &IncidentUpdate, with_year: bool) -> Markup {
    let pill = if u.status == "resolved" {
        "pill pill--ok"
    } else {
        "pill"
    };
    html! {
        li {
            span class=(pill) { (status_label(&u.status)) }
            p class="sp-body" { (u.body) }
            small { (stamp(clock, u.at, with_year)) }
        }
    }
}

/// 事件标题的颜色：none 正文色、minor 警告色、其余（major / critical / 不认识的）危险色。
fn impact_class(impact: &str) -> &'static str {
    match impact {
        "none" => "sp-inc-title",
        "minor" => "sp-inc-title warn",
        _ => "sp-inc-title danger",
    }
}

fn bars_svg(days: &[Day]) -> Markup {
    let range = range_label(days);
    // 触屏没有 hover：色条的每一天都留 `<title>`（长按/读屏可读），并给整条一个文字范围与图例。
    let aria = format!(
        "{range} 的运行状态色条（共 {} 天）：绿色正常、黄色性能下降、橙色部分中断、红色严重中断；\
         长按或聚焦某一天可以看当天的记录。",
        days.len()
    );
    html! {
        svg preserveAspectRatio="none" height="34"
            viewBox=(format!("0 0 {} 34", days.len() * 5 - 2)) {
            // 读屏先读到整条色条的说明；每一天的 <rect><title> 还能逐个聚焦查看
            title { (aria) }
            @for (i, d) in days.iter().enumerate() {
                @let tip = match d.level {
                    None => format!("{}：无数据", short_day(&d.day)),
                    Some(0) if d.titles.is_empty() => format!("{}：没有记录的停机", short_day(&d.day)),
                    // 没有事件、但按上游失败率自动算出了级别
                    Some(_) if d.titles.is_empty() => format!("{}：上游失败率偏高", short_day(&d.day)),
                    Some(_) => format!("{}：{}", short_day(&d.day), d.titles.join("；")),
                };
                rect class=(bar_class(d.level)) height="34" width="3" x=(i * 5) y="0" tabindex="0" { title { (tip) } }
            }
        }
    }
}

/// 色条的可读日期范围（数据从第一个有数据的日子画起，不出现「75 天灰条」）。
fn range_label(days: &[Day]) -> String {
    let (Some(first), Some(last)) = (days.first(), days.last()) else {
        return "没有数据".into();
    };
    format!("{} – {}", short_day(&first.day), short_day(&last.day))
}

/// 一天里的一条（合并后）事件；`incidents` 新的在前。
struct Entry<'a> {
    incidents: Vec<&'a Incident>,
}

impl<'a> Entry<'a> {
    /// 最新的一次（标题、id、最新进展都以它为准）。
    fn lead(&self) -> &'a Incident {
        self.incidents[0]
    }
    fn count(&self) -> usize {
        self.incidents.len()
    }
    fn merged(&self) -> bool {
        self.incidents.len() > 1
    }
    fn worst_rank(&self) -> usize {
        self.incidents
            .iter()
            .map(|i| impact_rank(&i.impact))
            .max()
            .unwrap_or(0)
    }
}

/// 同一天里，同标题、严重度不高（none / minor）的事件合成一条：
/// 出图接口每次部署/抖动都记一条「暂时无法访问」，不合并的话一页 42 条、9,461px。
/// major / critical 是人工发布的重大事件，永远单独显示（不与自动事件合并）。
fn day_entries<'a>(incs: &[&'a Incident]) -> Vec<Entry<'a>> {
    let mut out: Vec<Entry<'a>> = Vec::new();
    for inc in incs {
        if impact_rank(&inc.impact) >= 2 {
            out.push(Entry {
                incidents: vec![inc],
            });
            continue;
        }
        if let Some(e) = out
            .iter_mut()
            .find(|e| e.worst_rank() < 2 && e.lead().title == inc.title)
        {
            e.incidents.push(inc);
            continue;
        }
        out.push(Entry {
            incidents: vec![inc],
        });
    }
    out
}

/// 合并事件的一行摘要：`共 4 次 · 02:38–02:55（北京时间）`；跨天时写出两天的日期。
fn merged_meta(clock: &Clock, e: &Entry, now: f64) -> String {
    let start = e
        .incidents
        .iter()
        .map(|i| i.started_at)
        .fold(f64::INFINITY, f64::min);
    let end = e
        .incidents
        .iter()
        .map(|i| i.resolved_at.filter(|v| *v != 0.0).unwrap_or(now))
        .fold(f64::NEG_INFINITY, f64::max);
    let same_day = clock.format(start, "%Y-%m-%d") == clock.format(end, "%Y-%m-%d");
    let span = if same_day {
        let (a, b) = (hm(clock, start), hm(clock, end));
        if a == b {
            format!("{a}（北京时间）")
        } else {
            format!("{a}–{b}（北京时间）")
        }
    } else {
        let day = |ts: f64| match parts(clock, ts) {
            Some((_, mo, d, _, _)) => format!("{mo}月{d}日"),
            None => "时间无效".into(),
        };
        format!(
            "{} {} – {} {}（北京时间）",
            day(start),
            hm(clock, start),
            day(end),
            hm(clock, end)
        )
    };
    format!("共 {} 次 · {span}", e.count())
}

/// 一张事件卡片。合并组只显示最新一条进展（正文几乎一样，42 条变几条）；
/// 单条事件（含人工重大事件）保留全部进展。
fn entry_html(clock: &Clock, e: &Entry, now: f64, with_year: bool) -> Markup {
    let lead = e.lead();
    let impact = e
        .incidents
        .iter()
        .max_by_key(|i| impact_rank(&i.impact))
        .map_or("none", |i| i.impact.as_str());
    let id = lead.id;
    html! {
        div class="card sp-inc" id=(format!("incident-{id}")) {
            // 合并掉的旧事件保留锚点：`/status#incident-旧id` 的深链接（以及 /status.json 的 id）不会失效
            @for old in &e.incidents[1..] {
                span class="sp-anchor" id=(format!("incident-{}", old.id)) {}
            }
            p class=(impact_class(impact)) {
                a class="sp-inc-link" href=(format!("#incident-{id}")) { (lead.title) }
            }
            @if e.merged() {
                p class="sp-inc-meta" { (merged_meta(clock, e, now)) }
                @if let Some(u) = lead.updates.first() {
                    ol class="sp-updates" { (update_html(clock, u, with_year)) }
                }
            } @else {
                ol class="sp-updates" {
                    @for u in &lead.updates { (update_html(clock, u, with_year)) }
                }
            }
        }
    }
}

fn history_day(clock: &Clock, snap: &Snapshot, day: Date, is_today: bool) -> Markup {
    let d0 = midnight(clock, day);
    let incs: Vec<&Incident> = snap
        .incidents
        .iter()
        .filter(|i| d0 <= i.started_at && i.started_at < d0 + 86400.0)
        .collect();
    let entries = day_entries(&incs);
    html! {
        div class="sp-day" {
            h3 { (day.month()) "月" (day.day()) "日" @if is_today { "（今天）" } }
            @if entries.is_empty() {
                p class="muted" { (if is_today { "今天没有报告事件。" } else { "没有报告事件。" }) }
            }
            @for e in &entries { (entry_html(clock, e, snap.now, false)) }
        }
    }
}

/// lucide 的 Sun / Moon（和首页 lucide-preact 同一份几何），18px；显示哪个由样式表按 data-theme 决定。
const THEME_ICONS: &str = concat!(
    r#"<svg class="ic-sun" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">"#,
    r#"<circle cx="12" cy="12" r="4"/><path d="M12 2v2"/><path d="M12 20v2"/><path d="m4.93 4.93 1.41 1.41"/><path d="m17.66 17.66 1.41 1.41"/>"#,
    r#"<path d="M2 12h2"/><path d="M20 12h2"/><path d="m6.34 17.66-1.41 1.41"/><path d="m19.07 4.93-1.41 1.41"/></svg>"#,
    r#"<svg class="ic-moon" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">"#,
    r#"<path d="M20.985 12.486a9 9 0 1 1-9.473-9.472c.405-.022.617.46.402.803a6 6 0 0 0 8.268 8.268c.344-.215.825-.004.803.401"/></svg>"#,
);

/// 和首页一样的顶栏：Logo（回首页）、主题切换。
fn topbar() -> Markup {
    html! {
        header class="site-top" {
            div class="wrap site-top__in" {
                a class="brand" href="/" {
                    span class="brand__eyes" aria-hidden="true" { i {} i {} }
                    span class="brand__name" { "猫头鹰公益站" }
                }
                div class="site-top__right" {
                    button id="themeBtn" class="icon-btn" type="button"
                        aria-label="切换深色 / 浅色主题" title="切换深色 / 浅色主题" { (PreEscaped(THEME_ICONS)) }
                }
            }
        }
    }
}

/// 页面引用的静态资源地址（带内容哈希，见 `routes::status`）。
pub struct AssetRefs<'a> {
    pub css: &'a str,
    pub js: &'a str,
}

/// `subscribe_url`：优先站点邀请链接（`DISCORD_INVITE_URL`），没有就退回公告频道；
/// 两者都没有时不放按钮（不拿 `/status` 自己当链接）。
pub fn render(
    snap: &Snapshot,
    clock: &Clock,
    subscribe_url: Option<&str>,
    assets: &AssetRefs,
) -> Markup {
    let worst = snap.components.iter().map(|c| c.level).max().unwrap_or(0);
    let active_rank = worst_active_impact(snap).map_or(0, impact_rank);
    let banner_tone = tone_of(worst.max(active_rank));
    let today = local_date(clock, snap.now);
    html! {
        (DOCTYPE)
        html lang="zh-CN" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width,initial-scale=1,viewport-fit=cover";
                meta name="color-scheme" content="light dark";
                title { "猫头鹰公益站 · 服务状态" }
                link rel="icon" href="/favicon.svg" type="image/svg+xml";
                link rel="stylesheet" href=(assets.css);
                script src=(assets.js) {}
            }
            body {
                div class="sky" aria-hidden="true" {}
                (topbar())
                main class="wrap wrap--narrow sp-main" {
                    h1 class="sr-only" { "服务状态" }
                    div class=(format!("card sp-banner tone-{banner_tone}")) {
                        span class="dot" aria-hidden="true" {}
                        strong {
                            @if worst > 0 { "部分系统" (IMPACT_COMPONENT[worst]) } @else { "所有系统运行正常" }
                        }
                        @if let Some(url) = subscribe_url {
                            a class="btn sp-sub" href=(url) rel="noopener" { "加入 Discord 获取更新" }
                        }
                    }
                    @if !snap.active.is_empty() {
                        section class="sp-active" aria-labelledby="active-incidents" {
                            h2 class="sr-only" id="active-incidents" { "进行中的事件" }
                            @for &i in &snap.active {
                                @let inc = &snap.incidents[i];
                                div class="card sp-inc sp-inc--active" {
                                    p class=(impact_class(&inc.impact)) { (inc.title) }
                                    ol class="sp-updates" {
                                        @for u in &inc.updates { (update_html(clock, u, true)) }
                                    }
                                }
                            }
                        }
                    }
                    div class="card sp-comps" {
                        @for comp in &snap.components {
                            div class="sp-comp" {
                                div class="sp-comp-head" {
                                    span class="sp-name" role="heading" aria-level="2" {
                                        // 「出图接口（gate.davidzhao.top）」：括号里的域名单独一段，手机上换到第二行（不截成省略号）
                                        @match comp.name.split_once('（') {
                                            Some((main, host)) => { (main) span class="sp-host" { "（" (host) } }
                                            None => { (comp.name) }
                                        }
                                    }
                                    span class=(format!("sp-state {}", tone_of(comp.level))) { (comp.status) }
                                }
                                div class="sp-bars" { (bars_svg(&comp.days)) }
                                div class="sp-legend" {
                                    span class="sp-range" { (range_label(&comp.days)) }
                                    span class="sp-uptime" {
                                        @match comp.uptime {
                                            Some(u) => { b { (format!("{u:.2}%")) } " 正常运行" }
                                            None => { "无数据" }
                                        }
                                    }
                                }
                            }
                        }
                        p class="sp-bar-key small muted" {
                            "色条每天一格：绿色正常 · 黄色性能下降 · 橙色部分中断 · 红色严重中断"
                        }
                    }
                    section class="sp-history" aria-labelledby="past-incidents" {
                        h2 id="past-incidents" { "过去的事件" }
                        @for i in 0..RECENT_DAYS.min(HISTORY_DAYS) {
                            (history_day(clock, snap, days_before(today, i), i == 0))
                        }
                        @if HISTORY_DAYS > RECENT_DAYS {
                            details class="sp-older" {
                                summary { "更早的事件（" (HISTORY_DAYS - RECENT_DAYS) " 天）" }
                                @for i in RECENT_DAYS..HISTORY_DAYS {
                                    (history_day(clock, snap, days_before(today, i), false))
                                }
                            }
                        }
                    }
                    footer class="sp-foot" {
                        nav class="sp-foot-nav" aria-label="站内导航" {
                            a href="/" { "回首页" }
                            a href="/survey" { "使用体验问卷" }
                        }
                        p class="muted small" {
                            "事件按 调查中 → 已确定 → 监控中 → 已解决 更新。"
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gate_db::repo::status_page::Incident;
    use serde_json::json;

    fn clock() -> Clock {
        Clock::new("Asia/Shanghai", 7)
    }

    // 2026-10-14 12:00:00 +08:00
    const NOW: f64 = 1_791_950_400.0;

    fn inc(id: i64, title: &str, impact: &str, started: f64, resolved: Option<f64>) -> Incident {
        Incident {
            id,
            title: title.into(),
            impact: impact.into(),
            components: json!(["api"]),
            started_at: started,
            resolved_at: resolved,
            updates: vec![IncidentUpdate {
                status: "resolved".into(),
                body: "此事件已解决。".into(),
                at: resolved.unwrap_or(started),
            }],
        }
    }

    #[test]
    fn stamp_uses_chinese_beijing_time() {
        assert_eq!(
            stamp(&clock(), NOW, true),
            "2026年10月14日 12:00（北京时间）"
        );
        assert_eq!(stamp(&clock(), NOW, false), "10月14日 12:00（北京时间）");
        // 时间越界 / 非有限值
        assert_eq!(stamp(&clock(), f64::NAN, true), "时间无效");
        assert_eq!(stamp(&clock(), 1e30, true), "时间无效");
        // 日期短写
        assert_eq!(short_day("2026-10-09"), "10月9日");
        assert_eq!(short_day("bad"), "bad");
    }

    #[test]
    fn same_day_same_title_routine_events_merge() {
        // 新的在前（与 `list_incidents` 一致）
        let incidents = [
            inc(
                3,
                "出图接口暂时无法访问",
                "minor",
                NOW - 1800.0,
                Some(NOW - 1740.0),
            ),
            inc(
                2,
                "出图接口暂时无法访问",
                "minor",
                NOW - 3600.0,
                Some(NOW - 3540.0),
            ),
            inc(
                1,
                "出图接口暂时无法访问",
                "none",
                NOW - 5400.0,
                Some(NOW - 5340.0),
            ),
        ];
        let refs: Vec<&Incident> = incidents.iter().collect();
        let entries = day_entries(&refs);
        assert_eq!(entries.len(), 1, "同一天同标题应合并成一条");
        assert_eq!(entries[0].count(), 3);
        assert!(entries[0].merged());
        assert_eq!(entries[0].lead().id, 3);
        // 起止取最早开始与最晚结束
        assert_eq!(
            merged_meta(&clock(), &entries[0], NOW),
            "共 3 次 · 10:30–11:31（北京时间）"
        );
    }

    #[test]
    fn major_events_are_kept_separate() {
        let incidents = [
            inc(
                2,
                "出图接口无法访问",
                "major",
                NOW - 600.0,
                Some(NOW - 300.0),
            ),
            inc(
                1,
                "出图接口无法访问",
                "minor",
                NOW - 3600.0,
                Some(NOW - 3000.0),
            ),
        ];
        let refs: Vec<&Incident> = incidents.iter().collect();
        let entries = day_entries(&refs);
        assert_eq!(entries.len(), 2, "人工重大事件不和自动事件合并");
        assert_eq!(entries[0].lead().id, 2);
        assert!(!entries[0].merged());
    }

    #[test]
    fn different_titles_in_one_day_stay_separate() {
        let incidents = [
            inc(
                2,
                "出图接口暂时无法访问",
                "minor",
                NOW - 600.0,
                Some(NOW - 300.0),
            ),
            inc(
                1,
                "上游限流：出图速度已下调",
                "minor",
                NOW - 3600.0,
                Some(NOW - 3000.0),
            ),
        ];
        let refs: Vec<&Incident> = incidents.iter().collect();
        let entries = day_entries(&refs);
        assert_eq!(entries.len(), 2);
    }
}
