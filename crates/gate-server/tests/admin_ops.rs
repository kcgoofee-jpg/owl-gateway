//! 后台「运营 / 监控」接口：预算、排队与冷却、账号保护、领 Key / 功能 / 记录开关、公告与告警、
//! Discord 机器人、上游 Token 增删改、Anlas 核对、总览与监控。
//!
//! 验证原生类型校验、设置持久化、操作日志和实际请求行为。
//! 公开仓库：全部是合成数据；不会连 Discord / NovelAI（上游是本地假服务）。

mod common;
use common::*;
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

fn settings_write(mut value: Value) -> Value {
    if let Some(obj) = value.as_object_mut() {
        obj.remove("fields");
    }
    value
}

#[tokio::test]
async fn settings_get_put_and_stored_strings() {
    let e = setup(&[]).await;
    let api = Admin::login_bare(&e.state).await;
    let (_, initial) = api.json("GET", "/settings", None).await;
    assert_eq!(
        settings_write(initial.clone()),
        serde_json::from_str::<Value>(SETTINGS_DEFAULT).unwrap()
    );
    assert_eq!(initial["fields"]["daily_v5"]["max"], 9999);
    assert_eq!(initial["fields"]["monthly_anlas"]["max"], 99999.0);
    let before = settings(&e).await;
    for bad in [
        json!({}),
        json!({"budgets":{"daily_v5":1}}),
        json!({"global_daily_v5":200}),
    ] {
        assert_eq!(api.json("PUT", "/settings", Some(bad)).await.0, 422);
    }
    assert_eq!(settings(&e).await, before);
    let mut proposed = settings_write(initial.clone());
    proposed["budgets"] = json!({"daily_v5":200,"daily_anlas":350,"monthly_anlas":5000.0});
    let (status, saved) = api.json("PUT", "/settings", Some(proposed.clone())).await;
    assert_eq!(status, 200, "{saved}");
    assert_eq!(settings_write(saved.clone()), proposed);
    assert_eq!(saved["fields"], initial["fields"]);
    assert_eq!(api.get_ok("/settings").await, saved);
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
        json!({"enabled": true}), // 旧的布尔写法已删除，只认 mode
        json!({"mode": "yes"}),
        json!({"mode": "all", "extra": 1}),
    ] {
        assert_eq!(admin.put("/ops/maintenance", invalid).await.status, 422);
    }
    assert_eq!(common::setting(&e.state, "maintenance_mode").await, None);

    assert_eq!(
        admin
            .put("/ops/maintenance", json!({"mode": "all"}))
            .await
            .json(),
        json!({"enabled": true, "mode": "all"})
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
            .put("/ops/maintenance", json!({"mode": "off"}))
            .await
            .json(),
        json!({"enabled": false, "mode": "off"})
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
async fn owner_maintenance_lets_the_owner_key_generate_and_blocks_others() {
    let e = setup(&[("MAINTENANCE_OWNER_IDS", SUBJECT)]).await;
    let admin = Admin::login_bare(&e.state).await;
    assert_eq!(
        admin
            .put("/ops/maintenance", json!({"mode": "owner"}))
            .await
            .json(),
        json!({"enabled": true, "mode": "owner"})
    );

    let (status, _) = generate(&e.state, TOKEN).await;
    assert_eq!(status, 200, "站长 Key 在 owner 档应能出图");

    const OTHER: &str = "nai-test-member-key-000000000000000001";
    e.state
        .db
        .create_key(
            &gate_db::repo::keys::NewKey {
                name: "其他成员".into(),
                token: OTHER.into(),
                enabled: true,
                expires_at: None,
                discord_id: Some("900000000000000002".into()),
            },
            e.state.now(),
        )
        .await
        .unwrap();
    let (status, body) = generate(&e.state, OTHER).await;
    assert_eq!(status, 503);
    assert_eq!(
        serde_json::from_slice::<Value>(&body).unwrap()["detail"],
        "本站维护中，服务暂时暂停"
    );
    let (status, _) = generate(&e.state, "nai-unknown").await;
    assert_eq!(status, 503);
    assert_eq!(
        common::send(&e.state, "GET", "/public/live", None, None, &[])
            .await
            .status,
        503
    );
}

#[tokio::test]
async fn owner_maintenance_requires_configured_owner_ids() {
    let e = setup(&[]).await;
    let admin = Admin::login_bare(&e.state).await;
    let res = admin
        .put("/ops/maintenance", json!({"mode": "owner"}))
        .await;
    assert_eq!(res.status, 422);
    assert!(res.text().contains("MAINTENANCE_OWNER_IDS"));
    assert_eq!(common::setting(&e.state, "maintenance_mode").await, None);
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
    assert_eq!(
        api.json("PUT", "/settings", Some(settings_write(full)))
            .await
            .0,
        200
    );
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
            api.json("PUT", "/settings", Some(settings_write(full.clone())))
                .await,
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
        api.json("PUT", "/settings", Some(settings_write(full.clone())))
            .await,
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
    let limits = api.get_ok("/runtime-limits").await;
    assert_eq!(
        (
            limits["queue_timeout"].clone(),
            limits["image_429_cooldown_seconds"].clone()
        ),
        (json!(5), json!(60))
    );
    assert_eq!(
        (
            limits["fields"]["queue_timeout"]["min"].clone(),
            limits["fields"]["queue_timeout"]["max"].clone()
        ),
        (json!(15), json!(300))
    );
    assert_eq!(
        (
            limits["fields"]["image_429_cooldown_seconds"]["min"].clone(),
            limits["fields"]["image_429_cooldown_seconds"]["max"].clone()
        ),
        (json!(60), json!(3600))
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
    let (status, saved) = api.json("PUT", "/runtime-limits", Some(all.clone())).await;
    assert_eq!(status, 200);
    assert_eq!(
        (
            saved["queue_timeout"].clone(),
            saved["image_429_cooldown_seconds"].clone()
        ),
        (json!(120), json!(90))
    );
    assert!(saved.get("fields").is_some());
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
    // 只改一项：响应仍带上两项当前值
    let (status, partial) = api
        .json("PUT", "/runtime-limits", Some(json!({"queue_timeout": 20})))
        .await;
    assert_eq!(status, 200);
    assert_eq!(
        (
            partial["queue_timeout"].clone(),
            partial["image_429_cooldown_seconds"].clone()
        ),
        (json!(20), json!(90))
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
    ("queue_per_account", 5, 1, 9999),
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

    let label = "账号排队上限";
    let frozen = settings(&e).await;
    for (body, detail) in [
        (
            json!({"queue_per_account": 0}),
            format!("{label} 必须在 1～9999 之间"),
        ),
        (
            json!({"queue_per_account": 10000}),
            format!("{label} 必须在 1～9999 之间"),
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
            "单号每天上限 必须在 0～9999 之间".to_string(),
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
        "每张后歇（最多） 必须在 1～30 之间",
    )
    .await;
    // 下限 1 秒：上游并发锁比响应晚一点释放（原来写死的 SETTLE_GAP 1 秒）
    api.fails(
        "PUT",
        "/guard",
        Some(json!({"post_jitter_min": 0})),
        422,
        "每张后歇（最少） 必须在 1～30 之间",
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
    assert_eq!(api.get_ok("/upstream").await["accounts"][0]["full"], true);
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

// ------------------------------------------------------------------ 公告

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
    // in_flight：没有在途的图时立刻返回 false
    assert_eq!(
        api.text("PUT", &en, Some(json!({"enabled": false}))).await,
        (200, r#"{"ok":true,"enabled":false,"in_flight":false}"#.into())
    );
    assert!(!e.state.nai.tokens().await[0].fields().admin_enabled);
    assert_eq!(
        e.state.db.get_upstream_token_enabled().await.unwrap(),
        vec![(id.clone(), false)]
    );
    // 上游页的账号行反映这些设置
    let a = &api.get_ok("/upstream").await["accounts"][0];
    assert_eq!(
        (&a["token_id"], &a["position"], &a["usable"], &a["full"], &a["admin_enabled"]),
        (&json!(id), &json!(1), &json!(false), &json!(false), &json!(false))
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
async fn failed_token_edits_preserve_live_and_restarted_membership() {
    let e = setup(&[]).await;
    let api = Admin::login_bare(&e.state).await;
    let old = e.state.nai.tokens().await[0].token_id.clone();
    let endpoint = format!("/upstream-tokens/{old}");
    let body = json!({"token":"pst-synthetic-replacement-0002"});
    let path = e.state.env.upstream_tokens_path();
    // Force the atomic file replacement to fail even when the test runs as root.
    std::fs::create_dir(&path).unwrap();
    assert_eq!(api.text("PUT", &endpoint, Some(body.clone())).await.0, 500);
    assert_eq!(e.state.nai.tokens().await[0].token_id, old);
    std::fs::remove_dir(&path).unwrap();
    e.state.db.write(|tx| tx.execute_batch("CREATE TRIGGER fail_revision BEFORE INSERT ON site_settings WHEN NEW.key='upstream_token_pool_revision' BEGIN SELECT RAISE(ABORT,'injected'); END;")).await.unwrap();
    assert_eq!(api.text("PUT", &endpoint, Some(body.clone())).await.0, 500);
    assert_eq!(e.state.nai.tokens().await[0].token_id, old);
    assert_eq!(e.restart().await.nai.tokens().await[0].token_id, old);
    e.state
        .db
        .write(|tx| tx.execute_batch("DROP TRIGGER fail_revision"))
        .await
        .unwrap();
    assert_eq!(api.text("PUT", &endpoint, Some(body)).await.0, 200);
    let new = e.state.nai.tokens().await[0].token_id.clone();
    assert_ne!(old, new);
    assert_eq!(e.restart().await.nai.tokens().await[0].token_id, new);
}

#[tokio::test]
async fn concurrent_token_additions_both_survive_restart() {
    let e = setup(&[]).await;
    let api = Admin::login_bare(&e.state).await;
    let add = |token: &str| api.text("POST", "/upstream-tokens", Some(json!({"token":token})));
    let (a, b) = tokio::join!(
        add("pst-synthetic-concurrent-0001"),
        add("pst-synthetic-concurrent-0002")
    );
    assert_eq!((a.0, b.0), (200, 200));
    let ids = |tokens: Vec<std::sync::Arc<gate_upstream::pool::Token>>| {
        tokens
            .iter()
            .map(|t| t.token_id.clone())
            .collect::<Vec<_>>()
    };
    let live = ids(e.state.nai.tokens().await);
    assert_eq!(live.len(), 3);
    assert_eq!(ids(e.restart().await.nai.tokens().await), live);
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
    // 变更提醒已删除（告警私信下线），这里不再有对外请求
    settle().await;

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

// ------------------------------------------------------------------ 状态 / 监控

/// 总览与状态码分布接口已删除（页面和接口一起走）。
#[tokio::test]
async fn dashboard_routes_are_gone() {
    let e = setup(&[]).await;
    let api = Admin::login_bare(&e.state).await;
    for path in ["/overview", "/status-codes", "/quota-records"] {
        let (status, _) = api.json("GET", path, None).await;
        assert_eq!(status, 404, "{path}");
    }
}

/// 「系统」页只剩告警渠道；生成记录页读 audit。服务器状态卡片（磁盘 / 上游健康 / 防爆破）删了，字段跟着删。

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
async fn upstream_page_fills_v5_from_snapshots_until_live_query() {
    let e = setup(&[]).await;
    let token_id = e.state.nai.tokens().await[0].token_id.clone();
    let now = e.state.now();
    e.state
        .db
        .insert_upstream_snapshot(now - 60.0, &token_id, Some(7.0), None, Some(2710), 1)
        .await
        .unwrap();
    let api = Admin::login_bare(&e.state).await;
    let page = api.get_ok("/upstream").await;
    let v5 = &page["accounts"][0]["v5"];
    assert_eq!(v5["percent"], 7);
    assert_eq!(v5["fixed_anlas"], 2710);
    assert_eq!(v5["stale"], true);
    assert!(e.mock.requests_to(SUBSCRIPTION_OK).is_empty());
}

#[tokio::test]
async fn manual_info_refresh_also_loads_subscription_battery() {
    let e = setup(&[]).await;
    e.mock.set_v5_percent(42);
    let api = Admin::login(&e.state).await;
    let csrf = api.get_ok("/upstream").await["csrf_info"]
        .as_str()
        .unwrap()
        .to_string();
    let refreshed = api
        .raw(
            "POST",
            "/upstream/refresh-info",
            Some(json!({})),
            &[("x-nai-admin-csrf", csrf.as_str())],
        )
        .await;
    assert_eq!(refreshed.status, 200, "{}", refreshed.text());
    assert_eq!(e.mock.requests_to(SUBSCRIPTION_OK).len(), 1);
    let v5 = api.get_ok("/upstream").await["accounts"][0]["v5"].clone();
    assert_eq!(v5["percent"], 42);
    assert_eq!(v5["fixed_anlas"], 10000);
    assert_eq!(v5["stale"], false);
}

#[tokio::test]
async fn v5_medium_mode_is_validated_stored_and_audited() {
    let e = setup(&[]).await;
    let api = Admin::login_bare(&e.state).await;
    for mode in ["on", "off"] {
        let mut full = api.get_ok("/settings").await;
        full["v5_medium_mode"] = json!(mode);
        assert_eq!(
            api.json("PUT", "/settings", Some(settings_write(full)))
                .await
                .0,
            200
        );
        assert_eq!(settings(&e).await["v5_medium_mode"], mode);
    }
    let frozen = settings(&e).await;
    for bad in [json!("auto"), json!("ON"), json!(true), json!(null)] {
        let mut full = api.get_ok("/settings").await;
        full["v5_medium_mode"] = bad;
        full["v5_paused"] = json!(true);
        assert_eq!(
            api.json("PUT", "/settings", Some(settings_write(full)))
                .await
                .0,
            422
        );
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
    assert_eq!(
        api.json("PUT", "/settings", Some(settings_write(full)))
            .await
            .0,
        200
    );
    assert_eq!(api.get_ok("/settings").await["v5_medium_mode"], "on");
}
