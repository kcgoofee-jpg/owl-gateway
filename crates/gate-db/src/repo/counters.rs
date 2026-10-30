//! 每日计数器、额度基线、全站汇总。

use super::{OrderedPairs, marks};
use crate::{Db, DbError, Result, value};
use gate_core::{clock::Clock, primitives};
use rusqlite::params;
use serde::Serialize;
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

#[derive(Debug, Clone, Serialize)]
pub struct OverviewToday {
    pub images: i64,
    /// 成员出图：不含已删除的 Key；上面的 images 是账号真实消耗。
    pub member_images: i64,
    pub anlas: f64,
    pub text_tokens: i64,
    pub requests: i64,
    pub v5: i64,
    pub unconfirmed_requests: i64,
    pub unconfirmed_anlas: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct OverviewWeekDay {
    pub day: String,
    pub images: i64,
    pub anlas: f64,
    pub text_tokens: i64,
    pub requests: i64,
    pub v5: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct OverviewMonth {
    pub anlas: f64,
    pub unconfirmed_requests: i64,
    pub unconfirmed_anlas: f64,
}

/// 后台总览（键顺序固定，后台前端按它读）。
#[derive(Debug, Clone, Serialize)]
pub struct Overview {
    pub today: OverviewToday,
    pub active_members: i64,
    pub members_total: i64,
    pub keys_total: i64,
    pub keys_active: i64,
    pub week: Vec<OverviewWeekDay>,
    pub month: OverviewMonth,
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

    /// 后台总览。`today` 是服务日字符串；`week_days` 由调用方给（`state.week_days`）。
    /// 异常统计的时间窗按 `today` 当天的**本地自然日 0 点**起算（不按 7 点重置）。
    pub async fn overview(
        &self,
        today: &str,
        week_days: &[String],
        now: f64,
        clock: &Clock,
    ) -> Result<Overview> {
        let today = today.to_string();
        let week_days = week_days.to_vec();
        // today 当天的本地 0 点；month 窗从当月 1 日 0 点起，两窗同止于次日 0 点
        let date: jiff::civil::Date = today
            .parse()
            .map_err(|e| DbError::Invalid(format!("today 不是 YYYY-MM-DD：{e}")))?;
        let local = |d: jiff::civil::Date| {
            clock
                .local_midnight(&d.to_string())
                .ok_or_else(|| DbError::Invalid(format!("无法换算本地 0 点：{d}")))
        };
        let day_start = local(date)?;
        let month_start = local(date.first_of_month())?;
        let end = local(
            date.tomorrow()
                .map_err(|e| DbError::Invalid(e.to_string()))?,
        )?;

        self.read(move |c| {
            let one_i = |sql: &str, args: &[&dyn rusqlite::ToSql]| -> rusqlite::Result<i64> {
                c.query_row(sql, args, |r| value::i64_or(r, 0, 0))
            };
            let one_f = |sql: &str, args: &[&dyn rusqlite::ToSql]| -> rusqlite::Result<f64> {
                c.query_row(sql, args, |r| value::f64_or(r, 0, 0.0))
            };
            let today_images = one_i("SELECT COALESCE(SUM(images),0) FROM counters WHERE day=?", &[&today])?;
            let today_anlas = one_f(DAY_ANLAS_SQL, &[&today])?;
            let today_tokens = one_i("SELECT COALESCE(SUM(text_tokens),0) FROM counters WHERE day=?", &[&today])?;
            let today_requests = one_i("SELECT COALESCE(SUM(requests),0) FROM counters WHERE day=?", &[&today])?;
            let today_v5 = one_i(&day_v5_total_sql(), &[&today])?;
            let keys_total = one_i("SELECT COUNT(*) FROM api_keys", &[])?;
            let keys_active = one_i(
                "SELECT COUNT(*) FROM api_keys WHERE enabled=1 AND (expires_at IS NULL OR expires_at>?)",
                &[&now],
            )?;
            let active_members = one_i(
                "SELECT COUNT(DISTINCT c.key_id) \
                 FROM counters AS c \
                 JOIN api_keys AS k ON k.id = c.key_id \
                 WHERE c.day=? AND c.requests>0",
                &[&today],
            )?;
            let members_total = one_i("SELECT COUNT(*) FROM api_keys", &[])?;

            let mut anomaly = Vec::new();
            for start in [day_start, month_start] {
                let (n, sum) = c.query_row(
                    "SELECT COUNT(*), COALESCE(SUM(unconfirmed_anlas),0) FROM usage_log \
                     WHERE unconfirmed_anlas>0 AND ts>=? AND ts<?",
                    params![start, end],
                    |r| Ok((value::i64_or(r, 0, 0)?, value::f64_or(r, 1, 0.0)?)),
                )?;
                anomaly.push((n, primitives::round(sum, 2)));
            }

            let sql = format!(
                "SELECT c.day, \
                        SUM(c.images) AS images, SUM(c.anlas) AS anlas, \
                        SUM(c.text_tokens) AS text_tokens, SUM(c.requests) AS requests, \
                        SUM(c.v5) AS v5 \
                 FROM counters AS c \
                 WHERE c.day IN ({}) GROUP BY c.day",
                marks(week_days.len())
            );
            let mut stmt = c.prepare(&sql)?;
            let mut by_day: std::collections::HashMap<String, OverviewWeekDay> = Default::default();
            let rows = stmt.query_map(rusqlite::params_from_iter(week_days.iter()), |r| {
                let day = value::text_or(r, "day", "")?;
                Ok(OverviewWeekDay {
                    day: day.clone(),
                    images: value::i64_or(r, "images", 0)?,
                    anlas: value::f64_or(r, "anlas", 0.0)?,
                    text_tokens: value::i64_or(r, "text_tokens", 0)?,
                    requests: value::i64_or(r, "requests", 0)?,
                    v5: value::i64_or(r, "v5", 0)?,
                })
            })?;
            for r in rows {
                let r = r?;
                by_day.insert(r.day.clone(), r);
            }
            let week = week_days
                .iter()
                .map(|d| {
                    by_day.get(d).cloned().unwrap_or(OverviewWeekDay {
                        day: d.clone(),
                        images: 0,
                        anlas: 0.0,
                        text_tokens: 0,
                        requests: 0,
                        v5: 0,
                    })
                })
                .collect();

            let member_images = one_i(
                "SELECT COALESCE(SUM(c.images),0) FROM counters c JOIN api_keys k ON k.id=c.key_id \
                 WHERE c.day=?",
                &[&today],
            )?;
            let month_prefix: String = primitives::take_chars(&today, 7).to_string();
            let month_anlas = one_f(MONTH_ANLAS_ALL_SQL, &[&month_prefix])?;
            Ok(Overview {
                today: OverviewToday {
                    images: today_images,
                    member_images,
                    anlas: primitives::round(today_anlas, 2),
                    text_tokens: today_tokens,
                    requests: today_requests,
                    v5: today_v5,
                    unconfirmed_requests: anomaly[0].0,
                    unconfirmed_anlas: anomaly[0].1,
                },
                active_members,
                members_total,
                keys_total,
                keys_active,
                week,
                month: OverviewMonth {
                    anlas: primitives::round(month_anlas, 2),
                    unconfirmed_requests: anomaly[1].0,
                    unconfirmed_anlas: anomaly[1].1,
                },
            })
        })
        .await
    }
}
