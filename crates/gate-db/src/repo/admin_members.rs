//! 后台「成员」接口用到的裸 SQL（旧版 `app/admin.py` 里直接用 `db._db` 写的那些）：
//! 邀请 Key、标签、手动绑定 Discord、成员页聚合、全站重置。
//!
//! 已删除功能（`anlas_auto` 自动分配、`v5_pinned`）的列仍留在表里，这里一律不读不写。

use super::{INVITE_TAG, SqlValue};
use crate::{Db, Result, value};
use rusqlite::{OptionalExtension, params};

/// `discord_registrations` 的一行（成员页展示用；用户名 / 头像可能为 NULL）。
#[derive(Debug, Clone, PartialEq)]
pub struct RegistrationRow {
    pub key_id: i64,
    pub discord_id: String,
    pub username: Option<String>,
    pub display_name: Option<String>,
    pub avatar: Option<String>,
    pub created_at: Option<f64>,
}

const REG_COLS: &str = "key_id, discord_id, username, display_name, avatar, created_at";

fn reg_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<RegistrationRow> {
    Ok(RegistrationRow {
        key_id: value::i64_or(r, 0, 0)?,
        // 旧版 `str(row[1])`
        discord_id: value::text_or(r, 1, "None")?,
        username: value::text_opt(r, 2)?,
        display_name: value::text_opt(r, 3)?,
        avatar: value::text_opt(r, 4)?,
        created_at: value::f64_opt(r, 5)?,
    })
}

/// `bind_discord` 的结果。
#[derive(Debug, Clone, PartialEq)]
pub enum BindOutcome {
    /// 这个 Discord 账号已经绑在别的 Key 上（给出那把 Key 的 id）。
    Taken(i64),
    /// 绑定完成，附带落库后这把 Key 的领取记录。
    Bound(Option<RegistrationRow>),
}

/// `bind_discord` 要写进去的资料。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BindProfile {
    pub username: String,
    pub display_name: String,
    /// `Some` = 从 Discord 读到的头像 hash（空串 = 默认头像）；`None` = 没读到：
    /// 同一账号保留原头像，换成别的账号就清空（旧头像是上一个人的）。
    pub avatar: Option<String>,
    /// `Some` = 同时把这把 Key 的 `api_keys.name` 改成它。
    pub key_name: Option<String>,
}

/// 手动标签（`key_tags` 一行）。
#[derive(Debug, Clone, PartialEq)]
pub struct TagRow {
    pub key_id: i64,
    pub tag: String,
    pub note: String,
    pub by: String,
    pub ts: f64,
}

impl Db {
    /// 带「邀请」标签的 Key（后台 Key 列表据此标身份组）。
    pub async fn invite_key_ids(&self) -> Result<Vec<i64>> {
        self.read(|c| {
            let mut stmt = c.prepare("SELECT key_id FROM key_tags WHERE tag=?")?;
            stmt.query_map([INVITE_TAG], |r| value::i64_or(r, 0, 0))?
                .collect()
        })
        .await
    }

    /// 全部 Key 的名字（给新邀请 Key 编号用）。
    pub async fn key_names(&self) -> Result<Vec<String>> {
        self.read(|c| {
            let mut stmt = c.prepare("SELECT name FROM api_keys")?;
            stmt.query_map([], |r| value::text_or(r, 0, ""))?.collect()
        })
        .await
    }

    /// 打 / 改标签：同一把 Key 同名标签只留一行，改备注、操作者和时间。
    pub async fn upsert_key_tag(
        &self,
        key_id: i64,
        tag: &str,
        note: &str,
        by: &str,
        now: f64,
    ) -> Result<()> {
        let (tag, note, by) = (tag.to_string(), note.to_string(), by.to_string());
        self.write(move |tx| {
            tx.execute(
                "INSERT INTO key_tags(key_id, tag, note, by, ts) VALUES (?,?,?,?,?) \
                 ON CONFLICT(key_id, tag) DO UPDATE SET note=excluded.note, by=excluded.by, ts=excluded.ts",
                params![key_id, tag, note, by, now],
            )?;
            Ok(())
        })
        .await
    }

    /// 摘标签；没有这个标签返回 false。
    pub async fn delete_key_tag(&self, key_id: i64, tag: &str) -> Result<bool> {
        let tag = tag.to_string();
        self.write(move |tx| {
            let n = tx.execute(
                "DELETE FROM key_tags WHERE key_id=? AND tag=?",
                params![key_id, tag],
            )?;
            Ok(n > 0)
        })
        .await
    }

    /// 额度归属：1 = 动态额度算法管，-1 = 站长手动。
    pub async fn set_quota_auto(&self, key_id: i64, value: i64) -> Result<()> {
        self.write(move |tx| {
            tx.execute(
                "UPDATE api_keys SET quota_auto=? WHERE id=?",
                params![value, key_id],
            )?;
            Ok(())
        })
        .await
    }

    /// 管理员手动给 Key 绑定 / 换绑 Discord：一个 Discord 账号只能绑一把 Key；
    /// 这把 Key 已有绑定就改成新账号（保留领取时间），没有就新建一条。头像 / Key 名见 [`BindProfile`]。
    pub async fn bind_discord(
        &self,
        key_id: i64,
        discord_id: &str,
        profile: &BindProfile,
        now: f64,
    ) -> Result<BindOutcome> {
        let (discord_id, p) = (discord_id.to_string(), profile.clone());
        self.write(move |tx| {
            let holder: Option<i64> = tx
                .query_row(
                    "SELECT key_id FROM discord_registrations WHERE discord_id=?",
                    [&discord_id],
                    |r| value::i64_or(r, 0, 0),
                )
                .optional()?;
            if let Some(other) = holder.filter(|k| *k != key_id) {
                return Ok(BindOutcome::Taken(other));
            }
            let current: Option<(String, Option<String>)> = tx
                .query_row(
                    "SELECT discord_id, avatar FROM discord_registrations WHERE key_id=?",
                    [key_id],
                    |r| Ok((value::text_or(r, 0, "None")?, value::text_opt(r, 1)?)),
                )
                .optional()?;
            if let Some((old_id, old_avatar)) = current {
                let avatar = match &p.avatar {
                    Some(a) => Some(a.clone()),
                    None if old_id == discord_id => old_avatar,
                    None => Some(String::new()),
                };
                tx.execute(
                    "UPDATE discord_registrations SET discord_id=?, username=?, display_name=?, avatar=? WHERE key_id=?",
                    params![discord_id, p.username, p.display_name, avatar, key_id],
                )?;
            } else {
                tx.execute(
                    "INSERT INTO discord_registrations(discord_id, key_id, created_at, username, display_name, avatar) \
                     VALUES (?,?,?,?,?,?)",
                    params![
                        discord_id,
                        key_id,
                        now,
                        p.username,
                        p.display_name,
                        p.avatar.clone().unwrap_or_default()
                    ],
                )?;
            }
            if let Some(name) = &p.key_name {
                tx.execute("UPDATE api_keys SET name=? WHERE id=?", params![name, key_id])?;
            }
            tx.execute(
                "UPDATE api_keys SET discord_id=? WHERE id=?",
                params![discord_id, key_id],
            )?;
            let reg = tx
                .query_row(
                    &format!("SELECT {REG_COLS} FROM discord_registrations WHERE key_id=?"),
                    [key_id],
                    reg_row,
                )
                .optional()?;
            Ok(BindOutcome::Bound(reg))
        })
        .await
    }

    /// 这把 Key 的领取记录（`key_id` 唯一，至多一行）：属于哪个 Discord 账号、闲置回收拉黑时带上的资料。
    pub async fn registration_for_key(&self, key_id: i64) -> Result<Option<RegistrationRow>> {
        self.read(move |c| {
            c.query_row(
                &format!("SELECT {REG_COLS} FROM discord_registrations WHERE key_id=?"),
                [key_id],
                reg_row,
            )
            .optional()
        })
        .await
    }

    /// 全部领取记录（成员页按 Key 对应 Discord 资料）。
    pub async fn registration_rows(&self) -> Result<Vec<RegistrationRow>> {
        self.read(|c| {
            let mut stmt = c.prepare(&format!("SELECT {REG_COLS} FROM discord_registrations"))?;
            stmt.query_map([], reg_row)?.collect()
        })
        .await
    }

    /// Key 临时暂停表（`share_state`）里的 (key_id, paused_until)，包含已过期的行。
    pub async fn pause_rows(&self) -> Result<Vec<(i64, f64)>> {
        self.read(|c| {
            let mut stmt = c.prepare("SELECT key_id, paused_until FROM share_state")?;
            stmt.query_map([], |r| {
                Ok((value::i64_or(r, 0, 0)?, value::f64_or(r, 1, 0.0)?))
            })?
            .collect()
        })
        .await
    }

    /// 全部手动标签，按打标签时间升序。
    pub async fn key_tag_rows(&self) -> Result<Vec<TagRow>> {
        self.read(|c| {
            let mut stmt =
                c.prepare("SELECT key_id, tag, note, by, ts FROM key_tags ORDER BY ts")?;
            stmt.query_map([], |r| {
                Ok(TagRow {
                    key_id: value::i64_or(r, 0, 0)?,
                    tag: value::text_or(r, 1, "")?,
                    note: value::text_or(r, 2, "")?,
                    by: value::text_or(r, 3, "")?,
                    ts: value::f64_or(r, 4, 0.0)?,
                })
            })?
            .collect()
        })
        .await
    }

    /// 自动标签：`since` 之后被拒 / 失败的请求里，`detail` 命中 `patterns`（(标签, LIKE 模式)，先命中先得）的次数。
    /// 返回 (key_id, 标签, 次数)。
    pub async fn auto_tag_counts(
        &self,
        since: f64,
        patterns: &[(&str, &str)],
    ) -> Result<Vec<(i64, String, i64)>> {
        let lit = |s: &str| s.replace('\'', "''");
        let whens: Vec<String> = patterns
            .iter()
            .map(|(label, pat)| format!("WHEN detail LIKE '{}' THEN '{}'", lit(pat), lit(label)))
            .collect();
        let sql = format!(
            "SELECT key_id, CASE {} END AS label, COUNT(*) FROM usage_log WHERE ts>? \
             AND status IN ('rejected','error') AND key_id IS NOT NULL \
             GROUP BY key_id, label HAVING label IS NOT NULL",
            whens.join(" ")
        );
        self.read(move |c| {
            let mut stmt = c.prepare(&sql)?;
            stmt.query_map([since], |r| {
                Ok((
                    value::i64_or(r, 0, 0)?,
                    value::text_or(r, 1, "")?,
                    value::i64_or(r, 2, 0)?,
                ))
            })?
            .collect()
        })
        .await
    }

    /// `since` 之后成功的「重置今日额度」操作：(时间, 对象)。
    pub async fn quota_reset_marks(&self, since: f64) -> Result<Vec<(f64, String)>> {
        self.read(move |c| {
            let mut stmt = c.prepare(
                "SELECT ts, target FROM admin_actions WHERE action='重置今日额度' AND ok=1 AND ts>=?",
            )?;
            stmt.query_map([since], |r| {
                Ok((value::f64_or(r, 0, 0.0)?, value::text_or(r, 1, "")?))
            })?
            .collect()
        })
        .await
    }

    /// 某服务日每把 Key 的原始计数 (key_id, images, v5, anlas, text_tokens, requests)（不扣「重置额度」基线）。
    pub async fn counter_rows(&self, day: &str) -> Result<Vec<(i64, i64, i64, f64, i64, i64)>> {
        let day = day.to_string();
        self.read(move |c| {
            let mut stmt = c.prepare(
                "SELECT key_id, images, v5, anlas, text_tokens, requests FROM counters WHERE day=?",
            )?;
            stmt.query_map([day], |r| {
                Ok((
                    value::i64_or(r, 0, 0)?,
                    value::i64_or(r, 1, 0)?,
                    value::i64_or(r, 2, 0)?,
                    value::f64_or(r, 3, 0.0)?,
                    value::i64_or(r, 4, 0)?,
                    value::i64_or(r, 5, 0)?,
                ))
            })?
            .collect()
        })
        .await
    }

    /// 生图速率：`hour0` 起每把 Key 逐小时的成功生图数 (key_id, 第几个小时, 张数)。
    pub async fn hourly_image_counts(&self, hour0: i64) -> Result<Vec<(i64, i64, i64)>> {
        self.read(move |c| {
            let mut stmt = c.prepare(
                "SELECT key_id, CAST((ts - ?) / 3600 AS INT) b, COUNT(*) FROM usage_log \
                 WHERE ts >= ? AND status='ok' AND (kind LIKE 'image%' OR kind IN ('upscale','augment-image','vibe_encode')) \
                 GROUP BY key_id, b",
            )?;
            let rows = stmt.query_map(params![hour0, hour0], |r| {
                Ok((
                    value::i64_opt(r, 0)?,
                    value::i64_or(r, 1, 0)?,
                    value::i64_or(r, 2, 0)?,
                ))
            })?;
            let mut out = Vec::new();
            for row in rows {
                let (key_id, bucket, n) = row?;
                // key_id 为空的成功生图不存在（成功日志一定有 Key），跳过以防万一
                if let Some(k) = key_id {
                    out.push((k, bucket, n));
                }
            }
            Ok(out)
        })
        .await
    }

    /// 全站重置：失效所有非「邀请」的 Key，清掉它们的 Discord 绑定和（非邀请）标签，清空防分享状态。
    /// 带「邀请」标签的 Key 原样保留。返回 (失效的 Key 数, 保留的邀请标签数)。
    pub async fn reset_all_keys(&self) -> Result<(i64, i64)> {
        self.write(|tx| {
            let kept: i64 = tx.query_row(
                "SELECT COUNT(*) FROM key_tags WHERE tag=?",
                [INVITE_TAG],
                |r| value::i64_or(r, 0, 0),
            )?;
            let disabled = tx.execute(
                "UPDATE api_keys SET enabled=0, disabled_reason=? WHERE NOT EXISTS \
                 (SELECT 1 FROM key_tags t WHERE t.key_id=api_keys.id AND t.tag=?)",
                params![gate_core::key_status::SITE_RESET, INVITE_TAG],
            )? as i64;
            tx.execute(
                "DELETE FROM discord_registrations WHERE NOT EXISTS \
                 (SELECT 1 FROM key_tags t WHERE t.key_id=discord_registrations.key_id AND t.tag=?)",
                [INVITE_TAG],
            )?;
            tx.execute(
                "DELETE FROM key_tags WHERE tag!=? \
                 AND NOT EXISTS (SELECT 1 FROM key_tags t2 WHERE t2.key_id=key_tags.key_id AND t2.tag=?)",
                params![INVITE_TAG, INVITE_TAG],
            )?;
            tx.execute("DELETE FROM share_state", [])?;
            Ok((disabled, kept))
        })
        .await
    }

    /// 按 id 批量取 Key 名字 (id, name)。
    pub async fn key_names_for(&self, ids: &[i64]) -> Result<Vec<(i64, String)>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let args: Vec<SqlValue> = ids.iter().map(|i| SqlValue::Integer(*i)).collect();
        let sql = format!(
            "SELECT id, name FROM api_keys WHERE id IN ({})",
            super::marks(ids.len())
        );
        self.read(move |c| {
            let mut stmt = c.prepare(&sql)?;
            stmt.query_map(rusqlite::params_from_iter(args.iter()), |r| {
                Ok((value::i64_or(r, 0, 0)?, value::text_or(r, 1, "")?))
            })?
            .collect()
        })
        .await
    }
}
