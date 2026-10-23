//! 线上抓包对拍：wreq 实际发出的请求头集合、顺序、取值，以及指纹版本前后一致。
//!
//! 期望顺序来自 2026-10-15 在本机 Chromium 152 里对同站跨源地址发 `fetch()` 的原始套接字抓包
//! （依据与完整顺序见 `src/headers.rs` 文件头）。HTTP/1.1 用原始套接字抓；HTTP/2 用开了 h2c 的假上游抓
//! （服务端收到的 HeaderMap 保留帧里的先后顺序）。

use gate_testkit::MockUpstream;
use gate_upstream::headers::{BrowserProfile, browser_profiles, ordered, profile_for, to_wreq};
use std::io::{Read, Write};
use std::net::TcpListener;

const TOKEN: &str = "tok-abc";

/// 起一个只收一个请求的原始套接字服务，返回收到的请求头原文。
fn capture(f: impl FnOnce(u16) + Send + 'static) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let client = std::thread::spawn(move || f(port));
    let (mut s, _) = listener.accept().unwrap();
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
        let n = s.read(&mut chunk).unwrap();
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    s.write_all(
        b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nContent-Type: application/json\r\n\r\n{}",
    )
    .unwrap();
    client.join().unwrap();
    String::from_utf8(buf).unwrap()
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

/// 与网关同样的组法：指纹档位 0 的客户端 + 同一档位的请求头。
fn request(
    client: &wreq::Client,
    p: &BrowserProfile,
    method: wreq::Method,
    url: String,
    body: Option<&[u8]>,
    accept: &str,
) -> wreq::RequestBuilder {
    let (map, orig) = to_wreq(&ordered(p, Some(TOKEN), accept, body.is_some()));
    let mut req = client
        .request(method, url)
        .default_headers(false)
        .headers(map)
        .orig_headers(orig);
    if let Some(b) = body {
        req = req.body(b.to_vec());
    }
    req
}

fn send_h1(port: u16, method: wreq::Method, path: &str, body: Option<&[u8]>, accept: &str) {
    runtime().block_on(async move {
        let p = profile_for(0);
        let client = p.client_builder().build().unwrap();
        request(
            &client,
            &p,
            method,
            format!("http://127.0.0.1:{port}{path}"),
            body,
            accept,
        )
        .send()
        .await
        .unwrap()
        .bytes()
        .await
        .unwrap();
    });
}

fn header_lines(raw: &str) -> Vec<String> {
    let head = raw.split("\r\n\r\n").next().unwrap_or("");
    head.split("\r\n")
        .filter(|l| !l.is_empty())
        .map(|l| l.replace(TOKEN, "<TOKEN>"))
        .map(|l| {
            if l.starts_with("Host: ") {
                "Host: 127.0.0.1:PORT".to_string()
            } else {
                l
            }
        })
        .collect()
}

const UA_149_WIN: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/149.0.0.0 Safari/537.36";
const SEC_CH_UA_149: &str = r#""Google Chrome";v="149", "Chromium";v="149", "Not)A;Brand";v="24""#;

#[test]
fn post_wire_order_matches_chrome_fetch() {
    let raw = capture(|port| {
        send_h1(
            port,
            wreq::Method::POST,
            "/ai/generate-image",
            Some(br#"{"b":1,"a":[1.0,"x"]}"#),
            "*/*",
        )
    });
    let expected = [
        "POST /ai/generate-image HTTP/1.1".to_string(),
        "Host: 127.0.0.1:PORT".into(),
        "Content-Length: 21".into(),
        r#"sec-ch-ua-platform: "Windows""#.into(),
        "Authorization: Bearer <TOKEN>".into(),
        format!("User-Agent: {UA_149_WIN}"),
        "Accept: */*".into(),
        format!("sec-ch-ua: {SEC_CH_UA_149}"),
        "Content-Type: application/json".into(),
        "sec-ch-ua-mobile: ?0".into(),
        "Origin: https://novelai.net".into(),
        "Sec-Fetch-Site: same-site".into(),
        "Sec-Fetch-Mode: cors".into(),
        "Sec-Fetch-Dest: empty".into(),
        "Referer: https://novelai.net/".into(),
        "Accept-Encoding: gzip, deflate, br, zstd".into(),
        "Accept-Language: en-US,en;q=0.9".into(),
        "priority: u=1, i".into(),
    ];
    assert_eq!(header_lines(&raw), expected, "\n实际：\n{raw}");
}

#[test]
fn get_wire_order_has_no_body_headers() {
    let raw = capture(|port| {
        send_h1(
            port,
            wreq::Method::GET,
            "/ai/generate-image/suggest-tags?model=m&prompt=p",
            None,
            "application/json",
        )
    });
    let names: Vec<String> = header_lines(&raw)[1..]
        .iter()
        .map(|l| l.split(':').next().unwrap().to_string())
        .collect();
    assert_eq!(
        names,
        [
            "Host",
            "sec-ch-ua-platform",
            "Authorization",
            "User-Agent",
            "Accept",
            "sec-ch-ua",
            "sec-ch-ua-mobile",
            "Origin",
            "Sec-Fetch-Site",
            "Sec-Fetch-Mode",
            "Sec-Fetch-Dest",
            "Referer",
            "Accept-Encoding",
            "Accept-Language",
            "priority",
        ],
        "{raw}"
    );
    assert!(raw.contains("\r\nAccept: application/json\r\n"), "{raw}");
}

/// HTTP/2（线上实际走的协议）：伪头之后 content-length 在最前，没有 host / connection，最后是 priority。
#[tokio::test]
async fn http2_wire_order() {
    let up = MockUpstream::start().await;
    for (i, p) in browser_profiles().iter().enumerate() {
        // 明文 h2c 需要「先验知识」模式；TLS / HTTP2 参数仍是这一档指纹的
        let client = p.client_builder().http2_only().build().unwrap();
        let resp = request(
            &client,
            p,
            wreq::Method::POST,
            format!("{}/ai/encode-vibe", up.url()),
            Some(b"{}"),
            "*/*",
        )
        .send()
        .await
        .unwrap();
        assert_eq!(resp.version(), wreq::Version::HTTP_2);
        resp.bytes().await.unwrap();
        let rec = up.requests_to("/ai/encode-vibe").pop().unwrap();
        let names: Vec<&str> = rec.headers.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(
            names,
            [
                "content-length",
                "sec-ch-ua-platform",
                "authorization",
                "user-agent",
                "accept",
                "sec-ch-ua",
                "content-type",
                "sec-ch-ua-mobile",
                "origin",
                "sec-fetch-site",
                "sec-fetch-mode",
                "sec-fetch-dest",
                "referer",
                "accept-encoding",
                "accept-language",
                "priority",
            ],
            "指纹档位 {i}"
        );
        assert_eq!(rec.header("user-agent"), Some(p.user_agent.as_str()));
        assert_eq!(rec.header("sec-ch-ua"), Some(p.sec_ch_ua.as_str()));
        assert_eq!(rec.header("priority"), Some("u=1, i"));
    }
}

/// `"Google Chrome";v="149", "Chromium";v="149", …` 里某个品牌的版本。
fn brand_version(sec_ch_ua: &str, brand: &str) -> Option<u32> {
    sec_ch_ua.split(", ").find_map(|item| {
        let (name, v) = item.split_once(";v=")?;
        (name.trim_matches('"') == brand)
            .then(|| v.trim_matches('"').parse().ok())
            .flatten()
    })
}

/// UA 里的 Chrome 主版本号。
fn ua_major(ua: &str) -> Option<u32> {
    let rest = ua.split("Chrome/").nth(1)?;
    rest.split('.').next()?.parse().ok()
}

/// 指纹前后一致：UA 版本 = sec-ch-ua 版本（Google Chrome 与 Chromium 两个品牌）= TLS / HTTP2 profile 版本，
/// 平台三处一致；指纹池用的是 wreq-util 里最新（与次新）的 Chrome。
#[test]
fn fingerprint_versions_agree() {
    let newest = wreq_util::Profile::VARIANTS
        .iter()
        .filter_map(|p| format!("{p:?}").strip_prefix("Chrome")?.parse::<u32>().ok())
        .max()
        .unwrap();
    let mut windows = 0;
    for (i, p) in browser_profiles().iter().enumerate() {
        let tls = p.tls_major().expect("Chrome profile");
        assert_eq!(ua_major(&p.user_agent), Some(tls), "档位 {i} UA：{p:?}");
        assert_eq!(
            brand_version(&p.sec_ch_ua, "Google Chrome"),
            Some(tls),
            "档位 {i} sec-ch-ua：{p:?}"
        );
        assert_eq!(
            brand_version(&p.sec_ch_ua, "Chromium"),
            Some(tls),
            "档位 {i} sec-ch-ua：{p:?}"
        );
        assert!(
            tls == newest || tls + 1 == newest,
            "档位 {i} 用的是 Chrome {tls}，wreq-util 里最新是 {newest}"
        );
        match p.sec_ch_ua_platform.as_str() {
            r#""Windows""# => {
                windows += 1;
                assert!(
                    p.user_agent.contains("Windows NT 10.0; Win64; x64"),
                    "{p:?}"
                );
            }
            r#""macOS""# => assert!(p.user_agent.contains("Macintosh; Intel Mac OS X"), "{p:?}"),
            other => panic!("档位 {i} 平台 {other}"),
        }
        assert_eq!(p.sec_ch_ua_mobile, "?0");
        // 请求头就是从这一档 emulation 读出来的：重建一次完全相同
        assert_eq!(*p, BrowserProfile::new(p.chrome, p.platform));
    }
    assert_eq!(browser_profiles()[0].tls_major(), Some(newest));
    assert_eq!(windows, 2, "两个 Windows + 一个 macOS");
}

// ---------------------------------------------------------------- TLS ClientHello

/// 读 TLS 记录层里的第一条握手消息（ClientHello）。
fn client_hello(port_host: &str) -> Vec<u8> {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let host = port_host.to_string();
    let client = std::thread::spawn(move || {
        runtime().block_on(async move {
            let client = profile_for(0)
                .client_builder()
                // 本机若设了 HTTPS_PROXY，wreq 会走代理隧道：这里要直连到抓包的端口
                .no_proxy()
                .resolve(host.clone(), addr)
                .build()
                .unwrap();
            // 服务端读完 ClientHello 就关连接：这里必然失败
            let _ = client
                .get(format!("https://{host}:{}/", addr.port()))
                .send()
                .await;
        })
    });
    let (mut s, _) = listener.accept().unwrap();
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        if buf.len() >= 5 {
            let len = u16::from_be_bytes([buf[3], buf[4]]) as usize;
            if buf.len() >= 5 + len {
                break;
            }
        }
        let n = s.read(&mut chunk).unwrap();
        assert!(n > 0, "连接提前关闭");
        buf.extend_from_slice(&chunk[..n]);
    }
    drop(s);
    client.join().unwrap();
    assert_eq!(buf[0], 0x16, "TLS 握手记录");
    let len = u16::from_be_bytes([buf[3], buf[4]]) as usize;
    let hs = buf[5..5 + len].to_vec();
    assert_eq!(hs[0], 0x01, "ClientHello");
    hs[4..].to_vec()
}

fn is_grease(v: u16) -> bool {
    v & 0x0f0f == 0x0a0a && (v >> 8) == (v & 0xff)
}

struct Hello {
    ciphers: Vec<u16>,
    extensions: Vec<(u16, Vec<u8>)>,
}

fn parse_hello(b: &[u8]) -> Hello {
    let mut i = 2 + 32; // legacy_version + random
    i += 1 + b[i] as usize; // session id
    let n = u16::from_be_bytes([b[i], b[i + 1]]) as usize;
    let ciphers = b[i + 2..i + 2 + n]
        .chunks(2)
        .map(|c| u16::from_be_bytes([c[0], c[1]]))
        .collect();
    i += 2 + n;
    i += 1 + b[i] as usize; // compression methods
    let end = i + 2 + u16::from_be_bytes([b[i], b[i + 1]]) as usize;
    i += 2;
    let mut extensions = Vec::new();
    while i < end {
        let t = u16::from_be_bytes([b[i], b[i + 1]]);
        let l = u16::from_be_bytes([b[i + 2], b[i + 3]]) as usize;
        extensions.push((t, b[i + 4..i + 4 + l].to_vec()));
        i += 4 + l;
    }
    Hello {
        ciphers,
        extensions,
    }
}

fn sha12(s: &str) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(s.as_bytes()))[..12].to_string()
}

/// JA4（FoxIO）：`t` + TLS 版本 + SNI + 密码套件数 + 扩展数 + ALPN 首尾字符 _ 套件哈希 _ 扩展与签名算法哈希。
fn ja4(h: &Hello) -> String {
    let ext = |t: u16| h.extensions.iter().find(|(x, _)| *x == t).map(|(_, d)| d);
    let ciphers: Vec<u16> = h
        .ciphers
        .iter()
        .copied()
        .filter(|c| !is_grease(*c))
        .collect();
    let exts: Vec<u16> = h
        .extensions
        .iter()
        .map(|(t, _)| *t)
        .filter(|t| !is_grease(*t))
        .collect();
    let versions = ext(0x002b).expect("supported_versions");
    let best = versions[1..]
        .chunks(2)
        .map(|c| u16::from_be_bytes([c[0], c[1]]))
        .filter(|v| !is_grease(*v))
        .max()
        .unwrap();
    let version = if best == 0x0304 { "13" } else { "12" };
    let sni = if ext(0x0000).is_some() { "d" } else { "i" };
    let alpn = ext(0x0010).map_or("00".to_string(), |d| {
        let first = &d[3..3 + d[2] as usize];
        format!("{}{}", first[0] as char, first[first.len() - 1] as char)
    });
    let mut sorted_ciphers: Vec<String> = ciphers.iter().map(|c| format!("{c:04x}")).collect();
    sorted_ciphers.sort();
    let mut sorted_exts: Vec<String> = exts
        .iter()
        .filter(|t| **t != 0x0000 && **t != 0x0010)
        .map(|t| format!("{t:04x}"))
        .collect();
    sorted_exts.sort();
    let sigs = ext(0x000d).expect("signature_algorithms");
    let sigs: Vec<String> = sigs[2..]
        .chunks(2)
        .map(|c| format!("{:04x}", u16::from_be_bytes([c[0], c[1]])))
        .collect();
    format!(
        "t{version}{sni}{:02}{:02}{alpn}_{}_{}",
        ciphers.len(),
        exts.len(),
        sha12(&sorted_ciphers.join(",")),
        sha12(&format!("{}_{}", sorted_exts.join(","), sigs.join(",")))
    )
}

/// 网关的上游客户端发出的 ClientHello 是 Chrome 的：GREASE、Chrome 的 15 个密码套件（顺序也一样）、
/// ALPN h2 优先、证书压缩、ECH GREASE。
#[test]
fn tls_client_hello_is_chrome() {
    let raw = client_hello("image.novelai.test");
    let h = parse_hello(&raw);
    assert!(is_grease(h.ciphers[0]), "第一个密码套件是 GREASE");
    let ciphers: Vec<u16> = h
        .ciphers
        .iter()
        .copied()
        .filter(|c| !is_grease(*c))
        .collect();
    assert_eq!(
        ciphers,
        [
            0x1301, 0x1302, 0x1303, 0xc02b, 0xc02f, 0xc02c, 0xc030, 0xcca9, 0xcca8, 0xc013, 0xc014,
            0x009c, 0x009d, 0x002f, 0x0035
        ]
    );
    let has = |t: u16| h.extensions.iter().any(|(x, _)| *x == t);
    assert!(has(0x0000), "SNI");
    assert!(has(0x001b), "证书压缩");
    assert!(has(0xfe0d), "ECH GREASE");
    let alpn = &h.extensions.iter().find(|(t, _)| *t == 0x0010).unwrap().1;
    assert_eq!(&alpn[2..], b"\x02h2\x08http/1.1");
    // 第二段 8daaf6152771 是 Chrome 这 15 个套件的哈希；整串是 wreq-util 0.2.0 Chrome 132+ TLS 参数的快照
    // （三档指纹共用，这里核对第 0 档）。升级 wreq-util 后这里变了，要先确认真 Chrome 也是这么变的。
    assert_eq!(ja4(&h), "t13d1516h2_8daaf6152771_d8a2da3f94cd");
}
