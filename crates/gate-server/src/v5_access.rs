use crate::state::AppState;
use gate_core::v5_access::{self, Decision};

pub const MODE_KEY: &str = "v5_access_mode";
pub const SUBJECTS_KEY: &str = "v5_authorized_discord_ids";
pub const OWNERS_KEY: &str = "gateway_owner_discord_ids";
pub const ADMINS_KEY: &str = "gateway_admin_discord_ids";

async fn ids(state: &AppState, key: &str) -> Result<Vec<String>, gate_db::DbError> {
    Ok(state
        .db
        .get_setting(key)
        .await?
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default())
}

pub async fn decide(
    state: &AppState,
    discord_id: Option<&str>,
) -> Result<Decision, gate_db::DbError> {
    let mode = v5_access::mode_from_raw(state.db.get_setting(MODE_KEY).await?.as_deref());
    let authorized = ids(state, SUBJECTS_KEY).await?;
    Ok(v5_access::decide(mode, discord_id, &authorized))
}

pub async fn v5_allowed(state: &AppState, key: &crate::key::Key) -> Result<bool, gate_db::DbError> {
    Ok(decide(state, Some(key.str("discord_id"))).await?.allowed)
}

pub async fn roles(state: &AppState, discord_id: &str) -> Result<(bool, bool), gate_db::DbError> {
    let owners = ids(state, OWNERS_KEY).await?;
    let admins = ids(state, ADMINS_KEY).await?;
    Ok((
        owners.iter().any(|id| id == discord_id),
        admins.iter().any(|id| id == discord_id),
    ))
}
