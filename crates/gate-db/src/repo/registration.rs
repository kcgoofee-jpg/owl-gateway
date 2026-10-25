//! Discord 领取记录（`discord_registrations`）、待摘身份组（`pending_role_removals`）与成员反馈（`feedback`）。
//!
//! 领取记录 / 问卷的查询与写入。「几条语句 + 一次 commit」= 这里一个事务；
//! 中间夹着 Discord 请求的地方分成几个方法（不能在持有未提交写事务时等网络）。

use super::INVITE_TAG;
use crate::{Db, Result, value};
use rusqlite::{OptionalExtension, params};
use serde_json::Value;

/// 领取记录里的 Discord 资料（`SELECT username, display_name, avatar`）。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RegistrationProfile {
    pub username: Option<String>,
    pub display_name: Option<String>,
    pub avatar: Option<String>,
}

/// 新 Key 的领取记录与改名（`_provision` / `bind_invite` 落库的那几条语句）。
#[derive(Debug, Clone)]
pub struct NewRegistration<'a> {
    pub discord_id: &'a str,
    pub key_id: i64,
    pub now: f64,
    pub username: Option<&'a str>,
    pub display_name: Option<&'a str>,
    pub avatar: Option<&'a str>,
    /// `member_label(user)`：写回 Key 的名字。
    pub key_name: &'a str,
    /// 配了「已领 Key」身份组：清掉旧的待摘除并标记 `role_granted=1`。
    pub member_role: bool,
}

/// 退群核对的一个对象。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DepartedCandidate {
    pub discord_id: String,
    /// 领到的是站长发的邀请 Key（带「邀请」标签）时：`(key_id, Key 名字)`。
    /// 邀请 Key 只由站长手动删，退群时不回收。
    pub invite_key: Option<(i64, String)>,
}

impl Db {
    /// 这个 Discord 账号领到的 Key（`SELECT key_id FROM discord_registrations WHERE discord_id=?`）。
    pub async fn registration_key_id(&self, discord_id: &str) -> Result<Option<i64>> {
        let id = discord_id.to_string();
        self.read(move |c| {
            let mut stmt =
                c.prepare("SELECT key_id FROM discord_registrations WHERE discord_id=?")?;
            let mut rows = stmt.query([id])?;
            Ok(match rows.next()? {
                Some(r) => r.get::<_, Option<i64>>(0)?,
                None => None,
            })
        })
        .await
    }

    /// `key_row_for(discord_id)`：领到的 Key 行；没领过 / Key 已删为 None。
    pub async fn key_row_for_discord(&self, discord_id: &str) -> Result<Option<serde_json::Value>> {
        match self.registration_key_id(discord_id).await? {
            Some(id) => self.get_key(id).await,
            None => Ok(None),
        }
    }

    /// 近一小时新发出的 Key 数（每小时领取上限用）：`COUNT(*) FROM discord_registrations WHERE created_at>=?`。
    pub async fn registrations_since(&self, since: f64) -> Result<i64> {
        self.read(move |c| {
            c.query_row(
                "SELECT COUNT(*) FROM discord_registrations WHERE created_at>=?",
                [since],
                |r| value::i64_or(r, 0, 0),
            )
        })
        .await
    }

    /// 在黑名单里就返回拉黑理由（可能是空串），不在返回 None。
    pub async fn discord_ban_reason(&self, discord_id: &str) -> Result<Option<String>> {
        let id = discord_id.to_string();
        self.read(move |c| {
            c.query_row(
                "SELECT reason FROM discord_bans WHERE discord_id=?",
                [id],
                |r| value::text_or(r, 0, ""),
            )
            .optional()
        })
        .await
    }

    /// 撤销：删除该用户的 Key（连带标签 / 领取记录）。返回被删的 Key；没有领取记录为 None。
    pub async fn revoke_registration(&self, discord_id: &str, now: f64) -> Result<Option<i64>> {
        match self.registration_key_id(discord_id).await? {
            Some(key_id) => {
                // delete_key 之后还会再按 discord_id 删一次领取记录；key_id 是唯一键，
                // delete_key 已经按 key_id 删过了，那一步是空操作，不重复。
                self.delete_key(key_id, now).await?;
                Ok(Some(key_id))
            }
            None => Ok(None),
        }
    }

    // ------------------------------------------------------------------ 身份组核验（OAuth）

    /// 记下成员授权核验过的身份组（重复核验只刷新时间）。
    pub async fn upsert_role_verification(
        &self,
        discord_id: &str,
        guild_id: &str,
        role_id: &str,
        now: f64,
    ) -> Result<()> {
        let (d, g, r) = (
            discord_id.to_string(),
            guild_id.to_string(),
            role_id.to_string(),
        );
        self.write(move |tx| {
            tx.execute(
                "INSERT INTO role_verifications(discord_id, guild_id, role_id, verified_at) VALUES (?,?,?,?) \
                 ON CONFLICT(discord_id, guild_id, role_id) DO UPDATE SET verified_at=excluded.verified_at",
                params![d, g, r, now],
            )?;
            Ok(())
        })
        .await
    }

    /// 这个账号是否核验过指定服务器的指定身份组。
    pub async fn role_verified(
        &self,
        discord_id: &str,
        guild_id: &str,
        role_id: &str,
    ) -> Result<bool> {
        let (d, g, r) = (
            discord_id.to_string(),
            guild_id.to_string(),
            role_id.to_string(),
        );
        self.read(move |c| {
            c.query_row(
                "SELECT EXISTS(SELECT 1 FROM role_verifications WHERE discord_id=? AND guild_id=? AND role_id=?)",
                params![d, g, r],
                |row| row.get::<_, i64>(0),
            )
            .map(|n| n != 0)
        })
        .await
    }

    // ------------------------------------------------------------------ 身份组待办

    /// `INSERT OR REPLACE INTO pending_role_removals`。
    pub async fn queue_role_removal(&self, discord_id: &str, now: f64) -> Result<()> {
        let id = discord_id.to_string();
        self.write(move |tx| {
            tx.execute(
                "INSERT OR REPLACE INTO pending_role_removals(discord_id, created_at) VALUES (?,?)",
                params![id, now],
            )?;
            Ok(())
        })
        .await
    }

    /// 摘除成功后划掉待办。
    pub async fn finish_role_removal(&self, discord_id: &str) -> Result<()> {
        let id = discord_id.to_string();
        self.write(move |tx| {
            tx.execute(
                "DELETE FROM pending_role_removals WHERE discord_id=?",
                [&id],
            )?;
            Ok(())
        })
        .await
    }

    pub async fn pending_role_removals(&self) -> Result<Vec<String>> {
        self.read(|c| {
            let mut stmt = c.prepare("SELECT discord_id FROM pending_role_removals")?;
            stmt.query_map([], |r| value::text_or(r, 0, "None"))?
                .collect()
        })
        .await
    }

    /// `sync_roles` 的第一步：Key 已过期 / 被停用 / 被删除、却还标着「已挂身份组」的成员，
    /// 登记摘除待办并清掉标志（一个事务）。返回这些成员。
    pub async fn queue_expired_role_removals(&self, now: f64) -> Result<Vec<String>> {
        self.write(move |tx| {
            let ids: Vec<String> = {
                let mut stmt = tx.prepare(
                    "SELECT r.discord_id FROM discord_registrations r LEFT JOIN api_keys k ON k.id=r.key_id \
                     WHERE r.role_granted=1 AND (k.id IS NULL OR k.enabled=0 \
                           OR (k.expires_at IS NOT NULL AND k.expires_at < ?))",
                )?;
                stmt.query_map([now], |r| value::text_or(r, 0, "None"))?
                    .collect::<rusqlite::Result<_>>()?
            };
            for id in &ids {
                tx.execute(
                    "INSERT OR REPLACE INTO pending_role_removals(discord_id, created_at) VALUES (?,?)",
                    params![id, now],
                )?;
                tx.execute(
                    "UPDATE discord_registrations SET role_granted=0 WHERE discord_id=?",
                    [id],
                )?;
            }
            Ok(ids)
        })
        .await
    }

    // ------------------------------------------------------------------ 退群回收 / 资料补全

    /// 该核对「是否还在服务器」的领取记录（最久没核对的在前），并带上是不是邀请 Key。
    /// 邀请 Key 的持有人照样核对（退群时给站长留一条提醒），但不回收。
    pub async fn departed_candidates(
        &self,
        now: f64,
        batch: i64,
    ) -> Result<Vec<DepartedCandidate>> {
        self.read(move |c| {
            let mut stmt = c.prepare(
                "SELECT r.discord_id, k.id, k.name FROM discord_registrations r \
                 LEFT JOIN api_keys k ON k.id=r.key_id \
                   AND EXISTS (SELECT 1 FROM key_tags t WHERE t.key_id=r.key_id AND t.tag=?) \
                 WHERE COALESCE(r.member_checked_at, 0) < ? \
                 ORDER BY COALESCE(r.member_checked_at, 0) LIMIT ?",
            )?;
            stmt.query_map(params![INVITE_TAG, now - 86400.0, batch], |r| {
                let invite_id = value::i64_opt(r, 1)?;
                let name = value::text_or(r, 2, "")?;
                Ok(DepartedCandidate {
                    discord_id: value::text_or(r, 0, "None")?,
                    invite_key: invite_id.map(|id| (id, name)),
                })
            })?
            .collect()
        })
        .await
    }

    pub async fn set_member_checked(&self, discord_id: &str, at: f64) -> Result<()> {
        let id = discord_id.to_string();
        self.write(move |tx| {
            tx.execute(
                "UPDATE discord_registrations SET member_checked_at=? WHERE discord_id=?",
                params![at, id],
            )?;
            Ok(())
        })
        .await
    }

    // ------------------------------------------------------------------ 进服时间 / 成员标记位（防小号）

    /// 记下一次读到的成员资料（`GuildMember.joined_at` 换成 Unix 秒、`GuildMember.flags`）。
    /// 只更新已有的领取记录（没领过 Key 的账号不存）；返回是否有记录被更新。
    ///
    /// 合并而不是覆盖：进服时间取「已存的」和「这次的」里较早的那个（`joined_at=None` 不动已存的值），
    /// 标记位按位或（1 = DID_REJOIN 退群后又重新加入）。网页登录查的是门槛服务器、退群核对查的是机器人所在的服务器，
    /// 两边看到的服务器不一样，合并才不会来回跳。
    pub async fn record_member_join(
        &self,
        discord_id: &str,
        joined_at: Option<f64>,
        flags: i64,
    ) -> Result<bool> {
        let id = discord_id.to_string();
        self.write(move |tx| {
            let n = tx.execute(
                "UPDATE discord_registrations SET \
                 guild_joined_at = CASE \
                   WHEN ?1 IS NULL THEN guild_joined_at \
                   WHEN guild_joined_at IS NULL OR ?1 < guild_joined_at THEN ?1 \
                   ELSE guild_joined_at END, \
                 member_flags = COALESCE(member_flags, 0) | ?2 \
                 WHERE discord_id=?3",
                params![joined_at, flags, id],
            )?;
            Ok(n > 0)
        })
        .await
    }

    /// 成员的进服时间与标记位：`Some((joined_at, flags))`；没有领取记录为 None。
    /// `joined_at` 为 None = 还没读到过（老记录等退群核对补上）；`flags` 没读到过按 0。
    pub async fn get_member_join(&self, discord_id: &str) -> Result<Option<(Option<f64>, i64)>> {
        let id = discord_id.to_string();
        self.read(move |c| {
            c.query_row(
                "SELECT guild_joined_at, member_flags FROM discord_registrations WHERE discord_id=?",
                [id],
                |r| Ok((value::f64_opt(r, 0)?, value::i64_or(r, 1, 0)?)),
            )
            .optional()
        })
        .await
    }

    /// 还没有 Discord 用户名的领取记录：`(discord_id, key_id)`。
    pub async fn profile_backfill_candidates(&self, limit: i64) -> Result<Vec<(String, i64)>> {
        self.read(move |c| {
            let mut stmt = c.prepare(
                "SELECT discord_id, key_id FROM discord_registrations WHERE username IS NULL LIMIT ?",
            )?;
            stmt.query_map([limit], |r| {
                Ok((value::text_or(r, 0, "None")?, value::i64_or(r, 1, 0)?))
            })?
            .collect()
        })
        .await
    }

    /// 补全资料；Key 名字还是默认的「Discord:<id>」时顺带改成 `new_key_name`（一个事务）。
    pub async fn apply_profile_backfill(
        &self,
        discord_id: &str,
        key_id: i64,
        profile: &RegistrationProfile,
        new_key_name: &str,
    ) -> Result<()> {
        let (id, p, name) = (
            discord_id.to_string(),
            profile.clone(),
            new_key_name.to_string(),
        );
        self.write(move |tx| {
            tx.execute(
                "UPDATE discord_registrations SET username=?, display_name=?, avatar=? WHERE discord_id=?",
                params![p.username, p.display_name, p.avatar, id],
            )?;
            let current: Option<String> = tx
                .query_row("SELECT name FROM api_keys WHERE id=?", [key_id], |r| {
                    value::text_opt(r, 0)
                })
                .optional()?
                .flatten();
            if current.is_some_and(|n| n.starts_with("Discord:")) {
                tx.execute("UPDATE api_keys SET name=? WHERE id=?", params![name, key_id])?;
            }
            Ok(())
        })
        .await
    }

    // ------------------------------------------------------------------ 邀请 Key / 发 Key

    /// 站长发的邀请 Key（带「邀请」标签）还没绑定 Discord 账号。
    pub async fn is_unbound_invite(&self, key_id: i64) -> Result<bool> {
        self.read(move |c| {
            c.query_row(
                "SELECT 1 FROM key_tags WHERE key_id=? AND tag=? \
                 AND NOT EXISTS (SELECT 1 FROM discord_registrations WHERE key_id=?)",
                params![key_id, INVITE_TAG, key_id],
                |_| Ok(()),
            )
            .optional()
            .map(|r| r.is_some())
        })
        .await
    }

    /// 按 token 找 Key：`(id, enabled)`。
    pub async fn key_id_enabled_by_token(&self, token: &str) -> Result<Option<(i64, bool)>> {
        let token = token.to_string();
        self.read(move |c| {
            c.query_row(
                "SELECT id, enabled FROM api_keys WHERE token=?",
                [token],
                |r| Ok((value::i64_or(r, 0, 0)?, value::flag(r, 1)?)),
            )
            .optional()
        })
        .await
    }

    /// 领取 / 绑定邀请 Key 落库：登记领取、Key 改名、写 `api_keys.discord_id`、身份组标志（一个事务）。
    /// Key 本身由调用方先建好；`discord_id` 在建 Key 时已带上的话，这里再写一次是幂等的。
    pub async fn save_registration(&self, r: &NewRegistration<'_>) -> Result<()> {
        let (id, username, display, avatar, name) = (
            r.discord_id.to_string(),
            r.username.map(str::to_string),
            r.display_name.map(str::to_string),
            r.avatar.map(str::to_string),
            r.key_name.to_string(),
        );
        let (key_id, now, member_role) = (r.key_id, r.now, r.member_role);
        self.write(move |tx| {
            tx.execute(
                "INSERT INTO discord_registrations(discord_id,key_id,created_at,username,display_name,avatar) \
                 VALUES (?,?,?,?,?,?)",
                params![id, key_id, now, username, display, avatar],
            )?;
            tx.execute("UPDATE api_keys SET name=? WHERE id=?", params![name, key_id])?;
            tx.execute(
                "UPDATE api_keys SET discord_id=? WHERE id=?",
                params![id, key_id],
            )?;
            mark_role_granted(tx, &id, member_role)
        })
        .await
    }

    // ------------------------------------------------------------------ 成员反馈

    /// 这个人最近一份反馈的时间。
    pub async fn last_feedback_ts(&self, discord_id: &str) -> Result<Option<f64>> {
        let id = discord_id.to_string();
        self.read(move |c| {
            c.query_row(
                "SELECT MAX(ts) FROM feedback WHERE discord_id=?",
                [id],
                |r| value::f64_opt(r, 0),
            )
        })
        .await
    }

    /// 存一份反馈（`answers` 是已序列化的 JSON）。
    pub async fn insert_feedback(
        &self,
        ts: f64,
        discord_id: &str,
        username: &str,
        key_id: Option<i64>,
        answers: &str,
    ) -> Result<()> {
        let (id, username, answers) = (
            discord_id.to_string(),
            username.to_string(),
            answers.to_string(),
        );
        self.write(move |tx| {
            tx.execute(
                "INSERT INTO feedback (ts, discord_id, username, key_id, answers) VALUES (?,?,?,?,?)",
                params![ts, id, username, key_id, answers],
            )?;
            Ok(())
        })
        .await
    }

    /// 后台查看：新 → 旧，附 Key 名字（列顺序：id, ts, discord_id, username, key_id, answers, name）。
    pub async fn list_feedback(&self, limit: i64) -> Result<Vec<Value>> {
        self.read(move |c| {
            let mut stmt = c.prepare(
                "SELECT f.id, f.ts, f.discord_id, f.username, f.key_id, f.answers, k.name FROM feedback f \
                 LEFT JOIN api_keys k ON k.id=f.key_id ORDER BY f.ts DESC LIMIT ?",
            )?;
            stmt.query_map([limit], value::row_json)?.collect()
        })
        .await
    }

    /// 请求编号只在日志所属 Key 当前归属或已删除 Key 墓碑归属一致时才视为本人请求。
    pub async fn feedback_request_belongs_to(&self, discord_id: &str, rid: &str) -> Result<bool> {
        let (discord_id, rid) = (discord_id.to_string(), rid.to_string());
        self.read(move |c| {
            c.query_row(
                "SELECT EXISTS(\
                    SELECT 1 FROM usage_log u JOIN api_keys k ON k.id=u.key_id \
                    WHERE u.rid=?1 AND k.discord_id=?2 \
                    UNION ALL \
                    SELECT 1 FROM usage_log u JOIN deleted_key_owners d ON d.key_id=u.key_id \
                    WHERE u.rid=?1 AND d.discord_id=?2\
                )",
                params![rid, discord_id],
                |r| r.get::<_, bool>(0),
            )
        })
        .await
    }

    /// 反馈审核标记与 answers JSON 一起更新，避免额外 schema 迁移。
    pub async fn set_feedback_reviewed(
        &self,
        id: i64,
        reviewed_at: Option<f64>,
        reviewed_by: &str,
    ) -> Result<bool> {
        let reviewed_by = reviewed_by.to_string();
        self.write(move |tx| {
            let Some(raw) = tx
                .query_row("SELECT answers FROM feedback WHERE id=?", [id], |r| {
                    r.get::<_, String>(0)
                })
                .optional()?
            else {
                return Ok(false);
            };
            let mut answers = serde_json::from_str::<Value>(&raw)
                .ok()
                .and_then(|v| v.as_object().cloned())
                .unwrap_or_default();
            answers.insert(
                "reviewed_at".into(),
                reviewed_at.map(Value::from).unwrap_or(Value::Null),
            );
            answers.insert(
                "reviewed_by".into(),
                if reviewed_at.is_some() {
                    Value::String(reviewed_by.clone())
                } else {
                    Value::Null
                },
            );
            tx.execute(
                "UPDATE feedback SET answers=? WHERE id=?",
                params![
                    serde_json::to_string(&Value::Object(answers)).unwrap_or_else(|_| "{}".into()),
                    id
                ],
            )?;
            Ok(true)
        })
        .await
    }
}

/// 配了身份组：清掉旧的待摘除，标记 `role_granted=1`。
fn mark_role_granted(
    tx: &rusqlite::Transaction<'_>,
    discord_id: &str,
    member_role: bool,
) -> rusqlite::Result<()> {
    if member_role {
        tx.execute(
            "DELETE FROM pending_role_removals WHERE discord_id=?",
            [discord_id],
        )?;
        tx.execute(
            "UPDATE discord_registrations SET role_granted=1 WHERE discord_id=?",
            [discord_id],
        )?;
    }
    Ok(())
}
