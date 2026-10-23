//! 后台「成员 / 记录」接口用到的裸 SQL（`repo::admin_members` / `repo::admin_records`）。
//! 路由层的逐字节快照在 gate-server 的 `admin_snapshot.rs`，这里只验证 SQL 本身的语义。

use gate_db::Db;
use gate_db::repo::admin_members::{BindOutcome, BindProfile};
use gate_db::repo::admin_records::ActionFilter;
use gate_db::repo::keys::NewKey;
use serde_json::json;

async fn open() -> (Db, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(dir.path().join("t.db")).unwrap();
    (db, dir)
}

async fn key(db: &Db, name: &str) -> i64 {
    let k: NewKey = serde_json::from_value(json!({
        "name": name, "token": format!("nai-{name}"), "daily_images": 10, "monthly_anlas": 0.0,
        "daily_text_tokens": 0, "rpm": 5,
    }))
    .unwrap();
    db.create_key(&k, 1_000.0).await.unwrap()["id"]
        .as_i64()
        .unwrap()
}

async fn sql(db: &Db, s: &str) {
    let s = s.to_string();
    db.write(move |tx| tx.execute_batch(&s)).await.unwrap();
}

#[tokio::test]
async fn tags_invites_and_quota_owner() {
    let (db, _d) = open().await;
    let (a, b) = (key(&db, "邀请-3").await, key(&db, "普通").await);
    db.upsert_key_tag(a, "邀请", "n1", "站长", 5.0)
        .await
        .unwrap();
    db.upsert_key_tag(a, "邀请", "n2", "运维", 6.0)
        .await
        .unwrap();
    db.upsert_key_tag(b, "观察", "", "站长", 4.0).await.unwrap();
    assert_eq!(db.invite_key_ids().await.unwrap(), [a]);
    let tags = db.key_tag_rows().await.unwrap();
    assert_eq!(
        tags.iter()
            .map(|t| (t.key_id, t.tag.as_str(), t.note.as_str(), t.by.as_str()))
            .collect::<Vec<_>>(),
        [(b, "观察", "", "站长"), (a, "邀请", "n2", "运维")],
        "按时间升序；同名标签覆盖备注 / 操作者 / 时间"
    );
    assert!(!db.delete_key_tag(a, "不存在").await.unwrap());
    assert!(db.delete_key_tag(b, "观察").await.unwrap());
    assert_eq!(db.key_names().await.unwrap(), ["邀请-3", "普通"]);
    db.set_quota_auto(b, -1).await.unwrap();
    assert_eq!(db.get_key(b).await.unwrap().unwrap()["quota_auto"], -1);
    assert_eq!(
        db.get_key(a).await.unwrap().unwrap()["quota_auto"],
        1,
        "新建默认交给算法"
    );
    assert_eq!(
        db.key_names_for(&[b, a, 999])
            .await
            .unwrap()
            .into_iter()
            .collect::<std::collections::BTreeMap<_, _>>(),
        [(a, "邀请-3".to_string()), (b, "普通".to_string())].into()
    );
    assert!(db.key_names_for(&[]).await.unwrap().is_empty());
}

/// 站长手填的资料（没读到 Discord）。
fn typed(username: &str, display_name: &str) -> BindProfile {
    BindProfile {
        username: username.into(),
        display_name: display_name.into(),
        ..Default::default()
    }
}

#[tokio::test]
async fn bind_discord_outcomes() {
    let (db, _d) = open().await;
    let (a, b) = (key(&db, "甲").await, key(&db, "乙").await);
    // 新绑定：头像留空、领取时间 = now；同时写 api_keys.discord_id
    match db
        .bind_discord(a, "900000000000000001", &typed("u1", "一号"), 50.0)
        .await
        .unwrap()
    {
        BindOutcome::Bound(Some(r)) => {
            assert_eq!(
                (
                    r.key_id,
                    r.discord_id.as_str(),
                    r.username.as_deref(),
                    r.display_name.as_deref(),
                    r.avatar.as_deref(),
                    r.created_at
                ),
                (
                    a,
                    "900000000000000001",
                    Some("u1"),
                    Some("一号"),
                    Some(""),
                    Some(50.0)
                )
            );
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(
        db.get_key(a).await.unwrap().unwrap()["discord_id"],
        "900000000000000001"
    );
    // 别的 Key 想要同一个账号：被拒，什么都不改
    assert_eq!(
        db.bind_discord(b, "900000000000000001", &typed("x", "x"), 60.0)
            .await
            .unwrap(),
        BindOutcome::Taken(a)
    );
    assert!(db.get_key(b).await.unwrap().unwrap()["discord_id"].is_null());
    // 换绑：同一把 Key，领取时间不变
    match db
        .bind_discord(a, "900000000000000002", &typed("u2", ""), 70.0)
        .await
        .unwrap()
    {
        BindOutcome::Bound(Some(r)) => assert_eq!(
            (r.discord_id.as_str(), r.created_at),
            ("900000000000000002", Some(50.0))
        ),
        other => panic!("{other:?}"),
    }
    // 同一账号重复绑同一把 Key：只更新资料
    match db
        .bind_discord(a, "900000000000000002", &typed("again", "again"), 80.0)
        .await
        .unwrap()
    {
        BindOutcome::Bound(Some(r)) => assert_eq!(r.username.as_deref(), Some("again")),
        other => panic!("{other:?}"),
    }
    assert_eq!(db.registration_rows().await.unwrap().len(), 1);
    // 读到 Discord 资料：写头像、改 Key 名
    let fetched = BindProfile {
        username: "u2".into(),
        display_name: "二号".into(),
        avatar: Some("av2".into()),
        key_name: Some("二号 (@u2)".into()),
    };
    match db
        .bind_discord(a, "900000000000000002", &fetched, 90.0)
        .await
        .unwrap()
    {
        BindOutcome::Bound(Some(r)) => assert_eq!(
            (r.display_name.as_deref(), r.avatar.as_deref()),
            (Some("二号"), Some("av2"))
        ),
        other => panic!("{other:?}"),
    }
    assert_eq!(db.get_key(a).await.unwrap().unwrap()["name"], "二号 (@u2)");
    // 没读到、同一账号：头像保留，Key 名不动
    match db
        .bind_discord(a, "900000000000000002", &typed("u2", "u2"), 95.0)
        .await
        .unwrap()
    {
        BindOutcome::Bound(Some(r)) => assert_eq!(r.avatar.as_deref(), Some("av2")),
        other => panic!("{other:?}"),
    }
    assert_eq!(db.get_key(a).await.unwrap().unwrap()["name"], "二号 (@u2)");
    // 没读到、换了账号：旧头像是上一个人的，清空
    match db
        .bind_discord(a, "900000000000000003", &typed("u3", "u3"), 99.0)
        .await
        .unwrap()
    {
        BindOutcome::Bound(Some(r)) => assert_eq!(r.avatar.as_deref(), Some("")),
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn member_page_aggregates() {
    let (db, _d) = open().await;
    let a = key(&db, "甲").await;
    sql(
        &db,
        &format!(
            "INSERT INTO share_state(key_id, paused_until) VALUES ({a}, 5.0);\
             INSERT INTO counters(key_id, day, images, anlas, text_tokens, requests, v5) VALUES ({a}, '2026-09-22', 3, 1.5, 7, 4, 2);\
             INSERT INTO admin_actions(ts, actor, action, target, detail, ok) VALUES \
               (100.0, 'x', '重置今日额度', 'Key #{a} 甲', '', 1), (50.0, 'x', '重置今日额度', 'Key #{a} 甲', '', 1), \
               (120.0, 'x', '重置今日额度', 'Key #{a} 甲', '', 0), (130.0, 'x', '别的', 'Key #{a}', '', 1);\
             INSERT INTO usage_log(ts, key_id, key_name, kind, model, status, detail) VALUES \
               (10000.0, {a}, '甲', 'image', 'm', 'ok', ''), (10000.0, {a}, '甲', 'upscale', 'm', 'ok', ''), \
               (10000.0, {a}, '甲', 'chat', 'm', 'ok', ''), (13700.0, {a}, '甲', 'vibe_encode', 'm', 'ok', ''), \
               (10001.0, {a}, '甲', 'image', 'm', 'rejected', '画师串 NaN 且 img2img'), \
               (10002.0, {a}, '甲', 'image', 'm', 'error', '角色太多'), (10003.0, {a}, '甲', 'image', 'm', 'rejected', '正常的拒绝'), \
               (5.0, {a}, '甲', 'image', 'm', 'rejected', '请求过于频繁');"
        ),
    )
    .await;
    assert_eq!(db.pause_rows().await.unwrap(), [(a, 5.0)]);
    assert_eq!(
        db.counter_rows("2026-09-22").await.unwrap(),
        [(a, 3, 2, 1.5, 7, 4)]
    );
    assert!(db.counter_rows("2026-09-23").await.unwrap().is_empty());
    assert_eq!(
        db.quota_reset_marks(60.0).await.unwrap(),
        [(100.0, format!("Key #{a} 甲"))],
        "只要成功的、不早于起点的"
    );
    // 生图速率：hour0 = 9999 起每小时一格；chat 不算，vibe_encode 算
    let mut bars = db.hourly_image_counts(9999).await.unwrap();
    bars.sort();
    assert_eq!(bars, [(a, 0, 2), (a, 1, 1)]);
    // 自动标签：先命中先得（img2img 在 NaN 前面）；只看被拒 / 失败；不早于 since
    let pats = [
        ("传了底图", "%img2img%"),
        ("画师串 NaN", "%NaN%"),
        ("角色超 6 个", "%角色太多%"),
        ("请求过快", "%请求过于频繁%"),
    ];
    let mut tags = db.auto_tag_counts(10.0, &pats).await.unwrap();
    tags.sort();
    assert_eq!(
        tags,
        [
            (a, "传了底图".to_string(), 1),
            (a, "角色超 6 个".to_string(), 1)
        ]
    );
    assert_eq!(db.auto_tag_counts(0.0, &pats).await.unwrap().len(), 3);
}

#[tokio::test]
async fn reset_all_keys_spares_invites() {
    let (db, _d) = open().await;
    let (user, invite) = (key(&db, "普通").await, key(&db, "邀请").await);
    sql(
        &db,
        &format!(
            "INSERT INTO key_tags(key_id, tag, ts) VALUES ({invite}, '邀请', 1.0), ({invite}, '观察', 1.0), ({user}, '观察', 1.0), (999, '孤儿', 1.0);\
             INSERT INTO discord_registrations(discord_id, key_id, created_at) VALUES ('1', {user}, 1.0), ('2', {invite}, 1.0);\
             INSERT INTO share_state(key_id, paused_until) VALUES ({user}, 9.0);"
        ),
    )
    .await;
    assert_eq!(db.reset_all_keys().await.unwrap(), (1, 1));
    assert_eq!(db.get_key(user).await.unwrap().unwrap()["enabled"], 0);
    assert_eq!(db.get_key(invite).await.unwrap().unwrap()["enabled"], 1);
    let regs = db.registration_rows().await.unwrap();
    assert_eq!(
        regs.iter()
            .map(|r| r.discord_id.as_str())
            .collect::<Vec<_>>(),
        ["2"]
    );
    let tags: Vec<(i64, String)> = db
        .key_tag_rows()
        .await
        .unwrap()
        .into_iter()
        .map(|t| (t.key_id, t.tag))
        .collect();
    assert!(tags.contains(&(invite, "观察".into())) && !tags.contains(&(user, "观察".into())));
    assert!(
        !tags.contains(&(999, "孤儿".into())),
        "没有对应 Key 的孤儿标签也清掉（旧版如此）"
    );
    assert!(db.pause_rows().await.unwrap().is_empty());
    // 再来一次：没有可失效的了（已经失效的算在内——UPDATE 命中的行数）
    assert_eq!(db.reset_all_keys().await.unwrap(), (1, 1));
}

#[tokio::test]
async fn action_log_filters() {
    let (db, _d) = open().await;
    for (ts, actor, action) in [
        (1.0, "站长", "添加黑名单"),
        (2.0, "系统", "私信成员"),
        (3.0, "Discord:900000000000000001", "成员自助重置"),
        (4.0, "后台 *.*.0.1", "登录后台"),
        (5.0, "后台 *.*.0.1", "添加黑名单"),
    ] {
        db.add_admin_action(actor, action, "", "", true, ts)
            .await
            .unwrap();
    }
    let f = |kind: &str, actions: &[&str]| ActionFilter {
        actor_kind: kind.into(),
        actions: actions.iter().map(|s| s.to_string()).collect(),
    };
    assert_eq!(db.count_actions_filtered(&f("", &[])).await.unwrap(), 5);
    assert_eq!(
        db.count_actions_filtered(&f("admin", &[])).await.unwrap(),
        3
    );
    assert_eq!(
        db.count_actions_filtered(&f("system", &[])).await.unwrap(),
        1
    );
    assert_eq!(
        db.count_actions_filtered(&f("member", &[])).await.unwrap(),
        1
    );
    assert_eq!(
        db.count_actions_filtered(&f("whatever", &[]))
            .await
            .unwrap(),
        5
    );
    assert_eq!(
        db.count_actions_filtered(&f("admin", &["添加黑名单"]))
            .await
            .unwrap(),
        2
    );
    let rows = db
        .list_actions_filtered(&f("", &["添加黑名单", "私信成员"]), 2, 1)
        .await
        .unwrap();
    assert_eq!(
        rows.iter()
            .map(|r| r["ts"].as_f64().unwrap())
            .collect::<Vec<_>>(),
        [2.0, 1.0],
        "id 倒序 + 偏移"
    );
    assert_eq!(db.action_kinds().await.unwrap().len(), 4);
}

#[tokio::test]
async fn audit_queries_filter_by_integer_key_id() {
    let (db, _d) = open().await;
    let a = key(&db, "甲 乙").await;
    sql(
        &db,
        &format!(
            "INSERT INTO generation_audit(ts, key_id, key_name, prompt, negative, extra, image, image_type, image_size) VALUES \
             (10.0, {a}, '甲 乙', 'p1', 'n1', '', X'89504e47', 'image/png', 4), (20.0, {a}, '甲 乙', 'p2', '', '', NULL, '', NULL);"
        ),
    )
    .await;
    for (ids, n) in [(vec![a], 2), (vec![a + 1], 0), (vec![a, a + 1], 2)] {
        let (rows, total) = db.list_audit_ids(24, 0, &ids).await.unwrap();
        assert_eq!((total, rows.len() as i64), (n, n), "{ids:?}");
    }
    let (rows, total) = db.list_audit_ids(24, 0, &[]).await.unwrap();
    assert_eq!(
        (
            total,
            rows[0]["id"].clone(),
            rows[0]["has_image"].clone(),
            rows[1]["has_image"].clone()
        ),
        (2, json!(2), json!(0), json!(1))
    );
    assert_eq!(db.key_name(a).await.unwrap().as_deref(), Some("甲 乙"));
    assert_eq!(db.key_name(a + 1).await.unwrap(), None);
    let imgs = db.audit_images_for_key(a, 1 << 20, 0.0).await.unwrap();
    assert_eq!(imgs.len(), 1);
    assert_eq!(
        (imgs[0].prompt.as_str(), imgs[0].image.as_slice()),
        ("p1", &[0x89, b'P', b'N', b'G'][..])
    );
    assert!(
        db.audit_images_for_key(a + 1, 1 << 20, 0.0)
            .await
            .unwrap()
            .is_empty()
    );
}

/// 浏览器标记登录记录：同账号同浏览器只留一行（刷新 IP / 时间）。
#[tokio::test]
async fn browser_logins_upsert() {
    let (db, _d) = open().await;
    let (b1, b2) = ("a".repeat(32), "b".repeat(32));
    db.record_browser_login("u1", &b1, "1.1.1.1", 100.0)
        .await
        .unwrap();
    db.record_browser_login("u1", &b1, "203.0.113.22", 300.0)
        .await
        .unwrap();
    db.record_browser_login("u2", &b1, "", 150.0).await.unwrap();
    db.record_browser_login("u2", &b2, "", 50.0).await.unwrap();
    let rows = |db: Db| async move {
        db.read(|c| {
            let mut st = c.prepare(
                "SELECT discord_id, browser_id, ip, ts FROM browser_logins \
                 ORDER BY discord_id, browser_id",
            )?;
            let v = st
                .query_map([], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, f64>(3)?,
                    ))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(v)
        })
        .await
        .unwrap()
    };
    let r = rows(db.clone()).await;
    assert_eq!(r.len(), 3);
    assert_eq!(
        r[0],
        ("u1".into(), b1.clone(), "203.0.113.22".into(), 300.0)
    );
}

/// 进服时间 / 成员标记位：只更新已有领取记录；进服时间只往早了改，标记位按位或。
#[tokio::test]
async fn member_join_merges_earliest_and_ors_flags() {
    let (db, _d) = open().await;
    let k = key(&db, "m").await;
    sql(
        &db,
        &format!("INSERT INTO discord_registrations(discord_id, key_id, created_at) VALUES ('7', {k}, 1.0);"),
    )
    .await;
    // 没领过 Key：不存、不报错
    assert!(!db.record_member_join("8", Some(5.0), 1).await.unwrap());
    assert_eq!(db.get_member_join("8").await.unwrap(), None);
    // 老记录：还没读到过
    assert_eq!(db.get_member_join("7").await.unwrap(), Some((None, 0)));
    assert!(db.record_member_join("7", Some(500.0), 2).await.unwrap());
    assert_eq!(
        db.get_member_join("7").await.unwrap(),
        Some((Some(500.0), 2))
    );
    // 更晚的不覆盖；读不到进服时间不清空；标记位累积
    db.record_member_join("7", Some(900.0), 1).await.unwrap();
    db.record_member_join("7", None, 0).await.unwrap();
    assert_eq!(
        db.get_member_join("7").await.unwrap(),
        Some((Some(500.0), 3))
    );
    // 更早的覆盖
    db.record_member_join("7", Some(100.0), 0).await.unwrap();
    assert_eq!(
        db.get_member_join("7").await.unwrap(),
        Some((Some(100.0), 3))
    );
}
