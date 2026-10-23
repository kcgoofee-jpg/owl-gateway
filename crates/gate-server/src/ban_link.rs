//! 封禁号关联（站长 10-22）：账号 X 和一个已封禁的账号 B 在**同一个浏览器**（浏览器标记一样）或
//! **同一台非苹果设备**（设备指纹 ID 一样）上网页登录过 = X 关联了封禁号。
//!
//! - /register 不给 X 发 Key（邀请 Key 也不绑定），提示 [`BLOCK_NOTICE`]；已经有 Key 的不受影响，什么都不自动撤销；
//! - 待审里出一条 `banned_link`（95 分），同时记一条「注意」级 Bug（后台「Bug 追踪」和巡检都能看到）；
//! - 苹果设备同型号指纹几乎一样，只按设备撞上时只提醒（60 分）、不拦；
//! - 站长在待审里点「忽略」= 放行这一对；「确认」照样拦。被忽略后又多了更强的证据（分数涨 ≥ 10）会重新打开、重新拦。
//!
//! 信号在网页登录时（[`on_login`]）和每 6 小时的小号扫描里都会算；拦不拦在领 Key 时现查（[`blocked`]），不依赖信号已写好。

use crate::state::AppState;
use gate_db::repo::abuse::BanInfo;
use gate_db::{Db, DbError};
use serde_json::{Value, json};
use std::collections::BTreeMap;

pub const KIND: &str = "banned_link";
/// /register 拒绝时给成员看的话
pub const BLOCK_NOTICE: &str = "这台设备关联的账号已被封禁，如有误会请私信站长申诉";
/// 同浏览器 / 同（非苹果）设备：拦领 Key
pub const SCORE: f64 = 95.0;
/// 只在苹果设备上撞了指纹：只提醒
pub const APPLE_SCORE: f64 = 60.0;
const BUG_SOURCE: &str = "banned_link";
const BUG_TITLE: &str = "封禁号关联的账号登录";

/// 苹果设备：同型号的 Canvas / WebGL 几乎一样，指纹撞车不能当强证据。
pub fn is_apple(parts: &Value) -> bool {
    let g = parts
        .get("gpuRenderer")
        .and_then(Value::as_str)
        .unwrap_or("");
    let p = parts.get("platform").and_then(Value::as_str).unwrap_or("");
    g.contains("Apple") || p.contains("iPhone") || p.contains("iPad") || p.contains("Mac")
}

/// `device_fingerprints.data` → 各分项（兼容旧格式 `{id, data:{...}}`）。
pub fn device_parts(data: &str) -> Value {
    let v: Value = serde_json::from_str(data).unwrap_or(Value::Null);
    v.get("data").cloned().filter(Value::is_object).unwrap_or(v)
}

/// X 和某个封禁号是怎么关联上的。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Ways {
    pub browser: bool,
    /// 同一个设备指纹，且不是苹果设备
    pub device: bool,
    /// 同一个设备指纹，苹果设备（只提醒）
    pub apple_device: bool,
}

impl Ways {
    pub fn add_device(&mut self, apple: bool) {
        if apple {
            self.apple_device = true;
        } else {
            self.device = true;
        }
    }
    /// 拦不拦领 Key
    pub fn blocks(&self) -> bool {
        self.browser || self.device
    }
    fn label(&self) -> String {
        let mut v = Vec::new();
        if self.browser {
            v.push("同浏览器");
        }
        if self.device {
            v.push("同设备");
        }
        if self.apple_device && !self.device {
            v.push("同设备（苹果）");
        }
        v.join("、")
    }
}

/// 信号的 subject：两个 Discord ID 排序后用逗号连起来（和其他信号一样）。
pub fn subject(x: &str, banned: &str) -> String {
    let mut v = [x, banned];
    v.sort_unstable();
    v.join(",")
}

fn banned_name<'a>(banned: &'a str, info: &'a BanInfo) -> &'a str {
    if info.name.is_empty() {
        banned
    } else {
        &info.name
    }
}

/// 信号的分数和 detail。
pub fn signal(banned: &str, info: &BanInfo, ways: &Ways) -> (f64, Value) {
    let name = banned_name(banned, info);
    let (score, summary) = if ways.blocks() {
        (
            SCORE,
            format!(
                "和已封禁的「{name}」{}登录过，领 Key 已拦下（忽略 = 放行）",
                ways.label()
            ),
        )
    } else {
        (
            APPLE_SCORE,
            format!("和已封禁的「{name}」在同一台苹果设备上登录过（苹果易撞，只提醒、不拦）"),
        )
    };
    let mut evidence = vec![
        json!({"k": "关联的封禁号", "v": name}),
        json!({"k": "方式", "v": ways.label()}),
    ];
    if !info.reason.is_empty() {
        evidence.push(json!({"k": "封禁理由", "v": info.reason}));
    }
    (score, json!({"summary": summary, "evidence": evidence}))
}

/// 记 Bug 时的说明（不含 IP / Key）。
pub fn alert_text(x: &str, banned: &str, info: &BanInfo, ways: &Ways) -> String {
    let name = banned_name(banned, info);
    let tail = if ways.blocks() {
        "已拦下领 Key；到后台「待审」确认或忽略（忽略 = 放行）。"
    } else {
        "苹果设备易撞，只提醒、不拦。"
    };
    format!(
        "Discord {x} 和已封禁的「{name}」（{banned}）{}网页登录过。{tail}",
        ways.label()
    )
}

/// 这个账号（自己没被封）和哪些封禁号有关联。
pub async fn links_of(db: &Db, x: &str) -> Result<Vec<(String, BanInfo, Ways)>, DbError> {
    let peers = db.login_peers(x).await?;
    let mut ids: Vec<String> = peers.iter().map(|p| p.discord_id.clone()).collect();
    ids.push(x.to_string());
    let bans = db.ban_infos(ids).await?;
    if bans.contains_key(x) {
        return Ok(Vec::new());
    }
    let mut by: BTreeMap<String, Ways> = BTreeMap::new();
    for p in &peers {
        if !bans.contains_key(&p.discord_id) {
            continue;
        }
        let w = by.entry(p.discord_id.clone()).or_default();
        if p.via == "browser" {
            w.browser = true;
        } else {
            w.add_device(is_apple(&device_parts(&p.device_data)));
        }
    }
    Ok(by
        .into_iter()
        .map(|(b, w)| {
            let info = bans.get(&b).cloned().unwrap_or_default();
            (b, info, w)
        })
        .collect())
}

/// 领 Key 前查：有拦人的关联（同浏览器 / 同非苹果设备），且站长没在待审里忽略那一对 → true。
pub async fn blocked(db: &Db, x: &str) -> Result<bool, DbError> {
    for (b, _, ways) in links_of(db, x).await? {
        if !ways.blocks() {
            continue;
        }
        let status = db.abuse_signal_status(KIND, &subject(x, &b)).await?;
        if status.as_deref() != Some("dismissed") {
            return Ok(true);
        }
    }
    Ok(false)
}

/// 写进待审；这一对是新出现的（或被忽略 / 过期后又打开）就记一条「注意」级 Bug。
pub async fn record(
    state: &AppState,
    x: &str,
    banned: &str,
    info: &BanInfo,
    ways: &Ways,
) -> Result<(), DbError> {
    let subj = subject(x, banned);
    let prev = state.db.abuse_signal_status(KIND, &subj).await?;
    let key_ids = state
        .db
        .registration_key_id(x)
        .await?
        .map(|k| k.to_string())
        .unwrap_or_default();
    let (score, detail) = signal(banned, info, ways);
    state
        .db
        .upsert_abuse_signal(
            KIND,
            &subj,
            &key_ids,
            score,
            &detail.to_string(),
            state.now(),
        )
        .await?;
    let now = state.db.abuse_signal_status(KIND, &subj).await?;
    alert_if_opened(
        state,
        prev.as_deref(),
        now.as_deref(),
        &alert_text(x, banned, info, ways),
    );
    Ok(())
}

/// 信号从「不在待审」变成「待审」时记 Bug（同一对反复登录不重复记）。
pub fn alert_if_opened(state: &AppState, prev: Option<&str>, now: Option<&str>, text: &str) {
    if now == Some("open") && prev != Some("open") {
        state.bugs.capture(crate::bugs::Report {
            source: BUG_SOURCE,
            title: BUG_TITLE,
            detail: text,
            level: "warn",
            ..Default::default()
        });
    }
}

/// 网页登录后：这个账号和封禁号的关联写进待审。登录的人自己被封了，就反过来给和他同浏览器 / 同设备的未封账号记。
/// 尽力而为，失败只记日志。
pub async fn on_login(state: &AppState, who: &str) {
    if let Err(e) = on_login_inner(state, who).await {
        tracing::warn!(target: "web-login", "记封禁号关联失败：{e}");
    }
}

async fn on_login_inner(state: &AppState, who: &str) -> Result<(), DbError> {
    let me_banned = !state.db.ban_infos(vec![who.to_string()]).await?.is_empty();
    if me_banned {
        // 被封的号又来登录：和他共用浏览器 / 设备的未封账号都算关联
        let mut seen = std::collections::BTreeSet::new();
        for p in state.db.login_peers(who).await? {
            if !seen.insert(p.discord_id.clone()) {
                continue;
            }
            for (b, info, ways) in links_of(&state.db, &p.discord_id).await? {
                if b == who {
                    record(state, &p.discord_id, &b, &info, &ways).await?;
                }
            }
        }
        return Ok(());
    }
    for (b, info, ways) in links_of(&state.db, who).await? {
        record(state, who, &b, &info, &ways).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ways_label_and_blocking() {
        let mut w = Ways::default();
        w.add_device(true);
        assert!(!w.blocks());
        assert_eq!(w.label(), "同设备（苹果）");
        w.browser = true;
        assert!(w.blocks());
        assert_eq!(w.label(), "同浏览器、同设备（苹果）");
        w.add_device(false);
        assert_eq!(w.label(), "同浏览器、同设备");
        assert_eq!(subject("9", "10"), "10,9");
        let (score, d) = signal(
            "1",
            &BanInfo::default(),
            &Ways {
                device: true,
                ..Default::default()
            },
        );
        assert_eq!(score, SCORE);
        assert_eq!(d["evidence"][0]["v"], "1");
    }

    #[test]
    fn apple_detection() {
        assert!(!is_apple(
            &json!({"gpuRenderer": "ANGLE (NVIDIA)", "platform": "Win32"})
        ));
        assert!(is_apple(
            &json!({"gpuRenderer": "Apple GPU", "platform": "iPhone"})
        ));
        assert!(is_apple(&device_parts(
            r#"{"id":"x","data":{"platform":"MacIntel"}}"#
        )));
    }
}
