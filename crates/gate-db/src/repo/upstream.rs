//! 上游 Token 的每日计数与逐 Token 设置（V5 日上限 / 启停 / 出图并发）。

use super::OrderedPairs;
use crate::{Db, Result, value};
use rusqlite::params;
use serde::Serialize;

/// `get_upstream_counter` 的结果。表里的 `units` 列（节约模式遗留）不再读也不再写，列本身保留。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct UpstreamCounter {
    pub images: i64,
    pub v5: i64,
}

pub const POOL_REVISION: &str = "upstream_token_pool_revision";

fn migrate_token(
    tx: &rusqlite::Transaction<'_>,
    old_id: &str,
    new_id: &str,
) -> rusqlite::Result<()> {
    if old_id == new_id {
        return Ok(());
    }
    for table in [
        "upstream_token_settings",
        "upstream_token_enabled",
        "upstream_token_counters",
        "upstream_anlas_charges",
    ] {
        tx.execute(&format!("DELETE FROM {table} WHERE token_id=?"), [new_id])?;
        tx.execute(
            &format!("UPDATE {table} SET token_id=? WHERE token_id=?"),
            params![new_id, old_id],
        )?;
    }
    tx.execute(
        "UPDATE upstream_v5_reservations SET token_id=? WHERE token_id=?",
        params![new_id, old_id],
    )?;
    Ok(())
}

impl Db {
    /// 已确认的账号免费 V5 消耗，加上已派发但尚未确定的持久预留。
    pub async fn upstream_v5_used_and_held(&self, token: &str, day: &str) -> Result<i64> {
        let (token, day) = (token.to_owned(), day.to_owned());
        self.read(move |c| c.query_row(
            "SELECT COALESCE((SELECT v5 FROM upstream_token_counters WHERE token_id=?1 AND day=?2),0) + \
             (SELECT COUNT(*) FROM upstream_v5_reservations h JOIN request_settlements s ON s.request_id=h.request_id \
              WHERE (h.token_id=?1 OR h.token_id='') AND h.day=?2 AND s.status IN ('dispatched','result_saved','uncertain'))",
            params![token,day], |r| r.get(0))).await
    }

    /// 管理界面同一快照中读取已确认计数与持久预留。
    pub async fn upstream_counter_with_holds(
        &self,
        token: &str,
        day: &str,
    ) -> Result<(UpstreamCounter, i64)> {
        let (token, day) = (token.to_owned(), day.to_owned());
        self.read(move |c| c.query_row(
            "SELECT COALESCE((SELECT images FROM upstream_token_counters WHERE token_id=?1 AND day=?2),0), \
                    COALESCE((SELECT v5 FROM upstream_token_counters WHERE token_id=?1 AND day=?2),0), \
                    (SELECT COUNT(*) FROM upstream_v5_reservations h JOIN request_settlements s ON s.request_id=h.request_id \
                     WHERE (h.token_id=?1 OR h.token_id='') AND h.day=?2 AND s.status IN ('dispatched','result_saved','uncertain'))",
            params![token,day], |r| Ok((UpstreamCounter { images:r.get(0)?, v5:r.get(1)? },r.get(2)?)))).await
    }
    /// Includes unresolved dispatches. A repeated confirmation never reserves twice.
    pub async fn reserve_upstream_anlas(
        &self,
        request: &str,
        token: &str,
        day: &str,
        cap: i64,
        enabled: bool,
    ) -> Result<bool> {
        let (request, token, day) = (request.to_owned(), token.to_owned(), day.to_owned());
        self.write(move |tx| {
            let cost: f64 = tx.query_row("SELECT anlas + site_paid_hold FROM request_settlements WHERE request_id=?", [&request], |r| r.get(0))?;
            if cost <= 0.0 { return Ok(true); }
            if !enabled { return Ok(false); }
            let existing: f64 = tx.query_row("SELECT COALESCE(SUM(amount),0) FROM upstream_anlas_charges WHERE request_id=? AND token_id=?", params![request,token], |r| r.get(0))?;
            if existing >= cost { return Ok(true); }
            let used: f64 = tx.query_row("SELECT COALESCE(SUM(amount),0) FROM upstream_anlas_charges WHERE token_id=? AND day=?", params![token,day], |r| r.get(0))?;
            if cap < gate_core::UNLIMITED && used - existing + cost > cap as f64 { return Ok(false); }
            tx.execute("INSERT INTO upstream_anlas_charges(request_id,token_id,day,amount) VALUES(?,?,?,?) ON CONFLICT(request_id,token_id) DO UPDATE SET amount=excluded.amount", params![request,token,day,cost])?;
            Ok(true)
        }).await
    }

    pub async fn upstream_anlas_used(&self, token: &str, day: &str) -> Result<f64> {
        let (token, day) = (token.to_owned(), day.to_owned());
        self.read(move |c| c.query_row("SELECT COALESCE(SUM(amount),0) FROM upstream_anlas_charges WHERE token_id=? AND day=?", params![token,day], |r| r.get(0))).await
    }

    pub async fn get_upstream_counter(&self, token_id: &str, day: &str) -> Result<UpstreamCounter> {
        let (token_id, day) = (token_id.to_string(), day.to_string());
        self.read(move |c| {
            let mut stmt = c.prepare(
                "SELECT images, v5 FROM upstream_token_counters WHERE token_id=? AND day=?",
            )?;
            let mut rows = stmt.query(params![token_id, day])?;
            let Some(row) = rows.next()? else {
                return Ok(UpstreamCounter { images: 0, v5: 0 });
            };
            Ok(UpstreamCounter {
                images: value::i64_or(row, "images", 0)?,
                v5: value::i64_or(row, "v5", 0)?,
            })
        })
        .await
    }

    /// 令牌被替换（同一账号重置了 Token）：把设置和计数从旧标识迁移到新标识。
    pub async fn move_upstream_token(&self, old_id: &str, new_id: &str) -> Result<()> {
        let (old, new) = (old_id.to_owned(), new_id.to_owned());
        self.write(move |tx| migrate_token(tx, &old, &new)).await
    }

    /// Store only fingerprints and the commit revision in SQLite. The actual
    /// credentials have already been fsynced in the private token file.
    pub async fn commit_token_pool(
        &self,
        revision: String,
        replaced: Option<(String, String)>,
        quarantine: Option<String>,
    ) -> Result<()> {
        self.write_raw(move |conn| {
            // The credential file is fsynced. Its matching SQLite commit must
            // survive power loss too, prior to removing the previous credentials.
            conn.pragma_update(None, "synchronous", "FULL")?;
            let result = (|| {
                let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
                if let Some((old, new)) = &replaced {
                    migrate_token(&tx, old, new)?;
                }
                if let Some(value) = &quarantine {
                    tx.execute(
                        "INSERT OR REPLACE INTO site_settings(key,value) VALUES ('upstream_invalid_token_fingerprints',?)", [value],
                    )?;
                }
                tx.execute(
                    "INSERT OR REPLACE INTO site_settings(key,value) VALUES (?,?)",
                    [POOL_REVISION, &revision],
                )?;
                tx.commit()
            })();
            if let Err(e) = conn.pragma_update(None, "synchronous", "NORMAL") {
                tracing::warn!("无法恢复 SQLite 同步级别：{e}");
            }
            result
        }).await
    }

    pub async fn get_upstream_token_limits(&self) -> Result<OrderedPairs<String, i64>> {
        self.read(|c| {
            let mut stmt =
                c.prepare("SELECT token_id, v5_daily_limit FROM upstream_token_settings")?;
            stmt.query_map([], |r| {
                Ok((value::text_or(r, 0, "")?, value::i64_or(r, 1, 0)?))
            })?
            .collect()
        })
        .await
    }

    pub async fn set_upstream_token_limit(&self, token_id: &str, limit: i64) -> Result<()> {
        let token_id = token_id.to_string();
        self.write(move |tx| {
            tx.execute(
                "INSERT INTO upstream_token_settings(token_id, v5_daily_limit) VALUES(?,?) \
                 ON CONFLICT(token_id) DO UPDATE SET v5_daily_limit=excluded.v5_daily_limit",
                params![token_id, limit],
            )?;
            Ok(())
        })
        .await
    }

    pub async fn get_upstream_token_enabled(&self) -> Result<OrderedPairs<String, bool>> {
        self.read(|c| {
            let mut stmt = c.prepare("SELECT token_id, enabled FROM upstream_token_enabled")?;
            stmt.query_map([], |r| Ok((value::text_or(r, 0, "")?, value::flag(r, 1)?)))?
                .collect()
        })
        .await
    }

    pub async fn set_upstream_token_enabled(&self, token_id: &str, enabled: bool) -> Result<()> {
        let token_id = token_id.to_string();
        self.write(move |tx| {
            tx.execute(
                "INSERT INTO upstream_token_enabled(token_id, enabled) VALUES(?,?) \
                 ON CONFLICT(token_id) DO UPDATE SET enabled=excluded.enabled",
                params![token_id, i64::from(enabled)],
            )?;
            Ok(())
        })
        .await
    }

    /// 上游 Token 当日出图张数累加。`images < 1` 什么都不做。
    pub async fn bump_upstream_image_counter(
        &self,
        token_id: &str,
        day: &str,
        images: i64,
    ) -> Result<()> {
        if images < 1 {
            return Ok(());
        }
        let (token_id, day) = (token_id.to_string(), day.to_string());
        self.write(move |tx| {
            tx.execute(
                "INSERT INTO upstream_token_counters(token_id, day, images) VALUES (?,?,?) \
                 ON CONFLICT(token_id, day) DO UPDATE SET images=images+excluded.images",
                params![token_id, day, images],
            )?;
            Ok(())
        })
        .await
    }

    pub async fn bump_upstream_v5_counter(&self, token_id: &str, day: &str) -> Result<()> {
        let (token_id, day) = (token_id.to_string(), day.to_string());
        self.write(move |tx| {
            tx.execute(
                "INSERT INTO upstream_token_counters(token_id, day, v5) VALUES (?,?,1) \
                 ON CONFLICT(token_id, day) DO UPDATE SET v5=v5+1",
                params![token_id, day],
            )?;
            Ok(())
        })
        .await
    }

    /// 每个上游账号最近一条快照的余额读数：`(token_id, ts, anlas, v5_percent)`。
    /// 只读历史数据，不向上游发请求——这就是「上游」页展示余额的唯一来源。
    pub async fn latest_snapshots(&self) -> Result<Vec<(String, f64, Option<f64>, Option<f64>)>> {
        self.read(|c| {
            let mut stmt = c.prepare(
                "SELECT s.token_id, s.ts, s.anlas, s.v5_percent FROM upstream_snapshots s \
                 JOIN (SELECT token_id, MAX(ts) AS ts FROM upstream_snapshots \
                       WHERE token_id!='' GROUP BY token_id) m \
                   ON s.token_id=m.token_id AND s.ts=m.ts \
                 ORDER BY s.token_id",
            )?;
            stmt.query_map([], |r| {
                Ok((
                    value::text_or(r, 0, "")?,
                    value::f64_or(r, 1, 0.0)?,
                    value::f64_opt(r, 2)?,
                    value::f64_opt(r, 3)?,
                ))
            })?
            .collect()
        })
        .await
    }
}
