//! 线上冒烟：对本机假站点跑。重点：只做不出图的检查；Key 只进请求头，任何输出都不带它；任一项失败退出码非 0。

mod common;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Redirect};
use axum::routing::get;
use axum::{Json, Router};
use common::*;
use gate_ops::smoke::{self, Flag};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const KEY: &str = "SMOKE-KEY-synthetic-0123456789";

#[derive(Clone, Default)]
struct Site {
    /// 这些路径返回 500
    broken: Vec<&'static str>,
    /// /healthz 返回非 JSON
    health_not_json: bool,
    /// /v1/me 对任何请求（含没带 Key）都返回 200（鉴权漏洞）
    me_open: bool,
    /// /status.json 重定向
    redirect_status_json: bool,
    seen: Arc<Mutex<Vec<(String, String)>>>,
}

fn site(s: Site) -> Router {
    async fn serve(
        State(s): State<Site>,
        path: &'static str,
        headers: HeaderMap,
        body: Value,
    ) -> axum::response::Response {
        let auth = headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        s.seen.lock().unwrap().push((path.to_string(), auth));
        if s.broken.contains(&path) {
            return (StatusCode::INTERNAL_SERVER_ERROR, "boom").into_response();
        }
        Json(body).into_response()
    }
    Router::new()
        .route(
            "/healthz",
            get(|State(s): State<Site>, h: HeaderMap| async move {
                if s.health_not_json {
                    return "ok".into_response();
                }
                serve(
                    State(s),
                    "/healthz",
                    h,
                    json!({"ok": true, "upstream": true}),
                )
                .await
            }),
        )
        .route(
            "/public/status",
            get(|State(s): State<Site>, h: HeaderMap| async move {
                serve(State(s), "/public/status", h, json!({"registration": {}})).await
            }),
        )
        .route(
            "/public/live",
            get(|State(s): State<Site>, h: HeaderMap| async move {
                serve(State(s), "/public/live", h, json!({"meta": {}})).await
            }),
        )
        .route(
            "/status.json",
            get(|State(s): State<Site>, h: HeaderMap| async move {
                if s.redirect_status_json {
                    return Redirect::permanent("/elsewhere").into_response();
                }
                serve(
                    State(s),
                    "/status.json",
                    h,
                    json!({"components": [], "active": [], "incidents": []}),
                )
                .await
            }),
        )
        .route(
            "/queue-status",
            get(|State(s): State<Site>, h: HeaderMap| async move {
                serve(
                    State(s),
                    "/queue-status",
                    h,
                    json!({"global": {"active": 0, "waiting": 0, "concurrency": 1}}),
                )
                .await
            }),
        )
        .route(
            "/v1/me",
            get(|State(s): State<Site>, h: HeaderMap| async move {
                let good = h.get("authorization").and_then(|v| v.to_str().ok())
                    == Some(&format!("Bearer {KEY}"));
                if good || s.me_open {
                    return serve(
                        State(s),
                        "/v1/me",
                        h,
                        json!({"name": "synthetic", "today": {"images": 0}}),
                    )
                    .await;
                }
                s.seen.lock().unwrap().push((
                    "/v1/me".into(),
                    h.get("authorization")
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or("")
                        .to_string(),
                ));
                (
                    StatusCode::UNAUTHORIZED,
                    Json(json!({"detail": format!("bad key (not echoing {KEY})")})),
                )
                    .into_response()
            }),
        )
        .with_state(s)
}

async fn run(s: Site, key: Option<&str>) -> smoke::Report {
    let mock = Mock::start(site(s)).await;
    smoke::run(&mock.base, key, Duration::from_secs(5))
        .await
        .unwrap()
}

fn flags(r: &smoke::Report) -> Vec<(&str, Flag)> {
    r.outcomes
        .iter()
        .map(|o| (o.name.as_str(), o.flag))
        .collect()
}

#[tokio::test]
async fn healthy_site_passes_without_a_key() {
    let r = run(Site::default(), None).await;
    assert_eq!(
        flags(&r),
        vec![
            ("healthz", Flag::Pass),
            ("public/status", Flag::Pass),
            ("public/live", Flag::Pass),
            ("status.json", Flag::Pass),
            ("queue-status", Flag::Pass),
            ("v1/me without key -> 401", Flag::Pass),
            ("v1/me wrong key -> 401", Flag::Pass),
            ("v1/me valid key", Flag::Info),
        ]
    );
    assert_eq!(r.exit_code(), 0);
    assert!(
        r.summary().contains("FAIL 0\nPASS 7\nINFO 1\n"),
        "{}",
        r.summary()
    );
}

#[tokio::test]
async fn with_key_checks_v1_me_and_never_prints_it() {
    let s = Site::default();
    let seen = s.seen.clone();
    let r = run(s, Some(KEY)).await;
    assert_eq!(r.exit_code(), 0);
    assert_eq!(
        flags(&r).last().unwrap(),
        &("v1/me valid key (SMOKE_KEY)", Flag::Pass)
    );
    // Key 只进了请求头，且只发给 /v1/me
    let seen = seen.lock().unwrap();
    let with_key: Vec<_> = seen.iter().filter(|(_, a)| a.contains(KEY)).collect();
    assert_eq!(with_key.len(), 1);
    assert_eq!(with_key[0].0, "/v1/me");
    // 任何输出（逐项结果与汇总）都不含 Key
    for o in &r.outcomes {
        assert!(!o.line.contains(KEY) && !o.name.contains(KEY), "{}", o.line);
    }
    assert!(!r.summary().contains(KEY));
}

#[tokio::test]
async fn failures_make_the_exit_code_nonzero() {
    let r = run(
        Site {
            broken: vec!["/public/live"],
            ..Site::default()
        },
        None,
    )
    .await;
    assert_eq!(r.failed(), 1);
    assert_eq!(r.exit_code(), 1);
    let bad = r.outcomes.iter().find(|o| o.flag == Flag::Fail).unwrap();
    assert!(
        bad.name == "public/live" && bad.line.contains("[500]") && bad.line.contains("期望 200"),
        "{}",
        bad.line
    );

    let r = run(
        Site {
            health_not_json: true,
            ..Site::default()
        },
        None,
    )
    .await;
    assert!(
        r.outcomes[0].flag == Flag::Fail && r.outcomes[0].line.contains("正文不是 JSON"),
        "{}",
        r.outcomes[0].line
    );

    // 鉴权漏洞：没带 Key 也能 200 → 两条 401 检查都失败
    let r = run(
        Site {
            me_open: true,
            ..Site::default()
        },
        None,
    )
    .await;
    assert_eq!(r.failed(), 2);
    assert_eq!(r.exit_code(), 1);

    // 重定向不跟随：状态码对不上就是失败（线上 http→https 配错时能看出来）
    let r = run(
        Site {
            redirect_status_json: true,
            ..Site::default()
        },
        None,
    )
    .await;
    let o = r.outcomes.iter().find(|o| o.name == "status.json").unwrap();
    assert_eq!(o.flag, Flag::Fail);
    assert!(o.line.contains("[308]"), "{}", o.line);
}

#[tokio::test]
async fn unreachable_site_fails_every_check_without_leaking_the_key() {
    // 绑定后立刻关掉，拿到一个确定没人监听的端口
    let port = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let r = smoke::run(
        &format!("http://127.0.0.1:{port}"),
        Some(KEY),
        Duration::from_secs(2),
    )
    .await
    .unwrap();
    assert_eq!(r.failed(), 8);
    assert_eq!(r.exit_code(), 1);
    for o in &r.outcomes {
        assert_eq!(o.flag, Flag::Fail);
        assert!(!o.line.contains(KEY), "{}", o.line);
    }
}
