//! 领取条件与全站运行开关的读写。
//!
//! 所有值保存在 `site_settings`，没设过时用 [`crate::defaults`] 里的常量（没有环境变量）；机器人每次命令都实时读取，所以改了立刻生效。
//! 校验失败的文案、存进库里的字符串（整数不带小数点、开关 "1" / "0"）都是固定格式：库里的行要长期可读，客户端和后台依赖这些文案。
//!
//! 另外放几个后台接口共用的单例：
//! 测试告警的 10 秒节流、运行期限制的写锁、Anlas 核对的互斥锁。

use crate::error::{ApiError, HttpError, InternalError};
use crate::state::AppState;
use gate_core::features::{self, FEATURES, GLOBAL_KEY};
use gate_core::json;
use gate_core::site_flags;
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
    /// 手动账号信息查询：全局冷却与串行化。
    pub account_information: tokio::sync::Mutex<()>,
}

// ------------------------------------------------------------------ 手动额度

/// V5 暂停（手动开关 `v5_paused`，站长 10-23；按电量自动暂停已删除）。
pub async fn v5_paused(db: &Db) -> Result<bool, DbError> {
    crate::settings::site_bool(db, &site_flags::V5_PAUSED).await
}

/// 全站共享 V5 上限，0 表示停用。
pub async fn global_daily_v5(db: &Db) -> Result<i64, DbError> {
    Ok(i64::from(db.shared_budgets().await?.daily_v5))
}

pub use gate_core::member_policy::EligibilityRule as RoleRule;

#[derive(Debug, Clone, PartialEq)]
pub struct Registration {
    pub configured: bool,
    pub open: bool,
    pub max_users: i64,
    pub expires_days: i64,
    pub roles: Vec<RoleRule>,
}

impl Registration {
    pub fn role_rules(&self) -> Vec<RoleRule> {
        self.roles.clone()
    }
    pub fn role_note_text(&self) -> String {
        let notes = self
            .roles
            .iter()
            .map(|rule| rule.note.as_str())
            .filter(|note| !note.is_empty())
            .collect::<Vec<_>>();
        if notes.is_empty() {
            "指定身份组".into()
        } else {
            notes.join(" 或 ")
        }
    }
    pub fn to_map(&self) -> Map<String, Value> {
        json!({"configured":self.configured,"open":self.open,"max_users":self.max_users,
            "expires_days":self.expires_days,"roles":self.roles})
        .as_object()
        .expect("object")
        .clone()
    }
}

pub async fn registration_settings(state: &AppState) -> Result<Registration, DbError> {
    registration_settings_for(&state.db, state.env.registration.is_some()).await
}

pub async fn registration_settings_for(db: &Db, configured: bool) -> Result<Registration, DbError> {
    let rules = db.claim_rules().await?;
    Ok(Registration {
        configured,
        open: rules.open,
        max_users: i64::from(rules.max_users),
        expires_days: i64::from(rules.expires_days),
        roles: rules.roles,
    })
}

pub async fn set_registration(state: &AppState, body: &Map<String, Value>) -> Result<(), OpsError> {
    let rules: gate_core::member_policy::ClaimRules =
        serde_json::from_value(Value::Object(body.clone()))
            .map_err(|_| OpsError::Invalid("领取条件格式无效".into()))?;
    rules
        .validate()
        .map_err(|message| OpsError::Invalid(message.into()))?;
    state.db.set_claim_rules(&rules).await?;
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
        .set_setting(GLOBAL_KEY, &json::dumps(&Value::Object(current)))
        .await?;
    Ok(())
}

/// Save the request metadata retention policy.
pub async fn set_log_retention(
    state: &AppState,
    body: &Map<String, Value>,
) -> Result<Value, OpsError> {
    let days = body
        .get("origin_keep_days")
        .and_then(Value::as_i64)
        .filter(|v| (0..=365).contains(v))
        .ok_or_else(|| OpsError::Invalid("origin_keep_days 必须是 0 到 365 的整数".into()))?;
    state
        .db
        .set_setting(site_flags::ORIGIN_KEEP_DAYS.key, &days.to_string())
        .await?;
    Ok(json!({"origin_keep_days":days}))
}
