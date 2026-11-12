//! 网页设备指纹上报与跨登录复用的浏览器标记。
use crate::auth::{client_ip, header_str};
use crate::error::ok_json;
use crate::{session, state::AppState};
use axum::extract::{Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use gate_core::primitives::take_chars;
use serde_json::{Value, json};

fn no_content() -> Response {
    StatusCode::NO_CONTENT.into_response()
}
/// `urllib.parse.urlsplit(url).netloc`
fn netloc(url: &str) -> String {
    // urlsplit 先去掉首部的控制字符 / 空格，并删掉 \t \r \n
    let url: String = url
        .trim_start_matches(|c: char| c <= ' ')
        .chars()
        .filter(|c| !matches!(c, '\t' | '\r' | '\n'))
        .collect();
    let mut rest = url.as_str();
    if let Some(i) = rest.find(':')
        && i > 0
        && rest.as_bytes()[0].is_ascii_alphabetic()
        && rest[..i]
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
    {
        rest = &rest[i + 1..];
    }
    match rest.strip_prefix("//") {
        Some(r) => r.split(['/', '?', '#']).next().unwrap_or("").to_string(),
        None => String::new(),
    }
}

/// 请求明确来自别的站点的页面：取 Origin（没有就取 Referer），其 netloc 解析得出来、Host 也有、两者不同。
/// 解析不出 netloc（如 `Origin: null`）或没带来源一律放行。
fn foreign_page(h: &HeaderMap) -> bool {
    let origin = header_str(h, "origin");
    let from = if origin.is_empty() {
        header_str(h, "referer")
    } else {
        origin
    };
    if from.is_empty() {
        return false;
    }
    let host = header_str(h, "host").to_lowercase();
    let theirs = netloc(&from).to_lowercase();
    !theirs.is_empty() && !host.is_empty() && theirs != host
}

/// 网页登录前，前端把设备指纹的各项（显卡 / 屏幕 / 时区 / 硬件 / Canvas 与 WebGL 的哈希 …）发到这里：
/// `{"data": {...}}`。服务端把各项规整成固定格式（键排序、值一律转成字符串、限长限个数），
/// 指纹 id = 规整结果 sha256 的前 32 位十六进制，按 id 存一份明细，回 `{"id": id, "bid": 浏览器标记}`；前端拿 id 带去 /login?dev=。
/// 同时续种浏览器标记 Cookie `owl_bid`（见 [`BROWSER_COOKIE`]；请求体可带 `bid`，是前端 localStorage 里存的那份）。
/// id 由内容决定：谁也改不了别人那条明细，也没法凭空编一个 id 栽给别人（登录时服务端不认得的 id 记为疑似伪造）。
/// 只接受同源请求、限长、限频；防滥用用途。各项不合格时回 204（前端就不带 dev）。
pub(super) async fn device_fp(State(state): State<AppState>, req: Request) -> Response {
    let (parts, body) = req.into_parts();
    let h = &parts.headers;
    if foreign_page(h) {
        return no_content();
    }
    let ip = client_ip(&parts, &state.env.forwarded_allow_ips);
    if !state.live.device_fp_slot(&ip, state.now()) {
        return no_content();
    }
    let payload = crate::multipart::read_bounded_body(h, body, 8192)
        .await
        .ok()
        .and_then(|raw| serde_json::from_slice::<Value>(&raw).ok());
    // 浏览器标记：Cookie 优先；没有就认前端 localStorage 里的那份（只清 Cookie 不会换标记）；都没有就新发一个
    let stored = payload.as_ref().and_then(|v| v.get("bid")?.as_str());
    let (bid, bid_cookie) = issue_browser_cookie(h, stored);
    let with_bid = |mut resp: Response| {
        resp.headers_mut().append(header::SET_COOKIE, bid_cookie);
        resp
    };
    let Some(payload) = payload else {
        return with_bid(no_content());
    };
    let Some((id, canonical)) = payload
        .get("data")
        .and_then(Value::as_object)
        .and_then(canonical_parts)
    else {
        return with_bid(no_content());
    };
    if let Err(e) = state
        .db
        .upsert_device_fingerprint(&id, &canonical, state.now())
        .await
    {
        // 存不了就不给 id：登录时服务端认不出来，会被误记成「疑似伪造」
        tracing::warn!(target: "web-login", "记设备指纹失败：{e}");
        return with_bid(no_content());
    }
    // 补充项和电量（站长 10-23）：按 (设备指纹, 浏览器标记) 另存，尽力而为
    let extra = canonical_device_extra(&payload);
    let battery = device_battery(&payload);
    if extra.is_some() || battery.is_some() {
        let (xid, data) = extra.unwrap_or_default();
        if let Err(e) = state
            .db
            .upsert_device_extra(&id, &bid, &data, &xid, battery, state.now())
            .await
        {
            tracing::warn!(target: "web-login", "记设备指纹补充项失败：{e}");
        }
    }
    with_bid(ok_json(&json!({"id": id, "bid": bid})))
}

/// 浏览器标记 Cookie：第一次打开本站时种下的长期随机 ID（前端在 localStorage 另存一份）。
/// 同一个浏览器换 Discord 账号登录时标记不变（设备指纹变了也一样）= 几乎可以确定是同一个人。防滥用用途。
const BROWSER_COOKIE: &str = "owl_bid";
/// 浏览器标记 Cookie 的有效期（Chrome 最多认 400 天）；每次请求 /public/device-fp 都会续期。
const BROWSER_COOKIE_DAYS: i64 = 400;

/// 浏览器标记的格式：32 位小写十六进制（16 个随机字节）。
fn valid_browser_id(b: &str) -> bool {
    b.len() == 32 && b.bytes().all(|c| matches!(c, b'0'..=b'9' | b'a'..=b'f'))
}

pub(super) fn browser_id(headers: &HeaderMap) -> Option<String> {
    session::cookie(headers, BROWSER_COOKIE).filter(|b| valid_browser_id(b))
}

fn issue_browser_cookie(headers: &HeaderMap, stored: Option<&str>) -> (String, HeaderValue) {
    let bid = browser_id(headers)
        .or_else(|| stored.filter(|b| valid_browser_id(b)).map(str::to_owned))
        .unwrap_or_else(|| hex::encode(rand::random::<[u8; 16]>()));
    let cookie = session::set_cookie_header(
        BROWSER_COOKIE,
        &bid,
        BROWSER_COOKIE_DAYS * 86400,
        true,
        "lax",
    );
    (bid, cookie)
}

/// 设备指纹最多收几项、每项的键 / 值最长多少字。
const FP_MAX_PARTS: usize = 32;
const FP_MAX_KEY: usize = 32;
const FP_MAX_VALUE: usize = 512;

/// 设备指纹的补充项（请求体的 `extra`，站长 10-23）：同样规整，返回 (sha256 前 16 位, 规整后的 JSON)。
/// 和 `data` 分开算：指纹 id 不受补充项影响。
fn canonical_device_extra(v: &Value) -> Option<(String, String)> {
    let (id, canonical) = canonical_parts(v.get("extra")?.as_object()?)?;
    Some((id[..16].to_string(), canonical))
}

/// 电量快照（请求体的 `battery: {level, charging}`）：level 须在 0–1 之间。
fn device_battery(v: &Value) -> Option<gate_db::repo::footprint::Battery> {
    let b = v.get("battery")?;
    let level = b
        .get("level")?
        .as_f64()
        .filter(|l| (0.0..=1.0).contains(l))?;
    let charging = b.get("charging")?.as_bool()?;
    Some(gate_db::repo::footprint::Battery { level, charging })
}

/// 规整一组指纹分项，返回 (sha256 前 32 位, 规整后的 JSON)。
fn canonical_parts(data: &serde_json::Map<String, Value>) -> Option<(String, String)> {
    use sha2::{Digest, Sha256};
    let mut out = std::collections::BTreeMap::new();
    for (k, val) in data {
        let key: String = k
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '_')
            .take(FP_MAX_KEY)
            .collect();
        if key.is_empty() {
            continue;
        }
        let text = match val {
            Value::String(s) => s.clone(),
            Value::Number(n) => n.to_string(),
            Value::Bool(b) => b.to_string(),
            Value::Null => String::new(),
            Value::Array(_) | Value::Object(_) => continue,
        };
        out.insert(key, take_chars(&text, FP_MAX_VALUE).to_string());
    }
    // 键排好序后截前 32 项：同样的输入永远得到同样的结果
    let out: std::collections::BTreeMap<String, String> =
        out.into_iter().take(FP_MAX_PARTS).collect();
    if out.values().all(String::is_empty) {
        return None;
    }
    let canonical = serde_json::to_string(&out).ok()?;
    let id = hex::encode(Sha256::digest(canonical.as_bytes()))[..32].to_string();
    Some((id, canonical))
}

#[cfg(test)]
mod tests {
    use super::netloc;

    #[test]
    fn netloc_like_urlsplit() {
        assert_eq!(netloc("https://Gate.Example.top"), "Gate.Example.top");
        assert_eq!(
            netloc("https://gate.example.top:8443/a?b#c"),
            "gate.example.top:8443"
        );
        assert_eq!(netloc("https://u:p@h.test/x"), "u:p@h.test");
        assert_eq!(netloc("null"), "");
        assert_eq!(netloc("//h.test/x"), "h.test");
        assert_eq!(netloc("h.test/x"), "");
        assert_eq!(netloc(" \thttps://h.te\nst/x"), "h.test");
    }
}
