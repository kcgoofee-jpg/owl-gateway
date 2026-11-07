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
    let headers = req.headers().clone();
    let member = session::member_state(&state, &headers).await.ok().flatten();

    // 成员走 Discord 身份；游客用浏览器标记 owl_bid（没有就现种一个），两边都签同一套票
    let (identity, is_guest, cookie) = match member {
        Some((discord_id, _key)) => (discord_id, false, None),
        None => {
            let bid = headers
                .get(axum::http::header::COOKIE)
                .and_then(|v| v.to_str().ok())
                .and_then(|raw| {
                    raw.split(';')
                        .filter_map(|kv| kv.trim().split_once('='))
                        .find(|(k, _)| *k == super::public::BROWSER_COOKIE)
                        .map(|(_, v)| v.to_string())
                })
                .filter(|b| super::public::valid_browser_id(b))
                .unwrap_or_else(|| hex::encode(rand::random::<[u8; 16]>()));
            let cookie = crate::session::set_cookie_header(
                super::public::BROWSER_COOKIE,
                &bid,
                400 * 86400,
                true,
                "lax",
            );
            (format!("guest_{bid}"), true, Some(cookie))
        }
    };
    let discord_id = identity;
    // 昵称和头像由网关给，扑克侧不再回查
    let (username, display_name, avatar) = match state.db.discord_profile_row(&discord_id).await {
        Ok(Some(row)) => row,
        _ => (None, None, None),
    };
    let name = if is_guest {
        format!("游客{}", &discord_id[discord_id.len().saturating_sub(4)..])
    } else {
        display_name
            .filter(|s| !s.is_empty())
            .or(username)
            .unwrap_or_else(|| format!("玩家{}", &discord_id[discord_id.len().saturating_sub(4)..]))
    };
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
    let mut resp = ok_json(&json!({ "token": token, "exp": exp, "guest": is_guest }));
    if let Some(cookie) = cookie {
        resp.headers_mut()
            .append(axum::http::header::SET_COOKIE, cookie);
    }
    resp
}
