//! 成员网页接口：Discord 登录、个人状态与重置券。会话 Cookie 的签发 / 校验见 `crate::session`。

use super::registration::{internal, log_discord, no_store};
use crate::action_log::log_action;
use crate::auth::client_ip;
use crate::error::{ApiResult, HttpError, ok_json};
use crate::registration::{RegErr, RoleCheck};
use crate::session::{
    LoginMode, MEMBER_COOKIE, MEMBER_SESSION_DAYS, STATE_COOKIE, cookie, ct_eq,
    delete_cookie_header, key_tag, member_state, set_cookie_header, sign_login_state, sign_member,
    verify_login_state,
};
use crate::state::AppState;
use axum::Router;
use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::Response;
use axum::routing::{get, post};
use gate_core::pyval;
use serde_json::json;
use std::collections::HashMap;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/login", get(login_start))
        .route("/login/callback", get(login_callback))
        .route("/logout", post(logout))
        .route("/public/me", get(public_me))
        .route("/public/me/requests", get(my_requests))
        .route("/public/me/coupons/{coupon_id}/use", post(use_my_coupon))
}

/// `RedirectResponse(url)`：307，没有响应体。
fn redirect(location: &str) -> Response {
    let mut resp = Response::new(Body::empty());
    *resp.status_mut() = StatusCode::TEMPORARY_REDIRECT;
    if let Ok(v) = HeaderValue::from_str(location) {
        resp.headers_mut().insert("location", v);
    }
    resp.headers_mut()
        .insert("content-length", HeaderValue::from_static("0"));
    resp
}

/// 只保留十六进制、最多 32 位（前端设备指纹是 sha256 截断的十六进制）；其它一律当没有。
fn sanitize_dev(raw: Option<&String>) -> String {
    raw.map(|s| {
        s.chars()
            .filter(|c| c.is_ascii_hexdigit())
            .take(32)
            .flat_map(|c| c.to_lowercase())
            .collect::<String>()
    })
    .unwrap_or_default()
}

/// 网页登录相关的操作记录：操作者写「昵称 (@用户名)」，对象写 Discord ID（后台一眼看出是谁，ID 照样能搜）。
async fn log_login(
    state: &AppState,
    who: &crate::registration::WebIdentity,
    action: &str,
    detail: &str,
) {
    let user = crate::registration::DiscordUser {
        id: who.id.clone(),
        username: who.username.clone(),
        global_name: who.global_name.clone(),
        avatar: who.avatar.clone(),
    };
    log_action(
        &state.db,
        &crate::registration::member_label(&user),
        action,
        &format!("Discord:{}", who.id),
        detail,
        true,
        state.now(),
    )
    .await;
}

// ------------------------------------------------------------------ 用 Discord 登录

/// 身份组门槛配齐（后台「领 Key」的服务器和身份组都填了）时，所有网页登录都走验证模式：
/// 授权页一次申请 `identify guilds.members.read`，回调里就地核验身份组；没配门槛 = 普通登录（只要 identify）。
/// `/login?verify=1` 仍然有效（旧链接、已发出去的提示），行为同「门槛配齐」。
async fn login_start(
    State(state): State<AppState>,
    Query(q): Query<HashMap<String, String>>,
) -> ApiResult {
    let Some(svc) = state.registration.as_deref() else {
        return Err(HttpError::new(503, "登录尚未配置").into());
    };
    use base64::Engine;
    let nonce = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(rand::random::<[u8; 24]>());
    let mode = if q.get("verify").is_some_and(|v| v == "1") || svc.role_gate_configured().await? {
        LoginMode::Verify
    } else {
        LoginMode::Plain
    };
    // 浏览器设备指纹（POST /public/device-fp 时服务端算出并回给前端的十六进制短哈希，可选）：只留十六进制、最多 32 位；跟着签名 state 走一圈
    let dev = sanitize_dev(q.get("dev"));
    // state 自带用途、过期时间、设备指纹和签名；Cookie 里存同一份，两条路任意一条对得上都算数
    let st = sign_login_state(&state.secret_key, mode, &nonce, &dev, state.now() as i64);
    let mut resp = redirect(&svc.web_login_url(&st, mode == LoginMode::Verify));
    resp.headers_mut().append(
        "set-cookie",
        set_cookie_header(STATE_COOKIE, &st, 600, true, "lax"),
    );
    Ok(resp)
}

/// 登录时只认这么多秒内上报的电量快照（指纹在打开首页时就算好了，一般几十秒内就点登录）。
const BATTERY_FRESH_SECS: f64 = 600.0;

async fn login_callback(
    State(state): State<AppState>,
    Query(q): Query<HashMap<String, String>>,
    parts: axum::http::request::Parts,
) -> ApiResult {
    let Some(svc) = state.registration.as_deref() else {
        return Err(HttpError::new(503, "登录尚未配置").into());
    };
    let code = q.get("code").map_or("", String::as_str);
    let given = q.get("state").map_or("", String::as_str);
    let expected = cookie(&parts.headers, STATE_COOKIE).unwrap_or_default();
    let same = ct_eq(given.as_bytes(), expected.as_bytes());
    // 签名 state 不依赖 Cookie（Discord 内置浏览器常丢 Cookie）；它的用途只信签名里的。
    // 只有 Cookie 对得上的旧式 state 一律当普通登录，不可能借它进入验证模式。
    let signed = verify_login_state(&state.secret_key, given, state.now() as i64);
    let dev = signed.as_ref().map(|(_, d)| d.clone()).unwrap_or_default();
    let mode = match &signed {
        Some((m, _)) => Some(*m),
        None if same && !given.is_empty() => Some(LoginMode::Plain),
        None => None,
    };
    let Some(mode) = mode.filter(|_| !code.is_empty()) else {
        // error=access_denied（成员点了取消）/ state 过期或被改都会走到这里
        tracing::info!(
            "[web-login] callback rejected: code={} state_ok={} err={}",
            if code.is_empty() { 'n' } else { 'y' },
            if signed.is_some() || same { 'y' } else { 'n' },
            gate_core::pyc::take_chars(q.get("error").map_or("", String::as_str), 40)
        );
        return Ok(redirect("/?login=failed"));
    };
    let verify = mode == LoginMode::Verify;
    let who = match svc.web_identify(code, verify).await {
        Ok(w) => w,
        Err(RegErr::Denied(_)) => return Ok(redirect("/?login=failed")),
        Err(RegErr::Internal(m)) => return Err(internal(m)),
    };
    // 记一次网页登录：web_logins 永久保留，「每位成员到网页登录一次」开关按它放行；
    // 设备指纹状态：没带 = none；带了但服务端没算过（不是 /public/device-fp 发的）= unknown，疑似伪造；否则 ok。
    // 只有 ok 的指纹记进 device_logins（同一个设备指纹下登录了多个 Discord 账号 = 疑似小号）。
    // 尽力而为，失败不影响登录。
    {
        let ip = client_ip(&parts, &state.env.forwarded_allow_ips);
        let now = state.now();
        let fp = if dev.is_empty() {
            "none"
        } else {
            match state.db.has_device_fingerprint(&dev).await {
                Ok(true) => "ok",
                Ok(false) => "unknown",
                Err(e) => {
                    // 查不了就不扣「疑似伪造」的帽子
                    tracing::warn!(target: "web-login", "查设备指纹失败：{e}");
                    "ok"
                }
            }
        };
        if let Err(e) = state.db.record_web_login(&who.id, fp, &ip, now).await {
            tracing::warn!(target: "web-login", "记网页登录失败：{e}");
        }
        if fp == "ok"
            && let Err(e) = state.db.record_device_login(&who.id, &dev, &ip, now).await
        {
            tracing::warn!(target: "web-login", "记设备登录失败：{e}");
        }
        // 浏览器标记（/public/device-fp 种下的 owl_bid Cookie）：同一个浏览器登过多个账号 = 疑似小号
        let bid = cookie(&parts.headers, super::public::BROWSER_COOKIE)
            .filter(|b| super::public::valid_browser_id(b));
        if let Some(bid) = &bid
            && let Err(e) = state.db.record_browser_login(&who.id, bid, &ip, now).await
        {
            tracing::warn!(target: "web-login", "记浏览器登录失败：{e}");
        }
        // 电量快照（站长 10-23）：这台设备 + 这个浏览器 10 分钟内上报过电量才记（登录前刚算的那份）
        if fp == "ok"
            && let Some(bid) = &bid
            && let Err(e) = state
                .db
                .record_login_battery(&who.id, &dev, bid, now - BATTERY_FRESH_SECS)
                .await
        {
            tracing::warn!(target: "web-login", "记电量快照失败：{e}");
        }
        // 所在服务器 / 绑定的外部账号（scope guilds / connections；站长 10-23 防一人多号 / 共用 Key）
        if let Some(g) = &who.guilds
            && let Err(e) = state.db.record_discord_guilds(&who.id, g, now).await
        {
            tracing::warn!(target: "web-login", "记服务器列表失败：{e}");
        }
        if let Some(c) = &who.connections
            && let Err(e) = state.db.record_discord_connections(&who.id, c, now).await
        {
            tracing::warn!(target: "web-login", "记外部账号失败：{e}");
        }
        // 和封禁号同浏览器 / 同设备：进待审并记一条「注意」级 Bug（领 Key 时另外现查、拦下）
        crate::ban_link::on_login(&state, &who.id).await;
    }
    // 已有 Key 的成员无论身份组结果如何都要能进自己的额度页；没有 Key 的人在这里被身份组门槛挡住：
    // 不发会话、不记任何东西，只带着原因回首页（/register 那边还会再查一次核验记录）。
    let key = svc.key_row_for(&who.id).await?;
    let to = match (who.role, key.is_some()) {
        (None | Some(RoleCheck::NotRequired), _) => "/",
        (Some(RoleCheck::Verified), true) => "/",
        (Some(RoleCheck::Verified), false) => "/?verify=ok",
        (Some(RoleCheck::Missing), true) | (Some(RoleCheck::Unavailable), true) => "/",
        (Some(RoleCheck::Missing), false) => "/?verify=no",
        (Some(RoleCheck::Unavailable), false) => "/?verify=error",
    };
    let detail = match who.role {
        Some(RoleCheck::Verified) => "已核验",
        Some(RoleCheck::Missing) => "没有身份组",
        Some(RoleCheck::Unavailable) => "Discord 无法核验",
        _ => "",
    };
    let blocked =
        key.is_none() && matches!(who.role, Some(RoleCheck::Missing | RoleCheck::Unavailable));
    let mut resp = redirect(to);
    let clear_state = delete_cookie_header(STATE_COOKIE, false, "lax", false);
    if blocked {
        resp.headers_mut().append("set-cookie", clear_state);
        log_login(&state, &who, "网页登录被身份组门槛挡下", detail).await;
        return Ok(resp);
    }
    // 名字和头像：没领过 Key 的人首页也要认得出自己（领过的以注册记录为准）。存不了不影响登录。
    let shown = if who.global_name.is_empty() {
        &who.username
    } else {
        &who.global_name
    };
    if let Err(e) = state
        .db
        .upsert_web_profile(&who.id, &who.username, shown, &who.avatar, state.now())
        .await
    {
        tracing::warn!("[web-login] 记录登录资料失败：{e}");
    }
    let exp = state.now() as i64 + MEMBER_SESSION_DAYS * 86400;
    let tag = key_tag(key.as_ref());
    resp.headers_mut().append(
        "set-cookie",
        set_cookie_header(
            MEMBER_COOKIE,
            &sign_member(&state, &who.id, exp, &tag),
            MEMBER_SESSION_DAYS * 86400,
            true,
            "lax",
        ),
    );
    resp.headers_mut().append("set-cookie", clear_state);
    log_login(
        &state,
        &who,
        if verify {
            "验证身份组"
        } else {
            "网页登录"
        },
        detail,
    )
    .await;
    Ok(resp)
}

async fn logout() -> Response {
    let mut resp = ok_json(&json!({"ok": true}));
    resp.headers_mut().append(
        "set-cookie",
        delete_cookie_header(MEMBER_COOKIE, false, "lax", false),
    );
    resp
}

// ------------------------------------------------------------------ 个人状态 / 重置券

/// 登录后的个人状态：不用粘贴 Key 就能看额度、排队、到期；并提供自己的 Key 供一键复制。
/// 内容由首页首屏用的同一份 [`super::public::me_payload`] 生成，不另写一份。
async fn public_me(State(state): State<AppState>, headers: HeaderMap) -> ApiResult {
    let Some((discord_id, key)) = member_state(&state, &headers).await? else {
        return Ok(no_store(ok_json(&json!({"logged_in": false}))));
    };
    let me = super::public::me_payload(&state, &discord_id, key.as_ref()).await?;
    Ok(no_store(ok_json(&me)))
}

/// 「我的请求」每页条数。
const MY_REQUESTS_PAGE: i64 = 50;

/// 成员自己上次重置（服务日起点）以来的请求（首页「我的请求」）：`?before=<id>` 翻页，`?status=ok|rejected|error` 筛选。
/// 只回成员自己能看的东西：不含 IP、客户端、上游账号；站付的 Anlas 不算成员花的。
async fn my_requests(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<HashMap<String, String>>,
) -> ApiResult {
    let Some((_, key)) = member_state(&state, &headers).await? else {
        return Err(HttpError::new(401, "未登录或登录已过期，请刷新页面后重新登录。").into());
    };
    let Some(key) = key else {
        return Err(HttpError::new(404, "你还没有 Key").into());
    };
    let before = q
        .get("before")
        .and_then(|v| v.trim().parse::<i64>().ok())
        .unwrap_or(0)
        .max(0);
    let status = q
        .get("status")
        .map(String::as_str)
        .filter(|s| matches!(*s, "ok" | "rejected" | "error"))
        .unwrap_or_default();
    let since = state.clock.day_start(state.now());
    let rows = state
        .db
        .member_logs(key.id(), since, before, status, MY_REQUESTS_PAGE + 1)
        .await?;
    let more = rows.len() as i64 > MY_REQUESTS_PAGE;
    let items: Vec<serde_json::Value> = rows
        .iter()
        .take(MY_REQUESTS_PAGE as usize)
        .map(my_request_item)
        .collect();
    let next = more
        .then(|| items.last().and_then(|r| r["id"].as_i64()))
        .flatten();
    let summary = state.db.member_log_summary(key.id(), since).await?;
    Ok(no_store(ok_json(&json!({
        "items": items,
        "next": next,
        "summary": summary,
        "since": since,
    }))))
}

/// 一行请求日志 → 成员看到的一条（`rid` = 请求编号，与错误响应的 request_id / x-request-id、后台请求日志的「编号」同一个）：拒绝 / 失败的说明去掉开头的状态码另放；站付的图成员花的 Anlas 记 0。
fn my_request_item(r: &serde_json::Value) -> serde_json::Value {
    let detail = r["detail"].as_str().unwrap_or("");
    let (code, message) = match detail.split_once(' ') {
        Some((c, rest)) if c.len() == 3 && c.bytes().all(|b| b.is_ascii_digit()) => {
            (c.parse::<i64>().ok(), rest)
        }
        _ => (None, detail),
    };
    let site_paid = detail.contains(gate_core::paid_v5::MARKER);
    let anlas = if site_paid {
        0.0
    } else {
        r["anlas"].as_f64().unwrap_or(0.0)
    };
    let secs = |k: &str| {
        r[k].as_i64()
            .filter(|ms| *ms > 0)
            .map(|ms| ms as f64 / 1000.0)
    };
    json!({
        "id": r["id"],
        "rid": r["rid"],
        "ts": r["ts"],
        "kind": r["kind"],
        "model": r["model"],
        "status": r["status"],
        "images": r["images"],
        "anlas": anlas,
        "site_paid": site_paid,
        "wait_s": secs("wait_ms"),
        "dur_s": secs("dur_ms"),
        "reason": r["reason"],
        "code": code,
        "message": message,
        "delivery_state": r["delivery_state"].as_str().unwrap_or("unknown"),
    })
}

/// FastAPI 的路径参数 `coupon_id: int` 解析失败。
fn int_path_error(raw: &str) -> HttpError {
    HttpError {
        status: 422,
        detail: json!([{
            "type": "int_parsing", "loc": ["path", "coupon_id"],
            "msg": "Input should be a valid integer, unable to parse string as an integer",
            "input": raw,
        }]),
    }
}

/// 成员在首页用掉一张重置券：重置券上写明的几项（默认只有 V4.5；V5 / Anlas 要发券时勾选），和站长后台「重置今日额度」一样。
async fn use_my_coupon(
    State(state): State<AppState>,
    Path(coupon_id): Path<String>,
    headers: HeaderMap,
) -> ApiResult {
    let coupon_id: i64 = coupon_id
        .trim()
        .parse()
        .map_err(|_| int_path_error(&coupon_id))?;
    let Some((discord_id, key)) = member_state(&state, &headers).await? else {
        return Err(HttpError::new(401, "未登录或登录已过期，请刷新页面后重新登录。").into());
    };
    let Some(key) = key.filter(|k| k.enabled()) else {
        return Err(HttpError::new(404, "你还没有可用的 Key").into());
    };
    let now = state.now();
    let Some(c) = state.db.use_coupon(coupon_id, key.id(), now).await? else {
        return Err(HttpError::new(409, "该券已使用或已过期，额度未变更。").into());
    };
    let scope = gate_db::repo::counters::ResetScope::parse(
        c.get("scope").and_then(|v| v.as_str()).unwrap_or("v45"),
    );
    state
        .db
        .reset_daily_image_quota(key.id(), &state.day(), scope)
        .await?;
    let note = c
        .get("note")
        .filter(|n| pyval::truthy(n))
        .map(gate_core::pyext::py_str);
    log_discord(
        &state,
        &discord_id,
        "成员使用重置券",
        key.name(),
        &format!(
            "券 #{coupon_id}{}",
            note.map(|n| format!("（{n}）")).unwrap_or_default()
        ),
    )
    .await;
    Ok(no_store(ok_json(&json!({"ok": true}))))
}
