//! 关联扫描（站长 10-23，和小号扫描一起每 6 小时跑一次）：用新记下的足迹找「两个账号其实是同一个人」/
//! 「一把 Key 多人在用」，写进 `abuse_signals`，**不自动封号**。只看持有启用 Key 的账号。
//!
//! | kind | 信号 | 分数 |
//! |---|---|---|
//! | `ip_switch` | 近 24 小时同一个 IP 上 10 分钟内换了另一个账号的 Key（当天 ≥5 把 Key 用过的 IP 是公共出口，不算） | 20 起，每多一次 +8，最高 60 |
//! | `key_concurrent` | 一把 Key 近 24 小时里 ≥3 次在 60 秒内出现在两个不同网段（IPv4 /16，IPv6 前两组） | 40 起，最高 70 |
//! | `same_ref_image` | 近 14 天不同账号的 Key 上传了同一张参考图（sha256 相同），或几乎一样（dHash 汉明距离 ≤6）；≥5 把 Key 用过的图不算 | 完全相同 70 / 相近 55 起，最高 90 |
//! | `same_connection` | 同一个外部账号（Steam / GitHub …）绑在多个 Discord 账号上 | 100 |
//! | `guild_overlap` | 两个账号所在服务器的 Jaccard ≥0.6，且共同的冷门服务器（本站记录里不到 5 个账号在）≥2 个 | 50 起，最高 80 |
//! | `same_battery` | 两个账号 30 分钟内网页登录时电量和充电状态一模一样（满电 100% 太常见，不算） | 50 起，最高 80 |
//! | `foreign_origin` | 一把 Key 近 14 天的请求带着第三方网站的 Origin（Key 被填进了别的网站，会流经对方服务器）；已知域（本站 + `https://novelai.net` + 社区公益站，精确匹配）之外都算 | 30（只提醒，不封） |

use super::ScanSnapshot;
use super::{DAY, Signal, ev, subject};
use crate::clock::Clock;
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

/// 同 IP 换号：多少秒内算「换号」
const SWITCH_SECS: f64 = 600.0;
/// 一天里有这么多把 Key 用过的 IP 是公共出口（VPN / 运营商 NAT / 学校），不参与「同 IP 换号」
const SHARED_IP_KEYS: usize = 5;
/// 一把 Key 多少秒内出现在两个网段算一次「同时异地」，一天至少几次才出信号
const CONCURRENT_SECS: f64 = 60.0;
const CONCURRENT_MIN: usize = 3;
/// 参考图：看最近几天；几把 Key 以上用过的图不算（公开素材）；dHash 汉明距离多少以内算「几乎一样」
const REF_MAX_KEYS: usize = 5;
const DHASH_MAX_DIST: u32 = 6;
/// 服务器重合：Jaccard 门槛；冷门服务器 = 本站记录里不到这么多账号在；至少几个共同冷门服务器
const GUILD_JACCARD: f64 = 0.6;
const RARE_GUILD_MEMBERS: usize = 5;
const RARE_GUILD_MIN: usize = 2;
/// 电量：多少秒内算「同时」
const BATTERY_SECS: f64 = 1800.0;
/// 已知的非本站 origin（精确匹配，不猜子域）：这些来源的请求不算第三方，不出信号。
/// 本站的 origin（`SITE_URL`）由调用方传入，不在这个名单里。
const KNOWN_ORIGINS: [&str; 2] = [
    // 官方站
    "https://novelai.net",
    // 站长 10-24 确认的社区公益站
    "https://novelai-draw.minijiu.cfd",
];

/// 把 Origin 归一成 `scheme://host`（小写、去端口、去路径）。识别不了返回 None。
/// 官方桌面端是 `tauri://localhost` / `http://tauri.localhost`，本机反代是 `http://127.0.0.1:8000` /
/// `http://192.168.x.x:8000`，这些请求根本没出本机，不能算「Key 流经第三方网站」。
fn normalize_origin(origin: &str) -> Option<String> {
    let s = origin.trim().to_ascii_lowercase();
    if s.is_empty() {
        return None;
    }
    if let Some(rest) = s.strip_prefix("tauri://") {
        return Some(format!("tauri://{}", rest.split('/').next().unwrap_or("")));
    }
    if s.starts_with("file://") {
        return Some("file://".to_string());
    }
    let (scheme, rest) = s.split_once("://")?;
    if !matches!(scheme, "http" | "https") {
        return Some(s);
    }
    let host = rest.split('/').next().unwrap_or("");
    let host = if host.starts_with('[') {
        host.split(']')
            .next()
            .map(|h| format!("{h}]"))
            .unwrap_or_default()
    } else {
        host.split(':').next().unwrap_or("").to_string()
    };
    if host.is_empty() {
        return None;
    }
    Some(format!("{scheme}://{host}"))
}

/// 本机 / 局域网 / 桌面客户端的来源：不算第三方。
fn is_local_origin(origin: &str) -> bool {
    let s = origin.trim().to_ascii_lowercase();
    if s.starts_with("tauri://") || s.starts_with("file://") {
        return true;
    }
    let host = normalize_origin(&s)
        .and_then(|n| n.split("://").nth(1).map(str::to_string))
        .unwrap_or_else(|| s.clone());
    let host = host.trim_start_matches('[').trim_end_matches(']');
    if host == "localhost" || host.ends_with(".localhost") {
        return true;
    }
    let Ok(ip) = host.parse::<std::net::IpAddr>() else {
        return false;
    };
    match ip {
        std::net::IpAddr::V4(v4) => {
            v4.is_loopback() || v4.is_private() || v4.is_link_local() || v4.is_unspecified()
        }
        std::net::IpAddr::V6(v6) => {
            v6.is_loopback() || v6.is_unspecified() || (v6.segments()[0] & 0xfe00) == 0xfc00
        }
    }
}

/// 网段：IPv4 取前两段（/16），IPv6 取前两组。
pub fn net_prefix(ip: &str) -> String {
    if ip.contains(':') {
        ip.split(':').take(2).collect::<Vec<_>>().join(":")
    } else {
        ip.split('.').take(2).collect::<Vec<_>>().join(".")
    }
}

/// 两个账号之间的命中：次数、首末时刻、涉及的 IP（或其他样例）。
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Hits {
    pub n: usize,
    pub first: f64,
    pub last: f64,
    pub samples: BTreeSet<String>,
}

impl Hits {
    fn add(&mut self, ts: f64, sample: &str) {
        if self.n == 0 || ts < self.first {
            self.first = ts;
        }
        self.last = self.last.max(ts);
        self.n += 1;
        if self.samples.len() < 5 {
            self.samples.insert(sample.to_string());
        }
    }
}

fn pair(a: &str, b: &str) -> (String, String) {
    if a < b {
        (a.to_string(), b.to_string())
    } else {
        (b.to_string(), a.to_string())
    }
}

/// 同 IP 换号。`events`：(时刻, IP, Discord 账号)，可无序。返回账号对 → 命中。
/// 同一天（按 `day_of`）里 ≥[`SHARED_IP_KEYS`] 个账号用过的 IP 整天不算。
pub fn ip_switches(
    events: &[(f64, String, String)],
    day_of: impl Fn(f64) -> String,
) -> BTreeMap<(String, String), Hits> {
    let mut users: HashMap<(String, String), HashSet<&str>> = HashMap::new();
    let mut by_ip: HashMap<&str, Vec<(f64, &str)>> = HashMap::new();
    for (ts, ip, d) in events {
        if ip.is_empty() {
            continue;
        }
        users
            .entry((ip.clone(), day_of(*ts)))
            .or_default()
            .insert(d.as_str());
        by_ip
            .entry(ip.as_str())
            .or_default()
            .push((*ts, d.as_str()));
    }
    let mut out: BTreeMap<(String, String), Hits> = BTreeMap::new();
    for (ip, mut es) in by_ip {
        es.sort_by(|a, b| a.0.total_cmp(&b.0));
        for w in es.windows(2) {
            let ((ta, da), (tb, db)) = (w[0], w[1]);
            if da == db || tb - ta > SWITCH_SECS {
                continue;
            }
            let shared = users
                .get(&(ip.to_string(), day_of(tb)))
                .is_some_and(|u| u.len() >= SHARED_IP_KEYS);
            if !shared {
                out.entry(pair(da, db)).or_default().add(tb, ip);
            }
        }
    }
    out
}

/// 一把 Key 的「同时异地」次数：按时间排好的 (时刻, IP)，相邻两次在 60 秒内、网段不同记一次。
pub fn concurrent_hops(events: &[(f64, String)]) -> Hits {
    let mut es: Vec<&(f64, String)> = events.iter().filter(|(_, ip)| !ip.is_empty()).collect();
    es.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut h = Hits::default();
    for w in es.windows(2) {
        let (a, b) = (w[0], w[1]);
        if b.0 - a.0 <= CONCURRENT_SECS && net_prefix(&a.1) != net_prefix(&b.1) {
            h.add(b.0, &format!("{} / {}", a.1, b.1));
        }
    }
    h
}

/// 几乎一样的 dHash 对：输入 (dHash, 编号)，返回汉明距离 ≤`max` 的编号对（编号小的在前，去重）。
/// 用分段索引避免两两比较：64 位切成 max+1 段，距离 ≤max 的两个值至少有一段完全相同（抽屉原理）。
pub fn near_pairs(items: &[(u64, usize)], max: u32) -> BTreeSet<(usize, usize)> {
    let parts = max as usize + 1;
    let width = 64usize.div_ceil(parts);
    let mut buckets: HashMap<(usize, u64), Vec<usize>> = HashMap::new();
    for (i, (h, _)) in items.iter().enumerate() {
        for p in 0..parts {
            let shift = p * width;
            if shift >= 64 {
                break;
            }
            let mask = if width >= 64 {
                u64::MAX
            } else {
                (1u64 << width) - 1
            };
            buckets.entry((p, (h >> shift) & mask)).or_default().push(i);
        }
    }
    let mut out = BTreeSet::new();
    for idx in buckets.values() {
        for (x, &i) in idx.iter().enumerate() {
            for &j in &idx[x + 1..] {
                if (items[i].0 ^ items[j].0).count_ones() <= max {
                    let (a, b) = (items[i].1, items[j].1);
                    if a != b {
                        out.insert((a.min(b), a.max(b)));
                    }
                }
            }
        }
    }
    out
}

/// (账号对, Jaccard, 共同服务器数, 共同冷门服务器)
pub type GuildOverlap = ((String, String), f64, usize, Vec<String>);

/// 服务器重合：账号 → 服务器集合；`members` = 每个服务器在本站记录里有几个账号。
/// 返回 (账号对, Jaccard, 共同服务器数, 共同冷门服务器)。只比共同冷门服务器 ≥2 的对。
pub fn guild_overlaps(
    guilds_of: &HashMap<String, BTreeSet<String>>,
    members: &HashMap<String, usize>,
) -> Vec<GuildOverlap> {
    let mut rare_shared: BTreeMap<(String, String), Vec<String>> = BTreeMap::new();
    let mut by_guild: HashMap<&str, Vec<&str>> = HashMap::new();
    for (d, gs) in guilds_of {
        for g in gs {
            if members.get(g).copied().unwrap_or(0) < RARE_GUILD_MEMBERS {
                by_guild.entry(g.as_str()).or_default().push(d.as_str());
            }
        }
    }
    for (g, ds) in by_guild {
        for (i, a) in ds.iter().enumerate() {
            for b in &ds[i + 1..] {
                rare_shared
                    .entry(pair(a, b))
                    .or_default()
                    .push(g.to_string());
            }
        }
    }
    let mut out = Vec::new();
    for ((a, b), mut rare) in rare_shared {
        if rare.len() < RARE_GUILD_MIN {
            continue;
        }
        let (ga, gb) = (&guilds_of[&a], &guilds_of[&b]);
        let inter = ga.intersection(gb).count();
        let union = ga.union(gb).count();
        let j = inter as f64 / union.max(1) as f64;
        if j >= GUILD_JACCARD {
            rare.sort();
            out.push(((a, b), j, inter, rare));
        }
    }
    out
}

/// 电量相同：(时刻, 账号, 电量, 是否充电)。30 分钟内两个账号电量和充电状态完全一样记一次；满电不算。
pub fn battery_matches(rows: &[(f64, String, f64, bool)]) -> BTreeMap<(String, String), Hits> {
    let mut rs: Vec<&(f64, String, f64, bool)> = rows.iter().filter(|r| r.2 < 0.995).collect();
    rs.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut out: BTreeMap<(String, String), Hits> = BTreeMap::new();
    for (i, a) in rs.iter().enumerate() {
        for b in &rs[i + 1..] {
            if b.0 - a.0 > BATTERY_SECS {
                break;
            }
            if a.1 != b.1 && (a.2 - b.2).abs() < 1e-9 && a.3 == b.3 {
                let sample = format!("{:.0}%{}", a.2 * 100.0, if a.3 { " 充电中" } else { "" });
                out.entry(pair(&a.1, &b.1)).or_default().add(b.0, &sample);
            }
        }
    }
    out
}

fn when(clock: &Clock, h: &Hits) -> String {
    let f = |t: f64| clock.format(t, "%m-%d %H:%M");
    if h.n <= 1 || (h.last - h.first) < 60.0 {
        f(h.last)
    } else {
        format!("{} ~ {}", f(h.first), f(h.last))
    }
}

fn samples(h: &Hits) -> String {
    h.samples.iter().cloned().collect::<Vec<_>>().join("、")
}

/// 跑一遍七项，返回信号（由调用方统一写库）。`official` 是本站的 origin（`SITE_URL`，精确匹配）；
/// 它之外的已知域见 [`KNOWN_ORIGINS`]，都不算第三方。其余来源的请求说明 Key 被填进了第三方网站。
pub(super) fn scan(
    snapshot: &ScanSnapshot,
    now: f64,
    clock: &Clock,
    keys_of: &HashMap<String, Vec<i64>>,
    did_of: &HashMap<i64, String>,
    official: &[String],
) -> Vec<Signal> {
    let mut out = Vec::new();
    let kids = |ids: &BTreeSet<String>| -> String {
        ids.iter()
            .flat_map(|d| keys_of.get(d).cloned().unwrap_or_default())
            .map(|k| k.to_string())
            .collect::<Vec<_>>()
            .join(",")
    };
    let push = |out: &mut Vec<Signal>,
                kind: &'static str,
                ids: BTreeSet<String>,
                score: f64,
                summary: String,
                evidence: Vec<serde_json::Value>| {
        out.push(Signal {
            kind,
            subject: subject(&ids),
            key_ids: kids(&ids),
            score,
            detail: json!({"summary": summary, "evidence": evidence}),
            alert: None,
            all_banned: false,
        });
    };

    // ---------- 近 24 小时的请求（同 IP 换号 / 同时异地） ----------
    let recent: Vec<(f64, String, i64)> = snapshot
        .requests
        .iter()
        .filter(|(key, event)| event.ts >= now - DAY && did_of.contains_key(key))
        .map(|(key, event)| (event.ts, event.ip.clone(), *key))
        .collect();
    let by_account: Vec<(f64, String, String)> = recent
        .iter()
        .map(|(t, ip, k)| (*t, ip.clone(), did_of[k].clone()))
        .collect();
    for ((a, b), h) in ip_switches(&by_account, |t| clock.day(t)) {
        let score = (20.0 + 8.0 * (h.n as f64 - 1.0)).min(60.0);
        push(
            &mut out,
            "ip_switch",
            [a, b].into(),
            score,
            format!(
                "近 24 小时同一个 IP 上 {} 次在 10 分钟内换了另一个账号的 Key",
                h.n
            ),
            vec![
                ev("次数", h.n),
                ev("IP", samples(&h)),
                ev("时间", when(clock, &h)),
            ],
        );
    }
    let mut per_key: BTreeMap<i64, Vec<(f64, String)>> = BTreeMap::new();
    for (t, ip, k) in &recent {
        per_key.entry(*k).or_default().push((*t, ip.clone()));
    }
    for (k, es) in &per_key {
        let h = concurrent_hops(es);
        if h.n < CONCURRENT_MIN {
            continue;
        }
        out.push(Signal {
            kind: "key_concurrent",
            subject: format!("key:{k}"),
            key_ids: k.to_string(),
            score: (40.0 + 5.0 * (h.n - CONCURRENT_MIN) as f64).min(70.0),
            detail: json!({
                "summary": format!("Key #{k} 近 24 小时 {} 次在 60 秒内出现在两个不同网段", h.n),
                "evidence": [ev("次数", h.n), ev("IP", samples(&h)), ev("时间", when(clock, &h)), ev("Discord", &did_of[k])],
            }),
            alert: None,
            all_banned: false,
        });
    }

    // ---------- 同一张参考图 ----------
    {
        let rows = snapshot.references.clone();
        // 每张图（sha256）用过它的 Key；≥5 把 Key 用过的是公开素材
        let mut keys_by_sha: HashMap<&str, HashSet<i64>> = HashMap::new();
        for (sha, _, k, _) in &rows {
            keys_by_sha.entry(sha.as_str()).or_default().insert(*k);
        }
        // (账号对) → (完全相同的张数, 相近的张数, 首末时刻)
        let mut exact: BTreeMap<(String, String), Hits> = BTreeMap::new();
        let mut near: BTreeMap<(String, String), Hits> = BTreeMap::new();
        let mut last_ts: HashMap<&str, f64> = HashMap::new();
        for (sha, _, _, ts) in &rows {
            last_ts.insert(sha.as_str(), *ts);
        }
        for (sha, ks) in &keys_by_sha {
            if ks.len() < 2 || ks.len() >= REF_MAX_KEYS {
                continue;
            }
            let accts: BTreeSet<&str> = ks
                .iter()
                .filter_map(|k| did_of.get(k).map(String::as_str))
                .collect();
            let accts: Vec<&str> = accts.into_iter().collect();
            for (i, a) in accts.iter().enumerate() {
                for b in &accts[i + 1..] {
                    exact
                        .entry(pair(a, b))
                        .or_default()
                        .add(last_ts[sha], sha.get(..12).unwrap_or(sha));
                }
            }
        }
        // 相近：每个 (dHash, Key) 一项；排除全 0（纯色图）和 ≥5 把 Key 都有相近图的 dHash；
        // sha256 也相同的是「完全相同」，上面已经算过，不重复算成「几乎一样」
        let mut seen: HashSet<(u64, i64)> = HashSet::new();
        let mut items: Vec<(u64, i64, f64, &str)> = Vec::new();
        for (sha, dh, k, ts) in &rows {
            let Some(h) = dh.as_deref().and_then(|d| u64::from_str_radix(d, 16).ok()) else {
                continue;
            };
            if h == 0 || h == u64::MAX || !did_of.contains_key(k) {
                continue;
            }
            if seen.insert((h, *k)) {
                items.push((h, *k, *ts, sha.as_str()));
            }
        }
        let idx: Vec<(u64, usize)> = items.iter().enumerate().map(|(i, x)| (x.0, i)).collect();
        let pairs = near_pairs(&idx, DHASH_MAX_DIST);
        let mut keys_near: HashMap<usize, HashSet<i64>> = HashMap::new();
        for &(i, j) in &pairs {
            keys_near.entry(i).or_default().insert(items[j].1);
            keys_near.entry(j).or_default().insert(items[i].1);
        }
        for &(i, j) in &pairs {
            let (ki, kj) = (items[i].1, items[j].1);
            let popular = |x: usize| keys_near.get(&x).map_or(0, HashSet::len) + 1 >= REF_MAX_KEYS;
            if ki == kj || items[i].3 == items[j].3 || popular(i) || popular(j) {
                continue;
            }
            let (da, db) = (&did_of[&ki], &did_of[&kj]);
            if da == db {
                continue;
            }
            near.entry(pair(da, db))
                .or_default()
                .add(items[i].2.max(items[j].2), &format!("{:016x}", items[i].0));
        }
        let mut all: BTreeSet<(String, String)> = exact.keys().cloned().collect();
        all.extend(near.keys().cloned());
        for p in all {
            let (e, n) = (exact.get(&p), near.get(&p));
            let (ne, nn) = (e.map_or(0, |h| h.n), n.map_or(0, |h| h.n));
            let base = if ne > 0 { 70.0 } else { 55.0 };
            let score = (base + 5.0 * ((ne + nn) as f64 - 1.0)).min(90.0);
            let span = e.or(n).cloned().unwrap_or_default();
            push(
                &mut out,
                "same_ref_image",
                [p.0.clone(), p.1.clone()].into(),
                score,
                format!(
                    "近 14 天两个账号的 Key 上传过同一张参考图（完全相同 {ne} 张，几乎一样 {nn} 张）"
                ),
                vec![
                    ev("完全相同", format!("{ne} 张")),
                    ev("几乎一样", format!("{nn} 张")),
                    ev("时间", when(clock, &span)),
                ],
            );
        }
    }

    // ---------- 同一个外部账号 ----------
    {
        let mut by_ext: BTreeMap<(String, String), BTreeSet<String>> = BTreeMap::new();
        for (t, x, d) in snapshot.connections.iter().cloned() {
            if keys_of.contains_key(&d) {
                by_ext.entry((t, x)).or_default().insert(d);
            }
        }
        for ((t, _), ids) in by_ext.into_iter().filter(|(_, ids)| ids.len() >= 2) {
            let n = ids.len();
            push(
                &mut out,
                "same_connection",
                ids,
                100.0,
                format!("{n} 个 Discord 账号绑定了同一个 {t} 账号"),
                vec![ev("平台", &t), ev("账号", n)],
            );
        }
    }

    // ---------- 服务器重合 ----------
    {
        let mut guilds_of: HashMap<String, BTreeSet<String>> = HashMap::new();
        let mut members: HashMap<String, usize> = HashMap::new();
        let mut names: HashMap<String, String> = HashMap::new();
        for (d, g, n) in snapshot.guilds.iter().cloned() {
            *members.entry(g.clone()).or_default() += 1;
            names.insert(g.clone(), n);
            if keys_of.contains_key(&d) {
                guilds_of.entry(d).or_default().insert(g);
            }
        }
        for ((a, b), j, inter, rare) in guild_overlaps(&guilds_of, &members) {
            let shown: Vec<String> = rare
                .iter()
                .take(3)
                .map(|g| {
                    names
                        .get(g)
                        .filter(|n| !n.is_empty())
                        .cloned()
                        .unwrap_or_else(|| g.clone())
                })
                .collect();
            let score = (50.0 + 10.0 * (rare.len() - RARE_GUILD_MIN) as f64).min(80.0);
            push(
                &mut out,
                "guild_overlap",
                [a, b].into(),
                score,
                format!(
                    "两个账号所在的服务器高度重合（Jaccard {j:.2}），其中 {} 个是冷门服务器",
                    rare.len()
                ),
                vec![
                    ev("共同服务器", format!("{inter} 个")),
                    ev("冷门", format!("{} 个", rare.len())),
                    ev("例如", shown.join("、")),
                ],
            );
        }
    }

    // ---------- 电量相同 ----------
    {
        let rows = snapshot
            .batteries
            .iter()
            .filter(|row| keys_of.contains_key(&row.1))
            .cloned()
            .collect::<Vec<_>>();
        for ((a, b), h) in battery_matches(&rows) {
            push(
                &mut out,
                "same_battery",
                [a, b].into(),
                (50.0 + 10.0 * (h.n as f64 - 1.0)).min(80.0),
                format!(
                    "两个账号 {} 次在 30 分钟内网页登录时电量和充电状态一模一样（很可能是同一台设备上的两个浏览器）",
                    h.n
                ),
                vec![
                    ev("次数", h.n),
                    ev("电量", samples(&h)),
                    ev("时间", when(clock, &h)),
                ],
            );
        }
    }
    // ---------- 第三方站点来源（Key 被填进了别的网站） ----------
    {
        let rows = snapshot.origins.iter().cloned();
        // 一把 Key → 非官方来源的命中（次数、首末时刻、域名样例）
        let mut foreign: BTreeMap<i64, Hits> = BTreeMap::new();
        for row in rows {
            let (k, origin, n, first, last) = row;
            if !did_of.contains_key(&k) || is_local_origin(&origin) {
                continue;
            }
            let Some(norm) = normalize_origin(&origin) else {
                continue;
            };
            let official_match = official
                .iter()
                .any(|o| normalize_origin(o).as_deref() == Some(norm.as_str()))
                || KNOWN_ORIGINS
                    .iter()
                    .any(|o| normalize_origin(o).as_deref() == Some(norm.as_str()));
            if official_match {
                continue;
            }
            let h = foreign.entry(k).or_default();
            if h.n == 0 || first < h.first {
                h.first = first;
            }
            h.last = h.last.max(last);
            h.n += n as usize;
            if h.samples.len() < 5 {
                h.samples.insert(format!("{norm} ×{n}"));
            }
        }
        for (k, h) in &foreign {
            out.push(Signal {
                kind: "foreign_origin",
                subject: format!("key:{k}"),
                key_ids: k.to_string(),
                score: 30.0,
                detail: json!({
                    "summary": "这把 Key 的请求来自第三方网站（Key 会流经对方服务器，有泄露风险）",
                    "evidence": [ev("域名", samples(h)), ev("时间", when(clock, h)), ev("Discord", &did_of[k])],
                }),
                alert: None,
                all_banned: false,
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(ts: f64, ip: &str, d: &str) -> (f64, String, String) {
        (ts, ip.into(), d.into())
    }

    #[test]
    fn ip_switch_counts_quick_account_changes_and_skips_shared_exits() {
        let day = |_: f64| "d".to_string();
        let got = ip_switches(
            &[
                e(0.0, "1.2.3.4", "a"),
                e(100.0, "1.2.3.4", "b"), // 换号 1
                e(200.0, "1.2.3.4", "b"),
                e(2000.0, "1.2.3.4", "a"), // 超过 10 分钟
                e(2100.0, "1.2.3.4", "b"), // 换号 2
                e(0.0, "", "c"),
            ],
            day,
        );
        let h = &got[&("a".to_string(), "b".to_string())];
        assert_eq!((h.n, h.first, h.last), (2, 100.0, 2100.0));
        // 当天 5 个账号用过的 IP：公共出口，不算
        let shared: Vec<_> = ["a", "b", "c", "d", "e"]
            .iter()
            .enumerate()
            .map(|(i, d)| e(i as f64 * 10.0, "5.6.7.8", d))
            .collect();
        assert!(ip_switches(&shared, day).is_empty());
    }

    #[test]
    fn concurrent_needs_different_networks_within_a_minute() {
        let es = vec![
            (0.0, "1.2.3.4".to_string()),
            (30.0, "9.9.3.4".to_string()),  // 不同 /16，30 秒
            (40.0, "9.9.8.8".to_string()),  // 同 /16
            (200.0, "1.2.0.1".to_string()), // 超过 60 秒
            (210.0, "2001:db8::1".to_string()),
        ];
        assert_eq!(concurrent_hops(&es).n, 2);
        assert_eq!(net_prefix("2001:db8:1::2"), "2001:db8");
        assert_eq!(net_prefix("10.20.30.40"), "10.20");
    }

    #[test]
    fn near_pairs_finds_close_hashes_only() {
        let a = 0x0123_4567_89ab_cdefu64;
        let b = a ^ 0b1011; // 距离 3
        let c = a ^ 0xffff; // 距离 16
        let got = near_pairs(&[(a, 0), (b, 1), (c, 2), (a, 3)], 6);
        assert_eq!(got, [(0, 1), (0, 3), (1, 3)].into());
        // 暴力比对一致
        let items: Vec<(u64, usize)> = (0..200u64)
            .map(|i| (i.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ (i % 7), i as usize))
            .collect();
        let brute: BTreeSet<(usize, usize)> = (0..items.len())
            .flat_map(|i| (i + 1..items.len()).map(move |j| (i, j)))
            .filter(|&(i, j)| (items[i].0 ^ items[j].0).count_ones() <= 6)
            .collect();
        assert_eq!(near_pairs(&items, 6), brute);
    }

    #[test]
    fn guild_overlap_needs_jaccard_and_two_rare_guilds() {
        let set = |gs: &[&str]| gs.iter().map(|g| g.to_string()).collect::<BTreeSet<_>>();
        let mut of = HashMap::new();
        of.insert("a".to_string(), set(&["owl", "r1", "r2", "big"]));
        of.insert("b".to_string(), set(&["owl", "r1", "r2", "big", "x"]));
        of.insert("c".to_string(), set(&["owl", "r1", "y", "z", "w"]));
        let members: HashMap<String, usize> = [
            ("owl", 300),
            ("big", 50),
            ("r1", 3),
            ("r2", 2),
            ("x", 1),
            ("y", 1),
            ("z", 1),
            ("w", 1),
        ]
        .iter()
        .map(|(g, n)| (g.to_string(), *n))
        .collect();
        let got = guild_overlaps(&of, &members);
        assert_eq!(got.len(), 1);
        let ((a, b), j, inter, rare) = &got[0];
        assert_eq!((a.as_str(), b.as_str(), *inter), ("a", "b", 4));
        assert!((j - 0.8).abs() < 1e-9);
        assert_eq!(rare, &["r1".to_string(), "r2".to_string()]);
    }

    #[test]
    fn battery_matches_same_level_and_charging_within_30_minutes() {
        let r = |t: f64, d: &str, l: f64, c: bool| (t, d.to_string(), l, c);
        let got = battery_matches(&[
            r(0.0, "a", 0.57, true),
            r(600.0, "b", 0.57, true),  // 命中
            r(700.0, "c", 0.57, false), // 充电状态不同
            r(5000.0, "d", 0.57, true), // 超过 30 分钟
            r(10.0, "e", 1.0, true),    // 满电不算
            r(20.0, "f", 1.0, true),
        ]);
        assert_eq!(
            got.keys().collect::<Vec<_>>(),
            [&("a".to_string(), "b".to_string())]
        );
    }
    #[test]
    fn origin_normalization_ignores_local_and_official_sources() {
        // 本机反代 / 局域网 / 官方桌面端 webview：都没出本机，不算第三方
        for local in [
            "http://127.0.0.1:8000",
            "http://localhost:11451",
            "http://192.168.1.5:8000",
            "http://10.0.0.2",
            "http://172.20.0.3",
            "http://[::1]:8000",
            "tauri://localhost",
            "http://tauri.localhost",
            "file:///C:/app/index.html",
            "https://foo.localhost",
        ] {
            assert!(is_local_origin(local), "{local} should be local");
        }
        // 真实第三方不能被放行
        for foreign in [
            "https://evil.example",
            "https://novel.nekoprompt.top",
            "http://203.0.113.9:8080",
        ] {
            assert!(!is_local_origin(foreign), "{foreign} should be foreign");
        }
        // 端口 / 大小写 / 路径归一
        assert_eq!(
            normalize_origin("HTTPS://NovelAI.net:443/path?q=1").as_deref(),
            Some("https://novelai.net")
        );
        assert_eq!(
            normalize_origin("http://127.0.0.1:8000").as_deref(),
            Some("http://127.0.0.1")
        );
        assert_eq!(
            normalize_origin("tauri://localhost").as_deref(),
            Some("tauri://localhost")
        );
    }
}
