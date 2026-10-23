//! 成员体验问卷。

use crate::{Db, Result, value};
use gate_core::pyc;
use rusqlite::params;
use serde_json::Value;

/// `save_survey` 的各项回答（保存时按旧版的长度截断）。
#[derive(Debug, Clone, Default)]
pub struct Survey {
    pub client: String,
    pub understand: String,
    pub improve: String,
    pub improve_other: String,
}

impl Db {
    /// Whether this key has submitted the member experience survey.
    pub async fn has_survey(&self, key_id: i64) -> Result<bool> {
        self.read(move |c| {
            c.query_row(
                "SELECT EXISTS(SELECT 1 FROM member_survey WHERE key_id=?)",
                [key_id],
                |r| r.get(0),
            )
        })
        .await
    }

    /// 成员问卷：一人一份，重复提交覆盖（取最新想法）。
    pub async fn save_survey(&self, key_id: i64, s: &Survey, now: f64) -> Result<()> {
        let client = pyc::take_chars(&s.client, 60).to_string();
        let understand = pyc::take_chars(&s.understand, 20).to_string();
        let improve = pyc::take_chars(&s.improve, 200).to_string();
        let improve_other = pyc::take_chars(&s.improve_other, 300).to_string();
        self.write(move |tx| {
            tx.execute(
                "INSERT INTO member_survey(key_id, client, understand, improve, improve_other, ts) \
                 VALUES (?,?,?,?,?,?) ON CONFLICT(key_id) DO UPDATE SET client=excluded.client, \
                 understand=excluded.understand, improve=excluded.improve, improve_other=excluded.improve_other, \
                 ts=excluded.ts",
                params![key_id, client, understand, improve, improve_other, now],
            )?;
            Ok(())
        })
        .await
    }

    pub async fn list_survey(&self) -> Result<Vec<Value>> {
        self.read(|c| {
            let mut stmt = c.prepare(
                "SELECT f.key_id, f.ts, f.client, f.improve, f.improve_other, k.name, r.discord_id FROM member_survey f \
                 LEFT JOIN api_keys k ON k.id=f.key_id \
                 LEFT JOIN discord_registrations r ON r.key_id=f.key_id ORDER BY f.ts DESC",
            )?;
            stmt.query_map([], value::row_json)?.collect()
        })
        .await
    }
}
