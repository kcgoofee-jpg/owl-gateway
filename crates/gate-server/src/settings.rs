//! Typed site setting readers.
use gate_core::site_flags;
use gate_db::{Db, DbError};
use serde_json::Value;
pub(crate) async fn site_value(db: &Db, spec: &site_flags::Spec) -> Result<Value, DbError> {
    let raw = db.get_setting(spec.key).await?;
    Ok(site_flags::get_from_raw(spec, raw.as_deref()))
}

/// 数值读成整数：`number` 后向零取整，读不懂为 0。
pub(crate) fn int_of(v: &Value) -> i64 {
    v.as_i64().unwrap_or(0)
}

/// 整数类设置（登记表里是 Int 的 spec，解析后一定是整数，缺失时是登记表的默认值，所以和 `as_i64` 结果相同）。
pub(crate) async fn site_int(db: &Db, spec: &site_flags::Spec) -> Result<i64, DbError> {
    Ok(int_of(&site_value(db, spec).await?))
}

/// 数值型设置；读不懂为 0。
pub(crate) async fn site_f64(db: &Db, spec: &site_flags::Spec) -> Result<f64, DbError> {
    Ok(site_value(db, spec).await?.as_f64().unwrap_or(0.0))
}

pub(crate) async fn site_bool(db: &Db, spec: &site_flags::Spec) -> Result<bool, DbError> {
    Ok(site_value(db, spec).await?.as_bool().unwrap_or(false))
}

pub async fn usage_origin_keep_days(db: &Db) -> Result<i64, DbError> {
    site_int(db, &site_flags::ORIGIN_KEEP_DAYS).await
}
