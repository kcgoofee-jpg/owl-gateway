//! 旧权限的只读迁移预检。预检与实际写入使用同一份转换结果。

use gate_core::member_policy::{
    ClaimRules, EligibilityRule, MemberOverrides, MemberPolicy, SharedBudgets,
};
use gate_core::quota_limits::{GlobalLimits, Limit, MemberLimits};
use rusqlite::{Connection, OptionalExtension};
use serde::Serialize;
use std::collections::{BTreeMap, HashSet};
use std::path::Path;

pub const LEGACY_KEY_COLUMNS: &[&str] = &[
    "daily_images",
    "daily_anlas",
    "daily_v5",
    "monthly_anlas",
    "daily_text_tokens",
    "rpm",
    "allow_anlas",
    "allow_img2img",
    "exclude_global_v5",
    "image_model_scope",
    "is_admin",
    "is_test",
    "anlas_auto",
    "quota_auto",
    "v5_pinned",
    "features",
    "quota_limits",
];
const LEGACY_SETTINGS: &[&str] = &[
    "quota_modes_v1",
    "quota_modes_v2_migrated",
    "quota_9999_migrated",
    "register_daily_images",
    "register_daily_v5",
    "member_daily_anlas",
    "global_daily_v5",
    "global_daily_anlas",
    "global_monthly_anlas",
    "register_features",
    "register_image_scope",
    "register_open",
    "register_max_users",
    "register_expires_days",
    "register_role_guild",
    "register_role_id",
    "register_role_note",
    "register_role_extra",
    "v5_access_mode",
    "v5_authorized_discord_ids",
    "gateway_owner_discord_ids",
    "gateway_admin_discord_ids",
    "public_vibe_enabled",
    "algo_notice",
];

fn column_exists(conn: &Connection, table: &str, column: &str) -> rusqlite::Result<bool> {
    let mut statement = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    for value in statement.query_map([], |row| row.get::<_, String>(1))? {
        if value? == column {
            return Ok(true);
        }
    }
    Ok(false)
}

fn legacy_settings_present(conn: &Connection) -> rusqlite::Result<bool> {
    let query = format!(
        "SELECT EXISTS(SELECT 1 FROM site_settings WHERE key IN ({}))",
        crate::repo::marks(LEGACY_SETTINGS.len())
    );
    conn.query_row(
        &query,
        rusqlite::params_from_iter(LEGACY_SETTINGS.iter()),
        |row| row.get(0),
    )
}

fn cleanup_needed(conn: &Connection) -> rusqlite::Result<bool> {
    for name in LEGACY_KEY_COLUMNS {
        if column_exists(conn, "api_keys", name)? {
            return Ok(true);
        }
    }
    let old_table:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='deleted_key_usage_flags')",[],|row|row.get(0))?;
    Ok(old_table
        || column_exists(conn, "request_settlements", "exclude_global_v5")?
        || legacy_settings_present(conn)?)
}

fn cleanup(conn: &Connection) -> rusqlite::Result<()> {
    for name in LEGACY_KEY_COLUMNS {
        if column_exists(conn, "api_keys", name)? {
            conn.execute(&format!("ALTER TABLE api_keys DROP COLUMN {name}"), [])?;
        }
    }
    if column_exists(conn, "request_settlements", "exclude_global_v5")? {
        conn.execute(
            "ALTER TABLE request_settlements DROP COLUMN exclude_global_v5",
            [],
        )?;
    }
    conn.execute_batch("DROP TABLE IF EXISTS deleted_key_usage_flags;")?;
    let query = format!(
        "DELETE FROM site_settings WHERE key IN ({})",
        crate::repo::marks(LEGACY_SETTINGS.len())
    );
    conn.execute(&query, rusqlite::params_from_iter(LEGACY_SETTINGS.iter()))?;
    conn.execute("UPDATE site_settings SET value=json_remove(value,'$.quota') WHERE key='job_last' AND json_valid(value)",[])?;
    Ok(())
}

const MARKER: &str = "member_policy_v1_migrated";

pub struct MigratedMember {
    pub discord_id: String,
    pub overrides: MemberOverrides,
    pub recycle_not_before: Option<f64>,
}

pub struct Preview {
    pub defaults: MemberPolicy,
    pub budgets: SharedBudgets,
    pub claim: ClaimRules,
    pub members: Vec<MigratedMember>,
    pub summary: Summary,
}

/// 可安全输出的聚合预检结果，不含身份、凭据和请求内容。
#[derive(Debug, Default, Serialize)]
pub struct Summary {
    pub members: usize,
    pub member_exceptions: usize,
    pub v5_authorized: usize,
    pub bounded_unlimited_fields: usize,
    pub removed_shared_v5_exemptions: usize,
    pub invite_grace_periods: usize,
    pub unbound_invites: usize,
}

fn invalid(message: impl Into<String>) -> rusqlite::Error {
    rusqlite::Error::InvalidParameterName(message.into())
}

fn parse<T: serde::de::DeserializeOwned>(raw: &str, field: &str) -> rusqlite::Result<T> {
    serde_json::from_str(raw).map_err(|_| invalid(format!("权限迁移：{field} 存档无效")))
}

fn bounded(mode: Limit, inherited: f64, max: f64, summary: &mut Summary) -> rusqlite::Result<f64> {
    let value = match mode {
        Limit::Inherit => inherited,
        Limit::Unlimited => {
            summary.bounded_unlimited_fields += 1;
            max
        }
        Limit::Limit(value) => value,
    };
    if !value.is_finite() || !(0.0..=max).contains(&value) {
        return Err(invalid("权限迁移：额度超出有效范围"));
    }
    Ok(value)
}

fn count(value: f64) -> rusqlite::Result<u32> {
    if value.fract() != 0.0 || !(0.0..=9999.0).contains(&value) {
        return Err(invalid("权限迁移：每日额度必须是有效整数"));
    }
    Ok(value as u32)
}

fn subject(id: &str) -> rusqlite::Result<()> {
    let parsed = id
        .parse::<u64>()
        .map_err(|_| invalid("权限迁移：成员身份无效"))?;
    if parsed == 0 || parsed.to_string() != id {
        return Err(invalid("权限迁移：成员身份无效"));
    }
    Ok(())
}

pub fn preview(conn: &Connection, now: f64, idle_days: i64) -> rusqlite::Result<Preview> {
    if !now.is_finite() || now < 0.0 || idle_days < 0 {
        return Err(invalid("权限迁移：时间或闲置期限无效"));
    }
    let settings: BTreeMap<String, String> = conn
        .prepare("SELECT key,value FROM site_settings")?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    let raw_limits = settings
        .get("quota_modes_v1")
        .ok_or_else(|| invalid("权限迁移：缺少全站额度存档"))?;
    let global = GlobalLimits::from_value(&parse(raw_limits, "quota_modes_v1")?)
        .ok_or_else(|| invalid("权限迁移：全站额度格式无效"))?;
    let mode = settings
        .get("v5_access_mode")
        .map(String::as_str)
        .unwrap_or("legacy");
    if !["legacy", "closed", "restricted", "everyone"].contains(&mode) {
        return Err(invalid("权限迁移：V5 授权模式无效"));
    }
    let authorized: Vec<String> = settings
        .get("v5_authorized_discord_ids")
        .map(|raw| parse(raw, "V5 授权名单"))
        .transpose()?
        .unwrap_or_default();
    for id in &authorized {
        subject(id)?;
    }
    let features = settings
        .get("register_features")
        .map(String::as_str)
        .unwrap_or("image");
    let enabled = |feature: &str| features == "*" || features.split(',').any(|f| f == feature);
    let mut summary = Summary::default();
    let text = |key: &str| settings.get(key).map(String::as_str).unwrap_or("");
    let integer = |key: &str, fallback: u32| -> rusqlite::Result<u32> {
        match settings.get(key) {
            None => Ok(fallback),
            Some(value) => value
                .parse()
                .map_err(|_| invalid(format!("权限迁移：{key} 必须是整数"))),
        }
    };
    let open = match settings.get("register_open").map(String::as_str) {
        None | Some("0" | "false") => false,
        Some("1" | "true") => true,
        Some(_) => return Err(invalid("权限迁移：领取开关无效")),
    };
    let mut roles: Vec<EligibilityRule> = settings
        .get("register_role_extra")
        .filter(|raw| !raw.is_empty())
        .map(|raw| parse(raw, "领取身份组"))
        .transpose()?
        .unwrap_or_default();
    let (guild, role) = (text("register_role_guild"), text("register_role_id"));
    if guild.is_empty() != role.is_empty() {
        return Err(invalid("权限迁移：主身份组条件不完整"));
    }
    if !guild.is_empty() {
        roles.insert(
            0,
            EligibilityRule {
                guild: guild.into(),
                role: role.into(),
                note: text("register_role_note").into(),
            },
        );
    }
    let mut seen = HashSet::new();
    roles.retain(|rule| seen.insert((rule.guild.clone(), rule.role.clone())));
    let claim = ClaimRules {
        open,
        max_users: integer("register_max_users", 10)?,
        expires_days: integer("register_expires_days", 0)?,
        roles,
    };
    claim.validate().map_err(invalid)?;
    let defaults = MemberPolicy {
        daily_images: count(bounded(
            global.register_daily_images,
            0.0,
            9999.0,
            &mut summary,
        )?)?,
        daily_v5: count(bounded(
            global.register_daily_v5,
            0.0,
            9999.0,
            &mut summary,
        )?)?,
        daily_anlas: count(bounded(
            global.member_daily_anlas,
            0.0,
            9999.0,
            &mut summary,
        )?)?,
        monthly_anlas: 99999.0,
        rpm: 10,
        image: enabled("image"),
        v5: ["legacy", "everyone"].contains(&mode),
        vibe: enabled("vibe") || global.public_vibe_enabled,
        tags: enabled("tags"),
        upscale: false,
        augment: false,
        img2img: false,
        paid_parameters: false,
    };
    defaults.validate().map_err(invalid)?;
    let budgets = SharedBudgets {
        daily_v5: count(bounded(global.global_daily_v5, 0.0, 9999.0, &mut summary)?)?,
        daily_anlas: count(bounded(
            global.global_daily_anlas,
            0.0,
            9999.0,
            &mut summary,
        )?)?,
        monthly_anlas: bounded(global.global_monthly_anlas, 0.0, 99999.0, &mut summary)?,
    };
    budgets.validate().map_err(invalid)?;
    let mut members = Vec::new();
    let mut identities = HashSet::new();
    let mut stmt = conn.prepare(
        "SELECT k.discord_id,r.discord_id,k.daily_images,k.daily_v5,k.daily_anlas,k.monthly_anlas,\
         k.quota_auto,k.quota_limits,k.features,k.allow_anlas,k.allow_img2img,k.image_model_scope,\
         k.rpm,k.exclude_global_v5,EXISTS(SELECT 1 FROM key_tags t WHERE t.key_id=k.id AND t.tag='邀请') \
         FROM api_keys k LEFT JOIN discord_registrations r ON r.key_id=k.id ORDER BY k.id"
    )?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let key_id: Option<String> = row.get(0)?;
        let registration_id: Option<String> = row.get(1)?;
        let key_id = key_id.filter(|s| !s.is_empty());
        let registration_id = registration_id.filter(|s| !s.is_empty());
        if matches!((&key_id,&registration_id),(Some(a),Some(b)) if a != b) {
            return Err(invalid("权限迁移：Key 与领取记录的成员身份冲突"));
        }
        let invite: bool = row.get(14)?;
        let Some(id) = key_id.or(registration_id) else {
            if !invite {
                return Err(invalid("权限迁移：存在未绑定成员的普通 Key"));
            }
            summary.unbound_invites += 1;
            continue;
        };
        subject(&id)?;
        if !identities.insert(id.clone()) {
            return Err(invalid("权限迁移：同一成员绑定多把 Key"));
        }
        let raw: Option<String> = row.get(7)?;
        let limits = match raw {
            Some(raw) => MemberLimits::from_value(&parse(&raw, "成员额度")?)
                .ok_or_else(|| invalid("权限迁移：成员额度格式无效"))?,
            None => MemberLimits::legacy(
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
                row.get(6)?,
            ),
        };
        let raw_features: Option<String> = row.get(8)?;
        let has = |feature: &str| {
            raw_features
                .as_ref()
                .is_none_or(|raw| raw.split(',').any(|name| name == feature))
        };
        let paid: bool = row.get(9)?;
        let scope: String = row.get(11)?;
        if !["legacy", "all"].contains(&scope.as_str()) {
            return Err(invalid("权限迁移：模型范围无效"));
        }
        let configured = MemberPolicy {
            daily_images: count(bounded(
                limits.images,
                defaults.daily_images as f64,
                9999.0,
                &mut summary,
            )?)?,
            daily_v5: count(bounded(
                limits.v5,
                defaults.daily_v5 as f64,
                9999.0,
                &mut summary,
            )?)?,
            daily_anlas: count(bounded(
                limits.daily_anlas,
                defaults.daily_anlas as f64,
                9999.0,
                &mut summary,
            )?)?,
            monthly_anlas: bounded(
                limits.monthly_anlas,
                defaults.monthly_anlas,
                99999.0,
                &mut summary,
            )?,
            rpm: row.get(12)?,
            image: has("image"),
            v5: scope == "all"
                && (defaults.v5 || (mode == "restricted" && authorized.contains(&id))),
            vibe: has("vibe") || paid || global.public_vibe_enabled,
            tags: has("tags"),
            upscale: paid && row.get::<_, bool>(10)?,
            augment: paid && row.get::<_, bool>(10)?,
            img2img: row.get::<_, bool>(10)? && paid,
            paid_parameters: paid,
        };
        configured.validate().map_err(invalid)?;
        let overrides = defaults.differences(&configured);
        if overrides != MemberOverrides::default() {
            summary.member_exceptions += 1;
        }
        summary.v5_authorized += usize::from(configured.v5);
        summary.removed_shared_v5_exemptions += usize::from(row.get::<_, bool>(13)?);
        let grace = (invite && idle_days > 0).then_some(now + idle_days as f64 * 86400.0);
        summary.invite_grace_periods += usize::from(grace.is_some());
        members.push(MigratedMember {
            discord_id: id,
            overrides,
            recycle_not_before: grace,
        });
    }
    // Explicit pre-authorizations also belong to stable subjects who have not claimed a credential yet.
    if mode == "restricted" {
        for id in &authorized {
            if identities.insert(id.clone()) {
                members.push(MigratedMember {
                    discord_id: id.clone(),
                    overrides: MemberOverrides {
                        v5: Some(true),
                        ..Default::default()
                    },
                    recycle_not_before: None,
                });
                summary.member_exceptions += 1;
                summary.v5_authorized += 1;
            }
        }
    }
    summary.members = members.len();
    // Check orphaned registrations too; silently dropping one would change ownership.
    let orphan: Option<i64> = conn.query_row(
        "SELECT 1 FROM discord_registrations r LEFT JOIN api_keys k ON k.id=r.key_id WHERE k.id IS NULL LIMIT 1",
        [], |r| r.get(0),
    ).optional()?;
    if orphan.is_some() {
        return Err(invalid("权限迁移：存在失去 Key 的领取记录"));
    }
    Ok(Preview {
        defaults,
        budgets,
        claim,
        members,
        summary,
    })
}

/// 启动阶段调用，必须在任何请求或后台任务启动之前完成。
pub fn run(
    conn: &mut Connection,
    path: &Path,
    now: f64,
    idle_days: i64,
) -> crate::Result<Option<Summary>> {
    let migrated: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM site_settings WHERE key=?)",
        [MARKER],
        |r| r.get(0),
    )?;
    if migrated {
        let raw: String = conn.query_row(
            "SELECT value FROM site_settings WHERE key=?",
            [crate::repo::member_policy::CLAIM_KEY],
            |r| r.get(0),
        )?;
        let claim: ClaimRules = parse(&raw, "领取条件")?;
        claim.validate().map_err(invalid)?;
        let raw: String = conn.query_row(
            "SELECT value FROM site_settings WHERE key=?",
            [crate::repo::member_policy::BUDGETS_KEY],
            |r| r.get(0),
        )?;
        let budgets: SharedBudgets = parse(&raw, "全站预算")?;
        budgets.validate().map_err(invalid)?;
        let raw: String = conn.query_row(
            "SELECT value FROM site_settings WHERE key=?",
            [crate::repo::member_policy::DEFAULTS_KEY],
            |r| r.get(0),
        )?;
        let defaults: MemberPolicy = parse(&raw, "默认规则")?;
        defaults.validate().map_err(invalid)?;
        let mut statement = conn.prepare("SELECT overrides FROM member_rules")?;
        for row in statement.query_map([], |r| r.get::<_, String>(0))? {
            let overrides: MemberOverrides = parse(&row?, "成员例外")?;
            defaults.resolve(&overrides).map_err(invalid)?;
        }
        drop(statement);
        if cleanup_needed(conn)? {
            backup(conn, path, now)?;
        }
        let transaction = conn.transaction()?;
        cleanup(&transaction)?;
        transaction.commit()?;
        return Ok(None);
    }
    let legacy = column_exists(conn, "api_keys", "daily_images")?;
    let plan = if legacy {
        preview(conn, now, idle_days)?
    } else {
        let keys: i64 = conn.query_row("SELECT count(*) FROM api_keys", [], |row| row.get(0))?;
        let settings:i64=conn.query_row("SELECT count(*) FROM site_settings WHERE key IN ('member_policy_defaults','claim_rules','runtime_budgets')",[],|row|row.get(0))?;
        if keys != 0 || settings != 0 || legacy_settings_present(conn)? {
            return Err(invalid("成员规则缺少完成标记，停止以避免猜测权限").into());
        }
        Preview {
            defaults: MemberPolicy::initial(),
            budgets: SharedBudgets::initial(),
            claim: ClaimRules::initial(),
            members: vec![],
            summary: Summary::default(),
        }
    };
    let existing: i64 = conn.query_row("SELECT count(*) FROM member_rules", [], |r| r.get(0))?;
    if existing != 0 {
        return Err(invalid("权限迁移：已存在未标记的成员规则，停止以避免覆盖").into());
    }
    if legacy {
        backup(conn, path, now)?;
    }
    let transaction = conn.transaction()?;
    transaction.execute(
        "INSERT INTO site_settings(key,value) VALUES(?,?)",
        rusqlite::params![
            crate::repo::member_policy::CLAIM_KEY,
            serde_json::to_string(&plan.claim).map_err(|e| invalid(e.to_string()))?
        ],
    )?;
    transaction.execute(
        "INSERT INTO site_settings(key,value) VALUES(?,?)",
        rusqlite::params![
            crate::repo::member_policy::BUDGETS_KEY,
            serde_json::to_string(&plan.budgets).map_err(|e| invalid(e.to_string()))?
        ],
    )?;
    transaction.execute(
        "INSERT INTO site_settings(key,value) VALUES(?,?)",
        rusqlite::params![
            crate::repo::member_policy::DEFAULTS_KEY,
            serde_json::to_string(&plan.defaults).map_err(|e| invalid(e.to_string()))?
        ],
    )?;
    for member in plan.members {
        transaction.execute(
            "INSERT INTO member_rules(discord_id,overrides,recycle_not_before) VALUES(?,?,?)",
            rusqlite::params![
                member.discord_id,
                serde_json::to_string(&member.overrides).map_err(|e| invalid(e.to_string()))?,
                member.recycle_not_before
            ],
        )?;
    }
    cleanup(&transaction)?;
    transaction.execute(
        "INSERT INTO site_settings(key,value) VALUES(?,'1')",
        [MARKER],
    )?;
    transaction.commit()?;
    Ok(Some(plan.summary))
}

fn backup(conn: &Connection, path: &Path, now: f64) -> crate::Result<()> {
    let mut base = path.as_os_str().to_owned();
    base.push(".before-member-policy");
    let base = std::path::PathBuf::from(base);
    if base.is_symlink() {
        return Err(std::io::Error::other("权限备份目录不能是符号链接").into());
    }
    std::fs::create_dir_all(&base)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&base, std::fs::Permissions::from_mode(0o700))?;
    }
    let mut directory = None;
    for attempt in 0..100 {
        let candidate = base.join(format!("{now:.6}-{attempt}"));
        match std::fs::create_dir(&candidate) {
            Ok(()) => {
                directory = Some(candidate);
                break;
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.into()),
        }
    }
    let directory = directory.ok_or_else(|| invalid("权限备份重试次数过多"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))?;
    }
    let database = directory.join("database.sqlite");
    conn.execute("VACUUM INTO ?", [database.to_string_lossy().as_ref()])?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&database, std::fs::Permissions::from_mode(0o600))?;
    }
    let recovery =
        Connection::open_with_flags(&database, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let integrity: String = recovery.query_row("PRAGMA integrity_check", [], |r| r.get(0))?;
    if integrity != "ok" {
        return Err(invalid("权限备份完整性检查失败").into());
    }
    Ok(())
}
