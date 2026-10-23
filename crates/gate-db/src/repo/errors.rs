//! Bug 追踪（`error_events`）与成员接口状态码分布（`status_hourly`）。旧版 `app/errors.py` / `app/status_stats.py`
//! 直接在共享连接上写 SQL，这里收口成方法。

use crate::{Db, Result, value};
use rusqlite::OptionalExtension;
use serde_json::Value;

/// 一条要入库的错误事件（字段同旧版 dict）。
#[derive(Debug, Clone)]
pub struct ErrorRow {
    pub sig: String,
    pub source: String,
    pub level: String,
    pub title: String,
    pub detail: String,
    pub path: String,
    pub key_id: Option<i64>,
    pub rid: String,
    pub ts: f64,
}

/// 自动处理时读出的一条未处理 Bug。
#[derive(Debug, Clone)]
pub struct OpenError {
    pub sig: String,
    pub source: String,
    pub level: String,
    pub title: String,
    pub detail: String,
    pub last_seen: f64,
}

/// 要自动关闭的一条：`last_seen` 是读到时的值（并发保护，见 [`Db::auto_resolve_errors`]）。
#[derive(Debug, Clone)]
pub struct AutoResolve {
    pub sig: String,
    pub last_seen: f64,
    pub reason: &'static str,
}

/// 入库结果：新 bug / 已处理后复发 / 普通累加。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stored {
    New,
    Regressed,
    Repeat,
}

impl Db {
    /// 一个事务里完成「查旧行 → 插入或累加」，返回状态（决定要不要私信站长）。
    pub async fn store_error_event(&self, e: &ErrorRow) -> Result<Stored> {
        let e = e.clone();
        self.write(move |tx| {
            let existing: Option<Option<f64>> = tx
                .query_row("SELECT resolved_at FROM error_events WHERE sig=?", [&e.sig], |r| value::f64_opt(r, 0))
                .optional()?;
            match existing {
                None => {
                    tx.execute(
                        "INSERT INTO error_events (sig, source, level, title, detail, count, first_seen, last_seen, \
                         last_rid, last_key, last_path) VALUES (?,?,?,?,?,1,?,?,?,?,?)",
                        rusqlite::params![e.sig, e.source, e.level, e.title, e.detail, e.ts, e.ts, e.rid, e.key_id, e.path],
                    )?;
                    Ok(Stored::New)
                }
                Some(resolved) => {
                    tx.execute(
                        "UPDATE error_events SET count=count+1, last_seen=?, title=?, detail=?, last_rid=?, \
                         last_key=?, last_path=?, resolved_at=NULL, auto_resolved=NULL WHERE sig=?",
                        rusqlite::params![e.ts, e.title, e.detail, e.rid, e.key_id, e.path, e.sig],
                    )?;
                    Ok(if resolved.is_some() { Stored::Regressed } else { Stored::Repeat })
                }
            }
        })
        .await
    }

    pub async fn list_error_events(
        &self,
        include_resolved: bool,
        limit: i64,
    ) -> Result<Vec<Value>> {
        let where_ = if include_resolved {
            ""
        } else {
            " WHERE resolved_at IS NULL"
        };
        let order = if include_resolved {
            "ORDER BY (resolved_at IS NOT NULL), last_seen DESC"
        } else {
            "ORDER BY last_seen DESC"
        };
        let sql = format!(
            "SELECT sig, source, level, title, detail, count, first_seen, last_seen, last_rid, last_key, \
             last_path, resolved_at, auto_resolved FROM error_events{where_} {order} LIMIT ?"
        );
        let limit = limit.clamp(1, 500);
        self.read(move |c| {
            let mut stmt = c.prepare(&sql)?;
            let rows = stmt.query_map([limit], value::row_json)?;
            rows.collect()
        })
        .await
    }

    pub async fn open_error_count(&self) -> Result<i64> {
        self.read(|c| {
            c.query_row(
                "SELECT COUNT(*) FROM error_events WHERE resolved_at IS NULL",
                [],
                |r| r.get(0),
            )
        })
        .await
    }

    /// 标记已处理；不存在返回 false。
    pub async fn resolve_error_event(&self, sig: &str, now: f64) -> Result<bool> {
        let sig = sig.to_string();
        self.write(move |tx| {
            let found: Option<Option<f64>> = tx
                .query_row(
                    "SELECT resolved_at FROM error_events WHERE sig=?",
                    [&sig],
                    |r| value::f64_opt(r, 0),
                )
                .optional()?;
            match found {
                None => Ok(false),
                Some(Some(_)) => Ok(true),
                Some(None) => {
                    tx.execute(
                        "UPDATE error_events SET resolved_at=?, auto_resolved=NULL WHERE sig=?",
                        rusqlite::params![now, sig],
                    )?;
                    Ok(true)
                }
            }
        })
        .await
    }

    pub async fn resolve_all_error_events(&self, now: f64) -> Result<usize> {
        self.write(move |tx| {
            tx.execute(
                "UPDATE error_events SET resolved_at=?, auto_resolved=NULL WHERE resolved_at IS NULL",
                [now],
            )
        })
        .await
    }

    /// 自动处理的候选：所有还没处理的 Bug（`detail` 最长 4000 字符，一次最多取 2000 条）。
    pub async fn open_errors_for_triage(&self) -> Result<Vec<OpenError>> {
        self.read(|c| {
            let mut stmt = c.prepare(
                "SELECT sig, source, level, title, detail, last_seen FROM error_events \
                 WHERE resolved_at IS NULL ORDER BY last_seen LIMIT 2000",
            )?;
            let rows = stmt.query_map([], |r| {
                Ok(OpenError {
                    sig: r.get(0)?,
                    source: r.get(1)?,
                    level: r.get(2)?,
                    title: r.get(3)?,
                    detail: r.get(4)?,
                    last_seen: r.get(5)?,
                })
            })?;
            rows.collect()
        })
        .await
    }

    /// 自动关闭一批 Bug，`reason` 是 [`gate_core::errors::AutoReason::code`]。返回真正关掉的条数。
    /// 只关「读到时是什么样、现在还是什么样」的行：`last_seen` 变了说明读完之后又出现过，留给下一轮，
    /// 不能把刚发生的一次一并关掉。
    pub async fn auto_resolve_errors(&self, items: &[AutoResolve], now: f64) -> Result<usize> {
        let items = items.to_vec();
        self.write(move |tx| {
            let mut stmt = tx.prepare(
                "UPDATE error_events SET resolved_at=?, auto_resolved=? \
                 WHERE sig=? AND resolved_at IS NULL AND last_seen=?",
            )?;
            let mut n = 0;
            for it in &items {
                n += stmt.execute(rusqlite::params![now, it.reason, it.sig, it.last_seen])?;
            }
            Ok(n)
        })
        .await
    }

    /// 把内存里累计的 (小时, 状态码) → 次数 写进 `status_hourly`（累加）。
    pub async fn flush_status_hourly(&self, items: &[((i64, u16), i64)]) -> Result<()> {
        let items = items.to_vec();
        self.write(move |tx| {
            let mut stmt = tx.prepare(
                "INSERT INTO status_hourly(hour, code, n) VALUES (?,?,?) ON CONFLICT(hour, code) DO UPDATE SET n=n+excluded.n",
            )?;
            for ((h, c), n) in &items {
                stmt.execute(rusqlite::params![h, i64::from(*c), n])?;
            }
            Ok(())
        })
        .await
    }

    /// 某个整点之后各状态码的总次数。
    pub async fn status_totals_since(&self, since: i64) -> Result<Vec<(i64, i64)>> {
        self.read(move |c| {
            let mut stmt =
                c.prepare("SELECT code, SUM(n) FROM status_hourly WHERE hour>=? GROUP BY code")?;
            let rows = stmt.query_map([since], |r| {
                Ok((value::i64_or(r, 0, 0)?, value::i64_or(r, 1, 0)?))
            })?;
            rows.collect()
        })
        .await
    }
}
