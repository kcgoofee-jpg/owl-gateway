//! 虚拟 Key：增删改查、闲置判定、身份组。

use super::{INVITE_TAG, ROLE_INVITE, ROLE_REGULAR, SqlValue, json_to_sql};
use crate::{Db, DbError, Result, value};
use gate_core::{primitives, values};
use rusqlite::{OptionalExtension, params};
use serde::Deserialize;
use serde_json::Value;

fn yes() -> bool {
    true
}

fn legacy() -> String {
    "legacy".into()
}

/// `create_key` 的入参。缺省值见各字段；
/// 必填项（name / token / daily_images / monthly_anlas / daily_text_tokens / rpm）缺了返回 Err。
#[derive(Debug, Clone, Deserialize)]
pub struct NewKey {
    pub name: String,
    pub token: String,
    #[serde(default = "yes")]
    pub enabled: bool,
    pub daily_images: i64,
    #[serde(default)]
    pub daily_anlas: f64,
    #[serde(default)]
    pub daily_v5: i64,
    pub monthly_anlas: f64,
    pub daily_text_tokens: i64,
    pub rpm: i64,
    #[serde(default)]
    pub allow_anlas: bool,
    #[serde(default)]
    pub allow_img2img: bool,
    #[serde(default)]
    pub exclude_global_v5: bool,
    #[serde(default = "legacy")]
    pub image_model_scope: String,
    #[serde(default)]
    pub expires_at: Option<f64>,
    #[serde(default)]
    pub features: Option<String>,
    #[serde(default)]
    pub discord_id: Option<String>,
    #[serde(default)]
    pub quota_limits: Option<String>,
}

/// `update_key` 允许改的列（顺序无关，只是白名单）。
const UPDATE_ALLOWED: [&str; 15] = [
    "name",
    "enabled",
    "daily_images",
    "daily_anlas",
    "daily_v5",
    "monthly_anlas",
    "daily_text_tokens",
    "rpm",
    "allow_anlas",
    "allow_img2img",
    "exclude_global_v5",
    "image_model_scope",
    "expires_at",
    "features",
    "quota_limits",
];
const UPDATE_FLAGS: [&str; 4] = [
    "enabled",
    "allow_anlas",
    "allow_img2img",
    "exclude_global_v5",
];

impl Db {
    /// 新建 Key 并返回刚建好的整行（`created_at` = `now`）。
    pub async fn create_key(&self, f: &NewKey, now: f64) -> Result<Value> {
        let f = f.clone();
        self.write(move |tx| {
            tx.execute(
                "INSERT INTO api_keys \
                 (name, token, enabled, daily_images, daily_anlas, daily_v5, monthly_anlas, \
                 daily_text_tokens, rpm, allow_anlas, allow_img2img, exclude_global_v5, image_model_scope, expires_at, created_at, features, \
                 discord_id, quota_limits) \
                 VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
                params![
                    f.name,
                    f.token,
                    i64::from(f.enabled),
                    f.daily_images,
                    f.daily_anlas,
                    f.daily_v5,
                    f.monthly_anlas,
                    f.daily_text_tokens,
                    f.rpm,
                    i64::from(f.allow_anlas),
                    i64::from(f.allow_img2img),
                    i64::from(f.exclude_global_v5),
                    f.image_model_scope,
                    f.expires_at,
                    now,
                    f.features,
                    f.discord_id,
                    f.quota_limits,
                ],
            )?;
            let id = tx.last_insert_rowid();
            tx.query_row(
                "SELECT * FROM api_keys WHERE id=?",
                [id],
                value::row_json,
            )
        })
        .await
    }

    pub async fn get_key(&self, key_id: i64) -> Result<Option<Value>> {
        self.read(move |c| {
            c.query_row(
                "SELECT * FROM api_keys WHERE id=?",
                [key_id],
                value::row_json,
            )
            .optional()
        })
        .await
    }

    pub async fn get_key_by_token(&self, token: &str) -> Result<Option<Value>> {
        let token = token.to_string();
        self.read(move |c| {
            c.query_row(
                "SELECT * FROM api_keys WHERE token=?",
                [&token],
                value::row_json,
            )
            .optional()
        })
        .await
    }

    /// 换 token 但保留 id（在途请求、用量和限速仍归属于它）。token 撞了 UNIQUE 约束时返回 Err。
    pub async fn rotate_key_token(&self, key_id: i64, token: &str) -> Result<bool> {
        let token = token.to_string();
        self.write(move |tx| {
            let n = tx.execute(
                "UPDATE api_keys SET token=? WHERE id=?",
                params![token, key_id],
            )?;
            Ok(n == 1)
        })
        .await
    }

    /// 其余原样绑定（bool 当整数；list / dict 无法绑定，返回 Err）。没有可改的列时什么都不做。
    pub async fn update_key(
        &self,
        key_id: i64,
        fields: &serde_json::Map<String, Value>,
    ) -> Result<()> {
        let mut sets = Vec::new();
        let mut vals: Vec<SqlValue> = Vec::new();
        for (k, v) in fields {
            if !UPDATE_ALLOWED.contains(&k.as_str()) {
                continue;
            }
            sets.push(format!("{k}=?"));
            if UPDATE_FLAGS.contains(&k.as_str()) {
                vals.push(SqlValue::Integer(i64::from(values::enabled(v))));
            } else {
                if k == "quota_limits" && (v.is_object() || v.is_array()) {
                    vals.push(SqlValue::Text(
                        serde_json::to_string(v).map_err(|e| DbError::Invalid(e.to_string()))?,
                    ));
                } else {
                    vals.push(json_to_sql(v)?);
                }
            }
        }
        if sets.is_empty() {
            return Ok(());
        }
        // 改启用状态时一并记停用原因（3.0 新增）：启用就清掉；停用时调用方可传 `disabled_reason`，不传按站长手动停用
        if let Some(v) = fields.get("enabled") {
            if values::enabled(v) {
                sets.push("disabled_reason=NULL".into());
            } else {
                sets.push("disabled_reason=?".into());
                let why = fields
                    .get("disabled_reason")
                    .and_then(Value::as_str)
                    .unwrap_or(gate_core::key_status::ADMIN);
                vals.push(SqlValue::Text(why.to_string()));
            }
        }
        vals.push(SqlValue::Integer(key_id));
        let sql = format!("UPDATE api_keys SET {} WHERE id=?", sets.join(", "));
        self.write(move |tx| {
            tx.execute(&sql, rusqlite::params_from_iter(vals.iter()))?;
            Ok(())
        })
        .await
    }

    /// 删 Key：触发器把 V5 标记归档进墓碑表并清掉当日额度基线；
    /// 计数器 / 日志保留（已受理的请求事后结算仍可写入）。
    /// 删之前把「Key → Discord 账号」记进 `deleted_key_owners`（周报追溯已删 Key 的日志用，保留 14 天）。
    pub async fn delete_key(&self, key_id: i64, now: f64) -> Result<()> {
        self.write(move |tx| {
            tx.execute(
                "INSERT INTO deleted_key_owners(key_id, discord_id, name, deleted_at) \
                 SELECT k.id, COALESCE(k.discord_id, \
                   (SELECT r.discord_id FROM discord_registrations r WHERE r.key_id=k.id)), k.name, ? \
                 FROM api_keys k WHERE k.id=? \
                 ON CONFLICT(key_id) DO UPDATE SET discord_id=COALESCE(excluded.discord_id, discord_id), \
                   name=excluded.name, deleted_at=excluded.deleted_at",
                params![now, key_id],
            )?;
            tx.execute("DELETE FROM key_tags WHERE key_id=?", [key_id])?;
            tx.execute("DELETE FROM member_inquiries WHERE key_id=?", [key_id])?;
            tx.execute("DELETE FROM discord_registrations WHERE key_id=?", [key_id])?;
            tx.execute("DELETE FROM api_keys WHERE id=?", [key_id])?;
            Ok(())
        })
        .await
    }

    /// 闲置 = 连续没生图（2026-10-13 站长）：只看 last_image_at，从没生过图的 Key 从创建时间起算；
    /// 邀请标签的 Key 豁免。「宽限起点」设置损坏（解析不了 / 非有限 / 负数）时返回空，绝不触发批量删除。
    ///
    /// 已过期的 Key（`expires_at <= now`）不算闲置：过期后请求一律 403、`last_image_at` 不会再更新，
    /// 它们照旧在成员下次 /register 时释放，不进封禁表。
    pub async fn inactive_key_ids(&self, cutoff: f64, now: f64) -> Result<Vec<i64>> {
        self.read(move |c| {
            let raw: Option<String> = c
                .query_row(
                    "SELECT value FROM site_settings WHERE key=?",
                    ["key_inactivity_grace_started_at"],
                    |r| value::text_opt(r, 0),
                )
                .optional()?
                .flatten();
            let grace = match raw {
                None => 0.0, // 缺省 0
                Some(s) => match primitives::parse_float(&s) {
                    Some(g) => g,
                    None => return Ok(Vec::new()),
                },
            };
            if !grace.is_finite() || grace < 0.0 {
                return Ok(Vec::new());
            }
            let mut stmt = c.prepare(
                "SELECT id FROM api_keys \
                 WHERE enabled=1 \
                   AND (expires_at IS NULL OR expires_at > ?) \
                   AND MAX(COALESCE(last_image_at, created_at), ?) < ? \
                   AND id NOT IN (SELECT key_id FROM key_tags WHERE tag=?) \
                 ORDER BY id ASC",
            )?;
            stmt.query_map(params![now, grace, cutoff, INVITE_TAG], |r| {
                value::i64_or(r, 0, 0)
            })?
            .collect()
        })
        .await
    }

    pub async fn list_keys(&self) -> Result<Vec<Value>> {
        self.read(|c| {
            let mut stmt = c.prepare("SELECT * FROM api_keys ORDER BY id DESC")?;
            stmt.query_map([], value::row_json)?.collect()
        })
        .await
    }

    pub async fn touch_key(&self, key_id: i64, now: f64) -> Result<()> {
        self.write(move |tx| {
            tx.execute(
                "UPDATE api_keys SET last_used_at=? WHERE id=?",
                params![now, key_id],
            )?;
            Ok(())
        })
        .await
    }

    /// 原子切换账号身份组：regular / invite（邀请标签，豁免闲置回收）。未知身份组在动库之前就报错。
    pub async fn set_key_role(&self, key_id: i64, role: &str, now: f64) -> Result<()> {
        let invite = match role {
            ROLE_INVITE => true,
            ROLE_REGULAR => false,
            other => return Err(DbError::Invalid(format!("未知身份组: {other}"))),
        };
        self.write(move |tx| {
            if invite {
                tx.execute(
                    "INSERT INTO key_tags(key_id, tag, note, by, ts) VALUES (?,?,?,?,?) \
                     ON CONFLICT(key_id, tag) DO UPDATE SET note=excluded.note, by=excluded.by, ts=excluded.ts",
                    params![
                        key_id,
                        INVITE_TAG,
                        "站长手动设为邀请账号，豁免闲置回收",
                        "站长",
                        now
                    ],
                )?;
            } else {
                tx.execute(
                    "DELETE FROM key_tags WHERE key_id=? AND tag=?",
                    params![key_id, INVITE_TAG],
                )?;
            }
            Ok(())
        })
        .await
    }
}
