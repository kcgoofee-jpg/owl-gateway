//! 账号保护（guard）需要的查询：启动时回填最近 3 小时的出图时间等。

use crate::{Db, Result, value};

impl Db {
    /// 最近 3 小时成功出图的 (token_id, ts, images)，按时间排序（
    /// 3.0 起多取 token_id 以便按账号补回；老行 / 没记账号的为 None）。
    pub async fn recent_image_stamps(&self, since: f64) -> Result<Vec<(Option<String>, f64, i64)>> {
        self.read(move |c| {
            let mut stmt = c.prepare(
                "SELECT token_id, ts, images FROM usage_log WHERE ts>? AND status='ok' AND kind LIKE 'image%' AND images>0 ORDER BY ts",
            )?;
            let rows = stmt.query_map([since], |r| {
                Ok((
                    r.get::<_, Option<String>>(0)?.filter(|t| !t.is_empty()),
                    value::f64_or(r, 1, 0.0)?,
                    value::i64_or(r, 2, 0)?,
                ))
            })?;
            rows.collect()
        })
        .await
    }

    /// `SELECT MIN(ts) FROM usage_log`
    pub async fn first_log_ts(&self) -> Result<Option<f64>> {
        self.read(|c| {
            c.query_row("SELECT MIN(ts) FROM usage_log", [], |r| {
                value::f64_opt(r, 0)
            })
        })
        .await
    }

    /// Successful images since `since`: (count, average generation ms, average queue wait ms).
    pub async fn image_speed(&self, since: f64) -> Result<(i64, Option<f64>, Option<f64>)> {
        self.read(move |c| {
            c.query_row(
                "SELECT COUNT(*), AVG(dur_ms), AVG(wait_ms) FROM usage_log \
                 WHERE ts >= ? AND status = 'ok' AND kind IN ('image', 'image_stream')",
                [since],
                |r| Ok((r.get(0)?, value::f64_opt(r, 1)?, value::f64_opt(r, 2)?)),
            )
        })
        .await
    }

    /// `SELECT MAX(ts) FROM usage_log`（停机检测）。
    pub async fn last_log_ts(&self) -> Result<Option<f64>> {
        self.read(|c| {
            c.query_row("SELECT MAX(ts) FROM usage_log", [], |r| {
                value::f64_opt(r, 0)
            })
        })
        .await
    }
}
