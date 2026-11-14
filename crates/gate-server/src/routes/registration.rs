//! 机器人桥接 `/self-register/*`。
//!
//! 鉴权：`Authorization: Bearer <REGISTRATION_BRIDGE_SECRET>`；成员网页接口见 `member.rs`。
//! 检查次序固定：先校验请求体（422），再鉴权（503 / 401），再校验服务器（403）。
//! 机器人只有成员命令（/register /resetkey /反馈；/quota、/help 10-23 删除）；管理员命令 `ops` / `slots` / `revoke` 已删除（管理都在网页后台）。

use crate::action_log::log_action;
use crate::auth::{client_ip, header_str};
use crate::error::{ApiError, ApiResult, HttpError, InternalError, ok_json};
use crate::multipart::read_bounded_body;
use crate::registration::{DiscordUser, RegErr, RegistrationService};
use crate::session::ct_eq;
use crate::state::AppState;
use axum::Router;
use axum::extract::{ConnectInfo, Request, State};
use axum::http::HeaderValue;
use axum::http::request::Parts;
use axum::response::Response;
use axum::routing::{get, post};
use gate_core::bot_config;
use gate_core::primitives::take_chars;
use serde_json::{Map, Value, json};
use std::net::SocketAddr;
use tracing::info;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/self-register/resetkey", post(resetkey))
        .route("/self-register/issue", post(issue))
        .route("/self-register/bot/report", post(bot_report))
        .route("/self-register/bot/stats", get(bot_stats))
        .route("/self-register/bot/context", post(bot_context))
        .route("/self-register/bot/share", post(bot_share))
        .route("/self-register/bot/client", post(bot_client))
}

/// 桥接 / 成员页接口的请求体上限（正常请求都在几百字节）。
pub(super) const BODY_LIMIT: usize = 1024 * 1024;

/// 一个已解析的 JSON 对象请求体 + 字段校验的错误清单（`finish` 时有错误就回 422）。
pub(super) struct Model {
    body: Value,
    errors: Vec<String>,
}

impl Model {
    pub(super) fn parse(raw: &[u8]) -> Result<Self, HttpError> {
        let body = gate_core::json_body::parse_json_body(raw)
            .map_err(|e| HttpError::new(422, e.message()))?;
        Ok(Self {
            body: Value::Object(body),
            errors: Vec::new(),
        })
    }

    fn fail(&mut self, _kind: &str, loc: Vec<&str>, msg: &str, _input: &Value) {
        self.errors.push(format!("{}: {msg}", loc.join(".")));
    }

    fn get(&self, name: &str) -> Option<Value> {
        self.body.get(name).cloned()
    }

    /// `name: str`（`default=None` 为必填）。
    pub(super) fn string(&mut self, name: &str, default: Option<&str>) -> String {
        match self.get(name) {
            Some(Value::String(s)) => s,
            Some(other) => {
                self.fail(
                    "string_type",
                    vec![name],
                    "Input should be a valid string",
                    &other,
                );
                String::new()
            }
            None => {
                if default.is_none() {
                    let whole = self.body.clone();
                    self.fail("missing", vec![name], "Field required", &whole);
                }
                default.unwrap_or("").to_string()
            }
        }
    }

    fn number(&mut self, name: &str) -> f64 {
        match self.get(name) {
            Some(Value::Number(n)) => n.as_f64().unwrap_or_else(|| {
                self.fail(
                    "float_type",
                    vec![name],
                    "Input should be a valid number",
                    &Value::Null,
                );
                0.0
            }),
            Some(other) => {
                self.fail(
                    "float_type",
                    vec![name],
                    "Input should be a valid number",
                    &other,
                );
                0.0
            }
            None => {
                let whole = self.body.clone();
                self.fail("missing", vec![name], "Field required", &whole);
                0.0
            }
        }
    }

    fn opt_dict(&mut self, name: &str) -> Option<Map<String, Value>> {
        match self.get(name) {
            Some(Value::Object(m)) => Some(m),
            Some(Value::Null) | None => None,
            Some(other) => {
                self.fail(
                    "dict_type",
                    vec![name],
                    "Input should be a valid dictionary",
                    &other,
                );
                None
            }
        }
    }

    pub(super) fn finish(self) -> Result<(), HttpError> {
        if self.errors.is_empty() {
            Ok(())
        } else {
            Err(HttpError::new(422, self.errors.join("; ")))
        }
    }
}

/// 读请求体并交给 [`Model::parse`]。
pub(super) async fn read_model(parts: &Parts, body: axum::body::Body) -> Result<Model, HttpError> {
    let raw = read_bounded_body(&parts.headers, body, BODY_LIMIT).await?;
    Model::parse(&raw)
}

// ------------------------------------------------------------------ 公共小件

struct Who {
    discord_id: String,
    guild_id: String,
}

impl Who {
    fn read(m: &mut Model) -> Self {
        Self {
            discord_id: m.string("discord_id", None),
            guild_id: m.string("guild_id", None),
        }
    }
}

/// `_service(request)`：服务没配置 503；桥接密钥不对 401（`record_failure` 时也计入无效请求限流）。
fn authorize<'a>(
    state: &'a AppState,
    parts: &Parts,
    record_failure: bool,
) -> Result<&'a RegistrationService, HttpError> {
    let Some(svc) = state.registration.as_deref() else {
        return Err(HttpError::new(503, "自助领 Key 尚未配置"));
    };
    let given = header_str(&parts.headers, "authorization");
    let want = format!("Bearer {}", svc.bridge_secret());
    if !ct_eq(given.as_bytes(), want.as_bytes()) {
        // 猜桥接密钥也计入无效请求限流（拿不到对端地址时不记）
        if record_failure && parts.extensions.get::<ConnectInfo<SocketAddr>>().is_some() {
            let ip = client_ip(parts, &state.env.forwarded_allow_ips);
            crate::auth::record_auth_failure(state, &ip, state.now());
        }
        return Err(HttpError::new(401, "未授权"));
    }
    Ok(svc)
}

/// `_checked`：只认指定服务器；发命令的人要是纯数字 ID。
fn checked<'a>(svc: &RegistrationService, who: &'a Who) -> Result<&'a str, HttpError> {
    if who.guild_id != svc.command_guild() || !gate_core::validation::is_decimal(&who.discord_id) {
        return Err(HttpError::new(
            403,
            "请在猫头鹰公益站的 Discord 服务器里使用这个命令。",
        ));
    }
    Ok(&who.discord_id)
}

pub(super) fn no_store(mut resp: Response) -> Response {
    resp.headers_mut()
        .insert("cache-control", HeaderValue::from_static("no-store"));
    resp
}

pub(super) async fn log_discord(
    state: &AppState,
    who: &str,
    action: &str,
    target: &str,
    detail: &str,
) {
    log_action(
        &state.db,
        &format!("Discord:{who}"),
        action,
        target,
        detail,
        true,
        state.now(),
    )
    .await;
}

pub(super) fn internal(e: impl std::fmt::Display) -> ApiError {
    InternalError(e.to_string()).into()
}

// ------------------------------------------------------------------ 接口

async fn resetkey(State(state): State<AppState>, req: Request) -> ApiResult {
    let (parts, body) = req.into_parts();
    let mut m = read_model(&parts, body).await?;
    let who = Who::read(&mut m);
    m.finish()?;
    let svc = authorize(&state, &parts, true)?;
    let discord_id = checked(svc, &who)?;
    let Some(key) = svc.key_row_for(discord_id).await? else {
        return Err(HttpError::new(404, "你还没有领取 Key，请先使用 /register。").into());
    };
    let token = gate_core::policy::gen_key("nai", &rand::random::<[u8; 24]>());
    state.db.rotate_key_token(key.id(), &token).await?;
    log_discord(
        &state,
        &who.discord_id,
        "成员重置 Key（/resetkey）",
        &format!("Key #{} {}", key.id(), key.name()),
        "",
    )
    .await;
    Ok(no_store(ok_json(&json!({"key": token}))))
}

/// /register 直发 Key：斜杠命令已验明身份，直接发 Key（不走 OAuth、不私信），临时消息里回给本人。
/// 鉴权：桥接密钥。这是唯一的发 Key 路径（旧的 /intent + /callback OAuth 领取已删除）。
async fn issue(State(state): State<AppState>, req: Request) -> ApiResult {
    let (parts, body) = req.into_parts();
    let mut m = read_model(&parts, body).await?;
    let discord_id = m.string("discord_id", None);
    let guild_id = m.string("guild_id", None);
    let username = m.string("username", Some(""));
    let global_name = m.string("global_name", Some(""));
    let avatar = m.string("avatar", Some(""));
    let invite_key = m.string("invite_key", Some(""));
    m.finish()?;
    // 这条接口历来不把猜密钥计入无效请求限流
    let svc = authorize(&state, &parts, false)?;
    let user = DiscordUser {
        id: discord_id.clone(),
        username: take_chars(&username, 80).to_string(),
        global_name: take_chars(&global_name, 80).to_string(),
        avatar: take_chars(&avatar, 120).to_string(),
    };
    if !invite_key.is_empty() {
        return Err(HttpError::new(422, "邀请 Key 领取已下线，请直接使用 /register。").into());
    }
    let result = match svc.issue_direct(&discord_id, &guild_id, &user).await {
        Ok(r) => r,
        Err(RegErr::Denied(msg)) => {
            log_discord(
                &state,
                &discord_id,
                "领取 Key 失败（/register）",
                "",
                take_chars(&msg, 200),
            )
            .await;
            return Err(HttpError::new(403, msg).into());
        }
        Err(RegErr::Internal(msg)) => return Err(internal(msg)),
    };
    log_discord(&state, &discord_id, "领取 Key（/register 直发）", "", "").await;
    Ok(no_store(ok_json(
        &json!({"key": result.key, "message": result.message}),
    )))
}

/// 机器人每分钟上报一次心跳，后台「机器人状态」据此显示在线 / 离线。
/// 3.0 起只收 `status`（`event`、`ai_ready` 之类旧字段会被忽略）。
/// 奶妹 bot 拉站点运行摘要：排队数、今日（7 点边界）出图张数、近一小时排队等待。
/// 只读；桥接密钥鉴权；每次查询打一行结构化日志（审计落库版本跟 ops 重构走）。
async fn bot_stats(State(state): State<AppState>, parts: Parts) -> ApiResult {
    authorize(&state, &parts, false)?;
    let now = state.now();
    let day = state
        .db
        .image_stats_since(state.clock.day_start(now))
        .await?;
    let hour = state.db.image_stats_since(now - 3600.0).await?;
    let waiting = state
        .global_waiting
        .load(std::sync::atomic::Ordering::Relaxed);
    info!(
        "[bot-stats] waiting={waiting} images_today={} ok_1h={}",
        day.images, hour.ok_count
    );
    Ok(ok_json(&json!({
        "queue_waiting": waiting,
        "images_today": day.images,
        "ok_requests_today": day.ok_count,
        "avg_wait_ms_1h": hour.avg_wait_ms,
        "max_wait_ms_1h": hour.max_wait_ms,
        "ok_requests_1h": hour.ok_count,
        "server_time": now,
    })))
}

/// Read-only member facts for the authenticated bot. No credentials or other members' records.
async fn bot_context(State(state): State<AppState>, req: Request) -> ApiResult {
    let (parts, body) = req.into_parts();
    let mut m = read_model(&parts, body).await?;
    let who = Who::read(&mut m);
    let selected = m.string("message_id", Some(""));
    if !selected.is_empty()
        && (!gate_core::validation::is_decimal(&selected) || selected.parse::<u64>().is_err())
    {
        return Err(HttpError::new(422, "消息 ID 无效").into());
    }
    m.finish()?;
    let svc = authorize(&state, &parts, false)?;
    let discord_id = checked(svc, &who)?;
    let key = svc.key_row_for(discord_id).await?;
    let me = super::landing::me_payload(&state, discord_id, key.as_ref()).await?;
    let mut member = member_bot_projection(&me);
    let policy = crate::share_lock::detection(&state).await?;
    member["share_requirement"] = json!({
        "enabled": policy["enabled"], "mode":policy["mode"], "enforcing":policy["enforcing"],
        "shared_today":state.db.has_share_since(discord_id,state.clock.day_start(state.now())).await?,
        "fulfilled":state.db.has_share_since(discord_id,state.clock.day_start(state.now())-86400.0).await?,
        "valid_since":state.clock.day_start(state.now())-86400.0,
        "latest_share":state.db.latest_share(discord_id).await?,
        "selected_share":if selected.is_empty(){None}else{state.db.selected_share(discord_id,&selected).await?},
        "exempt":key.as_ref().is_some_and(|k|k.owner_image_full||k.patron),
        "required_now":me.get("share_lock").is_some(),
        "channel_url":crate::share_lock::channel_url(), "reset_hour":7,
    });
    if let Some(k) = &key {
        member["recent_failures"] = recent_failures(&state, k.id()).await?;
    }
    if me["today"]["daily_images"] == 0 {
        member["v45_paused_by_owner"] = json!(true);
    }
    let status = state.live.status(&state).await?;
    Ok(no_store(ok_json(&json!({
        "server_time": state.now(), "timezone": "Asia/Shanghai",
        "member": member, "site": status.as_ref(),
    }))))
}

/// 本人近 24 小时最后几条被拒 / 失败的原文：成员说「被风控了」时，奶妹以这里为准，不凭额度字段猜。
async fn recent_failures(state: &AppState, key_id: i64) -> Result<Value, crate::error::ApiError> {
    let since = state.now() - 86400.0;
    let mut out = Vec::new();
    for status in ["rejected", "error"] {
        for r in state.db.member_logs(key_id, since, 0, status, 4).await? {
            out.push(json!({
                "at": r["ts"], "model": r["model"], "status": r["status"],
                "reason": r["reason"], "detail": r["detail"], "client": r["client"],
            }));
        }
    }
    out.sort_by(|a, b| {
        b["at"]
            .as_f64()
            .partial_cmp(&a["at"].as_f64())
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    Ok(json!(out))
}

/// 成员自报所用的生图客户端（`/报客户端` 或对话里让奶妹登记）：写成后台成员标签「自报客户端」，
/// 备注按时间累加（最新在前），每条带上汇报时间和近 24 小时实际请求的 UA，站长对照后决定是否恢复。
/// 只改本人的标签。
async fn bot_client(State(state): State<AppState>, req: Request) -> ApiResult {
    const CLIENT_MAX: usize = 40;
    let (parts, body) = req.into_parts();
    let mut m = read_model(&parts, body).await?;
    let who = Who::read(&mut m);
    let client = m.string("client", None);
    m.finish()?;
    let svc = authorize(&state, &parts, false)?;
    let discord_id = checked(svc, &who)?;
    let client: String = take_chars(gate_core::primitives::strip(&client), CLIENT_MAX)
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    if client.trim().is_empty() {
        return Err(HttpError::new(422, "请写上客户端名称").into());
    }
    let Some(key) = svc.key_row_for(discord_id).await? else {
        return Err(HttpError::new(404, "你还没有领取 Key").into());
    };
    let since = state.now() - 86400.0;
    let mut uas: Vec<String> = Vec::new();
    for r in state.db.member_logs(key.id(), since, 0, "", 50).await? {
        let ua = take_chars(r["client"].as_str().unwrap_or(""), 60).to_string();
        if !ua.is_empty() && !uas.contains(&ua) {
            uas.push(ua);
        }
    }
    let seen = if uas.is_empty() {
        "近 24 小时无请求".to_string()
    } else {
        uas.into_iter().take(2).collect::<Vec<_>>().join(" / ")
    };
    let stamp = state.clock.format(state.now(), "%m-%d %H:%M");
    let entry = format!("{stamp} 自报：{client}｜UA：{seen}");
    let note = match state.db.key_tag(key.id(), "自报客户端").await? {
        Some(old) if !old.note.trim().is_empty() => {
            take_chars(&format!("{entry}；{}", old.note.trim()), 200).to_string()
        }
        _ => entry,
    };
    state
        .db
        .upsert_key_tag(key.id(), "自报客户端", &note, "奶妹", state.now())
        .await?;
    crate::action_log::log_action(
        &state.db,
        &format!("Discord:{discord_id}"),
        "成员自报客户端",
        &format!("Key #{}", key.id()),
        &client,
        true,
        state.now(),
    )
    .await;
    Ok(ok_json(&json!({"ok": true, "client": client})))
}

fn member_bot_projection(me: &Value) -> Value {
    let mut out = Map::new();
    for name in [
        "has_key",
        "requires_web_login",
        "share_lock",
        "policy",
        "today",
    ] {
        if let Some(value) = me.get(name) {
            out.insert(name.into(), value.clone());
        }
    }
    out.insert(
        "available_coupon_count".into(),
        json!(me["coupons"].as_array().map_or(0, Vec::len)),
    );
    Value::Object(out)
}

/// 奶妹见到存图频道里的一张图（新帖，或已有帖子里的新内容）就记一笔。
async fn bot_share(State(state): State<AppState>, req: Request) -> ApiResult {
    let (parts, body) = req.into_parts();
    let mut m = read_model(&parts, body).await?;
    let who = Who::read(&mut m);
    let message_id = m.string("message_id", None);
    let channel_id = m.string("channel_id", None);
    let parent_id = m.string("parent_id", Some(""));
    let at = m.number("at");
    m.finish()?;
    let svc = authorize(&state, &parts, false)?;
    let discord_id = checked(svc, &who)?;
    if !gate_core::validation::is_decimal(&message_id)
        || !gate_core::validation::is_decimal(&channel_id)
    {
        return Err(HttpError::new(422, "message_id: Field required").into());
    }
    let forum = crate::share_lock::FORUM_CHANNEL_ID;
    if channel_id != forum && parent_id != forum {
        return Err(HttpError::new(403, "不是存图频道").into());
    }
    let now = state.now();
    if !at.is_finite() || at < 1_000_000_000.0 || at > now + 120.0 {
        return Err(HttpError::new(422, "at: Field required").into());
    }
    let snowflake_at = gate_core::snowflake::snowflake_created_at(&message_id)
        .filter(|t| (*t - at).abs() < 0.002)
        .ok_or_else(|| HttpError::new(422, "消息时间与 message_id 不一致"))?;
    state
        .db
        .record_share(discord_id, &message_id, &channel_id, snowflake_at)
        .await?;
    Ok(ok_json(&json!({"ok": true})))
}

async fn bot_report(State(state): State<AppState>, req: Request) -> ApiResult {
    let (parts, body) = req.into_parts();
    let mut m = read_model(&parts, body).await?;
    let status = m.opt_dict("status");
    m.finish()?;
    authorize(&state, &parts, true)?;
    if let Some(status) = &status {
        if let Some(report) = status.get("share_detection") {
            let report: gate_core::share_detection::Report = serde_json::from_value(report.clone())
                .map_err(|_| HttpError::new(422, "分享检测状态格式不正确"))?;
            if !report.valid(state.now()) {
                return Err(HttpError::new(422, "分享检测时间或错误信息不正确").into());
            }
        }
        state
            .db
            .set_setting(
                bot_config::STATUS_KEY,
                &bot_config::status_to_store(status, state.now()),
            )
            .await?;
    }
    Ok(ok_json(&json!({"ok": true})))
}

#[cfg(test)]
mod bot_context_tests {
    use super::*;
    #[test]
    fn projection_excludes_credentials_and_coupon_identifiers() {
        let data = member_bot_projection(
            &json!({"key":"synthetic-secret", "discord":{"id":"synthetic"},
            "name":"private name", "today":{"reset_hour":7}, "has_key":true,
            "coupons":[{"id":42,"code":"private-code"}], "untrusted_extra":"secret"}),
        );
        assert_eq!(
            data,
            json!({"today":{"reset_hour":7},"has_key":true,"available_coupon_count":1})
        );
    }
}
