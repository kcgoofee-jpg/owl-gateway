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

/// 已终结（`committed` / `released` / `released_manual`）的结算行保留多久。
/// 结算行的用处是「请求在途时占住额度」和「崩溃后恢复」，两者都在秒级；1 小时足够覆盖提交重试。
pub const SETTLED_KEEP_SECS: f64 = 3600.0;

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
    pub async fn dispatch_request_settlement(
        &self,
        request_id: &str,
        correlation_id: &str,
        rid: &str,
        day: &str,
        month: &str,
        is_vibe: bool,
        anlas: f64,
        v5: i64,
        legacy: i64,
        now: f64,
    ) -> Result<bool> {
        let (request_id, correlation_id, rid, day, month) = (
            request_id.to_owned(),
            correlation_id.to_owned(),
            rid.to_owned(),
            day.to_owned(),
            month.to_owned(),
        );
        self.write(move |tx| Ok(tx.execute("UPDATE request_settlements SET status='dispatched',correlation_id=?,rid=?,day=?,month=?,anlas=?,v5=?,legacy=?,is_vibe=?,dispatched_at=CASE WHEN dispatched_at IS NULL THEN ? ELSE dispatched_at END,updated_at=? WHERE request_id=? AND status IN ('reserved','dispatched')",
            params![correlation_id, rid, day, month, anlas, v5, legacy, is_vibe, now, now, request_id])? == 1)).await
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
            let stored: Option<(String, Option<String>)> = self
                .read(move |c| {
                    c.query_row(
                        "SELECT status, result_json FROM request_settlements WHERE request_id=?",
                        [id],
                        |r| Ok((r.get(0)?, r.get(1)?)),
                    )
                    .optional()
                })
                .await?;
            match stored {
                Some((status, _)) if status == "committed" => return Ok(()),
                Some((status, Some(saved))) if status == "result_saved" && saved == payload => {
                    return self
                        .commit_saved_request_result(request_id, &saved, now)
                        .await;
                }
                _ => {}
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
        let expected = request_id
            .as_ref()
            .map(|_| serde_json::to_string(&s))
            .transpose()
            .map_err(|e| crate::DbError::Invalid(e.to_string()))?;
        self.write(move |tx| {
            let held = s.remaining_anlas > 0.0 || s.remaining_v5 > 0 || s.remaining_legacy > 0;
            if let Some(id) = request_id.as_deref() {
                let (status, stored): (String, Option<String>) = tx.query_row("SELECT status, result_json FROM request_settlements WHERE request_id=?", [id], |r| Ok((r.get(0)?, r.get(1)?)))?;
                if status == "committed" { return Ok(()); }
                if status != "result_saved" || stored != expected { return Err(rusqlite::Error::InvalidQuery); }
            }
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
                tx.execute("UPDATE upstream_anlas_charges SET amount=?,spent=? WHERE request_id=? AND token_id=?",
                    params![s.anlas + s.site_paid_anlas + s.remaining_anlas, s.anlas + s.site_paid_anlas, id, s.token_id])?;
                tx.execute("UPDATE request_settlements SET status=?,site_paid_hold=MAX(0,site_paid_hold-?),anlas=?,v5=?,legacy=?,result_json=NULL,updated_at=? WHERE request_id=? AND status='result_saved'",
                    params![if held { "uncertain" } else { "committed" }, s.site_paid_anlas, s.remaining_anlas, s.remaining_v5, s.remaining_legacy, now, id])?;
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
            tx.execute("UPDATE upstream_anlas_charges SET amount=spent WHERE request_id=?", [&request_id])?;
            tx.execute("INSERT INTO admin_actions(ts,actor,action,target,detail,ok) VALUES(?,?,?,?,?,1)", params![now,actor,"request_settlement_release",request_id,reason])?;
            Ok(true)
        }).await
    }

    /// Admit a site-paid dispatch against committed spending and all unresolved
    /// holds in the same SQLite write transaction. Reconfirming excludes itself.
    pub async fn reserve_site_paid(
        &self,
        id: &str,
        token: &str,
        cost: f64,
        now: f64,
    ) -> Result<bool> {
        use gate_core::paid_v5;
        let (id, token) = (id.to_owned(), token.to_owned());
        self.write(move |tx| {
            let raw: Option<String> = tx
                .query_row(
                    "SELECT value FROM site_settings WHERE key=?",
                    [paid_v5::KEY],
                    |r| r.get(0),
                )
                .optional()?;
            let Some(grant) = paid_v5::grant(raw.as_deref(), &token, now) else {
                return Ok(false);
            };
            let spent: f64 = tx.query_row(
                "SELECT COALESCE(SUM(anlas),0) FROM usage_log
                 WHERE token_id=? AND status='ok' AND detail LIKE ?",
                params![token, format!("%{}%", paid_v5::MARKER)],
                |r| r.get(0),
            )?;
            let held: f64 = tx.query_row(
                "SELECT COALESCE(SUM(site_paid_hold),0) FROM request_settlements
                 WHERE site_paid_token=? AND site_paid_hold>0 AND request_id<>?
                 AND status IN ('reserved','dispatched','uncertain','result_saved')",
                params![token, id],
                |r| r.get(0),
            )?;
            if !paid_v5::fits(grant, spent + held, cost) {
                return Ok(false);
            }
            Ok(tx.execute(
                "UPDATE request_settlements SET site_paid_token=?, site_paid_hold=?
                 WHERE request_id=? AND status IN ('reserved','dispatched')",
                params![token, cost, id],
            )? == 1)
        })
        .await
    }

    pub async fn clear_site_paid(&self, id: &str) -> Result<()> {
        let id = id.to_owned();
        self.write(move |tx| {
            tx.execute(
                "UPDATE request_settlements SET site_paid_hold=0, site_paid_token=''
                 WHERE request_id=? AND status IN ('reserved','dispatched')",
                [&id],
            )?;
            Ok(())
        })
        .await
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

    /// 账本只留未决行：`committed` / `released` / `released_manual` 与用量日志重复
    /// （成功的已写 usage_log，失败的也已写 usage_log），留着只是同一件事的两份数据。
    /// 保留 [`SETTLED_KEEP_SECS`] 是为了提交重试的幂等判断还能读到上一状态。
    /// 指向已删结算的 `usage_request_links` 一并清掉——它只用来从 request_id 找回用量日志。
    pub async fn prune_settled_requests(&self, before: f64) -> Result<usize> {
        self.write(move |tx| {
            let n = tx.execute(
                "DELETE FROM request_settlements WHERE status IN ('committed','released','released_manual') AND updated_at<?",
                [before],
            )?;
            tx.execute(
                "DELETE FROM usage_request_links WHERE request_id NOT IN (SELECT request_id FROM request_settlements)",
                [],
            )?;
            Ok(n)
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
            if !uncertain { tx.execute("UPDATE upstream_anlas_charges SET amount=spent WHERE request_id=?", [&id])?; }
            insert_usage_log(tx, &l, now)?;
            tx.execute("INSERT INTO usage_request_links(request_id,usage_log_id) VALUES(?,last_insert_rowid())", [&id])?;
            // 派到号之后 public rid 会改前缀。取消路径会先把预留标成 released，落账时仍要把编号写回去。
            tx.execute(
                "UPDATE request_settlements SET rid=?,updated_at=? WHERE request_id=?",
                params![primitives::take_chars(&l.rid, 16), now, id],
            )?;
            tx.execute(
                "UPDATE request_settlements SET status=?,updated_at=? WHERE request_id=? AND status IN ('reserved','dispatched')",
                params![target, now, id],
            )?;
            Ok(())
        }).await
    }
}
