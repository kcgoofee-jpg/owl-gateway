//! 后台接口 `/admin/api/*`。每个分组一个文件；所有会改动数据的接口都经过
//! [`audited`] 写操作日志（谁、何时、做了什么、对象、参数摘要、结果）。未登录的请求不记录（防止被刷），
//! 登录本身无论成败都记录。

mod abuse;
mod auth;
mod jobs;
mod members;
mod num;
mod ops;
mod records;
mod req;

use crate::action_log::{label, log_action, summarize};
use crate::error::HttpErrorInfo;
use crate::session::{ADMIN_COOKIE, check_admin_cookie, cookie};
use crate::state::AppState;
use axum::Router;
use axum::body::Body;
use axum::extract::{MatchedPath, Request, State};
use axum::http::Method;
use axum::middleware::{Next, from_fn_with_state};
use axum::response::Response;
use serde_json::Value;

pub const PREFIX: &str = "/admin/api";
/// 后台 JSON 请求体上限（1 MiB）；子模块共用。
const BODY_LIMIT: usize = 1024 * 1024;
const AUDIT_BODY_LIMIT: usize = 64 * 1024;

pub fn router(state: AppState) -> Router<AppState> {
    Router::new()
        .merge(abuse::router())
        .merge(auth::router())
        .merge(ops::router())
        .merge(jobs::router())
        .merge(members::router())
        .merge(records::router())
        .route_layer(from_fn_with_state(state, audited))
}

/// 处理函数自己写了操作日志时放进响应扩展，中间件就不再重复记。
#[derive(Clone, Copy)]
pub struct ActionLogged;

/// 处理函数自己已经记了一条可读的操作日志：给响应打上标记，中间件就不再记一条原样的请求体。
fn mark_logged(mut resp: Response) -> Response {
    resp.extensions_mut().insert(ActionLogged);
    resp
}

/// nai-ops 发的请求带这个头（值 `claude`）：操作日志记成「手动（Claude）」，不记「后台 IP」。
/// 只改操作者的写法，不影响登录校验；没登录的请求本来就不记。
pub const ACTOR_HEADER: &str = "x-owl-actor";

/// 「后台 *.*.x.y」：操作者（IP 打码）；nai-ops 的请求是「手动（Claude）」。
pub fn actor(state: &AppState, parts: &axum::http::request::Parts) -> String {
    if crate::auth::header_str(&parts.headers, ACTOR_HEADER).eq_ignore_ascii_case("claude") {
        return gate_core::actor::CLAUDE.to_string();
    }
    let ip = crate::auth::client_ip(parts, &state.env.forwarded_allow_ips);
    format!("后台 {}", crate::runtime::mask_ip(&ip))
}

/// 操作前先记下对象名称（删除之后就查不到了）。
async fn target_of(state: &AppState, template: &str, path: &str) -> String {
    let segs: Vec<&str> = template.split('/').collect();
    let vals: Vec<&str> = path.split('/').collect();
    let param = |name: &str| {
        segs.iter()
            .position(|s| *s == format!("{{{name}}}"))
            .and_then(|i| vals.get(i))
            .map(|v| percent_decode(v))
    };
    if let Some(id) = param("key_id") {
        let Ok(n) = id.parse::<i64>() else {
            return String::new();
        };
        let name = match state.db.get_key(n).await {
            Ok(Some(k)) => gate_core::pyval::py_str(k.get("name").unwrap_or(&Value::Null)),
            Ok(None) => "(不存在)".into(),
            Err(_) => return String::new(),
        };
        return format!("Key #{id} {name}");
    }
    if let Some(id) = param("abuse_id") {
        return format!("待审 #{id}");
    }
    if let Some(t) = param("token_id") {
        return format!("上游 {}", gate_core::pyc::take_chars(&t, 24));
    }
    if let Some(sig) = param("sig") {
        return format!("Bug {}", gate_core::pyc::take_chars(&sig, 12));
    }
    let suffix: &[(&str, &str)] = &[
        ("/errors/resolve-all", "全部未处理 Bug"),
        ("/ops/registration", "自助领 Key 设置"),
        ("/ops/features", "全站功能开关"),
        ("/guard", "账号保护与排队"),
        ("/runtime-limits", "图片排队与冷却"),
        ("/settings", "Anlas 与 V5 预算"),
        ("/quota-algo", "动态额度参数"),
    ];
    if let Some((_, t)) = suffix.iter().find(|(s, _)| template.ends_with(s)) {
        return (*t).to_string();
    }
    let tail: &[(&str, &str)] = &[("/password", "后台登录密码")];
    tail.iter()
        .find(|(s, _)| template.ends_with(s))
        .map_or_else(String::new, |(_, t)| (*t).to_string())
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && let (Some(h), Some(l)) = (
                b.get(i + 1).and_then(|c| (*c as char).to_digit(16)),
                b.get(i + 2).and_then(|c| (*c as char).to_digit(16)),
            )
        {
            out.push((h * 16 + l) as u8);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

async fn audited(State(state): State<AppState>, req: Request, next: Next) -> Response {
    if matches!(*req.method(), Method::GET | Method::HEAD | Method::OPTIONS) {
        return next.run(req).await;
    }
    let template = req
        .extensions()
        .get::<MatchedPath>()
        .map(|m| m.as_str().to_string())
        .unwrap_or_else(|| req.uri().path().to_string());
    let path = template
        .strip_prefix(PREFIX)
        .unwrap_or(&template)
        .to_string();
    let method = req.method().as_str().to_string();
    let mut action = label(&method, &path);
    let is_login = path == "/login";
    let (parts, body) = req.into_parts();
    // 缓存请求体（≤ 64 KB）：处理函数随后仍能读到同一份内容
    let declared = crate::auth::header_str(&parts.headers, "content-length");
    let small = !declared.is_empty()
        && declared.bytes().all(|b| b.is_ascii_digit())
        && declared
            .parse::<usize>()
            .is_ok_and(|n| n <= AUDIT_BODY_LIMIT);
    let (body, parsed) = if small {
        match axum::body::to_bytes(body, AUDIT_BODY_LIMIT).await {
            Ok(bytes) => {
                let v = if bytes.is_empty() {
                    Value::Null
                } else {
                    serde_json::from_slice(&bytes).unwrap_or(Value::Null)
                };
                (Body::from(bytes), v)
            }
            Err(_) => (Body::empty(), Value::Null),
        }
    } else {
        (body, Value::Null)
    };
    let mut target = target_of(&state, &template, parts.uri.path()).await;
    let authed = is_login
        || check_admin_cookie(
            &state,
            &cookie(&parts.headers, ADMIN_COOKIE).unwrap_or_default(),
        );
    let who = actor(&state, &parts);
    let uri = parts.uri.clone();
    let resp = next.run(Request::from_parts(parts, body)).await;
    if !authed {
        return resp;
    }
    if let Some(HttpErrorInfo(e)) = resp.extensions().get::<HttpErrorInfo>() {
        let detail = gate_core::pyc::take_chars(
            &format!(
                "失败（{}）：{}",
                e.status,
                gate_core::pyval::py_str(&e.detail)
            ),
            200,
        )
        .to_string();
        log_action(
            &state.db,
            &who,
            &action,
            &target,
            &detail,
            false,
            state.now(),
        )
        .await;
        return resp;
    }
    if resp.status().is_server_error() || resp.extensions().get::<ActionLogged>().is_some() {
        return resp;
    }
    if path == "/keys"
        && let Some(obj) = parsed.as_object()
    {
        target = obj
            .get("name")
            .filter(|v| gate_core::pyval::truthy(v))
            .map(gate_core::pyext::py_str)
            .unwrap_or_default();
    }
    if path == "/keys/{key_id}" && method == "DELETE" {
        let ban = axum::extract::Query::<Vec<(String, String)>>::try_from_uri(&uri)
            .ok()
            .and_then(|q| q.0.into_iter().rev().find(|(k, _)| k == "ban"))
            .map(|(_, v)| v);
        if ban.as_deref() == Some("true") {
            action = "删除 Key 并永久禁止领取".into();
        }
    }
    let detail = if is_login {
        String::new()
    } else {
        summarize(&parsed, 300)
    };
    log_action(
        &state.db,
        &who,
        &action,
        &target,
        &detail,
        true,
        state.now(),
    )
    .await;
    resp
}
