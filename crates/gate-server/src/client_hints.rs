//! 请求的客户端特征（站长 10-23：防一人多号 / 共用 Key，要能从用量日志里举证）。
//!
//! 在 `middleware::request_context` 里和 UA / IP 一起登记，写用量日志时落到 `usage_log` 的六列
//! （见 `gate_db::repo::usage::ClientHints`），保留期和 UA / IP 相同（后台设置 usage_origin_keep_days，0 = 永久，默认永久）。
//!
//! 隐私：Authorization / Cookie 的原文一律不碰；浏览器标记 Cookie 只存哈希前 16 位。

use axum::http::HeaderMap;
use gate_core::primitives::take_chars;
use gate_db::repo::usage::ClientHints;
use sha2::{Digest, Sha256};

/// 浏览器标记 Cookie（`/public/device-fp` 种下的）。
const BROWSER_COOKIE: &str = crate::routes::public::BROWSER_COOKIE;

/// 请求头指纹里只取名字、不取值，而且连名字也不算进去的头：带身份 / 每次都变 / 由反代或 CDN 加的。
/// 反代加的头因部署而异，算进去会让同一个客户端换条线路就换指纹。
fn excluded(name: &str) -> bool {
    matches!(
        name,
        "authorization"
            | "cookie"
            | "content-length"
            | "host"
            | "x-real-ip"
            | "forwarded"
            | "via"
            | "cdn-loop"
            | "true-client-ip"
    ) || name.starts_with("x-forwarded-")
        || name.starts_with("cf-")
}

/// 值也算进指纹的头：稳定、不指向具体身份（同一个客户端每次都一样）。顺序固定。
const STABLE_VALUES: [&str; 9] = [
    "accept",
    "accept-encoding",
    "accept-language",
    "user-agent",
    "sec-ch-ua",
    "sec-ch-ua-mobile",
    "sec-ch-ua-platform",
    "x-requested-with",
    "origin",
];

fn text(h: &HeaderMap, name: &str) -> String {
    h.get(name)
        .map(|v| String::from_utf8_lossy(v.as_bytes()).trim().to_string())
        .unwrap_or_default()
}

fn sha16(s: &str) -> String {
    hex::encode(Sha256::digest(s.as_bytes()))[..16].to_string()
}

/// URL / Origin 只留 `scheme://host[:port]`（不要路径和查询串）；不像 URL 的原样返回（如 `null`）。
pub fn origin_only(url: &str) -> String {
    let url = url.trim();
    match url.split_once("://") {
        Some((scheme, rest)) => {
            let host = rest.split(['/', '?', '#']).next().unwrap_or("");
            // 去掉 userinfo（user:pass@host）
            let host = host.rsplit_once('@').map_or(host, |(_, h)| h);
            format!(
                "{}://{}",
                scheme.to_ascii_lowercase(),
                host.to_ascii_lowercase()
            )
        }
        None => url.to_string(),
    }
}

/// 请求头指纹：sha256 的前 16 位十六进制，输入是
/// 1. 请求头名字（小写），按收到的顺序、去重、去掉 [`excluded`] 的；
/// 2. [`STABLE_VALUES`] 里各头的值（Referer 不在其中，只取它的 origin 部分另算一项）。
///
/// 顺序：hyper 解析 HTTP/1 时按收到的顺序 `append` 进 `HeaderMap`，`HeaderMap` 的条目存在按插入顺序排列的
/// Vec 里，`keys()` 按首次出现的顺序遍历（同名多值只出一次）——所以这里拿到的就是客户端（或前面反代）发的顺序。
/// 不同的 HTTP 库发头的顺序不同，这正是指纹要区分的东西。
pub fn header_fingerprint(h: &HeaderMap) -> String {
    let mut buf = String::new();
    for name in h.keys() {
        let n = name.as_str();
        if !excluded(n) {
            buf.push_str(n);
            buf.push('\n');
        }
    }
    buf.push('\n');
    for n in STABLE_VALUES {
        buf.push_str(n);
        buf.push('=');
        buf.push_str(&text(h, n));
        buf.push('\n');
    }
    buf.push_str("referer=");
    let referer = text(h, "referer");
    if !referer.is_empty() {
        buf.push_str(&origin_only(&referer));
    }
    sha16(&buf)
}

/// 从请求头里取出要记的客户端特征。
pub fn from_headers(h: &HeaderMap) -> ClientHints {
    let chua = [
        text(h, "sec-ch-ua"),
        text(h, "sec-ch-ua-platform"),
        text(h, "sec-ch-ua-mobile"),
    ];
    let chua = if chua.iter().all(String::is_empty) {
        String::new()
    } else {
        chua.join(" | ")
    };
    let bid = crate::session::cookie(h, BROWSER_COOKIE)
        .filter(|v| !v.is_empty())
        .map(|v| sha16(&v))
        .unwrap_or_default();
    let origin = text(h, "origin");
    ClientHints {
        xrw: take_chars(&text(h, "x-requested-with"), 80).to_string(),
        lang: take_chars(&text(h, "accept-language"), 80).to_string(),
        chua: take_chars(&chua, 160).to_string(),
        hfp: header_fingerprint(h),
        bid,
        origin: if origin.is_empty() {
            String::new()
        } else {
            take_chars(&origin_only(&origin), 120).to_string()
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.append(
                axum::http::HeaderName::from_bytes(k.as_bytes()).unwrap(),
                HeaderValue::from_str(v).unwrap(),
            );
        }
        h
    }

    const BASE: &[(&str, &str)] = &[
        ("host", "owl.example"),
        ("user-agent", "Mozilla/5.0 (Linux; Android 14)"),
        ("accept", "*/*"),
        ("accept-language", "zh-CN,zh;q=0.9"),
        ("x-requested-with", "com.example.tavern"),
        ("content-type", "application/json"),
    ];

    #[test]
    fn hfp_is_stable_and_ignores_identity_and_proxy_headers() {
        let a = header_fingerprint(&headers(BASE));
        assert_eq!(a.len(), 16);
        assert!(a.bytes().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(
            a,
            header_fingerprint(&headers(BASE)),
            "同样的请求头 → 同一个指纹"
        );

        // 换 Key / Cookie / 长度 / 反代和 CDN 加的头 / Host：指纹不变
        let mut more = BASE.to_vec();
        more.insert(0, ("authorization", "Bearer nai-a-secret"));
        more.insert(2, ("cookie", "owl_bid=0123; owl_sess=x"));
        more.push(("content-length", "1234"));
        more.push(("x-forwarded-for", "203.0.113.7"));
        more.push(("x-forwarded-proto", "https"));
        more.push(("cf-connecting-ip", "203.0.113.7"));
        more.push(("cf-ray", "abc"));
        more.push(("x-real-ip", "203.0.113.7"));
        let mut b = headers(&more);
        b.insert("host", HeaderValue::from_static("other.example"));
        assert_eq!(header_fingerprint(&b), a);

        // Referer 只取 origin：路径不同不影响
        let mut r1 = BASE.to_vec();
        r1.push(("referer", "https://app.example/page/1?q=1"));
        let mut r2 = BASE.to_vec();
        r2.push(("referer", "https://app.example/other"));
        assert_eq!(
            header_fingerprint(&headers(&r1)),
            header_fingerprint(&headers(&r2))
        );
        assert_ne!(header_fingerprint(&headers(&r1)), a);
    }

    #[test]
    fn hfp_changes_with_header_order_names_and_stable_values() {
        let a = header_fingerprint(&headers(BASE));
        let mut swapped = BASE.to_vec();
        swapped.swap(1, 2);
        assert_ne!(header_fingerprint(&headers(&swapped)), a, "顺序不同");
        let mut extra = BASE.to_vec();
        extra.push(("x-stainless-lang", "js"));
        assert_ne!(header_fingerprint(&headers(&extra)), a, "多一个头");
        let mut ua = BASE.to_vec();
        ua[1] = ("user-agent", "axios/1.18.0");
        assert_ne!(header_fingerprint(&headers(&ua)), a, "UA 不同");
        // content-type 只算名字不算值（multipart 的 boundary 每次都变）
        let mut ct = BASE.to_vec();
        ct[5] = ("content-type", "multipart/form-data; boundary=xyz");
        assert_eq!(header_fingerprint(&headers(&ct)), a);
    }

    #[test]
    fn hints_are_extracted_trimmed_and_never_hold_raw_secrets() {
        let mut pairs = BASE.to_vec();
        pairs.extend([
            ("authorization", "Bearer nai-a-secret"),
            ("cookie", "a=1; owl_bid=0123456789abcdef0123456789abcdef"),
            ("sec-ch-ua", "\"Chromium\";v=\"130\""),
            ("sec-ch-ua-platform", "\"Android\""),
            ("sec-ch-ua-mobile", "?1"),
            ("origin", "https://App.Example:8443"),
        ]);
        let h = from_headers(&headers(&pairs));
        assert_eq!(h.xrw, "com.example.tavern");
        assert_eq!(h.lang, "zh-CN,zh;q=0.9");
        assert_eq!(h.chua, "\"Chromium\";v=\"130\" | \"Android\" | ?1");
        assert_eq!(h.origin, "https://app.example:8443");
        assert_eq!(h.bid, sha16("0123456789abcdef0123456789abcdef"));
        assert_eq!(h.bid.len(), 16);
        let all = format!("{h:?}");
        assert!(!all.contains("secret") && !all.contains("0123456789abcdef0123"));

        // 没有这些头：全空（hfp 除外）
        let none = from_headers(&headers(&[("user-agent", "curl/8")]));
        assert_eq!(
            (none.xrw, none.lang, none.chua, none.bid, none.origin),
            Default::default()
        );
        // 超长截断
        let long = "x".repeat(300);
        let h = from_headers(&headers(&[
            ("x-requested-with", &long),
            ("accept-language", &long),
        ]));
        assert_eq!((h.xrw.len(), h.lang.len()), (80, 80));
    }

    #[test]
    fn origin_only_drops_path_query_and_userinfo() {
        assert_eq!(origin_only("https://a.b/c?d#e"), "https://a.b");
        assert_eq!(origin_only("HTTP://u:p@A.B:81/x"), "http://a.b:81");
        assert_eq!(origin_only("null"), "null");
    }
}
