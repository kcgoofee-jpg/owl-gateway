//! `nai-ops report`：只读的「周报」——每把 Key 的滥用 / 健康信号。
//!
//! 输出里**只有 Key 编号、显示名和计数**：不打印提示词、UA 原文、IP、Discord ID。
//!
//! 小号 / 共用筛查按 **Discord 账号**算（同一账号换过的新旧 Key 合并）：Key 的归属依次看 `api_keys.discord_id`、
//! 删 Key 前留下的 `deleted_key_owners`、`discord_registrations`，都没有的旧 Key 再按显示名里的 `(@用户名)`
//! 归到同名账号上；再认不出的，没账号的 Key 之间完全同名的算一个账号，其余各自算一个。
//! 内容风险一节的词表放在服务器上的外部文件（默认 `/opt/owl/risk-terms.txt`，环境变量 `NAI_RISK_TERMS`
//! 或 `--terms` 可改），仓库只带一个全是注释的示例（`deploy/risk-terms.example.txt`）；报告里只有每个分类的
//! 命中条数和 Key 编号。输出顺序固定（按 Key 编号），配合合成数据库可做快照测试。
//!
//! 词表格式：`# 分类名` 开一个分类，其后每行一条正则（不区分大小写）；`##` 开头是注释；空行忽略。
//! 各项阈值见下面的常量。

use crate::util;
use anyhow::{Context, Result};
use gate_core::snowflake::snowflake_created_at;
use jiff::tz::TimeZone;
use regex::{Regex, RegexBuilder};
use rusqlite::Connection;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

pub const DEFAULT_DAYS: i64 = 7;
pub const DEFAULT_TERMS: &str = "/opt/owl/risk-terms.txt";

/// key_rate + key_busy 拒绝次数 ≥ 这个数才列出。
pub const HAMMER_MIN: usize = 20;
/// 两次请求间隔不超过这么多秒，算「连续不断」的一段。
pub const RUN_GAP_S: f64 = 120.0;
/// 最长连续一段 ≥ 这么多分钟才列出。
pub const LONG_RUN_MIN: f64 = 180.0;
/// 一周里有这么多天碰到个人额度上限才列出。
pub const SATURATION_DAYS_MIN: usize = 3;
/// 提示词至少这么长才参与「多 Key 相同提示词」比对。
pub const LONG_PROMPT_CHARS: usize = 80;
/// UA 摘要至少这么长才算「具体设备」（短的只是客户端名称，很多人共用）。
pub const SPECIFIC_UA_CHARS: usize = 24;
/// Discord 账号在领 Key 时不足这么多天算新号。
pub const NEW_ACCOUNT_DAYS: f64 = 7.0;
/// 领取突发：这么多秒内……
pub const BURST_WINDOW_S: f64 = 600.0;
/// ……领了不少于这么多把。
pub const BURST_MIN: usize = 5;
/// 一个 UA 被不超过这么多个账号用过，才算「罕见设备」（常见浏览器 UA 动辄几十人共用，说明不了问题）。
pub const RARE_UA_MAX_ACCOUNTS: usize = 3;
/// 两个账号共用至少这么多个罕见设备才列出；「同一完整 IP + 同一 UA」有一组就列。
pub const SHARED_RARE_UA_MIN: usize = 2;
/// 提示词相似度：账号在窗口里至少有这么多条提示词……
pub const SIM_MIN_PROMPTS: usize = 3;
/// ……去掉只有它一个账号用过的词以后还剩至少这么多个词，才参与比较。
pub const SIM_MIN_TOKENS: usize = 5;
/// 相似度不低于全站两两相似度的这个分位、并且设备或 IP 有重合，才列出。
pub const SIM_QUANTILE: f64 = 0.999;
/// 一行里最多列几把 Key。
const LIST_MAX: usize = 15;
/// 每节最多列几对账号。
const PAIR_MAX: usize = 30;

#[derive(Debug, Clone)]
pub struct ReportArgs {
    pub days: i64,
    pub terms: Option<PathBuf>,
    /// 统计窗口的结束时刻（Unix 秒）
    pub until: f64,
}

// ---------------------------------------------------------------- 纯逻辑

pub fn median(v: &mut [f64]) -> Option<f64> {
    if v.is_empty() {
        return None;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n = v.len();
    Some(if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) / 2.0
    })
}

/// 最长的「连续不断」一段：返回 (张数, 持续秒数)。`ts` 需升序。
pub fn longest_run(ts: &[f64], max_gap: f64) -> (usize, f64) {
    let mut best = (0usize, 0.0f64);
    let mut start = 0usize;
    for i in 0..ts.len() {
        if i > 0 && ts[i] - ts[i - 1] > max_gap {
            start = i;
        }
        let cand = (i - start + 1, ts[i] - ts[start]);
        if cand.1 > best.1 || (cand.1 == best.1 && cand.0 > best.0) {
            best = cand;
        }
    }
    best
}

/// 设备类别（凭 UA 摘要里的关键词；认不出归「其它」，不参与「多设备」判断）。
pub fn device_class(client: &str) -> &'static str {
    let c = client.to_ascii_lowercase();
    if c.contains("iphone") || c.contains("ipad") || c.contains("ios") {
        "ios"
    } else if c.contains("android") {
        "android"
    } else if c.contains("windows") {
        "windows"
    } else if c.contains("macintosh") || c.contains("mac os") || c.contains("macos") {
        "mac"
    } else if c.contains("linux") || c.contains("x11") {
        "linux"
    } else {
        "other"
    }
}

/// 领取突发：滑动窗口里 ≥ `min` 次领取。`regs` 需按时间升序。返回 (起点, 张数, Key 编号)。
pub fn claim_bursts(regs: &[(f64, i64)], window: f64, min: usize) -> Vec<(f64, usize, Vec<i64>)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < regs.len() {
        let mut j = i;
        while j + 1 < regs.len() && regs[j + 1].0 - regs[i].0 <= window {
            j += 1;
        }
        if j - i + 1 >= min {
            out.push((
                regs[i].0,
                j - i + 1,
                regs[i..=j].iter().map(|r| r.1).collect(),
            ));
            i = j + 1;
        } else {
            i += 1;
        }
    }
    out
}

/// 解析词表：返回 (分类, 正则) 与无法编译的行号。
pub fn parse_terms(text: &str) -> (Vec<(String, Regex)>, Vec<usize>) {
    let mut cat = "未分类".to_string();
    let mut out = Vec::new();
    let mut bad = Vec::new();
    for (i, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with("##") {
            continue;
        }
        if let Some(name) = line.strip_prefix('#') {
            let name = name.trim();
            if !name.is_empty() {
                cat = name.to_string();
            }
            continue;
        }
        match RegexBuilder::new(line)
            .case_insensitive(true)
            .size_limit(1 << 20)
            .build()
        {
            Ok(re) => out.push((cat.clone(), re)),
            Err(_) => bad.push(i + 1),
        }
    }
    (out, bad)
}

/// 提示词按逗号（和换行）切成词：小写，去掉首尾空白、括号和 `1.2::` 这类权重写法。
pub fn prompt_tokens(prompt: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for raw in prompt.split([',', '\n']) {
        let mut t = raw.trim().to_lowercase();
        if let Some((w, rest)) = t.split_once("::")
            && w.trim().parse::<f64>().is_ok()
        {
            t = rest.to_string();
        }
        let t = t.replace("::", "");
        let t = t.trim_matches(|c: char| c.is_whitespace() || "{}[]()".contains(c));
        if !t.is_empty() {
            out.insert(t.to_string());
        }
    }
    out
}

/// 两个升序、无重复的词编号列表的 Jaccard 相似度（交集 / 并集）；都为空算 0。
pub fn jaccard(a: &[u32], b: &[u32]) -> f64 {
    let (mut i, mut j, mut inter) = (0, 0, 0usize);
    while i < a.len() && j < b.len() {
        match a[i].cmp(&b[j]) {
            std::cmp::Ordering::Less => i += 1,
            std::cmp::Ordering::Greater => j += 1,
            std::cmp::Ordering::Equal => {
                inter += 1;
                i += 1;
                j += 1;
            }
        }
    }
    let union = a.len() + b.len() - inter;
    if union == 0 {
        0.0
    } else {
        inter as f64 / union as f64
    }
}

/// 显示名里的 Discord 用户名：领 Key 时记的是「显示名 (@用户名)」，或者直接就是用户名。
/// 用来把没登记账号的旧 Key（2.x 时期的、过期释放的）归到同一个人身上。认不出返回 None。
pub fn name_handle(name: &str) -> Option<String> {
    let n = name.trim();
    let h = match n
        .strip_suffix(')')
        .and_then(|r| r.rfind("(@").map(|p| &r[p + 2..]))
    {
        Some(h) => h.trim().to_lowercase(),
        None => n.to_lowercase(),
    };
    let ok = (2..=32).contains(&h.len())
        && h.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '.');
    ok.then_some(h)
}

/// `src` 是完整 IP（3.0 起）；2.x 留下的 `a.b.*.*` 这类打码标签和空串不算。
pub fn is_full_ip(src: &str) -> bool {
    src.parse::<std::net::IpAddr>().is_ok()
}

fn clean(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect::<String>()
        .trim()
        .chars()
        .take(24)
        .collect()
}

// ---------------------------------------------------------------- 取数与排版

struct Keys {
    names: BTreeMap<i64, String>,
}

impl Keys {
    fn label(&self, id: i64) -> String {
        match self.names.get(&id) {
            Some(n) if !n.is_empty() => format!("#{id} {n}"),
            _ => format!("#{id}"),
        }
    }
    fn list(&self, ids: &BTreeSet<i64>) -> String {
        let shown: Vec<String> = ids.iter().take(LIST_MAX).map(|i| self.label(*i)).collect();
        let more = ids.len().saturating_sub(LIST_MAX);
        if more > 0 {
            format!("{}……另有 {more} 把", shown.join("、"))
        } else {
            shown.join("、")
        }
    }
}

fn section(out: &mut String, title: &str, lines: Vec<String>) {
    out.push_str(&format!("\n[{title}]\n"));
    if lines.is_empty() {
        out.push_str("  （无）\n");
    }
    for l in lines {
        out.push_str(&format!("  {l}\n"));
    }
}

struct LogRow {
    ts: f64,
    key: i64,
    kind: String,
    reason: String,
    client: String,
    src: String,
    ok: bool,
}

/// 账号：同一 Discord 账号的新旧 Key 合并成一个。按最小的 Key 编号排序，输出顺序固定。
struct Accounts {
    of: BTreeMap<i64, usize>,
    keys: Vec<BTreeSet<i64>>,
}

impl Accounts {
    fn build(
        all: &BTreeSet<i64>,
        discord: &BTreeMap<i64, String>,
        raw: &BTreeMap<i64, String>,
    ) -> Self {
        // 有账号的 Key 的用户名 → 账号；没账号的旧 Key 按同一个用户名归过去
        let mut by_handle: BTreeMap<String, &str> = BTreeMap::new();
        for (k, d) in discord {
            if let Some(h) = raw.get(k).and_then(|n| name_handle(n)) {
                by_handle.entry(h).or_insert(d);
            }
        }
        let mut groups: BTreeMap<String, BTreeSet<i64>> = BTreeMap::new();
        for &k in all {
            let id = match discord.get(&k) {
                Some(d) => format!("d:{d}"),
                None => match raw.get(&k).and_then(|n| name_handle(n)) {
                    Some(h) => by_handle
                        .get(&h)
                        .map_or_else(|| format!("h:{h}"), |d| format!("d:{d}")),
                    // 没账号、也认不出用户名的 Key（后台手动发的、2.x 的）：完全同名的算同一个人
                    None => match raw.get(&k).map(|n| n.trim()).filter(|n| !n.is_empty()) {
                        Some(n) => format!("n:{n}"),
                        None => format!("k:{k}"),
                    },
                },
            };
            groups.entry(id).or_default().insert(k);
        }
        let mut keys: Vec<BTreeSet<i64>> = groups.into_values().collect();
        keys.sort_by_key(|s| s.first().copied());
        let of = keys
            .iter()
            .enumerate()
            .flat_map(|(i, s)| s.iter().map(move |k| (*k, i)))
            .collect();
        Accounts { of, keys }
    }

    fn get(&self, key: i64) -> usize {
        self.of[&key]
    }

    /// 「#新 Key 显示名（旧 Key #a、#b）」
    fn label(&self, a: usize, keys: &Keys) -> String {
        let s = &self.keys[a];
        let newest = *s.last().expect("账号至少有一把 Key");
        let older: Vec<String> = s.iter().rev().skip(1).map(|k| format!("#{k}")).collect();
        if older.is_empty() {
            keys.label(newest)
        } else {
            format!("{}（旧 Key {}）", keys.label(newest), older.join("、"))
        }
    }
}

/// 两个账号之间的重合：共用罕见 UA 几个、共用完整 IP 几个、共用「完整 IP + UA」几组。
#[derive(Default)]
struct Overlap {
    rare_ua: usize,
    ip: usize,
    ip_ua: usize,
}

/// 集合里两两配对（升序）。
fn pairs_of(set: &BTreeSet<usize>) -> Vec<(usize, usize)> {
    let v: Vec<usize> = set.iter().copied().collect();
    let mut out = Vec::new();
    for (i, a) in v.iter().enumerate() {
        for b in &v[i + 1..] {
            out.push((*a, *b));
        }
    }
    out
}

fn cap_lines(mut lines: Vec<String>) -> Vec<String> {
    let more = lines.len().saturating_sub(PAIR_MAX);
    lines.truncate(PAIR_MAX);
    if more > 0 {
        lines.push(format!("……另有 {more} 对"));
    }
    lines
}

pub fn build(conn: &Connection, args: &ReportArgs, tz: &TimeZone) -> Result<String> {
    // 整份报告在同一个读事务里：线上库边跑边写，前后几次查询看到的是同一份快照
    let tx = conn.unchecked_transaction()?;
    let conn: &Connection = &tx;
    let until = args.until;
    let since = until - args.days as f64 * 86400.0;
    let day = |ts: f64| util::fmt_ts(ts, tz, "%Y-%m-%d");

    // Key 名单；raw 是没截断的显示名（认 `(@用户名)` 用），discord 是 Key → Discord 账号
    let mut keys = Keys {
        names: BTreeMap::new(),
    };
    let mut raw: BTreeMap<i64, String> = BTreeMap::new();
    let mut discord: BTreeMap<i64, String> = BTreeMap::new();
    let mut live: BTreeSet<i64> = BTreeSet::new();
    {
        let mut st = conn.prepare("SELECT id, name, discord_id FROM api_keys")?;
        let rows = st.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
            ))
        })?;
        for r in rows {
            let (id, name, d) = r?;
            live.insert(id);
            keys.names.insert(id, clean(&name));
            raw.insert(id, name);
            if let Some(d) = d.filter(|d| !d.is_empty()) {
                discord.insert(id, d);
            }
        }
    }
    // 删 Key 前留下的归属（3.0 新增，保留 14 天）
    let mut owned_deleted: BTreeSet<i64> = BTreeSet::new();
    let mut st = conn.prepare("SELECT key_id, discord_id, name FROM deleted_key_owners")?;
    let rows = st.query_map([], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, Option<String>>(1)?,
            r.get::<_, String>(2)?,
        ))
    })?;
    for r in rows {
        let (id, d, name) = r?;
        if live.contains(&id) {
            continue;
        }
        owned_deleted.insert(id);
        keys.names.entry(id).or_insert_with(|| clean(&name));
        raw.entry(id).or_insert(name);
        if let Some(d) = d.filter(|d| !d.is_empty()) {
            discord.insert(id, d);
        }
    }
    // 领取记录兜底（Key 上没写 discord_id 的老行）
    {
        let mut st = conn.prepare("SELECT key_id, discord_id FROM discord_registrations")?;
        let rows = st.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?;
        for r in rows {
            let (k, d) = r?;
            discord.entry(k).or_insert(d);
        }
    }

    // 用量日志
    let mut logs: Vec<LogRow> = Vec::new();
    {
        let mut st = conn.prepare(
            "SELECT ts, key_id, kind, reason, client, src, status, key_name FROM usage_log \
             WHERE ts>=? AND ts<? AND key_id IS NOT NULL ORDER BY ts, id",
        )?;
        let rows = st.query_map([since, until], |r| {
            Ok((
                LogRow {
                    ts: r.get(0)?,
                    key: r.get(1)?,
                    kind: r.get(2)?,
                    reason: r.get(3)?,
                    client: r.get(4)?,
                    src: r.get(5)?,
                    ok: r.get::<_, String>(6)? == "ok",
                },
                r.get::<_, String>(7)?,
            ))
        })?;
        for r in rows {
            let (r, name) = r?;
            // 已删且没有归属记录的 Key：用日志里记的显示名
            keys.names.entry(r.key).or_insert_with(|| clean(&name));
            raw.entry(r.key).or_insert(name);
            logs.push(r);
        }
    }
    let active: BTreeSet<i64> = logs.iter().map(|l| l.key).collect();
    let mut st = conn.prepare(
        "SELECT DISTINCT key_id, key_name FROM generation_audit \
         WHERE ts>=? AND ts<? AND key_id IS NOT NULL",
    )?;
    let rows = st.query_map([since, until], |r| {
        Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
    })?;
    for r in rows {
        let (k, name) = r?;
        keys.names.entry(k).or_insert_with(|| clean(&name));
        raw.entry(k).or_insert(name);
    }
    let all: BTreeSet<i64> = keys.names.keys().copied().collect();
    let accts = Accounts::build(&all, &discord, &raw);

    let mut out = format!(
        "=== 周报：近 {} 天（{} ~ {}）===\n有请求的 Key：{} 把\n",
        args.days,
        util::fmt_ts(since, tz, "%Y-%m-%d %H:%M"),
        util::fmt_ts(until, tz, "%Y-%m-%d %H:%M"),
        active.len()
    );

    // 1. 重试轰炸
    let mut rate: BTreeMap<i64, (usize, usize, Vec<f64>)> = BTreeMap::new();
    for l in &logs {
        let (a, b) = match l.reason.as_str() {
            "key_rate" => (1, 0),
            // member_queue_full 是 10-22 起 key_busy 的新码（每人排队已满），一起算
            "key_busy" | "member_queue_full" => (0, 1),
            _ => continue,
        };
        let e = rate.entry(l.key).or_default();
        e.0 += a;
        e.1 += b;
        e.2.push(l.ts);
    }
    let mut lines = Vec::new();
    for (k, (a, b, ts)) in &rate {
        if a + b < HAMMER_MIN {
            continue;
        }
        let mut gaps: Vec<f64> = ts.windows(2).map(|w| w[1] - w[0]).collect();
        let med = median(&mut gaps).map_or("n/a".into(), |m| format!("{m:.1}s"));
        lines.push(format!(
            "{}  key_rate={a} key_busy={b} 中位间隔={med}",
            keys.label(*k)
        ));
    }
    section(
        &mut out,
        &format!("重试轰炸：key_rate + key_busy ≥ {HAMMER_MIN}"),
        lines,
    );

    // 2. 最长连续
    let mut img_ts: BTreeMap<i64, Vec<f64>> = BTreeMap::new();
    for l in logs.iter().filter(|l| l.kind.starts_with("image")) {
        img_ts.entry(l.key).or_default().push(l.ts);
    }
    let mut lines = Vec::new();
    for (k, ts) in &img_ts {
        let (n, dur) = longest_run(ts, RUN_GAP_S);
        if dur / 60.0 >= LONG_RUN_MIN {
            lines.push(format!(
                "{}  最长连续 {:.1} 小时 · {n} 张（间隔不超过 {RUN_GAP_S:.0} 秒算连续）",
                keys.label(*k),
                dur / 3600.0
            ));
        }
    }
    section(
        &mut out,
        &format!("最长连续使用 ≥ {LONG_RUN_MIN:.0} 分钟"),
        lines,
    );

    // 3. 额度打满的天数
    let mut sat: BTreeMap<i64, BTreeSet<String>> = BTreeMap::new();
    for l in logs.iter().filter(|l| l.reason == "member_quota") {
        sat.entry(l.key).or_default().insert(day(l.ts));
    }
    let lines = sat
        .iter()
        .filter(|(_, d)| d.len() >= SATURATION_DAYS_MIN)
        .map(|(k, d)| format!("{}  {} 天碰到个人额度上限", keys.label(*k), d.len()))
        .collect();
    section(
        &mut out,
        &format!("个人额度打满的天数 ≥ {SATURATION_DAYS_MIN}"),
        lines,
    );

    // 4. 同一天多种设备类别
    let mut classes: BTreeMap<(i64, String), BTreeSet<&'static str>> = BTreeMap::new();
    for l in logs.iter().filter(|l| l.kind.starts_with("image")) {
        let c = device_class(&l.client);
        if c != "other" {
            classes.entry((l.key, day(l.ts))).or_default().insert(c);
        }
    }
    let mut multi: BTreeMap<i64, usize> = BTreeMap::new();
    for ((k, _), set) in &classes {
        if set.len() >= 2 {
            *multi.entry(*k).or_default() += 1;
        }
    }
    let lines = multi
        .iter()
        .map(|(k, n)| format!("{}  {n} 天里有出现 ≥ 2 类设备的", keys.label(*k)))
        .collect();
    section(&mut out, "同一天多种设备类别", lines);

    // 5 + 6. 不同账号共用罕见设备 / 同一完整 IP + 同一 UA；提示词相似 + 设备或 IP 重合
    let mut ua_accts: BTreeMap<&str, BTreeSet<usize>> = BTreeMap::new();
    let mut ip_accts: BTreeMap<&str, BTreeSet<usize>> = BTreeMap::new();
    let mut ip_ua_accts: BTreeMap<(&str, &str), BTreeSet<usize>> = BTreeMap::new();
    let mut hours: BTreeMap<usize, BTreeSet<i64>> = BTreeMap::new();
    let mut full_ip_rows = 0usize;
    for l in &logs {
        let a = accts.get(l.key);
        if l.client.chars().count() >= SPECIFIC_UA_CHARS {
            ua_accts.entry(&l.client).or_default().insert(a);
        }
        if is_full_ip(&l.src) {
            full_ip_rows += 1;
            ip_accts.entry(&l.src).or_default().insert(a);
            if !l.client.is_empty() {
                ip_ua_accts
                    .entry((&l.src, &l.client))
                    .or_default()
                    .insert(a);
            }
        }
        if l.ok && l.kind.starts_with("image") {
            hours
                .entry(a)
                .or_default()
                .insert((l.ts / 3600.0).floor() as i64);
        }
    }
    let mut overlap: BTreeMap<(usize, usize), Overlap> = BTreeMap::new();
    for set in ua_accts
        .values()
        .filter(|s| s.len() <= RARE_UA_MAX_ACCOUNTS)
    {
        for p in pairs_of(set) {
            overlap.entry(p).or_default().rare_ua += 1;
        }
    }
    for set in ip_accts.values() {
        for p in pairs_of(set) {
            overlap.entry(p).or_default().ip += 1;
        }
    }
    for set in ip_ua_accts.values() {
        for p in pairs_of(set) {
            overlap.entry(p).or_default().ip_ua += 1;
        }
    }
    let activity = |a: usize, b: usize| {
        let empty = BTreeSet::new();
        let (ha, hb) = (
            hours.get(&a).unwrap_or(&empty),
            hours.get(&b).unwrap_or(&empty),
        );
        format!(
            "活跃 {} / {} 小时，同一小时都活跃 {} 小时",
            ha.len(),
            hb.len(),
            ha.intersection(hb).count()
        )
    };
    let pair_label =
        |a: usize, b: usize| format!("{} ↔ {}", accts.label(a, &keys), accts.label(b, &keys));

    let mut found: Vec<(&(usize, usize), &Overlap)> = overlap
        .iter()
        .filter(|(_, o)| o.rare_ua >= SHARED_RARE_UA_MIN || o.ip_ua >= 1)
        .collect();
    found.sort_by_key(|(p, o)| (std::cmp::Reverse((o.ip_ua, o.rare_ua)), **p));
    let deleted: BTreeSet<i64> = active
        .iter()
        .filter(|k| !live.contains(k))
        .copied()
        .collect();
    let untraced = deleted
        .iter()
        .filter(|k| !owned_deleted.contains(k))
        .count();
    let mut lines = vec![format!(
        "（罕见设备 = 近 {} 天不超过 {RARE_UA_MAX_ACCOUNTS} 个账号用过的 UA；带完整 IP 的日志 {full_ip_rows} 条，只有前两段的老日志不比 IP）",
        args.days
    )];
    if !deleted.is_empty() {
        lines.push(format!(
            "（已删 Key 有日志的 {} 把，其中 {untraced} 把没有归属记录：按显示名归到账号，认不出的各算一个账号）",
            deleted.len()
        ));
    }
    lines.extend(cap_lines(
        found
            .iter()
            .map(|((a, b), o)| {
                format!(
                    "{}  罕见设备 {} 个 · 同 IP 同 UA {} 组 · {}",
                    pair_label(*a, *b),
                    o.rare_ua,
                    o.ip_ua,
                    activity(*a, *b)
                )
            })
            .collect(),
    ));
    section(
        &mut out,
        &format!("不同账号共用罕见设备（≥ {SHARED_RARE_UA_MIN} 个）或同一完整 IP + 同一 UA"),
        lines,
    );

    // 内容风险词表（第 10 节输出）：先读好解析好，下面扫提示词时顺手累计命中，不再读第二遍审计表
    let terms_path = args
        .terms
        .clone()
        .or_else(|| util::env_nonempty("NAI_RISK_TERMS").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from(DEFAULT_TERMS));
    let risk = std::fs::read_to_string(&terms_path).ok().map(|text| {
        let (terms, bad) = parse_terms(&text);
        let mut cats: Vec<String> = Vec::new();
        for (c, _) in &terms {
            if !cats.contains(c) {
                cats.push(c.clone());
            }
        }
        (terms, bad, cats)
    });
    let mut risk_hits: BTreeMap<&str, (usize, BTreeSet<i64>)> = BTreeMap::new();

    // 提示词：每个账号的词集合；同时找不同账号完全相同的长提示词（只显示短哈希）
    let mut vocab: HashMap<String, u32> = HashMap::new();
    let mut words: BTreeMap<usize, BTreeSet<u32>> = BTreeMap::new();
    let mut prompts: BTreeMap<usize, usize> = BTreeMap::new();
    let mut same: BTreeMap<String, (BTreeSet<usize>, BTreeSet<i64>, usize)> = BTreeMap::new();
    let mut st = conn.prepare(
        "SELECT key_id, prompt FROM generation_audit WHERE ts>=? AND ts<? AND key_id IS NOT NULL ORDER BY id",
    )?;
    let rows = st.query_map([since, until], |r| {
        Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
    })?;
    for r in rows {
        let (k, p) = r?;
        if let Some((terms, _, cats)) = &risk {
            for c in cats {
                if terms.iter().any(|(tc, re)| tc == c && re.is_match(&p)) {
                    let e = risk_hits.entry(c.as_str()).or_default();
                    e.0 += 1;
                    e.1.insert(k);
                }
            }
        }
        let a = accts.get(k);
        *prompts.entry(a).or_default() += 1;
        let set = words.entry(a).or_default();
        for t in prompt_tokens(&p) {
            let n = vocab.len() as u32;
            set.insert(*vocab.entry(t).or_insert(n));
        }
        if p.chars().count() >= LONG_PROMPT_CHARS {
            let h = hex::encode(Sha256::digest(p.trim().as_bytes()))[..8].to_string();
            let e = same.entry(h).or_default();
            e.0.insert(a);
            e.1.insert(k);
            e.2 += 1;
        }
    }
    let lines = same
        .iter()
        .filter(|(_, (a, _, _))| a.len() >= 2)
        .map(|(h, (a, ks, n))| {
            format!(
                "提示词#{h}：{n} 条，出现在 {} 个账号：{}",
                a.len(),
                keys.list(ks)
            )
        })
        .collect();
    section(
        &mut out,
        &format!("不同账号相同的长提示词（≥ {LONG_PROMPT_CHARS} 字）"),
        lines,
    );

    // 只有一个账号用过的词不可能重合，只会把用词多的人拉低，先去掉；
    // 去掉之后还剩够多的词、提示词也够多的账号才参与（词太少的随便两个都能「很像」）
    let mut df: HashMap<u32, usize> = HashMap::new();
    for (a, w) in &words {
        if prompts[a] >= SIM_MIN_PROMPTS {
            for t in w {
                *df.entry(*t).or_default() += 1;
            }
        }
    }
    let shared: BTreeMap<usize, Vec<u32>> = words
        .iter()
        .filter(|(a, _)| prompts[*a] >= SIM_MIN_PROMPTS)
        .map(|(a, w)| {
            (
                *a,
                w.iter()
                    .copied()
                    .filter(|t| df[t] >= 2)
                    .collect::<Vec<u32>>(),
            )
        })
        .filter(|(_, w)| w.len() >= SIM_MIN_TOKENS)
        .collect();
    let eligible: Vec<usize> = shared.keys().copied().collect();
    let mut sims: Vec<((usize, usize), f64)> = Vec::new();
    for (i, a) in eligible.iter().enumerate() {
        for b in &eligible[i + 1..] {
            sims.push(((*a, *b), jaccard(&shared[a], &shared[b])));
        }
    }
    let mut sorted: Vec<f64> = sims.iter().map(|s| s.1).collect();
    sorted.sort_by(f64::total_cmp);
    let mut lines = Vec::new();
    if let Some(cut) = util::nearest_rank(&sorted, SIM_QUANTILE) {
        lines.push(format!(
            "（基线：{} 个账号两两 {} 对，p{:.1} = {cut:.3}；只算两个以上账号都用过的词）",
            eligible.len(),
            sorted.len(),
            SIM_QUANTILE * 100.0
        ));
        let none = Overlap::default();
        let mut hits: Vec<((usize, usize), f64, &Overlap)> = sims
            .iter()
            .filter(|(_, s)| *s > 0.0 && *s >= cut)
            .map(|(p, s)| (*p, *s, overlap.get(p).unwrap_or(&none)))
            .filter(|(_, _, o)| o.rare_ua >= 1 || o.ip >= 1)
            .collect();
        hits.sort_by(|x, y| y.1.total_cmp(&x.1).then(x.0.cmp(&y.0)));
        let n = sorted.len() as f64;
        lines.extend(cap_lines(
            hits.iter()
                .map(|((a, b), s, o)| {
                    let top = sorted.iter().filter(|v| **v >= *s).count() as f64 / n * 100.0;
                    format!(
                        "{}  相似度 {s:.3}（全站前 {top:.2}%）· 罕见设备 {} 个 · 同 IP {} 个 · {}",
                        pair_label(*a, *b),
                        o.rare_ua,
                        o.ip,
                        activity(*a, *b)
                    )
                })
                .collect(),
        ));
    }
    section(
        &mut out,
        &format!(
            "提示词相似（≥ 全站 p{:.1}）且设备或 IP 有重合",
            SIM_QUANTILE * 100.0
        ),
        lines,
    );

    // 7 + 8. 领取时的 Discord 账号年龄、领取突发
    let mut regs: Vec<(f64, i64, Option<f64>)> = Vec::new();
    {
        let mut st = conn.prepare(
            "SELECT created_at, key_id, discord_id FROM discord_registrations \
             WHERE created_at>=? AND created_at<? ORDER BY created_at, key_id",
        )?;
        let rows = st.query_map([since, until], |r| {
            Ok((
                r.get::<_, f64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?;
        for r in rows {
            let (ts, k, did) = r?;
            regs.push((
                ts,
                k,
                snowflake_created_at(&did).map(|c| (ts - c) / 86400.0),
            ));
        }
    }
    let lines = regs
        .iter()
        .filter_map(|(_, k, age)| age.filter(|a| *a < NEW_ACCOUNT_DAYS).map(|a| (k, a)))
        .map(|(k, a)| format!("{}  领 Key 时 Discord 账号才 {a:.1} 天", keys.label(*k)))
        .collect();
    section(
        &mut out,
        &format!("领 Key 时 Discord 账号不足 {NEW_ACCOUNT_DAYS:.0} 天"),
        lines,
    );
    let plain: Vec<(f64, i64)> = regs.iter().map(|r| (r.0, r.1)).collect();
    let lines = claim_bursts(&plain, BURST_WINDOW_S, BURST_MIN)
        .into_iter()
        .map(|(start, n, ids)| {
            let set: BTreeSet<i64> = ids.into_iter().collect();
            format!(
                "{} 起 {:.0} 分钟内领了 {n} 把：{}",
                util::fmt_ts(start, tz, "%m-%d %H:%M"),
                BURST_WINDOW_S / 60.0,
                keys.list(&set)
            )
        })
        .collect();
    section(
        &mut out,
        &format!(
            "领取突发：{:.0} 分钟内 ≥ {BURST_MIN} 把",
            BURST_WINDOW_S / 60.0
        ),
        lines,
    );

    // 9. 现有暂停
    let mut lines = Vec::new();
    let mut st = conn.prepare(
        "SELECT key_id, paused_until, pause_reason FROM share_state WHERE paused_until>? ORDER BY key_id",
    )?;
    let rows = st.query_map([until], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, f64>(1)?,
            r.get::<_, String>(2)?,
        ))
    })?;
    for r in rows {
        let (k, until_ts, reason) = r?;
        let why = if reason.is_empty() {
            String::new()
        } else {
            format!(" · 原因：{reason}")
        };
        lines.push(format!(
            "{}  暂停到 {}{why}",
            keys.label(k),
            util::fmt_ts(until_ts, tz, "%m-%d %H:%M")
        ));
    }
    section(&mut out, "目前被暂停的 Key", lines);

    // 10. 内容风险（外部词表；命中在上面扫提示词时已经累计好）
    out.push_str("\n[内容风险（只列分类计数和 Key，不显示提示词）]\n");
    match &risk {
        None => out.push_str(&format!(
            "  未配置词表（{}），跳过。示例见仓库 deploy/risk-terms.example.txt\n",
            terms_path.display()
        )),
        Some((_, bad, cats)) => {
            if cats.is_empty() {
                out.push_str("  词表里没有任何规则，跳过\n");
            } else {
                for c in cats {
                    let (n, ks) = risk_hits.get(c.as_str()).cloned().unwrap_or_default();
                    if n == 0 {
                        out.push_str(&format!("  {c}：0 条\n"));
                    } else {
                        out.push_str(&format!(
                            "  {c}：{n} 条 · {} 把 Key：{}\n",
                            ks.len(),
                            keys.list(&ks)
                        ));
                    }
                }
            }
            if !bad.is_empty() {
                out.push_str(&format!(
                    "  词表第 {} 行不是合法正则，已跳过\n",
                    bad.iter()
                        .map(usize::to_string)
                        .collect::<Vec<_>>()
                        .join("、")
                ));
            }
        }
    }
    Ok(out)
}

/// 命令入口：只读打开数据库，窗口结束于「现在」。
pub fn run(db: &Path, days: i64, terms: Option<PathBuf>) -> Result<String> {
    let conn =
        crate::patrol::open_ro(db).with_context(|| format!("打不开数据库 {}", db.display()))?;
    build(
        &conn,
        &ReportArgs {
            days: days.clamp(1, 90),
            terms,
            until: util::now_secs(),
        },
        &util::local_tz(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn median_even_odd() {
        assert_eq!(median(&mut [3.0, 1.0, 2.0]), Some(2.0));
        assert_eq!(median(&mut [1.0, 2.0, 3.0, 10.0]), Some(2.5));
        assert_eq!(median(&mut []), None);
    }

    #[test]
    fn longest_run_splits_on_gap() {
        let ts = [0.0, 60.0, 120.0, 1000.0, 1050.0];
        assert_eq!(longest_run(&ts, 120.0), (3, 120.0));
        assert_eq!(longest_run(&[], 120.0), (0, 0.0));
        assert_eq!(longest_run(&[5.0], 120.0), (1, 0.0));
    }

    #[test]
    fn device_classes() {
        assert_eq!(
            device_class("Mozilla/5.0 (iPhone; CPU iPhone OS 17)"),
            "ios"
        );
        assert_eq!(device_class("Mozilla/5.0 (Linux; Android 14)"), "android");
        assert_eq!(device_class("Mozilla/5.0 (Windows NT 10.0)"), "windows");
        assert_eq!(device_class("SomeClient 1.2"), "other");
    }

    #[test]
    fn bursts() {
        let regs: Vec<(f64, i64)> = (0..6).map(|i| (i as f64 * 60.0, i)).collect();
        let b = claim_bursts(&regs, 600.0, 5);
        assert_eq!(b.len(), 1);
        assert_eq!(b[0].1, 6);
        let spread: Vec<(f64, i64)> = (0..6).map(|i| (i as f64 * 1000.0, i)).collect();
        assert!(claim_bursts(&spread, 600.0, 5).is_empty());
    }

    #[test]
    fn terms_file() {
        let (t, bad) = parse_terms("## 注释\n\n# 分类甲\nfoo\nba+r\n# 分类乙\n(unclosed\nbaz\n");
        assert_eq!(t.len(), 3);
        assert_eq!(t[0].0, "分类甲");
        assert_eq!(t[2].0, "分类乙");
        assert_eq!(bad, vec![7]);
        assert!(t[0].1.is_match("xFOOx"));
        // 只有注释的示例文件 = 没有规则
        assert!(parse_terms("## a\n## b\n").0.is_empty());
    }
}
