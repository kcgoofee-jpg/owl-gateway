//! 成员接口的鉴权与准入前检查（鉴权 / 功能开关 / 每分钟次数 / 出图冷却）。

use crate::error::{ApiError, GateError, InternalError};
use crate::key::Key;
use crate::reqctx::ReqCtx;
use crate::state::AppState;
use axum::extract::ConnectInfo;
use axum::http::HeaderMap;
use axum::http::request::Parts;
use gate_core::reasons;
use gate_core::site_flags::{self, Spec};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

/// 请求头按 latin-1 解码（Starlette 的 `request.headers.get` 行为）；没有这个头为空串。
pub fn header_str(headers: &HeaderMap, name: &str) -> String {
    headers
        .get(name)
        .map(|v| v.as_bytes().iter().map(|&b| b as char).collect())
        .unwrap_or_default()
}

/// 对端地址是否在信任列表里（uvicorn `FORWARDED_ALLOW_IPS`：精确地址、CIDR 或 `*`）。
fn trusted(host: &str, list: &[String]) -> bool {
    list.iter().any(|t| {
        if t == "*" || t == host {
            return true;
        }
        let (Some((net, bits)), Ok(ip)) = (t.split_once('/'), host.parse::<IpAddr>()) else {
            return false;
        };
        let (Ok(net), Ok(bits)) = (net.parse::<IpAddr>(), bits.parse::<u32>()) else {
            return false;
        };
        match (net, ip) {
            (IpAddr::V4(n), IpAddr::V4(i)) if bits <= 32 => {
                let mask = if bits == 0 {
                    0
                } else {
                    u32::MAX << (32 - bits)
                };
                u32::from(n) & mask == u32::from(i) & mask
            }
            (IpAddr::V6(n), IpAddr::V6(i)) if bits <= 128 => {
                let mask = if bits == 0 {
                    0
                } else {
                    u128::MAX << (128 - bits)
                };
                u128::from(n) & mask == u128::from(i) & mask
            }
            _ => false,
        }
    })
}

/// 成员的 IP（反代场景按信任列表解析 X-Forwarded-For）：
/// 直连对端受信任时取 X-Forwarded-For 从右往左第一个不受信任的地址；全部受信任取最左边那个。
pub fn client_ip(parts: &Parts, trusted_list: &[String]) -> String {
    let peer = parts
        .extensions
        .get::<ConnectInfo<SocketAddr>>()
        .map(|c| c.0.ip().to_string());
    client_ip_from(
        peer.as_deref(),
        &header_str(&parts.headers, "x-forwarded-for"),
        trusted_list,
    )
}

/// 同 [`client_ip`]，但直接给「直连对端地址」和「X-Forwarded-For 头」（用量日志记来源 IP 时在请求上下文里取这两个值）。
pub fn client_ip_from(peer: Option<&str>, xff: &str, trusted_list: &[String]) -> String {
    let Some(peer) = peer else {
        return "unknown".into();
    };
    if xff.is_empty() || !trusted(peer, trusted_list) {
        return peer.to_string();
    }
    let hosts: Vec<&str> = xff.split(',').map(str::trim).collect();
    if trusted_list.iter().any(|t| t == "*") {
        return hosts[0].to_string();
    }
    hosts
        .iter()
        .rev()
        .find(|h| !trusted(h, trusted_list))
        .unwrap_or(&hosts[0])
        .to_string()
}

/// 请求上下文（中间件一定会放；单测里直接调路由时补一个）。
pub fn ctx_of(parts: &Parts) -> ReqCtx {
    parts
        .extensions
        .get::<ReqCtx>()
        .cloned()
        .unwrap_or_else(ReqCtx::new)
}

/// Key 已停用或已过期时的拒绝（403）；正常返回 None。`authenticate` 和问卷的会话分支共用，文案只此一份。
pub fn key_status_error(key: &Key, now: f64) -> Option<GateError> {
    if !key.enabled() {
        let why = key.get("disabled_reason").as_str();
        return Some(GateError::with_code(
            403,
            gate_core::key_status::disabled_message(why),
            reasons::KEY_DISABLED,
        ));
    }
    if key.expired(now) {
        return Some(GateError::with_code(
            403,
            "这把 Key 已过期。请在 Discord 用 /register 重新领取。",
            reasons::KEY_DISABLED,
        ));
    }
    None
}

/// `Authorization: Bearer <key>` 里的 Key（大小写不敏感；没有为空串）。
pub fn bearer(headers: &HeaderMap) -> String {
    let auth = header_str(headers, "authorization");
    if auth
        .get(..7)
        .is_some_and(|p| p.eq_ignore_ascii_case("bearer "))
    {
        gate_core::pyc::strip(&auth[7..]).to_string()
    } else {
        String::new()
    }
}

/// 校验虚拟 Key。`passive`：首页每秒查排队位置，只读，不算使用，不参与暂停闸门。
pub async fn authenticate(state: &AppState, parts: &Parts, passive: bool) -> Result<Key, ApiError> {
    let ctx = ctx_of(parts);
    let ip = client_ip(parts, &state.env.forwarded_allow_ips);
    let now = state.now();
    let wait = state.auth_guard.blocked(&ip, now);
    let token = bearer(&parts.headers);
    let too_many = || {
        GateError::with_code(
            403,
            format!(
                "无效 Key 请求次数过多，已临时拦截。请确认客户端中的 Key 完整无误，{} 分钟后重试。",
                wait / 60 + 1
            ),
            reasons::KEY_INVALID,
        )
    };
    if token.is_empty() {
        if wait > 0 {
            return Err(too_many().into());
        }
        return Err(GateError::with_code(401, "未收到 API Key。请在客户端中填入本站发放的 Key。开发者参考：放在 Authorization: Bearer 请求头中", reasons::KEY_INVALID).into());
    }
    let Some(row) = state.db.get_key_by_token(&token).await? else {
        // 被拦截的 IP 只拦「无效 Key」；持有有效 Key 的成员（如同一出口 IP 的其他人）不受影响。
        if wait > 0 {
            return Err(too_many().into());
        }
        record_auth_failure(state, &ip, now);
        return Err(GateError::with_code(
            401,
            "API Key 无效。请检查客户端中的 Key 是否完整，或在 Discord 用 /resetkey 重新获取。",
            reasons::KEY_INVALID,
        )
        .into());
    };
    let key = Key(row);
    ctx.set_key(key.id(), key.name());
    if let Some(e) = key_status_error(&key, now) {
        return Err(e.into());
    }
    if passive {
        return Ok(key);
    }
    if let Some((until, reason)) = state.pauses.paused(key.id(), now) {
        let until = state.clock.format(until, "%m-%d %H:%M");
        let msg = if reason.is_empty() {
            format!("这把 Key 已被临时暂停，未扣费。{until} 自动恢复。")
        } else {
            format!("{reason}，{until} 自动恢复。")
        };
        return Err(GateError::with_code(403, msg, reasons::KEY_PAUSED).into());
    }
    // 站长要求每位成员到网页用 Discord 登录一次（设置 web_login_required_since = 开始时刻，0 = 不要求）：
    // 这之后还没在网页登录过的 Key 先拦下，登录一次立即恢复。没绑 Discord 的 Key 不拦。
    if let Some(since) = web_login_required_since(state).await {
        let discord_id = key.get("discord_id").as_str().unwrap_or("").to_string();
        if !discord_id.is_empty() && !web_logged_in_since(state, &discord_id, since).await {
            return Err(GateError::with_code(
                403,
                format!(
                    "请先在 {} 用 Discord 登录一次，登录后自动恢复。",
                    state.env.site_url.trim_end_matches('/')
                ),
                reasons::KEY_PAUSED,
            )
            .into());
        }
    }
    // 只要 Key 实际通过鉴权即视为使用。记「最后使用时间」只是记账：写失败不能让成员的请求 500。
    if let Err(e) = state.db.touch_key(key.id(), now).await {
        state.bug("touch_key", Some(("DbError", &e.to_string())), &ctx);
    }
    Ok(key)
}

/// 「每位成员到网页登录一次」的开始时刻；没设或为 0 = 不要求。
pub const WEB_LOGIN_SINCE_KEY: &str = "web_login_required_since";

/// 开关值缓存多久（每次出图都要查，省掉一次读库；站长改了最多 15 秒生效）。
const WEB_LOGIN_SETTING_TTL: Duration = Duration::from_secs(15);

/// 「网页登录一次」开关的进程内缓存：开关值（带读取时刻），以及当前 `since` 下已确认登录过的 Discord ID
/// （只记放行的结果，`since` 一变就清空）。同进程里可能并存多个网关（测试），按数据库路径区分。
#[derive(Default)]
struct WebLoginGate {
    setting: Option<(Instant, Option<f64>)>,
    since: f64,
    verified: HashSet<String>,
}

static WEB_LOGIN_GATES: LazyLock<Mutex<HashMap<PathBuf, WebLoginGate>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn gates() -> std::sync::MutexGuard<'static, HashMap<PathBuf, WebLoginGate>> {
    WEB_LOGIN_GATES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// 清掉这个库的「网页登录一次」缓存（改了开关后想立刻生效时调用，例如测试或后台保存设置）。
pub fn clear_web_login_cache(db_path: &Path) {
    gates().remove(db_path);
}

/// 开关的开始时刻（缓存 15 秒）。读库出错不拦出图：当作开关关着，记一条警告。
async fn web_login_required_since(state: &AppState) -> Option<f64> {
    let path = state.db.path();
    if let Some((at, v)) = gates().get(path).and_then(|g| g.setting)
        && at.elapsed() < WEB_LOGIN_SETTING_TTL
    {
        return v;
    }
    let v = match state.db.get_setting(WEB_LOGIN_SINCE_KEY).await {
        Ok(v) => v
            .and_then(|s| s.trim().parse::<f64>().ok())
            .filter(|t| *t > 0.0),
        Err(e) => {
            tracing::warn!("读「网页登录一次」开关失败，这次按不要求处理：{e}");
            return None;
        }
    };
    gates().entry(path.to_path_buf()).or_default().setting = Some((Instant::now(), v));
    v
}

/// 这个成员在 `since` 之后有没有网页登录过。放行的结果缓存起来（登录过就不会变回没登录过）；
/// 没登录过的每次都查库，登录后下一张图立即放行。读库出错不拦，记一条警告。
async fn web_logged_in_since(state: &AppState, discord_id: &str, since: f64) -> bool {
    let path = state.db.path();
    if gates()
        .get(path)
        .is_some_and(|g| g.since == since && g.verified.contains(discord_id))
    {
        return true;
    }
    match state.db.has_web_login_since(discord_id, since).await {
        Ok(true) => {
            let mut all = gates();
            let g = all.entry(path.to_path_buf()).or_default();
            if g.since != since {
                g.since = since;
                g.verified.clear();
            }
            g.verified.insert(discord_id.to_string());
            true
        }
        Ok(false) => false,
        Err(e) => {
            tracing::warn!("查网页登录记录失败，这次不拦：{e}");
            true
        }
    }
}

/// 记一次无效 Key；达到阈值时拦截该 IP 并写一行日志。
pub fn record_auth_failure(state: &AppState, ip: &str, now: f64) {
    let e = &state.env;
    if state.auth_guard.record_failure(
        ip,
        e.auth_fail_max,
        e.auth_fail_window,
        e.auth_block_seconds,
        now,
    ) {
        tracing::info!(
            "有 IP 在 {} 秒内用无效 Key 反复请求 {} 次，已临时拦截 {} 分钟（IP 后两段：{}）",
            e.auth_fail_window.max(1),
            e.auth_fail_max,
            e.auth_block_seconds.div_euclid(60),
            crate::runtime::mask_ip(ip)
        );
    }
}

/// `site_flags.get(db, spec, settings)`：库里有就用库里的，没有就用登记表里的默认值（不再有环境变量回退）。
pub async fn site_flag(state: &AppState, spec: &Spec) -> Result<Value, gate_db::DbError> {
    crate::audit::site_value(&state.db, spec).await
}

/// 数值型站点设置。
pub async fn site_flag_f64(state: &AppState, spec: &Spec) -> Result<f64, gate_db::DbError> {
    crate::audit::site_f64(&state.db, spec).await
}

/// 功能开关（站长全局开关 + 这把 Key 的功能列表）。
pub async fn require_feature(state: &AppState, key: &Key, name: &str) -> Result<(), ApiError> {
    use gate_core::features::{GLOBAL_KEY, VibeAnlas, check_decision, needs_vibe_site_flags};
    let global = state.db.get_setting(GLOBAL_KEY).await?;
    let vibe = if needs_vibe_site_flags(global.as_deref(), &key.0, name) {
        Some(VibeAnlas {
            daily_cap: site_flag_f64(state, &site_flags::GLOBAL_DAILY_ANLAS).await? as i64,
        })
    } else {
        None
    };
    match check_decision(global.as_deref(), &key.0, name, vibe) {
        Ok(None) => Ok(()),
        Ok(Some(reason)) => Err(GateError::with_code(403, reason, reasons::FEATURE_OFF).into()),
        Err(e) => Err(InternalError(e.to_string()).into()),
    }
}

/// 每分钟请求数。
pub fn check_rpm(state: &AppState, key: &Key) -> Result<(), GateError> {
    let limit = key.rpm().max(1) as usize;
    if state
        .rpm
        .check_then_push(&key.id(), limit, 60.0, state.now())
    {
        return Ok(());
    }
    Err(GateError::with_code(
        429,
        format!(
            "请求过于频繁，每把 Key 每分钟最多 {} 次，未扣费。请稍后重试。",
            key.rpm()
        ),
        reasons::KEY_RATE,
    ))
}

/// 上游 429 后的全站图片冷却。
pub fn check_image_cooldown(state: &AppState) -> Result<(), GateError> {
    let remaining = state.cooldown.remaining(state.now());
    if remaining > 0 {
        return Err(GateError::with_code(
            429,
            format!("NovelAI 正在限流，全站暂停出图约 {remaining} 秒，未扣费。请稍后重试。"),
            reasons::COOLDOWN,
        )
        .retry_after(remaining as u64));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parts(peer: &str, xff: Option<&str>) -> Parts {
        let mut b = axum::http::Request::builder().uri("/");
        if let Some(x) = xff {
            b = b.header("x-forwarded-for", x);
        }
        let (mut p, _) = b.body(()).unwrap().into_parts();
        p.extensions.insert(ConnectInfo(
            format!("{peer}:1234").parse::<SocketAddr>().unwrap(),
        ));
        p
    }

    #[test]
    fn forwarded_for_trust() {
        let t = vec!["127.0.0.1".to_string(), "172.30.0.1".to_string()];
        assert_eq!(
            client_ip(&parts("172.30.0.1", Some("203.0.113.9")), &t),
            "203.0.113.9"
        );
        // 不受信任的对端：XFF 被忽略
        assert_eq!(
            client_ip(&parts("198.51.100.7", Some("203.0.113.9")), &t),
            "198.51.100.7"
        );
        // 多跳：从右往左第一个不受信任的
        assert_eq!(
            client_ip(
                &parts("127.0.0.1", Some("10.9.9.9, 203.0.113.9, 172.30.0.1")),
                &t
            ),
            "203.0.113.9"
        );
        assert_eq!(client_ip(&parts("127.0.0.1", None), &t), "127.0.0.1");
        let cidr = vec!["172.30.0.0/16".to_string()];
        assert_eq!(
            client_ip(&parts("172.30.5.5", Some("203.0.113.9")), &cidr),
            "203.0.113.9"
        );
    }

    #[test]
    fn bearer_parsing() {
        let mut h = HeaderMap::new();
        h.insert("authorization", "bEaReR   nai-abc  ".parse().unwrap());
        assert_eq!(bearer(&h), "nai-abc");
        h.insert("authorization", "Basic x".parse().unwrap());
        assert_eq!(bearer(&h), "");
    }
}
