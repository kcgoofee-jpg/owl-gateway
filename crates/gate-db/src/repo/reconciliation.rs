//! Anlas 对账快照。

use crate::{Db, DbError, Result, value};
use gate_core::json;
use rusqlite::params;
use serde_json::Value;

impl Db {
    /// 累计对账总数：含已删除的 Key，不随每日额度重置。
    /// 返回 dict 的原样行：SUM 在空表上是整数 0、有数据时是浮点——保持 SQLite 给的类型。
    /// `site_paid`：账号自己付的 V5 Anlas（站付，不进成员计数），也是官方余额减少的一部分。
    pub async fn reconciliation_totals(&self) -> Result<Value> {
        let marker = format!("%{}%", gate_core::paid_v5::MARKER);
        self.read(move |c| {
            c.query_row(
                "SELECT (SELECT COALESCE(SUM(anlas), 0) FROM counters) AS anlas, \
                        COALESCE(SUM(unconfirmed_anlas), 0) AS unconfirmed_anlas, \
                        COALESCE(SUM(unconfirmed_anlas > 0), 0) AS unconfirmed_requests, \
                        COALESCE(MAX(id), 0) AS last_log_id, \
                        COALESCE(SUM(CASE WHEN status='ok' AND detail LIKE ?1 THEN anlas END), 0) AS site_paid \
                 FROM usage_log",
                [marker],
                value::row_json,
            )
        })
        .await
    }

    /// 某一刻之前累计的站付 V5 Anlas：给还没记 `site_paid` 的旧快照补上起点。
    pub async fn site_paid_before(&self, ts: f64) -> Result<f64> {
        let marker = format!("%{}%", gate_core::paid_v5::MARKER);
        self.read(move |c| {
            c.query_row(
                "SELECT COALESCE(SUM(anlas), 0) FROM usage_log \
                 WHERE status='ok' AND detail LIKE ?1 AND ts <= ?2",
                params![marker, ts],
                |r| r.get(0),
            )
        })
        .await
    }

    /// 存一份对账快照。序列化与 `json.dumps(…, ensure_ascii=False, allow_nan=False)` 等价：
    /// 默认分隔符（", " / ": "）、保留 UTF-8；`Value` 里不可能有 NaN / Infinity。
    pub async fn save_reconciliation(&self, snapshot: &Value) -> Result<()> {
        let text = json::dumps(snapshot);
        self.write(move |tx| {
            tx.execute(
                "INSERT INTO anlas_reconciliations(snapshot) VALUES (?)",
                params![text],
            )?;
            Ok(())
        })
        .await
    }

    /// 最近的对账快照（新→旧）；每条是快照内容再追加 / 覆盖 `id`（`{**snapshot, "id": id}`）。
    pub async fn reconciliation_history(&self, limit: i64) -> Result<Vec<Value>> {
        let limit = limit.clamp(1, 20);
        let rows: Vec<(i64, String)> = self
            .read(move |c| {
                let mut stmt = c.prepare(
                    "SELECT id, snapshot FROM anlas_reconciliations ORDER BY id DESC LIMIT ?",
                )?;
                stmt.query_map([limit], |r| {
                    Ok((
                        value::i64_or(r, "id", 0)?,
                        value::text_or(r, "snapshot", "")?,
                    ))
                })?
                .collect()
            })
            .await?;
        let mut out = Vec::with_capacity(rows.len());
        for (id, snapshot) in rows {
            let parsed: Value = serde_json::from_str(&snapshot)
                .map_err(|e| DbError::Invalid(format!("对账快照不是合法 JSON：{e}")))?;
            let Value::Object(mut map) = parsed else {
                return Err(DbError::Invalid("对账快照不是 JSON 对象".into()));
            };
            map.insert("id".into(), Value::from(id));
            out.push(Value::Object(map));
        }
        Ok(out)
    }
}
