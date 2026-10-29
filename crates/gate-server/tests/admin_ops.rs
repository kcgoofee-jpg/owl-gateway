//! 后台「运营 / 监控」接口：预算、排队与冷却、账号保护、领 Key / 功能 / 记录开关、公告与告警、
//! Discord 机器人、上游 Token 增删改、Anlas 核对、总览与监控。
//!
//! 验证原生类型校验、设置持久化、操作日志和实际请求行为。
//! 公开仓库：全部是合成数据；不会连 Discord / NovelAI（上游是本地假服务）。

mod common;
use common::*;
use gate_db::repo::usage::{NewLog, Success};
use gate_testkit::{MockUpstream, Reply};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;

// ------------------------------------------------------------------ 小工具

/// `site_settings` 全部行。
async fn settings(e: &Env) -> BTreeMap<String, String> {
    common::query(&e.state.db, "SELECT key, value FROM site_settings")
        .await
        .into_iter()
        .map(|r| {
            (
                r["key"].as_str().unwrap().to_string(),
                r["value"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// 只留下与 `before` 不同（新增 / 改动）的行。
fn changed(
    before: &BTreeMap<String, String>,
    after: &BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    after
        .iter()
        .filter(|(k, v)| before.get(*k) != Some(*v))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

fn map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

/// 操作日志（旧→新）：(操作者, 操作, 对象, 详情, 成功?)。
async fn actions(e: &Env) -> Vec<(String, String, String, String, bool)> {
    let mut rows = common::admin_actions(&e.state.db, 200).await;
    rows.reverse();
    rows.iter()
        .map(|r| {
            let s = |k: &str| r[k].as_str().unwrap_or_default().to_string();
            (
                s("actor"),
                s("action"),
                s("target"),
                s("detail"),
                r["ok"].as_i64() == Some(1),
            )
        })
        .collect()
}

// ------------------------------------------------------------------ 预算

const SETTINGS_DEFAULT: &str = r#"{"budgets":{"daily_v5":150,"daily_anlas":200,"monthly_anlas":10000.0},"v5_paused":false,"v5_medium_mode":"off"}"#;

#[tokio::test]
async fn settings_get_put_and_stored_strings() {
    let e = setup(&[]).await;
    let api = Admin::login_bare(&e.state).await;
    let (_, initial) = api.json("GET", "/settings", None).await;
    assert_eq!(
        initial,
        serde_json::from_str::<Value>(SETTINGS_DEFAULT).unwrap()
    );
    let before = settings(&e).await;
    for bad in [
        json!({}),
        json!({"budgets":{"daily_v5":1}}),
        json!({"global_daily_v5":200}),
    ] {
        assert_eq!(api.json("PUT", "/settings", Some(bad)).await.0, 422);
    }
    assert_eq!(settings(&e).await, before);
    let mut proposed = initial;
    proposed["budgets"] = json!({"daily_v5":200,"daily_anlas":350,"monthly_anlas":5000.0});
    assert_eq!(
        api.json("PUT", "/settings", Some(proposed.clone())).await,
        (200, proposed.clone())
    );
    assert_eq!(api.get_ok("/settings").await, proposed);
    assert_eq!(
        serde_json::from_str::<Value>(&settings(&e).await["runtime_budgets"]).unwrap(),
        proposed["budgets"]
    );
    let frozen = settings(&e).await;
    for bad in [json!(-1), json!(1.5), json!("20"), json!(true), json!(null)] {
        let mut invalid = proposed.clone();
        invalid["budgets"]["daily_v5"] = bad;
        invalid["v5_paused"] = json!(true);
        assert_eq!(api.json("PUT", "/settings", Some(invalid)).await.0, 422);
    }
    assert_eq!(settings(&e).await, frozen);
}

#[tokio::test]
async fn maintenance_switch_pauses_public_routes_and_keeps_admin_and_health_available() {
    let e = setup(&[]).await;
    let admin = Admin::login_bare(&e.state).await;
    assert_eq!(admin.get_ok("/ops").await["maintenance"]["enabled"], false);

    for invalid in [
        json!({}),
        json!({"enabled": "true"}),
        json!({"enabled": true, "extra": 1}),
    ] {
        assert_eq!(admin.put("/ops/maintenance", invalid).await.status, 422);
    }
    assert_eq!(common::setting(&e.state, "maintenance_mode").await, None);

    assert_eq!(
        admin
            .put("/ops/maintenance", json!({"enabled": true}))
            .await
            .json(),
        json!({"enabled": true})
    );
    assert_eq!(
        common::setting(&e.state, "maintenance_mode")
            .await
            .as_deref(),
        Some("1")
    );

    let page = common::send(&e.state, "GET", "/", None, None, &[("accept", "text/html")]).await;
    assert_eq!(page.status, 200);
    assert!(page.text().contains("维护中"));
    assert_eq!(page.headers["cache-control"], "no-store");

    let api = common::send(
        &e.state,
        "GET",
        "/public/live",
        None,
        None,
        &[("accept", "application/json")],
    )
    .await;
    assert_eq!(api.status, 503);
    assert_eq!(api.json()["detail"], "本站维护中，服务暂时暂停");
    assert_eq!(api.headers["retry-after"], "30");
    assert_eq!(
        common::send(&e.state, "GET", "/healthz", None, None, &[])
            .await
            .status,
        200
    );
    assert!(
        admin.get_ok("/ops").await["maintenance"]["enabled"]
            .as_bool()
            .unwrap()
    );

    let restarted = e.restart().await;
    let after_restart = Admin::login_bare(&restarted).await;
    assert_eq!(
        after_restart.get_ok("/ops").await["maintenance"]["enabled"],
        true
    );
    assert_eq!(
        common::send(&restarted, "GET", "/public/live", None, None, &[])
            .await
            .status,
        503
    );
    assert_eq!(
        after_restart
            .put("/ops/maintenance", json!({"enabled": false}))
            .await
            .json(),
        json!({"enabled": false})
    );
    assert_eq!(
        common::send(&restarted, "GET", "/public/live", None, None, &[])
            .await
            .status,
        200
    );
    restarted
        .db
        .set_setting("maintenance_mode", "corrupt")
        .await
        .unwrap();
    assert_eq!(
        common::send(&restarted, "GET", "/public/live", None, None, &[])
            .await
            .status,
        503,
        "损坏的维护开关不能意外恢复公共流量"
    );
}

#[tokio::test]
async fn retired_identity_lists_cannot_be_written_as_runtime_settings() {
    let env = setup(&[]).await;
    let api = Admin::login_bare(&env.state).await;
    let (status,_)=api.json("PUT","/settings",Some(json!({"v5_access_mode":"restricted","gateway_owner_discord_ids":["900000000000000051"]}))).await;
    assert_eq!(status, 422);
    assert!(
        env.state
            .db
            .get_setting("gateway_owner_discord_ids")
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn settings_patch_never_rewrites_an_omitted_emergency_pause() {
    let e = setup(&[]).await;
    let api = Admin::login_bare(&e.state).await;
    common::set_setting(&e, "v5_paused", "1").await;
    let mut full = api.get_ok("/settings").await;
    assert_eq!(
        api.json(
            "PUT",
            "/settings",
            Some(json!({"budgets":{"daily_anlas":63}}))
        )
        .await
        .0,
        422
    );
    assert_eq!(settings(&e).await["v5_paused"], "1");
    full["budgets"]["daily_anlas"] = json!(63);
    assert_eq!(api.json("PUT", "/settings", Some(full)).await.0, 200);
    assert_eq!(settings(&e).await["v5_paused"], "1");
}

#[tokio::test]
async fn global_daily_anlas_pool_is_editable() {
    let e = setup(&[]).await;
    let api = Admin::login_bare(&e.state).await;
    for limit in [350, 120, 0, 12, 9999] {
        let mut full = api.get_ok("/settings").await;
        full["budgets"]["daily_anlas"] = json!(limit);
        assert_eq!(
            api.json("PUT", "/settings", Some(full.clone())).await,
            (200, full)
        );
        assert_eq!(
            e.state.db.shared_budgets().await.unwrap().daily_anlas,
            limit
        );
    }
}

#[tokio::test]
async fn canonical_global_limit_zero_round_trips_without_legacy_widening() {
    let e = setup(&[]).await;
    let api = Admin::login_bare(&e.state).await;
    let mut full = api.get_ok("/settings").await;
    full["budgets"]["daily_v5"] = json!(0);
    assert_eq!(
        api.json("PUT", "/settings", Some(full.clone())).await,
        (200, full)
    );
    assert_eq!(e.state.db.shared_budgets().await.unwrap().daily_v5, 0);
    assert_eq!(api.json("GET", "/quota-modes", None).await.0, 404);
}

#[tokio::test]
async fn runtime_limits_validate_apply_and_persist() {
    let e = setup(&[]).await;
    let api = Admin::login_bare(&e.state).await;
    // 每 Key 出图间隔 key_image_min_interval、账号出图间隔 image_min_interval 已删除：只剩两项，传它们算未知设置
    assert_eq!(
        api.text("GET", "/runtime-limits", None).await,
        (
            200,
            r#"{"queue_timeout":5,"image_429_cooldown_seconds":60}"#.into()
        )
    );
    let before = settings(&e).await;
    for (body, detail) in [
        (json!({}), "包含未知或空的运行限制设置"),
        (json!({"unknown": 15}), "包含未知或空的运行限制设置"),
        (
            json!({"queue_timeout": 0}),
            "queue_timeout 必须是 15～300 的整数",
        ),
        (
            json!({"queue_timeout": true}),
            "queue_timeout 必须是 15～300 的整数",
        ),
        (
            json!({"queue_timeout": 15.0}),
            "queue_timeout 必须是 15～300 的整数",
        ),
        (
            json!({"queue_timeout": "20"}),
            "queue_timeout 必须是 15～300 的整数",
        ),
        (
            json!({"queue_timeout": 301}),
            "queue_timeout 必须是 15～300 的整数",
        ),
        (
            json!({"image_429_cooldown_seconds": 59}),
            "image_429_cooldown_seconds 必须是 60～3600 的整数",
        ),
        (
            json!({"key_image_min_interval": 20}),
            "包含未知或空的运行限制设置",
        ),
        (
            json!({"image_min_interval": 20}),
            "包含未知或空的运行限制设置",
        ),
        // 多个字段时，按请求体顺序报第一个非法的；已校验过的也不会写
        (
            json!({"queue_timeout": 20, "image_429_cooldown_seconds": 1}),
            "image_429_cooldown_seconds 必须是 60～3600 的整数",
        ),
    ] {
        api.fails("PUT", "/runtime-limits", Some(body), 422, detail)
            .await;
    }
    assert_eq!(settings(&e).await, before);

    let all = json!({"queue_timeout": 120, "image_429_cooldown_seconds": 90});
    assert_eq!(
        api.text("PUT", "/runtime-limits", Some(all.clone())).await,
        (200, all.to_string())
    );
    let l = e.state.limits();
    assert_eq!(
        (l.queue_timeout, l.image_429_cooldown_seconds),
        (120.0, 90.0)
    );
    assert_eq!(
        changed(&before, &settings(&e).await),
        map(&[
            ("runtime_queue_timeout", "120"),
            ("runtime_image_429_cooldown_seconds", "90"),
        ])
    );
    // 只改一项：响应仍是两项全量
    assert_eq!(
        api.text("PUT", "/runtime-limits", Some(json!({"queue_timeout": 20})))
            .await,
        (
            200,
            r#"{"queue_timeout":20,"image_429_cooldown_seconds":90}"#.into()
        )
    );
    assert_eq!(api.get_ok("/runtime-limits").await["queue_timeout"], 20);

    // 重启后（读回库里保存的值）优先于 .env
    e.state
        .apply_runtime_limits(&[("queue_timeout", 200), ("image_429_cooldown_seconds", 99)])
        .await;
    e.state.load_runtime_limits().await.unwrap();
    assert_eq!(e.state.limits().queue_timeout, 20.0);
    assert_eq!(e.state.limits().image_429_cooldown_seconds, 90.0);

    let rows = actions(&e).await;
    assert!(rows.iter().any(|r| r.1 == "修改排队与冷却"
        && r.2 == "图片排队与冷却"
        && r.3 == all.to_string().replace(' ', "")
        && r.4));
}

// ------------------------------------------------------------------ 账号保护

/// 账号保护字段（名字, 默认, 下限, 上限），顺序同接口。每小时上限 / 按时段放行 10-22 删除；
/// 间隔抖动 `interval_jitter` 10-23 删除（出图节奏只剩出完后歇 N～M 秒，下限 1 秒）。
const GUARD_FIELDS: [(&str, i64, i64, i64); 5] = [
    ("account_daily_cap", 2400, 0, 9999),
    ("post_jitter_min", 1, 1, 30),
    ("post_jitter_max", 3, 1, 30),
    ("queue_per_member", 2, 1, 5),
    ("queue_per_account", 5, 0, 100),
];

#[tokio::test]
async fn guard_get_put() {
    let e = setup(&[]).await;
    let api = Admin::login_bare(&e.state).await;
    let v = api.get_ok("/guard").await;
    let names: Vec<&str> = v["fields"]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(names, GUARD_FIELDS.map(|f| f.0));
    for (name, default, min, max) in GUARD_FIELDS {
        let f = &v["fields"][name];
        assert_eq!(
            (&f["default"], &f["min"], &f["max"], &v["values"][name]),
            (&json!(default), &json!(min), &json!(max), &json!(default)),
            "{name}"
        );
    }
    // 每小时上限、安静时段都已删除
    assert!(v.get("hourly_cap_now").is_none() && v.get("quiet_now").is_none());

    let before = settings(&e).await;
    let (s, v) = api
        .json("PUT", "/guard", Some(json!({"queue_per_account": 8})))
        .await;
    assert_eq!(s, 200);
    assert_eq!(v["values"]["queue_per_account"], 8);
    // 不认识的字段（含已删除的每小时上限、安静时段）忽略；浮点但是整数值（3.0）可以；只写改过的字段
    api.json(
        "PUT",
        "/guard",
        Some(json!({"foo": 1, "account_hourly_cap": 120, "daily_burst": 0, "quiet_start": 3, "interval_jitter": 4, "post_jitter_max": 4.0})),
    )
    .await;
    assert_eq!(
        changed(&before, &settings(&e).await),
        map(&[
            ("guard_queue_per_account", "8"),
            ("guard_post_jitter_max", "4"),
        ])
    );
    assert!(
        e.state
            .guard
            .with(|g| !g.values.contains_key("interval_jitter"))
    );

    let label = "每个上游账号全站最多同时排几张图";
    let frozen = settings(&e).await;
    for (body, detail) in [
        (
            json!({"queue_per_account": 500}),
            format!("{label} 必须在 0～100 之间"),
        ),
        (
            json!({"queue_per_account": 1.5}),
            format!("{label} 必须是整数"),
        ),
        (
            json!({"queue_per_account": true}),
            format!("{label} 必须是整数"),
        ),
        (
            json!({"account_daily_cap": -1}),
            "每个上游账号每天最多出图张数 必须在 0～9999 之间".to_string(),
        ),
    ] {
        api.fails("PUT", "/guard", Some(body), 422, &detail).await;
    }
    assert_eq!(settings(&e).await, frozen);

    let rows = actions(&e).await;
    assert!(rows.iter().any(|r| r.1 == "修改账号保护与排队设置"
        && r.2 == "账号保护与排队"
        && r.3 == r#"{"queue_per_account":8}"#
        && r.4));
}

/// 出图后静默：后台可调，0～30 秒，保存后上游客户端立即按新值静默。
#[tokio::test]
async fn guard_post_jitter_is_adjustable() {
    let e = setup(&[]).await;
    let api = Admin::login_bare(&e.state).await;
    assert_eq!(
        e.state.guard.post_jitter(),
        (1.0, 3.0),
        "默认与原来的环境变量一致"
    );
    let before = settings(&e).await;
    api.json(
        "PUT",
        "/guard",
        Some(json!({"post_jitter_min": 2, "post_jitter_max": 6})),
    )
    .await;
    assert_eq!(e.state.guard.post_jitter(), (2.0, 6.0));
    assert_eq!(
        changed(&before, &settings(&e).await),
        map(&[
            ("guard_post_jitter_min", "2"),
            ("guard_post_jitter_max", "6")
        ])
    );
    api.fails(
        "PUT",
        "/guard",
        Some(json!({"post_jitter_max": 31})),
        422,
        "每张出完后最多歇几秒（小于最少时按最少） 必须在 1～30 之间",
    )
    .await;
    // 下限 1 秒：上游并发锁比响应晚一点释放（原来写死的 SETTLE_GAP 1 秒）
    api.fails(
        "PUT",
        "/guard",
        Some(json!({"post_jitter_min": 0})),
        422,
        "每张出完后至少歇几秒（实际在最少～最多之间均匀随机） 必须在 1～30 之间",
    )
    .await;
    // 重启后从库里读回
    e.state.guard.with(|g| {
        g.values.insert("post_jitter_min".into(), 1);
        g.values.insert("post_jitter_max".into(), 3);
    });
    e.state.guard.load().await.unwrap();
    assert_eq!(e.state.guard.post_jitter(), (2.0, 6.0));
}

/// 出图间隔 AIMD 的四个参数站长 10-23 删除：后台再提交也只是忽略，不写库。
#[tokio::test]
async fn removed_pace_params_are_ignored() {
    let e = setup(&[]).await;
    let api = Admin::login_bare(&e.state).await;
    let before = settings(&e).await;
    let (_, v) = api
        .json(
            "PUT",
            "/guard",
            Some(json!({"interval_cap": 90, "interval_decay_pct": 50})),
        )
        .await;
    assert!(v["values"].get("interval_cap").is_none(), "{v}");
    assert_eq!(changed(&before, &settings(&e).await), map(&[]));
}

#[tokio::test]
async fn guard_accounts_show_usage_and_pacing() {
    let e = setup(&[]).await;
    let api = Admin::login_bare(&e.state).await;
    let id = e.state.nai.tokens().await[0].token_id.clone();
    e.state
        .db
        .bump_upstream_image_counter(&id, &e.state.day(), 3)
        .await
        .unwrap();
    e.state.guard.with(|g| g.image_inflight.insert(7, 2));
    let v = api.get_ok("/guard").await;
    let a = &v["accounts"][0];
    assert_eq!(
        (&a["position"], &a["usable"], &a["today"]),
        (&json!(1), &json!(true), &json!(3))
    );
    // 出图节奏：测试环境出完不歇、还没有出图耗时样本 → 周期 0，估不出排队上限 = 不限；没挨过 429，没人在等
    assert!(a.get("interval").is_none(), "出图间隔 10-23 删除");
    assert_eq!(a["cycle"], json!(0.0));
    assert_eq!(a["queue_cap"], Value::Null);
    assert_eq!(a["last_429"], Value::Null);
    assert_eq!((&a["cooling"], &a["waiting"]), (&json!(0), &json!(0)));
    assert_eq!(v["queued_images"], 2);
    assert_eq!(
        v["queue_timeout"],
        json!(e.state.limits().queue_timeout as i64)
    );
    // 实测出图 32.5 秒：周期 = 出图耗时（测试环境不歇）；排队超时内能开始 ⌈超时 ÷ 32.5⌉ 张，
    // 全站上限取它和「账号排队」5 的较小值
    e.state.nai.tokens().await[0].set_gen_secs(32.5);
    let v = api.get_ok("/guard").await;
    let a = &v["accounts"][0];
    assert_eq!(a["cycle"], json!(32.5));
    let realistic = (e.state.limits().queue_timeout / 32.5).ceil() as i64;
    assert_eq!(a["queue_cap"], json!(realistic.max(1)));
    assert_eq!(v["queue_cap"], json!(realistic.clamp(1, 5)));
    assert_eq!(
        (&a["full"], &a["next_start_in"]),
        (&json!(false), &json!(0.0))
    );

    // 今天出满（单号每日上限 2400）：账号本身可用，但标「今日已满」，不再说「0 秒后能开始」
    e.state
        .db
        .bump_upstream_image_counter(&id, &e.state.day(), 2397)
        .await
        .unwrap();
    let v = api.get_ok("/guard").await;
    let a = &v["accounts"][0];
    assert_eq!(
        (&a["usable"], &a["full"], &a["today"], &a["next_start_in"]),
        (&json!(true), &json!(true), &json!(2400), &Value::Null)
    );
    assert_eq!(api.get_ok("/overview").await["pool"][0]["full"], true);
}

// ------------------------------------------------------------------ Discord 机器人

#[tokio::test]
async fn discord_bot_corrupt_store_is_a_server_error() {
    let e = setup(&[]).await;
    let api = Admin::login_bare(&e.state).await;
    // 库里的心跳不是合法 JSON：旧版 json.loads 抛异常没人接，500
    e.state
        .db
        .set_setting(gate_core::bot_config::STATUS_KEY, "not json")
        .await
        .unwrap();
    assert_eq!(api.text("GET", "/discord-bot", None).await.0, 500);
    // 是 JSON 但不是对象
    e.state
        .db
        .set_setting(gate_core::bot_config::STATUS_KEY, "[1]")
        .await
        .unwrap();
    assert_eq!(api.text("GET", "/discord-bot", None).await.0, 500);
}

/// 后台只剩「机器人状态」：跑图分享设置和最近动作删了（机器人从未实现），
/// 库里残留的 `bot_gallery_*` / `bot_events` 行不再被读（哪怕是坏的），也没有写设置的接口。
#[tokio::test]
async fn discord_bot_status_only() {
    let e = setup(&[]).await;
    let api = Admin::login_bare(&e.state).await;
    assert_eq!(
        api.text("GET", "/discord-bot", None).await,
        (200, r#"{"status":{"online":false}}"#.to_string())
    );

    for (key, value) in [
        ("bot_gallery_like", "1"),
        ("bot_gallery_forum", "作品"),
        ("bot_events", "not json"),
    ] {
        e.state.db.set_setting(key, value).await.unwrap();
    }
    assert_eq!(
        api.text("GET", "/discord-bot", None).await,
        (200, r#"{"status":{"online":false}}"#.to_string())
    );

    // 修改设置的接口没有了
    let frozen = settings(&e).await;
    let (s, _) = api
        .text("PUT", "/discord-bot", Some(json!({"gallery_like": 1})))
        .await;
    assert_eq!(s, 405);
    assert_eq!(settings(&e).await, frozen);

    // 机器人心跳：在线判断
    let now = e.state.now();
    let status = gate_core::bot_config::status_to_store(
        json!({"user": "奶妹", "guild": "测试服", "latency_ms": 42})
            .as_object()
            .unwrap(),
        now,
    );
    e.state
        .db
        .set_setting(gate_core::bot_config::STATUS_KEY, &status)
        .await
        .unwrap();
    let v = api.get_ok("/discord-bot").await;
    assert_eq!(v["status"]["online"], true);
    assert_eq!(v["status"]["user"], "奶妹");
    assert_eq!(v.as_object().unwrap().len(), 1, "只有 status：{v}");
    // 3 分钟没心跳 → 离线
    e.state
        .db
        .set_setting(
            gate_core::bot_config::STATUS_KEY,
            &gate_core::bot_config::status_to_store(&Map::new(), now - 600.0),
        )
        .await
        .unwrap();
    assert_eq!(api.get_ok("/discord-bot").await["status"]["online"], false);
}

// ------------------------------------------------------------------ 领 Key / 功能 / 记录

#[tokio::test]
async fn ops_snapshot_defaults() {
    let e = setup(&[]).await;
    let api = Admin::login_bare(&e.state).await;
    let v = api.get_ok("/ops").await;
    assert_eq!(
        v["registration"],
        json!({"configured":false,"open":false,"max_users":10,"expires_days":0,"roles":[],"active":0})
    );
    assert!(v.get("new_key").is_none());
    assert!(v["features"].get("new_key").is_none());
    assert_eq!(
        v["features"]["global"],
        json!({"image":true,"upscale":true,"augment":true,"vibe":true,"tags":true})
    );
    assert_eq!(v["log_retention"], json!({"origin_keep_days":0}));
    assert_eq!(
        v["upstream"],
        json!({"status":"idle","recent":0,"failed":0,"image_cooldown_seconds":0})
    );
}

#[tokio::test]
async fn ops_registration_put() {
    let e = setup(&[]).await;
    let api = Admin::login_bare(&e.state).await;
    let before = e.state.db.member_defaults().await.unwrap();
    let full = json!({"open":true,"max_users":20,"expires_days":10,"roles":[{"guild":"900000000000000001","role":"900000000000000002","note":"成员"}]});
    let (status, response) = api
        .json("PUT", "/ops/registration", Some(full.clone()))
        .await;
    assert_eq!(status, 200, "{response}");
    for field in ["open", "max_users", "expires_days", "roles"] {
        assert_eq!(response["registration"][field], full[field]);
    }
    assert_eq!(e.state.db.member_defaults().await.unwrap(), before);
    let frozen = settings(&e).await;
    for bad in [
        json!({"open":true}),
        json!({"daily_images":10}),
        json!({"role_extra":[]}),
        json!({"open":true,"max_users":-1,"expires_days":0,"roles":[]}),
    ] {
        assert_eq!(api.json("PUT", "/ops/registration", Some(bad)).await.0, 422);
    }
    assert_eq!(settings(&e).await, frozen);
}

#[tokio::test]
async fn ops_registration_defaults_are_constants_not_env() {
    let e = setup(&[
        ("REGISTER_MAX_USERS", "12"),
        ("REGISTER_DAILY_IMAGES", "40"),
        ("REGISTER_DAILY_V5", "5"),
        ("REGISTER_EXPIRES_DAYS", "14"),
    ])
    .await;
    let api = Admin::login_bare(&e.state).await;
    assert_eq!(api.get_ok("/ops").await["registration"]["max_users"], 10);
    assert_eq!(
        e.state.db.member_defaults().await.unwrap().daily_images,
        150
    );
    let full = json!({"open":false,"max_users":3,"expires_days":0,"roles":[]});
    assert_eq!(
        api.json("PUT", "/ops/registration", Some(full)).await.0,
        200
    );
    assert_eq!(api.get_ok("/ops").await["registration"]["max_users"], 3);
}

#[tokio::test]
async fn ops_features_put() {
    let e = setup(&[]).await;
    let api = Admin::login_bare(&e.state).await;
    let (s, v) = api
        .json(
            "PUT",
            "/ops/features",
            Some(json!({"flags": {"upscale": false}})),
        )
        .await;
    assert_eq!(s, 200);
    assert_eq!(v["features"]["global"]["upscale"], false);
    assert_eq!(
        settings(&e).await["features_global"],
        r#"{"image":true,"upscale":false,"augment":true,"vibe":true,"tags":true}"#
    );
    // 多项一起改，库里始终按功能顺序、Python 的默认分隔符
    api.json(
        "PUT",
        "/ops/features",
        Some(json!({"flags": {"tags": false, "image": true}})),
    )
    .await;
    assert_eq!(
        settings(&e).await["features_global"],
        r#"{"image":true,"upscale":false,"augment":true,"vibe":true,"tags":false}"#
    );
    let frozen = settings(&e).await;
    for body in [
        json!({"flags": {}}),
        json!({"flags": {"bogus": true}}),
        json!({"flags": {"upscale": 1}}),
        // 文本生成 / 语音已不是功能
        json!({"flags": {"text": false}}),
        json!({"flags": {"voice": true}}),
        json!({"flags": [1]}),
        json!({}),
    ] {
        api.fails(
            "PUT",
            "/ops/features",
            Some(body),
            422,
            "flags 必须是 {功能名: true/false}",
        )
        .await;
    }
    assert_eq!(settings(&e).await, frozen);
    let rows = actions(&e).await;
    assert!(rows.iter().any(|r| r.1 == "修改全局功能开关"
        && r.2 == "全站功能开关"
        && r.3 == r#"{"flags":{"upscale":false}}"#
        && r.4));
}

#[tokio::test]
async fn ops_features_corrupt_store_is_a_server_error() {
    let e = setup(&[]).await;
    let api = Admin::login_bare(&e.state).await;
    e.state
        .db
        .set_setting("features_global", "[1]")
        .await
        .unwrap();
    let (s, _) = api.text("GET", "/ops", None).await;
    assert_eq!(s, 500);
}

#[tokio::test]
async fn log_retention_put() {
    let e = setup(&[]).await;
    let api = Admin::login_bare(&e.state).await;
    let (status, value) = api
        .json(
            "PUT",
            "/ops/log-retention",
            Some(json!({"origin_keep_days":7})),
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(value["log_retention"]["origin_keep_days"], 7);
    for body in [
        json!({"origin_keep_days":-1}),
        json!({"origin_keep_days":366}),
        json!({"origin_keep_days":1.5}),
        json!({"origin_keep_days":true}),
    ] {
        let (status, _) = api.json("PUT", "/ops/log-retention", Some(body)).await;
        assert_eq!(status, 422);
    }
    assert_eq!(
        api.get_ok("/ops").await["log_retention"]["origin_keep_days"],
        7
    );
}

// ------------------------------------------------------------------ 公告与告警

/// 站长告警私信发到的频道（假 Discord 建私信频道时回这个 id）。
const OWNER_DM: &str = "/channels/900000000000000030/messages";

/// 告警私信接到本地假 Discord：机器人 Token + 站长 ID，建私信频道回固定 id。
fn owner_dm(hook: &MockUpstream) -> Vec<(&'static str, &'static str)> {
    hook.push(
        "/users/@me/channels",
        Reply::json(200, &json!({"id": "900000000000000030"})),
    );
    let base: &'static str = Box::leak(hook.url().into_boxed_str());
    vec![
        ("DISCORD_BOT_TOKEN", "synthetic-bot"),
        ("DISCORD_API_BASE", base),
        ("ALERT_USER_ID", "900000000000000031"),
    ]
}

#[tokio::test]
async fn alerts_test_unconfigured_then_throttled() {
    let e = setup(&[]).await;
    let api = Admin::login_bare(&e.state).await;
    api.fails(
        "POST",
        "/alerts/test",
        None,
        409,
        "尚未配置告警私信（DISCORD_BOT_TOKEN 与 ALERT_USER_ID）",
    )
    .await;
    // 配了私信（Discord 指向本地假服务）：发一条，10 秒内再发被节流
    let hook = MockUpstream::start().await;
    let e = setup(&owner_dm(&hook)).await;
    let api = Admin::login_bare(&e.state).await;
    assert_eq!(
        api.text("POST", "/alerts/test", None).await,
        (200, r#"{"ok":true}"#.into())
    );
    settle().await;
    let sent = hook.requests_to(OWNER_DM);
    assert_eq!(sent.len(), 1);
    assert_eq!(
        sent[0].json(),
        json!({"content": "⚠ **猫头鹰公益站告警**（test）\n这是一条测试告警，收到说明告警渠道正常。",
               "allowed_mentions": {"parse": []}})
    );
    let (s, t) = api.text("POST", "/alerts/test", None).await;
    assert_eq!(s, 429);
    assert!(
        t == r#"{"detail":"测试告警发送过于频繁，请等待 9 秒后再试"}"#
            || t == r#"{"detail":"测试告警发送过于频繁，请等待 10 秒后再试"}"#,
        "{t}"
    );
    settle().await;
    assert_eq!(hook.requests_to(OWNER_DM).len(), 1, "被节流的请求不会再发");
    // 操作日志里是「发送测试告警」
    let rows = actions(&e).await;
    assert_eq!(
        rows.iter()
            .filter(|r| r.1 == "发送测试告警")
            .map(|r| r.4)
            .collect::<Vec<_>>(),
        [true, false]
    );
}

// ------------------------------------------------------------------ 上游 Token

const SUBSCRIPTION_OK: &str = "/user/subscription";

fn pool_ids(v: &Value) -> Vec<String> {
    v["pool"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["token_id"].as_str().unwrap().to_string())
        .collect()
}

#[tokio::test]
async fn upstream_token_settings_endpoints() {
    let e = setup(&[]).await;
    let api = Admin::login_bare(&e.state).await;
    let id = e.state.nai.tokens().await[0].token_id.clone();
    assert_eq!(id, "token-12d24769535a51a7", "token_id 与旧版同一算法");
    let v5 = format!("/upstream-tokens/{id}/v5-limit");
    let conc = format!("/upstream-tokens/{id}/image-concurrency");
    let en = format!("/upstream-tokens/{id}/enabled");

    let msg5 = "上游 V5 日限额必须是 0～9999 的整数（0 = 停用免费 V5）";
    for bad in [
        json!(-1),
        json!(100001),
        json!(1.5),
        json!("12"),
        json!(true),
        json!(null),
    ] {
        api.fails("PUT", &v5, Some(json!({"v5_daily_limit": bad})), 422, msg5)
            .await;
    }
    api.fails("PUT", &v5, Some(json!({})), 422, msg5).await;
    api.fails(
        "PUT",
        "/upstream-tokens/token-unknown/v5-limit",
        Some(json!({"v5_daily_limit": 12})),
        404,
        "上游 Token 不存在",
    )
    .await;
    // 路径里的百分号编码会被解码（操作日志的对象名也是解码后的）
    api.fails(
        "PUT",
        "/upstream-tokens/%E4%BD%A0%E5%A5%BD/v5-limit",
        Some(json!({"v5_daily_limit": 12})),
        404,
        "上游 Token 不存在",
    )
    .await;
    assert!(actions(&e).await.iter().any(|r| r.2 == "上游 你好" && !r.4));
    assert_eq!(
        api.text("PUT", &v5, Some(json!({"v5_daily_limit": 12})))
            .await,
        (200, r#"{"ok":true,"v5_daily_limit":12}"#.into())
    );
    assert_eq!(e.state.nai.tokens().await[0].fields().v5_daily_limit, 12);
    assert_eq!(
        e.state.db.get_upstream_token_limits().await.unwrap(),
        vec![(id.clone(), 12)]
    );

    // 出图并发固定 1：后台的并发设置接口 10-23 删除
    assert_eq!(
        api.text("PUT", &conc, Some(json!({"image_concurrency": 2})))
            .await
            .0,
        405
    );
    assert_eq!(e.state.nai.tokens().await[0].image_slots.limit(), 1);

    for bad in [json!(0), json!(1), json!("false"), json!(null)] {
        api.fails(
            "PUT",
            &en,
            Some(json!({"enabled": bad})),
            422,
            "enabled 必须是布尔值",
        )
        .await;
    }
    api.fails(
        "PUT",
        "/upstream-tokens/token-unknown/enabled",
        Some(json!({"enabled": false})),
        404,
        "上游 Token 不存在",
    )
    .await;
    assert_eq!(
        api.text("PUT", &en, Some(json!({"enabled": false}))).await,
        (200, r#"{"ok":true,"enabled":false}"#.into())
    );
    assert!(!e.state.nai.tokens().await[0].fields().admin_enabled);
    assert_eq!(
        e.state.db.get_upstream_token_enabled().await.unwrap(),
        vec![(id.clone(), false)]
    );
    // 令牌池状态反映这些设置
    let ov = api.get_ok("/overview").await;
    assert_eq!(
        ov["pool"],
        json!([{"token_id": id, "position": 1, "token": "pst-sy…oken", "usable": false, "full": false, "admin_enabled": false,
                "disabled": false, "fails": 0, "blocked_for": 0, "last_ok": 0.0, "allow_anlas": true,
                "image_concurrency": 1, "image_active": 0, "v5_daily_limit": 12, "v5_used": 0, "images_today": 0}])
    );
    assert_eq!(
        api.text("PUT", &en, Some(json!({"enabled": true}))).await.0,
        200
    );
    assert!(e.state.nai.tokens().await[0].fields().admin_enabled);

    let rows = actions(&e).await;
    let find = |a: &str, ok: bool| {
        rows.iter()
            .any(|r| r.1 == a && r.2 == format!("上游 {id}") && r.4 == ok)
    };
    assert!(find("修改上游 Token V5 日限", true) && find("修改上游 Token V5 日限", false));
    assert!(find("启用/停用上游 Token", true));
    assert!(rows.iter().any(|r| r.1 == "修改上游 Token V5 日限"
        && r.2 == "上游 token-unknown"
        && r.3 == "失败（404）：上游 Token 不存在"));
}

#[tokio::test]
async fn upstream_token_add_replace_remove() {
    let hook = MockUpstream::start().await;
    let e = setup(&owner_dm(&hook)).await;
    let api = Admin::login_bare(&e.state).await;
    let first = e.state.nai.tokens().await[0].token_id.clone();
    let file = e.state.env.upstream_tokens_path();
    assert!(
        !file.exists(),
        "后台没接管过：令牌池来自 NAI_TOKENS，没有 Token 文件"
    );

    // 格式不对：还没向 NovelAI 验证就拒绝
    let fmt = "Token 格式不对：应以 pst- 开头（在 NovelAI 的 账号设置 → Get Persistent API Token 里获取）";
    for body in [
        json!({}),
        json!({"token": "abc"}),
        json!({"token": "pst-short"}),
        json!({"token": 5}),
        json!({"token": null}),
    ] {
        api.fails("POST", "/upstream-tokens", Some(body), 422, fmt)
            .await;
    }
    assert!(e.mock.requests_to(SUBSCRIPTION_OK).is_empty());
    // 官方说无效
    e.mock.push(
        SUBSCRIPTION_OK,
        Reply::json(401, &json!({"statusCode": 401})),
    );
    api.fails(
        "POST",
        "/upstream-tokens",
        Some(json!({"token": "pst-bad-token-value-0001"})),
        422,
        "NovelAI 拒绝了这把 Token（无效，或已被重置）",
    )
    .await;
    // 订阅无效 / 官方 5xx / 看不懂
    e.mock.push(
        SUBSCRIPTION_OK,
        Reply::json(200, &json!({"tier": 3, "active": false})),
    );
    api.fails(
        "POST",
        "/upstream-tokens",
        Some(json!({"token": "pst-bad-token-value-0001"})),
        422,
        "这把 Token 对应的账号没有有效订阅",
    )
    .await;
    e.mock.push(SUBSCRIPTION_OK, Reply::status(503, "down"));
    api.fails(
        "POST",
        "/upstream-tokens",
        Some(json!({"token": "pst-bad-token-value-0001"})),
        422,
        "NovelAI 返回 503，暂时无法验证",
    )
    .await;
    // 已在池里
    api.fails(
        "POST",
        "/upstream-tokens",
        Some(json!({"token": "pst-synthetic-upstream-token"})),
        409,
        "这把 Token 已经在令牌池里",
    )
    .await;
    assert_eq!(e.state.nai.tokens().await.len(), 1);
    assert!(!file.exists());

    // 添加（首尾空白被去掉）
    e.mock.clear();
    let (s, t) = api
        .text(
            "POST",
            "/upstream-tokens",
            Some(json!({"token": " pst-test-added-token-0001 ", "allow_anlas": false})),
        )
        .await;
    assert_eq!(s, 200);
    let v: Value = serde_json::from_str(&t).unwrap();
    assert_eq!(
        (v["ok"].clone(), v["tier"].clone()),
        (json!(true), json!(3))
    );
    assert!(
        !t.contains("pst-test-added-token-0001") && !t.contains("pst-synthetic-upstream-token"),
        "响应里没有完整 Token"
    );
    let ids = pool_ids(&v);
    assert_eq!(ids.len(), 2);
    assert_eq!(v["pool"][1]["token"], "pst-te…0001");
    assert_eq!(
        (
            v["pool"][1]["position"].clone(),
            v["pool"][1]["allow_anlas"].clone()
        ),
        (json!(2), json!(false))
    );
    // 验证请求带着这把 Token
    let seen = e.mock.requests_to(SUBSCRIPTION_OK);
    assert_eq!(seen.len(), 1);
    assert_eq!(
        seen[0].header("authorization"),
        Some("Bearer pst-test-added-token-0001")
    );
    // Token 文件：权限 600，完整 Token 只在这里
    assert_eq!(
        serde_json::from_str::<Value>(&std::fs::read_to_string(&file).unwrap()).unwrap(),
        json!({"tokens": [{"token": "pst-synthetic-upstream-token", "allow_anlas": true},
                          {"token": "pst-test-added-token-0001", "allow_anlas": false}]})
    );
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&file).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    // 站长私信：变更提醒
    settle().await;
    let alerts: Vec<String> = hook
        .requests_to(OWNER_DM)
        .iter()
        .map(|r| r.json()["content"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(alerts.len(), 1);
    assert!(alerts[0].contains("（upstream_token_changed）\n后台添加了一把上游 Token（订阅等级 3）。如果这不是你操作的，请立刻更换后台密码。"), "{alerts:?}");

    // 替换第二把：V5 日限 / 并发 / 启停 / 当天计数延续到新标识
    let second = ids[1].clone();
    api.json(
        "PUT",
        &format!("/upstream-tokens/{second}/v5-limit"),
        Some(json!({"v5_daily_limit": 7})),
    )
    .await;
    api.json(
        "PUT",
        &format!("/upstream-tokens/{second}/enabled"),
        Some(json!({"enabled": false})),
    )
    .await;
    e.state
        .db
        .bump_upstream_image_counter(&second, &e.state.day(), 2)
        .await
        .unwrap();
    let (s, t) = api
        .text(
            "PUT",
            &format!("/upstream-tokens/{second}"),
            Some(json!({"token": "pst-test-replaced-token-0002"})),
        )
        .await;
    assert_eq!(s, 200);
    let v: Value = serde_json::from_str(&t).unwrap();
    assert!(!t.contains("pst-test-replaced-token-0002"));
    let ids2 = pool_ids(&v);
    assert_eq!(
        (ids2[0].as_str(), ids2[1].as_str() != second),
        (first.as_str(), true)
    );
    assert_eq!(v["pool"][1]["token"], "pst-te…0002");
    assert_eq!(
        (
            v["pool"][1]["v5_daily_limit"].clone(),
            v["pool"][1]["admin_enabled"].clone(),
            v["pool"][1]["images_today"].clone(),
            v["pool"][1]["allow_anlas"].clone()
        ),
        (json!(7), json!(false), json!(2), json!(false))
    );
    assert_eq!(
        e.state.db.get_upstream_token_limits().await.unwrap(),
        vec![(ids2[1].clone(), 7)],
        "库里的设置从旧标识搬到新标识"
    );
    assert_eq!(
        serde_json::from_str::<Value>(&std::fs::read_to_string(&file).unwrap()).unwrap()["tokens"]
            [1],
        json!({"token": "pst-test-replaced-token-0002", "allow_anlas": false})
    );
    // 旧标识已不存在 → 404；和池里另一把重复 → 409；官方不认 → 422
    api.fails(
        "PUT",
        &format!("/upstream-tokens/{second}"),
        Some(json!({"token": "pst-test-replaced-token-0003"})),
        404,
        "上游 Token 不存在",
    )
    .await;
    api.fails(
        "PUT",
        "/upstream-tokens/token-unknown",
        Some(json!({"token": "pst-test-replaced-token-0003"})),
        404,
        "上游 Token 不存在",
    )
    .await;
    api.fails(
        "PUT",
        &format!("/upstream-tokens/{first}"),
        Some(json!({"token": "pst-test-replaced-token-0002"})),
        409,
        "这把 Token 已经在令牌池的另一个位置",
    )
    .await;
    e.mock.push(SUBSCRIPTION_OK, Reply::json(403, &json!({})));
    api.fails(
        "PUT",
        &format!("/upstream-tokens/{first}"),
        Some(json!({"token": "pst-bad-token-value-0001"})),
        422,
        "NovelAI 拒绝了这把 Token（无效，或已被重置）",
    )
    .await;

    // 删除
    api.fails(
        "DELETE",
        "/upstream-tokens/token-unknown",
        None,
        404,
        "上游 Token 不存在",
    )
    .await;
    let (s, t) = api
        .text("DELETE", &format!("/upstream-tokens/{}", ids2[1]), None)
        .await;
    assert_eq!(s, 200);
    let v: Value = serde_json::from_str(&t).unwrap();
    assert_eq!(
        (v["ok"].clone(), pool_ids(&v)),
        (json!(true), vec![first.clone()])
    );
    assert_eq!(
        serde_json::from_str::<Value>(&std::fs::read_to_string(&file).unwrap()).unwrap(),
        json!({"tokens": [{"token": "pst-synthetic-upstream-token", "allow_anlas": true}]})
    );
    // 至少保留一把（即使要删的不存在，也是这个提示——与旧版一致）
    api.fails(
        "DELETE",
        &format!("/upstream-tokens/{first}"),
        None,
        409,
        "至少需要保留一把上游 Token",
    )
    .await;
    api.fails(
        "DELETE",
        "/upstream-tokens/token-unknown",
        None,
        409,
        "至少需要保留一把上游 Token",
    )
    .await;

    // 操作日志：完整 Token 不会出现；对象是「上游 <标识前 24 位>」
    let rows = actions(&e).await;
    for r in &rows {
        assert!(
            !format!("{r:?}").contains("pst-test-"),
            "操作日志里不能有 Token：{r:?}"
        );
    }
    assert!(rows.iter().any(|r| r.1 == "添加上游 Token"
        && r.2.is_empty()
        && r.3 == r#"{"token":"（已隐去）","allow_anlas":false}"#
        && r.4));
    assert!(rows.iter().any(|r| r.1 == "替换上游 Token"
        && r.2 == format!("上游 {second}")
        && r.3 == r#"{"token":"（已隐去）"}"#
        && r.4));
    assert!(rows.iter().any(|r| r.1 == "删除上游 Token"
        && r.2 == format!("上游 {}", ids2[1])
        && r.3.is_empty()
        && r.4));
    assert!(rows.iter().any(|r| r.1 == "删除上游 Token"
        && r.3 == "失败（409）：至少需要保留一把上游 Token"
        && !r.4));
}

#[tokio::test]
async fn upstream_token_body_is_limited_to_4k() {
    let e = setup(&[]).await;
    let api = Admin::login_bare(&e.state).await;
    api.fails(
        "POST",
        "/upstream-tokens",
        Some(json!({"token": "p".repeat(5000)})),
        413,
        "请求内容过大，未处理。请缩小图片或减少参考图。",
    )
    .await;
}

// ------------------------------------------------------------------ Anlas 核对

async fn csrf(api: &Admin) -> String {
    api.get_ok("/upstream").await["csrf_token"]
        .as_str()
        .unwrap()
        .to_string()
}

#[tokio::test]
async fn reconciliation_requests_are_guarded() {
    let e = setup(&[]).await;
    let api = Admin::login_bare(&e.state).await;
    let (s, h, body) = api.raw("GET", "/upstream", None, &[]).await.parts();
    assert_eq!(s, 200);
    assert_eq!(h["cache-control"], "no-store");
    let v: Value = serde_json::from_str(&body).unwrap();
    let token = v["csrf_token"].as_str().unwrap().to_string();
    assert_eq!(
        (
            v["reconciliation"]["history"].clone(),
            v["reconciliation"]["retry_after"].clone(),
            v["reconciliation"]["running"].clone(),
            token.len()
        ),
        (json!([]), json!(0), json!(false), 64)
    );
    // 同一会话令牌稳定；换一个会话就变
    assert_eq!(csrf(&api).await, token);
    let other = Admin::login_bare(&e.state).await;
    if other.cookie != api.cookie {
        assert_ne!(csrf(&other).await, token);
    }

    // 缺 / 错的 CSRF 头：403，什么也没查
    api.fails(
        "POST",
        "/reconciliation",
        Some(json!({})),
        403,
        "会话校验失败，请刷新后台后重试",
    )
    .await;
    let wrong = "0".repeat(64);
    let (s, _, t) = api
        .raw(
            "POST",
            "/reconciliation",
            Some(json!({})),
            &[("x-nai-admin-csrf", wrong.as_str())],
        )
        .await
        .parts();
    assert_eq!(
        (s, t.as_str()),
        (403, r#"{"detail":"会话校验失败，请刷新后台后重试"}"#)
    );
    // 另一个会话的令牌不能拿来用
    if other.cookie != api.cookie {
        let theirs = csrf(&other).await;
        let (s, _, _) = api
            .raw(
                "POST",
                "/reconciliation",
                Some(json!({})),
                &[("x-nai-admin-csrf", theirs.as_str())],
            )
            .await
            .parts();
        assert_eq!(s, 403);
    }
    let h = [("x-nai-admin-csrf", token.as_str())];
    // 内容类型必须是 JSON
    let (s, _, t) = api
        .raw(
            "POST",
            "/reconciliation",
            Some(json!({})),
            &[h[0], ("content-type", "text/plain")],
        )
        .await
        .parts();
    assert_eq!(
        (s, t.as_str()),
        (415, r#"{"detail":"请使用 JSON 请求"}"#),
        "多个 content-type 头时取第一个"
    );
    // 不接受任何自定义内容
    let (s, _, t) = api
        .raw(
            "POST",
            "/reconciliation",
            Some(json!({"url": "https://untrusted.invalid"})),
            &h,
        )
        .await
        .parts();
    assert_eq!(
        (s, t.as_str()),
        (400, r#"{"detail":"本操作不接受自定义账号或地址"}"#)
    );
    let (s, _, t) = api
        .raw("POST", "/reconciliation", Some(json!([])), &h)
        .await
        .parts();
    assert_eq!(
        (s, t.as_str()),
        (400, r#"{"detail":"请求体必须是 JSON 对象"}"#)
    );
    let (s, _, t) = api
        .raw(
            "POST",
            "/reconciliation",
            Some(json!({"big": "a".repeat(1100)})),
            &h,
        )
        .await
        .parts();
    assert_eq!(
        (s, t.as_str()),
        (
            413,
            r#"{"detail":"请求内容过大，未处理。请缩小图片或减少参考图。"}"#
        )
    );
    let (s, _, t) = api
        .raw(
            "POST",
            "/reconciliation",
            None,
            &[h[0], ("content-type", "application/json")],
        )
        .await
        .parts();
    assert_eq!(
        (s, t.as_str()),
        (
            400,
            r#"{"detail":"请求体不是合法的 UTF-8 JSON，或嵌套超过 32 层"}"#
        )
    );
    let (s, _, t) = api.raw("POST", "/reconciliation", None, &h).await.parts();
    assert_eq!(
        (s, t.as_str()),
        (415, r#"{"detail":"请使用 JSON 请求"}"#),
        "没有 Content-Type 也是 415"
    );
    assert!(
        e.mock.requests_to(SUBSCRIPTION_OK).is_empty(),
        "校验没过之前不能去查官方"
    );
    assert_eq!(
        api.get_ok("/upstream").await["reconciliation"]["retry_after"],
        0,
        "也没有写冷却"
    );

    let rows = actions(&e).await;
    assert!(
        rows.iter()
            .all(|r| r.1 == "Anlas 核对" || r.1 == "登录后台")
    );
    let forbidden = rows
        .iter()
        .filter(|r| r.1 == "Anlas 核对" && r.3 == "失败（403）：会话校验失败，请刷新后台后重试")
        .count();
    assert_eq!(forbidden, 2 + usize::from(other.cookie != api.cookie));
}

#[tokio::test]
async fn reconciliation_compares_balances() {
    let e = setup(&[]).await;
    let api = Admin::login_bare(&e.state).await;
    let token = csrf(&api).await;
    let h = [("x-nai-admin-csrf", token.as_str())];
    let id = e.state.nai.tokens().await[0].token_id.clone();

    let (s, hd, t) = api
        .raw("POST", "/reconciliation", Some(json!({})), &h)
        .await
        .parts();
    assert_eq!(s, 200);
    assert_eq!(hd["cache-control"], "no-store");
    let v: Value = serde_json::from_str(&t).unwrap();
    assert_eq!(v["running"], false);
    assert_eq!(v["csrf_token"], token);
    let (retry, first) = (v["retry_after"].as_i64().unwrap(), v["history"][0].clone());
    assert!(
        (59..=60).contains(&retry),
        "每次核对先写 60 秒冷却：{retry}"
    );
    assert_eq!(first["id"], 1);
    assert_eq!(
        first["accounts"],
        json!({ id.clone(): {"fixed": 10000.0, "purchased": 0.0, "expires_at": 1893456000.0} })
    );
    assert_eq!(
        first["ledger"],
        json!({"anlas": 0, "unconfirmed_anlas": 0, "unconfirmed_requests": 0, "last_log_id": 0, "site_paid": 0})
    );
    assert_eq!(
        (first["balance"].clone(), first["comparison"].clone()),
        (json!(10000.0), json!({"reason": "first"}))
    );
    // 键顺序与旧版快照一致
    let keys: Vec<&str> = first
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        [
            "checked_at",
            "accounts",
            "ledger",
            "balance",
            "comparison",
            "id"
        ]
    );
    assert!(first["checked_at"].is_f64());
    // 查询带的是这把上游 Token
    let q = e.mock.requests_to(SUBSCRIPTION_OK);
    assert_eq!(q.len(), 1);
    assert_eq!(
        q[0].header("authorization"),
        Some("Bearer pst-synthetic-upstream-token")
    );
    // 冷却中：429 + Retry-After，不再查官方
    let (s, hd, t) = api
        .raw("POST", "/reconciliation", Some(json!({})), &h)
        .await
        .parts();
    assert_eq!(s, 429);
    let secs: i64 = hd["retry-after"].to_str().unwrap().parse().unwrap();
    assert!((1..=60).contains(&secs));
    assert_eq!(
        t,
        json!({"detail": format!("请在 {secs} 秒后重试")}).to_string()
    );
    assert_eq!(e.mock.requests_to(SUBSCRIPTION_OK).len(), 1);

    // 冷却过后：账上记了 7 Anlas、官方余额少了 10 → 差 3；未确认 3 笔里 1 个请求
    e.state
        .db
        .set_setting(gate_server::reconciliation::COOLDOWN_SETTING, "0")
        .await
        .unwrap();
    let key = e.state.db.get_key_by_token(TOKEN).await.unwrap().unwrap();
    let kid = key["id"].as_i64().unwrap();
    e.state
        .db
        .record_success(
            &Success {
                key_id: kid,
                key_name: "测试成员".into(),
                kind: "image".into(),
                model: "nai-diffusion-4-5-full".into(),
                day: e.state.day(),
                images: 1,
                anlas: 7.0,
                ..Default::default()
            },
            e.state.now(),
        )
        .await
        .unwrap();
    e.state
        .db
        .add_log(
            &NewLog {
                key_id: Some(kid),
                key_name: "测试成员".into(),
                kind: "image".into(),
                status: "error".into(),
                unconfirmed_anlas: 3.0,
                ..Default::default()
            },
            e.state.now(),
        )
        .await
        .unwrap();
    // 官方：余额减少 10
    e.mock.push(
        SUBSCRIPTION_OK,
        Reply::json(200, &json!({"active": true, "expiresAt": 1893456000, "trainingStepsLeft": {"fixedTrainingStepsLeft": 9990, "purchasedTrainingSteps": 0}})),
    );
    let (s, _, t) = api
        .raw("POST", "/reconciliation", Some(json!({})), &h)
        .await
        .parts();
    assert_eq!(s, 200, "{t}");
    let v: Value = serde_json::from_str(&t).unwrap();
    let second = v["history"][0].clone();
    assert_eq!(v["history"][1]["id"], 1);
    assert_eq!(
        second["comparison"],
        json!({"reason": null, "since": first["checked_at"], "balance_decrease": 10.0, "recorded": 7.0, "site_paid": 0.0, "difference": 3.0,
               "unconfirmed_anlas": 3.0, "unconfirmed_requests": 1.0})
    );
    // 新快照里的数值一律是 f64（整数值也带小数点）
    assert!(
        t.contains(r#""balance_decrease":10.0,"recorded":7.0,"site_paid":0.0,"difference":3.0,"#),
        "{t}"
    );
    // 完整 Token 不在任何响应里
    assert!(!t.contains("pst-synthetic-upstream-token"));

    // 官方出错：不推进基线，不泄露响应；失败的尝试也进了冷却
    for (reply, why) in [
        (Reply::status(500, "leak-me"), "500"),
        (Reply::status(200, "not-json"), "not-json"),
        (
            Reply::json(
                200,
                &json!({"active": true, "trainingStepsLeft": {"fixedTrainingStepsLeft": -1, "purchasedTrainingSteps": 0}}),
            ),
            "负余额",
        ),
        (Reply::json(200, &json!({"active": false})), "未订阅"),
    ] {
        e.state
            .db
            .set_setting(gate_server::reconciliation::COOLDOWN_SETTING, "0")
            .await
            .unwrap();
        e.mock.push(SUBSCRIPTION_OK, reply);
        let (s, _, t) = api
            .raw("POST", "/reconciliation", Some(json!({})), &h)
            .await
            .parts();
        assert_eq!(
            (s, t.as_str()),
            (503, r#"{"detail":"官方余额未能完整读取；上次结果已保留"}"#),
            "{why}"
        );
        let st = api.get_ok("/upstream").await;
        assert_eq!(
            st["reconciliation"]["history"].as_array().unwrap().len(),
            2,
            "{why}"
        );
        assert!(
            st["reconciliation"]["retry_after"].as_i64().unwrap() > 0,
            "{why}"
        );
    }

    // 401 仍使用通用错误文案，但会隔离失效账号；放在最后，避免改变后续错误场景的前置条件。
    e.state
        .db
        .set_setting(gate_server::reconciliation::COOLDOWN_SETTING, "0")
        .await
        .unwrap();
    e.mock.push(
        SUBSCRIPTION_OK,
        Reply::json(401, &json!({"token": "leak-me"})),
    );
    let (s, _, t) = api
        .raw("POST", "/reconciliation", Some(json!({})), &h)
        .await
        .parts();
    assert_eq!(
        (s, t.as_str()),
        (503, r#"{"detail":"官方余额未能完整读取；上次结果已保留"}"#)
    );
    assert!(
        api.get_ok("/upstream").await["reconciliation"]["retry_after"]
            .as_i64()
            .unwrap()
            > 0
    );
}

#[tokio::test]
async fn reconciliation_official_rate_limit_is_remembered() {
    let e = setup(&[]).await;
    let api = Admin::login_bare(&e.state).await;
    let token = csrf(&api).await;
    let h = [("x-nai-admin-csrf", token.as_str())];
    for (header, want) in [
        ("600", 600),
        ("5", 300),
        ("999999", 86400),
        ("invalid", 300),
    ] {
        e.state
            .db
            .set_setting(gate_server::reconciliation::COOLDOWN_SETTING, "0")
            .await
            .unwrap();
        e.mock.push(SUBSCRIPTION_OK, Reply::rate_limited(header));
        let (s, hd, t) = api
            .raw("POST", "/reconciliation", Some(json!({})), &h)
            .await
            .parts();
        assert_eq!(s, 429, "{header}");
        assert_eq!(hd["retry-after"], want.to_string().as_str(), "{header}");
        assert_eq!(
            t,
            r#"{"detail":"官方查询限流，请稍后重试；上次结果已保留"}"#
        );
        let st = api.get_ok("/upstream").await["reconciliation"].clone();
        assert!(
            (want - 2..=want).contains(&st["retry_after"].as_i64().unwrap()),
            "{header}: {st}"
        );
        assert_eq!(st["history"], json!([]));
    }
}

#[tokio::test]
async fn reconciliation_refuses_when_account_is_unusable() {
    let e = setup(&[]).await;
    let api = Admin::login_bare(&e.state).await;
    let token = csrf(&api).await;
    let h = [("x-nai-admin-csrf", token.as_str())];
    // 账号被上游 401 判了失效 → 不查
    let t = e.state.nai.tokens().await[0].clone();
    e.state.nai.isolate_unauthorized(&t).await;
    let (s, _, body) = api
        .raw("POST", "/reconciliation", Some(json!({})), &h)
        .await
        .parts();
    assert_eq!(
        (s, body.as_str()),
        (
            503,
            r#"{"detail":"有上游账号已失效，请先检查令牌池；尚未查询官方"}"#
        )
    );
    assert!(e.mock.requests_to(SUBSCRIPTION_OK).is_empty());
    assert_eq!(
        api.get_ok("/upstream").await["reconciliation"]["retry_after"],
        0
    );
}

#[tokio::test]
async fn reconciliation_waits_for_inflight_reservations_and_blocks_admission() {
    use gate_server::state::Reservation;
    let e = setup(&[]).await;
    e.state.budget.lock().await.reservations.insert(
        1,
        Reservation {
            request_id: "test-reservation".into(),
            active: true,
            key_id: 1,
            day: "2026-10-25".into(),
            month: "2026-10".into(),
            is_vibe: false,
            anlas: 0.0,
            v5: 0,
            legacy: 0,
        },
    );
    // 预留还在：等不到 → 409，也没有查官方、没有写冷却
    let busy =
        gate_server::reconciliation::run_with(&e.state, std::time::Duration::from_millis(150))
            .await;
    match busy {
        Err(gate_server::reconciliation::RunError::Refused(r)) => {
            assert_eq!(
                (r.status, r.message.as_str()),
                (409, "图片任务仍在处理，请稍后核对；尚未查询官方")
            );
        }
        other => panic!("{other:?}"),
    }
    assert!(e.mock.requests_to(SUBSCRIPTION_OK).is_empty());
    assert_eq!(
        gate_server::reconciliation::retry_after(&e.state)
            .await
            .unwrap(),
        0
    );

    // 预留清空后立刻继续（不用等满超时）；运行期间 running = true，预算锁被持着
    let state = e.state.clone();
    let task = tokio::spawn(async move {
        gate_server::reconciliation::run_with(&state, std::time::Duration::from_secs(10)).await
    });
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert!(!task.is_finished());
    assert!(e.mock.requests_to(SUBSCRIPTION_OK).is_empty());
    {
        let mut b = e.state.budget.lock().await;
        b.reservations.clear();
        e.state.budget_idle.notify_waiters();
    }
    let v = tokio::time::timeout(std::time::Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(v["history"][0]["comparison"], json!({"reason": "first"}));
    assert_eq!(v["running"], false);
    assert_eq!(e.mock.requests_to(SUBSCRIPTION_OK).len(), 1);
}

#[tokio::test]
async fn reconciliation_only_one_at_a_time() {
    let e = setup(&[]).await;
    e.mock.push(SUBSCRIPTION_OK, Reply::Hang);
    let state = e.state.clone();
    let first = tokio::spawn(async move { gate_server::reconciliation::run(&state).await });
    for _ in 0..50 {
        if !e.mock.requests_to(SUBSCRIPTION_OK).is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert_eq!(e.mock.requests_to(SUBSCRIPTION_OK).len(), 1);
    // 正在核对：状态里 running = true；再点一次 → 409
    let st = gate_server::reconciliation::status(&e.state).await.unwrap();
    assert_eq!(st["running"], true);
    match gate_server::reconciliation::run(&e.state).await {
        Err(gate_server::reconciliation::RunError::Refused(r)) => {
            assert_eq!(
                (r.status, r.message.as_str()),
                (409, "已有核对正在进行，请稍后查看结果")
            );
        }
        other => panic!("{other:?}"),
    }
    // 取消（调用方断开）：锁都释放，历史里没有半条
    first.abort();
    let _ = first.await;
    let st = gate_server::reconciliation::status(&e.state).await.unwrap();
    assert_eq!(
        (st["running"].clone(), st["history"].clone()),
        (json!(false), json!([]))
    );
    assert!(e.state.budget.try_lock().is_ok());
    assert!(st["retry_after"].as_i64().unwrap() > 0);
}

// ------------------------------------------------------------------ 总览 / 状态 / 监控

/// 总览页 10-23 删除：`/overview` 只剩「上游」页用的令牌池两项；状态码分布接口删除。
#[tokio::test]
async fn overview_is_only_the_token_pool() {
    let e = setup(&[]).await;
    let api = Admin::login_bare(&e.state).await;
    let v = api.get_ok("/overview").await;
    let keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
    assert_eq!(keys, ["pool", "pool_configured"]);
    assert_eq!(v["pool_configured"], true);
    let (status, _) = api.json("GET", "/status-codes", None).await;
    assert_eq!(status, 404);
}

/// 「系统」页只剩告警渠道；生成记录页读 audit。服务器状态卡片（磁盘 / 上游健康 / 防爆破）删了，字段跟着删。
#[tokio::test]
async fn server_status_reports_alerts() {
    let e = setup(&[]).await;
    let api = Admin::login_bare(&e.state).await;
    assert_eq!(
        api.get_ok("/status").await,
        json!({"alerts":{"configured":false,"sent":0}})
    );
}

#[tokio::test]
async fn allowance_snapshot_is_read_only() {
    let e = setup(&[]).await;
    let api = Admin::login_bare(&e.state).await;
    let page = api.get_ok("/upstream").await;
    assert_eq!(page["threshold"], 20);
    // V5 官方余量是缓存投影：还没出过图时全部未知，行挂在账号上
    assert_eq!(
        page["accounts"][0]["v5"],
        json!({"account":1,"percent":null,"time_until_next_percent_seconds":null,"current_response_recharge_per_day_estimate":null,"historical_sample_based_recharge_per_day_estimate":null,"recharge_per_day":null,"active":null,"tier":null,"expires_at":null,"is_grace_period":null,"account_type":null,"purchased_percent":null,"fixed_anlas":null,"purchased_anlas":null,"is_negative":null,"checked_at":null,"low":false,"uncertain":true,"error":null,"stale":true})
    );
    // 查看额度不会去问 NovelAI（只有真实出图会刷新缓存）
    assert!(e.mock.requests_to(SUBSCRIPTION_OK).is_empty());
    // 「低」的线固定 20%（后台的 V5 提示线 10-23 删除，旧设置不再读）
    e.state
        .db
        .set_setting("v5_alert_threshold", "35")
        .await
        .unwrap();
    assert_eq!(api.get_ok("/upstream").await["threshold"], 20);
}

#[tokio::test]
async fn v5_medium_mode_is_validated_stored_and_audited() {
    let e = setup(&[]).await;
    let api = Admin::login_bare(&e.state).await;
    for mode in ["on", "off"] {
        let mut full = api.get_ok("/settings").await;
        full["v5_medium_mode"] = json!(mode);
        assert_eq!(api.json("PUT", "/settings", Some(full)).await.0, 200);
        assert_eq!(settings(&e).await["v5_medium_mode"], mode);
    }
    let frozen = settings(&e).await;
    for bad in [json!("auto"), json!("ON"), json!(true), json!(null)] {
        let mut full = api.get_ok("/settings").await;
        full["v5_medium_mode"] = bad;
        full["v5_paused"] = json!(true);
        assert_eq!(api.json("PUT", "/settings", Some(full)).await.0, 422);
    }
    assert_eq!(settings(&e).await, frozen);
    assert!(actions(&e).await.iter().any(|r| r.1 == "修改预算" && r.4));
}

#[tokio::test]
async fn v5_medium_state_follows_the_manual_switch() {
    let e = setup(&[]).await;
    let api = Admin::login_bare(&e.state).await;
    let mut full = api.get_ok("/settings").await;
    assert_eq!(full["v5_medium_mode"], "off");
    assert!(full.get("v5_medium_state").is_none());
    full["v5_medium_mode"] = json!("on");
    assert_eq!(api.json("PUT", "/settings", Some(full)).await.0, 200);
    assert_eq!(api.get_ok("/settings").await["v5_medium_mode"], "on");
}
