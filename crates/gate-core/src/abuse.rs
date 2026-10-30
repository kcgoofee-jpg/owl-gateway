//! Pure anti-abuse evidence and scoring. No database, clock reads, network or enforcement side effects.
pub mod banned_link;
mod linkage;

use crate::clock::Clock;
use crate::snowflake::snowflake_created_at;
use banned_link::{BanInfo, Ways};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

/// 苹果设备：同型号的 Canvas / WebGL 几乎一样，指纹撞车不能当强证据。
pub fn is_apple(parts: &Value) -> bool {
    let g = parts
        .get("gpuRenderer")
        .and_then(Value::as_str)
        .unwrap_or("");
    let p = parts.get("platform").and_then(Value::as_str).unwrap_or("");
    g.contains("Apple") || p.contains("iPhone") || p.contains("iPad") || p.contains("Mac")
}

/// `device_fingerprints.data` → 各分项（兼容旧格式 `{id, data:{...}}`）。
pub fn device_parts(data: &str) -> Value {
    let v: Value = serde_json::from_str(data).unwrap_or(Value::Null);
    v.get("data").cloned().filter(Value::is_object).unwrap_or(v)
}

pub fn net24(ip: &str) -> &str {
    ip.rsplit_once('.').map_or(ip, |(a, _)| a)
}
pub fn net16(ip: &str) -> String {
    ip.split('.').take(2).collect::<Vec<_>>().join(".")
}

#[derive(Default, Debug, Clone)]
pub struct RequestEvent {
    pub ts: f64,
    pub ip: String,
    pub ok: bool,
    pub member_quota: bool,
}

/// 两个账号之间的行为证据。
#[derive(Default, Debug, Clone, PartialEq)]
pub struct PairEvidence {
    pub shared_ip: usize,
    pub simul_same_net: usize,
    pub simul_diff_net: usize,
    pub handoffs: usize,
    /// 两个 Discord 账号注册时间差（小时）
    pub created_gap_h: Option<f64>,
    /// 轮流使用：合并两人的请求时间线，10 分钟内从一个号切到另一个号的次数
    pub switches: usize,
}

/// 行为分（0–100）。门槛：至少有三次换号续图才出信号；
/// 只有同 IP、同时在线的（典型的室友 / 一家人）不进队列，不管同了几个 IP。
pub fn pair_score(e: &PairEvidence) -> Option<f64> {
    let strong = e.handoffs >= 3;
    if !strong {
        return None;
    }
    let mut s = 0.0;
    if e.shared_ip >= 1 {
        s += 15.0 + 5.0 * (e.shared_ip.min(3) - 1) as f64;
    }
    if e.simul_same_net >= 3 {
        s += 15.0;
    }
    s += 5.0 * e.handoffs.min(4) as f64;
    if e.created_gap_h.is_some_and(|h| h <= 48.0) {
        s += 15.0;
    }
    // 一个人轮流用几个号：时间线上频繁「这个号停、那个号接着用」，而且从没在不同城市同时用过
    if e.switches >= 5 && e.simul_diff_net == 0 {
        s += 10.0;
    }
    // 同一分钟里从不同城市的网络同时在用 = 更像两个人
    if e.simul_diff_net >= 3 {
        s -= 30.0;
    }
    let s = s.clamp(0.0, 100.0);
    (s >= 40.0).then_some(s)
}

/// 同一分钟内两个账号都在用：(同网络次数, 不同 /16 网络次数)。双指针，按时间排好序的输入。
pub fn simultaneous(a: &[RequestEvent], b: &[RequestEvent]) -> (usize, usize) {
    let (mut same, mut diff) = (0, 0);
    let mut j0 = 0;
    for x in a {
        while j0 < b.len() && b[j0].ts < x.ts - 60.0 {
            j0 += 1;
        }
        let mut j = j0;
        while j < b.len() && b[j].ts <= x.ts + 60.0 {
            if !x.ip.is_empty() && !b[j].ip.is_empty() {
                if net16(&x.ip) == net16(&b[j].ip) {
                    same += 1;
                } else {
                    diff += 1;
                }
            }
            j += 1;
        }
    }
    (same, diff)
}

/// 合并两人的请求时间线（都按时间排好序），数相邻两条属于不同账号、间隔 ≤10 分钟的次数。
pub fn switches(a: &[RequestEvent], b: &[RequestEvent]) -> usize {
    let (mut i, mut j, mut n) = (0, 0, 0);
    let mut last: Option<(bool, f64)> = None;
    while i < a.len() || j < b.len() {
        let take_a = j >= b.len() || (i < a.len() && a[i].ts <= b[j].ts);
        let (who, ts) = if take_a {
            i += 1;
            (true, a[i - 1].ts)
        } else {
            j += 1;
            (false, b[j - 1].ts)
        };
        if let Some((w, t)) = last
            && w != who
            && ts - t <= 600.0
        {
            n += 1;
        }
        last = Some((who, ts));
    }
    n
}

/// A 撞到个人额度后 10 分钟内，B 在同一个 /24 出图成功的次数。
pub fn handoffs(a: &[RequestEvent], b: &[RequestEvent]) -> usize {
    let mut n = 0;
    for x in a.iter().filter(|e| e.member_quota) {
        if b.iter().any(|y| {
            y.ok && y.ts >= x.ts
                && y.ts - x.ts <= 600.0
                && !x.ip.is_empty()
                && net24(&x.ip) == net24(&y.ip)
        }) {
            n += 1;
        }
    }
    n
}

/// Discord 给没自己起名的账号自动生成的用户名：显示名 + 下划线 + 4～5 位数字（如 xxx_12345）。
pub fn is_auto_username(u: &str) -> bool {
    let Some((head, tail)) = u.rsplit_once('_') else {
        return false;
    };
    !head.is_empty() && (4..=5).contains(&tail.len()) && tail.bytes().all(|b| b.is_ascii_digit())
}

pub fn subject(ids: &BTreeSet<String>) -> String {
    ids.iter().cloned().collect::<Vec<_>>().join(",")
}

/// 同浏览器 / 同设备类信号：关联的 Discord 账号里只有一个持有 Key、其余都没有 Key 时，
/// 多半是一个人换了号登录（或还没领 Key 的新号），不是两个人各拿一把 Key：分数减半，并补一条证据。
/// 两个及以上账号持有 Key 时原样返回。
pub fn lower_if_solo_key_holder(
    score: f64,
    holders: usize,
    accounts: usize,
) -> (f64, Option<Value>) {
    if holders == 1 && accounts >= 2 {
        (score / 2.0, Some(ev("其余账号", "无 Key")))
    } else {
        (score, None)
    }
}

pub fn ev(k: &str, v: impl ToString) -> Value {
    json!({"k": k, "v": v.to_string()})
}

/// 模糊指纹：去掉浏览器版本 / 语言这类会变的项，只留跟硬件走的。
pub fn fuzzy_key(parts: &Value) -> Option<String> {
    let f = |k: &str| match parts.get(k) {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Null) | None => String::new(),
        Some(v) => v.to_string(),
    };
    let key = [
        f("gpuRenderer"),
        f("screen"),
        f("hw"),
        f("mem"),
        f("tz"),
        f("canvas"),
    ];
    // 显卡和 Canvas 都拿不到时，剩下的几项太常见，不比
    (!key[0].is_empty() || !key[5].is_empty()).then(|| key.join("|"))
}

pub struct Signal {
    pub kind: &'static str,
    pub subject: String,
    pub key_ids: String,
    pub score: f64,
    pub detail: Value,
    /// 封禁号关联的新线索告警说明
    pub alert: Option<String>,
    /// 涉及的账号已经全部封禁：信号不再带来新信息，写进去就标成已确认
    pub all_banned: bool,
}

/// 同一台机器上的多个账号：用并查集把「同浏览器 / 同设备 / 同硬件指纹」任意一种关系连起来的账号并成一组。
#[derive(Default)]
struct Groups {
    parent: HashMap<String, String>,
}

impl Groups {
    fn add(&mut self, x: &str) {
        self.parent
            .entry(x.to_string())
            .or_insert_with(|| x.to_string());
    }

    fn find(&mut self, x: &str) -> String {
        let mut root = x.to_string();
        loop {
            let next = self
                .parent
                .get(&root)
                .cloned()
                .unwrap_or_else(|| root.clone());
            if next == root {
                break;
            }
            root = next;
        }
        let mut node = x.to_string();
        while node != root {
            let next = self
                .parent
                .get(&node)
                .cloned()
                .unwrap_or_else(|| root.clone());
            self.parent.insert(node.clone(), root.clone());
            node = next;
        }
        root
    }

    fn union(&mut self, a: &str, b: &str) {
        self.add(a);
        self.add(b);
        let (ra, rb) = (self.find(a), self.find(b));
        if ra != rb {
            self.parent.insert(rb, ra);
        }
    }

    fn groups(&mut self) -> BTreeMap<String, BTreeSet<String>> {
        let nodes: Vec<String> = self.parent.keys().cloned().collect();
        let mut out: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for node in nodes {
            let root = self.find(&node);
            out.entry(root).or_default().insert(node);
        }
        out
    }
}

/// 账号之间的关联方式（设备簇用）。权重按可伪造难度：浏览器标记最难换，硬件指纹最易撞。
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum Link {
    Browser,
    Device,
    Hardware,
}

impl Link {
    fn label(self) -> &'static str {
        match self {
            Link::Browser => "同浏览器",
            Link::Device => "同设备",
            Link::Hardware => "同硬件指纹",
        }
    }

    /// 苹果的指纹（iPad / iPhone）撞得多，降到一半。
    fn weight(self, apple: bool) -> f64 {
        let base = match self {
            Link::Browser => 80.0,
            Link::Device => 60.0,
            Link::Hardware => 45.0,
        };
        if apple && self != Link::Browser {
            base / 2.0
        } else {
            base
        }
    }
}

/// 一轮扫描里「只记录、不提示」的东西：网络证据和链路健康度。写进 `job_last` 的摘要，
/// 不进待审队列（站长 10-31：同 IP 多账号没有意义，别老是提示）。
#[derive(Default, Debug, Clone)]
pub struct Notes {
    /// 同 IP 换号：NAT / 手机流量 / 机场会把一堆真人塞在同一个出口
    pub same_ip_switches: usize,
    /// 网页登录时没交设备指纹（链路健康度：我们自己没收到的也有）
    pub fp_missing: usize,
    /// 设备簇数、涉及账号数
    pub device_clusters: usize,
    pub device_cluster_accounts: usize,
}

pub struct Outcome {
    pub signals: Vec<Signal>,
    pub notes: Notes,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn solo_key_holder_halves_the_score_and_adds_a_chip() {
        let (score, chip) = lower_if_solo_key_holder(80.0, 1, 2);
        assert_eq!(score, 40.0);
        assert_eq!(chip, Some(ev("其余账号", "无 Key")));
        // 两个及以上持有 Key，或者一个都没有：分数不变
        assert_eq!(lower_if_solo_key_holder(80.0, 2, 3), (80.0, None));
        assert_eq!(lower_if_solo_key_holder(60.0, 0, 2), (60.0, None));
    }

    #[test]
    fn auto_generated_usernames() {
        assert!(is_auto_username("huanongying0053_69617"));
        assert!(is_auto_username("fdsg_76831"));
        assert!(!is_auto_username("lucid_dream213"));
        assert!(!is_auto_username("alice"));
        assert!(!is_auto_username("_12345"));
    }

    #[test]
    fn switches_count_handovers_within_ten_minutes() {
        let at = |t: f64| RequestEvent {
            ts: t,
            ..Default::default()
        };
        let a = vec![at(0.0), at(100.0), at(2000.0)];
        let b = vec![at(200.0), at(5000.0)];
        // 100(a)→200(b) 算一次；2000(a)→5000(b) 隔太久不算
        assert_eq!(switches(&a, &b), 1);
    }

    #[test]
    fn housemates_with_only_shared_ip_are_not_flagged() {
        let e = PairEvidence {
            shared_ip: 3,
            simul_same_net: 10,
            created_gap_h: Some(1.0),
            ..Default::default()
        };
        assert_eq!(pair_score(&e), None);
    }

    #[test]
    fn repeated_handoffs_plus_shared_ip_score_high() {
        let e = PairEvidence {
            shared_ip: 3,
            simul_same_net: 5,
            handoffs: 4,
            ..Default::default()
        };
        assert!(pair_score(&e).unwrap() >= 50.0);
        // 同时从不同城市的网络在用：扣分
        let e2 = PairEvidence {
            simul_diff_net: 5,
            ..e.clone()
        };
        assert!(pair_score(&e2).unwrap_or(0.0) < pair_score(&e).unwrap());
    }

    #[test]
    fn simultaneous_counts_same_and_different_networks() {
        let a = vec![RequestEvent {
            ts: 100.0,
            ip: "1.2.3.4".into(),
            ..Default::default()
        }];
        let b = vec![
            RequestEvent {
                ts: 130.0,
                ip: "1.2.9.9".into(),
                ..Default::default()
            },
            RequestEvent {
                ts: 150.0,
                ip: "8.8.8.8".into(),
                ..Default::default()
            },
            RequestEvent {
                ts: 500.0,
                ip: "1.2.3.4".into(),
                ..Default::default()
            },
        ];
        assert_eq!(simultaneous(&a, &b), (1, 1));
    }

    #[test]
    fn handoff_needs_quota_hit_then_success_on_same_subnet() {
        let a = vec![RequestEvent {
            ts: 0.0,
            ip: "10.0.0.5".into(),
            member_quota: true,
            ..Default::default()
        }];
        let b = vec![
            RequestEvent {
                ts: 120.0,
                ip: "10.0.0.9".into(),
                ok: true,
                ..Default::default()
            },
            RequestEvent {
                ts: 150.0,
                ip: "10.0.1.9".into(),
                ok: true,
                ..Default::default()
            },
        ];
        assert_eq!(handoffs(&a, &b), 1);
        assert_eq!(handoffs(&b, &a), 0);
    }

    #[test]
    fn fuzzy_key_ignores_browser_version_and_apple_is_flagged() {
        let p1 = json!({"ua": "Chrome/140", "gpuRenderer": "ANGLE (NVIDIA RTX 3060)", "screen": "1920x1080x24", "hw": 12, "mem": 8, "tz": -480, "canvas": "abc"});
        let p2 = json!({"ua": "Chrome/141", "gpuRenderer": "ANGLE (NVIDIA RTX 3060)", "screen": "1920x1080x24", "hw": 12, "mem": 8, "tz": -480, "canvas": "abc"});
        assert_eq!(fuzzy_key(&p1), fuzzy_key(&p2));
        assert!(!is_apple(&p1));
        assert!(is_apple(
            &json!({"gpuRenderer": "Apple GPU", "platform": "iPhone"})
        ));
        assert_eq!(fuzzy_key(&json!({"screen": "1x1"})), None);
    }
}

/// Owned metadata snapshot supplied by the database adapter; never contains prompts or image bytes.
#[derive(Default, Debug, Clone)]
pub struct ScanSnapshot {
    /// (key id, Discord subject)
    pub keys: Vec<(i64, String)>,
    /// (Discord subject, ban reason, display name)
    pub bans: Vec<(String, String, String)>,
    pub requests: Vec<(i64, RequestEvent)>,
    /// (device id, stored fingerprint JSON)
    pub devices: Vec<(String, String)>,
    /// (Discord subject, device id)
    pub device_logins: Vec<(String, String)>,
    /// (Discord subject, browser id)
    pub browser_logins: Vec<(String, String)>,
    /// (Discord subject, fingerprint status, last login timestamp)
    pub incomplete_logins: Vec<(String, String, f64)>,
    pub registrations: Vec<ScanRegistration>,
    /// (Discord subject, login source IP)
    pub login_networks: Vec<(String, String)>,
    /// Hashes only: (SHA256, optional dHash, key id, timestamp)
    pub references: Vec<ReferenceFootprint>,
    /// (connection type, external account id, Discord subject)
    pub connections: Vec<(String, String, String)>,
    /// (Discord subject, guild id, guild name)
    pub guilds: Vec<(String, String, String)>,
    /// (timestamp, Discord subject, battery level, charging)
    pub batteries: Vec<(f64, String, f64, bool)>,
    /// (key id, origin, count, first timestamp, last timestamp)
    pub origins: Vec<OriginFootprint>,
}
/// Discord subject, registration time, guild join time, member flags, avatar and username.
pub type ScanRegistration = (String, f64, Option<f64>, i64, String, String);
pub type ReferenceFootprint = (String, Option<String>, i64, f64);
pub type OriginFootprint = (i64, String, i64, f64, f64);

/// Bounds belong to the rules; the database only applies these explicit query parameters.
#[derive(Debug, Clone, Copy)]
pub struct ScanWindow {
    pub behavior_since: f64,
    pub metadata_since: f64,
    pub fingerprint_since: f64,
    pub linkage_since: f64,
}
impl ScanWindow {
    pub fn new(now: f64, login_required_since: f64) -> Self {
        const DAY: f64 = 86400.0;
        Self {
            behavior_since: now - 14.0 * DAY,
            metadata_since: now - 30.0 * DAY,
            fingerprint_since: login_required_since.max(now - 30.0 * DAY),
            linkage_since: now - 14.0 * DAY,
        }
    }
}

pub const MEMBER_FLAG_DID_REJOIN: i64 = 1;
const DAY: f64 = 86400.0;
#[derive(Clone, Copy, Debug)]
pub struct NetworkClass {
    pub asn: u32,
    pub hosting: bool,
}
/// Offline classification supplied by the orchestration adapter.
pub type NetworkLookup<'a> = dyn Fn(&str) -> Option<NetworkClass> + 'a;

/// Evaluate a consistent metadata snapshot. Network classification is a supplied pure lookup.
pub fn evaluate(
    snapshot: &ScanSnapshot,
    now: f64,
    clock: &Clock,
    official: &[String],
    lookup: Option<&NetworkLookup<'_>>,
) -> Outcome {
    let mut out: Vec<Signal> = Vec::new();
    let mut notes = Notes::default();
    // 设备簇的边：(账号 a, 账号 b, 关联方式, 苹果?, 显卡)
    let mut links: Vec<(String, String, Link, bool, Option<String>)> = Vec::new();
    // Discord 账号 → Key（启用）
    let mut keys_of: HashMap<String, Vec<i64>> = HashMap::new();
    let mut did_of: HashMap<i64, String> = HashMap::new();
    {
        for (k, d) in snapshot.keys.iter().cloned() {
            keys_of.entry(d.clone()).or_default().push(k);
            did_of.insert(k, d);
        }
    }
    // 封禁名单：Discord ID → 名字 / 理由
    let mut bans: HashMap<String, BanInfo> = HashMap::new();
    {
        for (d, reason, name) in snapshot.bans.iter().cloned() {
            bans.insert(d, BanInfo { name, reason });
        }
    }
    // 关联账号里有几个持有 Key（启用、非站长 / 测试）
    let holders = |ids: &BTreeSet<String>| -> usize {
        ids.iter().filter(|d| keys_of.contains_key(*d)).count()
    };
    let kids = |ids: &BTreeSet<String>| -> String {
        ids.iter()
            .flat_map(|d| keys_of.get(d).cloned().unwrap_or_default())
            .map(|k| k.to_string())
            .collect::<Vec<_>>()
            .join(",")
    };

    // ---------- 行为 ----------
    let mut events: HashMap<i64, Vec<RequestEvent>> = HashMap::new();
    {
        for (k, e) in snapshot.requests.iter().cloned() {
            if did_of.contains_key(&k) {
                events.entry(k).or_default().push(e);
            }
        }
    }
    let ips: HashMap<i64, HashSet<&str>> = events
        .iter()
        .map(|(k, es)| {
            (
                *k,
                es.iter()
                    .filter(|e| !e.ip.is_empty())
                    .map(|e| e.ip.as_str())
                    .collect(),
            )
        })
        .collect();
    let active: Vec<i64> = {
        let mut v: Vec<i64> = events.keys().copied().collect();
        v.sort_unstable();
        v
    };
    for (i, &a) in active.iter().enumerate() {
        for &b in &active[i + 1..] {
            let (da, db) = (&did_of[&a], &did_of[&b]);
            if da == db {
                continue;
            }
            let shared_ip = ips[&a].intersection(&ips[&b]).count();
            // 便宜的先筛：没有同 IP 的，不再算贵的
            if shared_ip == 0 {
                continue;
            }
            let (ea, eb) = (&events[&a], &events[&b]);
            let (simul_same_net, simul_diff_net) = simultaneous(ea, eb);
            let e = PairEvidence {
                shared_ip,
                simul_same_net,
                simul_diff_net,
                handoffs: handoffs(ea, eb) + handoffs(eb, ea),
                switches: switches(ea, eb),
                created_gap_h: match (snowflake_created_at(da), snowflake_created_at(db)) {
                    (Some(x), Some(y)) => Some((x - y).abs() / 3600.0),
                    _ => None,
                },
            };
            let Some(score) = pair_score(&e) else {
                continue;
            };
            let ids: BTreeSet<String> = [da.clone(), db.clone()].into();
            let mut evidence = vec![
                ev("同 IP", format!("{} 个", e.shared_ip)),
                ev("同网同时", format!("{} 次", e.simul_same_net)),
                ev("换号续图", format!("{} 次", e.handoffs)),
            ];
            if let Some(h) = e.created_gap_h {
                evidence.push(ev(
                    "注册相差",
                    if h < 48.0 {
                        format!("{h:.0} 小时")
                    } else {
                        format!("{:.0} 天", h / 24.0)
                    },
                ));
            }
            if e.simul_diff_net > 0 {
                evidence.push(ev("异地同时", format!("{} 次", e.simul_diff_net)));
            }
            if e.switches > 0 {
                evidence.push(ev("轮流使用", format!("{} 次", e.switches)));
            }
            out.push(Signal {
                alert: None,
                all_banned: false,
                kind: "behavior",
                subject: subject(&ids),
                key_ids: kids(&ids),
                score,
                detail: json!({
                    "summary": format!("Key #{a} 和 #{b} 近 14 天的出图行为高度相似"),
                    "evidence": evidence,
                }),
            });
        }
    }

    // ---------- 设备 ----------
    let mut parts_of: HashMap<String, Value> = HashMap::new();
    {
        for (d, data) in snapshot.devices.iter().cloned() {
            parts_of.insert(d, device_parts(&data));
        }
    }
    let mut by_dev: HashMap<String, BTreeSet<String>> = HashMap::new();
    {
        for (d, dev) in snapshot.device_logins.iter().cloned() {
            by_dev.entry(dev).or_default().insert(d);
        }
    }
    let mut by_fuzzy: HashMap<String, (BTreeSet<String>, BTreeSet<String>, bool)> = HashMap::new();
    for (dev, ids) in &by_dev {
        let parts = parts_of.get(dev).cloned().unwrap_or(Value::Null);
        let apple = is_apple(&parts);
        if ids.len() >= 2 {
            let gpu = parts
                .get("gpuRenderer")
                .and_then(Value::as_str)
                .map(str::to_string);
            let ids: Vec<&String> = ids.iter().collect();
            for pair in ids.windows(2) {
                links.push((
                    pair[0].clone(),
                    pair[1].clone(),
                    Link::Device,
                    apple,
                    gpu.clone(),
                ));
            }
        }
        if let Some(fk) = fuzzy_key(&parts) {
            let e = by_fuzzy.entry(fk).or_default();
            e.0.extend(ids.iter().cloned());
            e.1.insert(dev.clone());
            e.2 |= apple;
        }
    }
    for (fk, (ids, devs, apple)) in &by_fuzzy {
        // 只有一个指纹 ID 的组已经算在「同设备」里
        if ids.len() < 2 || devs.len() < 2 {
            continue;
        }
        let gpu = fk.split('|').next().unwrap_or("");
        let gpu = (!gpu.is_empty()).then(|| gpu.to_string());
        let ids: Vec<&String> = ids.iter().collect();
        for pair in ids.windows(2) {
            links.push((
                pair[0].clone(),
                pair[1].clone(),
                Link::Hardware,
                *apple,
                gpu.clone(),
            ));
        }
    }

    // ---------- 登录时的指纹状态 ----------
    {
        for (d, fp, _) in snapshot.incomplete_logins.iter().cloned() {
            if !keys_of.contains_key(&d) {
                continue;
            }
            let ids: BTreeSet<String> = [d].into();
            if fp != "unknown" {
                // 没交指纹不是罪证：换官网镜像那段时间我们自己也收不到，只当链路健康度记一笔
                notes.fp_missing += 1;
                continue;
            }
            out.push(Signal {
                alert: None,
                all_banned: false,
                kind: "fp_unknown",
                subject: subject(&ids),
                key_ids: kids(&ids),
                score: 50.0,
                detail: json!({
                    "summary": "网页登录时交了服务端不认得的设备指纹（疑似伪造）",
                    "evidence": [ev("指纹", "对不上")],
                }),
            });
        }
    }

    // ---------- 新号 / 刚进服 / 退了又进 ----------
    {
        for (d, reg, joined, flags, avatar, username) in snapshot.registrations.iter().cloned() {
            if !keys_of.contains_key(&d) {
                continue;
            }
            let age = snowflake_created_at(&d).map(|born| (reg - born) / DAY);
            let young = age.is_some_and(|a| a < 30.0) && reg >= now - 14.0 * DAY;
            let join_gap = joined.map(|j| (reg - j) / DAY);
            let fresh_join = join_gap.is_some_and(|g| (0.0..1.0).contains(&g));
            let rejoined = flags & MEMBER_FLAG_DID_REJOIN != 0;
            if !young && !fresh_join && !rejoined {
                continue;
            }
            let mut score: f64 = match age {
                Some(a) if young && a < 7.0 => 35.0,
                _ if young => 20.0,
                _ => 0.0,
            };
            let mut evidence = Vec::new();
            let mut why = Vec::new();
            if let Some(a) = age {
                evidence.push(ev("账号年龄", format!("{a:.0} 天")));
                if young {
                    why.push(format!("领 Key 时 Discord 账号才注册 {a:.0} 天"));
                }
            }
            if fresh_join {
                score += 15.0;
                evidence.push(ev("进服", "当天就领 Key"));
                why.push("进服当天就领了 Key".to_string());
            }
            if rejoined {
                score += 10.0;
                evidence.push(ev("退群", "退了又进"));
                why.push("退群后又重新加入".to_string());
            }
            if avatar.is_empty() {
                score += 5.0;
                evidence.push(ev("头像", "没有"));
            }
            if is_auto_username(&username) {
                score += 5.0;
                evidence.push(ev("用户名", "系统自动生成"));
            }
            let ids: BTreeSet<String> = [d].into();
            out.push(Signal {
                alert: None,
                all_banned: false,
                kind: "new_account",
                subject: subject(&ids),
                key_ids: kids(&ids),
                score: score.min(60.0),
                detail: json!({"summary": why.join("；"), "evidence": evidence}),
            });
        }
    }

    // ---------- 同一个浏览器（Cookie / localStorage 里的长期标记） ----------
    {
        let mut by_bid: HashMap<String, BTreeSet<String>> = HashMap::new();
        for (d, b) in snapshot.browser_logins.iter().cloned() {
            by_bid.entry(b).or_default().insert(d);
        }
        for ids in by_bid.values().filter(|ids| ids.len() >= 2) {
            let ids: Vec<&String> = ids.iter().collect();
            for pair in ids.windows(2) {
                links.push((pair[0].clone(), pair[1].clone(), Link::Browser, false, None));
            }
        }
    }

    // ---------- 设备簇：同一台机器上的多个账号合成一条 ----------
    // 站长 10-31：一个人的三个号以前会分别出「同设备 / 设备相近 / 同浏览器」三条，翻起来像三个人；
    // 现在并成一条，证据里写清是怎么连上的。网络证据（同 IP 换号）不再出信号，见 Notes。
    if !links.is_empty() {
        let mut groups = Groups::default();
        for (a, b, _, _, _) in &links {
            groups.union(a, b);
        }
        let mut kinds: HashMap<String, BTreeSet<Link>> = HashMap::new();
        let mut apple: HashMap<String, bool> = HashMap::new();
        let mut gpu: HashMap<String, String> = HashMap::new();
        for (a, _, link, is_apple, card) in &links {
            let root = groups.find(a);
            kinds.entry(root.clone()).or_default().insert(*link);
            *apple.entry(root.clone()).or_default() |= *is_apple;
            if let Some(card) = card {
                gpu.entry(root).or_insert_with(|| card.clone());
            }
        }
        for (root, ids) in groups.groups() {
            if ids.len() < 2 {
                continue;
            }
            let kinds = kinds.remove(&root).unwrap_or_default();
            let is_apple = apple.get(&root).copied().unwrap_or(false);
            let labels: Vec<&str> = kinds.iter().map(|k| k.label()).collect();
            let base = kinds
                .iter()
                .map(|k| k.weight(is_apple))
                .fold(0.0_f64, f64::max);
            // 两种关联同时成立（既同浏览器又同设备）比单一种更确定
            let raw = if kinds.len() >= 2 { base + 10.0 } else { base };
            let (score, chip) = lower_if_solo_key_holder(raw.min(100.0), holders(&ids), ids.len());
            let mut evidence = vec![ev("账号", ids.len()), ev("关联", labels.join(" + "))];
            if let Some(card) = gpu.get(&root) {
                evidence.push(ev("显卡", card.clone()));
            }
            if is_apple {
                evidence.push(ev("苹果", "是（易撞，降权）"));
            }
            evidence.extend(chip);
            notes.device_clusters += 1;
            notes.device_cluster_accounts += ids.len();
            out.push(Signal {
                alert: None,
                all_banned: false,
                kind: "device_cluster",
                subject: subject(&ids),
                key_ids: kids(&ids),
                score,
                detail: json!({
                    "summary": format!("{} 个账号在同一台机器上（{}）", ids.len(), labels.join(" + ")),
                    "evidence": evidence,
                }),
            });
        }
    }

    // ---------- 和封禁号同浏览器 / 同设备（见 banned_link） ----------
    if !bans.is_empty() {
        let mut links: BTreeMap<(String, String), Ways> = BTreeMap::new();
        let mut add = |ids: &BTreeSet<String>, device: Option<bool>| {
            for b in ids.iter().filter(|d| bans.contains_key(*d)) {
                for x in ids.iter().filter(|d| !bans.contains_key(*d)) {
                    let w = links.entry((x.clone(), b.clone())).or_default();
                    match device {
                        None => w.browser = true,
                        Some(apple) => w.add_device(apple),
                    }
                }
            }
        };
        let mut by_bid: HashMap<String, BTreeSet<String>> = HashMap::new();
        for (d, b) in snapshot
            .browser_logins
            .iter()
            .filter(|(_, b)| !b.is_empty())
            .cloned()
        {
            by_bid.entry(b).or_default().insert(d);
        }
        for ids in by_bid.values() {
            add(ids, None);
        }
        for (dev, ids) in &by_dev {
            add(ids, Some(parts_of.get(dev).is_some_and(is_apple)));
        }
        for ((x, b), w) in &links {
            let info = &bans[b];
            let (score, detail) = banned_link::signal(b, info, w);
            let ids: BTreeSet<String> = [x.clone(), b.clone()].into();
            out.push(Signal {
                kind: banned_link::KIND,
                subject: subject(&ids),
                key_ids: kids(&ids),
                score,
                detail,
                alert: Some(banned_link::alert_text(x, b, info, w)),
                all_banned: false,
            });
        }
    }

    // ---------- 共享 Key（要 ASN 库） ----------
    if let Some(network) = lookup {
        for (&k, es) in &events {
            // 开关 VPN / 代理会在「家里宽带」和「机房 IP」之间来回跳，不算共享：机房 IP 的请求不参与比较
            let recent: Vec<(f64, u32, &str)> = es
                .iter()
                .filter(|e| e.ts >= now - DAY && !e.ip.is_empty())
                .filter_map(|e| network(&e.ip).map(|a| (e.ts, a, e.ip.as_str())))
                .filter(|(_, a, _)| !a.hosting)
                .map(|(t, a, ip)| (t, a.asn, ip))
                .collect();
            // 相邻两次请求 5 分钟内换了运营商（不同 /16）记一次「跳网」
            let mut hops = 0;
            let mut nets: BTreeSet<u32> = BTreeSet::new();
            for w in recent.windows(2) {
                let (a, b) = (&w[0], &w[1]);
                nets.insert(a.1);
                nets.insert(b.1);
                if b.0 - a.0 <= 300.0 && a.1 != b.1 && net16(a.2) != net16(b.2) {
                    hops += 1;
                }
            }
            if hops < 3 {
                continue;
            }
            let d = &did_of[&k];
            out.push(Signal {
 alert: None, all_banned: false,
                        kind: "key_sharing",
                        subject: format!("key:{k}"),
                        key_ids: k.to_string(),
                        score: (40.0 + 5.0 * (hops.min(8) - 3) as f64).min(70.0),
                        detail: json!({
                            "summary": format!("Key #{k} 近 24 小时 {hops} 次在 5 分钟内换到另一家运营商的网络（不算机房 / VPN；手机流量和 Wi-Fi 来回切也可能这样）"),
                            "evidence": [ev("跳网", format!("{hops} 次")), ev("运营商", format!("{} 家", nets.len())), ev("Discord", d)],
                        }),
                    });
        }
    }
    // ---------- 登录用的是机房 / VPN IP：给涉及这些账号的信号补一条证据（单独不成信号） ----------
    if let Some(network) = lookup {
        let mut hosting: HashMap<String, usize> = HashMap::new();
        for (d, ip) in &snapshot.login_networks {
            if network(ip).is_some_and(|a| a.hosting) {
                *hosting.entry(d.clone()).or_default() += 1;
            }
        }
        for sig in &mut out {
            if sig.subject.starts_with("key:") {
                continue;
            }
            let n = sig
                .subject
                .split(',')
                .filter(|d| hosting.contains_key(*d))
                .count();
            if n > 0
                && let Some(arr) = sig.detail.get_mut("evidence").and_then(Value::as_array_mut)
            {
                arr.push(ev("机房 IP 登录", format!("{n} 个号")));
            }
        }
    }
    // ---------- 关联扫描（同 IP 换号 / 同时异地 / 同参考图 / 同外部账号 / 服务器重合 / 电量相同 / 第三方站点来源） ----------
    let (linked, same_ip_switches) =
        linkage::scan(snapshot, now, clock, &keys_of, &did_of, official);
    notes.same_ip_switches = same_ip_switches;
    out.extend(linked);
    for sig in &mut out {
        sig.all_banned = !sig.subject.starts_with("key:")
            && sig.subject.split(',').all(|d| bans.contains_key(d));
    }
    out.retain(|s| !s.key_ids.is_empty() || s.kind == banned_link::KIND);

    Outcome {
        signals: out,
        notes,
    }
}
