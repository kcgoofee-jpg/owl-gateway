//! 成员的券（现在只有「重置券」）。

use crate::{Db, Result, value};
use gate_core::primitives;
use rusqlite::{OptionalExtension, params};
use serde::Serialize;
use serde_json::Value;

/// `add_coupon` 返回的对象（键顺序固定）。
#[derive(Debug, Clone, Serialize)]
pub struct NewCoupon {
    pub id: i64,
    pub key_id: i64,
    pub kind: String,
    /// 已截到 200 个字符。
    pub note: String,
    pub created_at: f64,
    pub expires_at: f64,
    pub used_at: Option<f64>,
    /// 重置哪几项：`v45` / `v45,v5` / `v45,anlas` / `v45,v5,anlas`。
    pub scope: String,
}

impl Db {
    /// 发券。`note[:200]`、`by[:40]` 按码点截断；过期时间 = `now + days * 86400`（days 可为负，直接发出一张过期券）。
    #[allow(clippy::too_many_arguments)]
    pub async fn add_coupon(
        &self,
        key_id: i64,
        kind: &str,
        days: f64,
        note: &str,
        by: &str,
        scope: &str,
        now: f64,
    ) -> Result<NewCoupon> {
        let scope = scope.to_string();
        let kind = kind.to_string();
        let note = primitives::take_chars(note, 200).to_string();
        let by = primitives::take_chars(by, 40).to_string();
        let expires_at = now + days * 86400.0;
        self.write(move |tx| {
            tx.execute(
                "INSERT INTO coupons(key_id, kind, note, by, created_at, expires_at, scope) VALUES (?,?,?,?,?,?,?)",
                params![key_id, kind, note, by, now, expires_at, scope],
            )?;
            Ok(NewCoupon {
                id: tx.last_insert_rowid(),
                key_id,
                kind: kind.clone(),
                note: note.clone(),
                created_at: now,
                expires_at,
                used_at: None,
                scope: scope.clone(),
            })
        })
        .await
    }

    /// 某个 Key 的券；`active_only` 时只给没用过且没过期的。按过期时间升序。
    pub async fn list_coupons(
        &self,
        key_id: i64,
        active_only: bool,
        now: f64,
    ) -> Result<Vec<Value>> {
        self.read(move |c| {
            let mut sql =
                "SELECT id, kind, note, by, created_at, expires_at, used_at, scope FROM coupons WHERE key_id=?"
                    .to_string();
            if active_only {
                sql.push_str(" AND used_at IS NULL AND expires_at>?");
            }
            sql.push_str(" ORDER BY expires_at");
            let mut stmt = c.prepare(&sql)?;
            if active_only {
                stmt.query_map(params![key_id, now], value::row_json)?.collect()
            } else {
                stmt.query_map(params![key_id], value::row_json)?.collect()
            }
        })
        .await
    }

    /// 标记用掉（只能用一次、要没过期、要是自己的）。成功返回这张券，否则 None。
    pub async fn use_coupon(&self, coupon_id: i64, key_id: i64, now: f64) -> Result<Option<Value>> {
        self.write(move |tx| {
            let n = tx.execute(
                "UPDATE coupons SET used_at=? WHERE id=? AND key_id=? AND used_at IS NULL AND expires_at>?",
                params![now, coupon_id, key_id, now],
            )?;
            if n != 1 {
                return Ok(None);
            }
            tx.query_row(
                "SELECT id, kind, note, expires_at, used_at, scope FROM coupons WHERE id=?",
                [coupon_id],
                value::row_json,
            )
            .optional()
        })
        .await
    }
}
