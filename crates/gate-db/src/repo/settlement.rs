//! 请求结算：`request_settlements` 的全部状态转换（预留 → 派发 → 落账 / 释放 / 恢复 / 人工释放）。
//!
//! 状态机只在这里改：路由、任务、后台都调这些方法，不再自己写 UPDATE。用量日志的写入与单条查询在
//! `repo::usage`，统计聚合在 `repo::usage_stats`。

use super::usage::RequestSettlement;
use super::usage::{NewLog, Success, insert_usage_log, is_image_kind};
use super::{INSERT_LOG, UPSERT_COUNTERS};
use crate::{Db, Result};
use gate_core::primitives;
use rusqlite::{OptionalExtension, params};

impl Db {
    /// Create a durable reservation before a generation is allowed to leave the gateway.
    #[allow(clippy::too_many_arguments)]
    pub async fn reserve_request_settlement(
        &self,
        request_id: &str,
        rid: &str,
        key_id: i64,
        day: &str,
        month: &str,
        is_vibe: bool,
        anlas: f64,
        v5: i64,
        legacy: i64,
        now: f64,
    ) -> Result<()> {
        let (request_id, rid, day, month) = (
            request_id.to_owned(),
            rid.to_owned(),
            day.to_owned(),
            month.to_owned(),
        );
        self.write(move |tx| {
            tx.execute("INSERT INTO request_settlements(request_id,rid,key_id,status,day,month,anlas,v5,legacy,is_vibe,updated_at) VALUES(?,?,?,'reserved',?,?,?,?,?,?,?)",
                params![request_id, rid, key_id, day, month, anlas, v5, legacy, is_vibe, now])?;
            Ok(())
        }).await
    }

    /// Must commit before the upstream POST is sent. A failed write is a hard dispatch failure.
    #[allow(clippy::too_many_arguments)]
    /// Must commit before the upstream POST is sent. A failed write is a hard dispatch failure.
    #[allow(clippy::too_many_arguments)]
    pub async fn dispatch_request_settlement(
        &self,
        request_id: &str,
        correlation_id: &str,
        day: &str,
        month: &str,
        is_vibe: bool,
        anlas: f64,
        v5: i64,
        legacy: i64,
        now: f64,
    ) -> Result<bool> {
        let (request_id, correlation_id, day, month) = (
            request_id.to_owned(),
            correlation_id.to_owned(),
            day.to_owned(),
            month.to_owned(),
        );
        self.write(move |tx| Ok(tx.execute("UPDATE request_settlements SET status='dispatched',correlation_id=?,day=?,month=?,anlas=?,v5=?,legacy=?,is_vibe=?,dispatched_at=CASE WHEN dispatched_at IS NULL THEN ? ELSE dispatched_at END,updated_at=? WHERE request_id=? AND status IN ('reserved','dispatched')",
            params![correlation_id, day, month, anlas, v5, legacy, is_vibe, now, now, request_id])? == 1)).await
    }

    #[allow(clippy::too_many_arguments)]
    #[allow(clippy::too_many_arguments)]
    pub async fn update_request_reservation(
        &self,
        request_id: &str,
        day: &str,
        month: &str,
        is_vibe: bool,
        anlas: f64,
        v5: i64,
        legacy: i64,
    ) -> Result<bool> {
        let request_id = request_id.to_owned();
        let (day, month) = (day.to_owned(), month.to_owned());
        self.write(move |tx| Ok(tx.execute("UPDATE request_settlements SET day=?,month=?,anlas=?,v5=?,legacy=?,is_vibe=? WHERE request_id=? AND status IN ('reserved','dispatched')",
            params![day, month, anlas, v5, legacy, is_vibe, request_id])? == 1)).await
    }

    /// Persist the complete success payload before applying counters. Repeated identical calls are safe.
    pub async fn save_request_result(
        &self,
        request_id: &str,
        payload: &str,
        result_at: f64,
        now: f64,
    ) -> Result<bool> {
        let (request_id, payload) = (request_id.to_owned(), payload.to_owned());
        self.write(move |tx| Ok(tx.execute("UPDATE request_settlements SET status='result_saved',result_json=?,result_at=?,updated_at=? WHERE request_id=? AND status='dispatched'",
            params![payload, result_at, now, request_id])? == 1)).await
    }

    pub async fn mark_request_uncertain(&self, request_id: &str, now: f64) -> Result<bool> {
        let request_id = request_id.to_owned();
        self.write(move |tx| Ok(tx.execute("UPDATE request_settlements SET status='uncertain',updated_at=? WHERE request_id=? AND status='dispatched'",
            params![now, request_id])? == 1)).await
    }

    /// Release only requests proven not to have been dispatched.
    pub async fn release_request_reservation(&self, request_id: &str, now: f64) -> Result<bool> {
        let request_id = request_id.to_owned();
        self.write(move |tx| Ok(tx.execute("UPDATE request_settlements SET status='released',updated_at=? WHERE request_id=? AND status='reserved'",
            params![now, request_id])? == 1)).await
    }

    /// Conservative drop handling: an undispatched request is released; a dispatched request with
    /// no durable outcome is held as uncertain. Committed/released rows are untouched.
    pub async fn close_request_reservation(&self, request_id: &str, now: f64) -> Result<String> {
        let request_id = request_id.to_owned();
        self.write(move |tx| {
            tx.execute("UPDATE request_settlements SET status=CASE status WHEN 'reserved' THEN 'released' WHEN 'dispatched' THEN 'uncertain' ELSE status END,updated_at=? WHERE request_id=?",
                params![now, request_id])?;
            tx.query_row("SELECT status FROM request_settlements WHERE request_id=?", [&request_id], |r| r.get(0))
        }).await
    }

    pub async fn resolve_request_failure(
        &self,
        request_id: &str,
        uncertain: bool,
        now: f64,
    ) -> Result<()> {
        let (request_id, status) = (
            request_id.to_owned(),
            if uncertain { "uncertain" } else { "released" },
        );
        self.write(move |tx| {
            tx.execute("UPDATE request_settlements SET status=?,updated_at=? WHERE request_id=? AND status='dispatched'",
                params![status, now, request_id])?;
            Ok(())
        }).await
    }

    /// 成功日志、额度与使用时间一起提交；写入失败时整笔回退（一个事务）。
    /// 写线程独占写连接，锁冲突由 `Db::write` 的 BUSY 重试兜住。
    pub async fn record_success(&self, s: &Success, now: f64) -> Result<()> {
        self.commit_success(s, None, now, now).await
    }

    /// Save a durable result payload, then apply usage/counters and mark committed in one transaction.
    pub async fn record_success_settled(
        &self,
        s: &Success,
        request_id: &str,
        now: f64,
    ) -> Result<()> {
        let payload =
            serde_json::to_string(s).map_err(|e| crate::DbError::Invalid(e.to_string()))?;
        if !self
            .save_request_result(request_id, &payload, now, now)
            .await?
        {
            // A prior commit is idempotent; all other states are an invalid transition.
            let id = request_id.to_owned();
            let status: Option<String> = self
                .read(move |c| {
                    c.query_row(
                        "SELECT status FROM request_settlements WHERE request_id=?",
                        [id],
                        |r| r.get(0),
                    )
                    .optional()
                })
                .await?;
            if status.as_deref() == Some("committed") {
                return Ok(());
            }
            return Err(crate::DbError::Invalid(
                "request result is not dispatchable".into(),
            ));
        }
        self.commit_success(s, Some(request_id), now, now).await
    }

    async fn commit_success(
        &self,
        s: &Success,
        request_id: Option<&str>,
        now: f64,
        result_at: f64,
    ) -> Result<()> {
        let s = s.clone();
        let request_id = request_id.map(str::to_owned);
        self.write(move |tx| {
            let held = s.remaining_anlas > 0.0 || s.remaining_v5 > 0 || s.remaining_legacy > 0;
            if let Some(id) = request_id.as_deref() {
                let status: String = tx.query_row("SELECT status FROM request_settlements WHERE request_id=?", [id], |r| r.get(0))?;
                if status == "committed" { return Ok(()); }
                if status != "result_saved" { return Err(rusqlite::Error::InvalidQuery); }
            }
            let h = s.hints.columns();
            tx.execute(
                INSERT_LOG,
                params![
                    result_at,
                    s.key_id,
                    s.key_name,
                    s.kind,
                    s.model,
                    "ok",
                    s.images,
                    s.anlas + s.site_paid_anlas,
                    s.tokens,
                    primitives::take_chars(&s.detail, 500),
                    s.unconfirmed_anlas,
                    s.wait_ms.max(0),
                    s.dur_ms.max(0),
                    s.up_status,
                    primitives::take_chars(&s.rid, 16),
                    "",
                    s.token_id,
                    primitives::take_chars(&s.client, 160),
                    primitives::take_chars(&s.ip, 64),
                    h[0],
                    h[1],
                    h[2],
                    h[3],
                    h[4],
                    h[5],
                ],
            )?;
            if let Some(id) = request_id.as_deref() {
                tx.execute("INSERT INTO usage_request_links(request_id,usage_log_id) VALUES(?,last_insert_rowid())", [id])?;
            }
            tx.execute(
                UPSERT_COUNTERS,
                params![
                    s.key_id,
                    s.day,
                    s.images,
                    s.anlas,
                    s.tokens,
                    1_i64,
                    s.v5,
                    s.legacy_free_images
                ],
            )?;
            tx.execute(
                "UPDATE api_keys SET last_used_at=?, \
                 last_image_at=CASE WHEN ? THEN ? ELSE last_image_at END WHERE id=?",
                params![now, i64::from(is_image_kind(&s.kind)), result_at, s.key_id],
            )?;
            if let Some(id) = request_id.as_deref() {
                tx.execute("UPDATE request_settlements SET status=?,anlas=?,v5=?,legacy=?,result_json=NULL,updated_at=? WHERE request_id=? AND status='result_saved'",
                    params![if held { "uncertain" } else { "committed" }, s.remaining_anlas, s.remaining_v5, s.remaining_legacy, now, id])?;
            }
            Ok(())
        })
        .await
    }

    pub async fn commit_saved_request_result(
        &self,
        request_id: &str,
        payload: &str,
        now: f64,
    ) -> Result<()> {
        let success: Success =
            serde_json::from_str(payload).map_err(|e| crate::DbError::Invalid(e.to_string()))?;
        let id = request_id.to_owned();
        let result_at = self
            .read(move |c| {
                c.query_row(
                    "SELECT result_at FROM request_settlements WHERE request_id=?",
                    [id],
                    |r| r.get::<_, Option<f64>>(0),
                )
                .map(|v| v.unwrap_or(now))
            })
            .await?;
        self.commit_success(&success, Some(request_id), now, result_at)
            .await
    }

    /// Normalize crash states without replaying upstream: reserved was never dispatched; dispatched is unknown.
    pub async fn recover_request_settlements(&self, now: f64) -> Result<Vec<RequestSettlement>> {
        self.write(move |tx| {
            tx.execute("UPDATE request_settlements SET status='released',updated_at=? WHERE status='reserved'", [now])?;
            tx.execute("UPDATE request_settlements SET status='uncertain',updated_at=? WHERE status='dispatched'", [now])?;
            let mut stmt = tx.prepare("SELECT request_id,rid,key_id,status,day,month,anlas,v5,legacy,is_vibe,result_json,correlation_id FROM request_settlements WHERE status IN ('uncertain','result_saved') ORDER BY request_id")?;
            stmt.query_map([], |r| Ok(RequestSettlement {
                request_id:r.get(0)?, rid:r.get(1)?, key_id:r.get(2)?, status:r.get(3)?, day:r.get(4)?, month:r.get(5)?,
                anlas:r.get(6)?, v5:r.get(7)?, legacy:r.get(8)?, is_vibe:r.get::<_,i64>(9)? != 0, result_json:r.get(10)?, correlation_id:r.get(11)?
            }))?.collect()
        }).await
    }

    pub async fn list_unknown_request_settlements(&self) -> Result<Vec<RequestSettlement>> {
        self.read(|c| {
            let mut stmt=c.prepare("SELECT request_id,rid,key_id,status,day,month,anlas,v5,legacy,is_vibe,result_json,correlation_id FROM request_settlements WHERE status='uncertain' ORDER BY updated_at")?;
            stmt.query_map([], |r| Ok(RequestSettlement { request_id:r.get(0)?, rid:r.get(1)?, key_id:r.get(2)?, status:r.get(3)?, day:r.get(4)?, month:r.get(5)?, anlas:r.get(6)?, v5:r.get(7)?, legacy:r.get(8)?, is_vibe:r.get::<_,i64>(9)? != 0, result_json:r.get(10)?, correlation_id:r.get(11)? }))?.collect()
        }).await
    }

    pub async fn set_request_delivery(
        &self,
        request_id: &str,
        state: &str,
        now: f64,
    ) -> Result<()> {
        let (id, state) = (request_id.to_owned(), state.to_owned());
        self.write(move |tx| {
            tx.execute("UPDATE request_settlements SET delivery_state=?,updated_at=? WHERE request_id=? AND status NOT IN ('released','released_manual') AND delivery_state='pending'", params![state, now, id])?;
            Ok(())
        }).await
    }

    /// Explicit administrator decision: release the unknown remainder and audit the rationale atomically.
    pub async fn manually_release_unknown_request(
        &self,
        request_id: &str,
        actor: &str,
        reason: &str,
        now: f64,
    ) -> Result<bool> {
        let (request_id, actor, reason) =
            (request_id.to_owned(), actor.to_owned(), reason.to_owned());
        self.write(move |tx| {
            let n=tx.execute("UPDATE request_settlements SET status='released_manual',resolution_note=?,updated_at=? WHERE request_id=? AND status='uncertain'", params![reason,now,request_id])?;
            if n == 0 { return Ok(false); }
            tx.execute("INSERT INTO admin_actions(ts,actor,action,target,detail,ok) VALUES(?,?,?,?,?,1)", params![now,actor,"request_settlement_release",request_id,reason])?;
            Ok(true)
        }).await
    }

    /// Anlas already spent on site-paid images through `token_id` (rows whose detail carries `marker`).
    pub async fn site_paid_spent(&self, token_id: &str, marker: &str) -> Result<f64> {
        let (token_id, pattern) = (token_id.to_string(), format!("%{marker}%"));
        self.read(move |c| {
            c.query_row(
                "SELECT COALESCE(SUM(anlas), 0) FROM usage_log \
                 WHERE token_id = ? AND status = 'ok' AND detail LIKE ?",
                params![token_id, pattern],
                |r| r.get(0),
            )
        })
        .await
    }

    pub async fn add_log(&self, l: &NewLog, now: f64) -> Result<()> {
        let l = l.clone();
        self.write(move |tx| insert_usage_log(tx, &l, now)).await
    }

    /// Persist a member-visible failure and link it to its settlement in one transaction.
    pub async fn record_settled_failure(
        &self,
        l: &NewLog,
        request_id: &str,
        uncertain: bool,
        now: f64,
    ) -> Result<()> {
        let (l, id, target) = (
            l.clone(),
            request_id.to_owned(),
            if uncertain { "uncertain" } else { "released" },
        );
        self.write(move |tx| {
            insert_usage_log(tx, &l, now)?;
            tx.execute("INSERT INTO usage_request_links(request_id,usage_log_id) VALUES(?,last_insert_rowid())", [&id])?;
            tx.execute("UPDATE request_settlements SET status=?,updated_at=? WHERE request_id=? AND status IN ('reserved','dispatched')", params![target, now, id])?;
            Ok(())
        }).await
    }
}
