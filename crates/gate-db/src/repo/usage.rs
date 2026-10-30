//! 用量日志：成功记账、拒绝 / 失败日志、各类统计查询。

use super::{INSERT_LOG, LOG_COLUMNS, SqlValue, marks};
use crate::{Db, Result, value};
use gate_core::primitives;
use rusqlite::params;
use serde::Serialize;
use serde_json::Value;

/// 一次成功请求的记账内容（`record_success`）。字段缺省值见各字段。
#[derive(Debug, Clone, Default, Serialize, serde::Deserialize)]
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
    pub remaining_anlas: f64,
    pub remaining_v5: i64,
    pub remaining_legacy: i64,
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

#[derive(Debug, Clone)]
pub struct RequestSettlement {
    pub request_id: String,
    pub rid: String,
    pub key_id: i64,
    pub status: String,
    pub day: String,
    pub month: String,
    pub is_vibe: bool,
    pub anlas: f64,
    pub v5: i64,
    pub legacy: i64,
    pub result_json: Option<String>,
    pub correlation_id: String,
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
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, serde::Deserialize)]
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
            let t = primitives::take_chars(s.trim(), n);
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
    /// 文生图模型版本（v45 / v5）；空串 = 不筛。
    pub model_version: String,
    /// 同一成员的多把 Key 一起查；空 = 不筛（与 `key_id` 同时给时两条都要满足）。
    pub key_ids: Vec<i64>,
    /// ok / rejected / error；空串 = 不筛。
    pub status: String,
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
pub(super) fn is_image_kind(kind: &str) -> bool {
    matches!(kind, "image" | "image_stream")
}

pub(super) fn insert_usage_log(
    tx: &rusqlite::Transaction<'_>,
    l: &NewLog,
    now: f64,
) -> rusqlite::Result<()> {
    let h = l.hints.columns();
    tx.execute(
        INSERT_LOG,
        params![
            now,
            l.key_id,
            primitives::take_chars(&l.key_name, 80),
            l.kind,
            primitives::take_chars(&l.model, 80),
            l.status,
            l.images,
            l.anlas,
            l.tokens,
            primitives::take_chars(&l.detail, 500),
            l.unconfirmed_anlas,
            l.wait_ms.max(0),
            l.dur_ms.max(0),
            l.up_status,
            primitives::take_chars(&l.rid, 16),
            primitives::take_chars(&l.reason, 24),
            l.token_id,
            primitives::take_chars(&l.client, 160),
            primitives::take_chars(&l.ip, 64),
            h[0],
            h[1],
            h[2],
            h[3],
            h[4],
            h[5]
        ],
    )?;
    Ok(())
}

/// 组装日志筛选的 WHERE 子句：返回 (` WHERE ...` 或空串, 绑定参数)。
fn log_filter(f: &LogFilter) -> (String, Vec<SqlValue>) {
    let mut clauses: Vec<String> = Vec::new();
    let mut args: Vec<SqlValue> = Vec::new();
    if !f.rid.is_empty() {
        clauses.push("rid=?".into());
        args.push(SqlValue::Text(f.rid.clone()));
    }
    match f.model_version.as_str() {
        "v45" => clauses.push("lower(model) LIKE '%diffusion-4-5%'".into()),
        "v5" => clauses.push("lower(model) LIKE '%diffusion-5%'".into()),
        _ => {}
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
                "SELECT {LOG_COLUMNS}, COALESCE((SELECT s.correlation_id FROM usage_request_links u JOIN request_settlements s ON s.request_id=u.request_id WHERE u.usage_log_id=usage_log.id),'') AS correlation_id FROM usage_log{where_} ORDER BY id DESC LIMIT ? OFFSET ?"
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
                "SELECT l.id, l.rid, l.ts, l.kind, l.model, l.status, l.images, l.anlas, l.wait_ms, l.dur_ms, l.reason, l.detail, l.client, \
                 COALESCE(s.delivery_state,'unknown') AS delivery_state, s.status AS settlement_status, \
                 COALESCE(s.anlas,0) AS remaining_anlas, COALESCE(s.v5,0) AS remaining_v5, COALESCE(s.legacy,0) AS remaining_legacy, u.request_id, COALESCE(s.correlation_id,'') AS correlation_id \
                 FROM usage_log l LEFT JOIN usage_request_links u ON u.usage_log_id=l.id LEFT JOIN request_settlements s ON s.request_id=u.request_id \
                 WHERE l.key_id=?1 AND l.ts>=?2 AND (?3=0 OR l.id<?3) \
                 AND (?4='' OR l.status=?4) ORDER BY l.id DESC LIMIT ?5",
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
}
