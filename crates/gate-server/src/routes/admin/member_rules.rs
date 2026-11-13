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
            "/admin/api/members/{discord_id}/patron",
            axum::routing::put(set_patron),
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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PatronSeat {
    enabled: bool,
    queue_jump: Option<bool>,
    self_service: Option<bool>,
    bound_token_id: Option<String>,
    portal_mode: Option<String>,
    #[serde(default)]
    reset_password: bool,
}

async fn set_patron(
    State(state): State<AppState>,
    _session: AdminSession,
    AdminPath(subject): AdminPath<String>,
    request: Request,
) -> R {
    let (_, body) = json_of(request).await?;
    let seat: PatronSeat = serde_json::from_value(Value::Object(body))
        .map_err(|_| HttpError::new(422, "赞助席格式无效"))?;
    if !gate_core::member_policy::is_discord_id(&subject) {
        return Err(HttpError::new(422, "成员身份无效").into());
    }
    let bound = seat
        .bound_token_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    if let Some(id) = bound.as_deref() {
        let known = state.nai.tokens().await.iter().any(|t| t.token_id == id);
        if !known {
            return Err(HttpError::new(422, "上游 Token 不存在").into());
        }
    }
    let existing = state.db.patron(&subject).await?;
    let portal_mode = seat.portal_mode.unwrap_or_else(|| {
        existing
            .as_ref()
            .map(|p| p.portal_mode.clone())
            .unwrap_or_else(|| "basic".into())
    });
    if !gate_db::repo::patron::PORTAL_MODES.contains(&portal_mode.as_str()) {
        return Err(HttpError::new(422, "门户模式只能是 basic 或 full").into());
    }
    let mut password = None;
    let mut password_hash = existing
        .as_ref()
        .map(|p| p.password_hash.clone())
        .unwrap_or_default();
    if seat.enabled && (seat.reset_password || password_hash.is_empty()) {
        let generated = crate::session::generate_portal_password();
        password_hash = crate::session::hash_password(&generated);
        password = Some(generated);
    }
    state
        .db
        .save_patron(
            existing,
            gate_db::repo::patron::PatronRecord {
                subject: subject.clone(),
                enabled: seat.enabled,
                queue_jump: seat.queue_jump.unwrap_or(true),
                self_service: seat.self_service.unwrap_or(true),
                bound_token_id: bound,
                password_hash,
                portal_mode,
            },
        )
        .await?
        .map_err(|e| HttpError::new(409, e.to_string()))?;
    state.refresh_exclusive_tokens().await;
    let mut body = json!(state.db.member_policy(&subject).await?);
    if let Some(obj) = body.as_object_mut() {
        obj.insert("fields".into(), gate_core::member_policy::policy_fields());
        if let Some(password) = password {
            obj.insert("portal_password".into(), json!(password));
        }
    }
    Ok(ok_json(&body))
}
