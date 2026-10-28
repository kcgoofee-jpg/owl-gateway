//! 用量日志里的 User-Agent / 来源 IP / 客户端特征：写入、截断、到期清除（站长 10-18 / 10-23；保留天数按后台设置 usage_origin_keep_days，0 = 永久，默认永久）。

use gate_db::Db;
use gate_db::repo::usage::{ClientHints, NewLog, Success};
use rusqlite::Connection;

fn origin_of(path: &std::path::Path, rid: &str) -> (String, String) {
    let c = Connection::open(path).unwrap();
    c.query_row(
        "SELECT client, src FROM usage_log WHERE rid=?",
        [rid],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
    .unwrap()
}

type Hints = [Option<String>; 6];
const NO_HINTS: Hints = [None, None, None, None, None, None];

fn hints_of(path: &std::path::Path, rid: &str) -> Hints {
    let c = Connection::open(path).unwrap();
    c.query_row(
        "SELECT xrw, lang, chua, hfp, bid, origin FROM usage_log WHERE rid=?",
        [rid],
        |r| {
            Ok([
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
            ])
        },
    )
    .unwrap()
}

#[tokio::test]
async fn origin_is_recorded_for_success_and_rejected_and_purged_after_retention() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("gate.db");
    let db = Db::open(&path).unwrap();
    let ua = "DSH-Tavern-Image (Mozilla/5.0 (Linux; Android 16; V2338A))";

    // 成功请求
    db.record_success(
        &Success {
            key_id: 1,
            key_name: "成员".into(),
            kind: "image".into(),
            day: "2026-10-18".into(),
            images: 1,
            rid: "ok1".into(),
            client: ua.into(),
            ip: "203.0.113.7".into(),
            hints: ClientHints {
                xrw: "com.example.tavern".into(),
                lang: "zh-CN".into(),
                chua: "\"Chromium\" | \"Android\" | ?1".into(),
                hfp: "0123456789abcdef".into(),
                bid: "fedcba9876543210".into(),
                origin: "https://app.example".into(),
            },
            ..Default::default()
        },
        1000.0,
    )
    .await
    .unwrap();
    // 被拒的请求
    db.add_log(
        &NewLog {
            key_id: Some(1),
            kind: "image".into(),
            status: "rejected".into(),
            rid: "rej1".into(),
            reason: "key_rate".into(),
            client: "axios/1.18.0".into(),
            ip: "2001:db8::1".into(),
            ..Default::default()
        },
        1001.0,
    )
    .await
    .unwrap();
    // 很长的 UA 会被截断（160 字符）
    db.add_log(
        &NewLog {
            kind: "image".into(),
            status: "error".into(),
            rid: "long1".into(),
            client: "x".repeat(500),
            ip: "198.51.100.1".into(),
            hints: ClientHints {
                xrw: "y".repeat(500),
                hfp: "0123456789abcdef".into(),
                ..Default::default()
            },
            ..Default::default()
        },
        2000.0,
    )
    .await
    .unwrap();

    assert_eq!(
        origin_of(&path, "ok1"),
        (ua.to_string(), "203.0.113.7".to_string())
    );
    assert_eq!(
        origin_of(&path, "rej1"),
        ("axios/1.18.0".to_string(), "2001:db8::1".to_string())
    );
    assert_eq!(origin_of(&path, "long1").0.chars().count(), 160);
    assert_eq!(
        hints_of(&path, "ok1"),
        [
            Some("com.example.tavern".to_string()),
            Some("zh-CN".into()),
            Some("\"Chromium\" | \"Android\" | ?1".into()),
            Some("0123456789abcdef".into()),
            Some("fedcba9876543210".into()),
            Some("https://app.example".into()),
        ]
    );
    assert_eq!(hints_of(&path, "rej1"), NO_HINTS, "空串写成 NULL");
    let long = hints_of(&path, "long1");
    assert_eq!(long[0].as_ref().unwrap().chars().count(), 80);
    assert_eq!(long[1], None);

    // 到期清除：只清 before 之前的，日志本身保留；再清一次没有可清的了
    assert_eq!(db.purge_usage_origin(1500.0).await.unwrap(), 2);
    assert_eq!(origin_of(&path, "ok1"), (String::new(), String::new()));
    assert_eq!(origin_of(&path, "rej1"), (String::new(), String::new()));
    assert_eq!(hints_of(&path, "ok1"), NO_HINTS, "客户端特征一起清");
    assert_eq!(origin_of(&path, "long1").1, "198.51.100.1", "窗口内的不动");
    assert!(hints_of(&path, "long1")[3].is_some(), "窗口内的不动");
    assert_eq!(db.purge_usage_origin(1500.0).await.unwrap(), 0);
    let rows: i64 = Connection::open(&path)
        .unwrap()
        .query_row("SELECT COUNT(*) FROM usage_log", [], |r| r.get(0))
        .unwrap();
    assert_eq!(rows, 3, "清的只是 UA / IP，日志行不删");
}

#[tokio::test]
async fn web_profile_is_shown_for_members_without_a_key_and_purged_after_30_days() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(dir.path().join("gate.db")).unwrap();
    assert_eq!(db.discord_profile_row("42").await.unwrap(), None);
    db.upsert_web_profile("42", "someone", "某人", "abc123", 1000.0)
        .await
        .unwrap();
    assert_eq!(
        db.discord_profile_row("42").await.unwrap(),
        Some((
            Some("someone".into()),
            Some("某人".into()),
            Some("abc123".into())
        ))
    );
    // 再次登录刷新
    db.upsert_web_profile("42", "someone", "新名字", "def456", 2000.0)
        .await
        .unwrap();
    let row = db.discord_profile_row("42").await.unwrap().unwrap();
    assert_eq!(row.1.as_deref(), Some("新名字"));
}

#[tokio::test]
async fn reset_scope_resets_v45_always_and_v5_anlas_only_when_selected() {
    use gate_db::repo::counters::ResetScope;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("gate.db");
    let db = Db::open(&path).unwrap();
    // 先放一行计数：V4.5 免费图 7 张、V5 3 张、Anlas 9.0
    db.write(|tx| {
        tx.execute(
            "INSERT INTO api_keys(id, name, token, enabled, created_at) \
             VALUES(1,'a','nai-a-0123456789abcdef',1,1.0)",
            [],
        )?;
        tx.execute(
            "INSERT INTO counters(key_id, day, images, legacy_free_images, anlas, v5, requests) \
             VALUES(1,'2030-03-15',10,7,9.0,3,10)",
            [],
        )?;
        Ok(())
    })
    .await
    .unwrap();
    let left = |db: &Db| {
        let db = db.clone();
        async move {
            let c = db.get_counter(1, "2030-03-15").await.unwrap();
            (c.legacy_free_images, c.v5, c.anlas)
        }
    };
    assert_eq!(left(&db).await, (7, 3, 9.0));
    // 默认只重置 V4.5：V5 和 Anlas 的用量还在
    db.reset_daily_image_quota(1, "2030-03-15", ResetScope::V45_ONLY)
        .await
        .unwrap();
    assert_eq!(left(&db).await, (0, 3, 9.0));
    // 之后又用了一些，再只勾 V5：V5 清零，Anlas 仍保持原来的基线（没被清），V4.5 同样按本次重置
    db.write(|tx| {
        tx.execute(
            "UPDATE counters SET legacy_free_images=legacy_free_images+2, v5=v5+1, anlas=anlas+1.0 \
             WHERE key_id=1 AND day='2030-03-15'",
            [],
        )?;
        Ok(())
    })
    .await
    .unwrap();
    assert_eq!(left(&db).await, (2, 4, 10.0));
    db.reset_daily_image_quota(
        1,
        "2030-03-15",
        ResetScope {
            v5: true,
            anlas: false,
        },
    )
    .await
    .unwrap();
    assert_eq!(left(&db).await, (0, 0, 10.0));
    // 全选
    db.reset_daily_image_quota(1, "2030-03-15", ResetScope::ALL)
        .await
        .unwrap();
    assert_eq!(left(&db).await, (0, 0, 0.0));
    // 存库写法互转
    assert_eq!(ResetScope::V45_ONLY.canonical(), "v45");
    assert_eq!(ResetScope::ALL.canonical(), "v45,v5,anlas");
    assert_eq!(
        ResetScope::parse("v45,anlas"),
        ResetScope {
            v5: false,
            anlas: true
        }
    );
    assert_eq!(ResetScope::parse(""), ResetScope::V45_ONLY);
}

#[tokio::test]
async fn deleted_key_keeps_discord_owner_for_14_day_log_tracing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("gate.db");
    let db = Db::open(&path).unwrap();
    db.write(|tx| {
        // 1 号：Key 上有 discord_id；2 号：只有领取记录（2.x 时期的 Key）；3 号：两边都没有
        tx.execute_batch(
            "INSERT INTO api_keys(id, name, token, enabled, created_at, discord_id) VALUES \
               (1,'甲','nai-a-1',1,1.0,'100000000000000001'), \
               (2,'乙','nai-a-2',1,1.0,NULL), \
               (3,'丙','nai-a-3',1,1.0,NULL); \
             INSERT INTO discord_registrations(discord_id, key_id, created_at) VALUES ('100000000000000002', 2, 1.0);",
        )?;
        Ok(())
    })
    .await
    .unwrap();
    db.delete_key(1, 1000.0).await.unwrap();
    db.revoke_registration("100000000000000002", 2000.0)
        .await
        .unwrap();
    db.delete_key(3, 3000.0).await.unwrap();
    db.delete_key(99, 3000.0).await.unwrap(); // 不存在的 Key：什么也不记
    let owners = || {
        let c = Connection::open(&path).unwrap();
        let mut st = c
            .prepare("SELECT key_id, discord_id, name, deleted_at FROM deleted_key_owners ORDER BY key_id")
            .unwrap();
        st.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, f64>(3)?,
            ))
        })
        .unwrap()
        .map(Result::unwrap)
        .collect::<Vec<_>>()
    };
    assert_eq!(
        owners(),
        vec![
            (1, Some("100000000000000001".into()), "甲".into(), 1000.0),
            (2, Some("100000000000000002".into()), "乙".into(), 2000.0),
            (3, None, "丙".into(), 3000.0),
        ]
    );
    // Key 本身确实删掉了
    let left: i64 = Connection::open(&path)
        .unwrap()
        .query_row("SELECT COUNT(*) FROM api_keys", [], |r| r.get(0))
        .unwrap();
    assert_eq!(left, 0);
}
