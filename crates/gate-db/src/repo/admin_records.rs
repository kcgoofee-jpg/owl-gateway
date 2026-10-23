//! 后台「日志与记录」接口用到的裸 SQL（旧版 `app/admin.py` / `feedback.py` 里直接用 `db._db` 写的那些）：
//! 操作日志筛选、Bug 页的 Key 名、成员反馈，以及按「文本形式的 Key 编号」查生成记录。

use super::{SqlValue, marks};
use crate::{Db, Result, value};
use serde_json::Value;

/// 操作日志筛选（旧版 `/admin/api/actions` 的 actor / action 参数，已规整）。
#[derive(Debug, Clone, Default)]
pub struct ActionFilter {
    /// "manual"（站长 + Claude）/ "auto"（算法与定时任务）/ "member"，以及旧的 "admin" / "system"；其它值不筛。
    pub actor_kind: String,
    /// 操作名（一组可含多个旧名），空 = 不筛。
    pub actions: Vec<String>,
}

fn action_clause(f: &ActionFilter) -> (String, Vec<SqlValue>) {
    let mut where_: Vec<String> = Vec::new();
    let mut args: Vec<SqlValue> = Vec::new();
    match f.actor_kind.as_str() {
        "admin" => where_.push("(actor LIKE '后台%' OR actor = '站长')".into()),
        "system" => where_.push("actor = '系统'".into()),
        k => {
            if let Some(k) = gate_core::actor::Kind::parse(k) {
                where_.push(gate_core::actor::sql_where(k));
            }
        }
    }
    if !f.actions.is_empty() {
        where_.push(format!("action IN ({})", marks(f.actions.len())));
        args.extend(f.actions.iter().cloned().map(SqlValue::Text));
    }
    let clause = if where_.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", where_.join(" AND "))
    };
    (clause, args)
}

impl Db {
    pub async fn count_actions_filtered(&self, f: &ActionFilter) -> Result<i64> {
        let (clause, args) = action_clause(f);
        self.read(move |c| {
            c.query_row(
                &format!("SELECT COUNT(*) FROM admin_actions{clause}"),
                rusqlite::params_from_iter(args.iter()),
                |r| value::i64_or(r, 0, 0),
            )
        })
        .await
    }

    /// 最新的在前；`SELECT *` 的整行（id, ts, actor, action, target, detail, ok）。
    pub async fn list_actions_filtered(
        &self,
        f: &ActionFilter,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<Value>> {
        let (clause, mut args) = action_clause(f);
        args.push(SqlValue::Integer(limit));
        args.push(SqlValue::Integer(offset));
        self.read(move |c| {
            let mut stmt = c.prepare(&format!(
                "SELECT * FROM admin_actions{clause} ORDER BY id DESC LIMIT ? OFFSET ?"
            ))?;
            stmt.query_map(rusqlite::params_from_iter(args.iter()), value::row_json)?
                .collect()
        })
        .await
    }

    /// 出现过的全部操作名（按字典序），给筛选下拉用。
    pub async fn action_kinds(&self) -> Result<Vec<String>> {
        self.read(|c| {
            let mut stmt =
                c.prepare("SELECT DISTINCT action FROM admin_actions ORDER BY action")?;
            stmt.query_map([], |r| value::text_or(r, 0, "None"))?
                .collect()
        })
        .await
    }

    /// `SELECT name FROM api_keys WHERE id=?`。
    pub async fn key_name(&self, key_id: i64) -> Result<Option<String>> {
        self.read(move |c| {
            let mut stmt = c.prepare("SELECT name FROM api_keys WHERE id=?")?;
            let mut rows = stmt.query([key_id])?;
            Ok(match rows.next()? {
                Some(r) => Some(value::text_or(r, 0, "None")?),
                None => None,
            })
        })
        .await
    }
}
