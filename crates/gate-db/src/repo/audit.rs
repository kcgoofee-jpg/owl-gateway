//! Prompt metadata storage. Images are never persisted.
use super::{SqlValue, marks};
use crate::{Db, Result, value};
use gate_core::clock::Clock;
use rusqlite::params;
use serde_json::Value;
#[derive(Debug, Clone, Copy, Default)]
pub struct NewAudit<'a> {
    pub key_id: Option<i64>,
    pub key_name: &'a str,
    pub kind: &'a str,
    pub model: &'a str,
    pub status: &'a str,
    pub prompt: &'a str,
    pub negative: &'a str,
    pub extra: &'a str,
    pub rid: &'a str,
    pub request_id: &'a str,
}
impl Db {
    pub async fn add_audit(&self, a: &NewAudit<'_>, now: f64, _clock: &Clock) -> Result<()> {
        let key_id = a.key_id;
        let fields = [
            a.key_name,
            a.kind,
            a.model,
            a.status,
            a.prompt,
            a.negative,
            a.extra,
            a.rid,
            a.request_id,
        ]
        .map(str::to_owned);
        self.write(move |tx| {
            tx.execute("INSERT INTO generation_audit(ts,key_id,key_name,kind,model,status,prompt,negative,extra,rid,request_id) VALUES(?,?,?,?,?,?,?,?,?,?,?)",params![now,key_id,fields[0],fields[1],fields[2],fields[3],fields[4],fields[5],fields[6],fields[7],fields[8]])?;
            Ok(())
        }).await
    }
    /// 生成记录分页，按多把 Key（同一成员重置 / 换 Key 后的历史）一起查；`key_ids` 为空表示不筛。
    pub async fn list_audit_ids(
        &self,
        limit: i64,
        offset: i64,
        key_ids: &[i64],
        since: f64,
    ) -> Result<(Vec<Value>, i64)> {
        let key_ids = key_ids.to_vec();
        self.read(move |c| {
            let where_ = if key_ids.is_empty() {
                "WHERE ts>=?".to_owned()
            } else {
                format!("WHERE ts>=? AND key_id IN ({})", marks(key_ids.len()))
            };
            let mut args = vec![SqlValue::Real(since)];
            args.extend(key_ids.iter().map(|k| SqlValue::Integer(*k)));
            let total: i64 = c.query_row(
                &format!("SELECT COUNT(*) FROM generation_audit {where_}"),
                rusqlite::params_from_iter(args.iter()),
                |r| r.get(0),
            )?;
            let mut stmt = c.prepare(&format!(
                "SELECT id,ts,rid,key_id,key_name,kind,model,status,prompt,negative,extra \
                 FROM generation_audit {where_} ORDER BY id DESC LIMIT ? OFFSET ?"
            ))?;
            args.push(SqlValue::Integer(limit));
            args.push(SqlValue::Integer(offset));
            let rows = stmt
                .query_map(rusqlite::params_from_iter(args.iter()), value::row_json)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok((rows, total))
        })
        .await
    }

    pub async fn purge_audit(&self, older_than: f64) -> Result<i64> {
        self.write(move |tx| {
            Ok(tx.execute("DELETE FROM generation_audit WHERE ts<?", [older_than])? as i64)
        })
        .await
    }
}
