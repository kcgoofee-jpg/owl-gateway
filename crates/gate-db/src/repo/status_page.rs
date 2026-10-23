//! 公开状态页（旧版 `app/status_page.py`）用到的表：`incidents` / `incident_updates`（事件）、
//! `status_hourly`（出图接口每小时各状态码次数）、`usage_log`（上游真实失败率）。
//!
//! 建表在 [`crate::bootstrap`]（`status_schema.sql`）。这里只放查询 / 写入；快照、可用率条、页面都在 gate-server。

use crate::{Db, Result};
use gate_core::pyjson;
use rusqlite::{OptionalExtension, params};
use serde_json::Value;

/// 事件的一条更新。
#[derive(Debug, Clone, PartialEq)]
pub struct IncidentUpdate {
    pub status: String,
    pub body: String,
    pub at: f64,
}

/// 一个事件 + 它的全部更新（最新的在前）。
#[derive(Debug, Clone, PartialEq)]
pub struct Incident {
    pub id: i64,
    pub title: String,
    pub impact: String,
    /// 库里是 JSON 数组（`["api", "upstream"]`）；原样保留，输出时不改动。
    pub components: Value,
    pub started_at: f64,
    pub resolved_at: Option<f64>,
    pub updates: Vec<IncidentUpdate>,
}

/// 过去一小时发到上游的请求（`up_status > 0`）的统计，用来判断「现在」上游是否异常。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct UpstreamRecent {
    pub total: i64,
    /// 失败数：5xx、429、403。
    pub bad: i64,
    /// 失败请求来自多少把不同的 Key（`key_id` 为空的不计）。
    pub bad_keys: i64,
}

/// 上游失败的判定，与旧版 `_upstream_days` / `_upstream_last_hour` 一致。
const BAD: &str = "up_status >= 500 OR up_status IN (429, 403)";

/// 按天聚合时的分桶宽度（秒）。SQLite 的 `'localtime'` 依赖进程时区，而网关的时区是代码常量（`gate_core::clock::TZ`）；
/// 所以这里只按 15 分钟桶聚合（所有现行时区的偏移都是 15 分钟的整数倍，桶不会跨日界线），
/// 由调用方按网关的时区把桶归到「天」。
pub const BUCKET_SECS: i64 = 900;

impl Db {
    /// `list_incidents(db, since)`：结束时间不早于 `since`（未结束的一律算）的事件，开始时间新的在前；
    /// 每个事件带全部更新（`at` 新的在前，同刻 id 大的在前）。
    pub async fn list_incidents(&self, since: f64) -> Result<Vec<Incident>> {
        self.read(move |c| {
            let mut stmt = c.prepare(
                "SELECT id, title, impact, components, started_at, resolved_at FROM incidents \
                 WHERE COALESCE(resolved_at, 9e18) >= ? ORDER BY started_at DESC",
            )?;
            let heads = stmt
                .query_map([since], |r| {
                    let raw: Option<String> = r.get(3)?;
                    Ok(Incident {
                        id: r.get(0)?,
                        title: r.get(1)?,
                        impact: r.get(2)?,
                        components: parse_components(raw.as_deref()),
                        started_at: r.get(4)?,
                        resolved_at: r.get(5)?,
                        updates: Vec::new(),
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let mut ups = c.prepare(
                "SELECT status, body, at FROM incident_updates WHERE incident_id=? ORDER BY at DESC, id DESC",
            )?;
            let mut out = Vec::with_capacity(heads.len());
            for mut inc in heads {
                inc.updates = ups
                    .query_map([inc.id], |r| {
                        Ok(IncidentUpdate {
                            status: r.get(0)?,
                            body: r.get(1)?,
                            at: r.get(2)?,
                        })
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                out.push(inc);
            }
            Ok(out)
        })
        .await
    }

    /// `create_incident`：事件 + 第一条更新同一个事务，返回新事件 id。
    /// `resolved_at` 只有第一条更新本身就是 `resolved` 时才等于 `at`。
    pub async fn create_incident(
        &self,
        title: &str,
        impact: &str,
        components: &[String],
        status: &str,
        body: &str,
        at: f64,
    ) -> Result<i64> {
        let (title, impact, status, body) = (
            title.to_string(),
            impact.to_string(),
            status.to_string(),
            body.to_string(),
        );
        // json.dumps(components, ensure_ascii=False)：默认分隔符 ", "
        let components = pyjson::dumps(&Value::from(components.to_vec()));
        self.write(move |tx| {
            let resolved_at: Option<f64> = (status == "resolved").then_some(at);
            tx.execute(
                "INSERT INTO incidents(title, impact, components, started_at, resolved_at) VALUES (?,?,?,?,?)",
                params![title, impact, components, at, resolved_at],
            )?;
            let id = tx.last_insert_rowid();
            tx.execute(
                "INSERT INTO incident_updates(incident_id, status, body, at) VALUES (?,?,?,?)",
                params![id, status, body, at],
            )?;
            Ok(id)
        })
        .await
    }

    /// `add_update`：追加一条更新，并按状态改写 `resolved_at`（resolved → `at`；其它 → NULL，即重新打开）。
    /// 事件不存在时什么都不写，返回 `false`（旧版会写一条挂不到任何事件的孤儿更新）。
    pub async fn add_incident_update(
        &self,
        incident_id: i64,
        status: &str,
        body: &str,
        at: f64,
    ) -> Result<bool> {
        let (status, body) = (status.to_string(), body.to_string());
        self.write(move |tx| {
            let exists = tx
                .query_row("SELECT 1 FROM incidents WHERE id=?", [incident_id], |_| {
                    Ok(())
                })
                .optional()?
                .is_some();
            if !exists {
                return Ok(false);
            }
            tx.execute(
                "INSERT INTO incident_updates(incident_id, status, body, at) VALUES (?,?,?,?)",
                params![incident_id, status, body, at],
            )?;
            let resolved_at: Option<f64> = (status == "resolved").then_some(at);
            tx.execute(
                "UPDATE incidents SET resolved_at=? WHERE id=?",
                params![resolved_at, incident_id],
            )?;
            Ok(true)
        })
        .await
    }

    /// `_api_error_days` 的数据：`hour >= start` 的所有 `(整点秒数, 状态码, 次数)`。
    pub async fn status_hourly_since(&self, start: i64) -> Result<Vec<(i64, i64, i64)>> {
        self.read(move |c| {
            let mut stmt = c.prepare("SELECT hour, code, n FROM status_hourly WHERE hour >= ?")?;
            stmt.query_map([start], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
                .collect()
        })
        .await
    }

    /// `_upstream_days` 的数据：`ts >= start` 且真的发到上游（`up_status > 0`）的请求，
    /// 按 [`BUCKET_SECS`] 秒一桶聚合：`(桶起点秒数, 请求数, 失败数)`，按桶起点升序。
    pub async fn upstream_buckets_since(&self, start: f64) -> Result<Vec<(i64, i64, i64)>> {
        self.read(move |c| {
            let mut stmt = c.prepare(&format!(
                "SELECT CAST(ts / {BUCKET_SECS} AS INTEGER) AS b, COUNT(*), \
                 SUM(CASE WHEN {BAD} THEN 1 ELSE 0 END) \
                 FROM usage_log WHERE ts >= ? AND up_status > 0 GROUP BY b ORDER BY b"
            ))?;
            stmt.query_map([start], |r| {
                Ok((
                    r.get::<_, i64>(0)? * BUCKET_SECS,
                    r.get(1)?,
                    r.get::<_, Option<i64>>(2)?.unwrap_or(0),
                ))
            })?
            .collect()
        })
        .await
    }

    /// `_upstream_last_hour` 的数据：`ts >= since` 的上游请求数、失败数、失败请求来自几把不同的 Key。
    pub async fn upstream_recent(&self, since: f64) -> Result<UpstreamRecent> {
        self.read(move |c| {
            c.query_row(
                &format!(
                    "SELECT COUNT(*), SUM(CASE WHEN {BAD} THEN 1 ELSE 0 END), \
                     COUNT(DISTINCT CASE WHEN {BAD} THEN key_id END) \
                     FROM usage_log WHERE ts >= ? AND up_status > 0"
                ),
                [since],
                |r| {
                    Ok(UpstreamRecent {
                        total: r.get::<_, Option<i64>>(0)?.unwrap_or(0),
                        bad: r.get::<_, Option<i64>>(1)?.unwrap_or(0),
                        bad_keys: r.get::<_, Option<i64>>(2)?.unwrap_or(0),
                    })
                },
            )
        })
        .await
    }
}

/// `json.loads(components or "[]")`。库里的值都是我们自己写的 JSON 数组；
/// 万一被手工改坏，旧版会让整个状态页 500，这里按「没有组件」处理。
fn parse_components(raw: Option<&str>) -> Value {
    let raw = raw.filter(|s| !s.is_empty()).unwrap_or("[]");
    serde_json::from_str(raw).unwrap_or_else(|_| Value::Array(Vec::new()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn components_fallbacks() {
        assert_eq!(parse_components(None), serde_json::json!([]));
        assert_eq!(parse_components(Some("")), serde_json::json!([]));
        assert_eq!(
            parse_components(Some(r#"["api", "upstream"]"#)),
            serde_json::json!(["api", "upstream"])
        );
        assert_eq!(parse_components(Some("{oops")), serde_json::json!([]));
    }
}
