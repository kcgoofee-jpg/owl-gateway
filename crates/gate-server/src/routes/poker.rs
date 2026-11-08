//! 牌局（/poker）的单点登录：把网关的 Discord 会话换成一次性签名 token。
//!
//! 只对已登录成员签发，游客一律 401。token 形状 `discord_id:name:avatar:exp.hmac`，
//! 扑克服务拿同一个 `POKER_SSO_SECRET` 验签，验过就直接认这个人，不再有自己的注册流程。

use crate::error::ok_json;
use crate::session;
use crate::state::AppState;
use axum::Router;
use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use serde_json::json;

/// token 有效期：进了牌局就不再校验，够页面完成一次握手即可。
const TTL_SECONDS: i64 = 300;

pub fn router() -> Router<AppState> {
    Router::new().route("/poker/session", get(poker_session))
}

async fn poker_session(State(state): State<AppState>, req: Request) -> Response {
    if state.env.poker_sso_secret.is_empty() {
        return (StatusCode::SERVICE_UNAVAILABLE, "牌局未开启").into_response();
    }
    let Some((discord_id, _key)) = session::member_state(&state, req.headers())
        .await
        .ok()
        .flatten()
    else {
        return (StatusCode::UNAUTHORIZED, "请先用 Discord 登录").into_response();
    };
    // 昵称和头像由网关给，扑克侧不再回查
    let (username, display_name, avatar) = match state.db.discord_profile_row(&discord_id).await {
        Ok(Some(row)) => row,
        _ => (None, None, None),
    };
    let name = display_name
        .filter(|s| !s.is_empty())
        .or(username)
        .unwrap_or_else(|| format!("玩家{}", &discord_id[discord_id.len().saturating_sub(4)..]));
    let avatar = avatar.unwrap_or_default();
    let exp = state.now() as i64 + TTL_SECONDS;
    // 昵称里可能有冒号，payload 用 JSON 再转 hex，避免分隔符歧义
    let claims = serde_json::json!({
        "id": discord_id,
        "name": name,
        "avatar": avatar,
        "exp": exp,
    })
    .to_string();
    let payload = hex::encode(claims.as_bytes());
    let token = format!(
        "{payload}.{}",
        session::hmac_hex(&state.env.poker_sso_secret, &payload)
    );
    ok_json(&json!({ "token": token, "exp": exp }))
}
