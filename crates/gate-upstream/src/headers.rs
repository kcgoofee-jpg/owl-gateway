//! 发往 NovelAI 的请求头：浏览器层的头取自 wreq-util 的 Chrome emulation——与 TLS / HTTP2 指纹是**同一个
//! profile**，不手写 UA；我们只补请求相关的头（站长 10-15 批准指纹统一升级）。
//!
//! ## 指纹池
//! 按上游 Token 的序号轮流分配（`index % 3`）：Chrome 149 / Windows、Chrome 148 / Windows、Chrome 149 / macOS
//! ——wreq-util 0.2.0 里最新与次新的 Chrome。UA、`sec-ch-ua`（版本与品牌串）、`sec-ch-ua-platform`、
//! `Accept-Encoding`、`Accept-Language` 都从这个 profile 的 emulation 里读出来，TLS / HTTP2 也用它建客户端。
//!
//! ## 顺序（HTTP/2，线上实际走的协议）
//! ```text
//! :method :authority :scheme :path   伪头，顺序由 emulation 给（m, a, s, p）
//! content-length                     有正文时
//! sec-ch-ua-platform                 emulation
//! authorization                      Bearer <Token>
//! user-agent                         emulation
//! accept                             各接口原来的值（出图 */*、流式 application/x-msgpack 或 text/event-stream、查询 application/json）
//! sec-ch-ua                          emulation
//! content-type                       有正文时：application/json
//! sec-ch-ua-mobile                   emulation（?0）
//! origin                             https://novelai.net
//! sec-fetch-site                     same-site
//! sec-fetch-mode                     cors
//! sec-fetch-dest                     empty
//! referer                            https://novelai.net/
//! accept-encoding                    emulation（gzip, deflate, br, zstd；四种都能解压）
//! accept-language                    emulation（en-US,en;q=0.9）
//! priority                           u=1, i（emulation 带这个头才发；值是 fetch 的，见下）
//! ```
//! HTTP/1.1 只在本地测试里出现（假上游是明文）：同样的顺序，最前面是 `Host`；不发 `Connection`。
//!
//! ## 依据
//! 1. 实测：2026-10-15 在本机 Chromium 152（Claude 桌面应用的内置浏览器）里，从 `http://127.0.0.1:A` 的页面对
//!    `http://127.0.0.1:B` 发 `fetch()`——与 novelai.net → image.novelai.net 一样是同站跨源
//!    （`Sec-Fetch-Site: same-site`、`cors`、`empty`）——带 `Authorization`、`Content-Type: application/json`、
//!    `Accept`，原始套接字抓到的 HTTP/1.1 顺序是
//!    `Host, Connection, Content-Length, sec-ch-ua-platform, Authorization, User-Agent, Accept, sec-ch-ua,
//!    Content-Type, sec-ch-ua-mobile, Origin, Sec-Fetch-Site, Sec-Fetch-Mode, Sec-Fetch-Dest, Referer,
//!    Accept-Encoding, Accept-Language`；GET（无正文、带 `Accept`）同序，只少 Content-Length / Content-Type。
//!    前半段客户端提示与页面设置的头交错，是 Blink 请求头表的散列顺序：头名集合相同，顺序就固定。
//!    大小写（`sec-ch-ua*` 小写、其余首字母大写）也照抄这次抓包；HTTP/2 下一律小写。
//! 2. Chromium 网络栈：`HttpNetworkTransaction::BuildRequestHeaders` 先放 Host、Connection、Content-Length 再合并上面
//!    那些头；HTTP/2 由 `CreateSpdyHeadersFromHttpRequest` 去掉 Host / Connection（content-length 仍在最前），
//!    最后追加 RFC 9218 的 `priority`（HTTP/1.1 不发）。fetch() 默认优先级 High → `u=1`，可增量 → `i`。
//!    emulation 自带的 `priority: u=0, i`、`Sec-Fetch-*: navigate/document/none`、`Sec-Fetch-User`、
//!    `Upgrade-Insecure-Requests` 和 HTML 的 `Accept` 是**地址栏导航**的头，fetch() 不发，所以不用。
//!
//! 不模拟的：CORS 预检。真实浏览器带 `Authorization` 时会先发 OPTIONS（预检缓存有效期内不再发），网关不发。

use std::sync::OnceLock;
use std::time::Duration;
use wreq::IntoEmulation;
use wreq::header::{HeaderMap, HeaderName, HeaderValue, OrigHeaderMap};

/// 指纹池：(Chrome 版本, 平台)。按上游 Token 的序号轮流分配；两个 Windows + 一个 macOS。
pub const POOL: [(wreq_util::Profile, wreq_util::Platform); 3] = [
    (wreq_util::Profile::Chrome149, wreq_util::Platform::Windows),
    (wreq_util::Profile::Chrome148, wreq_util::Platform::Windows),
    (wreq_util::Profile::Chrome149, wreq_util::Platform::MacOS),
];

/// fetch() 的 RFC 9218 优先级（emulation 里的 `u=0, i` 是导航的）。
pub const FETCH_PRIORITY: &str = "u=1, i";
pub const ORIGIN: &str = "https://novelai.net";
pub const REFERER: &str = "https://novelai.net/";

/// 一个浏览器指纹：TLS / HTTP2（[`BrowserProfile::emulation`]）与浏览器层的请求头出自同一个 wreq-util profile。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowserProfile {
    pub chrome: wreq_util::Profile,
    pub platform: wreq_util::Platform,
    pub user_agent: String,
    pub sec_ch_ua: String,
    pub sec_ch_ua_mobile: String,
    pub sec_ch_ua_platform: String,
    pub accept_encoding: String,
    pub accept_language: String,
    /// emulation 带不带 `priority`（wreq-util 里 Chrome 129 以后的 profile 都带）。
    pub priority: bool,
}

impl BrowserProfile {
    /// 从 emulation 的默认头里读出浏览器层的值。
    pub fn new(chrome: wreq_util::Profile, platform: wreq_util::Platform) -> Self {
        let defaults = wreq_util::Emulation::builder()
            .profile(chrome)
            .platform(platform)
            .headers(true)
            .build()
            .into_emulation()
            .headers;
        let get = |name: &str| {
            defaults
                .get(name)
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default()
                .to_string()
        };
        Self {
            chrome,
            platform,
            user_agent: get("user-agent"),
            sec_ch_ua: get("sec-ch-ua"),
            sec_ch_ua_mobile: get("sec-ch-ua-mobile"),
            sec_ch_ua_platform: get("sec-ch-ua-platform"),
            accept_encoding: get("accept-encoding"),
            accept_language: get("accept-language"),
            priority: defaults.contains_key("priority"),
        }
    }

    /// 建客户端用的 emulation：TLS / HTTP2 参数（Chrome 都走 HTTP/2，靠 ALPN 协商），不带默认头——
    /// 头由 [`ordered`] 每次给全，免得混进导航用的头。
    pub fn emulation(&self) -> wreq_util::Emulation {
        wreq_util::Emulation::builder()
            .profile(self.chrome)
            .platform(self.platform)
            .headers(false)
            .build()
    }

    /// 上游客户端的构建器：本指纹的 TLS / HTTP2 + 连接参数（代理由调用方加）。
    pub fn client_builder(&self) -> wreq::ClientBuilder {
        wreq::Client::builder()
            .emulation(self.emulation())
            .connect_timeout(Duration::from_secs(15))
            .read_timeout(Duration::from_secs(300))
            // 旧版 keepalive_expiry=120：出图间隔 15 秒，默认 5 秒会让每张图都重新 TLS 握手
            .pool_idle_timeout(Duration::from_secs(120))
            .pool_max_idle_per_host(8)
            .redirect(wreq::redirect::Policy::limited(20))
    }

    /// Chrome 主版本号，取自 profile 名（`Chrome149` → 149）：TLS / HTTP2 指纹的版本。
    pub fn tls_major(&self) -> Option<u32> {
        format!("{:?}", self.chrome)
            .strip_prefix("Chrome")?
            .parse()
            .ok()
    }
}

/// 整个指纹池（启动时从 emulation 读一次）。
pub fn browser_profiles() -> &'static [BrowserProfile; 3] {
    static POOL_PROFILES: OnceLock<[BrowserProfile; 3]> = OnceLock::new();
    POOL_PROFILES
        .get_or_init(|| POOL.map(|(chrome, platform)| BrowserProfile::new(chrome, platform)))
}

/// 第 `index` 把 Token 的指纹（`index % 3`）。
pub fn profile_for(index: usize) -> BrowserProfile {
    browser_profiles()[index % POOL.len()].clone()
}

/// 一次请求的有序头列表（不含 Host / Content-Length，这两个由协议层按位置补上，见 [`to_wreq`]）。
/// `has_body`：POST 等带 JSON 体的请求加 `Content-Type: application/json`；GET / HEAD 不加。
pub fn ordered(
    profile: &BrowserProfile,
    token: Option<&str>,
    accept: &str,
    has_body: bool,
) -> Vec<(&'static str, String)> {
    let mut h: Vec<(&'static str, String)> = Vec::with_capacity(16);
    h.push(("sec-ch-ua-platform", profile.sec_ch_ua_platform.clone()));
    if let Some(t) = token {
        h.push(("Authorization", format!("Bearer {t}")));
    }
    h.push(("User-Agent", profile.user_agent.clone()));
    h.push(("Accept", accept.into()));
    h.push(("sec-ch-ua", profile.sec_ch_ua.clone()));
    if has_body {
        h.push(("Content-Type", "application/json".into()));
    }
    h.push(("sec-ch-ua-mobile", profile.sec_ch_ua_mobile.clone()));
    h.push(("Origin", ORIGIN.into()));
    h.push(("Sec-Fetch-Site", "same-site".into()));
    h.push(("Sec-Fetch-Mode", "cors".into()));
    h.push(("Sec-Fetch-Dest", "empty".into()));
    h.push(("Referer", REFERER.into()));
    h.push(("Accept-Encoding", profile.accept_encoding.clone()));
    h.push(("Accept-Language", profile.accept_language.clone()));
    if profile.priority {
        h.push(("priority", FETCH_PRIORITY.into()));
    }
    h
}

/// 转成 wreq 需要的 (HeaderMap, OrigHeaderMap)：后者固定大小写与顺序。
/// `Host`（只有 HTTP/1.1 有）与 `Content-Length` 排最前，与 Chrome 一致。
pub fn to_wreq(list: &[(&'static str, String)]) -> (HeaderMap, OrigHeaderMap) {
    let mut map = HeaderMap::with_capacity(list.len());
    let mut orig = OrigHeaderMap::with_capacity(list.len() + 2);
    orig.insert("Host");
    orig.insert("Content-Length");
    for (name, value) in list {
        let n = HeaderName::from_bytes(name.as_bytes()).expect("请求头名是固定的合法名字");
        map.insert(
            n,
            HeaderValue::from_str(value).unwrap_or_else(|_| HeaderValue::from_static("")),
        );
        orig.insert(*name);
    }
    (map, orig)
}

/// 上游 Token 的不可逆短标识：`token-` + sha256 前 16 位（永远不把原始 Token 写进库或日志）。
pub fn token_id(token: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(token.as_bytes());
    format!("token-{}", &hex::encode(digest)[..16])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_id_matches_python() {
        // python: "token-" + hashlib.sha256(b"pst-abc").hexdigest()[:16]
        assert_eq!(token_id("pst-abc"), "token-249b0fb7a2455213");
    }

    #[test]
    fn values_come_from_emulation() {
        let p = profile_for(0);
        assert_eq!(p.chrome, wreq_util::Profile::Chrome149);
        assert_eq!(p.sec_ch_ua_platform, r#""Windows""#);
        assert_eq!(p.sec_ch_ua_mobile, "?0");
        assert_eq!(p.accept_encoding, "gzip, deflate, br, zstd");
        assert_eq!(p.accept_language, "en-US,en;q=0.9");
        assert!(p.priority);
        assert_eq!(profile_for(2).sec_ch_ua_platform, r#""macOS""#);
        assert_eq!(profile_for(3), profile_for(0), "按序号轮换");
    }
}
