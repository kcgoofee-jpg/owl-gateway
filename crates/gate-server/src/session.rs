//! 后台与成员网页的会话 Cookie（格式长期不变，成员与站长的登录一直有效）。
//!
//! - 后台 `nai_gate_admin`：`<到期秒>.<HMAC-SHA256 hex>`，密钥 = `secret + ":" + sha256(密码哈希或 ADMIN_PASSWORD)`，
//!   改密码即令所有旧会话失效；有效期 7 天；SameSite=Strict。
//! - 成员 `owl_member`：`<discord_id>:<到期秒>:<Key 指纹>.<HMAC>`，密钥 = `secret + ":member-login"`；
//!   Key 指纹 = sha256(Key)[:16]，换 Key 后旧会话立刻失效；有效期 30 天；SameSite=Lax。
//! - 后台密码：`scrypt$<盐 hex>$<摘要 hex>`（n=2^14, r=8, p=1, 32 字节）。

use crate::error::HttpError;
use crate::key::Key;
use crate::state::AppState;
use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::http::{HeaderMap, HeaderValue, Method};
use hmac::{Hmac, KeyInit, Mac};
use sha2::{Digest, Sha256};

pub const ADMIN_COOKIE: &str = "nai_gate_admin";
pub const MEMBER_COOKIE: &str = "owl_member";
pub const STATE_COOKIE: &str = "owl_login_state";
pub const ADMIN_SESSION_SECS: i64 = 7 * 86400;
pub const MEMBER_SESSION_DAYS: i64 = 30;

pub(crate) fn hmac_hex(secret: &str, payload: &str) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("hmac key");
    mac.update(payload.as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

fn sha256_hex(s: &str) -> String {
    hex::encode(Sha256::digest(s.as_bytes()))
}

pub(crate) fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    use subtle::ConstantTimeEq;
    a.len() == b.len() && bool::from(a.ct_eq(b))
}

/// Cookie 解析：按 `;` 切，`=` 前后去空白，重复的名字以最后一个为准。
/// 不处理引号转义：我们读的 Cookie 都是自己签发的，值里只有十六进制、数字、点、冒号、横线和 base64url 字符。
pub fn cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    let mut found = None;
    for h in headers.get_all("cookie") {
        let raw: String = h.as_bytes().iter().map(|&b| b as char).collect();
        for chunk in raw.split(';') {
            let (k, v) = chunk.split_once('=').unwrap_or(("", chunk));
            if k.trim() == name {
                found = Some(v.trim().to_string());
            }
        }
    }
    found
}

/// `Set-Cookie` 输出：`<name>=<value>; HttpOnly; Max-Age=<秒>; Path=/; SameSite=<策略>`，
/// `secure` 时追加 `; Secure`（属性顺序固定）。
pub fn set_cookie_header(
    name: &str,
    value: &str,
    max_age: i64,
    secure: bool,
    samesite: &str,
) -> HeaderValue {
    let mut s = format!("{name}={value}; HttpOnly; Max-Age={max_age}; Path=/; SameSite={samesite}");
    if secure {
        s.push_str("; Secure");
    }
    HeaderValue::from_str(&s).unwrap_or(HeaderValue::from_static(""))
}

/// 注销用的 `Set-Cookie`：空值 + 已过期的 `expires` / `Max-Age=0`，浏览器随即删掉该 Cookie。
pub fn delete_cookie_header(
    name: &str,
    secure: bool,
    samesite: &str,
    httponly: bool,
) -> HeaderValue {
    let mut s = format!("{name}=\"\"; expires=Thu, 01 Jan 1970 00:00:00 GMT;");
    if httponly {
        s.push_str(" HttpOnly;");
    }
    s.push_str(" Max-Age=0; Path=/");
    if !samesite.is_empty() {
        s.push_str(&format!("; SameSite={samesite}"));
    }
    if secure {
        s.push_str("; Secure");
    }
    HeaderValue::from_str(&s).unwrap_or(HeaderValue::from_static(""))
}

// ------------------------------------------------------------------ 后台

/// 改过的密码（scrypt 哈希）优先；否则用 ADMIN_PASSWORD。
pub(crate) fn admin_pw_material(state: &AppState) -> String {
    state
        .admin_pw_hash
        .read()
        .expect("pw")
        .clone()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| state.env.admin_password.clone())
}

/// 后台签名密钥：`secret + ":" + sha256(密码材料)`。改密码即令所有会话（和绑定会话的令牌）失效。
pub(crate) fn admin_key_from(secret: &str, pw_material: &str) -> String {
    format!("{secret}:{}", sha256_hex(pw_material))
}

fn admin_session_key(state: &AppState) -> String {
    admin_key_from(&state.secret_key, &admin_pw_material(state))
}

pub fn make_admin_cookie(state: &AppState) -> String {
    let payload = (state.now() as i64 + ADMIN_SESSION_SECS).to_string();
    let sig = hmac_hex(&admin_session_key(state), &payload);
    format!("{payload}.{sig}")
}

pub fn check_admin_cookie(state: &AppState, raw: &str) -> bool {
    let Some((payload, sig)) = raw.split_once('.') else {
        return false;
    };
    if !ct_eq(
        sig.as_bytes(),
        hmac_hex(&admin_session_key(state), payload).as_bytes(),
    ) {
        return false;
    }
    gate_core::pyc::parse_int(payload).is_some_and(|exp| exp as f64 > state.now())
}

/// `scrypt$<salt hex>$<digest hex>`
pub fn hash_password(password: &str) -> String {
    let salt: [u8; 16] = rand::random();
    format!(
        "scrypt${}${}",
        hex::encode(salt),
        hex::encode(scrypt_digest(password, &salt))
    )
}

fn scrypt_digest(password: &str, salt: &[u8]) -> [u8; 32] {
    let params = scrypt::Params::new(14, 8, 1).expect("scrypt params");
    let mut out = [0u8; 32];
    scrypt::scrypt(password.as_bytes(), salt, &params, &mut out).expect("scrypt");
    out
}

pub fn password_matches(password: &str, stored: &str) -> bool {
    let parts: Vec<&str> = stored.split('$').collect();
    let [_, salt, digest] = parts.as_slice() else {
        return false;
    };
    let (Ok(salt), Ok(digest)) = (hex::decode(salt), hex::decode(digest)) else {
        return false;
    };
    ct_eq(&scrypt_digest(password, &salt), &digest)
}

/// 站长在后台改过密码则以保存的哈希为准，否则比对 ADMIN_PASSWORD。
pub fn admin_password_ok(state: &AppState, password: &str) -> bool {
    match state
        .admin_pw_hash
        .read()
        .expect("pw")
        .clone()
        .filter(|s| !s.is_empty())
    {
        Some(stored) => password_matches(password, &stored),
        None => ct_eq(password.as_bytes(), state.env.admin_password.as_bytes()),
    }
}

/// `urlsplit("//" + netloc).hostname`
fn host_only(netloc: &str) -> String {
    let n = netloc.rsplit_once('@').map_or(netloc, |(_, h)| h);
    let host = if let Some(rest) = n.strip_prefix('[') {
        rest.split(']').next().unwrap_or("")
    } else {
        n.split(':').next().unwrap_or("")
    };
    host.to_lowercase()
}

fn netloc_of(url: &str) -> &str {
    let Some((_, rest)) = url.split_once("//") else {
        return "";
    };
    rest.split(['/', '?', '#']).next().unwrap_or("")
}

/// 跨源写请求的 Origin 主机名必须与本站 Host（或 ADMIN_ALLOWED_ORIGINS）一致；
/// 没有 Origin（curl 等非浏览器客户端）放行，它们拿不到浏览器里的 Cookie。X-Forwarded-* 不被信任。
pub fn origin_ok(state: &AppState, headers: &HeaderMap) -> bool {
    let Some(origin) = headers.get("origin") else {
        return true;
    };
    let origin: String = origin.as_bytes().iter().map(|&b| b as char).collect();
    let host = host_only(netloc_of(&origin));
    if host.is_empty() {
        return false;
    }
    let mut allowed: Vec<String> = vec![host_only(&crate::auth::header_str(headers, "host"))];
    allowed.extend(
        state
            .env
            .admin_allowed_origins
            .iter()
            .map(|o| host_only(netloc_of(o))),
    );
    allowed.iter().any(|a| !a.is_empty() && *a == host)
}

/// 后台登录 / 改密码的频率限制（按 IP，IPv6 聚合到 /64）。
pub fn hit_login(state: &AppState, client_id: &str) -> bool {
    let bucket = crate::runtime::bruteforce_bucket(client_id);
    let window = state.env.login_window_seconds.max(1) as f64;
    let limit = state.env.login_max_attempts.max(1) as usize;
    state
        .login_attempts
        .check_then_push(&bucket, limit, window, state.now())
}

/// 后台接口的鉴权：写请求先校验来源，再校验会话。
pub struct AdminSession;

impl FromRequestParts<AppState> for AdminSession {
    type Rejection = HttpError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, HttpError> {
        if !matches!(parts.method, Method::GET | Method::HEAD | Method::OPTIONS)
            && !origin_ok(state, &parts.headers)
        {
            return Err(HttpError::new(403, "来源校验失败"));
        }
        let raw = cookie(&parts.headers, ADMIN_COOKIE).unwrap_or_default();
        if !check_admin_cookie(state, &raw) {
            return Err(HttpError::new(401, "未登录或会话已过期"));
        }
        Ok(AdminSession)
    }
}

// ------------------------------------------------------------------ 成员

fn member_secret(state: &AppState) -> String {
    format!("{}:member-login", state.secret_key)
}

/// 登录授权的用途：普通登录只要 identify；验证身份组要多申请 guilds.members.read。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoginMode {
    Plain,
    Verify,
}

/// 授权 state 的有效期（秒）。
pub const LOGIN_STATE_SECS: i64 = 600;

fn login_state_secret(secret_key: &str) -> String {
    format!("{secret_key}:login-state")
}

/// 签一个自带用途和过期时间的 state：`<p|v>.<nonce>.<exp>.<hmac>`。
/// 不依赖 Cookie，所以 Discord 内置浏览器丢了 Cookie 也能完成回调。
pub fn sign_login_state(
    secret_key: &str,
    mode: LoginMode,
    nonce: &str,
    dev: &str,
    now: i64,
) -> String {
    let m = if mode == LoginMode::Verify { 'v' } else { 'p' };
    // dev = 浏览器设备指纹（十六进制，可空）：跟着签名 state 走一圈 Discord 再回来，不依赖 Cookie
    let payload = format!("{m}.{nonce}.{}.{dev}", now + LOGIN_STATE_SECS);
    let sig = hmac_hex(&login_state_secret(secret_key), &payload);
    format!("{payload}.{sig}")
}

/// 验签并检查过期；任何一步不对都是 None。
pub fn verify_login_state(secret_key: &str, raw: &str, now: i64) -> Option<(LoginMode, String)> {
    let (payload, sig) = raw.rsplit_once('.')?;
    if !ct_eq(
        sig.as_bytes(),
        hmac_hex(&login_state_secret(secret_key), payload).as_bytes(),
    ) {
        return None;
    }
    // 老的三段式 state（没有 dev 字段）照旧有效，dev 取空串
    let mut it = payload.splitn(4, '.');
    let (mode, nonce, exp) = (it.next()?, it.next()?, it.next()?);
    let dev = it.next().unwrap_or("");
    if nonce.is_empty() || gate_core::pyc::parse_int(exp)? <= now {
        return None;
    }
    let mode = match mode {
        "p" => LoginMode::Plain,
        "v" => LoginMode::Verify,
        _ => return None,
    };
    Some((mode, dev.to_string()))
}

/// 会话绑定的 Key 指纹：Key 一换（/resetkey、回收后重领），旧会话立刻失效。没有 Key 时为 "-"。
pub fn key_tag(key: Option<&Key>) -> String {
    match key {
        Some(k) => sha256_hex(&gate_core::pyval::py_str(k.get("token")))[..16].to_string(),
        None => "-".into(),
    }
}

pub fn sign_member(state: &AppState, discord_id: &str, exp: i64, tag: &str) -> String {
    let payload = format!("{discord_id}:{exp}:{tag}");
    let sig = hmac_hex(&member_secret(state), &payload);
    format!("{payload}.{sig}")
}

/// 验签并解析：返回 (discord_id, Key 指纹)。旧格式（不带指纹）与以前「Key 登录」签的会话一律作废。
pub fn parse_member(state: &AppState, raw: &str) -> Option<(String, String)> {
    let (payload, sig) = raw.rsplit_once('.')?;
    if !ct_eq(
        sig.as_bytes(),
        hmac_hex(&member_secret(state), payload).as_bytes(),
    ) {
        return None;
    }
    let mut it = payload.rsplitn(3, ':');
    let (tag, exp, discord_id) = (it.next()?, it.next()?, it.next()?);
    if discord_id.is_empty() || !discord_id.chars().all(|c| c.is_numeric()) {
        return None;
    }
    let exp = gate_core::pyc::parse_int(exp)?;
    (exp as f64 > state.now()).then(|| (discord_id.to_string(), tag.to_string()))
}

/// 有效会话返回 (discord_id, 领到的 Key)。会话签发时绑定了当时 Key 的指纹：Key 换了即失效。
pub async fn member_state(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<Option<(String, Option<Key>)>, gate_db::DbError> {
    let Some(raw) = cookie(headers, MEMBER_COOKIE) else {
        return Ok(None);
    };
    let Some((discord_id, tag)) = parse_member(state, &raw) else {
        return Ok(None);
    };
    // 和 `/self-register/*`、`/public/me` 同一个入口（RegistrationService::key_row_for），没配置自助领取就没有 Key
    let key = match state.registration.as_deref() {
        Some(svc) => svc.key_row_for(&discord_id).await?,
        None => None,
    };
    if !ct_eq(tag.as_bytes(), key_tag(key.as_ref()).as_bytes()) {
        return Ok(None);
    }
    Ok(Some((discord_id, key)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scrypt_matches_python() {
        // hashlib.scrypt(b"correct horse battery", salt=bytes.fromhex("00112233445566778899aabbccddeeff"),
        //                n=2**14, r=8, p=1, dklen=32).hex()
        let stored = "scrypt$00112233445566778899aabbccddeeff$8cc0de9218d3d6723ae203a934c046cf2bb9aa70723c2cf4b1cf06ca35c89adb";
        assert!(password_matches("correct horse battery", stored));
        assert!(!password_matches("correct horse batter", stored));
        let h = hash_password("correct horse battery");
        assert!(password_matches("correct horse battery", &h));
        assert!(!password_matches("wrong", &h));
        assert!(!password_matches("x", "garbage"));
    }

    #[test]
    fn hmac_matches_python() {
        // 后台：hmac(secret + ":" + sha256(ADMIN_PASSWORD), "1900000000")
        let key = format!(
            "test-secret-key-test-secret-key-0123456789:{}",
            sha256_hex("dev-admin-password-123")
        );
        assert_eq!(
            hmac_hex(&key, "1900000000"),
            "ea464fc1480ce4673f9a4a02c2dca72ab0cf114505af9c49b1196ffb0248df15"
        );
        // 成员：discord_id:exp:sha256(Key)[:16] 签名
        let tag = &sha256_hex("nai-test-member-key-000000000000000000")[..16];
        let payload = format!("900000000000000001:1900000000:{tag}");
        assert_eq!(
            format!(
                "{payload}.{}",
                hmac_hex(
                    "test-secret-key-test-secret-key-0123456789:member-login",
                    &payload
                )
            ),
            "900000000000000001:1900000000:178b8ba5e5a7866a.385eaae505ec346b943c6e7cdf9fe81368e57b61d2d7c02b4df7572bb4e271b2"
        );
    }

    #[test]
    fn login_state_roundtrip_and_tamper() {
        let k = "test-secret-key-test-secret-key-0123456789";
        let now = 1_900_000_000;
        let v = sign_login_state(k, LoginMode::Verify, "abc_-123", "beef1234", now);
        let p = sign_login_state(k, LoginMode::Plain, "abc_-123", "", now);
        assert_eq!(
            verify_login_state(k, &v, now),
            Some((LoginMode::Verify, "beef1234".to_string()))
        );
        assert_eq!(
            verify_login_state(k, &p, now + 599),
            Some((LoginMode::Plain, String::new()))
        );
        // 过期（10 分钟）
        assert_eq!(verify_login_state(k, &v, now + LOGIN_STATE_SECS), None);
        // 换密钥 / 改用途 / 空串 / 旧式随机串都不认
        assert_eq!(verify_login_state("other-secret", &v, now), None);
        assert_eq!(verify_login_state(k, &v.replacen("v.", "p.", 1), now), None);
        assert_eq!(verify_login_state(k, "", now), None);
        assert_eq!(verify_login_state(k, "Zm9vYmFyYmF6", now), None);
        assert_eq!(verify_login_state(k, "x.n.99999999999.00", now), None);
    }

    #[test]
    fn cookie_parsing() {
        let mut h = HeaderMap::new();
        h.insert(
            "cookie",
            "a=1; owl_member=abc.def ; nai_gate_admin=12.ab; a=2"
                .parse()
                .unwrap(),
        );
        assert_eq!(cookie(&h, "owl_member").as_deref(), Some("abc.def"));
        assert_eq!(cookie(&h, "nai_gate_admin").as_deref(), Some("12.ab"));
        assert_eq!(cookie(&h, "a").as_deref(), Some("2"));
        assert_eq!(cookie(&h, "missing"), None);
    }

    #[test]
    fn host_parsing() {
        assert_eq!(host_only("Example.COM:8443"), "example.com");
        assert_eq!(host_only("user@[::1]:80"), "::1");
        assert_eq!(host_only(netloc_of("https://a.b/c")), "a.b");
        assert_eq!(host_only(netloc_of("null")), "");
    }
}
