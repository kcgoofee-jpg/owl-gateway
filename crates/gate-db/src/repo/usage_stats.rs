//! 用量统计：按天 / 按模型 / 按 Key 聚合查询，只读。写入与单条日志查询在 `repo::usage`。

use super::OrderedPairs;
use super::usage::*;
use crate::{Db, Result, value};
use gate_core::primitives;
use serde_json::Value;

impl Db {
    pub async fn member_milestones(
        &self,
        since: f64,
        reason_label: &(dyn Fn(&str, &str) -> String + Sync),
    ) -> Result<OrderedPairs<i64, Milestone>> {
        // 先在读连接里取回原始分组行，再在写侧合并（reason_label 不必跨线程）
        type RawWhy = (i64, String, String, i64);
        type RawImg = (i64, Option<f64>, Option<f64>);
        let (img, rej, why): (Vec<RawImg>, Vec<(i64, i64)>, Vec<RawWhy>) = self
            .read(move |c| {
                let img = c
                    .prepare(
                        "SELECT key_id, MIN(ts), MAX(ts) FROM usage_log \
                         WHERE status='ok' AND kind IN ('image','image_stream') AND key_id IS NOT NULL GROUP BY key_id",
                    )?
                    .query_map([], |r| {
                        Ok((
                            value::i64_or(r, 0, 0)?,
                            value::f64_opt(r, 1)?,
                            value::f64_opt(r, 2)?,
                        ))
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                let rej = c
                    .prepare(
                        "SELECT key_id, COUNT(*) FROM usage_log WHERE status='rejected' AND ts>=? AND key_id IS NOT NULL \
                         GROUP BY key_id",
                    )?
                    .query_map([since], |r| Ok((value::i64_or(r, 0, 0)?, value::i64_or(r, 1, 0)?)))?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                let why = c
                    .prepare(
                        "SELECT key_id, reason, MIN(detail), COUNT(*) FROM usage_log \
                         WHERE status='rejected' AND ts>=? AND key_id IS NOT NULL \
                         GROUP BY key_id, CASE WHEN reason!='' THEN reason ELSE substr(detail, 1, 24) END",
                    )?
                    .query_map([since], |r| {
                        Ok((
                            value::i64_or(r, 0, 0)?,
                            value::text_or(r, 1, "")?,
                            value::text_or(r, 2, "")?,
                            value::i64_or(r, 3, 0)?,
                        ))
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                Ok((img, rej, why))
            })
            .await?;

        let mut out: OrderedPairs<i64, Milestone> = Vec::new();
        fn slot(out: &mut OrderedPairs<i64, Milestone>, key: i64) -> &mut Milestone {
            let idx = match out.iter().position(|(k, _)| *k == key) {
                Some(i) => i,
                None => {
                    out.push((
                        key,
                        Milestone {
                            first_image_at: None,
                            last_image_at: None,
                            rejected_24h: 0,
                            rejected_why: None,
                        },
                    ));
                    out.len() - 1
                }
            };
            &mut out[idx].1
        }
        for (key_id, first, last) in img {
            // out[key] = {...}：覆盖写（GROUP BY 之后键唯一）
            let m = slot(&mut out, key_id);
            m.first_image_at = first;
            m.last_image_at = last;
            m.rejected_24h = 0;
        }
        for (key_id, n) in rej {
            slot(&mut out, key_id).rejected_24h = n;
        }
        // 被拒原因：同一句话（只差数字）合并，按次数降序取前 3（次数相同保持首次出现的顺序）
        let mut by_key: OrderedPairs<i64, Vec<(String, i64)>> = Vec::new();
        for (key_id, code, detail, n) in why {
            let label = reason_label(&code, &detail);
            let idx = match by_key.iter().position(|(k, _)| *k == key_id) {
                Some(i) => i,
                None => {
                    by_key.push((key_id, Vec::new()));
                    by_key.len() - 1
                }
            };
            let items = &mut by_key[idx].1;
            match items.iter_mut().find(|(l, _)| *l == label) {
                Some(e) => e.1 += n,
                None => items.push((label, n)),
            }
        }
        for (key_id, mut items) in by_key {
            items.sort_by_key(|(_, n)| std::cmp::Reverse(*n)); // 稳定排序，同 Python sorted(key=-n)
            items.truncate(3);
            slot(&mut out, key_id).rejected_why = Some(items);
        }
        Ok(out)
    }

    /// 统计窗内的出图汇总（`/self-register/bot/stats` 用：今日全站张数、平均/最长排队）。
    pub async fn image_stats_since(&self, since: f64) -> Result<ImageStats> {
        self.read(move |c| {
            c.query_row(
                "SELECT COALESCE(SUM(images),0), COUNT(*), AVG(wait_ms), MAX(wait_ms) \
                 FROM usage_log \
                 WHERE kind IN ('image', 'image_stream') AND status='ok' AND ts>=?",
                [&since],
                |r| {
                    Ok(ImageStats {
                        images: r.get(0)?,
                        ok_count: r.get(1)?,
                        avg_wait_ms: r.get(2)?,
                        max_wait_ms: r.get(3)?,
                    })
                },
            )
        })
        .await
    }

    /// 出图请求的 (ts, model, status, images, wait_ms, dur_ms, up_status, detail)，供上游表现分析。
    pub async fn image_perf_rows(&self, since: f64) -> Result<Vec<ImagePerfRow>> {
        self.read(move |c| {
            let mut stmt = c.prepare(
"SELECT ts, model, status, images, wait_ms, dur_ms, up_status, detail FROM usage_log \
                 WHERE ts>=? AND kind IN ('image','image_stream') AND status IN ('ok','error') \
                 ORDER BY ts"
            )?;
            stmt.query_map([since], |r| {
                Ok((
                    value::f64_or(r, 0, 0.0)?,
                    value::text_or(r, 1, "None")?,
                    value::text_or(r, 2, "None")?,
                    value::i64_or(r, 3, 0)?,
                    value::i64_or(r, 4, 0)?,
                    value::i64_or(r, 5, 0)?,
                    value::i64_or(r, 6, 0)?,
                    value::text_or(r, 7, "None")?,
                ))
            })?
            .collect()
        })
        .await
    }

    /// `since` 之后出图请求的成功 / 上游失败次数（不含参数错误等被拒请求），用于首页稳定性图标。
    pub async fn image_stability(&self, since: f64) -> Result<ImageStability> {
        self.read(move |c| {
            let mut stmt = c.prepare(
                "SELECT status, COUNT(*) FROM usage_log \
                 WHERE ts>=? AND kind IN ('image','image_stream') AND status IN ('ok','error') \
                 GROUP BY status",
            )?;
            let rows = stmt.query_map([since], |r| {
                Ok((value::text_or(r, 0, "")?, value::i64_or(r, 1, 0)?))
            })?;
            let mut out = ImageStability { ok: 0, error: 0 };
            for row in rows {
                let (status, n) = row?;
                match status.as_str() {
                    "ok" => out.ok = n,
                    "error" => out.error = n,
                    _ => {}
                }
            }
            Ok(out)
        })
        .await
    }

    /// 按 kind / status 汇总 `since` 之后的用量日志（日志保留期内有效）。
    pub async fn usage_by_kind(&self, since: f64) -> Result<Vec<Value>> {
        self.read(move |c| {
            let mut stmt = c.prepare(
                "SELECT kind, status, COUNT(*) AS n, COALESCE(SUM(images),0) AS images, \
                        COALESCE(SUM(tokens),0) AS tokens, COALESCE(SUM(anlas),0) AS anlas, \
                        COUNT(DISTINCT key_id) AS users \
                 FROM usage_log WHERE ts>=? GROUP BY kind, status",
            )?;
            stmt.query_map([since], value::row_json)?.collect()
        })
        .await
    }

    /// 每把 Key 成功生成的张数（含流式完成的图）。
    pub async fn generated_image_totals(
        &self,
        key_id: Option<i64>,
    ) -> Result<OrderedPairs<i64, i64>> {
        self.read(move |c| {
            let mut sql = "SELECT key_id, COALESCE(SUM(images), 0) AS images \
                           FROM usage_log \
                           WHERE key_id IS NOT NULL AND kind IN ('image', 'image_stream') \
                             AND status='ok'"
                .to_string();
            if key_id.is_some() {
                sql.push_str(" AND key_id=?");
            }
            sql.push_str(" GROUP BY key_id");
            let mut stmt = c.prepare(&sql)?;
            let map = |r: &rusqlite::Row<'_>| {
                Ok((
                    value::i64_or(r, "key_id", 0)?,
                    value::i64_or(r, "images", 0)?,
                ))
            };
            match key_id {
                Some(k) => stmt.query_map([k], map)?.collect(),
                None => stmt.query_map([], map)?.collect(),
            }
        })
        .await
    }

    /// 指定 Key 的平均出图耗时（秒）及全站近 24 小时参考耗时。
    /// 该 Key 近 24 小时成功出图不足 3 次时改用它的历史全部记录（有的话）。
    pub async fn user_image_timing(&self, key_id: i64, now: f64) -> Result<ImageTiming> {
        let day_ago = now - 86400.0;
        self.read(move |c| {
            type Agg = (i64, Option<f64>, Option<f64>);
            let agg = |sql: &str, args: &[&dyn rusqlite::ToSql]| -> rusqlite::Result<Agg> {
                c.query_row(sql, args, |r| {
                    Ok((
                        value::i64_or(r, 0, 0)?,
                        value::f64_opt(r, 1)?,
                        value::f64_opt(r, 2)?,
                    ))
                })
            };
            let (mut cnt, mut avg_dur, mut avg_wait) = agg(
                "SELECT COUNT(*), AVG(dur_ms), AVG(wait_ms) \
                 FROM usage_log \
                 WHERE key_id=? AND kind IN ('image', 'image_stream') AND status='ok' AND dur_ms>0 AND ts>=?",
                &[&key_id, &day_ago],
            )?;
            if cnt < 3 {
                let all = agg(
                    "SELECT COUNT(*), AVG(dur_ms), AVG(wait_ms) \
                     FROM usage_log \
                     WHERE key_id=? AND kind IN ('image', 'image_stream') AND status='ok' AND dur_ms>0",
                    &[&key_id],
                )?;
                if all.0 > 0 {
                    (cnt, avg_dur, avg_wait) = all;
                }
            }
            let site: Option<f64> = c.query_row(
                "SELECT AVG(dur_ms) FROM usage_log \
                 WHERE kind IN ('image', 'image_stream') AND status='ok' AND dur_ms>0 AND ts>=?",
                [day_ago],
                |r| value::f64_opt(r, 0),
            )?;
            let enabled = |v: Option<f64>| v.filter(|x| *x != 0.0);
            let site_avg = enabled(site).unwrap_or(9000.0);
            Ok(ImageTiming {
                count: cnt,
                avg_dur_s: enabled(avg_dur).map(|x| primitives::round(x / 1000.0, 1)),
                avg_wait_s: enabled(avg_wait).map(|x| primitives::round(x / 1000.0, 1)),
                site_avg_dur_s: primitives::round(site_avg / 1000.0, 1),
            })
        })
        .await
    }
}

/// [`UsageRepo::image_stats_since`] 的返回：一个时间窗内的成功出图汇总。
pub struct ImageStats {
    /// 成功出图总张数（`SUM(images)`，通常每请求 1 张）。
    pub images: i64,
    /// 成功请求数。
    pub ok_count: i64,
    /// 排队等待均值（毫秒）；窗口内没有成功请求时为 `None`。
    pub avg_wait_ms: Option<f64>,
    /// 排队等待最长值（毫秒）；同上为 `None`。
    pub max_wait_ms: Option<f64>,
}
