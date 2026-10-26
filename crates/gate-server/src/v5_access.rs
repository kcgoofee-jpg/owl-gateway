use crate::state::AppState;
use gate_core::v5_access::{self, Decision};

pub const MODE_KEY: &str = "v5_access_mode";
pub const SUBJECTS_KEY: &str = "v5_authorized_discord_ids";
pub const OWNERS_KEY: &str = "gateway_owner_discord_ids";
pub const ADMINS_KEY: &str = "gateway_admin_discord_ids";

#[derive(Debug, Clone, Copy)]
pub struct Snapshot {
    pub decision: Decision,
    pub paused: bool,
    pub owner: bool,
    pub admin: bool,
    pub mode: v5_access::Mode,
}

#[derive(Debug, Clone)]
pub struct Configuration {
    pub mode: v5_access::Mode,
    pub authorized: Vec<String>,
    pub owners: Vec<String>,
    pub admins: Vec<String>,
    pub paused: bool,
}

fn ids(raw: Option<&String>) -> Vec<String> {
    raw.and_then(|raw| serde_json::from_str(raw).ok())
        .unwrap_or_default()
}

pub async fn configuration(state: &AppState) -> Result<Configuration, gate_db::DbError> {
    configuration_for(&state.db).await
}

pub async fn configuration_for(db: &gate_db::Db) -> Result<Configuration, gate_db::DbError> {
    let raw = db
        .get_settings_bulk(&[MODE_KEY, SUBJECTS_KEY, OWNERS_KEY, ADMINS_KEY, "v5_paused"])
        .await?;
    Ok(Configuration {
        mode: v5_access::mode_from_raw(raw.get(MODE_KEY).map(String::as_str)),
        authorized: ids(raw.get(SUBJECTS_KEY)),
        owners: ids(raw.get(OWNERS_KEY)),
        admins: ids(raw.get(ADMINS_KEY)),
        paused: raw
            .get("v5_paused")
            .is_some_and(|value| value == "1" || value.eq_ignore_ascii_case("true")),
    })
}

impl Configuration {
    pub fn evaluate(&self, discord_id: Option<&str>) -> Snapshot {
        let matches = |subjects: &[String]| {
            discord_id.is_some_and(|id| subjects.iter().any(|subject| subject == id))
        };
        Snapshot {
            decision: v5_access::decide(self.mode, discord_id, &self.authorized),
            paused: self.paused,
            owner: matches(&self.owners),
            admin: matches(&self.admins),
            mode: self.mode,
        }
    }
}

pub async fn evaluate(
    state: &AppState,
    discord_id: Option<&str>,
) -> Result<Snapshot, gate_db::DbError> {
    Ok(configuration(state).await?.evaluate(discord_id))
}

pub async fn v5_allowed(state: &AppState, key: &crate::key::Key) -> Result<bool, gate_db::DbError> {
    Ok(evaluate(state, Some(key.str("discord_id")))
        .await?
        .decision
        .allowed)
}
