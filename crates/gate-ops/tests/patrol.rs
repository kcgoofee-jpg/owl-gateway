//! 巡检各项检查的单元测试：全部在合成数据库上跑（gate-db 建库 + 手工灌数据），不碰 docker / 网络。
//! 期望的文案都是照旧版 patrol.py（见标签 py-final）手工算出来的。

mod common;

use common::*;
use gate_ops::patrol::*;
use jiff::tz::TimeZone;
use rusqlite::{Connection, OpenFlags};
use serde_json::{Value, json};
use std::path::Path;

/// 2026-09-21 22:13:20 +08:00
const NOW: f64 = 1_790_000_000.0;

fn shanghai() -> TimeZone {
    TimeZone::get("Asia/Shanghai").unwrap()
}

fn cfg(dir: &Path) -> Config {
    // 读凭据时进程环境变量优先于 .env：开发机上带着真实凭据会让告警用例失真
    assert!(
        std::env::var("DISCORD_BOT_TOKEN").is_err(),
        "测试环境不该有 DISCORD_BOT_TOKEN"
    );
    assert!(std::env::var("ALERT_USER_ID").is_err());
    Config {
        db: dir.join("nai_gate.db"),
        env_file: dir.join(".env"),
        health_url: "http://127.0.0.1:1/healthz".into(),
        state_dir: dir.join("state"),
        cooldown: 1800,
        api_base: "http://127.0.0.1:1".into(),
        containers: vec!["nai-gate".into(), "nai-gate-discord".into(), "caddy".into()],
    }
}

fn ro(path: &Path) -> Connection {
    Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap()
}

fn rows(rep: &Report) -> Vec<(Sev, &'static str, String)> {
    rep.rows
        .iter()
        .map(|r| (r.sev, r.area, r.text.clone()))
        .collect()
}

fn text_of(rep: &Report, needle: &str) -> Row {
    rep.rows
        .iter()
        .find(|r| r.text.contains(needle))
        .unwrap_or_else(|| panic!("报告里没有包含 {needle:?} 的行：{:#?}", rows(rep)))
        .clone()
}

fn real_now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs_f64()
}

fn healthy_infra() -> Infra {
    Infra {
        containers: Some(vec![
            ("nai-gate".to_string(), true),
            ("nai-gate-discord".to_string(), true),
            ("caddy".to_string(), true),
        ]),
        health_code: 200,
        resources: Ok((900, 24)),
    }
}

// ------------------------------------------------------------ Report / 系统探测

/// 非 ok 行的稳定标识（按出现顺序）。
fn pkeys(rep: &Report) -> Vec<String> {
    rep.rows
        .iter()
        .filter(|r| r.sev != Sev::Ok)
        .map(|r| r.key.clone())
        .collect()
}

#[test]
fn report_keys_use_stable_key_or_text() {
    let mut r = Report::default();
    r.add(Sev::Ok, "系统", "fine");
    r.add(Sev::Warn, "系统", "text with 12 numbers");
    r.add_key(Sev::Red, "数据", "bug 3 times", "bug-count");
    // 没给标识时用文本，数字抹成 #（数字变了还是同一个问题）
    assert_eq!(pkeys(&r), vec!["text with # numbers", "bug-count"]);
    assert_eq!(r.red_keys(), vec!["bug-count"]);
    assert_eq!(r.worst(), Sev::Red);
    assert_eq!(r.problems().len(), 2);
}

#[test]
fn infra_rows_and_thresholds() {
    let infra = Infra {
        containers: Some(vec![
            ("nai-gate".to_string(), true),
            ("nai-gate-discord".to_string(), false),
            ("caddy".to_string(), true),
        ]),
        health_code: 0,
        resources: Ok((150, 85)),
    };
    let mut rep = Report::default();
    check_infra(&mut rep, &infra);
    assert_eq!(
        rows(&rep),
        vec![
            (Sev::Ok, "系统", "nai-gate 容器 运行中".to_string()),
            (
                Sev::Red,
                "系统",
                "nai-gate-discord 容器 没有运行".to_string()
            ),
            (Sev::Ok, "系统", "caddy 容器 运行中".to_string()),
            (Sev::Red, "系统", "/healthz → 无响应".to_string()),
            (Sev::Red, "系统", "可用内存 150MB · 磁盘 85%".to_string()),
            (Sev::Warn, "系统", "磁盘已用 85%".to_string()),
        ]
    );
    assert_eq!(rep.data["mem_available_mb"], json!(150));
    assert_eq!(rep.data["disk_used_pct"], json!(85));
}

#[test]
fn infra_failures_are_reported_not_fatal() {
    let infra = Infra {
        containers: None,
        health_code: 503,
        resources: Err("FileNotFoundError"),
    };
    let mut rep = Report::default();
    check_infra(&mut rep, &infra);
    let texts: Vec<_> = rep.rows.iter().map(|r| r.text.as_str()).collect();
    assert_eq!(
        texts,
        [
            "读不到容器状态（docker 不可用？）",
            "/healthz → 503",
            "资源读取失败：FileNotFoundError"
        ]
    );
    assert_eq!(rep.rows[0].sev, Sev::Warn);
    assert_eq!(rep.rows[1].sev, Sev::Red);
    assert!(!rep.data.contains_key("mem_available_mb"));
}

#[test]
fn memory_and_disk_thresholds() {
    for (avail, disk, mem_sev, disk_sev) in [
        (349, 80, Sev::Warn, Sev::Ok),
        (350, 81, Sev::Ok, Sev::Warn),
        (199, 91, Sev::Red, Sev::Red),
        (200, 90, Sev::Warn, Sev::Warn),
    ] {
        let mut infra = healthy_infra();
        infra.resources = Ok((avail, disk));
        let mut rep = Report::default();
        check_infra(&mut rep, &infra);
        let mem = text_of(&rep, "可用内存");
        let dsk = text_of(&rep, "磁盘已用");
        assert_eq!(
            (mem.sev, dsk.sev),
            (mem_sev, disk_sev),
            "avail={avail} disk={disk}"
        );
    }
}

#[test]
fn parse_free_and_df_output() {
    let free = "              total        used        free      shared  buff/cache   available\n\
                Mem:           1963         812         151          12        1000         900\n\
                Swap:             0           0           0\n";
    let df = "Filesystem     1K-blocks    Used Available Use% Mounted on\n/dev/vda1       41151808 9000000  30000000  24% /\n";
    assert_eq!(parse_resources(free, df), Ok((900, 24)));
    assert_eq!(parse_resources("", df), Err("IndexError"));
    assert_eq!(
        parse_resources(free, "Filesystem only header\n"),
        Err("IndexError")
    );
    assert_eq!(
        parse_resources(free, "h\n/dev/x 1 2 3 abc% /\n"),
        Err("ValueError")
    );
}

// ------------------------------------------------------------ 上游 429 与领 Key 链路

#[test]
fn upstream_429_is_red_and_quiet_hours_are_ok() {
    let dir = tempfile::tempdir().unwrap();
    let path = new_db(dir.path());
    let c = rw(&path);
    add_key(&c, 1, "nai-synthetic-key-1", true);
    // 没有 429：一行「正常」
    let mut rep = Report::default();
    check_upstream_429(&mut rep, &ro(&path), NOW).unwrap();
    assert_eq!(
        rows(&rep),
        vec![(Sev::Ok, "通道", "近 60 分钟上游没有返回 429".to_string())]
    );
    // 设置里的最近时间 + 日志里的次数
    set_setting(&c, "guard_last_upstream_429", &format!("{}", NOW - 600.0));
    for i in 0..2 {
        add_log(
            &c,
            &Log1 {
                status: "error",
                images: 0,
                up_status: 429,
                ..Log1::image_ok(NOW - 1200.0 - f64::from(i), 1)
            },
        );
    }
    let mut rep = Report::default();
    check_upstream_429(&mut rep, &ro(&path), NOW).unwrap();
    assert_eq!(
        rows(&rep),
        vec![
            (Sev::Red, "通道", "上游 10 分钟前返回 429（限流）：上限已自动减半并进入冷却，请确认没有额外突发打到上游".to_string()),
            (Sev::Red, "通道", "近 60 分钟上游 429 × 2（账号级限流，必须立刻降速）".to_string()),
        ]
    );
    assert_eq!(pkeys(&rep), ["upstream-429", "upstream-429-count"]);
}

#[test]
fn old_429_is_ignored() {
    let dir = tempfile::tempdir().unwrap();
    let path = new_db(dir.path());
    let c = rw(&path);
    set_setting(&c, "guard_last_upstream_429", &format!("{}", NOW - 3600.0)); // 刚好满 1 小时：不再报
    add_log(
        &c,
        &Log1 {
            status: "error",
            images: 0,
            up_status: 429,
            ..Log1::image_ok(NOW - 3601.0, 1)
        },
    );
    let mut rep = Report::default();
    check_upstream_429(&mut rep, &ro(&path), NOW).unwrap();
    assert_eq!(rep.worst(), Sev::Ok, "{:?}", rows(&rep));
}

#[test]
fn non_finite_last_429_setting_is_ignored() {
    let dir = tempfile::tempdir().unwrap();
    let path = new_db(dir.path());
    let c = rw(&path);
    for bad in ["inf", "-inf", "nan", "abc", ""] {
        set_setting(&c, "guard_last_upstream_429", bad);
        let mut rep = Report::default();
        check_upstream_429(&mut rep, &ro(&path), NOW).unwrap();
        assert_eq!(rep.worst(), Sev::Ok, "{bad:?}: {:?}", rows(&rep));
    }
    // 浮点文本按截断取整（网关写的是秒级时间戳，可能带小数或科学计数法）
    set_setting(&c, "guard_last_upstream_429", &format!("{:e}", NOW - 120.5));
    let mut rep = Report::default();
    check_upstream_429(&mut rep, &ro(&path), NOW).unwrap();
    assert_eq!(rep.worst(), Sev::Red, "{:?}", rows(&rep));
}

#[test]
fn registration_open_without_bridge_secret_is_red() {
    let dir = tempfile::tempdir().unwrap();
    let path = new_db(dir.path());
    let no_env = dir.path().join("missing.env");
    // 关着：缺密钥也不报（成员这时领不了 Key 本来就是预期）
    let mut rep = Report::default();
    check_registration(&mut rep, &ro(&path), &no_env).unwrap();
    assert_eq!(
        rows(&rep),
        vec![(Sev::Ok, "注册", "领 Key 关闭".to_string())]
    );
    assert_eq!(rep.data["register_open"], json!(false));
    // 开着却没有桥接密钥 = 危险
    let c = rw(&path);
    set_setting(&c, "register_open", "on");
    let mut rep = Report::default();
    check_registration(&mut rep, &ro(&path), &no_env).unwrap();
    assert_eq!(
        rows(&rep),
        vec![(
            Sev::Red,
            "注册",
            "网关没有 REGISTRATION_BRIDGE_SECRET：/register 会直接失败".to_string()
        )]
    );
    // 配了密钥：正常
    let env = dir.path().join(".env");
    std::fs::write(&env, "REGISTRATION_BRIDGE_SECRET=synthetic\n").unwrap();
    let mut rep = Report::default();
    check_registration(&mut rep, &ro(&path), &env).unwrap();
    assert_eq!(
        rows(&rep),
        vec![(Sev::Ok, "注册", "领 Key 开放 · 桥接密钥已配".to_string())]
    );
    assert_eq!(rep.data["register_open"], json!(true));
}

// ------------------------------------------------------------ 整体流程与退出码

/// 死表回归：3.0 的迁移会 DROP waitlist / share_evidence，旧版巡检在这时直接崩溃。
#[test]
fn full_run_on_database_without_dropped_tables() {
    let dir = tempfile::tempdir().unwrap();
    let path = new_db(dir.path());
    let c = rw(&path);
    for t in ["waitlist", "share_evidence"] {
        let n: i64 = c
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE name=?",
                [t],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 0, "{t} 应该已被迁移删除");
    }
    let config = cfg(dir.path());
    let mut rep = Report::default();
    run_checks(
        &mut rep,
        &ro(&path),
        &healthy_infra(),
        NOW,
        &config.env_file,
    )
    .unwrap();
    assert!(
        rep.rows
            .iter()
            .all(|r| !r.text.contains("候补") && !r.text.contains("防分享"))
    );
    assert!(!rep.data.contains_key("waitlist"));
    // 检查的执行顺序决定报告里的顺序；只剩宿主机 / 上游 429 / 领 Key 链路三类
    let mut areas: Vec<&str> = rep.rows.iter().map(|r| r.area).collect();
    areas.dedup();
    assert_eq!(areas, ["系统", "通道", "注册"]);
    assert_eq!(rep.worst(), Sev::Ok, "{:?}", rows(&rep));
}

fn healthy_db(dir: &Path) -> std::path::PathBuf {
    let path = new_db(dir);
    let c = rw(&path);
    add_key(&c, 1, "nai-synthetic-key-1", true);
    for i in 0..20 {
        add_log(
            &c,
            &Log1::image_ok(real_now() - 120.0 - f64::from(i) * 20.0, 1),
        );
    }
    std::fs::write(dir.join(".env"), "REGISTRATION_BRIDGE_SECRET=synthetic\n").unwrap();
    path
}

#[tokio::test]
async fn exit_codes_follow_worst_severity() {
    let dir = tempfile::tempdir().unwrap();
    let path = healthy_db(dir.path());
    let config = cfg(dir.path());
    let args = Args::default();

    let code = run_with_extras(
        &config,
        &args,
        &ro(&path),
        &healthy_infra(),
        None,
        real_now(),
        shanghai(),
    )
    .await
    .unwrap();
    assert_eq!(code, 0, "全部正常");

    let mut warn = healthy_infra();
    warn.resources = Ok((300, 24)); // 可用内存 <350MB → 注意
    assert_eq!(
        run_with_extras(
            &config,
            &args,
            &ro(&path),
            &warn,
            None,
            real_now(),
            shanghai()
        )
        .await
        .unwrap(),
        1
    );

    let mut red = healthy_infra();
    red.containers = Some(vec![
        ("nai-gate".to_string(), false),
        ("nai-gate-discord".to_string(), true),
        ("caddy".to_string(), true),
    ]);
    assert_eq!(
        run_with_extras(
            &config,
            &args,
            &ro(&path),
            &red,
            None,
            real_now(),
            shanghai()
        )
        .await
        .unwrap(),
        2
    );

    for flags in [
        Args {
            json: true,
            ..Args::default()
        },
        Args {
            oneline: true,
            ..Args::default()
        },
    ] {
        assert_eq!(
            run_with_extras(
                &config,
                &flags,
                &ro(&path),
                &healthy_infra(),
                None,
                real_now(),
                shanghai()
            )
            .await
            .unwrap(),
            0
        );
    }
}

#[tokio::test]
async fn unreadable_database_is_exit_2_and_internal_error_is_err() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = cfg(dir.path());
    config.db = dir.path().join("does-not-exist.db");
    assert_eq!(
        run(&config, &Args::default()).await.unwrap(),
        2,
        "打不开数据库 = 危险（与旧版一致）"
    );

    // 库能打开但缺表：这是巡检自己遇到的内部错误 → Err（main 转成退出码 3，而不是 1）
    let empty = dir.path().join("empty.db");
    Connection::open(&empty)
        .unwrap()
        .execute_batch("CREATE TABLE unrelated(x)")
        .unwrap();
    let r = run_with_extras(
        &config,
        &Args::default(),
        &ro(&empty),
        &healthy_infra(),
        None,
        NOW,
        shanghai(),
    )
    .await;
    let err = r.expect_err("缺表必须是内部错误");
    assert!(format!("{err:#}").contains("no such table"), "{err:#}");
}

// ------------------------------------------------------------ 输出格式

fn sample_report() -> Report {
    let mut r = Report::default();
    r.add(Sev::Ok, "系统", "容器正常");
    r.add(Sev::Warn, "系统", "A 注意");
    r.add(Sev::Warn, "成员", "B 注意");
    r.add(Sev::Red, "通道", "C 危险");
    r.add(Sev::Warn, "数据", "D 注意");
    r.data.insert("x".into(), json!(1.0));
    r.data.insert("flag".into(), json!(true));
    r.data
        .insert("nested".into(), json!({"a": [], "b": {}, "c": [["k", 1]]}));
    r
}

#[test]
fn oneline_puts_red_first_and_keeps_three() {
    let line = render_oneline(&sample_report(), NOW, &shanghai());
    assert_eq!(
        line,
        "[patrol RED] 09-21 22:13 通道: C 危险；系统: A 注意；成员: B 注意"
    );
    assert_eq!(
        render_oneline(&Report::default(), NOW, &shanghai()),
        "[patrol OK] 09-21 22:13 全部正常"
    );
}

#[test]
fn human_report_groups_by_area() {
    let text = render_human(&sample_report(), NOW, &shanghai());
    assert_eq!(
        text,
        "=== 巡检 2026-09-21 22:13:20 → RED ===\n\
         \x20 [ok  ] 容器正常\n\
         \x20 [warn] A 注意\n\
         \x20 [warn] B 注意\n\
         \x20 [RED ] C 危险\n\
         \x20 [warn] D 注意\n\
         === 结论：RED（4 项需要关注）===\n"
    );
}

#[test]
fn json_output_is_two_space_indented() {
    let json_text = render_json(&sample_report());
    let expected = r#"{
  "worst": "red",
  "data": {
    "x": 1.0,
    "flag": true,
    "nested": {
      "a": [],
      "b": {},
      "c": [
        [
          "k",
          1
        ]
      ]
    }
  },
  "rows": [
    {
      "severity": "ok",
      "area": "系统",
      "text": "容器正常"
    },
    {
      "severity": "warn",
      "area": "系统",
      "text": "A 注意"
    },
    {
      "severity": "warn",
      "area": "成员",
      "text": "B 注意"
    },
    {
      "severity": "red",
      "area": "通道",
      "text": "C 危险"
    },
    {
      "severity": "warn",
      "area": "数据",
      "text": "D 注意"
    }
  ]
}"#;
    assert_eq!(json_text, expected);
    let _: Value = serde_json::from_str(&json_text).unwrap();
}

#[test]
fn alert_body_lists_only_red_and_counts_warnings() {
    let mut r = Report::default();
    for i in 0..3 {
        r.add(Sev::Warn, "系统", format!("w{i}"));
    }
    for i in 0..14 {
        r.add(Sev::Red, "通道", format!("boom{i}"));
    }
    let body = alert_body(&r, NOW, &shanghai());
    let lines: Vec<_> = body.lines().collect();
    assert_eq!(lines[0], "🔎 **巡检** 09-21 22:13");
    assert_eq!(lines[1], "❗ 通道：boom0");
    assert_eq!(lines.len(), 1 + 12 + 1, "最多 12 条危险 + 一行「注意」计数");
    assert_eq!(lines[13], "（另有 3 项「注意」，不单独提醒，后台总览可看）");
}

// ------------------------------------------------------------ 告警（假 Discord）

fn alert_env(dir: &Path, extra: &str) -> Config {
    let config = cfg(dir);
    std::fs::write(
        &config.env_file,
        format!("DISCORD_BOT_TOKEN=synthetic-bot-token\n{extra}"),
    )
    .unwrap();
    config
}

fn report_with(text: &str, key: &str) -> Report {
    let mut r = Report::default();
    r.add_key(Sev::Red, "数据", text, key);
    r
}

fn warn_only() -> Report {
    let mut r = Report::default();
    r.add(
        Sev::Warn,
        "注册",
        "每人 104 张 × 161 人 = 16744 张 vs 日容量 1700 张",
    );
    r
}

fn content(post: &Req) -> String {
    post.body["content"].as_str().unwrap().to_string()
}

/// 只看发出的消息（每次发告警前都会先建一次私信频道）。
fn messages(log: &Log) -> Vec<Req> {
    log.lock()
        .unwrap()
        .iter()
        .filter(|r| r.path.ends_with("/messages"))
        .cloned()
        .collect()
}

/// 站长 ID（假 Discord 给它的私信频道是 `dm-<ID>`）。
const OWNER: &str = "ALERT_USER_ID=900000000000000011\n";
const OWNER_DM: &str = "/channels/dm-900000000000000011/messages";

#[tokio::test]
async fn warnings_alone_never_message_the_owner() {
    let dir = tempfile::tempdir().unwrap();
    let (mock, log) = discord_mock(DiscordScript::default()).await;
    let mut config = alert_env(dir.path(), OWNER);
    config.api_base = mock.base.clone();
    for i in 0..5 {
        maybe_alert(
            &config,
            &warn_only(),
            NOW + f64::from(i) * 300.0,
            &shanghai(),
        )
        .await
        .unwrap();
    }
    assert!(
        log.lock().unwrap().is_empty(),
        "「注意」只进日志和后台，不私信"
    );
    assert!(!config.state_dir.join("last_alert.json").exists());
}

#[tokio::test]
async fn red_is_deduplicated_by_check_key_and_recovery_is_announced_once() {
    let dir = tempfile::tempdir().unwrap();
    let (mock, log) = discord_mock(DiscordScript::default()).await;
    let mut config = alert_env(dir.path(), OWNER);
    config.api_base = mock.base.clone();
    let tz = shanghai();

    maybe_alert(&config, &report_with("/healthz → 502", "healthz"), NOW, &tz)
        .await
        .unwrap();
    {
        let posts = messages(&log);
        assert_eq!(posts.len(), 1);
        assert_eq!(posts[0].path, OWNER_DM);
        assert_eq!(posts[0].auth, "Bot synthetic-bot-token");
        assert!(
            posts[0].user_agent.starts_with("DiscordBot ("),
            "UA 必须是 Cloudflare 放行的那组：{}",
            posts[0].user_agent
        );
        assert_eq!(
            posts[0].body,
            json!({"content": "🔎 **巡检** 09-21 22:13\n❗ 数据：/healthz → 502", "allowed_mentions": {"parse": []}})
        );
    }
    let state: Value = serde_json::from_str(
        &std::fs::read_to_string(config.state_dir.join("last_alert.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        (state["red"].clone(), state["at"].clone()),
        (json!(true), json!(NOW))
    );
    assert_eq!(state["digest"].as_str().unwrap().len(), 16);

    // 冷却内、同一检查项（数字变了没关系），再加几条「注意」→ 不再发
    let mut same = report_with("/healthz → 503", "healthz");
    same.add(Sev::Warn, "注册", "成功率 59%");
    maybe_alert(&config, &same, NOW + 10.0, &tz).await.unwrap();
    assert_eq!(messages(&log).len(), 1);
    // 冷却内、出现了新的危险项 → 立刻发
    let mut more = report_with("/healthz → 503", "healthz");
    more.add_key(Sev::Red, "系统", "磁盘已用 95%", "disk");
    maybe_alert(&config, &more, NOW + 20.0, &tz).await.unwrap();
    assert_eq!(messages(&log).len(), 2);
    // 同一批问题，冷却过后 → 再提醒一次；差 1 秒 → 不发
    let cd = config.cooldown as f64;
    maybe_alert(&config, &more, NOW + 20.0 + cd + 1.0, &tz)
        .await
        .unwrap();
    assert_eq!(messages(&log).len(), 3);
    maybe_alert(&config, &more, NOW + 20.0 + 2.0 * cd, &tz)
        .await
        .unwrap();
    assert_eq!(messages(&log).len(), 3);
    // 危险消失（只剩「注意」）→ 一条「已恢复」，之后不再响
    maybe_alert(&config, &warn_only(), NOW + 600.0 + 2.0 * cd, &tz)
        .await
        .unwrap();
    maybe_alert(&config, &warn_only(), NOW + 900.0 + 2.0 * cd, &tz)
        .await
        .unwrap();
    let posts = messages(&log);
    assert_eq!(posts.len(), 4);
    let recovered = content(&posts[3]);
    assert!(recovered.starts_with("✅ **巡检**"), "{recovered}");
    assert!(
        recovered.contains("/healthz → 503；磁盘已用 95%"),
        "{recovered}"
    );
}

#[tokio::test]
async fn alert_goes_to_owner_dm() {
    let dir = tempfile::tempdir().unwrap();
    let (mock, log) = discord_mock(DiscordScript::default()).await;
    // 频道 ID 不再读：配了也只私信站长
    let mut config = alert_env(
        dir.path(),
        "ALERT_CHANNEL_ID=900000000000000010\nALERT_USER_ID=900000000000000011\n",
    );
    config.api_base = mock.base.clone();
    maybe_alert(&config, &report_with("x", "k"), NOW, &shanghai())
        .await
        .unwrap();
    let posts = log.lock().unwrap();
    assert_eq!(posts.len(), 2);
    assert_eq!(
        (posts[0].path.as_str(), &posts[0].body),
        (
            "/users/@me/channels",
            &json!({"recipient_id": "900000000000000011"})
        )
    );
    assert_eq!(posts[1].path, OWNER_DM);
}

#[tokio::test]
async fn failed_alert_leaves_no_state_so_it_retries() {
    let dir = tempfile::tempdir().unwrap();
    for script in [
        DiscordScript {
            message_status: 500,
            ..Default::default()
        },
        DiscordScript {
            cloudflare_1010: true,
            ..Default::default()
        },
    ] {
        let (mock, log) = discord_mock(script).await;
        let mut config = alert_env(dir.path(), OWNER);
        config.api_base = mock.base.clone();
        let rep = report_with("x", "k");
        maybe_alert(&config, &rep, NOW, &shanghai()).await.unwrap();
        maybe_alert(&config, &rep, NOW + 1.0, &shanghai())
            .await
            .unwrap();
        assert_eq!(
            messages(&log).len(),
            2,
            "没发成功就不能记冷却，下一轮要重试"
        );
        assert!(!config.state_dir.join("last_alert.json").exists());
    }
}

#[tokio::test]
async fn unconfigured_alert_channel_sends_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let (mock, log) = discord_mock(DiscordScript::default()).await;
    let mut config = cfg(dir.path()); // 没有 .env
    config.api_base = mock.base.clone();
    maybe_alert(&config, &report_with("x", "k"), NOW, &shanghai())
        .await
        .unwrap();
    std::fs::write(&config.env_file, "DISCORD_BOT_TOKEN=synthetic-bot-token\n").unwrap(); // 有 token 但没有收件人
    maybe_alert(&config, &report_with("x", "k"), NOW, &shanghai())
        .await
        .unwrap();
    // 只配了频道：频道不再读，等于没配
    std::fs::write(
        &config.env_file,
        "DISCORD_BOT_TOKEN=synthetic-bot-token\nALERT_CHANNEL_ID=900000000000000010\n",
    )
    .unwrap();
    maybe_alert(&config, &report_with("x", "k"), NOW, &shanghai())
        .await
        .unwrap();
    assert!(log.lock().unwrap().is_empty());
}
