//! 赞助席 `/patron/api/*`：独立密码、欢迎页和自己的功能开关。不进 `/admin`。

use crate::body::read_json_body;
use crate::error::{ApiError, HttpError, ok_json};
use crate::session::{
    ADMIN_SESSION_SECS, PATRON_COOKIE, PATRON_COOKIE_PATH, PatronSession,
    delete_cookie_header_path, hash_password, hit_login, make_patron_cookie, origin_ok,
    password_matches, set_cookie_header_path,
};
use crate::state::AppState;
use axum::Router;
use axum::extract::{Request, State};
use axum::http::HeaderMap;
use axum::response::Response;
use axum::routing::{get, post, put};
use gate_core::member_policy::{MemberOverrides, MemberPolicy};
use gate_core::policy::mask_token;
use gate_core::values::text;
use serde::Deserialize;
use serde_json::{Value, json};

const BODY_LIMIT: usize = 64 * 1024;
const WELCOME: &str =
    "感谢你的支持。这里只属于你的席位：可以打开自己 Key 的功能，插队在打开席位时就已经生效。";

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/patron/api/login", post(login))
        .route("/patron/api/logout", post(logout))
        .route("/patron/api/me", get(me))
        .route("/patron/api/me/features", put(set_features))
        .route("/patron/api/me/password", post(change_password))
}

fn field(body: &serde_json::Map<String, Value>, name: &str) -> String {
    body.get(name).map(text).unwrap_or_default()
}

async fn login(State(state): State<AppState>, req: Request) -> Result<Response, ApiError> {
    let (parts, body) = req.into_parts();
    if !origin_ok(&state, &parts.headers) {
        return Err(HttpError::new(403, "来源校验失败").into());
    }
    let ip = crate::auth::client_ip(&parts, &state.env.forwarded_allow_ips);
    if !hit_login(&state, &ip) {
        return Err(HttpError::new(429, "登录尝试过于频繁，请稍后再试").into());
    }
    let body = read_json_body(&parts.headers, body, BODY_LIMIT).await?;
    let password = field(&body, "password");
    let mut matched = None;
    for profile in state.db.patrons().await? {
        if profile.enabled
            && !profile.password_hash.is_empty()
            && password_matches(&password, &profile.password_hash)
        {
            matched = Some(profile);
            break;
        }
    }
    let Some(profile) = matched else {
        return Err(HttpError::new(401, "密码错误").into());
    };
    let mut resp = ok_json(&json!({"ok": true}));
    resp.headers_mut().append(
        "set-cookie",
        set_cookie_header_path(
            PATRON_COOKIE,
            &make_patron_cookie(&state, &profile.subject, &profile.password_hash),
            ADMIN_SESSION_SECS,
            state.env.admin_cookie_secure,
            "strict",
            PATRON_COOKIE_PATH,
        ),
    );
    Ok(resp)
}

async fn logout(
    _s: PatronSession,
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, HttpError> {
    if !origin_ok(&state, &headers) {
        return Err(HttpError::new(403, "来源校验失败"));
    }
    let mut resp = ok_json(&json!({"ok": true}));
    resp.headers_mut().append(
        "set-cookie",
        delete_cookie_header_path(
            PATRON_COOKIE,
            state.env.admin_cookie_secure,
            "strict",
            true,
            PATRON_COOKIE_PATH,
        ),
    );
    Ok(resp)
}

async fn me(State(state): State<AppState>, session: PatronSession) -> Result<Response, ApiError> {
    Ok(ok_json(&me_body(&state, &session).await?))
}

async fn me_body(state: &AppState, session: &PatronSession) -> Result<Value, ApiError> {
    let snapshot = state.db.member_policy(&session.subject).await?;
    let token = state
        .db
        .key_token_for_subject(&session.subject)
        .await?
        .unwrap_or_default();
    let exclusive = session
        .profile
        .bound_token_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty() && session.profile.enabled)
        .map(str::to_string);
    Ok(json!({
        "ok": true,
        "site_name": "猫头鹰公益站",
        "welcome": WELCOME,
        "queue_jump": session.profile.queue_jump,
        "self_service": session.profile.self_service,
        "exclusive_account": exclusive.is_some(),
        "account_hint": account_hint(state, exclusive.as_deref()).await,
        "key_fingerprint": if token.is_empty() { Value::Null } else { json!(mask_token(&token, 6)) },
        "features": feature_map(&snapshot.effective),
        "fields": gate_core::member_policy::policy_fields(),
    }))
}

async fn account_hint(state: &AppState, token_id: Option<&str>) -> Option<String> {
    let id = token_id?;
    let now = state.now();
    let tokens = state.nai.tokens().await;
    let token = tokens.iter().find(|t| t.token_id == id)?;
    let fields = token.fields();
    if !fields.admin_enabled || fields.disabled {
        return Some("专属账号已停用，请联系站长。".into());
    }
    if now < fields.blocked_until {
        return Some("专属账号正在冷却，请稍后再试。".into());
    }
    if now < fields.anlas_out_until {
        return Some("专属账号额度不足，请稍后再试。".into());
    }
    if !token.usable(now) {
        return Some("专属账号暂时不能出图，请稍后再试。".into());
    }
    None
}

fn feature_map(policy: &MemberPolicy) -> Value {
    json!({
        "image": policy.image,
        "v5": policy.v5,
        "vibe": policy.vibe,
        "tags": policy.tags,
        "img2img": policy.img2img,
        "upscale": policy.upscale,
        "augment": policy.augment,
        "paid_parameters": policy.paid_parameters,
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FeaturePatch {
    image: Option<bool>,
    v5: Option<bool>,
    vibe: Option<bool>,
    tags: Option<bool>,
    img2img: Option<bool>,
    upscale: Option<bool>,
    augment: Option<bool>,
    paid_parameters: Option<bool>,
}

async fn set_features(
    State(state): State<AppState>,
    session: PatronSession,
    req: Request,
) -> Result<Response, ApiError> {
    if !session.profile.self_service {
        return Err(HttpError::new(403, "此席位未开放自行改开关").into());
    }
    let (parts, body) = req.into_parts();
    let body = read_json_body(&parts.headers, body, BODY_LIMIT).await?;
    let patch: FeaturePatch = serde_json::from_value(Value::Object(body))
        .map_err(|_| HttpError::new(422, "功能开关格式无效"))?;
    let snapshot = state.db.member_policy(&session.subject).await?;
    if snapshot.owner_image.as_ref().is_some_and(|p| !p.enabled) {
        return Err(HttpError::new(409, "普通测试状态下请联系站长修改默认规则").into());
    }
    let mut next = snapshot.effective.clone();
    apply_feature(&mut next.image, patch.image);
    apply_feature(&mut next.v5, patch.v5);
    apply_feature(&mut next.vibe, patch.vibe);
    apply_feature(&mut next.tags, patch.tags);
    apply_feature(&mut next.img2img, patch.img2img);
    apply_feature(&mut next.upscale, patch.upscale);
    apply_feature(&mut next.augment, patch.augment);
    apply_feature(&mut next.paid_parameters, patch.paid_parameters);
    let overrides: MemberOverrides = snapshot.defaults.differences(&next);
    snapshot
        .defaults
        .resolve(&overrides)
        .map_err(|message| HttpError::new(422, message))?;
    if !state
        .db
        .set_patron_overrides(session.profile.clone(), snapshot, overrides)
        .await?
    {
        return Err(HttpError::new(409, "席位或成员规则已变更，请刷新后再试").into());
    }
    me(State(state), session).await
}

fn apply_feature(slot: &mut bool, value: Option<bool>) {
    if let Some(value) = value {
        *slot = value;
    }
}

async fn change_password(
    State(state): State<AppState>,
    session: PatronSession,
    req: Request,
) -> Result<Response, ApiError> {
    let (parts, body) = req.into_parts();
    let ip = crate::auth::client_ip(&parts, &state.env.forwarded_allow_ips);
    if !hit_login(&state, &ip) {
        return Err(HttpError::new(429, "尝试过于频繁，请稍后再试").into());
    }
    let body = read_json_body(&parts.headers, body, BODY_LIMIT).await?;
    let (current, new) = (field(&body, "current"), field(&body, "new"));
    if !password_matches(&current, &session.profile.password_hash) {
        return Err(HttpError::new(401, "当前密码不正确").into());
    }
    if new.chars().count() < 12 || new == current {
        return Err(HttpError::new(422, "新密码至少 12 个字符，且不能与当前密码相同").into());
    }
    let mut profile = session.profile.clone();
    profile.password_hash = hash_password(&new);
    state
        .db
        .save_patron(Some(session.profile), profile)
        .await?
        .map_err(|e| HttpError::new(409, e.to_string()))?;
    let mut resp = ok_json(&json!({"ok": true, "relogin": true}));
    resp.headers_mut().append(
        "set-cookie",
        delete_cookie_header_path(
            PATRON_COOKIE,
            state.env.admin_cookie_secure,
            "strict",
            true,
            PATRON_COOKIE_PATH,
        ),
    );
    Ok(resp)
}
