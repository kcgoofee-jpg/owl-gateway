//! 闲置回收不算站点自己出不了图的服务日。
//!
//! 冻结时钟构造三类场景：连续 3 天全站 0 成功、成员请求都 error（不回收）；平时（截止时间仍是「现在 − 3 天」）；
//! 安静时段（最近 60 分钟没有成功出图也照常判定）。时钟是全进程共享的（`time::freeze`），每个测试先拿 [`clock`] 的锁。
//! 公开仓库：全部是合成数据。

mod common;
use common::*;
use gate_server::jobs;
use serde_json::{Value, json};

const DAY: f64 = 86400.0;
/// 2026-10-14 07:00:00 +08:00：服务日 10-14 的起点。
const D0: f64 = 1_791_932_400.0;
/// 2026-10-14 12:00:00 +08:00
const NOON: f64 = D0 + 5.0 * 3600.0;

async fn setup_recycle() -> Env {
    setup(&[("KEY_INACTIVITY_DELETE_DAYS", "3")]).await
}

/// 建一把成员 Key，返回库里的主键。`last_image` 为 None 表示从没出过图。
async fn member(env: &Env, n: i64, created: f64, last_image: Option<f64>) -> i64 {
    let last = last_image.map_or(Value::Null, |t| json!(t));
    member_key(
        &env.state,
        n,
        &[("created_at", json!(created)), ("last_image_at", last)],
    )
    .await
}

async fn exists(env: &Env, id: i64) -> bool {
    env.state.db.get_key(id).await.unwrap().is_some()
}

/// 在 `[from, to)` 里均匀写 `n` 条用量日志（出图类）。
async fn logs(env: &Env, from: f64, to: f64, n: i64, status: &str, key_id: Option<i64>) {
    if n <= 0 {
        return;
    }
    let step = (to - from) / n as f64;
    let key = key_id.map_or("NULL".to_string(), |k| k.to_string());
    let (images, up) = if status == "ok" { (1, 200) } else { (0, 500) };
    env.state
        .db
        .run_script(vec![format!(
            "WITH RECURSIVE n(i) AS (SELECT 0 UNION ALL SELECT i + 1 FROM n WHERE i < {last}) \
             INSERT INTO usage_log (ts, key_id, key_name, kind, status, images, up_status) \
             SELECT {from} + i * {step}, {key}, '', 'image', '{status}', {images}, {up} FROM n",
            last = n - 1
        )])
        .await
        .unwrap();
}

/// 今天（服务日起点 `D0`）之前第 `i` 个已完结服务日的起点（i = 1 是昨天）。
fn day_start(i: i64) -> f64 {
    D0 - i as f64 * DAY
}

/// 第 `from..=to` 个已完结服务日每天 `per_day` 张成功出图。
async fn healthy_days(env: &Env, from: i64, to: i64, per_day: i64) {
    for i in from..=to {
        logs(env, day_start(i), day_start(i) + DAY, per_day, "ok", None).await;
    }
}

async fn recycle_actions(env: &Env) -> Vec<Value> {
    settle().await;
    common::admin_actions(&env.state.db, 50)
        .await
        .into_iter()
        .filter(|r| r["action"] == "闲置回收 Key")
        .collect()
}

#[tokio::test]
async fn three_dead_days_with_member_errors_do_not_recycle() {
    let _t = clock(Some(NOON)).await;
    let e = setup_recycle().await;
    // 故障前 2 小时还在出图的成员：按原来的「现在 − 3 天」会被回收
    let before_outage = member(&e, 1, NOON - 20.0 * DAY, Some(day_start(3) - 2.0 * 3600.0)).await;
    // 10 天前最后一次出图：真的闲置
    let long_idle = member(&e, 2, NOON - 20.0 * DAY, Some(NOON - 10.0 * DAY)).await;
    // 故障之前一直正常
    healthy_days(&e, 4, 31, 120).await;
    // 连续 3 天 + 今天：全站 0 成功，成员请求照样写日志（上游 401 / 熔断 → error、站长关生图 → rejected）
    for i in 1..=3 {
        logs(
            &e,
            day_start(i),
            day_start(i) + DAY,
            300,
            "error",
            Some(before_outage),
        )
        .await;
        logs(
            &e,
            day_start(i),
            day_start(i) + DAY,
            80,
            "rejected",
            Some(long_idle),
        )
        .await;
    }
    logs(&e, D0, NOON, 100, "error", Some(before_outage)).await;

    // 站点眼下还是出不了图（不在安静时段、最近 60 分钟 0 成功）→ 本轮直接跳过
    assert_eq!(
        jobs::idle_recycle_window(&e.state, 3, NOON).await.unwrap(),
        None
    );
    assert_eq!(jobs::delete_inactive_keys(&e.state).await.unwrap(), 0);
    assert!(exists(&e, before_outage).await && exists(&e, long_idle).await);
    assert!(
        recycle_actions(&e).await.is_empty(),
        "跳过只记 tracing，不写操作日志"
    );
    assert!(e.state.db.list_bans("").await.unwrap().is_empty());

    // 半小时前恢复了：今天算正常，3 个不健康的已完结服务日跳过 → 窗口变成 6 天
    logs(&e, NOON - 1800.0, NOON, 20, "ok", Some(long_idle)).await;
    let w = jobs::idle_recycle_window(&e.state, 3, NOON)
        .await
        .unwrap()
        .expect("站点在出图");
    assert_eq!((w.cutoff, w.skipped_days), (NOON - 6.0 * DAY, 3));
    assert_eq!(jobs::delete_inactive_keys(&e.state).await.unwrap(), 1);
    assert!(exists(&e, before_outage).await, "故障期间不算闲置");
    assert!(
        !exists(&e, long_idle).await,
        "故障之外也闲置满 3 天的照常回收"
    );
    let acts = recycle_actions(&e).await;
    assert_eq!(acts.len(), 1);
    assert_eq!(acts[0]["target"], format!("Key #{long_idle} 成员02"));
    assert_eq!(
        acts[0]["detail"],
        "连续 3 天未活动自动回收（不算站点没在正常出图的 3 天）"
    );
    gate_server::time::freeze(None);
}

#[tokio::test]
async fn normal_days_keep_the_plain_three_day_cutoff() {
    let _t = clock(Some(NOON)).await;
    let e = setup_recycle().await;
    let just_over = member(&e, 1, NOON - 20.0 * DAY, Some(NOON - 3.0 * DAY - 60.0)).await;
    let just_under = member(&e, 2, NOON - 20.0 * DAY, Some(NOON - 3.0 * DAY + 60.0)).await;
    let never = member(&e, 3, NOON - 4.0 * DAY, None).await;
    let fresh = member(&e, 4, NOON - 2.0 * DAY, None).await;
    // 刚好 50 张也算健康
    healthy_days(&e, 1, 31, 50).await;
    logs(&e, D0, NOON, 40, "ok", None).await;
    // 平时也有失败和被拒，不影响判断
    logs(&e, day_start(2), day_start(1), 30, "error", Some(just_over)).await;

    let w = jobs::idle_recycle_window(&e.state, 3, NOON)
        .await
        .unwrap()
        .unwrap();
    assert_eq!((w.cutoff, w.skipped_days), (NOON - 3.0 * DAY, 0));
    assert_eq!(jobs::delete_inactive_keys(&e.state).await.unwrap(), 2);
    assert!(!exists(&e, just_over).await);
    assert!(exists(&e, just_under).await);
    assert!(!exists(&e, never).await);
    assert!(exists(&e, fresh).await);
    let acts = recycle_actions(&e).await;
    assert_eq!(acts.len(), 2);
    assert!(
        acts.iter()
            .all(|a| a["detail"] == "连续 3 天未活动自动回收"),
        "平时的文案不变"
    );
    gate_server::time::freeze(None);
}

#[tokio::test]
async fn one_dead_day_below_threshold_adds_one_day() {
    let _t = clock(Some(NOON)).await;
    let e = setup_recycle().await;
    let idle_3_5 = member(&e, 1, NOON - 20.0 * DAY, Some(NOON - 3.5 * DAY)).await;
    let idle_4_5 = member(&e, 2, NOON - 20.0 * DAY, Some(NOON - 4.5 * DAY)).await;
    healthy_days(&e, 1, 31, 200).await;
    // 前天只出了 49 张（不到 50）：这一天不算
    e.state
        .db
        .run_script(vec![format!(
            "DELETE FROM usage_log WHERE ts>={} AND ts<{}",
            day_start(2),
            day_start(1)
        )])
        .await
        .unwrap();
    logs(&e, day_start(2), day_start(1), 49, "ok", None).await;
    logs(&e, NOON - 600.0, NOON, 5, "ok", None).await;
    let w = jobs::idle_recycle_window(&e.state, 3, NOON)
        .await
        .unwrap()
        .unwrap();
    assert_eq!((w.cutoff, w.skipped_days), (NOON - 4.0 * DAY, 1));
    assert_eq!(jobs::delete_inactive_keys(&e.state).await.unwrap(), 1);
    assert!(exists(&e, idle_3_5).await);
    assert!(!exists(&e, idle_4_5).await);
    gate_server::time::freeze(None);
}

#[tokio::test]
async fn expired_keys_are_not_idle() {
    let _t = clock(Some(NOON)).await;
    let e = setup_recycle().await;
    let expired = member(&e, 1, NOON - 20.0 * DAY, Some(NOON - 10.0 * DAY)).await;
    let expires_later = member(&e, 2, NOON - 20.0 * DAY, Some(NOON - 10.0 * DAY)).await;
    e.state
        .db
        .run_script(vec![
            format!(
                "UPDATE api_keys SET expires_at={} WHERE id={expired}",
                NOON - 5.0 * DAY
            ),
            format!(
                "UPDATE api_keys SET expires_at={} WHERE id={expires_later}",
                NOON + 5.0 * DAY
            ),
            format!(
                "INSERT INTO discord_registrations (discord_id, key_id, created_at) VALUES ('9000000000000000071', {expired}, 1.0)"
            ),
        ])
        .await
        .unwrap();
    healthy_days(&e, 1, 31, 200).await;
    logs(&e, NOON - 600.0, NOON, 5, "ok", None).await;
    // 查询本身：过期的不在闲置名单里
    let cutoff = NOON - 3.0 * DAY;
    assert_eq!(
        e.state.db.inactive_key_ids(cutoff, NOON).await.unwrap(),
        vec![expires_later]
    );
    assert_eq!(
        e.state
            .db
            .inactive_key_ids(cutoff, NOON - 6.0 * DAY)
            .await
            .unwrap(),
        vec![expired, expires_later],
        "还没到期时照常算闲置"
    );
    assert_eq!(jobs::delete_inactive_keys(&e.state).await.unwrap(), 1);
    assert!(exists(&e, expired).await);
    assert!(!exists(&e, expires_later).await);
    assert!(
        e.state.db.list_bans("").await.unwrap().is_empty(),
        "过期 Key 的领取人不进封禁表"
    );
    assert_eq!(
        e.state
            .db
            .registration_key_id("9000000000000000071")
            .await
            .unwrap(),
        Some(expired),
        "领取记录留着，等 /register 时释放"
    );
    gate_server::time::freeze(None);
}
