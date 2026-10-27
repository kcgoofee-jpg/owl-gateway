//! 按上游账号（`usage_log.token_id`）统计的只读查询：后台「上游」页的每账号请求统计和 Anlas 推算用。

use crate::{Db, Result, value};

/// 一条带账号的日志（或没记账号的成功出图，token_id 为 None）。
#[derive(Debug, Clone, PartialEq)]
pub struct AccountLogRow {
    pub ts: f64,
    pub token_id: Option<String>,
    pub status: String,
    pub up_status: i64,
    pub kind: String,
    pub wait_ms: i64,
    pub dur_ms: i64,
}

impl Db {
    /// `ts > since` 的日志里：所有带 token_id 的行，加上没记账号的成功出图行（用来估计「没法归属」的量）。
    pub async fn account_log_rows(&self, since: f64) -> Result<Vec<AccountLogRow>> {
        self.read(move |c| {
            let mut stmt = c.prepare(
                "SELECT ts, token_id, status, up_status, kind, wait_ms, dur_ms FROM usage_log \
                 WHERE ts>? AND (token_id IS NOT NULL OR (status='ok' AND kind LIKE 'image%'))",
            )?;
            stmt.query_map([since], |r| {
                Ok(AccountLogRow {
                    ts: value::f64_or(r, 0, 0.0)?,
                    token_id: r.get::<_, Option<String>>(1)?.filter(|t| !t.is_empty()),
                    status: value::text_or(r, 2, "")?,
                    up_status: value::i64_or(r, 3, 0)?,
                    kind: value::text_or(r, 4, "")?,
                    wait_ms: value::i64_or(r, 5, 0)?.max(0),
                    dur_ms: value::i64_or(r, 6, 0)?.max(0),
                })
            })?
            .collect()
        })
        .await
    }

    /// 成功请求记下的 Anlas，按账号汇总（None = 日志没记账号）。`id > min_id` 且 `ts > since`。
    pub async fn anlas_by_token(
        &self,
        min_id: i64,
        since: f64,
    ) -> Result<Vec<(Option<String>, f64)>> {
        self.read(move |c| {
            let mut stmt = c.prepare(
                "SELECT token_id, COALESCE(SUM(anlas), 0) FROM usage_log \
                 WHERE id>? AND ts>? AND status='ok' GROUP BY token_id",
            )?;
            stmt.query_map(rusqlite::params![min_id, since], |r| {
                Ok((
                    r.get::<_, Option<String>>(0)?.filter(|t| !t.is_empty()),
                    value::f64_or(r, 1, 0.0)?,
                ))
            })?
            .collect()
        })
        .await
    }
}
