//! Member request statistics over the same key and service-day range as request history.

use super::usage::LogSummary;
use crate::{Db, Result};
use rusqlite::params;
use serde::Serialize;

#[derive(Debug, Default, Serialize)]
pub struct RequestStats {
    pub summary: LogSummary,
    pub images: i64,
    pub anlas: f64,
    pub avg_wait_s: Option<f64>,
    pub avg_dur_s: Option<f64>,
    pub uncertain_requests: i64,
    pub breakdown: Vec<RequestCategory>,
}

#[derive(Debug, Serialize)]
pub struct RequestCategory {
    pub kind: String,
    pub model: String,
    pub requests: i64,
}

impl Db {
    pub async fn member_request_stats(&self, key_id: i64, since: f64) -> Result<RequestStats> {
        self.read(move |c| {
            // One statement keeps counts, costs and timings on the same SQLite snapshot.
            // Only confirmed successful usage is charged here; held settlement remainders are excluded.
            let mut stmt = c.prepare(
                "SELECT l.status, l.kind, l.model, COUNT(*), \
                 SUM(CASE WHEN l.status='ok' THEN MAX(l.images,0) ELSE 0 END), \
                 SUM(CASE WHEN l.status='ok' AND instr(l.detail,?3)=0 THEN MAX(l.anlas,0) ELSE 0 END), \
                 SUM(CASE WHEN l.status='ok' AND l.kind IN ('image','image_stream') AND l.dur_ms>0 AND l.wait_ms>=0 THEN l.wait_ms ELSE 0 END), \
                 SUM(CASE WHEN l.status='ok' AND l.kind IN ('image','image_stream') AND l.dur_ms>0 AND l.wait_ms>=0 THEN 1 ELSE 0 END), \
                 SUM(CASE WHEN l.status='ok' AND l.kind IN ('image','image_stream') AND l.dur_ms>0 THEN l.dur_ms ELSE 0 END), \
                 SUM(CASE WHEN l.status='ok' AND l.kind IN ('image','image_stream') AND l.dur_ms>0 THEN 1 ELSE 0 END), \
                 SUM(EXISTS(SELECT 1 FROM usage_request_links u JOIN request_settlements s ON s.request_id=u.request_id \
                     WHERE u.usage_log_id=l.id AND s.status='uncertain')) \
                 FROM usage_log l WHERE l.key_id=?1 AND l.ts>=?2 \
                 GROUP BY l.status,l.kind,l.model ORDER BY l.kind,l.model,l.status",
            )?;
            let mut out = RequestStats::default();
            let (mut wait_ms, mut wait_n, mut dur_ms, mut dur_n) = (0i64, 0i64, 0i64, 0i64);
            let mut categories = std::collections::BTreeMap::new();
            let mut rows = stmt.query(params![key_id, since, gate_core::paid_v5::MARKER])?;
            while let Some(row) = rows.next()? {
                let status: String = row.get(0)?;
                let kind: String = row.get(1)?;
                let model: String = row.get(2)?;
                let n: i64 = row.get(3)?;
                out.summary.add(&status, n);
                out.images += row.get::<_, i64>(4)?;
                out.anlas += row.get::<_, f64>(5)?;
                wait_ms += row.get::<_, i64>(6)?;
                wait_n += row.get::<_, i64>(7)?;
                dur_ms += row.get::<_, i64>(8)?;
                dur_n += row.get::<_, i64>(9)?;
                out.uncertain_requests += row.get::<_, i64>(10)?;
                *categories.entry((kind, model)).or_insert(0) += n;
            }
            out.avg_wait_s = (wait_n > 0).then(|| wait_ms as f64 / wait_n as f64 / 1000.0);
            out.avg_dur_s = (dur_n > 0).then(|| dur_ms as f64 / dur_n as f64 / 1000.0);
            out.breakdown = categories.into_iter().map(|((kind, model), requests)| RequestCategory {kind, model, requests}).collect();
            Ok(out)
        }).await
    }
}
