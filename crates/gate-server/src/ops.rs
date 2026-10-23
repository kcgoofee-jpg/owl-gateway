//! 站长可在运行中调整的开关：自助领 Key（注册）、全局功能、生成记录。
//!
//! 所有值保存在 `site_settings`，没设过时用 [`crate::defaults`] 里的常量（没有环境变量）；机器人每次命令都实时读取，所以改了立刻生效。
//! 校验失败的文案、存进库里的字符串（整数不带小数点、开关 "1" / "0"）都是固定格式：库里的行要长期可读，客户端和后台依赖这些文案。
//!
//! 另外放几个后台接口共用的单例：
//! 测试告警的 10 秒节流、运行期限制的写锁、Anlas 核对的互斥锁。

use crate::audit::{audit_flags, audit_image_cap_mb, audit_image_keep_hours};
use crate::defaults;
use crate::error::{ApiError, HttpError, InternalError};
use crate::state::AppState;
use gate_core::features::{self, FEATURES, GLOBAL_KEY};
use gate_core::site_flags::{self, GLOBAL_DAILY_V5, REGISTER_OPEN};
use gate_core::{pyc, pyjson, pyval};
use gate_db::{Db, DbError};
use serde_json::{Map, Value, json};
use std::sync::Mutex;

/// 后台这几个接口里的错误：参数不合法回 422，在别处管的设置回 400，库里的数据坏了回 500。
#[derive(Debug)]
pub enum OpsError {
    /// 设置值不合法：路由层回 422，原文作为 `detail`。
    Invalid(String),
    /// 想在这里改别处管的设置：400。
    Managed(String),
    Db(DbError),
    /// 库里的存档坏了（会让读到它的接口 500）。
    Corrupt(String),
}

impl From<DbError> for OpsError {
    fn from(e: DbError) -> Self {
        OpsError::Db(e)
    }
}

impl From<OpsError> for ApiError {
    fn from(e: OpsError) -> Self {
        match e {
            OpsError::Invalid(m) => HttpError::new(422, m).into(),
            OpsError::Managed(m) => HttpError::new(400, m).into(),
            OpsError::Db(e) => e.into(),
            OpsError::Corrupt(m) => InternalError(m).into(),
        }
    }
}

// ------------------------------------------------------------------ 进程内共享状态

/// 后台接口共用的单例，挂在 `AppState::ops` 上。
#[derive(Default)]
pub struct Shared {
    /// 上一次发测试告警的时间。
    pub alert_test_at: Mutex<f64>,
    /// 运行期限制的写锁：写库 + 生效必须连在一起。
    pub runtime_limits: tokio::sync::Mutex<()>,
    /// Anlas 核对的互斥锁：同一时刻只允许一次。
    pub reconcile: tokio::sync::Mutex<()>,
}

// ------------------------------------------------------------------ 手动额度

/// V5 暂停（手动开关 `v5_paused`，站长 10-23；按电量自动暂停已删除）。
pub async fn v5_paused(db: &Db) -> Result<bool, DbError> {
    crate::audit::site_bool(db, &site_flags::V5_PAUSED).await
}

/// 全站每日 V5（手动，0 = 不限）。
pub async fn global_daily_v5(db: &Db) -> Result<i64, DbError> {
    crate::audit::site_int(db, &GLOBAL_DAILY_V5).await
}

// ------------------------------------------------------------------ 注册设置

/// 生效的注册设置：数据库里保存的优先，其次是 [`crate::defaults`] 里的常量。
#[derive(Debug, Clone, PartialEq)]
pub struct Registration {
    pub configured: bool,
    pub open: bool,
    pub max_users: i64,
    /// None = 全部已开放功能（含以后新开放的）。
    pub features: Option<Vec<String>>,
    pub daily_images: i64,
    pub daily_v5: i64,
    pub image_scope: String,
    pub expires_days: i64,
    pub role_guild: String,
    pub role_id: String,
    pub role_note: String,
    /// 额外的限定身份组（站长 10-18：类脑「社区建设组」或棱镜「创作者」，满足任意一个即可）。
    pub role_extra: Vec<RoleRule>,
}

/// 一条身份组门槛：在 `guild` 这个服务器里拥有 `role` 这个身份组；`note` 是告诉成员需要什么身份的显示名。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoleRule {
    pub guild: String,
    pub role: String,
    pub note: String,
}

impl RoleRule {
    pub fn to_json(&self) -> Value {
        json!({"guild": self.guild, "role": self.role, "note": self.note})
    }
}

/// 最多几条额外身份组（加上主门槛一共 `MAX_ROLE_RULES + 1` 条）。
pub const MAX_ROLE_EXTRA: usize = 5;

/// 读额外身份组的存档（JSON 数组）；读不懂就当没有（门槛只会变严，不会放行）。
pub fn parse_role_extra(raw: &str) -> Vec<RoleRule> {
    let Ok(Value::Array(list)) = serde_json::from_str::<Value>(raw) else {
        return Vec::new();
    };
    let text = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    list.iter()
        .map(|v| RoleRule {
            guild: text(v, "guild"),
            role: text(v, "role"),
            note: text(v, "note"),
        })
        .filter(|r| !r.guild.is_empty() && !r.role.is_empty())
        .collect()
}

impl Registration {
    /// 生效的全部身份组门槛：主门槛（服务器 + 身份组都填了才算）在前，再加额外的；满足任意一条就算通过。
    /// 空 = 不限定。
    pub fn role_rules(&self) -> Vec<RoleRule> {
        let mut out = Vec::new();
        if !self.role_guild.is_empty() && !self.role_id.is_empty() {
            out.push(RoleRule {
                guild: self.role_guild.clone(),
                role: self.role_id.clone(),
                note: self.role_note.clone(),
            });
        }
        out.extend(self.role_extra.iter().cloned());
        out
    }

    /// 告诉成员需要什么身份：各条的显示名用「 或 」连起来；都没写显示名就是「指定身份组」。
    pub fn role_note_text(&self) -> String {
        let rules = self.role_rules();
        let notes: Vec<&str> = rules
            .iter()
            .map(|r| r.note.as_str())
            .filter(|n| !n.is_empty())
            .collect();
        if notes.is_empty() {
            "指定身份组".to_string()
        } else {
            notes.join(" 或 ")
        }
    }

    /// 键顺序固定，后台和机器人按它读。
    pub fn to_map(&self) -> Map<String, Value> {
        let features = match &self.features {
            Some(list) => json!(list),
            None => Value::Null,
        };
        let v = json!({
            "configured": self.configured,
            "open": self.open,
            "max_users": self.max_users,
            "features": features,
            "daily_images": self.daily_images,
            "daily_v5": self.daily_v5,
            "image_scope": self.image_scope,
            "expires_days": self.expires_days,
            "role_guild": self.role_guild,
            "role_id": self.role_id,
            "role_note": self.role_note,
            "role_extra": self.role_extra.iter().map(RoleRule::to_json).collect::<Vec<_>>(),
        });
        match v {
            Value::Object(m) => m,
            _ => Map::new(),
        }
    }
}

/// `max(0, int(float(value)))`，读不懂就用默认值。
fn to_int(raw: Option<&str>, default: i64) -> i64 {
    raw.and_then(pyc::parse_float)
        .and_then(pyc::int_of)
        .map_or(default, |n| n.max(0))
}

pub async fn registration_settings(state: &AppState) -> Result<Registration, DbError> {
    registration_settings_for(&state.db, state.env.registration.is_some()).await
}

/// 新建 Key 未指定功能时的默认功能：沿用「新成员默认开通的功能」（默认只有文生图），而不是全部功能。
/// `None` = 站长明确把默认设为「全部已开放功能」（不限）；否则是 `features::dump` 后的字符串。
pub async fn default_new_key_features(state: &AppState) -> Result<Option<String>, DbError> {
    let reg = registration_settings(state).await?;
    if reg.features.is_none()
        && state.db.get_setting("register_features").await?.as_deref() == Some("*")
    {
        return Ok(None);
    }
    let list = reg
        .features
        .filter(|f| !f.is_empty())
        .unwrap_or_else(|| vec![defaults::REGISTER_FEATURES.to_string()]);
    Ok(features::dump(Some(list)))
}

/// 同 [`registration_settings`]，但只要库和「注册服务有没有配齐」（`RegistrationService` 里没有 `AppState`，用这个）。
/// 没配注册服务时名额不限、功能不限（没有人能自助领 Key，这些值只给后台看）。
pub async fn registration_settings_for(db: &Db, configured: bool) -> Result<Registration, DbError> {
    let read = |name: &'static str| async move { db.get_setting(name).await };
    let open_raw = read("register_open").await?;
    let feats_raw = read("register_features").await?;
    let open = site_flags::parse(
        &REGISTER_OPEN,
        &open_raw.map_or(Value::Null, Value::String),
        Value::Bool(false), // 默认关闭：站长明确开放后才接受注册
    )
    .as_bool()
    .unwrap_or(false);
    let max_users = to_int(
        read("register_max_users").await?.as_deref(),
        if configured {
            defaults::REGISTER_MAX_USERS
        } else {
            0
        },
    );
    let features = match feats_raw {
        Some(s) if s == "*" => None,
        Some(s) => features::parse_list(&Value::String(s)),
        None if configured => {
            features::parse_list(&Value::String(defaults::REGISTER_FEATURES.into()))
        }
        None => None,
    };
    // 新成员的模型范围固定「含 V5」（10-23）：V5 用不用得了由手动的每人 / 全站 V5 和「V5 暂停」管，
    // 旧的 register_image_scope 不再读
    let image_scope = "all".to_string();
    // 默认不过期（站长 10-15）
    let expires_days = to_int(
        read("register_expires_days").await?.as_deref(),
        defaults::REGISTER_EXPIRES_DAYS,
    );
    // 限定身份组：只有在这个 Discord 服务器里拥有这个身份组的人能领 Key（可以是别的社区的服务器）。
    // 都为空 = 不限定（环境变量 DISCORD_ROLE_ID 已删除，身份组只在后台「领 Key」里设）。
    let role_guild = read("register_role_guild").await?.unwrap_or_default();
    let role_id = read("register_role_id").await?.unwrap_or_default();
    let role_note = read("register_role_note").await?.unwrap_or_default();
    let role_extra = parse_role_extra(&read("register_role_extra").await?.unwrap_or_default());
    Ok(Registration {
        configured,
        open,
        max_users,
        features,
        daily_images: crate::audit::site_int(db, &site_flags::REGISTER_DAILY_IMAGES).await?,
        daily_v5: crate::audit::site_int(db, &site_flags::REGISTER_DAILY_V5).await?,
        image_scope,
        expires_days,
        role_guild,
        role_id,
        role_note,
        role_extra,
    })
}

// 2.x 这里还有 `v5_capacity`（「名额 × 每人 V5 是否超过全站 V5 日限」的超卖提示）。额度是 PVP，
// 每人上限只是上限，「名额 × 每人」本来就可以超过全站池；3.0 删除。

/// 取整数字段：JSON 数字（小数向零取整，如 7.9 → 7）；不是数字（字符串、bool、null、数组）回 422「需要是整数」。
fn range_int(
    body: &Map<String, Value>,
    name: &str,
    low: i64,
    high: i64,
) -> Result<Option<i64>, OpsError> {
    let Some(raw) = body.get(name) else {
        return Ok(None);
    };
    let n = raw
        .as_f64()
        .map(f64::trunc)
        .ok_or_else(|| OpsError::Invalid(format!("{name} 需要是整数")))?;
    if !(low as f64..=high as f64).contains(&n) {
        return Err(OpsError::Invalid(format!("{name} 超出范围 {low}～{high}")));
    }
    Ok(Some(n as i64))
}

/// 文本字段：字符串去首尾空白；不是字符串当空。
fn text_field(v: &Value) -> String {
    v.as_str().map(str::trim).unwrap_or_default().to_string()
}

/// 后台提交的额外身份组（`[{guild, role, note}]`）→ 存档 JSON；空数组存成空串。
/// 每条的服务器 ID / 身份组 ID 都要是 15–21 位数字；最多 [`MAX_ROLE_EXTRA`] 条；重复的只留一条。
fn role_extra_to_store(v: &Value) -> Result<String, OpsError> {
    let list = match v {
        Value::Null => return Ok(String::new()),
        Value::Array(list) => list,
        _ => return Err(OpsError::Invalid("role_extra 需要是列表".into())),
    };
    let mut out: Vec<RoleRule> = Vec::new();
    for item in list {
        let get = |k: &str| item.get(k).map(text_field).unwrap_or_default();
        let (guild, role) = (get("guild"), get("role"));
        if guild.is_empty() && role.is_empty() {
            continue; // 空行：忽略
        }
        for (name, id) in [("服务器 ID", &guild), ("身份组 ID", &role)] {
            let ok = gate_core::pyext::is_decimal(id) && (15..=21).contains(&pyc::len_chars(id));
            if !ok {
                return Err(OpsError::Invalid(format!(
                    "额外身份组的{name}需要是 15～21 位的 Discord 数字 ID"
                )));
            }
        }
        let rule = RoleRule {
            guild,
            role,
            note: pyc::take_chars(&get("note"), 60).to_string(),
        };
        if !out
            .iter()
            .any(|r| r.guild == rule.guild && r.role == rule.role)
        {
            out.push(rule);
        }
    }
    if out.len() > MAX_ROLE_EXTRA {
        return Err(OpsError::Invalid(format!(
            "额外身份组最多 {MAX_ROLE_EXTRA} 条"
        )));
    }
    if out.is_empty() {
        return Ok(String::new());
    }
    Ok(Value::Array(out.iter().map(RoleRule::to_json).collect()).to_string())
}

/// 保存注册设置（只保存；开放 / 名额变化时发成员公告 10-23 删除，旧客户端带的 `notify` 忽略）。
/// 每人每日 V4.5 / V5 是全站统一的手动额度，在「额度与保护」里改（`/admin/api/settings`）；模型范围固定含 V5：
/// 这里带这三个字段回 400。
pub async fn set_registration(state: &AppState, body: &Map<String, Value>) -> Result<(), OpsError> {
    let db = &state.db;
    if ["daily_images", "daily_v5", "image_scope"]
        .iter()
        .any(|k| body.contains_key(*k))
    {
        return Err(OpsError::Managed(
            "每人每日 V4.5 / V5 张数是全站统一的手动额度，请在「额度与保护」里改".into(),
        ));
    }
    let mut values: Vec<(String, String)> = Vec::new();
    if let Some(v) = body.get("open") {
        values.push((
            "register_open".into(),
            if pyval::truthy(v) { "1" } else { "0" }.into(),
        ));
    }
    for (name, low, high) in [("max_users", 0, 1000), ("expires_days", 0, 3650)] {
        if let Some(n) = range_int(body, name, low, high)? {
            values.push((format!("register_{name}"), n.to_string()));
        }
    }
    for name in ["role_guild", "role_id"] {
        if let Some(v) = body.get(name) {
            let t = text_field(v);
            let digits = gate_core::pyext::is_decimal(&t);
            let len = pyc::len_chars(&t);
            if !t.is_empty() && !(digits && (15..=21).contains(&len)) {
                return Err(OpsError::Invalid(format!(
                    "{name} 需要是 Discord 的数字 ID"
                )));
            }
            values.push((format!("register_{name}"), t));
        }
    }
    if body.contains_key("role_guild") || body.contains_key("role_id") {
        let current = |name: &'static str| async move {
            Ok::<String, DbError>(
                db.get_setting(name)
                    .await?
                    .map(|s| pyc::strip(&s).to_string())
                    .unwrap_or_default(),
            )
        };
        let g = match body.get("role_guild") {
            Some(v) => text_field(v),
            None => current("register_role_guild").await?,
        };
        let r = match body.get("role_id") {
            Some(v) => text_field(v),
            None => current("register_role_id").await?,
        };
        if g.is_empty() != r.is_empty() {
            return Err(OpsError::Invalid(
                "限定身份组的服务器 ID 与身份组 ID 必须同时填写，或全部留空".into(),
            ));
        }
    }
    if let Some(v) = body.get("role_note") {
        values.push((
            "register_role_note".into(),
            pyc::take_chars(&text_field(v), 60).to_string(),
        ));
    }
    if let Some(v) = body.get("role_extra") {
        values.push(("register_role_extra".into(), role_extra_to_store(v)?));
    }
    if let Some(v) = body.get("features") {
        // null = 全部已开放功能，用 "*" 显式保存；空串表示「一个功能都不开」。
        let stored = match v {
            Value::Null => "*".to_string(),
            Value::Array(list) => {
                let names: Vec<&str> = list.iter().filter_map(Value::as_str).collect();
                features::dump(Some(names)).unwrap_or_default()
            }
            _ => String::new(),
        };
        values.push(("register_features".into(), stored));
    }
    if !values.is_empty() {
        db.set_settings_bulk(&values).await?;
    }
    Ok(())
}

// ------------------------------------------------------------------ 全局功能开关

/// 全局开关：功能名 → 是否开启，按 `FEATURES` 的顺序。缺省全部开启。
pub async fn global_flags(state: &AppState) -> Result<Map<String, Value>, OpsError> {
    let raw = state.db.get_setting(GLOBAL_KEY).await?;
    let flags = features::parse_global_flags(raw.as_deref())
        .map_err(|e| OpsError::Corrupt(format!("{GLOBAL_KEY} 存档损坏：{e}")))?;
    Ok(FEATURES
        .iter()
        .map(|(name, _)| {
            (
                (*name).to_string(),
                Value::Bool(flags.get(name).copied().unwrap_or(true)),
            )
        })
        .collect())
}

pub async fn set_global_features(
    state: &AppState,
    flags: &Map<String, Value>,
) -> Result<(), OpsError> {
    let mut current = global_flags(state).await?;
    for (name, value) in flags {
        if FEATURES.iter().any(|(n, _)| n == name) && value.is_boolean() {
            current.insert(name.clone(), value.clone());
        }
    }
    state
        .db
        .set_setting(GLOBAL_KEY, &pyjson::dumps(&Value::Object(current)))
        .await?;
    Ok(())
}

// ------------------------------------------------------------------ 生成记录

/// 保存记录开关与保留天数；返回生效后的值（机器人 `/ops audit` 把它连同提示语一起回给管理员）。
pub async fn set_audit(state: &AppState, body: &Map<String, Value>) -> Result<Value, OpsError> {
    let db = &state.db;
    let (mut prompts, mut thumbs, mut days) = audit_flags(db).await?;
    let mut cap_mb = audit_image_cap_mb(db).await?;
    let mut keep_hours = audit_image_keep_hours(db).await?;
    let mut origin_days = crate::audit::usage_origin_keep_days(db).await?;
    // 不是数字统一回 422「参数无效」；小数向零取整，再夹到 0～hi
    let clamped = |v: &Value, hi: i64| -> Result<i64, OpsError> {
        let n = v
            .as_f64()
            .ok_or_else(|| OpsError::Invalid("参数无效".into()))?;
        Ok(n.trunc().clamp(0.0, hi as f64) as i64)
    };
    if let Some(v) = body.get("prompts") {
        prompts = pyval::truthy(v);
    }
    if let Some(v) = body.get("thumbs") {
        thumbs = pyval::truthy(v);
    }
    if let Some(v) = body.get("retention_days") {
        days = clamped(v, 3650)?; // 0 = 长期保留
    }
    if let Some(v) = body.get("image_cap_mb") {
        cap_mb = clamped(v, 500_000)?; // 0 = 不限
    }
    if let Some(v) = body.get("image_keep_hours") {
        keep_hours = clamped(v, 8760)?; // 0 = 永久
    }
    if let Some(v) = body.get("origin_keep_days") {
        origin_days = clamped(v, 365)?; // 0 = 永久
    }
    // site_flags.put(spec, value)：写 encode 之后的串
    let spec = &site_flags::IMAGE_KEEP_HOURS;
    let text =
        site_flags::encode(spec, &Value::from(keep_hours)).map_err(|e| OpsError::Invalid(e.0))?;
    db.set_setting(spec.key, &text).await?;
    db.set_settings_bulk(&[
        (
            "audit_prompts".into(),
            if prompts { "1" } else { "0" }.into(),
        ),
        ("audit_thumbs".into(), if thumbs { "1" } else { "0" }.into()),
        ("audit_retention_days".into(), days.to_string()),
        (site_flags::IMAGE_CAP_MB.key.into(), cap_mb.to_string()),
        (
            site_flags::ORIGIN_KEEP_DAYS.key.into(),
            origin_days.to_string(),
        ),
    ])
    .await?;
    Ok(json!({
        "prompts": prompts, "thumbs": thumbs, "retention_days": days,
        "image_cap_mb": cap_mb, "image_keep_hours": keep_hours, "origin_keep_days": origin_days,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn to_int_is_forgiving() {
        assert_eq!(to_int(Some("12"), 5), 12);
        assert_eq!(to_int(Some("12.9"), 5), 12);
        assert_eq!(to_int(Some("-3"), 5), 0);
        assert_eq!(to_int(Some("abc"), 5), 5);
        assert_eq!(to_int(None, 5), 5);
    }
}
