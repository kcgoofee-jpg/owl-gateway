//! 自动驾驶的两个定时任务（全站熔断、单 Key 限流）：日志 → 查询口径 → 规则 → 内存熔断 / 暂停表 / 库。
//! 规则本身的边界和不变量在 gate-core 的 `autopilot_rules.rs`。公开仓库：全部是合成数据。

mod common;
use common::*;
use gate_core::autopilot::KEY_GUARD_SKIP_REASONS;
use gate_core::pyc;
use gate_server::AppState;
use serde_json::{Value, json};

/// 2026-10-14 12:00:00 +08:00
const NOW: f64 = 1_791_950_400.0;

fn tripped(s: &AppState) -> bool {
    s.guard.with(|g| g.breaker_until) > s.now()
}

/// `n` 条出图日志：(状态, 上游状态码)。
fn logs(key_id: i64, status: &str, up: i64, n: usize, ago: f64) -> Vec<String> {
    (0..n)
        .map(|i| {
            format!(
                "INSERT INTO usage_log (ts, key_id, key_name, kind, status, up_status) \
                 VALUES ({}, {key_id}, '', 'image', '{status}', {up})",
                NOW - ago - i as f64
            )
        })
        .collect()
}

async fn breaker_logs(e: &Env) -> usize {
    settle().await;
    admin_actions(&e.state.db, 50)
        .await
        .iter()
        .filter(|r| r["action"] == "自动驾驶：熔断")
        .count()
}

// ------------------------------------------------------------------ 全站熔断

#[tokio::test]
async fn one_member_key_cannot_stop_the_whole_site() {
    let _clock = clock(Some(NOW)).await;
    let e = setup(&[]).await;
    let s = &e.state;
    let m1 = member_key(s, 1, &[]).await;
    let m2 = member_key(s, 2, &[]).await;
    // 一个成员 8 次上游 502（占比 8/12），另有 4 次成功
    let mut rows = logs(m1, "error", 502, 8, 60.0);
    rows.extend(logs(m2, "ok", 200, 4, 60.0));
    run_sql(&e, rows).await;
    let summary = run_job(s, "breaker").await.unwrap();
    assert_eq!(
        summary,
        "最近 15 分钟上游失败 8/12，只来自 1 把 Key（多半是这把 Key 的请求有问题）：不熔断"
    );
    assert!(!tripped(s), "一把 Key 停不了全站");

    // 第二个成员也遇到上游失败：这才像上游故障
    run_sql(&e, logs(m2, "error", 503, 1, 60.0)).await;
    let summary = run_job(s, "breaker").await.unwrap();
    assert_eq!(summary, "最近 15 分钟上游失败 9/13：暂停出图 5 分钟");
    assert!(tripped(s));
    assert_eq!(s.guard.with(|g| g.breaker_until), NOW + 300.0);
    assert_eq!(
        setting(s, "autopilot_breaker_until").await,
        Some(pyc::float_repr(NOW + 300.0))
    );
    assert_eq!(breaker_logs(&e).await, 1);
}

#[tokio::test]
async fn breaker_thresholds_through_the_database() {
    let _clock = clock(Some(NOW)).await;
    for (fails, oks, trip) in [(4, 6, false), (5, 12, false), (5, 11, true)] {
        let e = setup(&[]).await;
        let (a, b) = (
            member_key(&e.state, 1, &[]).await,
            member_key(&e.state, 2, &[]).await,
        );
        let mut rows = logs(a, "error", 502, fails - 1, 30.0);
        rows.extend(logs(b, "error", 500, 1, 30.0));
        rows.extend(logs(b, "ok", 200, oks, 30.0));
        run_sql(&e, rows).await;
        run_job(&e.state, "breaker").await.unwrap();
        assert_eq!(tripped(&e.state), trip, "{fails}/{}", fails + oks);
    }
}

#[tokio::test]
async fn the_window_starts_after_the_last_breaker() {
    let _clock = clock(Some(NOW)).await;
    let e = setup(&[]).await;
    let (a, b) = (
        member_key(&e.state, 1, &[]).await,
        member_key(&e.state, 2, &[]).await,
    );
    // 10 分钟前的一波失败；上次熔断 2 分钟前才结束：那一波已经处理过，不再算
    let mut rows = logs(a, "error", 502, 5, 600.0);
    rows.extend(logs(b, "error", 502, 5, 600.0));
    run_sql(&e, rows).await;
    e.state
        .db
        .set_setting("autopilot_breaker_until", &pyc::float_repr(NOW - 120.0))
        .await
        .unwrap();
    assert_eq!(
        run_job(&e.state, "breaker").await.unwrap(),
        "最近 15 分钟上游失败 0/0：正常"
    );
    assert!(!tripped(&e.state));
    // 读不懂的熔断时间：按没有失败处理，不让这一轮失败
    e.state
        .db
        .set_setting("autopilot_breaker_until", "坏")
        .await
        .unwrap();
    assert_eq!(
        run_job(&e.state, "breaker").await.unwrap(),
        "最近 15 分钟上游失败 0/0：正常"
    );
    // 没有上次熔断：15 分钟内的都算
    e.state
        .db
        .run_script(vec![
            "DELETE FROM site_settings WHERE key='autopilot_breaker_until'".into(),
        ])
        .await
        .unwrap();
    run_job(&e.state, "breaker").await.unwrap();
    assert!(tripped(&e.state));
}

#[tokio::test]
async fn an_active_breaker_is_not_tripped_again() {
    let _clock = clock(Some(NOW)).await;
    let e = setup(&[]).await;
    let (a, b) = (
        member_key(&e.state, 1, &[]).await,
        member_key(&e.state, 2, &[]).await,
    );
    let mut rows = logs(a, "error", 502, 5, 60.0);
    rows.extend(logs(b, "error", 502, 5, 60.0));
    run_sql(&e, rows).await;
    run_job(&e.state, "breaker").await.unwrap();
    assert!(tripped(&e.state));
    // 熔断中又跑了一轮（库里的熔断时间被清掉，统计窗口还是那一波失败）：不重复触发、不重复记日志
    e.state
        .db
        .run_script(vec![
            "DELETE FROM site_settings WHERE key='autopilot_breaker_until'".into(),
        ])
        .await
        .unwrap();
    gate_server::time::freeze(Some(NOW + 60.0));
    let s = run_job(&e.state, "breaker").await.unwrap();
    assert!(s.ends_with("（已在熔断中，不重复触发）"), "{s}");
    assert_eq!(e.state.guard.with(|g| g.breaker_until), NOW + 300.0);
    assert_eq!(breaker_logs(&e).await, 1);
}

#[tokio::test]
async fn which_failures_count() {
    let _clock = clock(Some(NOW)).await;
    let e = setup(&[]).await;
    let (a, b) = (
        member_key(&e.state, 1, &[]).await,
        member_key(&e.state, 2, &[]).await,
    );
    // 不算失败：429、4xx、没收到响应（0）、本地拦截（rejected 连总数都不算）
    let mut rows = Vec::new();
    for (k, up) in [(a, 429), (b, 429), (a, 400), (b, 403), (a, 0), (b, 0)] {
        rows.extend(logs(k, "error", up, 2, 60.0));
    }
    rows.extend(logs(a, "rejected", 0, 20, 60.0));
    run_sql(&e, rows).await;
    assert_eq!(
        run_job(&e.state, "breaker").await.unwrap(),
        "最近 15 分钟上游失败 0/12：正常"
    );
    // 流中途断开（上游回了 200，数据没传完）算失败
    let mut rows = logs(a, "error", 200, 3, 60.0);
    rows.extend(logs(b, "error", 200, 3, 60.0));
    run_sql(&e, rows).await;
    run_job(&e.state, "breaker").await.unwrap();
    assert!(tripped(&e.state), "6/18 = 33%，来自 2 把 Key");
}

// ------------------------------------------------------------------ 单 Key 限流

/// 这把 Key 1 小时内被拒 `n` 次（原因码 `reason`），`ago` 秒前开始往前排。
fn rejects(key_id: i64, reason: &str, n: usize, ago: f64) -> Vec<String> {
    (0..n)
        .map(|i| {
            format!(
                "INSERT INTO usage_log (ts, key_id, key_name, kind, status, reason) \
                 VALUES ({}, {key_id}, '', 'image', 'rejected', '{reason}')",
                NOW - ago - i as f64
            )
        })
        .collect()
}

fn paused(s: &AppState, id: i64) -> bool {
    s.pauses.paused(id, s.now()).is_some()
}

#[tokio::test]
async fn threshold_member_rejections_within_an_hour_pause_the_key() {
    let _clock = clock(Some(NOW)).await;
    let e = setup(&[]).await;
    let s = &e.state;
    let (k119, k120, old) = (
        member_key(s, 1, &[]).await,
        member_key(s, 2, &[]).await,
        member_key(s, 3, &[]).await,
    );
    let mut rows = rejects(k119, "member_quota", 119, 60.0);
    rows.extend(rejects(k120, "key_rate", 120, 60.0));
    // 119 次在 1 小时内，1 次在 1 小时之前：只算 119
    rows.extend(rejects(old, "member_quota", 119, 60.0));
    rows.extend(rejects(old, "member_quota", 1, 3700.0));
    run_sql(&e, rows).await;
    assert_eq!(run_job(s, "key_guard").await.unwrap(), "暂停 1 把：成员02");
    assert!(!paused(s, k119) && paused(s, k120) && !paused(s, old));
    settle().await;
    let logs: Vec<Value> = admin_actions(&s.db, 20)
        .await
        .into_iter()
        .filter(|r| r["action"] == "自动驾驶：Key 限流")
        .collect();
    assert_eq!(logs.len(), 1);
    assert_eq!(logs[0]["target"], "成员02");
    assert_eq!(
        logs[0]["detail"],
        "1 小时内被拒绝 120 次（多半是客户端在死循环重试）：暂停 1 小时"
    );
    // 已暂停的不重复暂停、不重复记日志
    assert_eq!(
        run_job(s, "key_guard").await.unwrap(),
        "1 把 Key 触发，都已在暂停中"
    );
    settle().await;
    let again = admin_actions(&s.db, 20)
        .await
        .iter()
        .filter(|r| r["action"] == "自动驾驶：Key 限流")
        .count();
    assert_eq!(again, 1);
}

/// 「连接测试」被挡（`test_request`）算成员自己的拒绝、计入单 Key 限流：
/// 正常的新手试几次（线上最多十来次）离 120 次的暂停线很远；只有客户端死循环发测试才会被暂停。
#[tokio::test]
async fn connection_test_rejections_count_but_a_few_are_harmless() {
    let _clock = clock(Some(NOW)).await;
    let e = setup(&[]).await;
    let s = &e.state;
    let (few, loops) = (member_key(s, 1, &[]).await, member_key(s, 2, &[]).await);
    let mut rows = rejects(few, "test_request", 10, 60.0);
    rows.extend(rejects(loops, "test_request", 120, 60.0));
    run_sql(&e, rows).await;
    assert_eq!(run_job(s, "key_guard").await.unwrap(), "暂停 1 把：成员02");
    assert!(!paused(s, few) && paused(s, loops));
}

#[tokio::test]
async fn site_level_rejections_do_not_count() {
    let _clock = clock(Some(NOW)).await;
    let e = setup(&[]).await;
    let k = member_key(&e.state, 1, &[]).await;
    // 17 种站点层面的原因（含 key_paused 本身、上游并发锁、网页登录门槛）各 120 次：都不是这个成员的问题
    let mut rows = Vec::new();
    for reason in KEY_GUARD_SKIP_REASONS {
        rows.extend(rejects(k, reason, 120, 60.0));
    }
    run_sql(&e, rows).await;
    assert_eq!(KEY_GUARD_SKIP_REASONS.len(), 17);
    assert_eq!(
        run_job(&e.state, "key_guard").await.unwrap(),
        "没有 Key 触发"
    );
    assert!(!paused(&e.state, k));
}

#[tokio::test]
async fn key_busy_counts_half() {
    let _clock = clock(Some(NOW)).await;
    let e = setup(&[]).await;
    let s = &e.state;
    let (busy, looping) = (member_key(s, 1, &[]).await, member_key(s, 2, &[]).await);
    // 站点拥堵时「上一张没出完」多半是排队长：180 次只算 90 次，不到 120 次的门槛
    let mut rows = rejects(busy, "key_busy", 180, 60.0);
    // 真在死循环重发：240 次 = 120 次，照样暂停
    rows.extend(rejects(looping, "key_busy", 240, 60.0));
    run_sql(&e, rows).await;
    assert_eq!(run_job(s, "key_guard").await.unwrap(), "暂停 1 把：成员02");
    assert!(!paused(s, busy) && paused(s, looping));
}

/// 每人排队已满（member_queue_full，10-22 取代 key_busy）同样只算半次。
#[tokio::test]
async fn member_queue_full_counts_half() {
    let _clock = clock(Some(NOW)).await;
    let e = setup(&[]).await;
    let s = &e.state;
    let (busy, looping) = (member_key(s, 1, &[]).await, member_key(s, 2, &[]).await);
    let mut rows = rejects(busy, "member_queue_full", 180, 60.0);
    rows.extend(rejects(looping, "member_queue_full", 240, 60.0));
    run_sql(&e, rows).await;
    assert_eq!(run_job(s, "key_guard").await.unwrap(), "暂停 1 把：成员02");
    assert!(!paused(s, busy) && paused(s, looping));
}

#[tokio::test]
async fn disabled_keys_are_never_paused() {
    let _clock = clock(Some(NOW)).await;
    let e = setup(&[]).await;
    let s = &e.state;
    let ids = [member_key(s, 3, &[("enabled", json!(0))]).await];
    let mut rows = Vec::new();
    for id in ids {
        rows.extend(rejects(id, "member_quota", 200, 60.0));
    }
    run_sql(&e, rows).await;
    assert_eq!(run_job(s, "key_guard").await.unwrap(), "没有 Key 触发");
    for id in ids {
        assert!(!paused(s, id), "Key {id}");
    }
}

#[tokio::test]
async fn a_paused_key_gets_403_until_it_expires_and_survives_a_restart() {
    let _clock = clock(Some(NOW)).await;
    let e = setup(&[]).await;
    let token = "nai-synthetic-0001";
    let k = member_key(&e.state, 1, &[]).await;
    run_sql(&e, rejects(k, "member_quota", 120, 60.0)).await;
    run_job(&e.state, "key_guard").await.unwrap();
    // 暂停 Key 不碰站长手动设的领 Key 名额上限
    assert_eq!(setting(&e.state, "register_max_users").await, None);
    let body = json!({"input": "1girl", "model": "nai-diffusion-4-5-full", "action": "generate",
                      "parameters": {"width": 832, "height": 1216, "steps": 28, "n_samples": 1}});
    let send = |state: AppState| {
        let body = body.clone();
        async move {
            let (status, _, bytes) = Call::new("POST", "/ai/generate-image")
                .bearer(token)
                .json(body)
                .send(&state)
                .await;
            (
                status.as_u16(),
                String::from_utf8_lossy(&bytes).into_owned(),
            )
        }
    };
    let (status, text) = send(e.state.clone()).await;
    assert_eq!(status, 403, "{text}");
    assert!(
        text.contains("你的 Key 短时间内被大量拒绝，疑似客户端在反复重试，已暂停 1 小时，10-14 13:00 自动恢复。"),
        "{text}"
    );
    // 暂停写进了库：重启后还在
    let rows = query(
        &e.state.db,
        &format!("SELECT paused_until FROM share_state WHERE key_id={k}"),
    )
    .await;
    assert_eq!(rows[0]["paused_until"], json!(NOW + 3600.0));
    let restarted = e.restart().await;
    assert_eq!(send(restarted.clone()).await.0, 403);
    // 到期自动恢复
    gate_server::time::freeze(Some(NOW + 3601.0));
    let (status, text) = send(restarted).await;
    assert!(!text.contains("自动恢复"), "{status} {text}");
}
