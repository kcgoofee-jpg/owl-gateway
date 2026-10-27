//! 快照与脱敏：全部用合成数据。重点断言：
//! - 快照一致（同一个读事务）、完整性校验通过、除审计原图之外一行不少、索引 / 触发器 / 自增序号原样；
//! - 脱敏后没有 Key 明文、没有任何 BLOB、被清掉的内容连文件的空闲页里都翻不出来（VACUUM）。

mod common;

use common::*;
use gate_ops::{sanitize, snapshot};
use rusqlite::{Connection, OpenFlags};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::io::Read;
use std::path::{Path, PathBuf};

const TOKENS: [&str; 3] = [
    "nai-synthetic-key-AAAA1111",
    "nai-synthetic-key-BBBB2222",
    "nai-synthetic-key-CCCC3333",
];
const SALT: &str = "synthetic-salt-value-0000";
const DONOR_TOKEN: &str = "pst-synthetic-donated-token-not-real";

fn ro(path: &Path) -> Connection {
    Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap()
}

/// 灌一份覆盖所有要处理的列的合成库。
fn seed_source(path: &Path) {
    let c = rw(path);
    for (i, t) in TOKENS.iter().enumerate() {
        add_key(&c, i as i64 + 1, t, true);
    }
    // 审计：3 行带 1MB 原图，再删掉第 3 行 → 自增序号 3 > 最大 id 2，快照必须原样保留序号
    for i in 1..=3 {
        c.execute(
            "INSERT INTO generation_audit(ts,key_id,key_name,kind,model,status,prompt,negative,extra) VALUES(?,?,?,?,?,?,?,?,?)",
            rusqlite::params![1.79e9+f64::from(i),i,format!("synthetic-{i}"),"image","nai-diffusion-4-5-full","ok","synthetic prompt A, 1girl","synthetic negative, lowres","{\"seed\": 42}"]
        )
        .unwrap();
    }
    c.execute("DELETE FROM generation_audit WHERE id=3", [])
        .unwrap();
    for i in 0..50 {
        add_log(
            &c,
            &Log1::image_ok(1.79e9 + f64::from(i), 1 + i64::from(i) % 3),
        );
    }
    c.execute(
        "INSERT INTO counters(key_id, day, images, requests) VALUES (1, '2026-10-14', 7, 9)",
        [],
    )
    .unwrap();
    for (k, v) in [
        ("register_open", "1"),
        ("key_source_salt", SALT),
        ("admin_password_hash", "scrypt$synthetic"),
        ("webhook_secret_x", "synthetic-secret"),
        ("some_token_setting", "synthetic"),
        ("PasswordHint", "synthetic"), // LIKE 不区分大小写
        ("guard_account_daily_cap", "500"),
    ] {
        set_setting(&c, k, v);
    }
    c.execute(
        "INSERT INTO discord_registrations(discord_id, key_id, created_at, avatar) VALUES ('900000000000000001', 1, 1.0, 'avatar-hash-synthetic-1')", [],
    )
    .unwrap();
    c.execute(
        "INSERT INTO discord_bans(discord_id, created_at, reason, avatar) VALUES ('900000000000000009', 1.0, 'synthetic', 'avatar-hash-synthetic-ban')",
        [],
    )
    .unwrap();
    c.execute(
        "INSERT INTO discord_user_connections(discord_id, type, ext_id, name, verified, first_ts, last_ts) \
         VALUES ('900000000000000001', 'steam', '900000000000000088', 'synthetic-name', 1, 1.0, 1.0)",
        [],
    )
    .unwrap();
    c.execute(
        "INSERT INTO key_donations(ts, token, message, contact) VALUES (1.0, ?, 'synthetic message', 'someone@example.invalid')",
        [DONOR_TOKEN],
    )
    .unwrap();
    // 删一把 Key，让触发器在源库里留下 deleted_key_usage_flags（快照必须只有这一份，不能因为灌数据时触发触发器而翻倍）
    add_key(&c, 99, "nai-synthetic-key-deleted", true);
    c.execute("DELETE FROM api_keys WHERE id=99", []).unwrap();
}

fn table_names(c: &Connection) -> Vec<String> {
    let mut stmt = c.prepare("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name").unwrap();
    stmt.query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

fn count(c: &Connection, table: &str) -> i64 {
    c.query_row(&format!("SELECT COUNT(*) FROM \"{table}\""), [], |r| {
        r.get(0)
    })
    .unwrap()
}

fn objects(c: &Connection) -> BTreeSet<(String, String, String)> {
    let mut stmt = c
        .prepare("SELECT type, name, sql FROM sqlite_master WHERE name NOT LIKE 'sqlite_%'")
        .unwrap();
    stmt.query_map([], |r| {
        Ok((
            r.get(0)?,
            r.get(1)?,
            r.get::<_, Option<String>>(2)?.unwrap_or_default(),
        ))
    })
    .unwrap()
    .collect::<Result<_, _>>()
    .unwrap()
}

fn sequences(c: &Connection) -> BTreeSet<(String, i64)> {
    let mut stmt = c.prepare("SELECT name, seq FROM sqlite_sequence").unwrap();
    stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

fn make(src: &Path, dir: &Path, name: &str) -> (PathBuf, snapshot::Info) {
    let out = dir.join(name);
    let info = snapshot::make_snapshot(src, &out).unwrap();
    (out, info)
}

#[test]
fn snapshot_drops_only_the_original_images() {
    let dir = tempfile::tempdir().unwrap();
    let src = new_db(dir.path());
    seed_source(&src);
    let (out, info) = make(&src, dir.path(), "snap.db");
    assert_eq!(
        info,
        snapshot::Info {
            integrity: "ok".into(),
            keys: 3
        }
    );

    let (s, d) = (ro(&src), ro(&out));
    assert_eq!(table_names(&s), table_names(&d));
    for t in table_names(&s) {
        assert_eq!(count(&s, &t), count(&d, &t), "表 {t} 行数");
    }
    // 建表语句 / 索引 / 触发器逐字一致，自增序号原样
    assert_eq!(objects(&s), objects(&d));
    assert_eq!(sequences(&s), sequences(&d));
    assert!(
        sequences(&d).contains(&("generation_audit".to_string(), 3)),
        "序号 3 > 最大 id 2 必须保留"
    );
    // 灌数据时不触发触发器：被删 Key 的标记行只有源库里那一份
    assert_eq!(
        count(&d, "deleted_key_usage_flags"),
        count(&s, "deleted_key_usage_flags")
    );
    // 普通表整表一致（api_keys 含明文 Key——备份要靠它恢复）
    let dump = |c: &Connection| -> Vec<(i64, String)> {
        let mut stmt = c
            .prepare("SELECT id, token FROM api_keys ORDER BY id")
            .unwrap();
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    };
    assert_eq!(dump(&s), dump(&d));

    let kept: (String, String) = d
        .query_row(
            "SELECT prompt,extra FROM generation_audit WHERE id=1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(
        kept,
        (
            "synthetic prompt A, 1girl".to_owned(),
            "{\"seed\": 42}".to_owned()
        )
    );

    let (src_size, out_size) = (
        std::fs::metadata(&src).unwrap().len(),
        std::fs::metadata(&out).unwrap().len(),
    );
    // 快照里是几十张空表 / 索引的页（各占 4KB）+ 少量数据，远小于源库里的 2×1MB 原图
    assert!(
        src_size > 0 && out_size > 0,
        "元数据快照：源 {src_size}B → 快照 {out_size}B"
    );
    // 文件里翻不出原图内容
    let bytes = std::fs::read(&out).unwrap();
    assert!(!bytes.windows(64).any(|w| w.iter().all(|b| *b == 0xAB)));
}

#[test]
fn snapshot_is_one_consistent_point_in_time_even_while_writing() {
    let dir = tempfile::tempdir().unwrap();
    let src = new_db(dir.path());
    {
        let c = rw(&src);
        add_key(&c, 1, TOKENS[0], true);
        c.execute(
            "INSERT INTO counters(key_id, day, requests) VALUES (1, '2026-10-14', 0)",
            [],
        )
        .unwrap();
    }
    // 写线程：每个事务同时改 usage_log 与 counters，两边的数量任何时刻都相等
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let writer = {
        let (path, stop) = (src.clone(), stop.clone());
        std::thread::spawn(move || {
            let c = rw(&path);
            let mut i = 0.0;
            while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                let tx = c.unchecked_transaction().unwrap();
                add_log(&tx, &Log1::image_ok(1.79e9 + i, 1));
                tx.execute("UPDATE counters SET requests=requests+1 WHERE key_id=1", [])
                    .unwrap();
                tx.commit().unwrap();
                i += 1.0;
            }
        })
    };
    std::thread::sleep(std::time::Duration::from_millis(100));
    for n in 0..8 {
        let (out, info) = make(&src, dir.path(), &format!("snap-{n}.db"));
        assert_eq!(info.integrity, "ok");
        let d = ro(&out);
        let logs = count(&d, "usage_log");
        let requests: i64 = d
            .query_row("SELECT requests FROM counters WHERE key_id=1", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(logs, requests, "快照 {n} 里两张表不是同一时刻的数据");
        assert!(logs > 0);
    }
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    writer.join().unwrap();
}

#[test]
fn snapshot_handles_awkward_source_paths() {
    let dir = tempfile::tempdir().unwrap();
    let weird = dir.path().join("we ird#dir?%25 name");
    std::fs::create_dir_all(&weird).unwrap();
    let src = new_db(&weird);
    seed_source(&src);
    let (_, info) = make(&src, dir.path(), "snap.db");
    assert_eq!(info.keys, 3);
}

#[test]
fn run_gzips_with_owner_only_permissions_and_cleans_up() {
    let dir = tempfile::tempdir().unwrap();
    let src = new_db(dir.path());
    seed_source(&src);
    let out_dir = dir.path().join("out");
    std::fs::create_dir_all(&out_dir).unwrap();
    let out = out_dir.join("nai_gate-2026-10-14.db.gz");

    let line = snapshot::run(&src, &out).unwrap();
    assert!(
        line.starts_with(&format!("snapshot {} (", out.display())),
        "{line}"
    );
    assert!(line.ends_with(", integrity ok, keys=3)"), "{line}");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&out).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    let mut raw = Vec::new();
    flate2::read::GzDecoder::new(std::fs::File::open(&out).unwrap())
        .read_to_end(&mut raw)
        .unwrap();
    assert!(raw.starts_with(b"SQLite format 3\0"));
    let unpacked = dir.path().join("unpacked.db");
    std::fs::write(&unpacked, raw).unwrap();
    let d = ro(&unpacked);
    assert_eq!(
        d.query_row::<String, _, _>("PRAGMA integrity_check", [], |r| r.get(0))
            .unwrap(),
        "ok"
    );
    assert_eq!(count(&d, "api_keys"), 3);

    // 输出目录里只剩最终文件：没有 .snap-*.db / .part / -journal
    let left: Vec<_> = std::fs::read_dir(&out_dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(left, ["nai_gate-2026-10-14.db.gz"]);
}

#[test]
fn run_failure_leaves_no_partial_output() {
    let dir = tempfile::tempdir().unwrap();
    let out_dir = dir.path().join("out");
    std::fs::create_dir_all(&out_dir).unwrap();
    let out = out_dir.join("snap.db.gz");
    let err = snapshot::run(&dir.path().join("missing.db"), &out).unwrap_err();
    assert!(!format!("{err:#}").is_empty());
    assert!(!out.exists());
    assert_eq!(
        std::fs::read_dir(&out_dir).unwrap().count(),
        0,
        "失败后不能留下临时快照"
    );
}

// ------------------------------------------------------------ 脱敏

fn sha16(s: &str) -> String {
    format!(
        "sha256:{}",
        &hex::encode(Sha256::digest(s.as_bytes()))[..16]
    )
}

fn sanitized_copy(dir: &Path) -> (PathBuf, usize) {
    let src = new_db(dir);
    seed_source(&src);
    let (out, _) = make(&src, dir, "ro.db");
    let n = sanitize::sanitize(&out).unwrap();
    (out, n)
}

#[test]
fn sanitize_replaces_keys_and_drops_secrets_and_content() {
    let dir = tempfile::tempdir().unwrap();
    let (out, n) = sanitized_copy(dir.path());
    assert_eq!(n, 3);
    let d = ro(&out);

    // Key：不可逆短哈希，同一把 Key 仍能对上
    let mut stmt = d
        .prepare("SELECT id, token FROM api_keys ORDER BY id")
        .unwrap();
    let got: Vec<(i64, String)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(
        got,
        TOKENS
            .iter()
            .enumerate()
            .map(|(i, t)| (i as i64 + 1, sha16(t)))
            .collect::<Vec<_>>()
    );

    // 密钥类设置整条删除（名字匹配不区分大小写），其余保留
    let mut stmt = d
        .prepare("SELECT key FROM site_settings ORDER BY key")
        .unwrap();
    let keys: Vec<String> = stmt
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(
        keys,
        [
            "guard_account_daily_cap",
            "quota_9999_migrated",
            "quota_modes_v1",
            "quota_modes_v2_migrated",
            "register_open"
        ]
    );

    // Bug 追踪（error_events）已删：脱敏后的快照里不允许再出现这张表
    assert_eq!(
        d.query_row::<i64, _, _>(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='error_events'",
            [],
            |r| r.get(0)
        )
        .unwrap(),
        0
    );
    let (p, n_, e): (i64, i64, i64) = d
        .query_row(
            "SELECT SUM(prompt!=''), SUM(negative!=''), SUM(extra!='') FROM generation_audit",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!((p, n_, e), (0, 0, 0));
    assert_eq!(
        d.query_row::<String, _, _>("SELECT avatar FROM discord_registrations", [], |r| r.get(0))
            .unwrap(),
        ""
    );
    // 旧版漏掉的两处：捐赠的上游 Token 与封禁名单头像
    let (tok, contact, msg): (String, String, String) = d
        .query_row(
            "SELECT token, contact, message FROM key_donations",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(
        (tok, contact, msg),
        (sha16(DONOR_TOKEN), String::new(), String::new())
    );
    assert_eq!(
        d.query_row::<String, _, _>("SELECT avatar FROM discord_bans", [], |r| r.get(0))
            .unwrap(),
        ""
    );

    // 外部账号：ID 换成短哈希（跨账号仍能对上），显示名清空
    let (ext, name): (String, String) = d
        .query_row(
            "SELECT ext_id, name FROM discord_user_connections",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!((ext, name), (sha16("900000000000000088"), String::new()));

    // 分析需要的保留：用量、Discord ID
    assert_eq!(count(&d, "usage_log"), 50);
    assert_eq!(
        d.query_row::<String, _, _>("SELECT discord_id FROM discord_registrations", [], |r| r
            .get(0))
            .unwrap(),
        "900000000000000001"
    );
    assert_eq!(
        d.query_row::<String, _, _>("PRAGMA integrity_check", [], |r| r.get(0))
            .unwrap(),
        "ok"
    );
}

#[test]
fn sanitized_file_has_no_blobs_and_no_plaintext_anywhere() {
    let dir = tempfile::tempdir().unwrap();
    let (out, _) = sanitized_copy(dir.path());
    let d = ro(&out);
    // 所有表所有列都不能再有 BLOB
    for t in table_names(&d) {
        let cols: Vec<String> = {
            let mut stmt = d.prepare(&format!("PRAGMA table_info(\"{t}\")")).unwrap();
            stmt.query_map([], |r| r.get(1))
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap()
        };
        for col in cols {
            let n: i64 = d
                .query_row(
                    &format!("SELECT COUNT(*) FROM \"{t}\" WHERE typeof(\"{col}\")='blob'"),
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(n, 0, "{t}.{col} 里还有 BLOB");
        }
    }
    // VACUUM 之后连空闲页里都不该翻出明文
    assert_eq!(
        d.query_row::<i64, _, _>("PRAGMA freelist_count", [], |r| r.get(0))
            .unwrap(),
        0
    );
    let bytes = std::fs::read(&out).unwrap();
    let has = |needle: &[u8]| bytes.windows(needle.len()).any(|w| w == needle);
    for secret in TOKENS.iter().copied().chain([
        SALT,
        DONOR_TOKEN,
        "synthetic prompt A",
        "synthetic negative",
        "avatar-hash-synthetic",
        "someone@example.invalid",
        "scrypt$synthetic",
    ]) {
        assert!(!has(secret.as_bytes()), "脱敏后的文件里仍能找到 {secret}");
    }
    assert!(has(sha16(TOKENS[0]).as_bytes()), "短哈希应当在");
}

#[test]
fn sanitize_failure_rolls_back_everything() {
    let dir = tempfile::tempdir().unwrap();
    let src = new_db(dir.path());
    seed_source(&src);
    let (out, _) = make(&src, dir.path(), "ro.db");
    rw(&out)
        .execute_batch("DROP TABLE generation_audit")
        .unwrap(); // 缺表 → 脱敏中途失败
    let err = sanitize::sanitize(&out).unwrap_err();
    assert!(format!("{err:#}").contains("generation_audit"), "{err:#}");
    // 整个事务回滚：不会留下「Key 已换哈希、提示词还在」这种半脱敏的文件
    let d = ro(&out);
    let tok: String = d
        .query_row("SELECT token FROM api_keys WHERE id=1", [], |r| r.get(0))
        .unwrap();
    assert_eq!(tok, TOKENS[0]);
}

#[test]
fn old_snapshots_with_req_features_are_dropped() {
    let dir = tempfile::tempdir().unwrap();
    let (out, _) = sanitized_copy(dir.path());
    // 老快照还带着已删除的 req_features 表（toks 是无盐词条哈希，能还原提示词）：脱敏时整张表删掉
    let c = rw(&out);
    c.execute_batch("CREATE TABLE req_features(id INTEGER PRIMARY KEY, fp TEXT, sig TEXT, toks TEXT); INSERT INTO req_features(fp, sig, toks) VALUES ('fp1', 'sig1', 'tokhash1,tokhash2');").unwrap();
    drop(c);
    sanitize::sanitize(&out).unwrap();
    let d = ro(&out);
    let n: i64 = d
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE name='req_features'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        n, 0,
        "req_features 必须整张删掉，不能把词条哈希留在脱敏快照里"
    );
}
