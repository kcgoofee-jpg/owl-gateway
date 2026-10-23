//! 后台「运营 / 监控」几个接口用到的裸 SQL（旧版 `admin.py` / `registration.py` 里直接写在路由里的查询）。

use crate::{Db, Result, value};

impl Db {
    /// 上游实际返回的状态码分布（`usage_log.up_status`，`since`（时间戳）之后），次数多的在前；
    /// 0 = 没发到 NovelAI（网关自己拦下、排队超时或客户端断开）。次数相同的先后顺序由 SQLite 决定（旧版同样）。
    ///
    /// 旧版 SQL 原样：`SELECT up_status, COUNT(*) FROM usage_log WHERE ts>=? GROUP BY up_status ORDER BY 2 DESC`。
    pub async fn upstream_status_distribution(&self, since: i64) -> Result<Vec<(i64, i64)>> {
        self.read(move |c| {
            let mut stmt = c.prepare(
                "SELECT up_status, COUNT(*) FROM usage_log WHERE ts>=? GROUP BY up_status ORDER BY 2 DESC",
            )?;
            stmt.query_map([since], |r| {
                Ok((value::i64_or(r, 0, 0)?, value::i64_or(r, 1, 0)?))
            })?
            .collect()
        })
        .await
    }

    /// 拒绝与失败的原因分布：`(原因码, 状态 rejected|error|cancelled, 详情前 60 字, 次数)`，按次数从多到少，最多 400 行。
    /// 没有原因码的（上游错误、老记录），以及 `known` 之外的码（已删除机制留下的历史码）带上详情前缀，
    /// 交给调用方按首句归类；`known` 里的码详情不取（同一个码的详情常带时间、数字，取了会拆成很多行）。
    pub async fn failure_reason_distribution(
        &self,
        since: i64,
        known: &[&str],
    ) -> Result<Vec<(String, String, String, i64)>> {
        // known 全是代码里的常量（字母和下划线），直接拼进 SQL 的 IN 列表
        let list = known
            .iter()
            .filter(|k| k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
            .map(|k| format!("'{k}'"))
            .collect::<Vec<_>>()
            .join(",");
        self.read(move |c| {
            let sql = format!(
                "SELECT reason, status, CASE WHEN reason IN ({list}) THEN '' ELSE substr(detail,1,60) END, COUNT(*) \
                 FROM usage_log WHERE ts>=? AND status!='ok' GROUP BY 1,2,3 ORDER BY 4 DESC LIMIT 400"
            );
            let mut stmt = c.prepare(&sql)?;
            stmt.query_map([since], |r| {
                Ok((
                    value::text_or(r, 0, "")?,
                    value::text_or(r, 1, "")?,
                    value::text_or(r, 2, "")?,
                    value::i64_or(r, 3, 0)?,
                ))
            })?
            .collect()
        })
        .await
    }
}
