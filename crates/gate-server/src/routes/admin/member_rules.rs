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
use serde::Deserialize;
use serde_json::{Value, json};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/admin/api/member-rules", get(defaults).put(set_defaults))
        .route(
            "/admin/api/members/{discord_id}/image-access",
            axum::routing::put(set_image_access),
        )
        .route(
            "/admin/api/members/{discord_id}/rules",
            get(member).put(set_member),
        )
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ImageAccess {
    enabled: bool,
}

async fn set_image_access(
    State(state): State<AppState>,
    session: AdminSession,
    AdminPath(subject): AdminPath<String>,
    request: Request,
) -> R {
    let (_, body) = json_of(request).await?;
    let access: ImageAccess = serde_json::from_value(Value::Object(body))
        .map_err(|_| HttpError::new(422, "生图权限格式无效"))?;
    if !gate_core::member_policy::is_discord_id(&subject) {
        return Err(HttpError::new(422, "成员身份无效").into());
    }
    if !state
        .db
        .set_owner_image_enabled(&subject, access.enabled)
        .await?
    {
        return Err(HttpError::new(403, "仅绑定的站长成员可使用此操作").into());
    }
    member(State(state), session, AdminPath(subject)).await
}

async fn defaults(State(state): State<AppState>, _session: AdminSession) -> R {
    Ok(ok_json(&json!({
        "defaults": state.db.member_defaults().await?,
        "fields": gate_core::member_policy::policy_fields(),
    })))
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
    let mut body = json!(state.db.member_policy(&subject).await?);
    if let Some(obj) = body.as_object_mut() {
        obj.insert("fields".into(), gate_core::member_policy::policy_fields());
    }
    Ok(ok_json(&body))
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
    if state
        .db
        .member_policy(&subject)
        .await?
        .owner_image
        .is_some_and(|p| !p.enabled)
    {
        return Err(
            HttpError::new(409, "普通测试状态下请修改默认规则，或先启用全部生图功能").into(),
        );
    }
    state.db.set_member_overrides(&subject, &overrides).await?;
    member(State(state), session, AdminPath(subject)).await
}
