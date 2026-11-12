//! 成员规则存取。一次读取同时取得默认值与例外，避免跨版本拼接。

use super::conversion_error;
use crate::repo::patron::{PatronView, patron_view, patron_views};
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
    let encoded = serde_json::to_string(profile).map_err(conversion_error)?;
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub patron: Option<PatronView>,
    pub defaults: MemberPolicy,
    pub overrides: MemberOverrides,
    pub effective: MemberPolicy,
}

pub struct PolicyDirectory {
    owner: Option<OwnerImageProfile>,
    defaults: MemberPolicy,
    overrides: BTreeMap<String, MemberOverrides>,
    patrons: BTreeMap<String, PatronView>,
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
        let mut snapshot = apply_owner(
            PolicySnapshot {
                owner_image: None,
                patron: None,
                effective: self.defaults.resolve(&overrides).map_err(invalid)?,
                defaults: self.defaults.clone(),
                overrides,
            },
            subject,
            self.owner.as_ref(),
        );
        if let Some(subject) = subject.filter(|s| !s.is_empty()) {
            snapshot.patron = self.patrons.get(subject).cloned();
        }
        Ok(snapshot)
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
    let mut snapshot = apply_owner(
        PolicySnapshot {
            owner_image: None,
            patron: None,
            defaults,
            overrides,
            effective,
        },
        Some(subject),
        owner_profile(conn)?.as_ref(),
    );
    snapshot.patron = patron_view(conn, subject)?;
    Ok(snapshot)
}

impl Db {
    /// Server configuration binds one identity; the HTTP API cannot bind or reassign it.
    pub async fn bind_image_owner(&self, subject: &str) -> Result<()> {
        valid_subject(subject)?;
        let subject = subject.to_owned();
        self.write(move |tx| {
            let old = owner_profile(tx).map_err(conversion_error)?;
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
            let mut owner = owner_profile(tx).map_err(conversion_error)?;
            let Some(ref mut owner) = owner.as_mut().filter(|p| p.subject == subject) else {
                return Ok(false);
            };
            if enabled && owner.saved.is_none() {
                // Capture personal limits before applying the full-feature overlay.
                let policy = defaults(tx).map_err(conversion_error)?;
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
                    .map_err(conversion_error)?
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
            let defaults = defaults(&transaction).map_err(conversion_error)?;
            let mut statement =
                transaction.prepare("SELECT discord_id,overrides FROM member_rules")?;
            let raw = statement
                .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            drop(statement);
            let mut overrides = BTreeMap::new();
            for (subject, encoded) in raw {
                let parsed: MemberOverrides =
                    serde_json::from_str(&encoded).map_err(conversion_error)?;
                defaults
                    .resolve(&parsed)
                    .map_err(|e| conversion_error(invalid(e)))?;
                overrides.insert(subject, parsed);
            }
            let owner = owner_profile(&transaction).map_err(conversion_error)?;
            let patrons = patron_views(&transaction).map_err(conversion_error)?;
            transaction.commit()?;
            Ok(PolicyDirectory {
                owner,
                defaults,
                overrides,
                patrons,
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
            let value = snapshot(&transaction, &subject).map_err(conversion_error)?;
            transaction.commit()?;
            Ok(value)
        })
        .await
    }

    pub async fn set_member_defaults(&self, policy: &MemberPolicy) -> Result<()> {
        policy.validate().map_err(invalid)?;
        let policy = policy.clone();
        self.write(move |transaction| {
            let saved = {
                let mut statement = transaction.prepare("SELECT discord_id, overrides FROM member_rules")?;
                statement.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))?
                    .collect::<rusqlite::Result<Vec<_>>>()?
            };
            for (subject, encoded) in saved {
                let old: MemberOverrides = serde_json::from_str(&encoded).map_err(conversion_error)?;
                let effective = policy.resolve(&old).map_err(|e| conversion_error(invalid(e)))?;
                let normalized = policy.differences(&effective);
                if normalized != old {
                    transaction.execute(
                        "UPDATE member_rules SET overrides=? WHERE discord_id=?",
                        params![serde_json::to_string(&normalized).map_err(conversion_error)?, subject],
                    )?;
                }
            }
            transaction.execute(
                "INSERT INTO site_settings(key,value) VALUES(?,?) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
                params![DEFAULTS_KEY, serde_json::to_string(&policy).map_err(conversion_error)?],
            )?;
            Ok(())
        }).await
    }

    /// Only commit a portal edit while both its authenticated seat and the rules it edited still match.
    pub async fn set_patron_overrides(
        &self,
        seat: super::patron::PatronRecord,
        prior: PolicySnapshot,
        overrides: MemberOverrides,
    ) -> Result<bool> {
        self.write(move |tx| {
            let current =
                super::patron::patron_record(tx, &seat.subject).map_err(conversion_error)?;
            if !seat.enabled || !seat.self_service || current.as_ref() != Some(&seat) {
                return Ok(false);
            }
            let policy = snapshot(tx, &seat.subject).map_err(conversion_error)?;
            if policy != prior {
                return Ok(false);
            }
            store_overrides(tx, &seat.subject, &overrides)?;
            Ok(true)
        })
        .await
    }

    pub async fn set_member_overrides(
        &self,
        subject: &str,
        overrides: &MemberOverrides,
    ) -> Result<()> {
        valid_subject(subject)?;
        let subject = subject.to_owned();
        let overrides = overrides.clone();
        self.write(move |transaction| store_overrides(transaction, &subject, &overrides))
            .await
    }
}

fn store_overrides(
    transaction: &Connection,
    subject: &str,
    overrides: &MemberOverrides,
) -> rusqlite::Result<()> {
    let policy = defaults(transaction).map_err(conversion_error)?;
    let effective = policy
        .resolve(overrides)
        .map_err(|e| conversion_error(invalid(e)))?;
    if let Some(mut owner) = owner_profile(transaction)
        .map_err(conversion_error)?
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
    let encoded = serde_json::to_string(&normalized).map_err(conversion_error)?;
    transaction.execute(
        "INSERT INTO member_rules(discord_id,overrides) VALUES(?,?) \
                 ON CONFLICT(discord_id) DO UPDATE SET overrides=excluded.overrides",
        params![subject, encoded],
    )?;
    Ok(())
}
