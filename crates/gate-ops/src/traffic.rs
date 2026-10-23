//! `nai-ops traffic`：汇总 Caddy 的 JSON 访问日志（按路径分组的请求数、状态码分类、耗时 p50 / p95、每分钟峰值）。
//!
//! 只读日志文件，且**只解析需要的字段**（时间、状态码、耗时、请求路径）：IP、请求头、Cookie 一概不读不留；
//! 请求路径在 `?` 处截断，查询参数不会出现在任何输出里。支持滚动后的 `.log.gz`。
//! Caddy 端怎么配出「不含 IP / 请求头 / 查询参数」的日志，见 `deploy/Caddyfile.access-log.example`。

use crate::util;
use anyhow::{Context, Result, bail};
use flate2::read::GzDecoder;
use jiff::tz::TimeZone;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

pub const DEFAULT_LOG: &str = "/var/log/caddy/access.log";

#[derive(Deserialize)]
struct Line {
    ts: Option<f64>,
    status: Option<u16>,
    duration: Option<f64>,
    request: Option<Req>,
}

#[derive(Deserialize)]
struct Req {
    uri: Option<String>,
}

/// 请求路径 → 分组名（先去掉查询参数）。
pub fn path_group(uri: &str) -> &'static str {
    let path = uri.split(['?', '#']).next().unwrap_or("");
    if path.contains("generate-image") {
        "image-gen"
    } else if path.starts_with("/ai/")
        || path.starts_with("/nai/")
        || path.starts_with("/user/")
        || path.starts_with("/v1/")
    {
        "gate-api"
    } else if path.starts_with("/admin/api") {
        "admin-api"
    } else if path == "/admin" || path.starts_with("/admin/") {
        "admin-ui"
    } else if path.starts_with("/public/")
        || path.starts_with("/status")
        || path == "/queue-status"
        || path == "/healthz"
    {
        "public"
    } else if path.starts_with("/self-register") {
        "bridge"
    } else if path.starts_with("/_owl/") || path.starts_with("/favicon") {
        "static"
    } else if path.starts_with("/login")
        || path.starts_with("/logout")
        || path.starts_with("/member")
        || path.starts_with("/survey")
    {
        "member"
    } else {
        "other"
    }
}

/// `30m` / `1h` / `24h` / `7d` / `90s` → 秒。
pub fn parse_span(s: &str) -> Option<f64> {
    let s = s.trim();
    let (num, unit) = s.split_at(s.find(|c: char| !c.is_ascii_digit())?);
    let n: f64 = num.parse().ok()?;
    let mult = match unit {
        "s" => 1.0,
        "m" => 60.0,
        "h" => 3600.0,
        "d" => 86400.0,
        _ => return None,
    };
    (n > 0.0).then_some(n * mult)
}

#[derive(Debug, Default, Clone)]
pub struct Group {
    pub n: usize,
    pub class: [usize; 5], // 2xx 3xx 4xx 5xx 其它
    pub dur_ms: Vec<i64>,
    pub per_min: BTreeMap<i64, usize>,
}

#[derive(Debug, Default)]
pub struct Summary {
    pub groups: BTreeMap<&'static str, Group>,
    pub total: usize,
    pub skipped: usize,
    pub per_min: BTreeMap<i64, usize>,
}

impl Summary {
    /// 喂一行日志；窗口外的行忽略。
    pub fn add_line(&mut self, line: &str, since: f64, until: f64) {
        if line.trim().is_empty() {
            return;
        }
        let Ok(l) = serde_json::from_str::<Line>(line) else {
            self.skipped += 1;
            return;
        };
        let (Some(ts), Some(uri)) = (l.ts, l.request.and_then(|r| r.uri)) else {
            self.skipped += 1;
            return;
        };
        if ts < since || ts >= until {
            return;
        }
        let g = self.groups.entry(path_group(&uri)).or_default();
        g.n += 1;
        self.total += 1;
        let ci = match l.status.unwrap_or(0) / 100 {
            2 => 0,
            3 => 1,
            4 => 2,
            5 => 3,
            _ => 4,
        };
        g.class[ci] += 1;
        if let Some(d) = l.duration {
            g.dur_ms.push((d * 1000.0).round() as i64);
        }
        let minute = (ts / 60.0).floor() as i64;
        *g.per_min.entry(minute).or_default() += 1;
        *self.per_min.entry(minute).or_default() += 1;
    }
}

fn peak(m: &BTreeMap<i64, usize>) -> Option<(i64, usize)> {
    m.iter()
        .map(|(k, v)| (*k, *v))
        .max_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(&a.0)))
}

pub fn render(s: &mut Summary, since: f64, until: f64, tz: &TimeZone) -> String {
    let span = until - since;
    let span_txt = if span >= 86400.0 {
        format!("{:.0} 天", span / 86400.0)
    } else if span >= 3600.0 {
        format!("{:.0} 小时", span / 3600.0)
    } else {
        format!("{:.0} 分钟", span / 60.0)
    };
    let pk = peak(&s.per_min).map_or("n/a".to_string(), |(m, n)| {
        format!(
            "{n} 次/分钟（{}）",
            util::fmt_ts((m * 60) as f64, tz, "%m-%d %H:%M")
        )
    });
    let mut out = format!(
        "=== 流量：近 {span_txt}（{} ~ {}）· 共 {} 个请求 · 峰值 {pk} ===\n",
        util::fmt_ts(since, tz, "%m-%d %H:%M"),
        util::fmt_ts(until, tz, "%m-%d %H:%M"),
        s.total
    );
    out.push_str(&format!(
        "{:<10} {:>7} {:>6} {:>5} {:>6} {:>5} {:>6} {:>8} {:>8} {:>9}\n",
        "分组", "请求", "2xx", "3xx", "4xx", "5xx", "其它", "p50ms", "p95ms", "峰值/分钟"
    ));
    for (name, g) in &mut s.groups {
        g.dur_ms.sort_unstable();
        out.push_str(&format!(
            "{:<10} {:>7} {:>6} {:>5} {:>6} {:>5} {:>6} {:>8} {:>8} {:>9}\n",
            name,
            g.n,
            g.class[0],
            g.class[1],
            g.class[2],
            g.class[3],
            g.class[4],
            util::nearest_rank(&g.dur_ms, 0.5).unwrap_or(0),
            util::nearest_rank(&g.dur_ms, 0.95).unwrap_or(0),
            peak(&g.per_min).map_or(0, |p| p.1)
        ));
    }
    if s.skipped > 0 {
        out.push_str(&format!(
            "（跳过 {} 行无法解析或缺字段的日志）\n",
            s.skipped
        ));
    }
    out
}

/// 展开路径：目录 → 里面的 `*.log` / `*.log.gz`（按文件名排序）。
pub fn expand(paths: &[PathBuf]) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    for p in paths {
        if p.is_dir() {
            let mut v: Vec<PathBuf> = std::fs::read_dir(p)
                .with_context(|| format!("读不了目录 {}", p.display()))?
                .flatten()
                .map(|e| e.path())
                .filter(|f| {
                    f.file_name()
                        .map(|n| n.to_string_lossy())
                        .is_some_and(|n| n.ends_with(".log") || n.ends_with(".log.gz"))
                })
                .collect();
            v.sort();
            out.extend(v);
        } else {
            out.push(p.clone());
        }
    }
    Ok(out)
}

fn feed(path: &Path, s: &mut Summary, since: f64, until: f64) -> Result<()> {
    let f = std::fs::File::open(path).with_context(|| format!("打不开 {}", path.display()))?;
    let rd: Box<dyn BufRead> = if path.extension().is_some_and(|e| e == "gz") {
        Box::new(BufReader::new(GzDecoder::new(f)))
    } else {
        Box::new(BufReader::new(f))
    };
    for line in rd.split(b'\n') {
        let Ok(bytes) = line else { break };
        s.add_line(&String::from_utf8_lossy(&bytes), since, until);
    }
    Ok(())
}

pub fn run(logs: &[PathBuf], span_s: f64, until: f64) -> Result<String> {
    run_tz(logs, span_s, until, &util::local_tz())
}

pub fn run_tz(logs: &[PathBuf], span_s: f64, until: f64, tz: &TimeZone) -> Result<String> {
    let files = expand(logs)?;
    if files.is_empty() {
        bail!("没有找到访问日志文件");
    }
    let since = until - span_s;
    let mut s = Summary::default();
    for f in &files {
        feed(f, &mut s, since, until)?;
    }
    Ok(render(&mut s, since, until, tz))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_strip_query() {
        assert_eq!(path_group("/ai/generate-image?token=x"), "image-gen");
        assert_eq!(path_group("/v1/me"), "gate-api");
        assert_eq!(path_group("/admin/api/members?q=1"), "admin-api");
        assert_eq!(path_group("/admin"), "admin-ui");
        assert_eq!(path_group("/public/live"), "public");
        assert_eq!(path_group("/self-register/x"), "bridge");
        assert_eq!(path_group("/_owl/app.js"), "static");
        assert_eq!(path_group("/"), "other");
    }

    #[test]
    fn spans() {
        assert_eq!(parse_span("90s"), Some(90.0));
        assert_eq!(parse_span("2h"), Some(7200.0));
        assert_eq!(parse_span("7d"), Some(604_800.0));
        assert_eq!(parse_span("0h"), None);
        assert_eq!(parse_span("abc"), None);
        assert_eq!(parse_span("5"), None);
    }
}
