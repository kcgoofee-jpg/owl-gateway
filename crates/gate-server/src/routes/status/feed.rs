//! `/status.json` 的内容与状态页用到的小工具。

use super::snapshot::{Snapshot, impact_rank};
use gate_core::pyval::float;
use gate_db::repo::status_page::Incident;
use serde_json::{Value, json};

/// Statuspage 的更新状态词（`STATUS_LABEL`）。
pub const STATUS_LABEL: [(&str, &str); 5] = [
    ("investigating", "调查中"),
    ("identified", "已确定"),
    ("update", "更新"),
    ("monitoring", "监控中"),
    ("resolved", "已解决"),
];

/// Statuspage 标准句式（中文本地化）：直接写库发布事件时，正文留空就用它（`xtask seed` 在用）。
pub const STANDARD_BODY: [(&str, &str); 5] = [
    ("investigating", "我们正在调查此问题。"),
    ("identified", "问题已定位，正在实施修复。"),
    ("update", "我们正在继续处理此问题。"),
    ("monitoring", "修复已上线，我们正在观察结果。"),
    ("resolved", "此事件已解决。"),
];

/// `STATUS_LABEL.get(status, status)`
pub fn status_label(status: &str) -> &str {
    STATUS_LABEL
        .iter()
        .find(|(s, _)| *s == status)
        .map_or(status, |(_, l)| l)
}

pub fn standard_body(status: &str) -> Option<&'static str> {
    STANDARD_BODY
        .iter()
        .find(|(s, _)| *s == status)
        .map(|(_, b)| *b)
}

/// `Incident` → 事件 JSON（键顺序固定：id、title、impact、components、started_at、resolved_at、updates）。
pub fn incident_json(inc: &Incident) -> Value {
    json!({
        "id": inc.id,
        "title": inc.title,
        "impact": inc.impact,
        "components": inc.components,
        "started_at": float(inc.started_at),
        "resolved_at": inc.resolved_at.map_or(Value::Null, float),
        "updates": inc.updates.iter().map(|u| json!({
            "status": u.status,
            "body": u.body,
            "at": float(u.at),
        })).collect::<Vec<_>>(),
    })
}

/// `/status.json` 的内容：组件摘要 + 未解决事件 + 最近 50 个事件。
pub fn status_json(snap: &Snapshot) -> Value {
    json!({
        "components": snap.components.iter().map(|c| json!({
            "key": c.key,
            "name": c.name,
            "status": c.status,
            "uptime": c.uptime.map_or(Value::Null, float),
        })).collect::<Vec<_>>(),
        "active": snap.active.iter().map(|&i| incident_json(&snap.incidents[i])).collect::<Vec<_>>(),
        "incidents": snap.incidents.iter().take(50).map(incident_json).collect::<Vec<_>>(),
    })
}

/// 当前未解决事件里最严重的严重度（页面 `<body>` 的状态类用）；并列取先出现的（Python `max(key=)` 的行为）。
pub fn worst_active_impact(snap: &Snapshot) -> Option<&str> {
    let mut best: Option<(&str, usize)> = None;
    for &i in &snap.active {
        let imp = snap.incidents[i].impact.as_str();
        let rank = impact_rank(imp);
        if best.is_none_or(|(_, r)| rank > r) {
            best = Some((imp, rank));
        }
    }
    best.map(|(i, _)| i)
}
