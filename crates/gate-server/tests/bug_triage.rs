//! Bug 追踪的降噪与自动处理：
//! 1. 网页报错上报：浏览器网络失败、非本站脚本的 `Uncaught …` 不入库；
//! 2. 错误账本：上游故障造成的 5xx 不记 Bug，本站自己的 500 照记；
//! 3. 维护循环里的自动处理：噪音立刻关，注意级 24 小时 / 错误级 7 天没再出现的关；再出现按「复发」重新打开。

mod common;
use axum::Router;
use axum::extract::Extension;
use axum::middleware::{from_fn, from_fn_with_state};
use axum::routing::get;
use common::{Admin, Call, Env, TOKEN, settle, setup};
use gate_core::errors::{AutoReason, signature};
use gate_db::repo::errors::{AutoResolve, ErrorRow, Stored};
use gate_server::bugs::Report;
use gate_server::error::{GateError, InternalError, upstream_transport};
use gate_server::reqctx::ReqCtx;
use gate_server::{AppState, accounting, jobs, middleware};
use gate_testkit::Reply;
use serde_json::{Value, json};
use tower::ServiceExt;

const DAY: f64 = 86400.0;

// ------------------------------------------------------------------ 网页报错上报

async fn report(state: &AppState, body: Value) {
    let (status, _, _) = Call::new("POST", "/public/client-error")
        .header("host", "gate.example.top")
        .header("origin", "https://gate.example.top")
        .json(body)
        .send(state)
        .await;
    assert_eq!(status, 204);
}

async fn web_titles(state: &AppState) -> Vec<String> {
    settle().await;
    state.bugs.idle().await;
    let mut titles: Vec<String> = state
        .db
        .list_error_events(true, 100)
        .await
        .unwrap()
        .into_iter()
        .filter(|r| r["source"].as_str().is_some_and(|s| s.starts_with("web:")))
        .map(|r| r["title"].as_str().unwrap().to_string())
        .collect();
    titles.sort();
    titles
}

#[tokio::test]
async fn client_noise_is_not_stored() {
    let e = setup(&[]).await;
    let own = "https://gate.example.top/_owl/assets/app-abc.js";

    // 浏览器网络失败（断网 / 切后台 / 请求被取消）：新旧前端都可能报，一律不收
    for msg in [
        "Failed to fetch",
        "live: Failed to fetch",
        "NetworkError when attempting to fetch resource.",
        "TypeError: Load failed",
        "AbortError: The operation was aborted.",
    ] {
        report(&e.state, json!({"page": "landing", "msg": msg, "src": own})).await;
    }
    // 旧版缓存页 / 插件里的 `$`：出错脚本不在 /_owl/ 下
    report(
        &e.state,
        json!({"page": "landing", "msg": "Uncaught ReferenceError: $ is not defined",
               "src": "https://gate.example.top/", "filename": "https://gate.example.top/", "line": 12}),
    )
    .await;
    // 老页面不带 filename：Uncaught 的 src / 堆栈里也没有本站文件
    report(
        &e.state,
        json!({"page": "landing", "msg": "Uncaught ReferenceError: jQuery is not defined",
               "src": "/", "line": 3, "stack": "ReferenceError: jQuery is not defined\n    at https://gate.example.top/:3:9"}),
    )
    .await;
    assert!(web_titles(&e.state).await.is_empty());

    // 本站脚本的脚本错误照记（带 filename），Promise 拒绝（没有 filename）也照记
    report(
        &e.state,
        json!({"page": "landing", "msg": "Uncaught TypeError: x is undefined",
               "src": own, "filename": own, "line": 7}),
    )
    .await;
    report(
        &e.state,
        json!({"page": "admin", "msg": "TypeError: y is not a function", "src": "/admin",
               "stack": format!("TypeError: y is not a function\n    at f ({own}:1:2)")}),
    )
    .await;
    assert_eq!(
        web_titles(&e.state).await,
        [
            "TypeError: y is not a function",
            "Uncaught TypeError: x is undefined"
        ]
    );
}

// ------------------------------------------------------------------ 错误账本

/// 只带请求上下文 + 错误账本两层的小路由：用来造各种 5xx。
fn ledger_app(state: &AppState) -> Router {
    Router::new()
        // 上游状态码透传：对成员是 500，原因在上游
        .route(
            "/up500",
            get(|| async {
                GateError::new(500, "上游服务暂时不可用，请稍后重试（上游状态 500）")
                    .from_upstream(true)
            }),
        )
        // 上游连不上：对成员是 InternalError(500)，请求上下文里记「上游故障」
        .route(
            "/transport",
            get(|Extension(ctx): Extension<ReqCtx>| async move {
                upstream_transport(&ctx, "上游请求失败：connection reset".into())
            }),
        )
        // 本站自己的 5xx
        .route(
            "/own500",
            get(|| async { GateError::new(500, "请求体解析失败") }),
        )
        .route(
            "/internal",
            get(|| async { InternalError("数据库写入失败".into()) }),
        )
        .layer(from_fn_with_state(state.clone(), accounting::error_ledger))
        .layer(from_fn(middleware::request_context))
}

async fn hit(app: &Router, path: &str) -> u16 {
    let req = axum::http::Request::builder()
        .uri(path)
        .body(axum::body::Body::empty())
        .unwrap();
    app.clone().oneshot(req).await.unwrap().status().as_u16()
}

#[tokio::test]
async fn upstream_5xx_is_not_a_bug_but_own_500_is() {
    let e = setup(&[]).await;
    let app = ledger_app(&e.state);

    assert_eq!(hit(&app, "/up500").await, 500);
    assert_eq!(hit(&app, "/transport").await, 500);
    assert!(
        e.state.bugs.recent().is_empty(),
        "上游故障不该进 Bug 追踪：{:?}",
        e.state.bugs.recent()
    );

    assert_eq!(hit(&app, "/own500").await, 500);
    assert_eq!(hit(&app, "/internal").await, 500);
    let recent = e.state.bugs.recent();
    let got: Vec<(&str, &str)> = recent
        .iter()
        .map(|b| (b.source.as_str(), b.level.as_str()))
        .collect();
    assert_eq!(got, [("gate", "warn"), ("request", "error")], "{recent:?}");
    e.state.bugs.idle().await;
    assert_eq!(e.state.db.open_error_count().await.unwrap(), 2);
}

#[tokio::test]
async fn real_upstream_500_through_the_image_route_is_not_a_bug() {
    let e = setup(&[]).await;
    // 假上游连续回 500（网关对 5xx 可能重试，多备几条）
    for _ in 0..5 {
        e.mock.push(
            "/ai/generate-image",
            Reply::json(500, &json!({"message": "boom"})),
        );
    }
    let (status, _, body) = Call::new("POST", "/ai/generate-image")
        .bearer(TOKEN)
        .json(json!({
            "input": "1girl, solo", "model": "nai-diffusion-4-5-full", "action": "generate",
            "parameters": {"width": 832, "height": 1216, "steps": 28, "scale": 5,
                           "sampler": "k_euler_ancestral", "n_samples": 1, "seed": 1234},
        }))
        .send(&e.state)
        .await;
    assert!(
        status.is_server_error(),
        "{status} {}",
        String::from_utf8_lossy(&body)
    );
    settle().await; // 记账是另一组后台任务
    e.state.bugs.idle().await;
    // 用量日志里有一条 error，Bug 追踪里什么都没有
    assert!(
        e.state.bugs.recent().is_empty(),
        "{:?}",
        e.state.bugs.recent()
    );
    assert!(
        e.state
            .db
            .list_error_events(true, 10)
            .await
            .unwrap()
            .is_empty()
    );
    let rows = e
        .state
        .db
        .list_logs(10, 0, &gate_db::repo::usage::LogFilter::default())
        .await
        .unwrap();
    assert!(rows.iter().any(|r| r["status"] == "error"), "{rows:?}");
}

// ------------------------------------------------------------------ 自动处理

struct Seed<'a> {
    sig: &'a str,
    source: &'a str,
    level: &'a str,
    title: &'a str,
    detail: &'a str,
    last_seen_ago: f64,
}

async fn seed(e: &Env, rows: &[Seed<'_>]) {
    let now = e.state.now();
    let sql: Vec<String> = rows
        .iter()
        .map(|r| {
            format!(
                "INSERT INTO error_events (sig, source, level, title, detail, count, first_seen, last_seen) \
                 VALUES ('{}', '{}', '{}', '{}', '{}', 3, {}, {})",
                r.sig,
                r.source,
                r.level,
                r.title,
                r.detail.replace('\n', " "),
                now - 30.0 * DAY,
                now - r.last_seen_ago
            )
        })
        .collect();
    e.state.db.run_script(sql).await.unwrap();
}

async fn state_of(e: &Env, sig: &str) -> (bool, Option<String>) {
    let rows = e.state.db.list_error_events(true, 500).await.unwrap();
    let r = rows.iter().find(|r| r["sig"] == sig).expect("行不存在");
    (
        !r["resolved_at"].is_null(),
        r["auto_resolved"].as_str().map(str::to_string),
    )
}

#[tokio::test]
async fn auto_triage_closes_noise_and_stale_bugs() {
    let e = setup(&[]).await;
    let s = &e.state;
    seed(
        &e,
        &[
            // 噪音：立刻关（刚出现过也一样）
            Seed {
                sig: "net",
                source: "web:landing",
                level: "warn",
                title: "live: Failed to fetch",
                detail: ":0 UA",
                last_seen_ago: 5.0,
            },
            Seed {
                sig: "up",
                source: "gate",
                level: "warn",
                title: "500 上游服务暂时不可用，请稍后重试（上游状态 500）",
                detail: "",
                last_seen_ago: 5.0,
            },
            Seed {
                sig: "foreign",
                source: "web:landing",
                level: "warn",
                title: "Uncaught ReferenceError: $ is not defined",
                detail: "/:0 UA",
                last_seen_ago: 5.0,
            },
            // 久未出现
            Seed {
                sig: "stale_w",
                source: "maintenance:disk",
                level: "warn",
                title: "磁盘读数失败",
                detail: "d",
                last_seen_ago: 25.0 * 3600.0,
            },
            Seed {
                sig: "stale_e",
                source: "maintenance:disk",
                level: "error",
                title: "磁盘读数失败（错误）",
                detail: "d",
                last_seen_ago: 8.0 * DAY,
            },
            // 该留着的
            Seed {
                sig: "fresh_w",
                source: "maintenance:disk",
                level: "warn",
                title: "刚出现的注意",
                detail: "d",
                last_seen_ago: 3600.0,
            },
            Seed {
                sig: "e_3d",
                source: "request",
                level: "error",
                title: "InternalError: 3 天前的错误",
                detail: "d",
                last_seen_ago: 3.0 * DAY,
            },
            Seed {
                sig: "own_js",
                source: "web:landing",
                level: "warn",
                title: "Uncaught TypeError: x is undefined",
                detail: "https://gate.example.top/_owl/assets/app-abc.js:7 UA",
                last_seen_ago: 60.0,
            },
        ],
    )
    .await;
    // 站长手动处理过的：不动，也不打上「自动」标记
    s.db.run_script(vec![format!(
        "INSERT INTO error_events (sig, source, level, title, count, first_seen, last_seen, resolved_at) \
         VALUES ('manual', 'request', 'error', '手动关的', 1, 1.0, {0}, {0})",
        s.now() - 20.0 * DAY
    )])
    .await
    .unwrap();

    assert_eq!(s.bugs.auto_triage(s.now()).await.unwrap(), 5);

    let closed = |reason: &str| (true, Some(reason.to_string()));
    assert_eq!(state_of(&e, "net").await, closed("network"));
    assert_eq!(state_of(&e, "up").await, closed("upstream"));
    assert_eq!(state_of(&e, "foreign").await, closed("foreign_script"));
    assert_eq!(state_of(&e, "stale_w").await, closed("stale_warn"));
    assert_eq!(state_of(&e, "stale_e").await, closed("stale_error"));
    for sig in ["fresh_w", "e_3d", "own_js"] {
        assert_eq!(state_of(&e, sig).await, (false, None), "{sig}");
    }
    assert_eq!(state_of(&e, "manual").await, (true, None));
    assert_eq!(s.db.open_error_count().await.unwrap(), 3);

    // 再跑一遍什么都不会变
    assert_eq!(s.bugs.auto_triage(s.now()).await.unwrap(), 0);

    // 后台列表带原因码，页面据此显示「自动处理」
    let a = Admin::login(s).await;
    let v = a.get("/admin/api/errors?all=true").await.json();
    assert_eq!(v["open"], 3);
    let reasons: Vec<(&str, &str)> = v["errors"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|r| Some((r["sig"].as_str()?, r["auto_resolved"].as_str()?)))
        .collect();
    assert_eq!(reasons.len(), 5, "{reasons:?}");
    assert!(
        AutoReason::ALL
            .iter()
            .all(|r| reasons.iter().any(|(_, c)| *c == r.code()))
    );
}

#[tokio::test]
async fn auto_resolved_bug_reopens_as_regressed() {
    let e = setup(&[]).await;
    let s = &e.state;
    // 用真实的特征码，保证下面的再次出现命中同一行
    let sig = signature("maintenance:disk", None, "磁盘读数失败");
    seed(
        &e,
        &[Seed {
            sig: &sig,
            source: "maintenance:disk",
            level: "warn",
            title: "磁盘读数失败",
            detail: "旧堆栈",
            last_seen_ago: 2.0 * DAY,
        }],
    )
    .await;
    assert_eq!(s.bugs.auto_triage(s.now()).await.unwrap(), 1);
    assert_eq!(state_of(&e, &sig).await, (true, Some("stale_warn".into())));

    // 库层：已关闭的特征码再出现 = 复发
    let row = ErrorRow {
        sig: sig.clone(),
        source: "maintenance:disk".into(),
        level: "warn".into(),
        title: "磁盘读数失败".into(),
        detail: "新堆栈".into(),
        path: String::new(),
        key_id: None,
        rid: "abcd1234".into(),
        ts: s.now(),
    };
    assert_eq!(
        s.db.store_error_event(&row).await.unwrap(),
        Stored::Regressed
    );
    assert_eq!(
        state_of(&e, &sig).await,
        (false, None),
        "重新打开，自动处理的记号清掉"
    );

    // 整条链路：自动关闭后，真实的 capture 同样让它回到「未处理」，次数累加
    s.db.run_script(vec![format!(
        "UPDATE error_events SET last_seen={} WHERE sig='{sig}'",
        s.now() - 2.0 * DAY
    )])
    .await
    .unwrap();
    assert_eq!(s.bugs.auto_triage(s.now()).await.unwrap(), 1);
    assert!(state_of(&e, &sig).await.0);
    let got = s.bugs.capture(Report {
        source: "maintenance:disk",
        title: "磁盘读数失败",
        level: "warn",
        ..Default::default()
    });
    assert_eq!(got, sig);
    e.state.bugs.idle().await;
    assert_eq!(state_of(&e, &sig).await, (false, None));
    let rows = s.db.list_error_events(false, 10).await.unwrap();
    assert_eq!(rows[0]["count"], 5, "3 次种子 + 2 次复发");
}

#[tokio::test]
async fn auto_resolve_skips_rows_that_recurred_meanwhile() {
    let e = setup(&[]).await;
    let s = &e.state;
    seed(
        &e,
        &[Seed {
            sig: "racy",
            source: "maintenance:disk",
            level: "warn",
            title: "磁盘读数失败",
            detail: "",
            last_seen_ago: 2.0 * DAY,
        }],
    )
    .await;
    let read = s.db.open_errors_for_triage().await.unwrap();
    assert_eq!(read.len(), 1);
    // 读完之后又出现了一次（last_seen 变了）：这一轮不能把它关掉
    s.db.run_script(vec![format!(
        "UPDATE error_events SET last_seen={} WHERE sig='racy'",
        s.now()
    )])
    .await
    .unwrap();
    let n =
        s.db.auto_resolve_errors(
            &[AutoResolve {
                sig: "racy".into(),
                last_seen: read[0].last_seen,
                reason: "stale_warn",
            }],
            s.now(),
        )
        .await
        .unwrap();
    assert_eq!(n, 0);
    assert_eq!(state_of(&e, "racy").await, (false, None));
}

#[tokio::test]
async fn manual_resolve_is_not_marked_automatic() {
    let e = setup(&[]).await;
    let s = &e.state;
    seed(
        &e,
        &[Seed {
            sig: "m1",
            source: "request",
            level: "error",
            title: "InternalError: x",
            detail: "",
            last_seen_ago: 10.0,
        }],
    )
    .await;
    let a = Admin::login(s).await;
    assert_eq!(
        a.post("/admin/api/errors/m1/resolve", json!({}))
            .await
            .json(),
        json!({"ok": true})
    );
    assert_eq!(
        state_of(&e, "m1").await,
        (true, None),
        "站长点的不算自动处理"
    );
}

#[tokio::test]
async fn maintenance_round_runs_triage_at_most_every_ten_minutes() {
    let e = setup(&[]).await;
    let s = &e.state;
    s.jobs
        .set_official_api(Some("http://127.0.0.1:9/never".into()));
    seed(
        &e,
        &[Seed {
            sig: "w1",
            source: "maintenance:disk",
            level: "warn",
            title: "磁盘读数失败",
            detail: "",
            last_seen_ago: 2.0 * DAY,
        }],
    )
    .await;
    // 启动后的第一轮维护就会跑
    jobs::maintenance_once(s).await;
    assert_eq!(state_of(&e, "w1").await, (true, Some("stale_warn".into())));

    // 10 分钟之内的下一轮不再跑
    seed(
        &e,
        &[Seed {
            sig: "w2",
            source: "maintenance:disk",
            level: "warn",
            title: "另一个磁盘问题",
            detail: "",
            last_seen_ago: 2.0 * DAY,
        }],
    )
    .await;
    jobs::maintenance_once(s).await;
    assert_eq!(state_of(&e, "w2").await, (false, None));

    // 节流本身：到点才放行
    let b = &s.bugs;
    let t = s.now() + 10_000.0;
    assert!(b.triage_due(t));
    assert!(!b.triage_due(t + 599.0));
    assert!(b.triage_due(t + 600.0));
}
