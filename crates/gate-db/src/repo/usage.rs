//! 用量日志：成功记账、拒绝 / 失败日志、各类统计查询。

use super::{INSERT_LOG, LOG_COLUMNS, PyMap, SqlValue, UPSERT_COUNTERS, marks};
use crate::{Db, Result, value};
use gate_core::pyc;
use rusqlite::params;
use serde::Serialize;
use serde_json::Value;

/// 一次成功请求的记账内容（`record_success`）。字段缺省值见各字段。
#[derive(Debug, Clone, Default)]
pub struct Success {
    pub key_id: i64,
    pub key_name: String,
    pub kind: String,
    pub model: String,
    pub day: String,
    pub images: i64,
    pub anlas: f64,
    pub tokens: i64,
    pub v5: i64,
    pub legacy_free_images: i64,
    pub detail: String,
    pub unconfirmed_anlas: f64,
    pub wait_ms: i64,
    pub dur_ms: i64,
    pub up_status: i64,
    pub rid: String,
    /// 处理这条请求的上游账号；没发到上游为 None。
    pub token_id: Option<String>,
    /// 请求的 User-Agent / 来源 IP（原文；保留期见 `Db::purge_usage_origin`）。
    pub client: String,
    pub ip: String,
    /// Extra client headers (anti-abuse; purged with `client` / `src`).
    pub hints: ClientHints,
    /// Anlas the site paid from the upstream account itself (not charged to the member's counters):
    /// written to `usage_log.anlas` together with `anlas`, left out of `counters`.
    pub site_paid_anlas: f64,
}

/// 一条拒绝 / 失败日志（`add_log`）。
#[derive(Debug, Clone, Default)]
pub struct NewLog {
    pub key_id: Option<i64>,
    pub key_name: String,
    pub kind: String,
    pub model: String,
    pub status: String,
    pub images: i64,
    pub anlas: f64,
    pub tokens: i64,
    pub detail: String,
    pub unconfirmed_anlas: f64,
    pub wait_ms: i64,
    pub dur_ms: i64,
    pub up_status: i64,
    pub rid: String,
    pub reason: String,
    /// 选中过的上游账号（选号之后才失败的请求有值）；选号之前就被拒为 None。
    pub token_id: Option<String>,
    /// 请求的 User-Agent / 来源 IP（原文；保留期见 `Db::purge_usage_origin`）。
    pub client: String,
    pub ip: String,
    /// Extra client headers (anti-abuse; purged with `client` / `src`).
    pub hints: ClientHints,
}

/// 请求里除 UA / IP 之外的客户端特征（站长 10-23：防一人多号 / 共用 Key，要能从日志里举证）。
/// 都不含 Authorization / Cookie 原文；保留期和 `client` / `src` 相同（后台设置 usage_origin_keep_days，0 = 永久，默认永久；见 `Db::purge_usage_origin`）。
/// 空串写成 NULL。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClientHints {
    /// X-Requested-With（安卓 WebView 套壳 App 的包名），截到 80 个码点。
    pub xrw: String,
    /// Accept-Language，截到 80。
    pub lang: String,
    /// Sec-CH-UA / -Platform / -Mobile 拼成一行，截到 160。
    pub chua: String,
    /// 请求头指纹：16 位十六进制（见 gate-server `client_hints::header_fingerprint`）。
    pub hfp: String,
    /// 浏览器标记 Cookie `owl_bid` 的 sha256 前 16 位十六进制（不存原值）。
    pub bid: String,
    /// Origin（只留 scheme://host[:port]），截到 120。
    pub origin: String,
}

impl ClientHints {
    /// 写库用：按列长截断，空串 → NULL。顺序同 `INSERT_LOG` 末尾的 xrw, lang, chua, hfp, bid, origin。
    pub(crate) fn columns(&self) -> [Option<String>; 6] {
        let col = |s: &str, n: usize| {
            let t = pyc::take_chars(s.trim(), n);
            (!t.is_empty()).then(|| t.to_string())
        };
        [
            col(&self.xrw, 80),
            col(&self.lang, 80),
            col(&self.chua, 160),
            col(&self.hfp, 16),
            col(&self.bid, 16),
            col(&self.origin, 120),
        ]
    }
}

/// 日志筛选条件。
#[derive(Debug, Clone, Default)]
pub struct LogFilter {
    /// 0 与 None 一样表示不筛。
    pub key_id: Option<i64>,
    /// None = 不筛；Some(空) = 一条都不匹配（SQL 里绑定成 `0`）。
    pub kinds: Option<Vec<String>>,
    /// 空串 = 不筛。
    pub rid: String,
    /// 同一成员的多把 Key 一起查；空 = 不筛（与 `key_id` 同时给时两条都要满足）。
    pub key_ids: Vec<i64>,
    /// ok / rejected / error；空串 = 不筛。
    pub status: String,
    /// 来源 IP（`src` 原文，精确匹配）；空串 = 不筛。
    pub ip: String,
    /// 拒绝原因码（精确匹配）；空串 = 不筛。
    pub reason: String,
}

/// 按状态分的条数（`log_summary`）：`total` 含状态不是这三种的行。
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct LogSummary {
    pub total: i64,
    pub ok: i64,
    pub rejected: i64,
    pub error: i64,
}

impl LogSummary {
    fn add(&mut self, status: &str, n: i64) {
        self.total += n;
        match status {
            "ok" => self.ok += n,
            "rejected" => self.rejected += n,
            "error" => self.error += n,
            _ => {}
        }
    }
}

/// `member_milestones` 里每把 Key 的一项。`rejected_why` 只有被拒过的 Key 才有（序列化时省略）。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Milestone {
    pub first_image_at: Option<f64>,
    pub last_image_at: Option<f64>,
    pub rejected_24h: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rejected_why: Option<Vec<(String, i64)>>,
}

/// `image_perf_rows`：(ts, model, status, images, wait_ms, dur_ms, up_status, detail)
pub type ImagePerfRow = (f64, String, String, i64, i64, i64, i64, String);

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ImageStability {
    pub ok: i64,
    pub error: i64,
}

/// `user_image_timing` 的结果（秒，保留 1 位小数）。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ImageTiming {
    pub count: i64,
    pub avg_dur_s: Option<f64>,
    pub avg_wait_s: Option<f64>,
    pub site_avg_dur_s: f64,
}

/// 真正出图（`image` / `image_stream`）成功才刷新 last_image_at：闲置回收只认它。
/// 查额度、标签补全不算；放大、导演工具、Vibe 编码也不算，只用它们「保活」的 Key 不会被回收。
fn is_image_kind(kind: &str) -> bool {
    matches!(kind, "image" | "image_stream")
}

/// 组装日志筛选的 WHERE 子句：返回 (` WHERE ...` 或空串, 绑定参数)。
fn log_filter(f: &LogFilter) -> (String, Vec<SqlValue>) {
    let mut clauses: Vec<String> = Vec::new();
    let mut args: Vec<SqlValue> = Vec::new();
    if !f.rid.is_empty() {
        clauses.push("rid=?".into());
        args.push(SqlValue::Text(f.rid.clone()));
    }
    if let Some(k) = f.key_id.filter(|k| *k != 0) {
        clauses.push("key_id=?".into());
        args.push(SqlValue::Integer(k));
    }
    if !f.key_ids.is_empty() {
        clauses.push(format!("key_id IN ({})", marks(f.key_ids.len())));
        args.extend(f.key_ids.iter().copied().map(SqlValue::Integer));
    }
    if !f.status.is_empty() {
        clauses.push("status=?".into());
        args.push(SqlValue::Text(f.status.clone()));
    }
    if !f.ip.is_empty() {
        // `src!=''` 写出来才能用上部分索引 idx_log_src（WHERE src!=''）
        clauses.push("src!='' AND src=?".into());
        args.push(SqlValue::Text(f.ip.clone()));
    }
    if !f.reason.is_empty() {
        clauses.push("reason=?".into());
        args.push(SqlValue::Text(f.reason.clone()));
    }
    if let Some(kinds) = &f.kinds {
        if kinds.is_empty() {
            clauses.push("0".into());
        } else {
            clauses.push(format!("kind IN ({})", marks(kinds.len())));
            args.extend(kinds.iter().cloned().map(SqlValue::Text));
        }
    }
    let where_ = if clauses.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", clauses.join(" AND "))
    };
    (where_, args)
}

impl Db {
    /// 成功日志、额度与使用时间一起提交；写入失败时整笔回退（一个事务）。
    /// 写线程独占写连接，锁冲突由 `Db::write` 的 BUSY 重试兜住。
    pub async fn record_success(&self, s: &Success, now: f64) -> Result<()> {
        let s = s.clone();
        self.write(move |tx| {
            let h = s.hints.columns();
            tx.execute(
                INSERT_LOG,
                params![
                    now,
                    s.key_id,
                    s.key_name,
                    s.kind,
                    s.model,
                    "ok",
                    s.images,
                    s.anlas + s.site_paid_anlas,
                    s.tokens,
                    pyc::take_chars(&s.detail, 500),
                    s.unconfirmed_anlas,
                    s.wait_ms.max(0),
                    s.dur_ms.max(0),
                    s.up_status,
                    pyc::take_chars(&s.rid, 16),
                    "",
                    s.token_id,
                    pyc::take_chars(&s.client, 160),
                    pyc::take_chars(&s.ip, 64),
                    h[0],
                    h[1],
                    h[2],
                    h[3],
                    h[4],
                    h[5],
                ],
            )?;
            tx.execute(
                UPSERT_COUNTERS,
                params![
                    s.key_id,
                    s.day,
                    s.images,
                    s.anlas,
                    s.tokens,
                    1_i64,
                    s.v5,
                    s.legacy_free_images
                ],
            )?;
            tx.execute(
                "UPDATE api_keys SET last_used_at=?, \
                 last_image_at=CASE WHEN ? THEN ? ELSE last_image_at END WHERE id=?",
                params![now, i64::from(is_image_kind(&s.kind)), now, s.key_id],
            )?;
            Ok(())
        })
        .await
    }

    /// Anlas already spent on site-paid images through `token_id` (rows whose detail carries `marker`).
    pub async fn site_paid_spent(&self, token_id: &str, marker: &str) -> Result<f64> {
        let (token_id, pattern) = (token_id.to_string(), format!("%{marker}%"));
        self.read(move |c| {
            c.query_row(
                "SELECT COALESCE(SUM(anlas), 0) FROM usage_log \
                 WHERE token_id = ? AND status = 'ok' AND detail LIKE ?",
                params![token_id, pattern],
                |r| r.get(0),
            )
        })
        .await
    }

    pub async fn add_log(&self, l: &NewLog, now: f64) -> Result<()> {
        let l = l.clone();
        self.write(move |tx| {
            let h = l.hints.columns();
            tx.execute(
                INSERT_LOG,
                params![
                    now,
                    l.key_id,
                    pyc::take_chars(&l.key_name, 80),
                    l.kind,
                    pyc::take_chars(&l.model, 80),
                    l.status,
                    l.images,
                    l.anlas,
                    l.tokens,
                    pyc::take_chars(&l.detail, 500),
                    l.unconfirmed_anlas,
                    l.wait_ms.max(0),
                    l.dur_ms.max(0),
                    l.up_status,
                    pyc::take_chars(&l.rid, 16),
                    pyc::take_chars(&l.reason, 24),
                    l.token_id,
                    pyc::take_chars(&l.client, 160),
                    pyc::take_chars(&l.ip, 64),
                    h[0],
                    h[1],
                    h[2],
                    h[3],
                    h[4],
                    h[5],
                ],
            )?;
            Ok(())
        })
        .await
    }

    /// 清掉 `before`（Unix 秒）之前日志里的 User-Agent 和来源 IP（改成空串，日志本身保留）；返回清掉的行数。
    /// 站长 10-18：为筛脚本撞限速 / 转售记这两项，保留天数按后台设置 usage_origin_keep_days（0 = 永久，不调用；到期由每天的维护任务调用）。
    /// 10-23 起同一批清掉 `ClientHints` 的六列（改成 NULL）。
    pub async fn purge_usage_origin(&self, before: f64) -> Result<usize> {
        self.write(move |tx| {
            tx.execute(
                "UPDATE usage_log SET client='', src='', xrw=NULL, lang=NULL, chua=NULL, hfp=NULL, bid=NULL, origin=NULL \
                 WHERE ts<? AND (client!='' OR src!='' OR xrw IS NOT NULL OR lang IS NOT NULL \
                 OR chua IS NOT NULL OR hfp IS NOT NULL OR bid IS NOT NULL OR origin IS NOT NULL)",
                params![before],
            )
        })
        .await
    }

    /// 日志列表（新→旧）；`limit` 夹在 1..=500，`offset` 不小于 0。
    pub async fn list_logs(
        &self,
        limit: i64,
        offset: i64,
        filter: &LogFilter,
    ) -> Result<Vec<Value>> {
        let limit = limit.clamp(1, 500);
        let offset = offset.max(0);
        let (where_, mut args) = log_filter(filter);
        args.push(SqlValue::Integer(limit));
        args.push(SqlValue::Integer(offset));
        self.read(move |c| {
            let mut stmt = c.prepare(&format!(
                "SELECT {LOG_COLUMNS} FROM usage_log{where_} ORDER BY id DESC LIMIT ? OFFSET ?"
            ))?;
            stmt.query_map(rusqlite::params_from_iter(args.iter()), value::row_json)?
                .collect()
        })
        .await
    }

    /// 成员自己看的请求记录（首页「我的请求」）：只取这把 Key、`since` 之后、`id < before`（0 = 不限）
    /// 的行，`status` 空串 = 不筛。只给成员能看的列：不含 IP、客户端、上游账号。
    pub async fn member_logs(
        &self,
        key_id: i64,
        since: f64,
        before: i64,
        status: &str,
        limit: i64,
    ) -> Result<Vec<Value>> {
        let status = status.to_string();
        let limit = limit.clamp(1, 200);
        self.read(move |c| {
            let mut stmt = c.prepare(
                "SELECT id, rid, ts, kind, model, status, images, anlas, wait_ms, dur_ms, reason, detail \
                 FROM usage_log WHERE key_id=?1 AND ts>=?2 AND (?3=0 OR id<?3) \
                 AND (?4='' OR status=?4) ORDER BY id DESC LIMIT ?5",
            )?;
            stmt.query_map(params![key_id, since, before, status, limit], value::row_json)?
                .collect()
        })
        .await
    }

    /// 同一范围（这把 Key、`since` 之后）按状态分的条数。
    pub async fn member_log_summary(&self, key_id: i64, since: f64) -> Result<LogSummary> {
        self.read(move |c| {
            let mut stmt = c.prepare(
                "SELECT status, COUNT(*) FROM usage_log WHERE key_id=? AND ts>=? GROUP BY status",
            )?;
            let mut out = LogSummary::default();
            let rows = stmt.query_map(params![key_id, since], |r| {
                Ok((value::text_or(r, 0, "")?, value::i64_or(r, 1, 0)?))
            })?;
            for row in rows {
                let (status, n) = row?;
                out.add(&status, n);
            }
            Ok(out)
        })
        .await
    }

    /// 符合筛选的条数，按状态分（一次 GROUP BY）。
    pub async fn log_summary(&self, filter: &LogFilter) -> Result<LogSummary> {
        let (where_, args) = log_filter(filter);
        self.read(move |c| {
            let mut stmt = c.prepare(&format!(
                "SELECT status, COUNT(*) FROM usage_log{where_} GROUP BY status"
            ))?;
            let mut out = LogSummary::default();
            let rows = stmt.query_map(rusqlite::params_from_iter(args.iter()), |r| {
                Ok((value::text_or(r, 0, "")?, value::i64_or(r, 1, 0)?))
            })?;
            for row in rows {
                let (status, n) = row?;
                out.add(&status, n);
            }
            Ok(out)
        })
        .await
    }

    /// 每个时间段 `[from, to)` 里全站成功出图的条数（与 `ranges` 一一对应）。
    /// 闲置回收用它判断「站点那天有没有在正常出图」。
    pub async fn ok_image_counts(&self, ranges: Vec<(f64, f64)>) -> Result<Vec<i64>> {
        self.read(move |c| {
            let mut stmt = c.prepare(
                "SELECT COUNT(*) FROM usage_log \
                 WHERE ts>=? AND ts<? AND status='ok' AND kind IN ('image','image_stream')",
            )?;
            ranges
                .iter()
                .map(|(from, to)| stmt.query_row(params![from, to], |r| value::i64_or(r, 0, 0)))
                .collect()
        })
        .await
    }

    /// 只清理明细日志；每日计数器和账本不受影响。返回删除条数。
    pub async fn purge_usage_log(&self, older_than: f64) -> Result<i64> {
        self.write(move |tx| {
            Ok(tx.execute("DELETE FROM usage_log WHERE ts<?", [older_than])? as i64)
        })
        .await
    }

    /// 每把 Key：首次 / 最近一次成功出图时间，以及 `since` 之后被拒次数和前 3 个被拒原因（用于成员筛选）。
    /// 基于用量日志，超过日志保留期的早期记录不计入。`reason_label(code, detail)` 是 `app/reasons.py::reason_label`。
    pub async fn member_milestones(
        &self,
        since: f64,
        reason_label: &(dyn Fn(&str, &str) -> String + Sync),
    ) -> Result<PyMap<i64, Milestone>> {
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

        let mut out: PyMap<i64, Milestone> = Vec::new();
        fn slot(out: &mut PyMap<i64, Milestone>, key: i64) -> &mut Milestone {
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
        let mut by_key: PyMap<i64, Vec<(String, i64)>> = Vec::new();
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
    pub async fn generated_image_totals(&self, key_id: Option<i64>) -> Result<PyMap<i64, i64>> {
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
            // Python `if x`：None 与 0.0 都是假
            let truthy = |v: Option<f64>| v.filter(|x| *x != 0.0);
            let site_avg = truthy(site).unwrap_or(9000.0);
            Ok(ImageTiming {
                count: cnt,
                avg_dur_s: truthy(avg_dur).map(|x| pyc::round(x / 1000.0, 1)),
                avg_wait_s: truthy(avg_wait).map(|x| pyc::round(x / 1000.0, 1)),
                site_avg_dur_s: pyc::round(site_avg / 1000.0, 1),
            })
        })
        .await
    }
}
