//! Discord 领取记录与黑名单。

use crate::{Db, Result, value};
use gate_core::pyc;
use rusqlite::params;
use serde_json::Value;

/// 封禁时附带的资料（缺省值与旧版一致：banned_by = "系统"，其余空串）。
#[derive(Debug, Clone)]
pub struct BanInfo {
    pub reason: String,
    pub username: String,
    pub display_name: String,
    pub avatar: String,
    pub banned_by: String,
}

impl Default for BanInfo {
    fn default() -> Self {
        Self {
            reason: String::new(),
            username: String::new(),
            display_name: String::new(),
            avatar: String::new(),
            banned_by: "系统".into(),
        }
    }
}

impl Db {
    /// 删除该 Key 对应的领取记录，返回被删记录的 Discord ID（用于摘除身份组）。
    pub async fn forget_registration_for_key(&self, key_id: i64) -> Result<Vec<String>> {
        self.write(move |tx| {
            let ids: Vec<String> = {
                let mut stmt =
                    tx.prepare("SELECT discord_id FROM discord_registrations WHERE key_id=?")?;
                stmt.query_map([key_id], |r| value::text_or(r, 0, "None"))?
                    .collect::<rusqlite::Result<_>>()?
            };
            tx.execute("DELETE FROM discord_registrations WHERE key_id=?", [key_id])?;
            Ok(ids)
        })
        .await
    }

    /// 拉入黑名单：记录封禁理由、操作者及 Discord 用户资料。已在名单里时刷新时间 / 理由 / 操作者，
    /// 用户资料只在新值非空时覆盖（空串保留旧值）。
    pub async fn ban_discord(&self, discord_id: &str, info: &BanInfo, now: f64) -> Result<()> {
        let discord_id = discord_id.to_string();
        let info = info.clone();
        self.write(move |tx| {
            tx.execute(
                "INSERT INTO discord_bans(discord_id, created_at, reason, username, display_name, avatar, banned_by) \
                 VALUES (?,?,?,?,?,?,?) \
                 ON CONFLICT(discord_id) DO UPDATE SET \
                     created_at=excluded.created_at, \
                     reason=excluded.reason, \
                     username=COALESCE(NULLIF(excluded.username, ''), discord_bans.username), \
                     display_name=COALESCE(NULLIF(excluded.display_name, ''), discord_bans.display_name), \
                     avatar=COALESCE(NULLIF(excluded.avatar, ''), discord_bans.avatar), \
                     banned_by=excluded.banned_by",
                params![
                    discord_id,
                    now,
                    info.reason,
                    info.username,
                    info.display_name,
                    info.avatar,
                    info.banned_by
                ],
            )?;
            Ok(())
        })
        .await
    }

    /// 移出黑名单；本来就不在名单里返回 false。
    pub async fn unban_discord(&self, discord_id: &str) -> Result<bool> {
        let discord_id = discord_id.to_string();
        self.write(move |tx| {
            let n = tx.execute("DELETE FROM discord_bans WHERE discord_id=?", [&discord_id])?;
            Ok(n > 0)
        })
        .await
    }

    /// 黑名单列表（新→旧）；`q` 非空时按 discord_id / username / display_name / reason 模糊匹配
    /// （LIKE 通配符不转义，与旧版一致）。
    pub async fn list_bans(&self, q: &str) -> Result<Vec<Value>> {
        let q = pyc::strip(q).to_string();
        self.read(move |c| {
            if q.is_empty() {
                let mut stmt = c.prepare(
                    "SELECT discord_id, created_at, reason, username, display_name, avatar, banned_by \
                     FROM discord_bans \
                     ORDER BY created_at DESC",
                )?;
                stmt.query_map([], value::row_json)?.collect()
            } else {
                let pat = format!("%{q}%");
                let mut stmt = c.prepare(
                    "SELECT discord_id, created_at, reason, username, display_name, avatar, banned_by \
                     FROM discord_bans \
                     WHERE discord_id LIKE ? OR username LIKE ? OR display_name LIKE ? OR reason LIKE ? \
                     ORDER BY created_at DESC",
                )?;
                stmt.query_map(params![pat, pat, pat, pat], value::row_json)?.collect()
            }
        })
        .await
    }
}
