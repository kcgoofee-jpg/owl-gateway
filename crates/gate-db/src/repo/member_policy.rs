//! 成员规则存取。一次读取同时取得默认值与例外，避免跨版本拼接。

use crate::{Db, DbError, Result};
use gate_core::member_policy::{ClaimRules, MemberOverrides, MemberPolicy, SharedBudgets};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const DEFAULTS_KEY: &str = "member_policy_defaults";
pub const BUDGETS_KEY: &str = "runtime_budgets";
pub const CLAIM_KEY: &str = "claim_rules";
const OWNER_IMAGE_KEY: &str = "owner_image_profile";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct OwnerImageProfile {
    subject: String,
    enabled: bool,
    saved: Option<MemberPolicy>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct OwnerImageState {
    pub enabled: bool,
}

fn owner_profile(conn: &Connection) -> Result<Option<OwnerImageProfile>> {
    let raw: Option<String> = conn
        .query_row(
            "SELECT value FROM site_settings WHERE key=?",
            [OWNER_IMAGE_KEY],
            |r| r.get(0),
        )
        .optional()?;
    let profile: Option<OwnerImageProfile> = raw
        .map(|s| serde_json::from_str(&s).map_err(invalid))
        .transpose()?;
    if let Some(p) = &profile {
        valid_subject(&p.subject)?;
        if let Some(saved) = &p.saved {
            saved.validate().map_err(invalid)?;
        }
        if p.enabled && p.saved.is_none() {
            return Err(invalid("站长生图配置缺失"));
        }
    }
    Ok(profile)
}

fn store_owner(conn: &Connection, profile: &OwnerImageProfile) -> rusqlite::Result<()> {
    let encoded = serde_json::to_string(profile)
        .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
    conn.execute("INSERT INTO site_settings(key,value) VALUES(?,?) ON CONFLICT(key) DO UPDATE SET value=excluded.value", params![OWNER_IMAGE_KEY, encoded])?;
    Ok(())
}

fn apply_owner(
    mut snapshot: PolicySnapshot,
    subject: Option<&str>,
    owner: Option<&OwnerImageProfile>,
) -> PolicySnapshot {
    if let Some(owner) = owner.filter(|p| Some(p.subject.as_str()) == subject) {
        snapshot.owner_image = Some(OwnerImageState {
            enabled: owner.enabled,
        });
        snapshot.effective = if owner.enabled {
            let mut p = owner.saved.as_ref().expect("validated profile").clone();
            p.image = true;
            p.v5 = true;
            p.vibe = true;
            p.tags = true;
            p.upscale = true;
            p.augment = true;
            p.img2img = true;
            p.paid_parameters = true;
            p
        } else {
            snapshot.defaults.clone()
        };
        snapshot.overrides = snapshot.defaults.differences(&snapshot.effective);
    }
    snapshot
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PolicySnapshot {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner_image: Option<OwnerImageState>,
    pub defaults: MemberPolicy,
    pub overrides: MemberOverrides,
    pub effective: MemberPolicy,
}

pub struct PolicyDirectory {
    owner: Option<OwnerImageProfile>,
    defaults: MemberPolicy,
    overrides: BTreeMap<String, MemberOverrides>,
}

impl PolicyDirectory {
    pub fn for_subject(&self, subject: Option<&str>) -> Result<PolicySnapshot> {
        let overrides = match subject.filter(|s| !s.is_empty()) {
            Some(subject) => {
                valid_subject(subject)?;
                self.overrides.get(subject).cloned().unwrap_or_default()
            }
            None => MemberOverrides::default(),
        };
        Ok(apply_owner(
            PolicySnapshot {
                owner_image: None,
                effective: self.defaults.resolve(&overrides).map_err(invalid)?,
                defaults: self.defaults.clone(),
                overrides,
            },
            subject,
            self.owner.as_ref(),
        ))
    }
}

fn invalid(error: impl std::fmt::Display) -> DbError {
    DbError::Invalid(error.to_string())
}

fn defaults(conn: &Connection) -> Result<MemberPolicy> {
    let raw: String = conn.query_row(
        "SELECT value FROM site_settings WHERE key=?",
        [DEFAULTS_KEY],
        |r| r.get(0),
    )?;
    let policy: MemberPolicy = serde_json::from_str(&raw).map_err(invalid)?;
    policy.validate().map_err(invalid)?;
    Ok(policy)
}

fn valid_subject(subject: &str) -> Result<()> {
    if !gate_core::member_policy::is_discord_id(subject) {
        return Err(invalid("无效 Discord ID"));
    }
    Ok(())
}

fn snapshot(conn: &Connection, subject: &str) -> Result<PolicySnapshot> {
    let defaults = defaults(conn)?;
    let raw: Option<String> = conn
        .query_row(
            "SELECT overrides FROM member_rules WHERE discord_id=?",
            [subject],
            |r| r.get(0),
        )
        .optional()?;
    let overrides = raw
        .map(|s| serde_json::from_str(&s).map_err(invalid))
        .transpose()?
        .unwrap_or_default();
    let effective = defaults.resolve(&overrides).map_err(invalid)?;
    Ok(apply_owner(
        PolicySnapshot {
            owner_image: None,
            defaults,
            overrides,
            effective,
        },
        Some(subject),
        owner_profile(conn)?.as_ref(),
    ))
}

impl Db {
    /// Server configuration binds one identity; the HTTP API cannot bind or reassign it.
    pub async fn bind_image_owner(&self, subject: &str) -> Result<()> {
        valid_subject(subject)?;
        let subject = subject.to_owned();
        self.write(move |tx| {
            let old = owner_profile(tx)
                .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
            if let Some(old) = old {
                if old.subject != subject {
                    return Err(rusqlite::Error::InvalidParameterName(
                        "站长生图身份与持久绑定不一致".into(),
                    ));
                }
            } else {
                store_owner(
                    tx,
                    &OwnerImageProfile {
                        subject: subject.clone(),
                        enabled: false,
                        saved: None,
                    },
                )?;
            }
            Ok(())
        })
        .await
    }

    pub async fn set_owner_image_enabled(&self, subject: &str, enabled: bool) -> Result<bool> {
        valid_subject(subject)?;
        let subject = subject.to_owned();
        self.write(move |tx| {
            let mut owner = owner_profile(tx)
                .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
            let Some(ref mut owner) = owner.as_mut().filter(|p| p.subject == subject) else {
                return Ok(false);
            };
            if enabled && owner.saved.is_none() {
                // Capture personal limits before applying the full-feature overlay.
                let policy = defaults(tx)
                    .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
                let raw: Option<String> = tx
                    .query_row(
                        "SELECT overrides FROM member_rules WHERE discord_id=?",
                        [&subject],
                        |r| r.get(0),
                    )
                    .optional()?;
                let overrides: MemberOverrides = raw
                    .map(|s| serde_json::from_str(&s))
                    .transpose()
                    .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?
                    .unwrap_or_default();
                owner.saved = Some(
                    policy
                        .resolve(&overrides)
                        .map_err(|e| rusqlite::Error::InvalidParameterName(e.into()))?,
                );
            }
            owner.enabled = enabled;
            store_owner(tx, owner)?;
            Ok(true)
        })
        .await
    }

    pub async fn claim_rules(&self) -> Result<ClaimRules> {
        let raw = self
            .get_setting(CLAIM_KEY)
            .await?
            .ok_or_else(|| invalid("缺少领取条件"))?;
        let rules: ClaimRules = serde_json::from_str(&raw).map_err(invalid)?;
        rules.validate().map_err(invalid)?;
        Ok(rules)
    }

    pub async fn set_claim_rules(&self, rules: &ClaimRules) -> Result<()> {
        rules.validate().map_err(invalid)?;
        self.set_setting(CLAIM_KEY, &serde_json::to_string(rules).map_err(invalid)?)
            .await
    }
    pub async fn member_policy_directory(&self) -> Result<PolicyDirectory> {
        self.read(|conn| {
            let transaction = conn.unchecked_transaction()?;
            let defaults = defaults(&transaction)
                .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
            let mut statement =
                transaction.prepare("SELECT discord_id,overrides FROM member_rules")?;
            let raw = statement
                .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            drop(statement);
            let mut overrides = BTreeMap::new();
            for (subject, encoded) in raw {
                let parsed: MemberOverrides = serde_json::from_str(&encoded)
                    .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
                defaults
                    .resolve(&parsed)
                    .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(invalid(e))))?;
                overrides.insert(subject, parsed);
            }
            let owner = owner_profile(&transaction)
                .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
            transaction.commit()?;
            Ok(PolicyDirectory {
                owner,
                defaults,
                overrides,
            })
        })
        .await
    }
    pub async fn member_defaults(&self) -> Result<MemberPolicy> {
        let raw = self
            .get_setting(DEFAULTS_KEY)
            .await?
            .ok_or_else(|| invalid("缺少默认成员规则"))?;
        let policy: MemberPolicy = serde_json::from_str(&raw).map_err(invalid)?;
        policy.validate().map_err(invalid)?;
        Ok(policy)
    }

    pub async fn shared_budgets(&self) -> Result<SharedBudgets> {
        let raw = self
            .get_setting(BUDGETS_KEY)
            .await?
            .ok_or_else(|| invalid("缺少全站预算"))?;
        let budgets: SharedBudgets = serde_json::from_str(&raw).map_err(invalid)?;
        budgets.validate().map_err(invalid)?;
        Ok(budgets)
    }

    pub async fn set_shared_budgets(&self, budgets: &SharedBudgets) -> Result<()> {
        budgets.validate().map_err(invalid)?;
        self.set_setting(
            BUDGETS_KEY,
            &serde_json::to_string(budgets).map_err(invalid)?,
        )
        .await
    }

    pub async fn member_policy(&self, subject: &str) -> Result<PolicySnapshot> {
        valid_subject(subject)?;
        let subject = subject.to_owned();
        // The ordinary read helper supplies a Connection; a deferred transaction pins both reads.
        self.read(move |conn| {
            let transaction = conn.unchecked_transaction()?;
            let result = snapshot(&transaction, &subject);
            match result {
                Ok(value) => {
                    transaction.commit()?;
                    Ok(value)
                }
                Err(error) => Err(rusqlite::Error::ToSqlConversionFailure(Box::new(error))),
            }
        })
        .await
    }

    pub async fn set_member_defaults(&self, policy: &MemberPolicy) -> Result<()> {
        policy.validate().map_err(invalid)?;
        let policy = policy.clone();
        self.write(move |transaction| {
            let mut statement=transaction.prepare("SELECT discord_id,overrides FROM member_rules")?;
            let saved=statement.query_map([],|row|Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            drop(statement);
            for (subject,encoded) in saved {
                let old:MemberOverrides=serde_json::from_str(&encoded)
                    .map_err(|error|rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
                let effective=policy.resolve(&old).map_err(|error|
                    rusqlite::Error::ToSqlConversionFailure(Box::new(invalid(error))))?;
                let normalized=policy.differences(&effective);
                if normalized!=old {
                    let encoded=serde_json::to_string(&normalized).map_err(|error|
                        rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
                    transaction.execute("UPDATE member_rules SET overrides=? WHERE discord_id=?",params![encoded,subject])?;
                }
            }
            let encoded=serde_json::to_string(&policy).map_err(|error|
                rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
            transaction.execute("INSERT INTO site_settings(key,value) VALUES(?,?) ON CONFLICT(key) DO UPDATE SET value=excluded.value",params![DEFAULTS_KEY,encoded])?;
            Ok(())
        }).await
    }

    pub async fn set_member_overrides(
        &self,
        subject: &str,
        overrides: &MemberOverrides,
    ) -> Result<()> {
        valid_subject(subject)?;
        let subject = subject.to_owned();
        let overrides = overrides.clone();
        self.write(move |transaction| {
            let policy = defaults(transaction)
                .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
            let effective = policy
                .resolve(&overrides)
                .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(invalid(e))))?;
            if let Some(mut owner) = owner_profile(transaction)
                .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?
                .filter(|p| p.subject == subject)
            {
                if !owner.enabled {
                    return Err(rusqlite::Error::InvalidParameterName(
                        "普通测试状态下请修改默认规则，或先启用全部生图功能".into(),
                    ));
                }
                owner.saved = Some(effective);
                store_owner(transaction, &owner)?;
                return Ok(());
            }
            let normalized = policy.differences(&effective);
            let encoded = serde_json::to_string(&normalized)
                .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
            transaction.execute(
                "INSERT INTO member_rules(discord_id,overrides) VALUES(?,?) \
                 ON CONFLICT(discord_id) DO UPDATE SET overrides=excluded.overrides",
                params![subject, encoded],
            )?;
            Ok(())
        })
        .await
    }
}
