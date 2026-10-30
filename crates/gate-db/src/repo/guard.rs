//! 账号保护（guard）需要的查询：启动时回填最近 3 小时的出图时间等。

use crate::{Db, Result, value};

impl Db {
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
