//! 封禁号关联：同浏览器阻止领 Key，同设备指纹只提示。
//!
//! - /register 不给 X 发 Key（邀请 Key 也不绑定），提示 [`BLOCK_NOTICE`]；已经有 Key 的不受影响，什么都不自动撤销；
//! - 待审里出一条 `banned_link`（95 分），同时记一条「注意」级 Bug（后台「Bug 追踪」和巡检都能看到）；
//! - 只匹配设备指纹时提醒（60 分）、不拦；
//! - 站长在待审里点「忽略」= 放行这一对；「确认」照样拦。被忽略后又多了更强的证据（分数涨 ≥ 10）会重新打开、重新拦。
//!
//! 信号在网页登录时（[`on_login`]）和每 6 小时的小号扫描里都会算；拦不拦在领 Key 时现查（[`blocked`]），不依赖信号已写好。

use crate::state::AppState;
use gate_db::repo::abuse::BanInfo;
use gate_db::{Db, DbError};
use std::collections::BTreeMap;

pub use gate_core::abuse::banned_link::{
    BLOCK_NOTICE, DEVICE_SCORE, KIND, SCORE, Ways, alert_text, signal, subject,
};
pub use gate_core::abuse::{device_parts, is_apple};
const BUG_SOURCE: &str = "banned_link";
const BUG_TITLE: &str = "封禁号关联的账号登录";

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

/// 领 Key 前查：同浏览器且站长没忽略这一对时阻止。
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
