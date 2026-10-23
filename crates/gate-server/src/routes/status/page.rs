//! `/status` 页面：和首页同一套顶栏、设计令牌、字体和卡片（2026-10 起不再照搬 Statuspage 模板）。
//!
//! - 样式是 `web/src/ui/tokens.css` + `base.css` + `chrome.css` + `status-page.css` 拼成的一份同源样式表
//!   （见 `routes::status`），页面上没有 style 属性（CSP 是 `style-src 'self'`）；
//! - 90 天色条用网站的状态色（无数据 surface-3、正常 ok、轻微 warn、部分中断 orange、严重 danger），颜色走类名；
//!   按天从左往右长出来的动画在 `status-page.js` 里（系统开了「减少动态效果」时不播）；
//! - 文案（横幅、组件状态、事件进展）和 Statuspage 的标准说法一致。

use super::feed::{status_label, worst_active_impact};
use super::snapshot::{
    DAYS, Day, HISTORY_DAYS, IMPACT_COMPONENT, Snapshot, days_before, impact_rank, local_date,
    midnight,
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

/// `_stamp`：`2026年10月14日 12:00 UTC+8`（过去事件里不带年）。时间超出 Python `datetime` 的范围写「时间无效」。
fn stamp(clock: &Clock, ts: f64, with_year: bool) -> String {
    // datetime 只认公元 1–9999 年
    if !ts.is_finite() || !(-62_135_596_800.0..=253_402_300_799.0).contains(&ts) {
        return "时间无效".into();
    }
    let parts: Vec<i64> = clock
        .format(ts, "%Y|%m|%d|%H|%M")
        .split('|')
        .filter_map(|p| p.parse().ok())
        .collect();
    let [y, mo, d, h, mi] = parts[..] else {
        return "时间无效".into();
    };
    format!(
        "{}{mo}月{d}日 {h:02}:{mi:02} UTC+8",
        if with_year {
            format!("{y}年")
        } else {
            String::new()
        }
    )
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
    html! {
        svg preserveAspectRatio="none" height="34"
            viewBox=(format!("0 0 {} 34", days.len() * 5 - 2)) {
            @for (i, d) in days.iter().enumerate() {
                @let tip = match d.level {
                    None => format!("{}：无数据", d.day),
                    Some(0) if d.titles.is_empty() => format!("{}：没有记录的停机", d.day),
                    // 没有事件、但按上游失败率自动算出了级别
                    Some(_) if d.titles.is_empty() => format!("{}：上游失败率偏高", d.day),
                    Some(_) => format!("{}：{}", d.day, d.titles.join("；")),
                };
                rect class=(bar_class(d.level)) height="34" width="3" x=(i * 5) y="0" { title { (tip) } }
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
    html! {
        div class="sp-day reveal" {
            h3 { (day.month()) "月" (day.day()) "日" }
            @if incs.is_empty() {
                p class="muted" { (if is_today { "今天没有报告事件。" } else { "没有报告事件。" }) }
            }
            @for inc in incs {
                div class="card sp-inc" id=(format!("incident-{}", inc.id)) {
                    p class=(impact_class(&inc.impact)) {
                        a class="sp-inc-link" href=(format!("#incident-{}", inc.id)) { (inc.title) }
                    }
                    ol class="sp-updates" {
                        @for u in &inc.updates { (update_html(clock, u, false)) }
                    }
                }
            }
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

pub fn render(snap: &Snapshot, clock: &Clock, subscribe_url: &str, assets: &AssetRefs) -> Markup {
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
                        a class="btn sp-sub" href=(subscribe_url) rel="noopener" { "订阅更新" }
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
                                    span { (DAYS) " 天前" }
                                    span class="sp-uptime" {
                                        @match comp.uptime {
                                            Some(u) => { b { (format!("{u:.2}%")) } " 正常运行" }
                                            None => { "无数据" }
                                        }
                                    }
                                    span { "今天" }
                                }
                            }
                        }
                    }
                    section class="sp-history" aria-labelledby="past-incidents" {
                        h2 id="past-incidents" { "过去的事件" }
                        @for i in 0..HISTORY_DAYS {
                            (history_day(clock, snap, days_before(today, i), i == 0))
                        }
                    }
                    p class="muted small sp-foot" {
                        "所有时间均为北京时间（UTC+8）。事件按 调查中 → 已确定 → 监控中 → 已解决 更新。"
                    }
                }
            }
        }
    }
}
