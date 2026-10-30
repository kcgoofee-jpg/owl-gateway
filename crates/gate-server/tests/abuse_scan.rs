//! 小号扫描：两个账号同 IP、多次换号续图，又在同一台设备上网页登录过 →
//! 待审队列里出现「行为相似」和「同设备」；登录时交了服务端不认得的指纹 → 「指纹异常」；只同 IP 的室友不进队列。

mod common;
use common::{member_key, query, run_job, setup};
use serde_json::json;

#[tokio::test]
async fn scan_writes_review_signals() {
    let e = setup(&[]).await;
    let st = &e.state;
    let (da, db_, dc) = (
        "900000000000000901",
        "900000000000000902",
        "900000000000000903",
    );
    let a = member_key(st, 1, &[("discord_id", json!(da))]).await;
    let b = member_key(st, 2, &[("discord_id", json!(db_))]).await;
    let c = member_key(st, 3, &[("discord_id", json!(dc))]).await;
    let now = st.now();
    let mut sql = Vec::new();
    for i in 0..6 {
        let t = now - 3600.0 + f64::from(i) * 30.0;
        let ip = if i % 2 == 0 {
            "203.0.113.7"
        } else {
            "203.0.113.8"
        };
        for (k, dt) in [(a, 0.0), (b, 10.0), (c, 20.0)] {
            sql.push(format!(
                "INSERT INTO usage_log(ts, key_id, key_name, kind, model, status, images, detail, client, src, reason) \
                 VALUES ({}, {k}, 'x', 'image', 'm', 'ok', 1, '', '', '{ip}', '')",
                t + dt
            ));
        }
    }
    // a 多次撞额度后 b 在同网段续图；c 只是同 IP 的室友。
    for i in 0..3 {
        let t = now - 200.0 + f64::from(i) * 50.0;
        sql.push(format!("INSERT INTO usage_log(ts,key_id,kind,status,src,reason) VALUES ({t},{a},'image','rejected','203.0.113.7','member_quota'),({},{b},'image','ok','203.0.113.8','')",t+10.0));
    }
    sql.push(format!(
        "INSERT INTO device_logins(discord_id, device_id, ip, ts) VALUES ('{da}', 'dev1', '1.1.1.1', {now}), ('{db_}', 'dev1', '1.1.1.1', {now})"
    ));
    sql.push(
        "INSERT INTO device_fingerprints(device_id, data, ts) VALUES ('dev1', '{\"gpuRenderer\":\"ANGLE (NVIDIA RTX 3060)\"}', 0)"
            .to_string(),
    );
    sql.push(format!(
        "INSERT INTO web_logins(discord_id, first_ts, last_ts, fp, ip) VALUES ('{dc}', {now}, {now}, 'unknown', '')"
    ));
    // c 和 a 在同一个浏览器里登录过（浏览器标记相同）
    sql.push(format!(
        "INSERT INTO browser_logins(discord_id, browser_id, ip, ts) VALUES ('{da}', 'b0b0', '', {now}), ('{dc}', 'b0b0', '', {now})"
    ));
    st.db.run_script(sql).await.unwrap();

    let summary = run_job(st, "abuse_scan").await.unwrap();
    assert!(summary.contains("behavior 1"), "{summary}");

    // 设备线索从「同设备 / 同浏览器」两条合并成一条 device_cluster（10-31）
    let rows = query(
        &st.db,
        "SELECT kind, subject, key_ids, score FROM abuse_signals WHERE kind!='ip_switch' ORDER BY kind",
    )
    .await;
    let kinds: Vec<&str> = rows.iter().map(|r| r["kind"].as_str().unwrap()).collect();
    assert_eq!(
        kinds,
        ["behavior", "device_cluster", "fp_unknown"],
        "{rows:?}"
    );
    let pair = format!("{da},{db_}");
    assert_eq!(rows[0]["subject"], pair);
    assert!(rows[0]["score"].as_f64().unwrap() >= 40.0);
    // 三个人都在这台机器上：一条簇，账号和 Key 都列全
    assert_eq!(rows[1]["subject"], format!("{da},{db_},{dc}"));
    assert_eq!(rows[1]["key_ids"], format!("{a},{b},{c}"));
    assert!(rows[1]["score"].as_f64().unwrap() >= 80.0);
    assert_eq!(rows[2]["subject"], dc);

    // 再跑一次：同一组证据只更新，不重复进队列；不再出现的待审信号标成过期（记录留着）
    st.db
        .run_script(vec![format!(
            "INSERT INTO abuse_signals(kind, subject, key_ids, score, detail, status, first_ts, last_ts) \
             VALUES ('key_sharing', 'key:999', '999', 40, '{{}}', 'open', {0}, {0})",
            now - 100.0
        )])
        .await
        .unwrap();
    run_job(st, "abuse_scan").await.unwrap();
    let n = query(
        &st.db,
        "SELECT status, COUNT(*) AS n FROM abuse_signals WHERE kind!='ip_switch' GROUP BY status ORDER BY status",
    )
    .await;
    assert_eq!(n[0]["status"], "expired");
    assert_eq!(n[0]["n"], 1);
    assert_eq!(n[1]["status"], "open");
    // 行为 1 + 设备簇 1 + 指纹异常 1（以前是「同设备 / 同浏览器」两条）
    assert_eq!(n[1]["n"], 3);
}

/// 设备簇里只有一个账号持有 Key（其余没有）：分数减半并带「其余账号：无 Key」。
#[tokio::test]
async fn solo_key_holder_signals_are_scored_lower() {
    let e = setup(&[]).await;
    let st = &e.state;
    let (keyed, bare) = ("900000000000000911", "900000000000000913");
    member_key(st, 1, &[("discord_id", json!(keyed))]).await;
    let now = st.now();
    st.db
        .run_script(vec![
            format!("INSERT INTO browser_logins(discord_id, browser_id, ip, ts) VALUES ('{keyed}', 'b1', '', {now}), ('{bare}', 'b1', '', {now})"),
            format!("INSERT INTO device_logins(discord_id, device_id, ip, ts) VALUES ('{keyed}', 'dev1', '', {now}), ('{bare}', 'dev1', '', {now})"),
            "INSERT INTO device_fingerprints(device_id, data, ts) VALUES ('dev1', '{\"gpuRenderer\":\"ANGLE (AMD Radeon)\"}', 0)".to_string(),
        ])
        .await
        .unwrap();
    run_job(st, "abuse_scan").await.unwrap();

    let rows = query(
        &st.db,
        "SELECT kind, subject, score, detail FROM abuse_signals ORDER BY kind, subject",
    )
    .await;
    assert_eq!(rows.len(), 1, "{rows:?}");
    let solo = &rows[0];
    assert_eq!(solo["kind"], "device_cluster");
    assert_eq!(solo["subject"], format!("{keyed},{bare}"));
    // 同浏览器 80 + 同设备 60、两种关联 +10 → 90；只有一个人持有 Key → 减半
    assert_eq!(solo["score"], 45.0);
    assert!(
        solo["detail"].as_str().unwrap().contains("其余账号"),
        "{solo:?}"
    );
}

/// 涉及的账号一个 Key 都没有：不写信号（「无 Key」卡片是噪音）。
#[tokio::test]
async fn signals_without_any_key_holder_are_not_written() {
    let e = setup(&[]).await;
    let st = &e.state;
    let (x, y) = ("900000000000000921", "900000000000000922");
    let now = st.now();
    st.db
        .run_script(vec![
            format!("INSERT INTO browser_logins(discord_id, browser_id, ip, ts) VALUES ('{x}', 'b9', '', {now}), ('{y}', 'b9', '', {now})"),
            format!("INSERT INTO device_logins(discord_id, device_id, ip, ts) VALUES ('{x}', 'dev9', '', {now}), ('{y}', 'dev9', '', {now})"),
        ])
        .await
        .unwrap();
    run_job(st, "abuse_scan").await.unwrap();
    let rows = query(&st.db, "SELECT kind, subject FROM abuse_signals").await;
    assert!(rows.is_empty(), "{rows:?}");
}

/// 站长 10-23：关联扫描的六项信号（同 IP 换号 / 同时异地 / 同参考图 / 同外部账号 / 服务器重合 / 电量相同）都写进待审队列；
/// 没有启用 Key 的账号不参与。
#[tokio::test]
async fn linkage_signals_are_written() {
    let e = setup(&[]).await;
    let st = &e.state;
    let (da, db_, dn) = (
        "900000000000000921",
        "900000000000000922",
        "900000000000000923", // 没有 Key
    );
    let a = member_key(st, 1, &[("discord_id", json!(da))]).await;
    let b = member_key(st, 2, &[("discord_id", json!(db_))]).await;
    let now = st.now();
    let log = |t: f64, k: i64, ip: &str| {
        format!(
            "INSERT INTO usage_log(ts, key_id, key_name, kind, model, status, images, detail, client, src, reason) \
             VALUES ({t}, {k}, 'x', 'image', 'm', 'ok', 1, '', '', '{ip}', '')"
        )
    };
    let mut sql = vec![
        // 同 IP 换号：a → b 在 2 分钟内
        log(now - 3000.0, a, "198.51.100.5"),
        log(now - 2880.0, b, "198.51.100.5"),
    ];
    // 同时异地：Key a 三次在 60 秒内跳到另一个 /16
    for i in 0..3 {
        let t = now - 1000.0 + f64::from(i) * 200.0;
        sql.push(log(t, a, "203.0.113.9"));
        sql.push(log(t + 20.0, a, "192.0.2.9"));
    }
    sql.extend([
        // 同参考图：完全相同的一张；另一张几乎一样（dHash 差 2 位）
        format!("INSERT INTO ref_images(rid, key_id, ts, kind, sha256, dhash) VALUES ('r1', {a}, {now}, 'vibe', 'aaaa', '00ff00ff00ff00ff'), ('r2', {b}, {now}, 'vibe', 'aaaa', '00ff00ff00ff00ff')"),
        format!("INSERT INTO ref_images(rid, key_id, ts, kind, sha256, dhash) VALUES ('r3', {a}, {now}, 'img2img', 'bbbb', '123456789abcdef0'), ('r4', {b}, {now}, 'img2img', 'cccc', '123456789abcdef3')"),
        // 同外部账号：a、b 和没有 Key 的 n 都绑了同一个 Steam
        format!("INSERT INTO discord_user_connections(discord_id, type, ext_id, name, verified, first_ts, last_ts) VALUES ('{da}', 'steam', 'S1', '', 1, {now}, {now}), ('{db_}', 'steam', 'S1', '', 1, {now}, {now}), ('{dn}', 'steam', 'S1', '', 1, {now}, {now})"),
        // 服务器重合：两个冷门服务器 + 一个大服务器（本站记录里另有 3 个账号在，共 5 个，不算冷门）
        "INSERT INTO discord_user_guilds(discord_id, guild_id, name, first_ts, last_ts) VALUES ('900000000000000931', 'owl', '猫头鹰', 0, 0), ('900000000000000932', 'owl', '猫头鹰', 0, 0), ('900000000000000933', 'owl', '猫头鹰', 0, 0)".to_string(),
        format!("INSERT INTO discord_user_guilds(discord_id, guild_id, name, first_ts, last_ts) VALUES ('{da}', 'g1', '小圈子', {now}, {now}), ('{da}', 'g2', '', {now}, {now}), ('{da}', 'owl', '猫头鹰', {now}, {now}), ('{db_}', 'g1', '小圈子', {now}, {now}), ('{db_}', 'g2', '', {now}, {now}), ('{db_}', 'owl', '猫头鹰', {now}, {now})"),
        // 电量相同：10 分钟内都是 43%、没在充电
        format!("INSERT INTO device_battery(discord_id, ts, level, charging) VALUES ('{da}', {}, 0.43, 0), ('{db_}', {}, 0.43, 0), ('{dn}', {}, 0.43, 0)", now - 900.0, now - 300.0, now - 200.0),
    ]);
    st.db.run_script(sql).await.unwrap();
    run_job(st, "abuse_scan").await.unwrap();
    let rows = query(
        &st.db,
        "SELECT kind, subject, key_ids, score, detail FROM abuse_signals \
         WHERE kind IN ('ip_switch','key_concurrent','same_ref_image','same_connection','guild_overlap','same_battery') ORDER BY kind",
    )
    .await;
    let pair = format!("{da},{db_}");
    let got: Vec<(String, String, f64)> = rows
        .iter()
        .map(|r| {
            (
                r["kind"].as_str().unwrap().to_string(),
                r["subject"].as_str().unwrap().to_string(),
                r["score"].as_f64().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        got,
        [
            ("guild_overlap".into(), pair.clone(), 50.0),
            ("key_concurrent".into(), format!("key:{a}"), 40.0),
            ("same_battery".into(), pair.clone(), 50.0),
            ("same_connection".into(), pair.clone(), 100.0),
            ("same_ref_image".into(), pair.clone(), 75.0),
        ],
        "{rows:?}"
    );
    let ref_row = rows
        .iter()
        .find(|r| r["kind"] == "same_ref_image")
        .expect("same_ref_image missing");
    let detail: serde_json::Value =
        serde_json::from_str(ref_row["detail"].as_str().unwrap()).unwrap();
    assert!(
        detail["summary"]
            .as_str()
            .unwrap()
            .contains("完全相同 1 张，几乎一样 1 张"),
        "{detail}"
    );
    assert_eq!(
        rows[4]["key_ids"],
        format!("{a},{b}"),
        "没有 Key 的账号不进"
    );
}

/// 匿名请求（usage_log.key_id 为 NULL，比如没绑 Key 的调用）不能让整轮扫描失败，
/// 也不能进任何信号；有归属的行照常评分。（快照曾直接按 i64 读 key_id，NULL 会弄挂整轮 SQL。）
#[tokio::test]
async fn anonymous_usage_rows_do_not_break_the_scan() {
    let e = setup(&[]).await;
    let st = &e.state;
    let (da, db_) = ("900000000000000951", "900000000000000952");
    let a = member_key(st, 1, &[("discord_id", json!(da))]).await;
    let b = member_key(st, 2, &[("discord_id", json!(db_))]).await;
    let now = st.now();
    let mut sql = Vec::new();
    // 匿名行：第三方 origin 也不该触发 foreign_origin（按 key_id 过滤）
    for i in 0..3 {
        sql.push(format!(
            "INSERT INTO usage_log(ts, key_name, kind, status, images, src, origin) \
             VALUES ({}, '', 'image', 'ok', 1, '203.0.113.10', 'https://draw.example')",
            now - f64::from(i) * 60.0
        ));
    }
    // 有归属的行：两个账号同 IP（现在只统计、不出信号，验证匿名行不影响统计）
    sql.push(format!(
        "INSERT INTO usage_log(ts, key_id, key_name, kind, model, status, images, src, reason) \
         VALUES ({}, {a}, 'x', 'image', 'm', 'ok', 1, '198.51.100.5', '')",
        now - 3000.0
    ));
    sql.push(format!(
        "INSERT INTO usage_log(ts, key_id, key_name, kind, model, status, images, src, reason) \
         VALUES ({}, {b}, 'x', 'image', 'm', 'ok', 1, '198.51.100.5', '')",
        now - 2880.0
    ));
    st.db.run_script(sql).await.unwrap();

    // 整轮扫描成功（不因匿名行报错），有归属的信号照常写出
    run_job(st, "abuse_scan").await.unwrap();
    let rows = query(
        &st.db,
        "SELECT kind, subject, key_ids FROM abuse_signals ORDER BY kind",
    )
    .await;
    assert!(
        rows.is_empty(),
        "匿名行和纯网络线索都不该进待审队列：{rows:?}"
    );
    // 匿名行不进任何信号：foreign_origin 只会按 Key 记，匿名 origin 不产生行
    assert!(
        !rows.iter().any(|r| r["kind"] == "foreign_origin"),
        "{rows:?}"
    );
}

/// 一把 Key 的请求带着第三方网站的 Origin（Key 被填进了别的网站，会流经对方服务器）→
/// 「foreign_origin」信号（分数 30，只提醒不封）；已知域（本站 / novelai.net / 社区公益站，精确匹配）的不出信号。
#[tokio::test]
async fn foreign_origin_requests_are_flagged() {
    let e = setup(&[("SITE_URL", "https://gate.test")]).await;
    let st = &e.state;
    let (da, db_) = ("900000000000000941", "900000000000000942");
    let leaked = member_key(st, 1, &[("discord_id", json!(da))]).await;
    let clean = member_key(st, 2, &[("discord_id", json!(db_))]).await;
    let now = st.now();
    let log = |t: f64, k: i64, origin: &str| {
        format!(
            "INSERT INTO usage_log(ts, key_id, key_name, kind, model, status, images, detail, client, src, reason, origin) \
             VALUES ({t}, {k}, 'x', 'image', 'm', 'ok', 1, '', '', '', '', '{origin}')"
        )
    };
    st.db
        .run_script(vec![
            // 已知域（本站 + 官方站 + 站长确认的社区公益站）不算；名单之外的第三方网站算
            log(now - 3600.0, leaked, "https://gate.test"),
            log(now - 3500.0, leaked, "https://novelai.net"),
            log(now - 3400.0, leaked, "https://novelai-draw.minijiu.cfd"),
            log(now - 3000.0, leaked, "https://draw.example"),
            log(now - 2000.0, leaked, "https://draw.example"),
            log(now - 1500.0, clean, "https://gate.test"),
        ])
        .await
        .unwrap();
    let summary = run_job(st, "abuse_scan").await.unwrap();
    assert!(summary.contains("foreign_origin 1"), "{summary}");
    let rows = query(
        &st.db,
        "SELECT subject, key_ids, score, detail FROM abuse_signals WHERE kind='foreign_origin'",
    )
    .await;
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0]["subject"], format!("key:{leaked}"));
    assert_eq!(rows[0]["key_ids"], leaked.to_string());
    assert_eq!(rows[0]["score"], 30.0);
    let detail: serde_json::Value =
        serde_json::from_str(rows[0]["detail"].as_str().unwrap()).unwrap();
    assert_eq!(
        detail["summary"],
        "这把 Key 的请求来自第三方网站（Key 会流经对方服务器，有泄露风险）"
    );
    let item = |k: &str| {
        detail["evidence"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["k"] == k)
            .map(|e| e["v"].as_str().unwrap().to_string())
            .unwrap_or_else(|| panic!("没有 {k}：{detail}"))
    };
    assert_eq!(item("域名"), "https://draw.example ×2");
    assert_eq!(item("Discord"), da);
    assert!(item("时间").contains("~"), "{detail}");
}

#[tokio::test]
async fn shared_reference_images_need_a_shared_device_once_three_keys_use_them() {
    // 线上 10-31：3 张图被 4 个账号各上传几百上千次，设备一台都不重合（素材在群里流传）。
    // 这种不该进待复核；但其中两个账号要是在同一台机器上登录过，那就是真信号。
    let e = setup(&[]).await;
    let st = &e.state;
    let (da, db_, dc) = (
        "900000000000000911",
        "900000000000000912",
        "900000000000000913",
    );
    let a = member_key(st, 1, &[("discord_id", json!(da))]).await;
    let b = member_key(st, 2, &[("discord_id", json!(db_))]).await;
    let c = member_key(st, 3, &[("discord_id", json!(dc))]).await;
    let now = st.now();
    let sql = [format!(
        "INSERT INTO ref_images(rid, key_id, ts, kind, sha256, dhash) VALUES \
         ('r1', {a}, {now}, 'vibe', 'aaaa', '00ff00ff00ff00ff'), \
         ('r2', {b}, {now}, 'vibe', 'aaaa', '00ff00ff00ff00ff'), \
         ('r3', {c}, {now}, 'vibe', 'aaaa', '00ff00ff00ff00ff')"
    )];
    st.db.run_script(sql.to_vec()).await.unwrap();
    run_job(st, "abuse_scan").await.unwrap();
    let n = query(
        &st.db,
        "SELECT count(*) AS n FROM abuse_signals WHERE kind = 'same_ref_image'",
    )
    .await;
    assert_eq!(
        n[0]["n"].as_i64().unwrap(),
        0,
        "三个账号传同一张图，没有共同设备"
    );

    // a、b 在同一台机器上网页登录过 → 只出这一对
    st.db
        .run_script(vec![format!(
            "INSERT INTO device_logins(discord_id, device_id, ip, ts) VALUES \
             ('{da}', 'dev-shared', '', {now}), ('{db_}', 'dev-shared', '', {now})"
        )])
        .await
        .unwrap();
    run_job(st, "abuse_scan").await.unwrap();
    let rows = query(
        &st.db,
        "SELECT subject, score FROM abuse_signals WHERE kind = 'same_ref_image'",
    )
    .await;
    assert_eq!(rows.len(), 1, "{rows:?}");
    let pair = format!("{da},{db_}");
    assert_eq!(rows[0]["subject"].as_str().unwrap(), pair);
    assert_eq!(rows[0]["score"].as_f64().unwrap(), 70.0);
}
