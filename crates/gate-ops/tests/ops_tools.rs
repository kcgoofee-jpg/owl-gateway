//! 扩展巡检、report、traffic：全部在合成数据库 / 合成日志上跑（gate-db 建库，Discord 指向本机假服务）。

mod common;

use common::*;
use gate_ops::checks::{ExtraCtx, run_extra_checks};
use gate_ops::patrol::{Report, Sev};
use gate_ops::{report, traffic};
use jiff::tz::TimeZone;
use rusqlite::{Connection, OpenFlags, params};
use std::path::Path;

/// 2026-09-21 22:13:20 +08:00
const NOW: f64 = 1_790_000_000.0;

/// 合成的自签证书（CN=owl.example，2026-10-16 签发，10 年有效），notAfter = 2036-10-13 15:53:25 UTC。
const CERT: &str = "-----BEGIN CERTIFICATE-----
MIIBYDCCAQagAwIBAgIUNEOZlznuFPR9UhUHAIzzzlhPYdYwCgYIKoZIzj0EAwIw
FjEUMBIGA1UEAwwLb3dsLmV4YW1wbGUwHhcNMjYxMDE2MTU1MzI1WhcNMzYxMDEz
MTU1MzI1WjAWMRQwEgYDVQQDDAtvd2wuZXhhbXBsZTBZMBMGByqGSM49AgEGCCqG
SM49AwEHA0IABLqj2tn916OT0EbuRVEB2OWF3H1aGPIp7xtkKQEen/fogAv1IIud
XD8UZaWnKM4hlWZXKQFbb25G76huqTVUhtKjMjAwMB0GA1UdDgQWBBSsqPeFhg6U
wh04LKgWIGZJVZyG/TAPBgNVHRMBAf8EBTADAQH/MAoGCCqGSM49BAMCA0gAMEUC
IQDv5eltUxHf0vYg5GmMnDBQFTwoFB5jdsAXk14xsNReRwIgMYceR7U62VI73WP8
syp5fwaFfo4ct/G1t2E50jBYk3w=
-----END CERTIFICATE-----
";
const CERT_NOT_AFTER: i64 = 2_107_526_005;

fn shanghai() -> TimeZone {
    TimeZone::get("Asia/Shanghai").unwrap()
}

fn ro(path: &Path) -> Connection {
    Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap()
}

#[allow(clippy::too_many_arguments)]
fn log(
    c: &Connection,
    ts: f64,
    key: i64,
    kind: &str,
    status: &str,
    reason: &str,
    wait_ms: i64,
    up: i64,
    anlas: f64,
    client: &str,
) {
    c.execute(
        "INSERT INTO usage_log(ts, key_id, key_name, kind, status, reason, wait_ms, up_status, anlas, client) \
         VALUES (?,?,?,?,?,?,?,?,?,?)",
        params![ts, key, "synthetic", kind, status, reason, wait_ms, up, anlas, client],
    )
    .unwrap();
}

fn ctx(dir: &Path) -> ExtraCtx {
    ExtraCtx {
        audit_dir: dir.join("audit_images"),
        caddy_data: None,
        site_host: Some("owl.example".into()),
        gate_started_at: None,
        gate_container: "owl-gate".into(),
    }
}

fn find<'a>(rep: &'a Report, area: &str) -> Vec<&'a gate_ops::patrol::Row> {
    rep.rows.iter().filter(|r| r.area == area).collect()
}

// ------------------------------------------------------------ 扩展巡检

#[test]
fn extras_on_empty_db_are_quiet() {
    let dir = tempfile::tempdir().unwrap();
    let db = new_db(dir.path());
    let conn = ro(&db);
    let mut rep = Report::default();
    run_extra_checks(&mut rep, &conn, &ctx(dir.path()), NOW).unwrap();
    assert_eq!(rep.worst(), Sev::Ok);
    let skew = &find(&rep, "账号")[0].text;
    assert!(skew.contains("样本不足"), "{skew}");
    assert!(find(&rep, "Anlas")[0].text.contains("n/a"));
    assert!(find(&rep, "证书")[0].text.contains("n/a"));
    assert!(find(&rep, "原图")[0].text.contains("n/a"));
}

#[test]
fn queue_health_and_upstream_auth() {
    let dir = tempfile::tempdir().unwrap();
    let db = new_db(dir.path());
    let w = rw(&db);
    add_key(&w, 1, "tok-1", true);
    for i in 0..10 {
        log(
            &w,
            NOW - 60.0 - i as f64,
            1,
            "image",
            "ok",
            "",
            50_000,
            200,
            0.0,
            "",
        );
    }
    for i in 0..6 {
        log(
            &w,
            NOW - 30.0 - i as f64,
            1,
            "image",
            "rejected",
            "queue_full",
            0,
            0,
            0.0,
            "",
        );
    }
    // 窗口外的不算
    log(
        &w,
        NOW - 4000.0,
        1,
        "image",
        "rejected",
        "queue_timeout",
        0,
        0,
        0.0,
        "",
    );
    for i in 0..2 {
        log(
            &w,
            NOW - 100.0 - i as f64,
            1,
            "image",
            "error",
            "",
            0,
            403,
            0.0,
            "",
        );
    }
    drop(w);
    let conn = ro(&db);
    let mut rep = Report::default();
    run_extra_checks(&mut rep, &conn, &ctx(dir.path()), NOW).unwrap();
    let q = &find(&rep, "队列")[0];
    assert_eq!(q.sev, Sev::Warn, "{}", q.text);
    assert!(
        q.text.contains("queue_full 6") && q.text.contains("queue_timeout 0"),
        "{}",
        q.text
    );
    assert!(q.text.contains("p90 50.0s"), "{}", q.text);
    let up = find(&rep, "通道");
    assert_eq!(up[0].sev, Sev::Warn);
    assert!(up[0].text.contains("403 × 2"));
}

#[test]
fn account_skew_uses_token_id_when_present() {
    let dir = tempfile::tempdir().unwrap();
    let db = new_db(dir.path());
    let w = rw(&db);
    add_key(&w, 1, "tok-1", true);
    for i in 0..110 {
        w.execute(
            "INSERT INTO usage_log(ts, key_id, kind, status, token_id) VALUES (?,?,?,?,?)",
            params![NOW - 100.0 - i as f64, 1, "image", "ok", "acct-aaaaaaaa"],
        )
        .unwrap();
    }
    for i in 0..10 {
        w.execute(
            "INSERT INTO usage_log(ts, key_id, kind, status, token_id) VALUES (?,?,?,?,?)",
            params![NOW - 100.0 - i as f64, 1, "image", "ok", "acct-bbbbbbbb"],
        )
        .unwrap();
    }
    drop(w);
    let conn = ro(&db);
    let mut rep = Report::default();
    run_extra_checks(&mut rep, &conn, &ctx(dir.path()), NOW).unwrap();
    let r = &find(&rep, "账号")[0];
    assert_eq!(r.sev, Sev::Warn, "{}", r.text);
    assert!(
        r.text.contains("acct-a") && r.text.contains("92%"),
        "{}",
        r.text
    );
}

#[test]
fn anlas_estimate_from_latest_reconciliation() {
    let dir = tempfile::tempdir().unwrap();
    let db = new_db(dir.path());
    let w = rw(&db);
    add_key(&w, 1, "tok-1", true);
    // 对账之前的一条（id=1）和之后的两条
    log(
        &w,
        NOW - 5.0 * 86400.0,
        1,
        "image",
        "ok",
        "",
        0,
        200,
        40.0,
        "",
    );
    let expires = NOW + 2.0 * 86400.0;
    let snap = serde_json::json!({
        "checked_at": NOW - 4.0 * 86400.0,
        "balance": 1000,
        "ledger": {"last_log_id": 1},
        "accounts": {"acct-a": {"fixed": 900, "purchased": 100, "expires_at": expires}},
    });
    w.execute(
        "INSERT INTO anlas_reconciliations(snapshot) VALUES (?)",
        [snap.to_string()],
    )
    .unwrap();
    log(
        &w,
        NOW - 3.0 * 86400.0,
        1,
        "image",
        "ok",
        "",
        0,
        200,
        100.0,
        "",
    );
    log(&w, NOW - 3600.0, 1, "image", "ok", "", 0, 200, 50.0, "");
    drop(w);
    let conn = ro(&db);
    let mut rep = Report::default();
    run_extra_checks(&mut rep, &conn, &ctx(dir.path()), NOW).unwrap();
    let rows = find(&rep, "Anlas");
    assert!(
        rows[0]
            .text
            .contains("对账余额 1000，之后记账消耗 150，约剩 850"),
        "{}",
        rows[0].text
    );
    assert!(
        rows.iter()
            .any(|r| r.text.contains("对账已") && r.sev == Sev::Warn)
    );
    assert!(
        rows.iter()
            .any(|r| r.text.contains("到期") && r.sev == Sev::Warn)
    );
}

#[test]
fn originals_dir_and_tls() {
    let dir = tempfile::tempdir().unwrap();
    let db = new_db(dir.path());
    let w = rw(&db);
    set_setting(&w, "audit_image_cap_mb", "1");
    drop(w);
    let audit = dir.path().join("audit_images/20260921");
    std::fs::create_dir_all(&audit).unwrap();
    std::fs::write(audit.join("a.png"), vec![0u8; 1_700_000]).unwrap();
    // Caddy 数据目录
    let certs = dir.path().join("caddy/certificates/acme-v02/owl.example");
    std::fs::create_dir_all(&certs).unwrap();
    std::fs::write(certs.join("owl.example.crt"), CERT).unwrap();
    assert_eq!(
        gate_ops::checks::cert_not_after_pem(CERT),
        Some(CERT_NOT_AFTER)
    );
    let mut c = ctx(dir.path());
    c.caddy_data = Some(dir.path().join("caddy"));
    // 证书剩 3 天
    let now = CERT_NOT_AFTER as f64 - 3.0 * 86400.0;
    let conn = ro(&db);
    let mut rep = Report::default();
    run_extra_checks(&mut rep, &conn, &c, now).unwrap();
    let o = &find(&rep, "原图")[0];
    assert_eq!(o.sev, Sev::Red, "{}", o.text); // 1.7MB / 1MB 上限 = 162%
    let t = &find(&rep, "证书")[0];
    assert_eq!(t.sev, Sev::Red, "{}", t.text);
    assert!(
        t.text.contains("owl.example") && t.text.contains("3.0 天"),
        "{}",
        t.text
    );
}

#[test]
fn role_pending_counts_only_enabled_real_keys() {
    let dir = tempfile::tempdir().unwrap();
    let db = new_db(dir.path());
    let w = rw(&db);
    for i in 1..=3 {
        add_key(&w, i, &format!("tok-{i}"), i != 3);
        w.execute(
            "INSERT INTO discord_registrations(discord_id, key_id, created_at, role_granted) VALUES (?,?,?,0)",
            params![format!("90000000000000000{i}"), i, NOW - 100.0],
        )
        .unwrap();
    }
    drop(w);
    let conn = ro(&db);
    let mut rep = Report::default();
    run_extra_checks(&mut rep, &conn, &ctx(dir.path()), NOW).unwrap();
    let r = &find(&rep, "成员")[0];
    assert!(r.text.contains("2 人"), "{}", r.text);
    assert_eq!(r.sev, Sev::Ok);
}

#[test]
fn restart_note_only_when_recent() {
    let dir = tempfile::tempdir().unwrap();
    let db = new_db(dir.path());
    let conn = ro(&db);
    let mut c = ctx(dir.path());
    c.gate_started_at = Some(NOW - 600.0);
    let mut rep = Report::default();
    run_extra_checks(&mut rep, &conn, &c, NOW).unwrap();
    assert!(find(&rep, "系统")[0].text.contains("10 分钟前启动"));
    c.gate_started_at = Some(NOW - 7200.0);
    let mut rep = Report::default();
    run_extra_checks(&mut rep, &conn, &c, NOW).unwrap();
    assert!(find(&rep, "系统").is_empty());
}

// ------------------------------------------------------------ report（快照）

fn snowflake(unix_secs: f64) -> String {
    let ms = (unix_secs * 1000.0) as u64 - 1_420_070_400_000;
    (ms << 22).to_string()
}

const UA_A: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) SyntheticBrowser/1.0";
const UA_IOS: &str = "Mozilla/5.0 (iPhone; CPU iPhone OS 17_0) SyntheticMobile/1.0";
const UA_ANDROID: &str = "Mozilla/5.0 (Linux; Android 14) SyntheticMobile/1.0";

fn build_report_db(dir: &Path) -> std::path::PathBuf {
    let db = new_db(dir);
    let w = rw(&db);
    for (id, name) in [
        (1, "alpha"),
        (2, "beta"),
        (3, "gamma"),
        (5, "n5"),
        (6, "n6"),
        (7, "n7"),
        (8, "n8"),
        (9, "n9"),
    ] {
        w.execute(
            "INSERT INTO api_keys(id, name, token, enabled, created_at) VALUES (?,?,?,1,?)",
            params![id, name, format!("tok-{id}"), 1.7e9],
        )
        .unwrap();
    }
    // 1 号：重试轰炸（22 个 key_rate + 3 个 key_busy，每 2 秒一次）
    for i in 0..25 {
        let reason = if i < 22 { "key_rate" } else { "key_busy" };
        log(
            &w,
            NOW - 3600.0 + 2.0 * i as f64,
            1,
            "image",
            "rejected",
            reason,
            0,
            0,
            0.0,
            UA_A,
        );
    }
    // 2 号：每分钟一张，连续 4 小时，共用设备 UA_A
    for i in 0..240 {
        log(
            &w,
            NOW - 86400.0 + 60.0 * i as f64,
            2,
            "image",
            "ok",
            "",
            0,
            200,
            0.0,
            UA_A,
        );
    }
    // 3 号：三天都碰到个人额度上限；同一天 iOS + Android
    for d in 0..3 {
        log(
            &w,
            NOW - 86400.0 * (d as f64 + 1.0),
            3,
            "image",
            "rejected",
            "member_quota",
            0,
            0,
            0.0,
            UA_IOS,
        );
    }
    log(
        &w,
        NOW - 86400.0 * 1.0 + 30.0,
        3,
        "image",
        "ok",
        "",
        0,
        200,
        0.0,
        UA_ANDROID,
    );
    // 审计：1、2 号相同的长提示词；3 号命中词表
    let long = "synthetic long prompt ".repeat(6);
    for (k, p) in [
        (1, long.as_str()),
        (2, long.as_str()),
        (3, "a forbiddenword here"),
        (3, "harmless"),
    ] {
        w.execute(
            "INSERT INTO generation_audit(ts, key_id, key_name, prompt) VALUES (?,?,?,?)",
            params![NOW - 500.0, k, "synthetic", p],
        )
        .unwrap();
    }
    // 领取：1 号账号年龄 2 天；5~9 号 5 分钟内连领；其余老号
    let reg = |w: &Connection, key: i64, ts: f64, age_days: f64| {
        w.execute(
            "INSERT INTO discord_registrations(discord_id, key_id, created_at) VALUES (?,?,?)",
            params![snowflake(ts - age_days * 86400.0), key, ts],
        )
        .unwrap();
    };
    reg(&w, 1, NOW - 5.0 * 86400.0, 2.0);
    reg(&w, 2, NOW - 4.0 * 86400.0, 900.0);
    for (i, k) in (5..=9).enumerate() {
        reg(&w, k, NOW - 3.0 * 86400.0 + 60.0 * i as f64, 400.0);
    }
    // 现有暂停
    w.execute(
        "INSERT INTO share_state(key_id, pause_reason, paused_until) VALUES (3, ?, ?)",
        params![gate_core::autopilot::KEY_GUARD_PAUSE_REASON, NOW + 3600.0],
    )
    .unwrap();
    db
}

/// 对照 tests/snapshots/ops/<name>；`UPDATE_SNAPSHOT=1` 本地重写（CI 里禁止）。
fn golden(name: &str, actual: &str) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/snapshots/ops")
        .join(name);
    if std::env::var("UPDATE_SNAPSHOT").is_ok_and(|v| v == "1") {
        assert!(std::env::var("CI").is_err(), "CI 里禁止更新快照");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, actual).unwrap();
        return;
    }
    let want = std::fs::read_to_string(&path).unwrap_or_else(|_| {
        panic!(
            "缺快照 {}；本地用 UPDATE_SNAPSHOT=1 生成并审阅",
            path.display()
        )
    });
    assert_eq!(
        actual, want,
        "快照 {name} 不一致（有意改动请 UPDATE_SNAPSHOT=1 后审阅 git diff）"
    );
}

#[test]
fn weekly_report_snapshot_without_terms() {
    let dir = tempfile::tempdir().unwrap();
    let db = build_report_db(dir.path());
    let text = report::build(
        &ro(&db),
        &report::ReportArgs {
            days: 7,
            terms: Some(std::path::PathBuf::from("/nonexistent/risk-terms.txt")),
            until: NOW,
        },
        &shanghai(),
    )
    .unwrap();
    golden("report_weekly.txt", &text);
    // 只有 Key 编号 / 显示名 / 计数：没有提示词、UA 原文
    assert!(!text.contains("synthetic long prompt"));
    assert!(!text.contains("SyntheticBrowser"));
    assert!(!text.contains("forbiddenword"));
}

#[test]
fn weekly_report_with_terms_file_prints_counts_only() {
    let dir = tempfile::tempdir().unwrap();
    let db = build_report_db(dir.path());
    let terms = dir.path().join("terms.txt");
    std::fs::write(&terms, "## 合成词表\n# demo-category\nforbidden\\w+\n(broken\n# empty-category\nnever-matches-anything\n").unwrap();
    let text = report::build(
        &ro(&db),
        &report::ReportArgs {
            days: 7,
            terms: Some(terms),
            until: NOW,
        },
        &shanghai(),
    )
    .unwrap();
    golden("report_weekly_terms.txt", &text);
    assert!(!text.contains("forbiddenword"));
}

// ------------------------------------------------------------ report：小号 / 共用筛查（按账号）

/// 合成的设备 UA（都 ≥ 24 字，算「具体设备」）。
const UA_RARE1: &str = "Mozilla/5.0 (Linux; Android 15; SYN-PHONE-1) SyntheticMobile/2.0";
const UA_RARE2: &str = "Mozilla/5.0 (Linux; Android 14; SYN-PHONE-2) SyntheticMobile/2.0";
const UA_RARE3: &str = "Mozilla/5.0 (Linux; Android 14; SYN-PHONE-2) SyntheticWebView/9.0";
const UA_RARE4: &str = "Mozilla/5.0 (Linux; Android 14; SYN-PHONE-2) SyntheticLauncher/1.0";
const UA_COMMON: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) SyntheticBrowser/1.0";

/// 合成 Discord ID（不是任何真实账号）。
fn did(n: i64) -> String {
    format!("9000000000000{n:05}")
}

/// 账号布局（Key 编号 / 显示名 / Discord）：
/// - 甲：#1（旧，没登记账号，显示名同为「(@jia_a)」）+ #11 → 同一账号，共用 UA_RARE1 不算两个人；
/// - P：#2（已删，有归属记录）+ #12；Q：#3（已删，没有归属记录，只能靠日志里的显示名「(@qq)」）+ #13；
///   P、Q 共用 UA_RARE2 / UA_RARE3 / UA_RARE4 三个罕见设备，提示词完全一样 → 两节都要报；
/// - R #14、S #15：同一完整 IP + 同一（常见）UA → 设备一节报；
/// - #16、#17：提示词完全一样但设备 / IP 不重合 → 不报；
/// - #30、#31：都没登记账号、显示名完全一样 → 算同一个人，不报；
/// - #20～#25：六个账号共用 UA_COMMON（大组，不算罕见）→ 不报，提示词只有一部分相近；
/// - 2.x 留下的打码 `src`（`203.0.*.*`）不参与 IP 比对。
fn build_alt_db(dir: &Path) -> std::path::PathBuf {
    let db = new_db(dir);
    let w = rw(&db);
    let key = |id: i64, name: &str, d: Option<String>| {
        w.execute(
            "INSERT INTO api_keys(id, name, token, enabled, created_at, discord_id) VALUES (?,?,?,1,?,?)",
            params![id, name, format!("tok-{id}"), 1.7e9, d],
        )
        .unwrap();
    };
    key(1, "小甲 (@jia_a)", None);
    key(11, "小甲 (@jia_a)", Some(did(1)));
    key(12, "P (@pp)", Some(did(2)));
    key(13, "Q (@qq)", Some(did(3)));
    key(14, "R", Some(did(4)));
    key(15, "S", Some(did(5)));
    key(16, "T6", Some(did(6)));
    key(17, "T7", Some(did(7)));
    for k in 20..=25 {
        key(k, &format!("C{k}"), Some(did(k)));
    }
    // #30、#31：都没有账号、显示名完全一样（没有用户名）→ 同一个人，共用两个罕见设备也不报
    key(30, "月影", None);
    key(31, "月影", None);
    // #2 删 Key 前留下了归属；#3 是归属表上线前删的，什么也没留
    w.execute(
        "INSERT INTO deleted_key_owners(key_id, discord_id, name, deleted_at) VALUES (2, ?, 'P 旧 (@pp)', ?)",
        params![did(2), NOW - 6.0 * 86400.0],
    )
    .unwrap();
    let log = |ts: f64, k: i64, name: &str, ua: &str, src: &str| {
        w.execute(
            "INSERT INTO usage_log(ts, key_id, key_name, kind, status, images, client, src) VALUES (?,?,?,'image','ok',1,?,?)",
            params![ts, k, name, ua, src],
        )
        .unwrap();
    };
    let h = 3600.0;
    let t0 = NOW - 3.0 * 86400.0;
    // 甲：新旧 Key 同一设备
    log(t0, 1, "小甲 (@jia_a)", UA_RARE1, "");
    log(t0 + h, 11, "小甲 (@jia_a)", UA_RARE1, "");
    // P：#2（已删）、#12；Q：#3（已删）、#13。P 活跃 4 个小时，Q 活跃 3 个小时，其中 2 个小时重合
    log(NOW - 5.0 * 86400.0, 2, "P 旧 (@pp)", UA_RARE2, "203.0.*.*");
    log(NOW - 4.0 * 86400.0, 3, "旧号 (@qq)", UA_RARE2, "203.0.*.*");
    log(t0, 12, "P (@pp)", UA_RARE3, "");
    log(t0 + h, 12, "P (@pp)", UA_RARE4, "");
    log(t0 + 10.0, 13, "Q (@qq)", UA_RARE3, "");
    log(t0 + h + 10.0, 13, "Q (@qq)", UA_RARE4, "");
    // R、S：同一完整 IP、同一常见 UA（各 1 小时，同一小时）
    log(t0, 14, "R", UA_COMMON, "203.0.113.50");
    log(t0 + 60.0, 15, "S", UA_COMMON, "203.0.113.50");
    // #16、#17：各自的设备和 IP
    log(
        t0,
        16,
        "T6",
        UA_RARE1.replace("SYN-PHONE-1", "SYN-PHONE-6").as_str(),
        "198.51.100.6",
    );
    log(
        t0,
        17,
        "T7",
        UA_RARE1.replace("SYN-PHONE-1", "SYN-PHONE-7").as_str(),
        "198.51.100.7",
    );
    for k in [30, 31] {
        log(
            t0 + k as f64,
            k,
            "月影",
            &UA_RARE1.replace("SYN-PHONE-1", "SYN-PHONE-30"),
            "",
        );
        log(
            t0 + h + k as f64,
            k,
            "月影",
            &UA_RARE1.replace("SYN-PHONE-1", "SYN-PHONE-31"),
            "",
        );
    }
    // 大组：六个账号同一 UA，各自不同的 IP
    for k in 20..=25 {
        log(
            t0 + k as f64,
            k,
            &format!("C{k}"),
            UA_COMMON,
            &format!("192.0.2.{k}"),
        );
    }
    // 提示词：P / Q 一样，#16 / #17 一样
    let audit = |k: i64, p: &str| {
        w.execute(
            "INSERT INTO generation_audit(ts, key_id, key_name, prompt) VALUES (?,?,?,?)",
            params![t0, k, "synthetic", p],
        )
        .unwrap();
    };
    for (k, base) in [(12, "pq"), (13, "pq"), (16, "tt"), (17, "tt")] {
        for i in 0..3 {
            audit(
                k,
                &format!(
                    "masterpiece, {{best quality}}, 1.2::{base} hat::, {base} scarf{i}, {base} park, {base} rain"
                ),
            );
        }
    }
    // Q 的旧 Key 也写过一样的提示词（归到 Q 账号）
    audit(3, "masterpiece, pq park");
    // 相邻的两个账号共用两个词，其余只共用两个常见词：相似度 0.5 / 0.2，都低于 P / Q
    for k in 20..=25 {
        for i in 0..3 {
            audit(
                k,
                &format!(
                    "masterpiece, best quality, x{k}, x{}, y{k}, y{}, c{k} only{i}",
                    k + 1,
                    k + 1
                ),
            );
        }
    }
    db
}

#[test]
fn weekly_report_groups_by_account_and_flags_shared_rare_devices_and_similar_prompts() {
    let dir = tempfile::tempdir().unwrap();
    let db = build_alt_db(dir.path());
    let text = report::build(
        &ro(&db),
        &report::ReportArgs {
            days: 7,
            terms: Some(std::path::PathBuf::from("/nonexistent/risk-terms.txt")),
            until: NOW,
        },
        &shanghai(),
    )
    .unwrap();
    golden("report_alt_accounts.txt", &text);
    let section = |title: &str| {
        let start = text
            .find(title)
            .unwrap_or_else(|| panic!("缺一节：{title}"));
        let rest = &text[start..];
        rest[..rest[1..].find("\n[").map_or(rest.len(), |i| i + 1)].to_string()
    };
    let devices = section("[不同账号共用罕见设备");
    let similar = section("[提示词相似");
    // P、Q：新旧 Key 合并成账号；三个罕见设备；同一小时都活跃 2 小时
    let pq = "#12 P (@pp)（旧 Key #2） ↔ #13 Q (@qq)（旧 Key #3）";
    assert!(
        devices.contains(&format!(
            "{pq}  罕见设备 3 个 · 同 IP 同 UA 0 组 · 活跃 3 / 3 小时，同一小时都活跃 2 小时"
        )),
        "{devices}"
    );
    assert!(
        similar.contains(&format!("{pq}  相似度 1.000")),
        "{similar}"
    );
    // R、S：同一完整 IP + 同一 UA
    assert!(
        devices.contains("#14 R ↔ #15 S  罕见设备 0 个 · 同 IP 同 UA 1 组"),
        "{devices}"
    );
    // 同一账号的新旧 Key 不算两个人；六个账号共用的常见 UA 不报；提示词一样但没有设备 / IP 重合的不报
    assert!(
        !text.contains("#1 小甲") && !text.contains("↔ #11"),
        "{text}"
    );
    assert!(
        !devices.contains("C20") && !devices.contains("C25"),
        "{devices}"
    );
    assert!(
        !similar.contains("T6") && !similar.contains("T7"),
        "{similar}"
    );
    assert!(
        devices.contains("已删 Key 有日志的 2 把，其中 1 把没有归属记录"),
        "{devices}"
    );
    // 输出里没有提示词、UA、IP、Discord ID
    for secret in [
        "pq park",
        "masterpiece",
        "SYN-PHONE",
        "Synthetic",
        "203.0.113",
        "198.51.100",
        "192.0.2",
        "9000000000000",
    ] {
        assert!(!text.contains(secret), "报告里不该出现 {secret}");
    }
}

#[test]
fn prompt_tokens_and_similarity_helpers() {
    let t = report::prompt_tokens(
        "Masterpiece, {best quality}, 1.2::red hat::, [[ blue ]],\n, , -0.5::bad hands::",
    );
    assert_eq!(
        t.into_iter().collect::<Vec<_>>(),
        vec![
            "bad hands",
            "best quality",
            "blue",
            "masterpiece",
            "red hat"
        ]
    );
    assert_eq!(report::jaccard(&[1, 2, 3], &[2, 3, 4]), 0.5);
    assert_eq!(report::jaccard(&[], &[]), 0.0);
    assert_eq!(
        report::name_handle("小甲 (@Jia_A)").as_deref(),
        Some("jia_a")
    );
    assert_eq!(
        report::name_handle("plain.user").as_deref(),
        Some("plain.user")
    );
    assert_eq!(report::name_handle("只有中文名"), None);
    assert!(report::is_full_ip("203.0.113.9") && report::is_full_ip("2001:db8::1"));
    assert!(!report::is_full_ip("203.0.*.*") && !report::is_full_ip(""));
}

#[test]
fn shipped_example_terms_file_is_empty_of_rules() {
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../deploy/risk-terms.example.txt");
    let text = std::fs::read_to_string(&p).expect("仓库应带 deploy/risk-terms.example.txt");
    let (rules, bad) = report::parse_terms(&text);
    assert!(rules.is_empty(), "示例词表里不许有任何规则");
    assert!(bad.is_empty());
    for l in text.lines() {
        assert!(
            l.trim().is_empty() || l.starts_with("##"),
            "示例词表只许有 ## 注释：{l}"
        );
    }
}

// ------------------------------------------------------------ traffic

#[test]
fn traffic_summary_strips_ip_and_query() {
    let dir = tempfile::tempdir().unwrap();
    let lines = [
        // 带 IP / 头 / 查询参数的原始 Caddy 行：这些都不该出现在输出里
        r#"{"level":"info","ts":1789999000.5,"msg":"handled request","request":{"remote_ip":"203.0.113.9","client_ip":"203.0.113.9","method":"POST","uri":"/ai/generate-image?secret=abc","headers":{"Authorization":["Bearer synthetic"]}},"duration":2.0,"size":10,"status":200}"#,
        r#"{"level":"info","ts":1789999010.0,"request":{"method":"POST","uri":"/ai/generate-image-stream"},"duration":4.0,"status":200}"#,
        r#"{"level":"info","ts":1789999020.0,"request":{"method":"GET","uri":"/healthz"},"duration":0.001,"status":200}"#,
        r#"{"level":"info","ts":1789999030.0,"request":{"method":"GET","uri":"/admin/api/members?page=2"},"duration":0.05,"status":401}"#,
        r#"{"level":"info","ts":1789999040.0,"request":{"method":"GET","uri":"/v1/me"},"duration":0.01,"status":502}"#,
        r#"{"level":"info","ts":1789000000.0,"request":{"method":"GET","uri":"/old"},"duration":0.01,"status":200}"#,
        "not json at all",
    ];
    let p = dir.path().join("access.log");
    std::fs::write(&p, lines.join("\n")).unwrap();
    // 再来一个 .gz 滚动文件
    {
        use std::io::Write;
        let f = std::fs::File::create(dir.path().join("access-old.log.gz")).unwrap();
        let mut gz = flate2::write::GzEncoder::new(f, flate2::Compression::fast());
        gz.write_all(lines[2].as_bytes()).unwrap();
        gz.finish().unwrap();
    }
    let text = traffic::run_tz(&[dir.path().to_path_buf()], 3600.0, NOW, &shanghai()).unwrap();
    assert!(!text.contains("203.0.113"), "{text}");
    assert!(!text.contains("secret"), "{text}");
    assert!(!text.contains("Authorization"), "{text}");
    assert!(text.contains("image-gen") && text.contains("admin-api") && text.contains("gate-api"));
    // 6 个窗口内请求（5 个来自 access.log + 1 个来自 .gz）；/old 在窗口外；1 行坏数据被跳过
    assert!(text.contains("共 6 个请求"), "{text}");
    assert!(text.contains("跳过 1 行"), "{text}");
    let _ = p;
    golden("traffic.txt", &text);
}
