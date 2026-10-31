//! 机器人桥接 `/self-register/*`。
//!
//! 鉴权：`Authorization: Bearer <REGISTRATION_BRIDGE_SECRET>`；成员网页接口见 `member.rs`。
//! 检查次序固定：先校验请求体（422），再鉴权（503 / 401），再校验服务器（403）。
//! 机器人只有成员命令（/register /resetkey /反馈；/quota、/help 10-23 删除）；管理员命令 `ops` / `slots` / `revoke` 已删除（管理都在网页后台）。

use crate::action_log::log_action;
use crate::auth::{client_ip, header_str};
use crate::error::{ApiError, ApiResult, HttpError, InternalError, ok_json};
use crate::multipart::read_bounded_body;
use crate::registration::{DiscordUser, Issued, RegErr, RegistrationService};
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
    let invite = take_chars(&invite_key, 80).to_string();
    let result = match svc
        .issue_direct(&discord_id, &guild_id, &user, &invite)
        .await
    {
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
    match result {
        Issued::Bound { key_id, message } => {
            log_discord(
                &state,
                &discord_id,
                "绑定邀请 Key（/register）",
                &format!("Key #{key_id}"),
                "",
            )
            .await;
            Ok(no_store(ok_json(&json!({"message": message}))))
        }
        Issued::Provisioned(p) => {
            log_discord(&state, &discord_id, "领取 Key（/register 直发）", "", "").await;
            Ok(no_store(ok_json(
                &json!({"key": p.key, "message": p.message}),
            )))
        }
    }
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

async fn bot_report(State(state): State<AppState>, req: Request) -> ApiResult {
    let (parts, body) = req.into_parts();
    let mut m = read_model(&parts, body).await?;
    let status = m.opt_dict("status");
    m.finish()?;
    authorize(&state, &parts, true)?;
    if let Some(status) = &status {
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
