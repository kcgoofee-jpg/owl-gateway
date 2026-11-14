//! 每日计数器、额度基线、全站汇总。

use super::OrderedPairs;
use crate::{Db, Result, value};
use rusqlite::params;
use serde_json::Value;

/// `get_counter` 的结果：已扣除「重置额度」基线后的当日用量（`anlas` 不会小于 0；没有记录就全是 0）。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Counter {
    pub images: i64,
    pub legacy_free_images: i64,
    pub anlas: f64,
    pub text_tokens: i64,
    pub requests: i64,
    pub v5: i64,
}

/// 全站当日 Anlas 消耗 / 全站本月 Anlas 消耗（`day_anlas_total` 和后台总览共用）。
const DAY_ANLAS_SQL: &str = "SELECT COALESCE(SUM(anlas), 0) AS a FROM counters WHERE day=?";
const MONTH_ANLAS_ALL_SQL: &str =
    "SELECT COALESCE(SUM(anlas),0) AS a FROM counters WHERE substr(day,1,7)=?";

fn day_v5_total_sql() -> String {
    "SELECT COALESCE(SUM(v5),0) AS c FROM counters WHERE day=?".into()
}

/// 「重置今日额度」要重置哪几项。V4.5 一定会重置；V5 和 Anlas 要明确勾选（站长 10-18：默认只重置 V4.5）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResetScope {
    pub v5: bool,
    pub anlas: bool,
}

impl ResetScope {
    /// 只重置 V4.5（默认）。
    pub const V45_ONLY: Self = Self {
        v5: false,
        anlas: false,
    };
    /// 三项都重置（旧券、旧快照用）。
    pub const ALL: Self = Self {
        v5: true,
        anlas: true,
    };

    /// 解析存库的 `v45,v5,anlas`；认不得的片段忽略，v45 总是在。
    pub fn parse(text: &str) -> Self {
        let has = |k: &str| text.split(',').any(|p| p.trim() == k);
        Self {
            v5: has("v5"),
            anlas: has("anlas"),
        }
    }

    /// 存库的规范写法：`v45`、`v45,v5`、`v45,anlas`、`v45,v5,anlas`。
    pub fn canonical(&self) -> String {
        let mut out = String::from("v45");
        if self.v5 {
            out.push_str(",v5");
        }
        if self.anlas {
            out.push_str(",anlas");
        }
        out
    }
}

impl Db {
    pub async fn get_counter(&self, key_id: i64, day: &str) -> Result<Counter> {
        let day = day.to_string();
        self.read(move |c| {
            let mut stmt = c.prepare(
                "SELECT c.*, COALESCE(o.anlas, 0) AS _quota_offset_anlas, \
                        COALESCE(o.v5, 0) AS _quota_offset_v5, \
                        COALESCE(o.legacy, 0) AS _quota_offset_legacy \
                 FROM counters AS c \
                 LEFT JOIN daily_quota_offsets AS o ON o.key_id=c.key_id AND o.day=c.day \
                 WHERE c.key_id=? AND c.day=?",
            )?;
            let mut rows = stmt.query(params![key_id, day])?;
            let Some(row) = rows.next()? else {
                return Ok(Counter::default());
            };
            let anlas =
                value::f64_or(row, "anlas", 0.0)? - value::f64_or(row, "_quota_offset_anlas", 0.0)?;
            let v5 = value::i64_or(row, "v5", 0)? - value::i64_or(row, "_quota_offset_v5", 0)?;
            let legacy = value::i64_or(row, "legacy_free_images", 0)?
                - value::i64_or(row, "_quota_offset_legacy", 0)?;
            Ok(Counter {
                images: value::i64_or(row, "images", 0)?,
                legacy_free_images: legacy.max(0),
                anlas: anlas.max(0.0),
                text_tokens: value::i64_or(row, "text_tokens", 0)?,
                requests: value::i64_or(row, "requests", 0)?,
                v5: v5.max(0),
            })
        })
        .await
    }

    /// 一次取回某个服务日**全部** Key 的计数（一次查询）。口径与 [`Db::get_counter`] 完全一致：
    /// `anlas` / `v5` / `legacy_free_images` 都扣除当日「重置额度」基线，且不小于 0。
    ///
    /// 后台列表用：370 把 Key 逐把 `get_counter` 要 370 次读库（每几秒重拉一次），这里一条 SQL 全取回。
    pub async fn counter_map(&self, day: &str) -> Result<std::collections::HashMap<i64, Counter>> {
        let day = day.to_string();
        self.read(move |c| {
            let mut stmt = c.prepare(
                "SELECT c.key_id, c.images, c.legacy_free_images, c.anlas, c.text_tokens, c.requests, c.v5, \
                        COALESCE(o.anlas, 0) AS _quota_offset_anlas, \
                        COALESCE(o.v5, 0) AS _quota_offset_v5, \
                        COALESCE(o.legacy, 0) AS _quota_offset_legacy \
                 FROM counters AS c \
                 LEFT JOIN daily_quota_offsets AS o ON o.key_id=c.key_id AND o.day=c.day \
                 WHERE c.day=?",
            )?;
            let rows = stmt.query_map(params![day], |row| {
                let anlas = value::f64_or(row, "anlas", 0.0)?
                    - value::f64_or(row, "_quota_offset_anlas", 0.0)?;
                let v5 = value::i64_or(row, "v5", 0)? - value::i64_or(row, "_quota_offset_v5", 0)?;
                let legacy = value::i64_or(row, "legacy_free_images", 0)?
                    - value::i64_or(row, "_quota_offset_legacy", 0)?;
                Ok((
                    value::i64_or(row, "key_id", 0)?,
                    Counter {
                        images: value::i64_or(row, "images", 0)?,
                        legacy_free_images: legacy.max(0),
                        anlas: anlas.max(0.0),
                        text_tokens: value::i64_or(row, "text_tokens", 0)?,
                        requests: value::i64_or(row, "requests", 0)?,
                        v5: v5.max(0),
                    },
                ))
            })?;
            let mut out = std::collections::HashMap::new();
            for row in rows {
                let (key_id, counter) = row?;
                out.insert(key_id, counter);
            }
            Ok(out)
        })
        .await
    }

    /// 重置单个 Key 当日的可用额度基线：V4.5 免费图一定重置，V5 / Anlas 按 `scope`；
    /// 保留原始记账用量，以重置时的累计量更新基线，没勾选的项保持原来的基线不动。
    /// 已删除的 Key 被触发器 `prevent_deleted_key_quota_reset` 静默忽略。
    pub async fn reset_daily_image_quota(
        &self,
        key_id: i64,
        day: &str,
        scope: ResetScope,
    ) -> Result<()> {
        let day = day.to_string();
        self.write(move |tx| {
            tx.execute(
                "INSERT INTO daily_quota_offsets (key_id, day, anlas, v5, legacy) \
                 SELECT key_id, day, CASE WHEN ?3 THEN anlas ELSE 0 END, CASE WHEN ?4 THEN v5 ELSE 0 END, \
                        legacy_free_images \
                 FROM counters WHERE key_id=?1 AND day=?2 \
                 ON CONFLICT(key_id, day) DO UPDATE SET \
                   anlas=CASE WHEN ?3 THEN excluded.anlas ELSE daily_quota_offsets.anlas END, \
                   v5=CASE WHEN ?4 THEN excluded.v5 ELSE daily_quota_offsets.v5 END, \
                   legacy=excluded.legacy",
                params![key_id, day, scope.anlas, scope.v5],
            )?;
            Ok(())
        })
        .await
    }

    pub async fn month_anlas(&self, key_id: i64, month: &str) -> Result<f64> {
        let month = month.to_string();
        self.read(move |c| {
            c.query_row(
                "SELECT COALESCE(SUM(anlas),0) AS a FROM counters WHERE key_id=? AND substr(day,1,7)=?",
                params![key_id, month],
                |r| value::f64_or(r, "a", 0.0),
            )
        })
        .await
    }

    /// 所有成员当日 V5 消耗，均计入共享预算。
    /// 全站当日出图张数（不含已删除 Key 的账号真实消耗，和后台总览同一个口径）。
    pub async fn day_images_total(&self, day: &str) -> Result<i64> {
        let day = day.to_string();
        self.read(move |c| {
            c.query_row(
                "SELECT COALESCE(SUM(images),0) AS c FROM counters WHERE day=?",
                [day],
                |r| value::i64_or(r, "c", 0),
            )
        })
        .await
    }

    pub async fn day_v5_total(&self, day: &str) -> Result<i64> {
        let day = day.to_string();
        self.read(move |c| c.query_row(&day_v5_total_sql(), [day], |r| value::i64_or(r, "c", 0)))
            .await
    }

    /// 全站当日消耗的 Anlas 总数。
    pub async fn day_anlas_total(&self, day: &str) -> Result<f64> {
        let day = day.to_string();
        self.read(move |c| c.query_row(DAY_ANLAS_SQL, [day], |r| value::f64_or(r, "a", 0.0)))
            .await
    }

    /// 全站所有 Key 本月的 Anlas 消耗（全站预算总闸）。
    pub async fn month_anlas_all(&self, month: &str) -> Result<f64> {
        let month = month.to_string();
        self.read(move |c| {
            c.query_row(MONTH_ANLAS_ALL_SQL, [month], |r| value::f64_or(r, "a", 0.0))
        })
        .await
    }

    /// 每个 Key 近一段时间的用量汇总（来自每日计数器）；值里带 key_id。
    pub async fn member_usage(&self, since_day: &str) -> Result<OrderedPairs<i64, Value>> {
        let since_day = since_day.to_string();
        self.read(move |c| {
            let mut stmt = c.prepare(
                "SELECT key_id, COALESCE(SUM(images),0) AS images, COALESCE(SUM(v5),0) AS v5, \
                        COALESCE(SUM(anlas),0) AS anlas, COALESCE(SUM(text_tokens),0) AS text_tokens, \
                        COALESCE(SUM(requests),0) AS requests \
                 FROM counters WHERE day>=? GROUP BY key_id",
            )?;
            let rows = stmt.query_map([since_day], |r| {
                Ok((value::i64_or(r, "key_id", 0)?, value::row_json(r)?))
            })?;
            // GROUP BY key_id 之后键已经唯一
            rows.collect()
        })
        .await
    }
}
