//! 定时任务（额度同步 / 自动驾驶 / 上游快照 / 维护循环 / 闲置回收）要用的查询与写入。
//!
//! 定时任务（额度同步 / 上游快照 / 维护循环 / 闲置回收）要用的查询与写入，
//! 按「同一个任务读到的一批数据」收成一个方法一次读完。
//!
//! SQL 文本固定（包括没有 `ORDER BY` 的地方：行顺序由 SQLite 决定），快照测试按它对拍；
//! 这里**不做任何「顺手优化」**。

use crate::{Db, Result, value};
use gate_core::autopilot::BreakerRows;
use gate_core::quota::{ManagedKey, QuietInputs, QuietSnap, quiet_count_pairs, quiet_snaps_since};
use rusqlite::params;

fn count_i64(
    c: &rusqlite::Connection,
    sql: &str,
    args: impl rusqlite::Params,
) -> rusqlite::Result<i64> {
    c.query_row(sql, args, |r| value::i64_or(r, 0, 0))
}

impl Db {
    // ------------------------------------------------------------------ 额度同步 / V5 电量记录

    /// `quiet_recovery` 的输入：72 小时内「主动读」的快照，以及相邻两条（间隔 ≤ 3 小时）之间的 V5 出图条数。
    pub async fn quota_quiet_inputs(&self, now: f64) -> Result<QuietInputs> {
        self.read(move |c| {
            let snaps: Vec<QuietSnap> = {
                let mut stmt = c.prepare(
                    "SELECT ts, v5_percent FROM upstream_snapshots WHERE ts>? AND fresh=1 AND v5_percent IS NOT NULL ORDER BY ts",
                )?;
                stmt.query_map([quiet_snaps_since(now)], |r| {
                    Ok(QuietSnap {
                        ts: value::f64_or(r, 0, 0.0)?,
                        percent: value::f64_or(r, 1, 0.0)?,
                    })
                })?
                .collect::<rusqlite::Result<_>>()?
            };
            let mut counts = vec![0_i64; snaps.len().saturating_sub(1)];
            for i in quiet_count_pairs(&snaps) {
                counts[i] = count_i64(
                    c,
                    "SELECT COUNT(*) FROM usage_log WHERE ts>? AND ts<=? AND status='ok' AND model LIKE '%diffusion-5%'",
                    params![snaps[i].ts, snaps[i + 1].ts],
                )?;
            }
            Ok(QuietInputs { snaps, counts })
        })
        .await
    }

    /// `quota_auto=1` 的成员 Key 当前的额度（额度同步用它数「要改几把」）。
    pub async fn quota_managed_keys(&self) -> Result<Vec<ManagedKey>> {
        self.read(|c| {
            let mut stmt = c.prepare(
                "SELECT daily_images, daily_v5, image_model_scope FROM api_keys \
                 WHERE quota_auto=1",
            )?;
            stmt.query_map([], |r| {
                Ok(ManagedKey {
                    daily_images: value::i64_or(r, 0, 0)?,
                    daily_v5: value::i64_or(r, 1, 0)?,
                    image_model_scope: value::text_or(r, 2, "")?,
                })
            })?
            .collect()
        })
        .await
    }

    /// 把手动额度下发给所有 `quota_auto=1` 的成员 Key。
    pub async fn quota_apply_to_keys(&self, daily_images: i64, daily_v5: i64) -> Result<()> {
        self.write(move |tx| {
            tx.execute(
                "UPDATE api_keys SET daily_images=?, daily_v5=?, image_model_scope='all' \
                 WHERE quota_auto=1",
                params![daily_images, daily_v5],
            )?;
            Ok(())
        })
        .await
    }

    // ------------------------------------------------------------------ 自动驾驶（autopilot.run）

    /// 熔断统计：`since` 之后上游真正出错的次数与出图请求总数（没有行时 SUM 为 NULL），
    /// 以及这些失败来自多少把不同的 Key。
    pub async fn autopilot_breaker_rows(&self, since: f64) -> Result<BreakerRows> {
        const FAIL: &str = "status='error' AND (up_status>=500 OR up_status BETWEEN 200 AND 299)";
        self.read(move |c| {
            c.query_row(
                &format!(
                    "SELECT SUM(CASE WHEN {FAIL} THEN 1 ELSE 0 END), COUNT(*), \
                     COUNT(DISTINCT CASE WHEN {FAIL} AND key_id IS NOT NULL THEN key_id END) \
                     FROM usage_log WHERE ts>=? AND kind LIKE 'image%' AND status IN ('ok','error')"
                ),
                [since],
                |r| {
                    Ok(BreakerRows {
                        fails: value::i64_opt(r, 0)?,
                        total: value::i64_opt(r, 1)?,
                        fail_keys: value::i64_or(r, 2, 0)?,
                    })
                },
            )
        })
        .await
    }

    /// 每把 Key 近 1 小时的被拒次数（排除「Key 暂停」本身和全站层面的拒绝）。
    pub async fn autopilot_rejects(
        &self,
        since: f64,
        skip: &[&str],
        half: &[&str],
    ) -> Result<Vec<(i64, i64)>> {
        let skip = skip.join("','");
        let half = half.join("','");
        self.read(move |c| {
            // `half` 里的原因按半次算（整数运算：先按 2 倍累加，最后除以 2 向下取整）
            let mut stmt = c.prepare(&format!(
                "SELECT key_id, SUM(CASE WHEN reason IN ('{half}') THEN 1 ELSE 2 END)/2 FROM usage_log \
                 WHERE ts>=? AND status='rejected' \
                 AND key_id IS NOT NULL AND reason NOT IN ('{skip}') GROUP BY key_id"
            ))?;
            stmt.query_map([since], |r| {
                Ok((value::i64_or(r, 0, 0)?, value::i64_or(r, 1, 0)?))
            })?
            .collect()
        })
        .await
    }

    /// 参与单 Key 限流判断的成员 Key（id, name）。
    pub async fn autopilot_keys(&self) -> Result<Vec<(i64, String)>> {
        self.read(|c| {
            // 没写 ORDER BY：全表扫描就是 rowid 顺序，这里写明，免得换了查询计划后顺序漂移
            let mut stmt =
                c.prepare("SELECT id, name FROM api_keys WHERE enabled=1 ORDER BY id")?;
            stmt.query_map([], |r| {
                Ok((value::i64_or(r, 0, 0)?, value::text_or(r, 1, "None")?))
            })?
            .collect()
        })
        .await
    }

    // ------------------------------------------------------------------ 上游 V5 快照（定时任务 upstream_snapshot）

    /// `SELECT MAX(ts) FROM upstream_snapshots`
    pub async fn latest_snapshot_ts(&self) -> Result<Option<f64>> {
        self.read(|c| {
            c.query_row("SELECT MAX(ts) FROM upstream_snapshots", [], |r| {
                value::f64_opt(r, 0)
            })
        })
        .await
    }

    /// 每小时记一次上游真实状态。
    pub async fn insert_upstream_snapshot(
        &self,
        ts: f64,
        v5_percent: Option<f64>,
        v5_rate: Option<f64>,
        anlas: Option<i64>,
        fresh: i64,
    ) -> Result<()> {
        self.write(move |tx| {
            tx.execute(
                "INSERT INTO upstream_snapshots(ts, v5_percent, v5_rate, anlas, fresh) VALUES (?,?,?,?,?)",
                params![ts, v5_percent, v5_rate, anlas, fresh],
            )?;
            Ok(())
        })
        .await
    }

    // ------------------------------------------------------------------ 维护循环 / 闲置回收

    /// 关停时把 WAL 并回主库（`PRAGMA wal_checkpoint(TRUNCATE)`）。返回 `(被占用?, WAL 页数, 已并回页数)`。
    pub async fn wal_checkpoint(&self) -> Result<(i64, i64, i64)> {
        self.write_raw(|conn| {
            conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })
        })
        .await
    }

    // ------------------------------------------------------------------ 测试支撑

    /// 在一个事务里依次执行一批 SQL（对拍夹具的 `seed_sql`）。
    pub async fn run_script(&self, statements: Vec<String>) -> Result<()> {
        self.write(move |tx| {
            for s in &statements {
                tx.execute_batch(s)?;
            }
            Ok(())
        })
        .await
    }
}
