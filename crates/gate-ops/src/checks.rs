//! 巡检的扩展检查：排队健康、账号均衡、Anlas 估算与到期、原图目录、TLS 证书、身份组未授予、上游 401/403、
//! V5 方案输入核对、重启提示。
//!
//! 全部只读（`mode=ro` 的库连接 + 读文件 + 只读的 `docker inspect`）。每项检查拆成「取数」和「判断」：
//! 判断部分是纯函数，阈值都是下面的具名常量，单元测试直接喂数字。
//! 不确定的事实（没有 `token_id` 列、找不到证书目录、没有对账快照）一律写成 `n/a` 并跳过，不报警。
//!
//! 严重度约定（巡检 `--alert` 用）：**危险（RED）只留给真正的事故**——
//! 证书快过期（续签失败）、原图目录远超上限（清理失效）、上游连续 401/403（账号失效）、排队长期堵死；其余最多「注意」。

use crate::patrol::{Config, Report, Sev, run_cmd};
use crate::util;
use base64::Engine as _;
use rusqlite::{Connection, OptionalExtension};
use serde_json::Value;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------- 阈值（具名常量）

/// 排队健康的统计窗口（分钟）。
pub const QUEUE_WINDOW_MIN: i64 = 35;
/// 窗口内图片请求少于这个数就不判断（样本太少，比例没有意义）。
pub const QUEUE_MIN_SAMPLES: usize = 5;
/// 排队等待 p90（毫秒）：注意 / 危险。
pub const QUEUE_P90_WARN_MS: i64 = 45_000;
pub const QUEUE_P90_RED_MS: i64 = 100_000;
/// 拒绝比例（拒绝 / 全部图片请求）：只给「注意」。额度用完的拒绝也算，所以不会升到危险。
pub const QUEUE_REJECT_RATIO_WARN: f64 = 0.25;
/// 窗口内 queue_full + queue_timeout 次数：注意 / 危险。
pub const QUEUE_FULL_WARN: i64 = 5;
pub const QUEUE_FULL_RED: i64 = 40;

/// 账号均衡看最近多少小时；至少这么多张、至少两个账号才判断。
pub const SKEW_WINDOW_H: i64 = 24;
pub const SKEW_MIN_IMAGES: i64 = 40;
/// 最忙账号的份额 / 平均份额 ≥ 这个值 → 注意。
pub const SKEW_WARN_RATIO: f64 = 1.8;

/// 对账快照超过几天没更新 → 注意；到期不足几天 → 注意；日均消耗按最近几天算。
pub const ANLAS_STALE_DAYS: f64 = 3.0;
pub const ANLAS_EXPIRY_WARN_DAYS: f64 = 3.0;
pub const ANLAS_BURN_DAYS: f64 = 3.0;

/// 原图目录占上限的比例：注意 / 危险（清理任务没起作用）。
pub const ORIGINALS_WARN_RATIO: f64 = 0.9;
pub const ORIGINALS_RED_RATIO: f64 = 1.5;
/// 遍历原图目录最多看这么多个文件（防止异常目录拖慢巡检）。
pub const ORIGINALS_MAX_FILES: usize = 500_000;

/// TLS 证书剩余天数：注意 / 危险。Caddy 在到期前 30 天开始续签，所以不足 15 天说明续签一直失败。
pub const TLS_WARN_DAYS: f64 = 15.0;
pub const TLS_RED_DAYS: f64 = 5.0;

/// 领了 Key 但身份组没授予的人数 ≥ 这个值 → 注意（只报人数）。
pub const ROLE_PENDING_WARN: i64 = 5;

/// 近 60 分钟上游 401 + 403 的次数：注意 / 危险。
pub const UPSTREAM_AUTH_WARN: i64 = 1;
pub const UPSTREAM_AUTH_RED: i64 = 5;

/// 网关在这么多分钟内启动过，就提示一条（仅供参考）。
pub const RESTART_NOTE_MIN: f64 = 60.0;

// ---------------------------------------------------------------- 事实

/// 需要问宿主机 / 文件系统才知道的事实；判断和取数分开，测试可以直接构造。
#[derive(Debug, Clone, Default)]
pub struct ExtraCtx {
    /// 审计原图目录（`<库所在目录>/audit_images`）
    pub audit_dir: PathBuf,
    /// Caddy 数据目录（含 `certificates/`）；None = 没找到
    pub caddy_data: Option<PathBuf>,
    /// 站点域名（`PUBLIC_HOST` 或 `SITE_URL` 的主机部分）
    pub site_host: Option<String>,
    /// 网关容器的启动时间（Unix 秒）
    pub gate_started_at: Option<f64>,
    /// 网关容器名（报告里用）
    pub gate_container: String,
}

/// `https://owl.example:8443/x` → `owl.example`。
pub fn host_of(url_or_host: &str) -> Option<String> {
    let s = url_or_host.trim();
    let s = s.split_once("://").map_or(s, |(_, r)| r);
    let s = s.split(['/', '?', '#']).next().unwrap_or("");
    let s = s.rsplit_once('@').map_or(s, |(_, h)| h);
    let s = match s.rsplit_once(':') {
        Some((h, p)) if !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()) => h,
        _ => s,
    };
    let s = s.trim().to_ascii_lowercase();
    (!s.is_empty()).then_some(s)
}

impl ExtraCtx {
    /// 从环境、`.env` 和 `docker inspect` 收集（任何一步失败都只是留空）。
    pub async fn gather(cfg: &Config) -> Self {
        let site_host = util::env_nonempty("PUBLIC_HOST")
            .or_else(|| util::env_nonempty("SITE_URL"))
            .or_else(|| {
                ["PUBLIC_HOST", "SITE_URL"]
                    .iter()
                    .map(|k| util::env_file_value(&cfg.env_file, k))
                    .find(|v| !v.is_empty())
            })
            .and_then(|v| host_of(&v));
        let gate_container = cfg
            .containers
            .first()
            .cloned()
            .unwrap_or_else(|| "owl-gate".into());
        let caddy_data = match util::env_nonempty("CADDY_DATA_DIR") {
            Some(p) => Some(PathBuf::from(p)),
            None => caddy_data_from_docker().await,
        };
        let gate_started_at = container_started_at(&gate_container).await;
        Self {
            audit_dir: cfg
                .db
                .parent()
                .map_or_else(|| PathBuf::from("audit_images"), |p| p.join("audit_images")),
            caddy_data,
            site_host,
            gate_started_at,
            gate_container,
        }
    }
}

async fn caddy_data_from_docker() -> Option<PathBuf> {
    let out = run_cmd(
        "docker",
        &[
            "inspect",
            "-f",
            "{{range .Mounts}}{{.Destination}}={{.Source}}{{println}}{{end}}",
            "caddy",
        ],
        10,
    )
    .await
    .ok()?;
    out.lines()
        .find_map(|l| l.strip_prefix("/data="))
        .map(|s| PathBuf::from(s.trim()))
        .filter(|p| !p.as_os_str().is_empty())
}

async fn container_started_at(name: &str) -> Option<f64> {
    let out = run_cmd(
        "docker",
        &["inspect", "-f", "{{.State.StartedAt}}", name],
        10,
    )
    .await
    .ok()?;
    out.trim()
        .parse::<jiff::Timestamp>()
        .ok()
        .map(|t| t.as_second() as f64)
}

// ---------------------------------------------------------------- 小工具

pub(crate) fn setting(conn: &Connection, key: &str) -> rusqlite::Result<Option<String>> {
    conn.query_row("SELECT value FROM site_settings WHERE key=?", [key], |r| {
        r.get::<_, String>(0)
    })
    .optional()
}

fn secs(ms: i64) -> String {
    format!("{:.1}s", ms as f64 / 1000.0)
}

fn worse(a: Sev, b: Sev) -> Sev {
    if b.rank() > a.rank() { b } else { a }
}

// ---------------------------------------------------------------- 1. 排队健康

#[derive(Debug, Default, Clone, PartialEq)]
pub struct QueueStats {
    /// 窗口内图片请求总数
    pub total: usize,
    /// 成功请求的等待时间（毫秒），升序
    pub waits_ms: Vec<i64>,
    pub rejected: usize,
    pub queue_full: i64,
    pub queue_timeout: i64,
}

pub fn judge_queue(s: &QueueStats) -> Vec<(Sev, String, &'static str)> {
    if s.total < QUEUE_MIN_SAMPLES {
        return vec![(
            Sev::Ok,
            format!(
                "排队：近 {QUEUE_WINDOW_MIN} 分钟只有 {} 张图片请求，样本太少不判断",
                s.total
            ),
            "queue-health",
        )];
    }
    let avg = if s.waits_ms.is_empty() {
        0
    } else {
        s.waits_ms.iter().sum::<i64>() / s.waits_ms.len() as i64
    };
    let p90 = util::nearest_rank(&s.waits_ms, 0.9).unwrap_or(0);
    let ratio = s.rejected as f64 / s.total as f64;
    let q = s.queue_full + s.queue_timeout;
    let mut sev = Sev::Ok;
    let mut notes: Vec<String> = Vec::new();
    if p90 >= QUEUE_P90_RED_MS {
        sev = Sev::Red;
        notes.push(format!("等待 p90 ≥ {}", secs(QUEUE_P90_RED_MS)));
    } else if p90 >= QUEUE_P90_WARN_MS {
        sev = worse(sev, Sev::Warn);
        notes.push(format!("等待 p90 ≥ {}", secs(QUEUE_P90_WARN_MS)));
    }
    if q >= QUEUE_FULL_RED {
        sev = Sev::Red;
        notes.push(format!("排队满 / 超时 ≥ {QUEUE_FULL_RED} 次"));
    } else if q >= QUEUE_FULL_WARN {
        sev = worse(sev, Sev::Warn);
        notes.push(format!("排队满 / 超时 ≥ {QUEUE_FULL_WARN} 次"));
    }
    if ratio >= QUEUE_REJECT_RATIO_WARN {
        sev = worse(sev, Sev::Warn);
        notes.push(format!(
            "拒绝比例 ≥ {:.0}%",
            QUEUE_REJECT_RATIO_WARN * 100.0
        ));
    }
    let tail = if notes.is_empty() {
        String::new()
    } else {
        format!("（{}）", notes.join("；"))
    };
    vec![(
        sev,
        format!(
            "排队：近 {QUEUE_WINDOW_MIN} 分钟 {} 张 · 等待均值 {} / p90 {} · 拒绝 {}（{:.0}%）· queue_full {} · queue_timeout {}{tail}",
            s.total,
            secs(avg),
            secs(p90),
            s.rejected,
            ratio * 100.0,
            s.queue_full,
            s.queue_timeout
        ),
        "queue-health",
    )]
}

pub fn queue_stats(conn: &Connection, now: f64) -> rusqlite::Result<QueueStats> {
    let since = now - (QUEUE_WINDOW_MIN * 60) as f64;
    let mut stmt = conn.prepare(
        "SELECT status, reason, wait_ms FROM usage_log WHERE kind LIKE 'image%' AND ts>?",
    )?;
    let rows = stmt.query_map([since], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, i64>(2)?,
        ))
    })?;
    let mut s = QueueStats::default();
    for row in rows {
        let (status, reason, wait) = row?;
        s.total += 1;
        match status.as_str() {
            "ok" => s.waits_ms.push(wait.max(0)),
            "rejected" => s.rejected += 1,
            _ => {}
        }
        match reason.as_str() {
            "queue_full" => s.queue_full += 1,
            "queue_timeout" => s.queue_timeout += 1,
            _ => {}
        }
    }
    s.waits_ms.sort_unstable();
    Ok(s)
}

// ---------------------------------------------------------------- 2. 账号均衡（需要 usage_log.token_id）

/// `(账号, 张数)` → 判断。账号标识只显示前 6 位。
pub fn judge_skew(counts: &[(String, i64)]) -> (Sev, String) {
    let total: i64 = counts.iter().map(|c| c.1).sum();
    let n = counts.len();
    let hours = SKEW_WINDOW_H;
    if n < 2 || total < SKEW_MIN_IMAGES {
        return (
            Sev::Ok,
            format!("账号均衡：近 {hours} 小时 {total} 张 · {n} 个账号，样本不足不判断"),
        );
    }
    let (top_id, top) = counts
        .iter()
        .max_by_key(|c| c.1)
        .map(|c| (c.0.as_str(), c.1))
        .expect("n >= 2");
    let share = top as f64 / total as f64;
    let fair = 1.0 / n as f64;
    let ratio = share / fair;
    let id6: String = top_id.chars().take(6).collect();
    let text = format!(
        "账号均衡：近 {hours} 小时 {total} 张分布在 {n} 个账号，最多的 {id6} 占 {:.0}%（均分 {:.0}%）",
        share * 100.0,
        fair * 100.0
    );
    if ratio >= SKEW_WARN_RATIO {
        (Sev::Warn, format!("{text}，明显偏斜"))
    } else {
        (Sev::Ok, text)
    }
}

pub fn check_skew(rep: &mut Report, conn: &Connection, now: f64) -> rusqlite::Result<()> {
    let mut stmt = conn.prepare(
        "SELECT token_id, COUNT(*) FROM usage_log WHERE kind LIKE 'image%' AND status='ok' \
         AND ts>? AND token_id IS NOT NULL AND token_id<>'' GROUP BY token_id ORDER BY token_id",
    )?;
    let counts: Vec<(String, i64)> = stmt
        .query_map([now - (SKEW_WINDOW_H * 3600) as f64], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
        })?
        .collect::<rusqlite::Result<_>>()?;
    let (sev, text) = judge_skew(&counts);
    rep.add_key(sev, "账号", text, "account-skew");
    Ok(())
}

// ---------------------------------------------------------------- 3. Anlas 估算与到期

#[derive(Debug, Clone, PartialEq)]
pub struct AnlasInputs {
    /// 最近一次对账时官方余额（固定 + 购买）
    pub balance: f64,
    /// 对账之后我们记账的 Anlas 消耗
    pub spent_since: f64,
    /// 对账快照的年龄（天）
    pub age_days: f64,
    /// 最近几天的日均消耗
    pub burn_per_day: f64,
    /// 距最早到期还有几天；None = 快照里没有到期时间
    pub expires_in_days: Option<f64>,
}

pub fn judge_anlas(i: &AnlasInputs) -> Vec<(Sev, String, &'static str)> {
    let est = i.balance - i.spent_since;
    let expiry = match i.expires_in_days {
        Some(d) if d >= 0.0 => format!(" · 距到期约 {d:.1} 天"),
        Some(d) => format!(" · 已过期 {:.1} 天", -d),
        None => String::new(),
    };
    let mut rows = vec![(
        Sev::Ok,
        format!(
            "Anlas 估算（近似）：{:.1} 天前对账余额 {:.0}，之后记账消耗 {:.0}，约剩 {:.0}{expiry}",
            i.age_days, i.balance, i.spent_since, est
        ),
        "anlas-estimate",
    )];
    if i.age_days > ANLAS_STALE_DAYS {
        rows.push((
            Sev::Warn,
            format!(
                "Anlas 对账已 {:.1} 天没更新（超过 {ANLAS_STALE_DAYS:.0} 天），上面的估算不可靠，请到后台点一次「核对」",
                i.age_days
            ),
            "anlas-stale",
        ));
    }
    if let Some(d) = i.expires_in_days {
        if d < ANLAS_EXPIRY_WARN_DAYS {
            rows.push((
                Sev::Warn,
                if d >= 0.0 {
                    format!(
                        "Anlas 订阅约 {d:.1} 天后到期（少于 {ANLAS_EXPIRY_WARN_DAYS:.0} 天），请确认续费"
                    )
                } else {
                    format!("Anlas 订阅已过期 {:.1} 天，请确认续费", -d)
                },
                "anlas-expiry",
            ));
        } else if i.burn_per_day > 0.0 && est > 0.0 && est / i.burn_per_day < d {
            rows.push((
                Sev::Warn,
                format!(
                    "按近 {ANLAS_BURN_DAYS:.0} 天日均 {:.0}，估算约 {:.1} 天后耗尽，早于到期（{d:.1} 天后）",
                    i.burn_per_day,
                    est / i.burn_per_day
                ),
                "anlas-runout",
            ));
        }
    }
    if est <= 0.0 && i.balance > 0.0 {
        rows.push((
            Sev::Warn,
            "Anlas 估算余额已为 0 或负数（可能对账后有充值，或记账有偏差），请先核对".to_string(),
            "anlas-empty",
        ));
    }
    rows
}

/// 快照里的到期时间可能是秒也可能是毫秒。
fn expiry_secs(v: f64) -> f64 {
    if v > 1e12 { v / 1000.0 } else { v }
}

pub fn check_anlas(rep: &mut Report, conn: &Connection, now: f64) -> rusqlite::Result<()> {
    let snap: Option<String> = conn
        .query_row(
            "SELECT snapshot FROM anlas_reconciliations ORDER BY id DESC LIMIT 1",
            [],
            |r| r.get(0),
        )
        .optional()?;
    let Some(snap) = snap else {
        rep.add(Sev::Ok, "Anlas", "Anlas 估算：n/a（还没有对账快照）");
        return Ok(());
    };
    let Ok(v) = serde_json::from_str::<Value>(&snap) else {
        rep.add(
            Sev::Ok,
            "Anlas",
            "Anlas 估算：n/a（最近一次对账快照读不懂）",
        );
        return Ok(());
    };
    let (Some(checked), Some(balance)) = (
        v.get("checked_at").and_then(Value::as_f64),
        v.get("balance").and_then(Value::as_f64),
    ) else {
        rep.add(Sev::Ok, "Anlas", "Anlas 估算：n/a（对账快照缺字段）");
        return Ok(());
    };
    let last_id = v
        .pointer("/ledger/last_log_id")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let spent: f64 = conn.query_row(
        "SELECT COALESCE(SUM(anlas),0) FROM usage_log WHERE id>?",
        [last_id],
        |r| r.get(0),
    )?;
    let burn_total: f64 = conn.query_row(
        "SELECT COALESCE(SUM(anlas),0) FROM usage_log WHERE ts>?",
        [now - ANLAS_BURN_DAYS * 86400.0],
        |r| r.get(0),
    )?;
    let expires = v
        .get("accounts")
        .and_then(Value::as_object)
        .map(|m| {
            m.values()
                .filter_map(|a| a.get("expires_at").and_then(Value::as_f64))
                .filter(|e| *e > 0.0)
                .map(expiry_secs)
                .fold(f64::INFINITY, f64::min)
        })
        .filter(|e| e.is_finite());
    let inputs = AnlasInputs {
        balance,
        spent_since: spent,
        age_days: (now - checked).max(0.0) / 86400.0,
        burn_per_day: burn_total / ANLAS_BURN_DAYS,
        expires_in_days: expires.map(|e| (e - now) / 86400.0),
    };
    for (sev, text, key) in judge_anlas(&inputs) {
        rep.add_key(sev, "Anlas", text, key);
    }
    Ok(())
}

// ---------------------------------------------------------------- 4. 原图目录

pub fn judge_originals(bytes: u64, cap_mb: i64, grown_last_hour: i64) -> (Sev, String) {
    let grown = format!(
        "近 1 小时新增约 {}",
        util::human_size(grown_last_hour.max(0) as u64)
    );
    if cap_mb <= 0 {
        return (
            Sev::Ok,
            format!("原图目录：{}（上限不限）· {grown}", util::human_size(bytes)),
        );
    }
    let cap_bytes = cap_mb as u64 * 1024 * 1024;
    let ratio = bytes as f64 / cap_bytes as f64;
    let base = format!(
        "原图目录：{} / 上限 {}（{:.0}%）· {grown}",
        util::human_size(bytes),
        util::human_size(cap_bytes),
        ratio * 100.0
    );
    if ratio >= ORIGINALS_RED_RATIO {
        (Sev::Red, format!("{base}，远超上限，自动清理没有起作用"))
    } else if ratio >= ORIGINALS_WARN_RATIO {
        (Sev::Warn, format!("{base}，接近上限"))
    } else {
        (Sev::Ok, base)
    }
}

/// 递归累加文件大小；遇到读不了的条目跳过。返回 (总字节, 是否因为文件太多提前停止)。
pub fn dir_size(root: &Path) -> (u64, bool) {
    let mut total = 0u64;
    let mut files = 0usize;
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in rd.flatten() {
            let Ok(md) = e.metadata() else { continue };
            if md.is_dir() {
                stack.push(e.path());
            } else if md.is_file() {
                total += md.len();
                files += 1;
                if files >= ORIGINALS_MAX_FILES {
                    return (total, true);
                }
            }
        }
    }
    (total, false)
}

pub fn check_originals(
    rep: &mut Report,
    conn: &Connection,
    ctx: &ExtraCtx,
    now: f64,
) -> rusqlite::Result<()> {
    if !ctx.audit_dir.is_dir() {
        rep.add(Sev::Ok, "原图", "原图目录：n/a（找不到 audit_images 目录）");
        return Ok(());
    }
    let cap_mb = setting(conn, "audit_image_cap_mb")?
        .and_then(|s| s.trim().parse::<i64>().ok())
        .unwrap_or(8192);
    let grown: i64 = conn.query_row(
        "SELECT COALESCE(SUM(image_size),0) FROM generation_audit WHERE ts>? AND image_file IS NOT NULL",
        [now - 3600.0],
        |r| r.get(0),
    )?;
    let (bytes, truncated) = dir_size(&ctx.audit_dir);
    let (sev, mut text) = judge_originals(bytes, cap_mb, grown);
    if truncated {
        text.push_str(&format!(
            "（文件超过 {ORIGINALS_MAX_FILES} 个，只统计了一部分）"
        ));
    }
    rep.add_key(sev, "原图", text, "originals-size");
    Ok(())
}

// ---------------------------------------------------------------- 5. TLS 证书（读 Caddy 数据目录里的证书文件）

/// 读一个 DER 元素：(标签, 内容, 后面的字节)。
fn der_next(buf: &[u8]) -> Option<(u8, &[u8], &[u8])> {
    let tag = *buf.first()?;
    let b1 = *buf.get(1)?;
    let (len, hdr) = if b1 < 0x80 {
        (b1 as usize, 2)
    } else {
        let n = (b1 & 0x7f) as usize;
        if n == 0 || n > 4 {
            return None;
        }
        let mut l = 0usize;
        for i in 0..n {
            l = (l << 8) | *buf.get(2 + i)? as usize;
        }
        (l, 2 + n)
    };
    let end = hdr.checked_add(len)?;
    Some((tag, buf.get(hdr..end)?, buf.get(end..)?))
}

fn asn1_time(tag: u8, s: &[u8]) -> Option<i64> {
    let t = std::str::from_utf8(s).ok()?.strip_suffix('Z')?;
    let (year, rest): (i16, &str) = match tag {
        0x17 if t.len() >= 10 => {
            let yy: i16 = t.get(0..2)?.parse().ok()?;
            (if yy < 50 { 2000 + yy } else { 1900 + yy }, t.get(2..)?)
        }
        0x18 if t.len() >= 12 => (t.get(0..4)?.parse().ok()?, t.get(4..)?),
        _ => return None,
    };
    let f = |i: usize| -> Option<i8> { rest.get(i..i + 2)?.parse().ok() };
    let (mo, d, h, mi) = (f(0)?, f(2)?, f(4)?, f(6)?);
    let sec = f(8).unwrap_or(0);
    let dt = jiff::civil::DateTime::new(year, mo, d, h, mi, sec, 0).ok()?;
    Some(
        dt.to_zoned(jiff::tz::TimeZone::UTC)
            .ok()?
            .timestamp()
            .as_second(),
    )
}

/// 从 DER 证书里取 notAfter（Unix 秒）。
pub fn cert_not_after_der(der: &[u8]) -> Option<i64> {
    let (t, cert, _) = der_next(der)?;
    if t != 0x30 {
        return None;
    }
    let (t, tbs, _) = der_next(cert)?;
    if t != 0x30 {
        return None;
    }
    let (tag, _, mut rest) = der_next(tbs)?;
    if tag == 0xA0 {
        // 有版本号：下一个才是序列号
        rest = der_next(rest)?.2;
    }
    rest = der_next(rest)?.2; // 签名算法
    rest = der_next(rest)?.2; // 颁发者
    let (t, validity, _) = der_next(rest)?;
    if t != 0x30 {
        return None;
    }
    let after = der_next(validity)?.2; // 跳过 notBefore
    let (tag, na, _) = der_next(after)?;
    asn1_time(tag, na)
}

/// PEM 文本里第一张证书的 notAfter。
pub fn cert_not_after_pem(pem: &str) -> Option<i64> {
    const BEGIN: &str = "-----BEGIN CERTIFICATE-----";
    const END: &str = "-----END CERTIFICATE-----";
    let start = pem.find(BEGIN)? + BEGIN.len();
    let end = start + pem[start..].find(END)?;
    let b64: String = pem[start..end]
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    let der = base64::engine::general_purpose::STANDARD.decode(b64).ok()?;
    cert_not_after_der(&der)
}

/// Caddy 数据目录下的证书文件（`certificates/<issuer>/<host>/<host>.crt`）。
pub fn find_certs(data: &Path) -> Vec<PathBuf> {
    let mut roots = vec![
        data.join("certificates"),
        data.join("caddy").join("certificates"),
    ];
    roots.retain(|r| r.is_dir());
    let mut out = Vec::new();
    for root in roots {
        let mut stack = vec![(root, 0usize)];
        while let Some((dir, depth)) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&dir) else {
                continue;
            };
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() && depth < 4 {
                    stack.push((p, depth + 1));
                } else if p.extension().is_some_and(|x| x == "crt") {
                    out.push(p);
                }
            }
        }
    }
    out.sort();
    out
}

pub fn judge_tls(host: &str, days_left: f64) -> (Sev, String) {
    let base = format!("证书 {host}：剩 {days_left:.1} 天");
    if days_left < TLS_RED_DAYS {
        (
            Sev::Red,
            format!("{base}（少于 {TLS_RED_DAYS:.0} 天，Caddy 续签一直失败，请看 caddy 日志）"),
        )
    } else if days_left < TLS_WARN_DAYS {
        (
            Sev::Warn,
            format!("{base}（少于 {TLS_WARN_DAYS:.0} 天，续签可能失败）"),
        )
    } else {
        (Sev::Ok, base)
    }
}

pub fn check_tls(rep: &mut Report, ctx: &ExtraCtx, now: f64) {
    let Some(dir) = &ctx.caddy_data else {
        rep.add(
            Sev::Ok,
            "证书",
            "证书：n/a（没找到 Caddy 数据目录；可设环境变量 CADDY_DATA_DIR）",
        );
        return;
    };
    let mut certs = find_certs(dir);
    if let Some(h) = &ctx.site_host {
        let want = format!("{h}.crt");
        let hit: Vec<PathBuf> = certs
            .iter()
            .filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy() == want))
            .cloned()
            .collect();
        if !hit.is_empty() {
            certs = hit;
        }
    }
    let best = certs
        .iter()
        .filter_map(|p| {
            let na = cert_not_after_pem(&std::fs::read_to_string(p).ok()?)?;
            let host = p
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            Some((host, na))
        })
        .min_by_key(|c| c.1);
    match best {
        None => rep.add(
            Sev::Ok,
            "证书",
            "证书：n/a（Caddy 数据目录里没读到可用的证书）",
        ),
        Some((host, na)) => {
            let (sev, text) = judge_tls(&host, (na as f64 - now) / 86400.0);
            rep.add_key(sev, "证书", text, "tls-expiry");
        }
    }
}

// ---------------------------------------------------------------- 6. 领了 Key 但身份组没授予

pub fn check_role_pending(rep: &mut Report, conn: &Connection) -> rusqlite::Result<()> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM discord_registrations r JOIN api_keys k ON k.id=r.key_id \
         WHERE k.enabled=1 AND r.role_granted=0",
        [],
        |r| r.get(0),
    )?;
    let sev = if n >= ROLE_PENDING_WARN {
        Sev::Warn
    } else {
        Sev::Ok
    };
    rep.add_key(
        sev,
        "成员",
        format!("领了 Key 但身份组还没授予：{n} 人（只报人数）"),
        "role-pending",
    );
    Ok(())
}

// ---------------------------------------------------------------- 7. 上游 401 / 403（429 由 patrol::check_upstream_429 单独报）

pub fn judge_upstream_auth(n401: i64, n403: i64) -> (Sev, String) {
    let text = format!("近 60 分钟上游 401 × {n401} · 403 × {n403}");
    let bad = n401 + n403;
    if bad >= UPSTREAM_AUTH_RED {
        (Sev::Red, format!("{text}：上游账号可能失效，请检查令牌池"))
    } else if bad >= UPSTREAM_AUTH_WARN {
        (Sev::Warn, text)
    } else {
        (Sev::Ok, text)
    }
}

pub fn check_upstream_auth(rep: &mut Report, conn: &Connection, now: f64) -> rusqlite::Result<()> {
    let count = |code: i64| -> rusqlite::Result<i64> {
        conn.query_row(
            "SELECT COUNT(*) FROM usage_log WHERE ts>? AND up_status=?",
            rusqlite::params![now - 3600.0, code],
            |r| r.get(0),
        )
    };
    let (sev, text) = judge_upstream_auth(count(401)?, count(403)?);
    rep.add_key(sev, "通道", text, "upstream-auth");
    Ok(())
}

// ---------------------------------------------------------------- 8. V5 方案输入核对

pub fn check_restart(rep: &mut Report, ctx: &ExtraCtx, now: f64) {
    let Some(started) = ctx.gate_started_at else {
        return;
    };
    let mins = (now - started) / 60.0;
    if (0.0..RESTART_NOTE_MIN).contains(&mins) {
        rep.add_key(
            Sev::Ok,
            "系统",
            format!(
                "提示：{} 在 {:.0} 分钟前启动过。每小时出图计数重启后由用量日志补回，跨重启的那个小时窗口可能多算一次（仅供参考，不影响放行）",
                ctx.gate_container,
                mins.max(1.0)
            ),
            "gate-restarted",
        );
    }
}

// ---------------------------------------------------------------- 汇总

/// 按「队列 → 账号 → Anlas → 原图 → 证书 → 成员 → 通道 → 额度 → 系统提示」的顺序追加到报告末尾。
pub fn run_extra_checks(
    rep: &mut Report,
    conn: &Connection,
    ctx: &ExtraCtx,
    now: f64,
) -> rusqlite::Result<()> {
    let stats = queue_stats(conn, now)?;
    for (sev, text, key) in judge_queue(&stats) {
        rep.add_key(sev, "队列", text, key);
    }
    check_skew(rep, conn, now)?;
    check_anlas(rep, conn, now)?;
    check_originals(rep, conn, ctx, now)?;
    check_tls(rep, ctx, now);
    check_role_pending(rep, conn)?;
    check_upstream_auth(rep, conn, now)?;
    check_restart(rep, ctx, now);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_parsing() {
        assert_eq!(
            host_of("https://Owl.Example:8443/x?y=1").as_deref(),
            Some("owl.example")
        );
        assert_eq!(host_of("owl.example").as_deref(), Some("owl.example"));
        assert_eq!(host_of(""), None);
    }

    fn mk(waits: Vec<i64>, rejected: usize, qf: i64, qt: i64, total: usize) -> QueueStats {
        QueueStats {
            total,
            waits_ms: waits,
            rejected,
            queue_full: qf,
            queue_timeout: qt,
        }
    }

    #[test]
    fn queue_thresholds() {
        // 样本太少
        assert_eq!(judge_queue(&mk(vec![], 0, 0, 0, 3))[0].0, Sev::Ok);
        // 正常
        let ok = judge_queue(&mk(vec![1000; 10], 0, 0, 0, 10));
        assert_eq!(ok[0].0, Sev::Ok);
        assert!(ok[0].1.contains("p90 1.0s"), "{}", ok[0].1);
        // p90 到注意 / 危险
        assert_eq!(
            judge_queue(&mk(vec![QUEUE_P90_WARN_MS; 10], 0, 0, 0, 10))[0].0,
            Sev::Warn
        );
        assert_eq!(
            judge_queue(&mk(vec![QUEUE_P90_RED_MS; 10], 0, 0, 0, 10))[0].0,
            Sev::Red
        );
        // 排队满 / 超时次数
        assert_eq!(judge_queue(&mk(vec![0; 10], 3, 3, 2, 20))[0].0, Sev::Warn);
        assert_eq!(judge_queue(&mk(vec![0; 10], 40, 20, 20, 60))[0].0, Sev::Red);
        // 拒绝比例只到注意
        assert_eq!(judge_queue(&mk(vec![0; 10], 10, 0, 0, 20))[0].0, Sev::Warn);
    }

    #[test]
    fn skew_thresholds() {
        let c = |v: &[(&str, i64)]| {
            v.iter()
                .map(|(a, n)| ((*a).to_string(), *n))
                .collect::<Vec<_>>()
        };
        assert_eq!(judge_skew(&c(&[("aaaaaaaa", 10)])).0, Sev::Ok);
        assert_eq!(
            judge_skew(&c(&[("aaaaaaaa", 20), ("bbbbbbbb", 10)])).0,
            Sev::Ok
        ); // 样本少
        assert_eq!(
            judge_skew(&c(&[("aaaaaaaa", 60), ("bbbbbbbb", 60)])).0,
            Sev::Ok
        );
        let (sev, text) = judge_skew(&c(&[("aaaaaaaa", 110), ("bbbbbbbb", 10)]));
        assert_eq!(sev, Sev::Warn);
        assert!(text.contains("aaaaaa 占 92%"), "{text}");
    }

    fn anlas(burn: f64, spent: f64, age: f64, exp: Option<f64>) -> AnlasInputs {
        AnlasInputs {
            balance: 1000.0,
            spent_since: spent,
            age_days: age,
            burn_per_day: burn,
            expires_in_days: exp,
        }
    }

    #[test]
    fn anlas_rules() {
        // 全部正常
        let r = judge_anlas(&anlas(10.0, 100.0, 1.0, Some(20.0)));
        assert_eq!(r.len(), 1);
        assert!(
            r[0].1.contains("约剩 900") && r[0].1.contains("距到期约 20.0 天"),
            "{}",
            r[0].1
        );
        let has = |a: AnlasInputs, key: &str| judge_anlas(&a).iter().any(|r| r.2 == key);
        assert!(has(anlas(10.0, 0.0, 3.5, None), "anlas-stale"));
        assert!(has(anlas(10.0, 0.0, 1.0, Some(2.0)), "anlas-expiry"));
        assert!(
            judge_anlas(&anlas(0.0, 0.0, 1.0, Some(-1.0)))
                .iter()
                .any(|r| r.1.contains("已过期"))
        );
        // 到期前耗尽：900 / 100 = 9 天 < 20 天
        assert!(has(anlas(100.0, 100.0, 1.0, Some(20.0)), "anlas-runout"));
        assert!(!has(anlas(10.0, 100.0, 1.0, Some(20.0)), "anlas-runout"));
        assert!(has(anlas(0.0, 1500.0, 1.0, None), "anlas-empty"));
    }

    #[test]
    fn originals_thresholds() {
        let mb = 1024 * 1024;
        assert_eq!(judge_originals(100 * mb, 1000, 0).0, Sev::Ok);
        assert_eq!(judge_originals(950 * mb, 1000, 0).0, Sev::Warn);
        assert_eq!(judge_originals(1600 * mb, 1000, 0).0, Sev::Red);
        assert_eq!(judge_originals(1600 * mb, 0, 0).0, Sev::Ok); // 不限
    }

    #[test]
    fn tls_thresholds() {
        assert_eq!(judge_tls("a", 60.0).0, Sev::Ok);
        assert_eq!(judge_tls("a", 10.0).0, Sev::Warn);
        assert_eq!(judge_tls("a", 2.0).0, Sev::Red);
    }

    #[test]
    fn upstream_auth_thresholds() {
        assert_eq!(judge_upstream_auth(0, 0).0, Sev::Ok);
        assert_eq!(judge_upstream_auth(1, 0).0, Sev::Warn);
        assert_eq!(judge_upstream_auth(2, 3).0, Sev::Red);
    }

    #[test]
    fn asn1_time_forms() {
        // 2026-10-16 00:00:00 UTC
        let want = 1_792_108_800;
        assert_eq!(asn1_time(0x17, b"261016000000Z"), Some(want));
        assert_eq!(asn1_time(0x18, b"20261016000000Z"), Some(want));
        assert_eq!(asn1_time(0x17, b"garbage"), None);
    }
}
