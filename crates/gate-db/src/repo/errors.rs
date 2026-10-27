//! 状态页的小时聚合写入。Bug 追踪（error_events）10-27 删除，本文件只剩这两条状态页辅助。

use crate::{Db, Result, value};

impl Db {
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
