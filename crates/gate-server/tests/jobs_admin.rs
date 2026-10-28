//! 定时任务：登记顺序、运行记录（库里的 `job_last`）；后台接口只剩额度记录 `quota-records`
//! （`GET /admin/api/jobs` 随「系统」页定时任务卡片删除，`quota-algo` 随自动额度算法删除，都回 404）。
//! 内核时代的 `modules*`、`autopilot` 接口已删除（回 404）。公开仓库：全部是合成数据；上游是本地假服务。

mod common;
use common::*;
use serde_json::{Value, json};

/// 2026-10-14 12:00:00 +08:00
const NOW: f64 = 1_791_950_400.0;

const NAMES: [&str; 7] = [
    "upstream_snapshot",
    "breaker",
    "key_guard",
    "maintenance",
    "abuse_scan",
    "anlas_reconcile",
    "idle_recycle",
];

#[tokio::test]
async fn routes_need_admin_login() {
    let e = setup(&[]).await;
    for (method, path) in [("GET", "/admin/api/quota-records")] {
        let (status, _, body) = Call::new(method, path).json(json!({})).send(&e.state).await;
        assert_eq!(status, 401, "{method} {path}");
        assert_eq!(
            serde_json::from_slice::<Value>(&body).unwrap(),
            json!({"detail": "未登录或会话已过期"})
        );
    }
}

#[tokio::test]
async fn the_kernel_routes_are_gone() {
    let e = setup(&[]).await;
    let api = Admin::login_bare(&e.state).await;
    for path in [
        "/modules",
        "/modules/capacity",
        "/autopilot",
        "/quota-algo",
        "/jobs",
    ] {
        let (status, v) = api.json("GET", path, None).await;
        assert_eq!((status, v), (404, json!({"detail": "Not Found"})), "{path}");
    }
    // 写操作：`/admin/{*rest}` 是后台前端页面的 GET 路由，别的方法落到它上面是 405（和其他不存在的后台写接口一样）
    for (method, path) in [
        ("PUT", "/modules/capacity"),
        ("PUT", "/modules/x"),
        ("POST", "/modules/run"),
        ("PUT", "/modules/run"),
    ] {
        let (status, v) = api.json(method, path, Some(json!({"enabled": true}))).await;
        assert_eq!(
            (status, v),
            (405, json!({"detail": "Method Not Allowed"})),
            "{method} {path}"
        );
    }
    assert_eq!(setting(&e.state, "module_capacity").await, None);
    assert_eq!(setting(&e.state, "quota_auto_enabled").await, None);
}

/// 运行记录：上次运行的时间 / 结果 / 耗时 / 摘要（Claude 从库里的 `job_last` 看）。
#[tokio::test]
async fn jobs_registry_and_last_run() {
    let _clock = clock(Some(NOW)).await;
    let e = setup(&[]).await;
    e.state
        .jobs
        .set_official_api(Some("http://127.0.0.1:9/never".into()));
    let names: Vec<&str> = gate_server::jobs::JOBS.iter().map(|j| j.name).collect();
    assert_eq!(names, NAMES, "按登记顺序");
    assert!(e.state.jobs.last().await.is_empty(), "还没跑过");

    for name in NAMES {
        run_job(&e.state, name).await.unwrap();
    }
    let last = Value::Object(e.state.jobs.last().await);
    for name in NAMES {
        let l = &last[name];
        assert_eq!(l["at"], json!(NOW), "{name} {l}");
        assert_eq!(l["ok"], true, "{name} {l}");
        assert!(l["ms"].as_i64().unwrap() >= 0, "{name} {l}");
        assert!(!l["summary"].as_str().unwrap().is_empty(), "{name} {l}");
    }
    let by = |name: &str| last[name]["summary"].clone();
    assert_eq!(by("maintenance"), "全部完成");
    assert_eq!(by("breaker"), "最近 15 分钟上游失败 0/0：正常");
    assert_eq!(by("key_guard"), "没有 Key 触发");
    assert_eq!(by("idle_recycle"), "最近 60 分钟没出图，本轮跳过");
    assert_eq!(by("abuse_scan"), "没有新的可疑信号");
}

#[tokio::test]
async fn one_failing_job_does_not_affect_the_others() {
    let _clock = clock(Some(NOW)).await;
    let e = setup(&[]).await;
    // 快照表坏了：「上游 V5 快照」这一轮失败，别的任务照常
    run_sql(&e, vec!["DROP TABLE upstream_snapshots".into()]).await;
    for name in ["upstream_snapshot", "breaker", "key_guard"] {
        let _ = run_job(&e.state, name).await;
    }
    let last = Value::Object(e.state.jobs.last().await);
    assert_eq!(last["upstream_snapshot"]["ok"], false, "{last}");
    for name in ["breaker", "key_guard"] {
        assert_eq!(last[name]["ok"], true, "{name} {last}");
    }
}

#[tokio::test]
async fn the_run_record_survives_a_restart() {
    let _clock = clock(Some(NOW)).await;
    let e = setup(&[]).await;
    run_job(&e.state, "breaker").await.unwrap();
    let saved: Value = serde_json::from_str(&setting(&e.state, "job_last").await.unwrap()).unwrap();
    assert_eq!(saved["breaker"]["ok"], true);
    // 同一个数据目录重新建一份 AppState，启动时读回
    let s2 = e.restart().await;
    assert!(s2.jobs.last().await.is_empty(), "启动序列之前还是空的");
    s2.jobs.load(&s2.db).await.unwrap();
    assert_eq!(Value::Object(s2.jobs.last().await), saved);
    // 读不懂的记录当没有
    s2.db.set_setting("job_last", "坏").await.unwrap();
    s2.jobs.load(&s2.db).await.unwrap();
    assert!(s2.jobs.last().await.is_empty());
}

/// 额度记录：每个上游账号的 V5 电量读数、今天 / 昨天出了几张（只显示）；账号给编号不给 Token。
#[tokio::test]
async fn quota_records_shape() {
    let _clock = clock(Some(NOW)).await;
    let e = setup(&[]).await;
    let id = e.state.nai.tokens().await[0].token_id.clone();
    e.state
        .db
        .bump_upstream_image_counter(&id, &e.state.day(), 7)
        .await
        .unwrap();
    let api = Admin::login_bare(&e.state).await;
    let (status, v) = api.json("GET", "/quota-records", None).await;
    assert_eq!(status, 200, "{v}");
    let a = &v["accounts"][0];
    assert_eq!(a["position"], 1);
    assert_eq!(a["today"], json!({"images": 7, "v5": 0}));
    assert_eq!(a["yesterday"], json!({"images": 0, "v5": 0}));
    assert!(a.get("token_id").is_none());
    // 实测恢复按账号给（快照没积累时为 null）；顶层聚合版已随分账号快照一起删除
    assert_eq!(a["quiet_rate"], Value::Null);
    assert!(v.get("quiet_rate").is_none());
    assert_eq!(v["manual"], json!({"account_caps": [], "paid_v5": []}));
}
