//! 默认规则与成员例外；请求字段与领域类型一致。
use super::req::{AdminPath, R, json_of};
use crate::error::{HttpError, ok_json};
use crate::session::AdminSession;
use crate::state::AppState;
use axum::{
    Router,
    extract::{Request, State},
    routing::get,
};
use gate_core::member_policy::{MemberOverrides, MemberPolicy};
use serde_json::{Value, json};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/admin/api/member-rules", get(defaults).put(set_defaults))
        .route(
            "/admin/api/members/{discord_id}/rules",
            get(member).put(set_member),
        )
}

async fn defaults(State(state): State<AppState>, _session: AdminSession) -> R {
    Ok(ok_json(
        &json!({"defaults":state.db.member_defaults().await?}),
    ))
}

async fn set_defaults(State(state): State<AppState>, session: AdminSession, request: Request) -> R {
    let (_, body) = json_of(request).await?;
    let policy: MemberPolicy = serde_json::from_value(Value::Object(body))
        .map_err(|_| HttpError::new(422, "默认规则格式无效"))?;
    policy
        .validate()
        .map_err(|message| HttpError::new(422, message))?;
    state.db.set_member_defaults(&policy).await?;
    defaults(State(state), session).await
}

async fn member(
    State(state): State<AppState>,
    _session: AdminSession,
    AdminPath(subject): AdminPath<String>,
) -> R {
    if !gate_core::member_policy::is_discord_id(&subject) {
        return Err(HttpError::new(422, "Discord ID 无效").into());
    }
    Ok(ok_json(&json!(state.db.member_policy(&subject).await?)))
}

async fn set_member(
    State(state): State<AppState>,
    session: AdminSession,
    AdminPath(subject): AdminPath<String>,
    request: Request,
) -> R {
    let (_, body) = json_of(request).await?;
    let overrides: MemberOverrides = serde_json::from_value(Value::Object(body))
        .map_err(|_| HttpError::new(422, "成员例外格式无效"))?;
    state
        .db
        .member_defaults()
        .await?
        .resolve(&overrides)
        .map_err(|message| HttpError::new(422, message))?;
    if !gate_core::member_policy::is_discord_id(&subject) {
        return Err(HttpError::new(422, "Discord ID 无效").into());
    }
    state.db.set_member_overrides(&subject, &overrides).await?;
    member(State(state), session, AdminPath(subject)).await
}
