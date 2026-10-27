//! 小号 / 共享 Key 扫描（每 6 小时）：把可疑信号写进 `abuse_signals`（站长让 Claude 读库复核），**不自动封号**。
//!
//! | kind | 信号 | 分数 |
//! |---|---|---|
//! | `behavior` | 两个账号近 14 天的行为：同一出口 IP、同网络里一分钟内交替出图、额度用完换号续图、Discord 账号注册时间挨着 | 0–100，见 [`pair_score`] |
//! | `device_exact` | 网页登录时同一个设备指纹下有多个 Discord 账号 | 60（苹果设备 30：同型号 iPhone 指纹几乎一样） |
//! | `device_fuzzy` | 指纹 ID 不同，但显卡 / 屏幕 / 核数 / 内存 / 时区 / Canvas 都一样（换浏览器、浏览器升级） | 45（苹果 20） |
//! | `fp_unknown` | 登录时交了服务端不认得的指纹 = 疑似伪造 | 50 |
//! | `fp_missing` | 登录时没交指纹（脚本被拦 / 直接打开 /login） | 15 |
//! | `new_account` | 近 14 天领 Key、领时 Discord 账号不满 30 天（不满 7 天更高）；或近 30 天进服当天就领 Key / 退了又进 | 20 / 35 起 |
//! | `key_sharing` | 一把 Key 近 24 小时里反复在 5 分钟内出现在两家不同运营商的网络上 | 40 起 |
//! | `same_browser` | 同一个浏览器（Cookie / localStorage 里的长期标记）登录过多个 Discord 账号 | 80 |
//! | `banned_link` | 和已封禁的账号同浏览器 / 同设备网页登录过（见 [`crate::ban_link`]，领 Key 时会被拦） | 95（只撞了苹果设备 60，不拦） |
//!
//! `device_exact` / `device_fuzzy` / `same_browser` 关联的账号里只有一个持有 Key、其余都没有时，分数减半并补一条「其余账号：无 Key」。
//! 涉及的账号已经全部封禁的信号不带来新信息：写进去就标成已确认，不留在待审里。
//! 涉及的账号一个 Key 都没有的信号是噪音（站长 10-23）：不写（`banned_link` 除外，领 Key 拦截靠它的忽略状态放行）。
//!
//! 另有七项关联信号（`ip_switch` / `key_concurrent` / `same_ref_image` / `same_connection` / `guild_overlap` /
//! `same_battery` / `foreign_origin`——Key 的请求来自第三方网站，分数 30，只提醒不封），见 [`linkage`]。
//!
//! 新号信号同时看：进服当天就领 Key（+15）、退群后又进（+10）、没头像（+5）、系统自动生成的用户名（+5）。
//! 行为信号另看「轮流使用」：时间线上频繁这个号停、那个号接着用（+10）。登录用过机房 / VPN IP 的账号，
//! 在它涉及的信号里补一条「机房 IP 登录」证据（国内开代理很普遍，单独不算信号）。
//!
//! 判定参考（站长 10-22 定）：至少 3 条强证据、设备类与行为类各至少 1 条、跨 2 天以上才封；
//! 只有同 IP（典型的室友）不封。这里只出证据，封不封由站长决定。

mod linkage;

use crate::asn::AsnDb;
use crate::ban_link::{self, Ways, is_apple};
use crate::jobs::JobError;
use crate::state::AppState;
use gate_core::snowflake::snowflake_created_at;
use gate_db::repo::abuse::BanInfo;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

const DAY: f64 = 86400.0;
/// 行为比对看最近几天
const BEHAVIOR_DAYS: f64 = 14.0;
/// ASN 数据文件（iptoasn.com 的 ip2asn-combined.tsv.gz，宿主机每周更新一次）
const ASN_FILE: &str = "ip2asn-combined.tsv.gz";

fn net24(ip: &str) -> &str {
    ip.rsplit_once('.').map_or(ip, |(a, _)| a)
}
fn net16(ip: &str) -> String {
    ip.split('.').take(2).collect::<Vec<_>>().join(".")
}

#[derive(Default, Clone)]
struct Ev {
    ts: f64,
    ip: String,
    ok: bool,
    member_quota: bool,
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
fn simultaneous(a: &[Ev], b: &[Ev]) -> (usize, usize) {
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
fn switches(a: &[Ev], b: &[Ev]) -> usize {
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
fn handoffs(a: &[Ev], b: &[Ev]) -> usize {
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
fn is_auto_username(u: &str) -> bool {
    let Some((head, tail)) = u.rsplit_once('_') else {
        return false;
    };
    !head.is_empty() && (4..=5).contains(&tail.len()) && tail.bytes().all(|b| b.is_ascii_digit())
}

fn subject(ids: &BTreeSet<String>) -> String {
    ids.iter().cloned().collect::<Vec<_>>().join(",")
}

/// 同浏览器 / 同设备类信号：关联的 Discord 账号里只有一个持有 Key、其余都没有 Key 时，
/// 多半是一个人换了号登录（或还没领 Key 的新号），不是两个人各拿一把 Key：分数减半，并补一条证据。
/// 两个及以上账号持有 Key 时原样返回。
fn lower_if_solo_key_holder(score: f64, holders: usize, accounts: usize) -> (f64, Option<Value>) {
    if holders == 1 && accounts >= 2 {
        (score / 2.0, Some(ev("其余账号", "无 Key")))
    } else {
        (score, None)
    }
}

fn ev(k: &str, v: impl ToString) -> Value {
    json!({"k": k, "v": v.to_string()})
}

/// 模糊指纹：去掉浏览器版本 / 语言这类会变的项，只留跟硬件走的。
fn fuzzy_key(parts: &Value) -> Option<String> {
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

struct Signal {
    kind: &'static str,
    subject: String,
    key_ids: String,
    score: f64,
    detail: Value,
    /// 封禁号关联：新进待审时记 Bug 用的说明
    alert: Option<String>,
    /// 涉及的账号已经全部封禁：信号不再带来新信息，写进去就标成已确认
    all_banned: bool,
}

pub async fn run(state: &AppState) -> Result<String, JobError> {
    let now = state.now();
    let since_beh = now - BEHAVIOR_DAYS * DAY;
    let gate_since: f64 = state
        .db
        .get_setting(crate::auth::WEB_LOGIN_SINCE_KEY)
        .await?
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0.0);

    // ASN 库：宿主机每周用 curl 更新到数据目录（deploy/v3/ops/asn-update.sh）；没有这个文件就跳过「共享 Key」这一项
    let asn_path = state.db.path().parent().map(|d| d.join(ASN_FILE));
    let asn = match asn_path.filter(|p| p.exists()) {
        Some(p) => tokio::task::spawn_blocking(move || AsnDb::load(&p).ok())
            .await
            .ok()
            .flatten(),
        None => None,
    };

    let clock = state.clock.clone();
    // 官方 origin（精确匹配，不猜子域）：本站 + 官方站。之外的来源说明 Key 被填进了第三方网站。
    let mut official = vec!["https://novelai.net".to_string()];
    let site = crate::client_hints::origin_only(&state.env.site_url);
    if !site.is_empty() {
        official.push(site);
    }
    let signals = state
        .db
        .read(move |c| {
            let mut out: Vec<Signal> = Vec::new();
            // Discord 账号 → Key（启用）
            let mut keys_of: HashMap<String, Vec<i64>> = HashMap::new();
            let mut did_of: HashMap<i64, String> = HashMap::new();
            {
                let mut st = c.prepare(
                    "SELECT id, discord_id FROM api_keys WHERE enabled=1 AND COALESCE(discord_id,'')!=''",
                )?;
                let rows = st.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?;
                for row in rows {
                    let (k, d) = row?;
                    keys_of.entry(d.clone()).or_default().push(k);
                    did_of.insert(k, d);
                }
            }
            // 封禁名单：Discord ID → 名字 / 理由
            let mut bans: HashMap<String, BanInfo> = HashMap::new();
            {
                let mut st = c.prepare(
                    "SELECT discord_id, reason, COALESCE(NULLIF(display_name,''), NULLIF(username,''), '') FROM discord_bans",
                )?;
                let rows = st.query_map([], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?))
                })?;
                for row in rows {
                    let (d, reason, name) = row?;
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
            let mut events: HashMap<i64, Vec<Ev>> = HashMap::new();
            {
                let mut st = c.prepare(
                    "SELECT key_id, ts, COALESCE(src,''), status, COALESCE(reason,'') FROM usage_log \
                     WHERE ts>=? ORDER BY ts",
                )?;
                let rows = st.query_map([since_beh], |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        Ev {
                            ts: r.get(1)?,
                            ip: r.get(2)?,
                            ok: r.get::<_, String>(3)? == "ok",
                            member_quota: r.get::<_, String>(4)? == "member_quota",
                        },
                    ))
                })?;
                for row in rows {
                    let (k, e) = row?;
                    if did_of.contains_key(&k) {
                        events.entry(k).or_default().push(e);
                    }
                }
            }
            let ips: HashMap<i64, HashSet<&str>> = events
                .iter()
                .map(|(k, es)| (*k, es.iter().filter(|e| !e.ip.is_empty()).map(|e| e.ip.as_str()).collect()))
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
                    let Some(score) = pair_score(&e) else { continue };
                    let ids: BTreeSet<String> = [da.clone(), db.clone()].into();
                    let mut evidence = vec![
                        ev("同 IP", format!("{} 个", e.shared_ip)),
                        ev("同网同时", format!("{} 次", e.simul_same_net)),
                        ev("换号续图", format!("{} 次", e.handoffs)),
                    ];
                    if let Some(h) = e.created_gap_h {
                        evidence.push(ev("注册相差", if h < 48.0 { format!("{h:.0} 小时") } else { format!("{:.0} 天", h / 24.0) }));
                    }
                    if e.simul_diff_net > 0 {
                        evidence.push(ev("异地同时", format!("{} 次", e.simul_diff_net)));
                    }
                    if e.switches > 0 {
                        evidence.push(ev("轮流使用", format!("{} 次", e.switches)));
                    }
                    out.push(Signal {
 alert: None, all_banned: false,
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
                let mut st = c.prepare("SELECT device_id, data FROM device_fingerprints")?;
                let rows = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
                for row in rows {
                    let (d, data) = row?;
                    parts_of.insert(d, ban_link::device_parts(&data));
                }
            }
            let mut by_dev: HashMap<String, BTreeSet<String>> = HashMap::new();
            {
                let mut st = c.prepare("SELECT discord_id, device_id FROM device_logins WHERE device_id!=''")?;
                let rows = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
                for row in rows {
                    let (d, dev) = row?;
                    by_dev.entry(dev).or_default().insert(d);
                }
            }
            let mut by_fuzzy: HashMap<String, (BTreeSet<String>, BTreeSet<String>, bool)> = HashMap::new();
            for (dev, ids) in &by_dev {
                let parts = parts_of.get(dev).cloned().unwrap_or(Value::Null);
                let apple = is_apple(&parts);
                if ids.len() >= 2 {
                    let gpu = parts.get("gpuRenderer").and_then(Value::as_str).unwrap_or("未知");
                    let (score, chip) = lower_if_solo_key_holder(if apple { 30.0 } else { 60.0 }, holders(ids), ids.len());
                    let mut evidence = vec![ev("账号", ids.len()), ev("显卡", gpu), ev("苹果", if apple { "是（易撞，降权）" } else { "否" })];
                    evidence.extend(chip);
                    out.push(Signal {
 alert: None, all_banned: false,
                        kind: "device_exact",
                        subject: subject(ids),
                        key_ids: kids(ids),
                        score,
                        detail: json!({
                            "summary": format!("{} 个账号在同一台设备上网页登录过", ids.len()),
                            "evidence": evidence,
                        }),
                    });
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
                let (score, chip) = lower_if_solo_key_holder(if *apple { 20.0 } else { 45.0 }, holders(ids), ids.len());
                let mut evidence = vec![ev("账号", ids.len()), ev("指纹", devs.len()), ev("显卡", if gpu.is_empty() { "未知" } else { gpu })];
                evidence.extend(chip);
                out.push(Signal {
 alert: None, all_banned: false,
                    kind: "device_fuzzy",
                    subject: subject(ids),
                    key_ids: kids(ids),
                    score,
                    detail: json!({
                        "summary": format!("{} 个账号的设备硬件特征一致（指纹 ID 不同，可能换了浏览器）", ids.len()),
                        "evidence": evidence,
                    }),
                });
            }

            // ---------- 登录时的指纹状态 ----------
            {
                let mut st = c.prepare(
                    "SELECT discord_id, fp, last_ts FROM web_logins WHERE fp IN ('none','unknown') AND last_ts>=?",
                )?;
                let rows = st.query_map([gate_since.max(now - 30.0 * DAY)], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, f64>(2)?))
                })?;
                for row in rows {
                    let (d, fp, _) = row?;
                    if !keys_of.contains_key(&d) {
                        continue;
                    }
                    let ids: BTreeSet<String> = [d].into();
                    let unknown = fp == "unknown";
                    out.push(Signal {
 alert: None, all_banned: false,
                        kind: if unknown { "fp_unknown" } else { "fp_missing" },
                        subject: subject(&ids),
                        key_ids: kids(&ids),
                        score: if unknown { 50.0 } else { 15.0 },
                        detail: json!({
                            "summary": if unknown { "网页登录时交了服务端不认得的设备指纹（疑似伪造）" } else { "网页登录时没交设备指纹（脚本被拦或直接打开登录链接）" },
                            "evidence": [ev("指纹", if unknown { "对不上" } else { "没交" })],
                        }),
                    });
                }
            }

            // ---------- 新号 / 刚进服 / 退了又进 ----------
            {
                let mut st = c.prepare(
                    "SELECT discord_id, created_at, guild_joined_at, COALESCE(member_flags,0), \
                     COALESCE(avatar,''), COALESCE(username,'') FROM discord_registrations WHERE created_at>=?",
                )?;
                let rows = st.query_map([now - 30.0 * DAY], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, f64>(1)?,
                        r.get::<_, Option<f64>>(2)?,
                        r.get::<_, i64>(3)?,
                        r.get::<_, String>(4)?,
                        r.get::<_, String>(5)?,
                    ))
                })?;
                for row in rows {
                    let (d, reg, joined, flags, avatar, username) = row?;
                    if !keys_of.contains_key(&d) {
                        continue;
                    }
                    let age = snowflake_created_at(&d).map(|born| (reg - born) / DAY);
                    let young = age.is_some_and(|a| a < 30.0) && reg >= now - 14.0 * DAY;
                    let join_gap = joined.map(|j| (reg - j) / DAY);
                    let fresh_join = join_gap.is_some_and(|g| (0.0..1.0).contains(&g));
                    let rejoined = flags & crate::registration::MEMBER_FLAG_DID_REJOIN != 0;
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
 alert: None, all_banned: false,
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
                let mut st = c.prepare("SELECT discord_id, browser_id FROM browser_logins")?;
                let rows = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
                for row in rows {
                    let (d, b) = row?;
                    by_bid.entry(b).or_default().insert(d);
                }
                for ids in by_bid.values().filter(|ids| ids.len() >= 2) {
                    let (score, chip) = lower_if_solo_key_holder(80.0, holders(ids), ids.len());
                    let mut evidence = vec![ev("账号", ids.len()), ev("浏览器", "同一个")];
                    evidence.extend(chip);
                    out.push(Signal {
 alert: None, all_banned: false,
                        kind: "same_browser",
                        subject: subject(ids),
                        key_ids: kids(ids),
                        score,
                        detail: json!({
                            "summary": format!("{} 个 Discord 账号在同一个浏览器里网页登录过（浏览器标记一模一样）", ids.len()),
                            "evidence": evidence,
                        }),
                    });
                }
            }

            // ---------- 和封禁号同浏览器 / 同设备（见 crate::ban_link） ----------
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
                let mut st = c.prepare("SELECT discord_id, browser_id FROM browser_logins WHERE browser_id!=''")?;
                let rows = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
                for row in rows {
                    let (d, b) = row?;
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
                    let (score, detail) = ban_link::signal(b, info, w);
                    let ids: BTreeSet<String> = [x.clone(), b.clone()].into();
                    out.push(Signal {
                        kind: ban_link::KIND,
                        subject: subject(&ids),
                        key_ids: kids(&ids),
                        score,
                        detail,
                        alert: Some(ban_link::alert_text(x, b, info, w)),
                        all_banned: false,
                    });
                }
            }

            // ---------- 共享 Key（要 ASN 库） ----------
            if let Some(db) = asn.as_ref() {
                for (&k, es) in &events {
                    // 开关 VPN / 代理会在「家里宽带」和「机房 IP」之间来回跳，不算共享：机房 IP 的请求不参与比较
                    let recent: Vec<(f64, u32, &str)> = es
                        .iter()
                        .filter(|e| e.ts >= now - DAY && !e.ip.is_empty())
                        .filter_map(|e| db.lookup(&e.ip).map(|a| (e.ts, a, e.ip.as_str())))
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
            if let Some(db) = asn.as_ref() {
                let mut hosting: HashMap<String, usize> = HashMap::new();
                for sql in [
                    "SELECT discord_id, ip FROM device_logins WHERE ip!=''",
                    "SELECT discord_id, ip FROM browser_logins WHERE ip!=''",
                    "SELECT discord_id, ip FROM web_logins WHERE ip!=''",
                ] {
                    let mut st = c.prepare(sql)?;
                    let rows = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
                    for row in rows {
                        let (d, ip) = row?;
                        if db.lookup(&ip).is_some_and(|a| a.hosting) {
                            *hosting.entry(d).or_default() += 1;
                        }
                    }
                }
                for sig in &mut out {
                    if sig.subject.starts_with("key:") {
                        continue;
                    }
                    let n = sig.subject.split(',').filter(|d| hosting.contains_key(*d)).count();
                    if n > 0
                        && let Some(arr) = sig.detail.get_mut("evidence").and_then(Value::as_array_mut)
                    {
                        arr.push(ev("机房 IP 登录", format!("{n} 个号")));
                    }
                }
            }
            // ---------- 关联扫描（同 IP 换号 / 同时异地 / 同参考图 / 同外部账号 / 服务器重合 / 电量相同 / 第三方站点来源） ----------
            out.extend(linkage::scan(c, now, &clock, &keys_of, &did_of, &official)?);
            for sig in &mut out {
                sig.all_banned = !sig.subject.starts_with("key:")
                    && sig.subject.split(',').all(|d| bans.contains_key(d));
            }
            out.retain(|s| !s.key_ids.is_empty() || s.kind == ban_link::KIND);
            Ok(out)
        })
        .await?;

    let mut counts: HashMap<&str, usize> = HashMap::new();
    for s in &signals {
        *counts.entry(s.kind).or_default() += 1;
        let prev = match s.alert {
            Some(_) => state.db.abuse_signal_status(s.kind, &s.subject).await?,
            None => None,
        };
        let id = state
            .db
            .upsert_abuse_signal(
                s.kind,
                &s.subject,
                &s.key_ids,
                s.score,
                &s.detail.to_string(),
                now,
            )
            .await?;
        if s.all_banned && state.db.confirm_open_abuse_signal(id).await? {
            *counts.entry("全已封禁").or_default() += 1;
        }
        if let Some(text) = &s.alert {
            let after = state.db.abuse_signal_status(s.kind, &s.subject).await?;
            ban_link::alert_if_opened(state, prev.as_deref(), after.as_deref(), text);
        }
    }
    // 这一轮没再出现的（比如证据过了 14 天窗口、规则改严了），而且站长还没处理过的：标成过期，记录留着
    let expired = state.db.expire_stale_open_abuse_signals(now).await?;
    if expired > 0 {
        *counts.entry("过期").or_default() += expired;
    }
    let mut parts: Vec<String> = counts.iter().map(|(k, n)| format!("{k} {n}")).collect();
    parts.sort();
    Ok(if parts.is_empty() {
        "没有新的可疑信号".to_string()
    } else {
        format!("可疑信号：{}", parts.join("，"))
    })
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
        let at = |t: f64| Ev {
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
        let a = vec![Ev {
            ts: 100.0,
            ip: "1.2.3.4".into(),
            ..Default::default()
        }];
        let b = vec![
            Ev {
                ts: 130.0,
                ip: "1.2.9.9".into(),
                ..Default::default()
            },
            Ev {
                ts: 150.0,
                ip: "8.8.8.8".into(),
                ..Default::default()
            },
            Ev {
                ts: 500.0,
                ip: "1.2.3.4".into(),
                ..Default::default()
            },
        ];
        assert_eq!(simultaneous(&a, &b), (1, 1));
    }

    #[test]
    fn handoff_needs_quota_hit_then_success_on_same_subnet() {
        let a = vec![Ev {
            ts: 0.0,
            ip: "10.0.0.5".into(),
            member_quota: true,
            ..Default::default()
        }];
        let b = vec![
            Ev {
                ts: 120.0,
                ip: "10.0.0.9".into(),
                ok: true,
                ..Default::default()
            },
            Ev {
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
