//! Typed site setting readers.

use gate_core::site_flags;
use gate_db::{Db, DbError};
use serde_json::Value;

pub(crate) async fn site_value(db: &Db, spec: &site_flags::Spec) -> Result<Value, DbError> {
    let raw = db.get_setting(spec.key).await?;
    Ok(site_flags::get_from_raw(spec, raw.as_deref()))
}

/// 数值型设置；读不懂为 0。
pub(crate) async fn site_f64(db: &Db, spec: &site_flags::Spec) -> Result<f64, DbError> {
    Ok(site_value(db, spec).await?.as_f64().unwrap_or(0.0))
}

pub(crate) async fn site_bool(db: &Db, spec: &site_flags::Spec) -> Result<bool, DbError> {
    Ok(site_value(db, spec).await?.as_bool().unwrap_or(false))
}
