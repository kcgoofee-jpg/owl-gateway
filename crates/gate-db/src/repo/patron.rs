//! 赞助席：指定成员的插队、自助开关和专属上游 Token。存在 `site_settings`，不进 git。

use crate::{Db, DbError, Result};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};

pub const PATRON_KEY: &str = "patron_profiles";

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PatronRecord {
    pub subject: String,
    pub enabled: bool,
    #[serde(default = "default_true")]
    pub queue_jump: bool,
    #[serde(default = "default_true")]
    pub self_service: bool,
    #[serde(default)]
    pub bound_token_id: Option<String>,
    #[serde(default)]
    pub password_hash: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PatronView {
    pub enabled: bool,
    pub queue_jump: bool,
    pub self_service: bool,
    pub has_password: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bound_token_id: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct PatronStore {
    #[serde(default)]
    profiles: Vec<PatronRecord>,
}

impl PatronRecord {
    pub fn view(&self) -> PatronView {
        PatronView {
            enabled: self.enabled,
            queue_jump: self.queue_jump,
            self_service: self.self_service,
            has_password: !self.password_hash.is_empty(),
            bound_token_id: nonempty(self.bound_token_id.as_deref()),
        }
    }
}

fn nonempty(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn invalid(error: impl std::fmt::Display) -> DbError {
    DbError::Invalid(error.to_string())
}

fn valid_subject(subject: &str) -> Result<()> {
    if !gate_core::member_policy::is_discord_id(subject) {
        return Err(invalid("无效 Discord ID"));
    }
    Ok(())
}

fn load_store(conn: &Connection) -> Result<PatronStore> {
    let raw: Option<String> = conn
        .query_row(
            "SELECT value FROM site_settings WHERE key=?",
            [PATRON_KEY],
            |r| r.get(0),
        )
        .optional()?;
    let Some(raw) = raw.filter(|s| !s.is_empty()) else {
        return Ok(PatronStore::default());
    };
    let store: PatronStore = serde_json::from_str(&raw).map_err(invalid)?;
    for profile in &store.profiles {
        valid_subject(&profile.subject)?;
    }
    Ok(store)
}

fn save_store(conn: &Connection, store: &PatronStore) -> rusqlite::Result<()> {
    let encoded = serde_json::to_string(store)
        .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
    conn.execute(
        "INSERT INTO site_settings(key,value) VALUES(?,?) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
        params![PATRON_KEY, encoded],
    )?;
    Ok(())
}

pub(crate) fn patron_view(conn: &Connection, subject: &str) -> Result<Option<PatronView>> {
    Ok(load_store(conn)?
        .profiles
        .into_iter()
        .find(|p| p.subject == subject)
        .map(|p| p.view()))
}

pub(crate) fn patron_views(conn: &Connection) -> Result<BTreeMap<String, PatronView>> {
    Ok(load_store(conn)?
        .profiles
        .into_iter()
        .map(|p| (p.subject.clone(), p.view()))
        .collect())
}

impl Db {
    pub async fn patron(&self, subject: &str) -> Result<Option<PatronRecord>> {
        valid_subject(subject)?;
        let subject = subject.to_owned();
        self.read(move |conn| {
            load_store(conn)
                .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))
                .map(|store| store.profiles.into_iter().find(|p| p.subject == subject))
        })
        .await
    }

    pub async fn patrons(&self) -> Result<Vec<PatronRecord>> {
        self.read(|conn| {
            load_store(conn)
                .map(|store| store.profiles)
                .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))
        })
        .await
    }

    pub async fn exclusive_token_ids(&self) -> Result<HashSet<String>> {
        Ok(self
            .patrons()
            .await?
            .into_iter()
            .filter(|p| p.enabled)
            .filter_map(|p| nonempty(p.bound_token_id.as_deref()))
            .collect())
    }

    /// 写入或更新一行赞助席。专属 Token 同一时刻只能绑给一个启用的席位。
    pub async fn upsert_patron(&self, record: PatronRecord) -> Result<()> {
        valid_subject(&record.subject)?;
        if let Some(id) = record.bound_token_id.as_deref()
            && id.trim().is_empty()
        {
            return Err(invalid("专属 Token 编号无效"));
        }
        self.write(move |tx| {
            let mut store =
                load_store(tx).map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
            if record.enabled
                && let Some(id) = nonempty(record.bound_token_id.as_deref())
            {
                let taken = store.profiles.iter().any(|p| {
                    p.enabled
                        && p.subject != record.subject
                        && nonempty(p.bound_token_id.as_deref()).as_deref() == Some(id.as_str())
                });
                if taken {
                    return Err(rusqlite::Error::InvalidParameterName(
                        "这把上游 Token 已绑定其他赞助席".into(),
                    ));
                }
            }
            if let Some(existing) = store
                .profiles
                .iter_mut()
                .find(|p| p.subject == record.subject)
            {
                *existing = record.clone();
            } else {
                store.profiles.push(record.clone());
            }
            store.profiles.sort_by(|a, b| a.subject.cmp(&b.subject));
            save_store(tx, &store)?;
            Ok(())
        })
        .await
    }

    pub async fn unbind_patron_token(&self, token_id: &str) -> Result<()> {
        let token_id = token_id.to_owned();
        self.write(move |tx| {
            let mut store =
                load_store(tx).map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
            let mut changed = false;
            for profile in &mut store.profiles {
                if profile.bound_token_id.as_deref() == Some(token_id.as_str()) {
                    profile.bound_token_id = None;
                    changed = true;
                }
            }
            if changed {
                save_store(tx, &store)?;
            }
            Ok(())
        })
        .await
    }
}
