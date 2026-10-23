//! 7×24 巡检（移植自旧版 patrol.py，见标签 py-final），宿主机 systemd 每 5 分钟跑一次。
//!
//! 只查网关自己报不了、或必须在宿主机上看的事：
//! - 容器在不在、`/healthz` 通不通、宿主机可用内存与磁盘；
//! - 上游 429（网关对 429 只写日志、不私信）；
//! - 开着领 Key 却没配桥接密钥（密钥在 `.env` 里，网关发现不了）。
//!
//! 和后台总览重复、只会产出「注意」的统计类检查（成员成功率 / 等待、拒绝结构、记账三方对数、未解决 Bug、
//! 上游 500 比例、日容量、V5 余量、队列……）都删了：站长在后台能看到同样的东西。
//!
//! 退出码：0 正常 · 1 注意 · 2 危险 · **3 巡检程序自己出错**（旧版崩溃时是 1，systemd 分不出
//! 「有警告」和「巡检坏了」）。
//! 数据库用 `mode=ro` 打开，不写任何业务表；告警冷却状态写在 `PATROL_STATE_DIR`。

use crate::checks::{self, ExtraCtx};
use crate::discord;
use crate::util;
use anyhow::{Context, Result};
use gate_core::pyc;
use jiff::tz::TimeZone;
use rusqlite::{Connection, OpenFlags};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sev {
    Ok,
    Warn,
    Red,
}

impl Sev {
    pub fn rank(self) -> i32 {
        match self {
            Sev::Ok => 0,
            Sev::Warn => 1,
            Sev::Red => 2,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Sev::Ok => "ok",
            Sev::Warn => "warn",
            Sev::Red => "red",
        }
    }
    fn upper(self) -> &'static str {
        match self {
            Sev::Ok => "OK",
            Sev::Warn => "WARN",
            Sev::Red => "RED",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Row {
    pub sev: Sev,
    pub area: &'static str,
    pub text: String,
    /// 检查项的稳定标识（告警去重用）：同一项检查，数字变了也是同一个问题
    pub key: String,
}

#[derive(Debug, Default)]
pub struct Report {
    pub rows: Vec<Row>,
    pub data: Map<String, Value>,
}

/// 没给标识时用文本，但把数字抹掉：「成功率 60%」和「成功率 59%」是同一个问题。
/// （10-15 的教训：带数字的文本当标识，每 5 分钟就算一个「新问题」，6 小时冷却形同虚设。）
fn stable_key(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if c.is_ascii_digit() {
            if !out.ends_with('#') {
                out.push('#');
            }
        } else {
            out.push(c);
        }
    }
    out
}

impl Report {
    pub fn add(&mut self, sev: Sev, area: &'static str, text: impl Into<String>) {
        self.add_key(sev, area, text, "");
    }

    pub fn add_key(&mut self, sev: Sev, area: &'static str, text: impl Into<String>, key: &str) {
        let text = text.into();
        let key = if key.is_empty() {
            stable_key(&text)
        } else {
            key.to_string()
        };
        self.rows.push(Row {
            sev,
            area,
            text,
            key,
        });
    }

    /// 危险项的稳定标识（排好序），私信去重用。
    pub fn red_keys(&self) -> Vec<&str> {
        let mut keys: Vec<&str> = self
            .rows
            .iter()
            .filter(|r| r.sev == Sev::Red)
            .map(|r| r.key.as_str())
            .collect();
        keys.sort_unstable();
        keys
    }

    pub fn worst(&self) -> Sev {
        self.rows
            .iter()
            .map(|r| r.sev)
            .max_by_key(|s| s.rank())
            .unwrap_or(Sev::Ok)
    }

    pub fn problems(&self) -> Vec<&Row> {
        self.rows.iter().filter(|r| r.sev != Sev::Ok).collect()
    }

    fn set(&mut self, key: &str, v: impl Into<Value>) {
        self.data.insert(key.to_string(), v.into());
    }
}

// ---------------------------------------------------------------- 配置

#[derive(Debug, Clone)]
pub struct Config {
    pub db: PathBuf,
    pub env_file: PathBuf,
    pub health_url: String,
    pub state_dir: PathBuf,
    pub cooldown: i64,
    pub api_base: String,
    /// 要检查的容器。`NAI_GATE_CONTAINERS` 逗号分隔。
    pub containers: Vec<String>,
}

impl Config {
    pub fn from_env(db: Option<&Path>) -> Result<Self> {
        let get = |k: &str, d: &str| util::env_nonempty(k).unwrap_or_else(|| d.to_string());
        let int = |k: &str, d: i64| -> Result<i64> {
            match util::env_nonempty(k) {
                None => Ok(d),
                Some(v) => {
                    pyc::parse_int(&v).with_context(|| format!("环境变量 {k}={v:?} 不是整数"))
                }
            }
        };
        Ok(Self {
            db: util::resolve_db(db),
            env_file: util::env_file(),
            health_url: get("NAI_GATE_HEALTH", "http://127.0.0.1:3003/healthz"),
            state_dir: get("PATROL_STATE_DIR", "/var/lib/nai-patrol").into(),
            cooldown: int("PATROL_COOLDOWN", 6 * 3600)?,
            api_base: discord::api_base_from_env(),
            containers: get("NAI_GATE_CONTAINERS", "owl-gate,owl-naimei,caddy")
                .split(',')
                .map(|c| c.trim().to_string())
                .filter(|c| !c.is_empty())
                .collect(),
        })
    }
}

// ---------------------------------------------------------------- 取值小工具

// ---------------------------------------------------------------- 系统探测（docker / http / 命令行）

/// `check_infra` 需要的外部事实；探测和判断分开，判断部分可以不碰 docker 直接测。
#[derive(Debug)]
pub struct Infra {
    /// None = 读不到容器状态（docker 不可用）
    pub containers: Option<Vec<(String, bool)>>,
    pub health_code: u16,
    /// (可用内存 MB, 磁盘已用 %)；Err = 旧版会抛的异常类名
    pub resources: std::result::Result<(i64, i64), &'static str>,
}

/// 跑外部命令取 stdout；错误用 Python 对应的异常类名表示（报告里会写出来）。
pub(crate) async fn run_cmd(
    prog: &str,
    args: &[&str],
    secs: u64,
) -> std::result::Result<String, &'static str> {
    let child = tokio::process::Command::new(prog)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .output();
    match tokio::time::timeout(Duration::from_secs(secs), child).await {
        Err(_) => Err("TimeoutExpired"),
        Ok(Err(e)) if e.kind() == std::io::ErrorKind::NotFound => Err("FileNotFoundError"),
        Ok(Err(_)) => Err("OSError"),
        Ok(Ok(out)) => Ok(String::from_utf8_lossy(&out.stdout).into_owned()),
    }
}

async fn http_code(http: &reqwest::Client, url: &str) -> u16 {
    match http.get(url).send().await {
        Ok(r) => r.status().as_u16(),
        Err(_) => 0, // 连不上就是连不上，交给调用方判断
    }
}

async fn docker_running(wanted: &[String]) -> Option<Vec<(String, bool)>> {
    let out = run_cmd("docker", &["ps", "--format", "{{.Names}}"], 10)
        .await
        .ok()?;
    let names: Vec<&str> = out.split_whitespace().collect();
    Some(
        wanted
            .iter()
            .map(|n| (n.clone(), names.contains(&n.as_str())))
            .collect(),
    )
}

pub fn parse_resources(
    free_out: &str,
    df_out: &str,
) -> std::result::Result<(i64, i64), &'static str> {
    let line = free_out
        .lines()
        .find(|l| l.starts_with("Mem:"))
        .ok_or("IndexError")?;
    let avail = line.split_whitespace().nth(6).ok_or("IndexError")?;
    let avail = pyc::parse_int(avail).ok_or("ValueError")?;
    let used = df_out
        .lines()
        .nth(1)
        .ok_or("IndexError")?
        .split_whitespace()
        .nth(4)
        .ok_or("IndexError")?;
    let disk = pyc::parse_int(used.trim_end_matches('%')).ok_or("ValueError")?;
    Ok((avail, disk))
}

async fn read_resources() -> std::result::Result<(i64, i64), &'static str> {
    let mem = run_cmd("free", &["-m"], 5).await?;
    let disk = run_cmd("df", &["/"], 5).await?;
    parse_resources(&mem, &disk)
}

pub async fn gather_infra(cfg: &Config, http: &reqwest::Client) -> Infra {
    Infra {
        containers: docker_running(&cfg.containers).await,
        health_code: http_code(http, &cfg.health_url).await,
        resources: read_resources().await,
    }
}

// ---------------------------------------------------------------- 各项检查

/// 宿主机上才看得到的：容器在不在、网关健康检查、内存与磁盘。
pub fn check_infra(rep: &mut Report, infra: &Infra) {
    match &infra.containers {
        None => rep.add(Sev::Warn, "系统", "读不到容器状态（docker 不可用？）"),
        Some(list) => {
            for (name, up) in list {
                rep.add(
                    if *up { Sev::Ok } else { Sev::Red },
                    "系统",
                    format!("{name} 容器 {}", if *up { "运行中" } else { "没有运行" }),
                );
            }
        }
    }
    let code = infra.health_code;
    let shown = if code == 0 {
        "无响应".to_string()
    } else {
        code.to_string()
    };
    rep.add(
        if code == 200 { Sev::Ok } else { Sev::Red },
        "系统",
        format!("/healthz → {shown}"),
    );
    match infra.resources {
        Ok((avail, disk)) => {
            rep.set("mem_available_mb", avail);
            rep.set("disk_used_pct", disk);
            rep.add(
                if avail < 200 {
                    Sev::Red
                } else if avail < 350 {
                    Sev::Warn
                } else {
                    Sev::Ok
                },
                "系统",
                format!("可用内存 {avail}MB · 磁盘 {disk}%"),
            );
            rep.add(
                if disk > 90 {
                    Sev::Red
                } else if disk > 80 {
                    Sev::Warn
                } else {
                    Sev::Ok
                },
                "系统",
                format!("磁盘已用 {disk}%"),
            );
        }
        Err(name) => rep.add(Sev::Warn, "系统", format!("资源读取失败：{name}")),
    }
}

/// 领 Key 链路：开着领 Key 却没配桥接密钥，成员的 `/register` 会直接失败（网关自己发现不了：密钥在 `.env` 里）。
pub fn check_registration(
    rep: &mut Report,
    conn: &Connection,
    env_file: &Path,
) -> rusqlite::Result<()> {
    let open_now = matches!(
        checks::setting(conn, "register_open")?
            .filter(|s| !s.is_empty())
            .as_deref()
            .unwrap_or("0"),
        "1" | "true" | "yes" | "on"
    );
    rep.set("register_open", open_now);
    let has_secret = !util::env_file_value(env_file, "REGISTRATION_BRIDGE_SECRET").is_empty();
    if open_now && !has_secret {
        rep.add(
            Sev::Red,
            "注册",
            "网关没有 REGISTRATION_BRIDGE_SECRET：/register 会直接失败",
        );
    } else {
        rep.add(
            Sev::Ok,
            "注册",
            if open_now {
                "领 Key 开放 · 桥接密钥已配"
            } else {
                "领 Key 关闭"
            },
        );
    }
    Ok(())
}

/// 上游 429 = 账号级红线（2026-10-13 01:30 第一次遇到）：一定要让站长立刻知道（网关对 429 只写日志、不私信）。
/// 我们的限流器只能挡住自己放行出去的量，挡不住「绕过网关直连上游」的突发。
pub fn check_upstream_429(rep: &mut Report, conn: &Connection, now: f64) -> rusqlite::Result<()> {
    let n429: i64 = conn.query_row(
        "SELECT COUNT(*) FROM usage_log WHERE ts>? AND up_status=429",
        [now - 3600.0],
        |r| r.get(0),
    )?;
    if n429 != 0 {
        rep.add_key(
            Sev::Red,
            "通道",
            format!("近 60 分钟上游 429 × {n429}（账号级限流，必须立刻降速）"),
            "upstream-429-count",
        );
    }
    if n429 == 0 {
        rep.add(Sev::Ok, "通道", "近 60 分钟上游没有返回 429");
    }
    Ok(())
}

/// 全部检查，按这个顺序出报告行（系统 → 通道 → 注册）。
pub fn run_checks(
    rep: &mut Report,
    conn: &Connection,
    infra: &Infra,
    now: f64,
    env_file: &Path,
) -> rusqlite::Result<()> {
    check_infra(rep, infra);
    check_upstream_429(rep, conn, now)?;
    check_registration(rep, conn, env_file)?;
    Ok(())
}

// ---------------------------------------------------------------- 输出

fn ranked(rep: &Report) -> Vec<&Row> {
    let mut v = rep.problems();
    v.sort_by_key(|r| std::cmp::Reverse(r.sev.rank())); // 稳定排序：同级保持原顺序
    v
}

pub fn render_json(rep: &Report) -> String {
    let v = json!({
        "worst": rep.worst().as_str(),
        "data": Value::Object(rep.data.clone()),
        "rows": rep.rows.iter().map(|r| json!({"severity": r.sev.as_str(), "area": r.area, "text": r.text})).collect::<Vec<_>>(),
    });
    serde_json::to_string_pretty(&v).expect("序列化到内存不会失败")
}

pub fn render_oneline(rep: &Report, now: f64, tz: &TimeZone) -> String {
    // 按严重度排序再截取：RED 不能被排在前面的 warn 挤出头条（2026-10-13 凌晨，429 的 RED 就被挤掉了）
    let head = ranked(rep)
        .iter()
        .take(3)
        .map(|r| format!("{}: {}", r.area, r.text))
        .collect::<Vec<_>>()
        .join("；");
    let head = if head.is_empty() {
        "全部正常".to_string()
    } else {
        head
    };
    format!(
        "[patrol {}] {} {head}",
        rep.worst().upper(),
        util::fmt_ts(now, tz, "%m-%d %H:%M")
    )
}

pub fn render_human(rep: &Report, now: f64, tz: &TimeZone) -> String {
    let worst = rep.worst();
    let mut out = format!(
        "=== 巡检 {} → {} ===\n",
        util::fmt_ts(now, tz, "%Y-%m-%d %H:%M:%S"),
        worst.upper()
    );
    // 按区域分组，区域按第一次出现的先后（`run_checks` 的顺序）
    let mut areas: Vec<&str> = Vec::new();
    for r in &rep.rows {
        if !areas.contains(&r.area) {
            areas.push(r.area);
        }
    }
    for area in areas {
        for r in rep.rows.iter().filter(|r| r.area == area) {
            let tag = match r.sev {
                Sev::Ok => "ok  ",
                Sev::Warn => "warn",
                Sev::Red => "RED ",
            };
            out.push_str(&format!("  [{tag}] {}\n", r.text));
        }
    }
    out.push_str(&format!(
        "=== 结论：{}（{} 项需要关注）===\n",
        worst.upper(),
        rep.problems().len()
    ));
    out
}

/// 私信正文：只列危险项（最多 12 条）；「注意」只报个数——它们在巡检日志和后台总览里。
pub fn alert_body(rep: &Report, now: f64, tz: &TimeZone) -> String {
    let red: Vec<&Row> = rep.rows.iter().filter(|r| r.sev == Sev::Red).collect();
    let mut lines: Vec<String> = red
        .iter()
        .take(12)
        .map(|r| format!("❗ {}：{}", r.area, r.text))
        .collect();
    let warn = rep.rows.iter().filter(|r| r.sev == Sev::Warn).count();
    if warn > 0 {
        lines.push(format!(
            "（另有 {warn} 项「注意」，不单独提醒，后台总览可看）"
        ));
    }
    format!(
        "🔎 **巡检** {}\n{}",
        util::fmt_ts(now, tz, "%m-%d %H:%M"),
        lines.join("\n")
    )
}

// ---------------------------------------------------------------- 告警

async fn send_alert(cfg: &Config, text: &str) -> bool {
    let (token, user) = discord::credentials(&cfg.env_file);
    if token.is_empty() || user.is_empty() {
        eprintln!("[patrol] 未配置告警私信（DISCORD_BOT_TOKEN 与 ALERT_USER_ID），只记日志");
        return false;
    }
    // 关掉 @ 提及（告警正文里可能带报错标题，不能让它真的 @everyone）
    match discord::send_dm(&cfg.api_base, &token, &user, text).await {
        Ok(()) => true,
        Err(why) => {
            eprintln!("[patrol] 告警发送失败：{why}");
            false
        }
    }
}

/// 只有**危险**才私信站长（站长 10-15：后台能看，非重大事故不私信）。
///
/// - 同一批危险项（按检查项标识去重，不看数字）冷却期内只发一次，出现新的危险项立刻发；
/// - 危险全部消失后补一条「已恢复」；
/// - 「注意」只打印在巡检日志里。
///
/// 状态文件 `last_alert.json`：`{digest, at, red, summary}`；没发成功不记状态，下一轮重试。
pub async fn maybe_alert(cfg: &Config, rep: &Report, now: f64, tz: &TimeZone) -> Result<()> {
    std::fs::create_dir_all(&cfg.state_dir)
        .with_context(|| format!("创建状态目录 {} 失败", cfg.state_dir.display()))?;
    let mark = cfg.state_dir.join("last_alert.json");
    let last = std::fs::read_to_string(&mark)
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .unwrap_or_else(|| json!({}));
    let was_red = last.get("red").and_then(Value::as_bool).unwrap_or(false);
    let save = |v: Value| -> Result<()> {
        std::fs::write(&mark, v.to_string()).with_context(|| format!("写 {} 失败", mark.display()))
    };

    let keys = rep.red_keys();
    if keys.is_empty() {
        if was_red {
            let since = last.get("since").and_then(Value::as_f64).unwrap_or(now);
            let summary = last.get("summary").and_then(Value::as_str).unwrap_or("");
            let text = format!(
                "✅ **巡检** {}\n已恢复（持续约 {} 分钟）：{summary}",
                util::fmt_ts(now, tz, "%m-%d %H:%M"),
                ((now - since) / 60.0).round().max(1.0) as i64
            );
            if send_alert(cfg, &text).await {
                save(json!({"red": false, "at": now}))?;
                eprintln!("[patrol] 已发送恢复通知");
            }
        }
        return Ok(());
    }

    let digest = hex::encode(Sha256::digest(keys.join("|").as_bytes()))[..16].to_string();
    let last_at = last.get("at").and_then(Value::as_f64).unwrap_or(0.0);
    if was_red
        && last.get("digest").and_then(Value::as_str) == Some(digest.as_str())
        && now - last_at < cfg.cooldown as f64
    {
        return Ok(());
    }
    if send_alert(cfg, &alert_body(rep, now, tz)).await {
        let since = if was_red {
            last.get("since").and_then(Value::as_f64).unwrap_or(now)
        } else {
            now
        };
        let summary: Vec<&str> = rep
            .rows
            .iter()
            .filter(|r| r.sev == Sev::Red)
            .map(|r| r.text.as_str())
            .collect();
        save(json!({
            "red": true,
            "digest": digest,
            "at": now,
            "since": since,
            "summary": pyc::take_chars(&summary.join("；"), 300),
        }))?;
        eprintln!("[patrol] 已发送告警");
    }
    Ok(())
}

// ---------------------------------------------------------------- 主流程

#[derive(Debug, Default, Clone)]
pub struct Args {
    pub alert: bool,
    pub json: bool,
    pub oneline: bool,
}

/// 只读打开数据库；打不开返回 Err（调用方按旧版打印并退出 2）。
pub fn open_ro(path: &Path) -> rusqlite::Result<Connection> {
    let conn = Connection::open_with_flags(
        util::sqlite_uri(path, "mode=ro"),
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_URI
            | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    conn.busy_timeout(Duration::from_secs(10))?;
    Ok(conn)
}

/// 返回退出码：0 / 1 / 2；内部错误由调用方（main）转成 3。
pub async fn run(cfg: &Config, args: &Args) -> Result<i32> {
    let conn = match open_ro(&cfg.db) {
        Ok(c) => c,
        Err(e) => {
            println!("[patrol] 打不开数据库 {}：{e}", cfg.db.display());
            return Ok(2);
        }
    };
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()?;
    let infra = gather_infra(cfg, &http).await;
    let extra = ExtraCtx::gather(cfg).await;
    run_with_extras(
        cfg,
        args,
        &conn,
        &infra,
        Some(&extra),
        util::now_secs(),
        util::local_tz(),
    )
    .await
}

/// 跑全部检查（基础检查 + 可选的扩展检查），返回报告。
pub fn build_report(
    cfg: &Config,
    conn: &Connection,
    infra: &Infra,
    extra: Option<&ExtraCtx>,
    now: f64,
) -> Result<Report> {
    let mut rep = Report::default();
    run_checks(&mut rep, conn, infra, now, &cfg.env_file).context("巡检查询失败")?;
    if let Some(ctx) = extra {
        checks::run_extra_checks(&mut rep, conn, ctx, now).context("扩展巡检查询失败")?;
    }
    Ok(rep)
}

/// 拿到系统事实之后的部分：跑检查、打印、（需要时）告警。测试可以直接喂合成的 `Infra`；
/// `extra` 传 `None` 就只跑基础检查。
pub async fn run_with_extras(
    cfg: &Config,
    args: &Args,
    conn: &Connection,
    infra: &Infra,
    extra: Option<&ExtraCtx>,
    now: f64,
    tz: TimeZone,
) -> Result<i32> {
    let rep = build_report(cfg, conn, infra, extra, now)?;

    let worst = rep.worst();
    if args.json {
        println!("{}", render_json(&rep));
    } else if args.oneline {
        println!("{}", render_oneline(&rep, now, &tz));
    } else {
        print!("{}", render_human(&rep, now, &tz));
    }
    if args.alert {
        maybe_alert(cfg, &rep, now, &tz).await?;
    }
    Ok(worst.rank())
}
