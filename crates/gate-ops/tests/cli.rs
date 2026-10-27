//! 二进制 `nai-ops` 的命令行层：参数解析、退出码。
//! 子进程清掉 Discord 相关环境变量；`DISCORD_API_BASE` 本身已无消费者（巡检删除后 nai-ops 不再发消息），
//! 这里保留「指向本机没人监听的端口」的兜底，代码出错也不可能外发。

mod common;

use common::*;
use std::path::Path;
use std::process::{Command, Output};

fn nai(dir: &Path) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_nai-ops"));
    c.current_dir(dir)
        .env_remove("DISCORD_BOT_TOKEN")
        .env_remove("NAI_GATE_DB")
        .env_remove("SMOKE_KEY")
        .env_remove("SITE_URL")
        .env("DISCORD_API_BASE", "http://127.0.0.1:1")
        .env("DATA_DIR", dir.join("data"))
        .env("NAI_GATE_ENV", dir.join("missing.env"));
    c
}

fn out(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}
fn err(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}
fn code(o: &Output) -> i32 {
    o.status.code().unwrap_or(-1)
}

#[test]
fn help_lists_every_subcommand() {
    let dir = tempfile::tempdir().unwrap();
    let o = nai(dir.path()).arg("--help").output().unwrap();
    assert_eq!(code(&o), 0);
    let text = out(&o);
    for sub in ["set", "get", "traffic", "snapshot", "ro-sanitize", "smoke"] {
        assert!(text.contains(sub), "--help 里没有 {sub}：{text}");
    }
    // 群发私信、公告、状态页事件、提示词统计都已删除（后台已无入口，事件直接写库）；
    // 周报与巡检 10-27 删除（站长不需要，扩展检查里的统计后台都有）。
    for gone in [
        "dm-broadcast",
        "announce",
        "incident",
        "prompt-stats",
        "patrol",
        "report",
    ] {
        assert!(!text.contains(gone), "--help 里不该再有 {gone}：{text}");
    }
}

#[test]
fn snapshot_then_sanitize_through_the_cli() {
    let dir = tempfile::tempdir().unwrap();
    let db = new_db(dir.path());
    add_key(&rw(&db), 1, "nai-synthetic-key-PLAINTEXT", true);
    let gz = dir.path().join("snap.db.gz");
    let o = nai(dir.path())
        .arg("snapshot")
        .arg(&gz)
        .arg("--db")
        .arg(&db)
        .output()
        .unwrap();
    assert_eq!(code(&o), 0, "{}", err(&o));
    assert!(out(&o).contains(", integrity ok, keys=1)"), "{}", out(&o));

    let mut raw = Vec::new();
    std::io::Read::read_to_end(
        &mut flate2::read::GzDecoder::new(std::fs::File::open(&gz).unwrap()),
        &mut raw,
    )
    .unwrap();
    let plain = dir.path().join("plain.db");
    std::fs::write(&plain, raw).unwrap();
    let o = nai(dir.path())
        .arg("ro-sanitize")
        .arg(&plain)
        .output()
        .unwrap();
    assert_eq!(
        (code(&o), out(&o).as_str()),
        (0, "sanitized 1 keys\n"),
        "{}",
        err(&o)
    );
    assert!(
        !std::fs::read(&plain)
            .unwrap()
            .windows(10)
            .any(|w| w == b"PLAINTEXT")
    );

    // 源库不存在：失败，不留下输出
    let o = nai(dir.path())
        .arg("snapshot")
        .arg(dir.path().join("x.db.gz"))
        .arg("--db")
        .arg(dir.path().join("none.db"))
        .output()
        .unwrap();
    assert_eq!(code(&o), 1);
    assert!(!dir.path().join("x.db.gz").exists());
}

#[test]
fn smoke_failures_exit_nonzero() {
    let dir = tempfile::tempdir().unwrap();
    let o = nai(dir.path())
        .args(["smoke", "http://127.0.0.1:1", "--timeout", "2"])
        .output()
        .unwrap();
    assert_eq!(code(&o), 1);
    assert!(
        out(&o).contains("FAIL") && out(&o).contains("=== 汇总 ==="),
        "{}",
        out(&o)
    );
}

#[test]
fn smoke_needs_a_base_url_and_reads_site_url() {
    let dir = tempfile::tempdir().unwrap();
    // 没有地址：参数错误（退出码 2），不会去连任何写死的站点
    let o = nai(dir.path()).arg("smoke").output().unwrap();
    assert_eq!(code(&o), 2, "{}", err(&o));
    // 帮助里说明了可以用环境变量 SITE_URL
    let o = nai(dir.path()).args(["smoke", "--help"]).output().unwrap();
    assert!(out(&o).contains("SITE_URL"), "{}", out(&o));
    // 环境变量 SITE_URL 可代替位置参数
    let o = nai(dir.path())
        .args(["smoke", "--timeout", "2"])
        .env("SITE_URL", "http://127.0.0.1:1")
        .output()
        .unwrap();
    assert_eq!(code(&o), 1, "{}", err(&o));
    assert!(out(&o).contains("FAIL"), "{}", out(&o));
}
