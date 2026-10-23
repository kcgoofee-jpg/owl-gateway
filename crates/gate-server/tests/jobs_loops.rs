//! 定时任务的循环：闲置回收（线上开关关着时绝不删 Key）、维护一轮、启动 / 关停序列，以及
//! 「任务出错或 panic 时循环不退出」。各任务本身的规则见 `jobs_quota.rs` / `jobs_capacity.rs` / `jobs_autopilot.rs`。
//! 公开仓库：全部是合成数据；Discord / NovelAI / 官方状态页都不出本机。

mod common;
use common::*;
use futures::future::BoxFuture;
use gate_server::AppState;
use gate_server::jobs::{self, JOBS, Job, JobError};
use gate_testkit::MockUpstream;
use serde_json::{Value, json};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

const DAY: f64 = 86400.0;

/// 建一把成员 Key（`created_days_ago` 天前创建），返回库里的主键。
async fn key(env: &Env, n: i64, created_days_ago: f64, extra: &[(&str, Value)]) -> i64 {
    let mut cols = vec![(
        "created_at",
        json!(env.state.now() - created_days_ago * DAY),
    )];
    cols.extend(extra.iter().cloned());
    member_key(&env.state, n, &cols).await
}

async fn key_exists(env: &Env, id: i64) -> bool {
    env.state.db.get_key(id).await.unwrap().is_some()
}

/// 注册服务需要的环境（Discord 指到本机假服务；配了「已领 Key」身份组，这样摘身份组会真的发请求）。
const GUILD: &str = "9000000000000000099";
const ROLE: &str = "9000000000000000098";

fn discord_env<'a>(base: &'a str, days: &'a str) -> Vec<(&'a str, &'a str)> {
    vec![
        ("KEY_INACTIVITY_DELETE_DAYS", days),
        ("DISCORD_API_BASE", base),
        ("DISCORD_CLIENT_ID", "100000000"),
        ("DISCORD_CLIENT_SECRET", "synthetic-secret"),
        ("DISCORD_BOT_TOKEN", "synthetic-bot-token"),
        (
            "REGISTRATION_BRIDGE_SECRET",
            "0123456789abcdef0123456789abcdef",
        ),
        ("DISCORD_GUILD_ID", GUILD),
        ("DISCORD_MEMBER_ROLE_ID", ROLE),
        ("SITE_URL", "https://owl.example"),
    ]
}

/// 平时的出图记录：往前 32 天每 20 分钟一张成功出图（每个服务日 72 张，≥ 健康线 50 张；最近一张在 30 秒前）。
/// 闲置回收不算站点出不了图的服务日，所以「平时」场景要先有正常出图的历史。
async fn seed_healthy_history(env: &Env) {
    let now = env.state.now();
    env.state
        .db
        .run_script(vec![format!(
            "WITH RECURSIVE n(i) AS (SELECT 0 UNION ALL SELECT i + 1 FROM n WHERE i < {}) \
             INSERT INTO usage_log (ts, key_id, key_name, kind, status, images) \
             SELECT {now} - 30 - i * 1200, NULL, '', 'image', 'ok', 1 FROM n",
            32 * 72
        )])
        .await
        .unwrap();
}

fn role_removals(discord: &MockUpstream) -> Vec<String> {
    discord
        .requests()
        .into_iter()
        .filter(|r| r.method == "DELETE" && r.path.contains("/roles/"))
        .map(|r| r.path)
        .collect()
}

// ------------------------------------------------------------------ 闲置回收

#[tokio::test]
async fn idle_recycle_switched_off_never_deletes_anything() {
    // 关掉回收（KEY_INACTIVITY_DELETE_DAYS=0；线上 10-13 起是 3，开着）
    let discord = MockUpstream::start().await;
    let e = setup(&discord_env(&discord.url(), "0")).await;
    let id1 = key(&e, 1, 400.0, &[]).await;
    let id2 = key(
        &e,
        2,
        90.0,
        &[("last_image_at", json!(e.state.now() - 80.0 * DAY))],
    )
    .await;
    e.state
        .db
        .run_script(vec![format!(
            "INSERT INTO discord_registrations (discord_id, key_id, created_at) VALUES ('9000000000000000001', {id1}, 1.0)"
        )])
        .await
        .unwrap();
    assert_eq!(jobs::delete_inactive_keys(&e.state).await.unwrap(), 0);
    assert!(key_exists(&e, id1).await && key_exists(&e, id2).await);
    settle().await;
    assert!(
        common::admin_actions(&e.state.db, 10).await.is_empty(),
        "什么都没做就不该有操作日志"
    );
    assert!(discord.requests().is_empty(), "不删 Key 就不碰 Discord");
    assert!(e.state.db.list_bans("").await.unwrap().is_empty());
    assert_eq!(
        e.state
            .db
            .key_row_for_discord("9000000000000000001")
            .await
            .unwrap()
            .map(|k| k["id"].clone()),
        Some(json!(id1)),
        "领取记录也不动"
    );

    // 通过真正的循环也一样：起一遍所有循环，等闲置回收跑完第一轮再关
    e.state
        .jobs
        .set_official_api(Some("http://127.0.0.1:9/never".into()));
    jobs::startup(&e.state).await.unwrap();
    let start = Instant::now();
    while Value::Object(e.state.jobs.last().await)["idle_recycle"]["summary"] != "关闭（天数=0）"
    {
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "闲置回收没有在启动后立即跑第一轮"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    jobs::shutdown(&e.state).await;
    assert!(key_exists(&e, id1).await && key_exists(&e, id2).await);
    assert!(role_removals(&discord).is_empty());
}

#[tokio::test]
async fn idle_recycle_deletes_and_logs_without_banning_when_switched_on() {
    let discord = MockUpstream::start().await;
    let e = setup(&discord_env(&discord.url(), "3")).await;
    let now = e.state.now();
    let mut ids = Vec::new();
    for (n, days, extra) in [
        (1, 10.0, vec![]), // 从没生过图，创建 10 天 → 闲置
        (2, 10.0, vec![("last_image_at", json!(now - 4.0 * DAY))]), // 4 天没生图 → 闲置
        (3, 10.0, vec![("last_image_at", json!(now - 3600.0))]), // 活跃
        (4, 10.0, vec![("last_image_at", json!(now - 2.0 * DAY))]), // 2 天没生图 → 未到 3 天
        (5, 10.0, vec![("last_image_at", json!(now - DAY))]), // 1 天没生图 → 未到 3 天
        (6, 10.0, vec![]), // 邀请标签豁免
        (7, 1.0, vec![]),  // 刚创建
        // 已过期：过期后请求一律 403、last_image_at 不再更新；不算闲置，照旧等 /register 时释放
        (8, 10.0, vec![("expires_at", json!(now - 4.0 * DAY))]),
        // 还没到期的期限 Key 照常判定
        (9, 10.0, vec![("expires_at", json!(now + 30.0 * DAY))]),
    ] {
        ids.push(key(&e, n, days, &extra).await);
    }
    seed_healthy_history(&e).await;
    let (id1, id2, id6, id8) = (ids[0], ids[1], ids[5], ids[7]);
    e.state
        .db
        .run_script(vec![
            format!("INSERT INTO key_tags (key_id, tag, note, by, ts) VALUES ({id6}, '邀请', '', 'x', 1.0)"),
            format!(
                "INSERT INTO discord_registrations (discord_id, key_id, created_at, username, display_name, avatar) \
                 VALUES ('9000000000000000001', {id1}, 1.0, 'user_one', '一号', 'av1')"
            ),
            format!("INSERT INTO discord_registrations (discord_id, key_id, created_at) VALUES ('9000000000000000002', {id2}, 1.0)"),
            format!("INSERT INTO discord_registrations (discord_id, key_id, created_at) VALUES ('9000000000000000008', {id8}, 1.0)"),
        ])
        .await
        .unwrap();
    // 通过定时任务跑一轮：摘要写明回收了几把
    let summary = run_job(&e.state, "idle_recycle").await.unwrap();
    assert_eq!(summary, "回收 3 把");
    let mut gone = Vec::new();
    for (n, id) in (1..=9).zip(&ids) {
        if !key_exists(&e, *id).await {
            gone.push(n);
        }
    }
    assert_eq!(gone, vec![1, 2, 9]);
    for (n, alive) in [
        (1, false),
        (2, false),
        (3, true),
        (4, true),
        (5, true),
        (6, true),
        (7, true),
        (8, true),
        (9, false),
    ] {
        assert_eq!(key_exists(&e, ids[n - 1]).await, alive, "Key {n}");
    }
    // 只删 Key、不拉黑（站长 10-23）：被回收的人之后还能再 /register 领新 Key
    assert!(e.state.db.list_bans("").await.unwrap().is_empty());
    for id in ["9000000000000000001", "9000000000000000002"] {
        assert_eq!(e.state.db.discord_ban_reason(id).await.unwrap(), None);
    }
    // 过期 Key 的领取记录不动（照旧等 /register 时释放）
    assert_eq!(
        e.state
            .db
            .registration_key_id("9000000000000000008")
            .await
            .unwrap(),
        Some(id8)
    );
    // 领取记录随 Key 一起删；身份组摘一次（`delete_key` 已经带走了领取记录，之后的「释放领取记录」没有可释放的）
    assert_eq!(
        e.state
            .db
            .registration_key_id("9000000000000000001")
            .await
            .unwrap(),
        None
    );
    let removals = role_removals(&discord);
    for id in ["9000000000000000001", "9000000000000000002"] {
        let want = format!("/guilds/{GUILD}/members/{id}/roles/{ROLE}");
        assert_eq!(
            removals.iter().filter(|p| **p == want).count(),
            1,
            "{removals:?}"
        );
    }
    // 操作日志
    let rows = common::admin_actions(&e.state.db, 10).await;
    let del: Vec<&Value> = rows
        .iter()
        .filter(|r| r["action"] == "闲置回收 Key")
        .collect();
    assert_eq!(del.len(), 3);
    assert!(
        del.iter()
            .any(|r| r["target"] == format!("Key #{id1} 成员01")
                && r["detail"] == "连续 3 天未活动自动回收"
                && r["actor"] == "系统")
    );
    // 再跑一遍：没有可回收的了
    assert_eq!(jobs::delete_inactive_keys(&e.state).await.unwrap(), 0);
}

#[tokio::test]
async fn idle_recycle_negative_days_is_off_too() {
    let e = setup(&[("KEY_INACTIVITY_DELETE_DAYS", "-5")]).await;
    let id = key(&e, 1, 400.0, &[]).await;
    assert_eq!(jobs::delete_inactive_keys(&e.state).await.unwrap(), 0);
    assert!(key_exists(&e, id).await);
}

// ------------------------------------------------------------------ 维护循环一轮

#[tokio::test]
async fn maintenance_round_purges_and_flushes() {
    let e = setup(&[("USAGE_LOG_RETENTION_DAYS", "10")]).await;
    let s = &e.state;
    s.jobs
        .set_official_api(Some("http://127.0.0.1:9/never".into()));
    let now = s.now();
    let old_hour = ((now - 40.0 * DAY) / 3600.0).floor() as i64 * 3600;
    s.db.run_script(vec![
        format!("INSERT INTO usage_log (ts, key_id, key_name, kind, status) VALUES ({}, NULL, '', 'text', 'ok')", now - 20.0 * DAY),
        format!("INSERT INTO usage_log (ts, key_id, key_name, kind, status) VALUES ({}, NULL, '', 'text', 'ok')", now - 2.0 * DAY),
        format!("INSERT INTO admin_actions (ts, actor, action, target, detail, ok) VALUES ({}, 'a', '旧', '', '', 1)", now - 200.0 * DAY),
        format!("INSERT INTO admin_actions (ts, actor, action, target, detail, ok) VALUES ({}, 'a', '新', '', '', 1)", now - 2.0 * DAY),
        format!("INSERT INTO status_hourly (hour, code, n) VALUES ({old_hour}, 200, 5)"),
        format!("INSERT INTO error_events (sig, source, first_seen, last_seen, resolved_at, count) VALUES ('old', 's', 1.0, {0}, {0}, 1)", now - 40.0 * DAY),
        format!("INSERT INTO error_events (sig, source, first_seen, last_seen, resolved_at, count) VALUES ('open', 's', 1.0, {0}, NULL, 1)", now - 40.0 * DAY),
    ])
    .await
    .unwrap();
    s.status_stats.record(200, now);
    s.status_stats.record(200, now);
    s.status_stats.record(429, now);

    assert_eq!(run_job(s, "maintenance").await.unwrap(), "全部完成");
    settle().await;

    // 明细日志按 USAGE_LOG_RETENTION_DAYS 只留 10 天内（设置了才清）；操作日志、状态码统计、旧 Bug 一律留存（站长 10-22）
    let (usage_rows, actions, errors) =
        s.db.read(|c| {
            let usage_rows: i64 =
                c.query_row("SELECT COUNT(*) FROM usage_log", [], |r| r.get(0))?;
            let actions = c
                .prepare("SELECT action FROM admin_actions ORDER BY rowid")?
                .query_map([], |r| r.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            let errors = c
                .prepare("SELECT sig FROM error_events ORDER BY rowid")?
                .query_map([], |r| r.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            Ok((usage_rows, actions, errors))
        })
        .await
        .unwrap();
    assert_eq!(usage_rows, 1);
    assert_eq!(actions, ["旧", "新"]);
    assert!(
        errors.iter().any(|e| e == "open") && errors.iter().any(|e| e == "old"),
        "{errors:?}"
    );
    // 内存里的状态码统计落库并清空
    let hour = (now / 3600.0).floor() as i64 * 3600;
    let totals = s.db.status_totals_since(hour).await.unwrap();
    assert!(
        totals.contains(&(200, 2)) && totals.contains(&(429, 1)),
        "{totals:?}"
    );
    assert!(s.status_stats.pending().is_empty());
    let kept: i64 =
        s.db.read(move |c| {
            c.query_row(
                "SELECT COUNT(*) FROM status_hourly WHERE hour=?",
                [old_hour],
                |r| r.get(0),
            )
        })
        .await
        .unwrap();
    assert_eq!(kept, 1);
}

#[tokio::test]
async fn flush_status_stats_puts_counts_back_when_the_write_fails() {
    let e = setup(&[]).await;
    let s = &e.state;
    s.status_stats.record(200, s.now());
    // 让写入失败：把表删掉
    s.db.run_script(vec!["DROP TABLE status_hourly".into()])
        .await
        .unwrap();
    assert!(jobs::flush_status_stats(s).await.is_err());
    let pending = s.status_stats.pending();
    assert_eq!(
        pending.iter().map(|(_, n)| n).sum::<i64>(),
        1,
        "写失败要放回去下次再写：{pending:?}"
    );
}

// ------------------------------------------------------------------ 维护里一步出错

#[tokio::test]
async fn a_failing_maintenance_step_is_shown_but_filed_only_once() {
    let e = setup(&[]).await;
    let s = &e.state;
    s.jobs
        .set_official_api(Some("http://127.0.0.1:9/never".into()));
    // 让清理这一步失败：把状态码统计表删掉，再留一笔要落库的统计
    s.db.run_script(vec!["DROP TABLE status_hourly".into()])
        .await
        .unwrap();
    s.status_stats.record(200, s.now());
    let err = run_job(s, "maintenance").await.unwrap_err();
    assert!(err.to_string().starts_with("清理过期记录出错："), "{err}");
    let last = Value::Object(s.jobs.last().await);
    assert_eq!(last["maintenance"]["ok"], false);
    // 只有那一步自己的 Bug（来源 maintenance:purge），不再另记一条 job:maintenance
    assert_eq!(bug_sources(s).await, ["maintenance:purge"]);
}

// ------------------------------------------------------------------ 启动 / 关停

#[tokio::test]
async fn startup_runs_every_job_and_shutdown_finishes_promptly() {
    let e = setup(&[]).await;
    let s = &e.state;
    s.jobs
        .set_official_api(Some("http://127.0.0.1:9/never".into()));
    // 上次的运行记录在启动时读回来（包括已经不存在的任务）
    s.db.set_setting(
        "job_last",
        &json!({"retired_job": {"at": 1.0, "ok": true, "ms": 0, "summary": "旧"}}).to_string(),
    )
    .await
    .unwrap();
    jobs::startup(s).await.unwrap();
    assert!(s.jobs.last().await.contains_key("retired_job"));
    // 每个任务的第一轮都立即执行（不等一个周期）
    let start = Instant::now();
    loop {
        let raw = setting(s, "job_last").await.unwrap_or_default();
        let saved: Value = serde_json::from_str(&raw).unwrap_or(Value::Null);
        if JOBS.iter().all(|j| saved.get(j.name).is_some()) && saved.get("retired_job").is_some() {
            break;
        }
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "定时任务没有在启动后立即跑第一轮：{raw}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    // 7 个循环都还挂着（跑完第一轮在睡觉，没有退出）
    assert!(s.tasks.len() >= JOBS.len(), "{}", s.tasks.len());
    // 关停前内存里还有没落库的状态码统计
    s.status_stats.record(503, s.now());
    let t = Instant::now();
    jobs::shutdown(s).await;
    assert!(
        t.elapsed() < Duration::from_secs(5),
        "关停太慢：{:?}",
        t.elapsed()
    );
    assert!(s.shutdown.is_cancelled() && s.tasks.is_closed() && s.tasks.is_empty());
    let hour = (s.now() / 3600.0).floor() as i64 * 3600;
    assert!(
        s.db.status_totals_since(hour)
            .await
            .unwrap()
            .contains(&(503, 1)),
        "关停时要把状态码统计落库"
    );
}

// ------------------------------------------------------------------ 出错 / panic 不让循环退出

static FLAKY_CALLS: AtomicUsize = AtomicUsize::new(0);

/// 第一轮 panic，第二轮报错，之后正常。
fn flaky(_: &AppState) -> BoxFuture<'_, Result<String, JobError>> {
    Box::pin(async {
        match FLAKY_CALLS.fetch_add(1, Ordering::SeqCst) {
            0 => panic!("合成的 panic"),
            1 => Err(JobError::new("ValueError", "合成的错误")),
            n => Ok(format!("第 {} 轮", n + 1)),
        }
    })
}

static FLAKY: Job = Job {
    name: "flaky_test",
    title: "测试用",
    every: "每 0.05 秒",
    every_secs: 0.05,
    jitter: 0.0,
    run: flaky,
};

#[tokio::test]
async fn a_failing_or_panicking_job_keeps_its_loop_alive() {
    let e = setup(&[]).await;
    jobs::spawn_job(&e.state, &FLAKY);
    let start = Instant::now();
    loop {
        let last = Value::Object(e.state.jobs.last().await);
        if last["flaky_test"]["ok"] == true {
            break;
        }
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "循环在出错后退出了：{last}"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(FLAKY_CALLS.load(Ordering::SeqCst) >= 3);
    // panic 和报错各记一条 Bug（来源 job:<名字>）
    let mut sources = bug_sources(&e.state).await;
    sources.dedup();
    assert_eq!(sources, ["job:flaky_test"]);
    let events = e.state.db.list_error_events(true, 10).await.unwrap();
    assert_eq!(events.len(), 2, "{events:?}");
    jobs::shutdown(&e.state).await;
    assert!(e.state.tasks.is_empty(), "关停后循环退出");
}

#[tokio::test]
async fn a_panic_becomes_a_job_error() {
    let r: Result<String, JobError> = jobs::guarded(async { panic!("合成的 panic") }).await;
    let err = r.unwrap_err();
    assert_eq!(
        (err.ty.as_str(), err.msg.as_str()),
        ("Panic", "合成的 panic")
    );
    let r = jobs::guarded(async { Ok::<_, JobError>("好".to_string()) }).await;
    assert_eq!(r.unwrap(), "好");
}

// ------------------------------------------------------------------ 停机检测

/// 重启前网关「最后一次有动静」的时间：请求日志在 `log_ago` 秒前，定时任务在 `job_ago` 秒前跑完。
async fn restart_after(log_ago: f64, job_ago: f64) -> (Option<String>, usize) {
    let now = 1_900_000_000.0;
    let _c = clock(Some(now)).await;
    let e = setup(&[]).await;
    let job_last =
        json!({"maintenance": {"at": now - job_ago, "ok": true, "ms": 3, "summary": "全部完成"}});
    e.state
        .db
        .write(move |tx| {
            tx.execute(
                "INSERT INTO usage_log(ts, key_id, key_name, kind, status) VALUES (?, 1, 'k', 'image', 'ok')",
                [now - log_ago],
            )?;
            tx.execute(
                "INSERT OR REPLACE INTO site_settings(key, value) VALUES ('job_last', ?)",
                [job_last.to_string()],
            )
        })
        .await
        .unwrap();
    let st = e.restart().await;
    let grace = setting(&st, "key_inactivity_grace_started_at").await;
    let resets = admin_actions(&st.db, 50)
        .await
        .iter()
        .filter(|a| a["action"] == "闲置计时重置")
        .count();
    (grace, resets)
}

/// 安静时段凌晨没人发请求、但定时任务一直在跑：重启不算停机，闲置计时不重置（10-14 清晨就这样误判过）。
#[tokio::test]
async fn restart_after_a_quiet_night_keeps_the_idle_timer() {
    assert_eq!(restart_after(2.0 * 3600.0, 120.0).await, (None, 0));
}

/// 真停机：请求日志和定时任务都停了超过 1 小时，重启时从现在重新起算闲置天数。
#[tokio::test]
async fn restart_after_real_downtime_resets_the_idle_timer() {
    let (grace, resets) = restart_after(2.0 * 3600.0, 2.0 * 3600.0).await;
    assert_eq!(grace.as_deref(), Some("1900000000.0"));
    assert_eq!(resets, 1);
}
