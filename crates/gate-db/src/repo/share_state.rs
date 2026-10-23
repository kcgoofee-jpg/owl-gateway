//! Key 临时暂停（`share_state` 表）：自动驾驶等模块对单把 Key 的暂停，只用 `paused_until` / `pause_reason` 两列。
//! 表名与风险分等旧列是 2.x 防分享（`app/share_guard.py`）留下的，兼容旧库原样保留，3.0 不读不写。

use crate::{Db, Result};

impl Db {
    /// 仍在暂停中的 Key：(key_id, paused_until, pause_reason)。
    pub async fn active_pauses(&self, now: f64) -> Result<Vec<(i64, f64, String)>> {
        self.read(move |c| {
            let mut stmt = c.prepare(
                "SELECT key_id, paused_until, pause_reason FROM share_state WHERE paused_until > ?",
            )?;
            let rows = stmt.query_map([now], |r| {
                Ok((
                    crate::value::i64_or(r, 0, 0)?,
                    crate::value::f64_or(r, 1, 0.0)?,
                    crate::value::text_or(r, 2, "")?,
                ))
            })?;
            rows.collect()
        })
        .await
    }

    pub async fn upsert_pause(&self, key_id: i64, until: f64, reason: &str) -> Result<()> {
        let reason = reason.to_string();
        self.write(move |tx| {
            tx.execute(
                "INSERT INTO share_state(key_id, paused_until, pause_reason) VALUES (?,?,?) \
                 ON CONFLICT(key_id) DO UPDATE SET paused_until=excluded.paused_until, pause_reason=excluded.pause_reason",
                rusqlite::params![key_id, until, reason],
            )?;
            Ok(())
        })
        .await
    }

    pub async fn clear_pause(&self, key_id: i64) -> Result<()> {
        self.write(move |tx| {
            tx.execute("DELETE FROM share_state WHERE key_id=?", [key_id])?;
            Ok(())
        })
        .await
    }
}
