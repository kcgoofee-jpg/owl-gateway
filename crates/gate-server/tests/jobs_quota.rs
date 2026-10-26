//! 定时任务「额度同步」与手动额度（站长 10-23：全部自动额度算法删除，额度都是站长手动定的数）。
//! 真实 SQLite + 假上游 + 冻结时钟。公开仓库：全部是合成数据；上游是本地假服务。

mod common;
use common::*;
use serde_json::{Value, json};

/// 2026-10-14 12:00:00 +08:00
const NOON: f64 = 1_791_950_400.0;

async fn quota(e: &Env) -> String {
    run_job(&e.state, "quota")
        .await
        .expect("额度同步这一轮应该成功")
}

async fn key_row(e: &Env, id: i64) -> Value {
    e.state.db.get_key(id).await.unwrap().unwrap()
}

fn v5_body() -> Value {
    json!({
        "input": "1girl", "model": "nai-diffusion-5-full", "action": "generate",
        "parameters": {"width": 832, "height": 1216, "steps": 23, "scale": 5, "n_samples": 1, "seed": 1},
    })
}

/// V4.5 每人每天就是手动的那个数：老成员（以前算法给过 175）和新号（以前爬坡只给一半）一样，没设过是 150。
/// 手动设过额度的 Key 不动。
#[tokio::test]
async fn v45_limit_is_the_manual_number_for_new_and_old_members() {
    let _clock = clock(Some(NOON)).await;
    let e = setup(&[]).await;
    let old = member_key(&e.state, 1, &[("daily_images", json!(175))]).await;
    // 新号：Discord 账号刚注册（以前新号爬坡会打折）
    let new = member_key(
        &e.state,
        2,
        &[
            ("daily_images", json!(75)),
            ("discord_id", json!("'1428000000000000000'")),
        ],
    )
    .await;
    let manual = member_key(
        &e.state,
        3,
        &[("quota_auto", json!(-1)), ("daily_images", json!(33))],
    )
    .await;
    let s = quota(&e).await;
    assert!(s.contains("V4.5 每人 150"), "{s}");
    assert!(s.contains("同步"), "{s}");
    for id in [old, new] {
        let k = key_row(&e, id).await;
        assert_eq!(
            (&k["daily_images"], &k["daily_v5"], &k["image_model_scope"]),
            (&json!(150), &json!(0), &json!("legacy")),
            "Key {id}"
        );
    }
    assert_eq!(key_row(&e, manual).await["daily_images"], 33);
    assert_eq!(
        setting(&e.state, "algo_notice").await.unwrap(),
        "今日额度：V4.5 每人 150 张/天（每日 7 点重置） · V5 每人不限（全站今日共 150 张，先到先得） · Anlas 每人 50/天"
    );
    // 再跑一轮：没有要同步的了
    assert!(!quota(&e).await.contains("同步"));

    // 新号鉴权后看到的也是满额（新号爬坡已删）
    let (status, _, me) = Call::new("GET", "/v1/me")
        .bearer("nai-synthetic-0002")
        .send(&e.state)
        .await;
    if status == 200 {
        let me: Value = serde_json::from_slice(&me).unwrap();
        assert!(!me.to_string().contains("逐步放开"), "{me}");
    }

    // 后台改手动值：立即同步，不等下一轮
    let api = Admin::login_bare(&e.state).await;
    let (status, v) = api
        .json(
            "PUT",
            "/settings",
            Some(json!({"register_daily_images": 120, "register_daily_v5": 5})),
        )
        .await;
    assert_eq!(status, 200, "{v}");
    for id in [old, new] {
        let k = key_row(&e, id).await;
        assert_eq!(
            (&k["daily_images"], &k["daily_v5"]),
            (&json!(120), &json!(5))
        );
    }
    let (_, g) = api.json("GET", "/settings", None).await;
    assert_eq!(
        (
            &g["register_daily_images"],
            &g["register_daily_v5"],
            &g["v5_paused"]
        ),
        (&json!(120), &json!(5), &json!(false))
    );
    // 领 Key 设置里不能再改每人额度（只在「额度与保护」里改）
    let (status, _) = api
        .json(
            "PUT",
            "/ops/registration",
            Some(json!({"daily_images": 10})),
        )
        .await;
    assert_eq!(status, 400);
}

/// V5 暂停（手动开关）：本地直接拒（403、v5_exhausted、不发上游），首页 / 成员页显示暂停，
/// 客户端看到的订阅信息是 0 张，不是「不限」。
#[tokio::test]
async fn manual_v5_pause_refuses_and_shows_zero_everywhere() {
    let _clock = clock(Some(NOON)).await;
    let e = setup(&[]).await;
    member_key(&e.state, 900, &[("image_model_scope", json!("all"))]).await;
    // 第二把 Key（不计全站 V5 池）：V5 暂停对每把 Key 都生效
    member_key(
        &e.state,
        901,
        &[
            ("image_model_scope", json!("all")),
            ("exclude_global_v5", json!(1)),
        ],
    )
    .await;
    // 全站和个人都不限：平时订阅信息是「不限」
    set_setting(&e, "global_daily_v5", "0").await;
    set_setting(&e, "register_daily_v5", "0").await;
    set_setting(&e, "quota_modes_v1", r#"{"register_daily_images":{"mode":"unlimited"},"register_daily_v5":{"mode":"unlimited"},"global_daily_v5":{"mode":"unlimited"},"global_daily_anlas":{"mode":"unlimited"},"member_daily_anlas":{"mode":"unlimited"},"global_monthly_anlas":{"mode":"unlimited"},"public_vibe_enabled":false}"#).await;
    quota(&e).await;
    assert_eq!(
        e.state
            .db
            .get_key_by_token("nai-synthetic-0900")
            .await
            .unwrap()
            .unwrap()["image_model_scope"],
        "all"
    );
    let sub = |e: &Env| {
        let state = e.state.clone();
        async move {
            let (status, _, body) = Call::new("GET", "/user/subscription")
                .bearer("nai-synthetic-0900")
                .send(&state)
                .await;
            assert_eq!(status, 200);
            serde_json::from_slice::<Value>(&body).unwrap()["naiGate"].clone()
        }
    };
    assert_eq!(sub(&e).await["v5Unlimited"], true);

    let api = Admin::login_bare(&e.state).await;
    let (status, v) = api
        .json("PUT", "/settings", Some(json!({"v5_paused": true})))
        .await;
    assert_eq!(status, 200, "{v}");
    let (status, _) = api
        .json("PUT", "/settings", Some(json!({"v5_paused": "yes"})))
        .await;
    assert_eq!(status, 422);

    let g = sub(&e).await;
    assert_eq!(
        (
            &g["v5Unlimited"],
            &g["v5LeftToday"],
            &g["v5DailyLimit"],
            &g["v5Paused"]
        ),
        (&json!(false), &json!(0), &json!(0), &json!(true))
    );
    assert!(
        setting(&e.state, "algo_notice")
            .await
            .unwrap()
            .contains("V5 暂停中")
    );
    // 10-27 起看板不再带 V5 暂停 / 余量字段：暂停状态通过 algo_notice 与拒绝原因（下面 403）体现
    let (_, _, live) = Call::new("GET", "/public/live").send(&e.state).await;
    let live: Value = serde_json::from_slice(&live).unwrap();
    assert!(live["quota"].get("v5_paused").is_none());
    assert!(live["rules"].get("quota").is_none());

    let (status, _, resp) = Call::new("POST", "/ai/generate-image")
        .bearer("nai-synthetic-0900")
        .json(v5_body())
        .send(&e.state)
        .await;
    let resp = String::from_utf8_lossy(&resp).to_string();
    assert_eq!(status, 403, "{resp}");
    assert!(resp.contains(gate_core::quota::V5_PAUSED_TEXT), "{resp}");
    let (status, _, resp) = Call::new("POST", "/ai/generate-image")
        .bearer("nai-synthetic-0901")
        .json(v5_body())
        .send(&e.state)
        .await;
    let resp = String::from_utf8_lossy(&resp).to_string();
    assert_eq!(status, 403, "every key must obey the V5 pause: {resp}");
    assert!(e.mock.requests_to("/ai/generate-image").is_empty());
    settle().await;
    let rows = query(
        &e.state.db,
        "SELECT reason, anlas FROM usage_log WHERE model LIKE '%diffusion-5%' AND status='rejected'",
    )
    .await;
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert!(
        rows.iter().all(|r| r["reason"] == "v5_exhausted"),
        "{rows:?}"
    );

    // 关掉暂停：恢复
    api.json("PUT", "/settings", Some(json!({"v5_paused": false})))
        .await;
    assert_eq!(sub(&e).await["v5Unlimited"], true);
    assert!(!gate_server::ops::v5_paused(&e.state.db).await.unwrap());
}

/// 旧的自动额度设置留在库里也不再读：V4.5 不按 `quota_ceiling`，V5 不按当天方案 / 自动暂停标记。
#[tokio::test]
async fn old_algorithm_settings_are_ignored() {
    let _clock = clock(Some(NOON)).await;
    let e = setup(&[]).await;
    let id = member_key(&e.state, 1, &[]).await;
    for (k, v) in [
        ("quota_ceiling", "300"),
        ("quota_auto_enabled", "0"),
        ("quota_v5_paused", r#"{"day": "2026-10-14", "reason": "x"}"#),
        (
            "quota_v5_day_plan",
            r#"{"day": "2026-10-14", "plan": {"global": 9, "each": 9}}"#,
        ),
        ("newbie_ramp_days", "30"),
    ] {
        set_setting(&e, k, v).await;
    }
    quota(&e).await;
    let k = key_row(&e, id).await;
    assert_eq!(
        (&k["daily_images"], &k["daily_v5"]),
        (&json!(150), &json!(0))
    );
    assert!(!gate_server::ops::v5_paused(&e.state.db).await.unwrap());
}
