//! `nai-ops set / get`：对着本机假的后台接口跑（登录 → Cookie → 读 → 写 → 读回）。

mod common;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post, put};
use axum::{Json, Router};
use common::Mock;
use serde_json::{Value, json};
use std::process::Command;
use std::sync::{Arc, Mutex};

const PASSWORD: &str = "synthetic-admin-password-not-real";
const COOKIE: &str = "owl_admin=synthetic-session-value";

#[derive(Default)]
struct St {
    daily_anlas: Mutex<i64>,
    open: Mutex<bool>,
    roles: Mutex<Vec<Value>>,
    per_account: Mutex<i64>,
    puts: Mutex<Vec<(String, Value)>>,
}

fn authed(h: &HeaderMap) -> bool {
    h.get("cookie").and_then(|v| v.to_str().ok()) == Some(COOKIE)
}

async fn app() -> Mock {
    let st = Arc::new(St::default());
    *st.daily_anlas.lock().unwrap() = 5000;
    *st.per_account.lock().unwrap() = 5;
    let r = Router::new()
        .route(
            "/admin/api/login",
            post(|Json(b): Json<Value>| async move {
                if b["password"] == PASSWORD {
                    (
                        StatusCode::OK,
                        [("set-cookie", format!("{COOKIE}; Path=/; HttpOnly"))],
                        Json(json!({"ok": true})),
                    )
                        .into_response()
                } else {
                    (StatusCode::UNAUTHORIZED, Json(json!({"detail": "密码错误"}))).into_response()
                }
            }),
        )
        .route(
            "/admin/api/settings",
            get(|State(st): State<Arc<St>>, h: HeaderMap| async move {
                if !authed(&h) {
                    return StatusCode::UNAUTHORIZED.into_response();
                }
                Json(json!({"budgets":{"daily_anlas":*st.daily_anlas.lock().unwrap(),"daily_v5":150,"monthly_anlas":10000},"v5_paused":false,"v5_medium_mode":"off"}))
                    .into_response()
            })
            .put(
                |State(st): State<Arc<St>>, h: HeaderMap, Json(b): Json<Value>| async move {
                    if !authed(&h) {
                        return StatusCode::UNAUTHORIZED.into_response();
                    }
                    st.puts.lock().unwrap().push(("/settings".into(), b.clone()));
                    if let Some(v) = b["budgets"]["daily_anlas"].as_i64() {
                        if v > 9999 {
                            return (
                                StatusCode::UNPROCESSABLE_ENTITY,
                                Json(json!({"detail": "global_daily_anlas 超出范围"})),
                            )
                                .into_response();
                        }
                        *st.daily_anlas.lock().unwrap() = v;
                    }
                    Json(json!({"ok": true})).into_response()
                },
            ),
        )
        .route(
            "/admin/api/guard",
            get(|State(st): State<Arc<St>>, h: HeaderMap| async move {
                if !authed(&h) {
                    return StatusCode::UNAUTHORIZED.into_response();
                }
                Json(json!({"values": {"queue_per_account": *st.per_account.lock().unwrap()}}))
                    .into_response()
            })
            .put(
                |State(st): State<Arc<St>>, h: HeaderMap, Json(b): Json<Value>| async move {
                    if !authed(&h) {
                        return StatusCode::UNAUTHORIZED.into_response();
                    }
                    st.puts.lock().unwrap().push(("/guard".into(), b.clone()));
                    if let Some(v) = b["queue_per_account"].as_i64() {
                        *st.per_account.lock().unwrap() = v;
                    }
                    Json(json!({"ok": true})).into_response()
                },
            ),
        )
        .route(
            "/admin/api/ops",
            get(|State(st): State<Arc<St>>, h: HeaderMap| async move {
                if !authed(&h) {
                    return StatusCode::UNAUTHORIZED.into_response();
                }
                Json(json!({"registration": {
                    "open": *st.open.lock().unwrap(),
                    "max_users": 10,
                    "expires_days":0,
                    "roles":st.roles.lock().unwrap().clone(),
                }}))
                .into_response()
            }),
        )
        .route(
            "/admin/api/ops/registration",
            put(|State(st): State<Arc<St>>, h: HeaderMap, Json(b): Json<Value>| async move {
                if !authed(&h) {
                    return StatusCode::UNAUTHORIZED.into_response();
                }
                st.puts.lock().unwrap().push(("/ops/registration".into(), b.clone()));
                if let Some(o) = b["open"].as_bool() {
                    *st.open.lock().unwrap() = o;
                }
                if let Some(roles)=b["roles"].as_array() {*st.roles.lock().unwrap()=roles.clone();}
                Json(json!({"ok": true})).into_response()
            }),
        )
        .with_state(st);
    Mock::start(r).await
}

struct Out {
    code: i32,
    out: String,
    err: String,
}

fn nai(base: &str, args: &[&str]) -> Out {
    let dir = tempfile::tempdir().unwrap();
    let o = Command::new(env!("CARGO_BIN_EXE_nai-ops"))
        .current_dir(dir.path())
        .args(args)
        .env("ADMIN_API_BASE", format!("{base}/admin/api"))
        .env("ADMIN_PASSWORD", PASSWORD)
        .env("NAI_GATE_ENV", dir.path().join("missing.env"))
        .output()
        .unwrap();
    Out {
        code: o.status.code().unwrap_or(-1),
        out: String::from_utf8_lossy(&o.stdout).into_owned(),
        err: String::from_utf8_lossy(&o.stderr).into_owned(),
    }
}

fn no_secrets(o: &Out) {
    for t in [&o.out, &o.err] {
        assert!(!t.contains(PASSWORD), "输出里不该有密码：{t}");
        assert!(
            !t.contains("synthetic-session-value"),
            "输出里不该有 Cookie：{t}"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn set_requires_yes_then_writes_and_reads_back() {
    let m = app().await;
    let base = m.base.clone();
    let r = tokio::task::spawn_blocking(move || {
        // 没有 --yes：只预览，退出码 2，不写
        let a = nai(&base, &["set", "global_daily_anlas", "8000"]);
        // --dry-run：预览，退出 0，不写
        let b = nai(&base, &["set", "global_daily_anlas", "8000", "--dry-run"]);
        let c = nai(&base, &["get", "global_daily_anlas"]);
        // --yes：写入并读回
        let d = nai(&base, &["set", "global_daily_anlas", "8000", "--yes"]);
        let e = nai(&base, &["get", "global_daily_anlas"]);
        // 同值：不写
        let f = nai(&base, &["set", "global_daily_anlas", "8000", "--yes"]);
        (a, b, c, d, e, f)
    })
    .await
    .unwrap();
    let (a, b, c, d, e, f) = r;
    assert_eq!(a.code, 2, "{}{}", a.out, a.err);
    assert!(a.out.contains("global_daily_anlas: 5000 → 8000") && a.out.contains("--yes"));
    assert_eq!(b.code, 0);
    assert!(b.out.contains("5000 → 8000") && b.out.contains("没有写入"));
    assert_eq!(c.out.trim(), "global_daily_anlas = 5000");
    assert_eq!(d.code, 0, "{}{}", d.out, d.err);
    assert!(d.out.contains("global_daily_anlas: 5000 → 8000") && d.out.contains("已写入"));
    assert_eq!(e.out.trim(), "global_daily_anlas = 8000");
    assert!(f.out.contains("没有变化"));
    for o in [&a, &b, &c, &d, &e, &f] {
        no_secrets(o);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn endpoints_and_payloads() {
    let m = app().await;
    let base = m.base.clone();
    tokio::task::spawn_blocking(move || {
        let g = nai(&base, &["set", "queue_per_account", "8", "--yes"]);
        assert_eq!(g.code, 0, "{}{}", g.out, g.err);
        assert!(g.out.contains("queue_per_account: 5 → 8"), "{}", g.out);
        // 每小时上限 10-22 删除：不再是可设置项
        let gone = nai(&base, &["set", "account_hourly_cap", "120", "--yes"]);
        assert_eq!(gone.code, 2);
        assert!(gone.out.contains("未知设置名"), "{}", gone.out);
        let r = nai(&base, &["set", "register_open", "on", "--yes"]);
        assert!(r.out.contains("register_open: false → true"), "{}", r.out);
        let gate = nai(
            &base,
            &[
                "set",
                "claim_roles",
                r#"[{"guild":"900000000000000001","role":"900000000000000002","note":""}]"#,
                "--yes",
            ],
        );
        assert_eq!(gate.code, 0, "{}{}", gate.out, gate.err);
        assert!(gate.out.contains("[] → ["), "{}", gate.out);
        let clear = nai(&base, &["set", "claim_roles", "[]", "--yes"]);
        assert!(clear.out.contains("→ []"), "{}", clear.out);
    })
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn refusals() {
    let m = app().await;
    let base = m.base.clone();
    tokio::task::spawn_blocking(move || {
        let unknown = nai(&base, &["set", "no_such_setting", "1", "--yes"]);
        assert_eq!(unknown.code, 2);
        assert!(unknown.out.contains("未知设置名"));
        let unsupported = nai(&base, &["set", "issue_hourly_cap", "5", "--yes"]);
        assert_eq!(unsupported.code, 2);
        assert!(unsupported.out.contains("没有") && unsupported.out.contains("接口"));
        let bad_type = nai(&base, &["set", "global_daily_anlas", "abc", "--yes"]);
        assert_eq!(bad_type.code, 2);
        // 服务端拒绝：原因原样转述，退出 1
        let too_big = nai(&base, &["set", "global_daily_anlas", "999999", "--yes"]);
        assert_eq!(too_big.code, 1);
        assert!(
            too_big.err.contains("422") && too_big.err.contains("超出范围"),
            "{}",
            too_big.err
        );
        let list = nai(&base, &["set", "--list"]);
        assert_eq!(list.code, 0);
        for name in [
            "global_daily_anlas",
            "queue_timeout",
            "issue_hourly_cap",
            "claim_roles",
        ] {
            assert!(list.out.contains(name), "--list 里没有 {name}");
        }
    })
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn wrong_password_never_echoed() {
    let m = app().await;
    let base = m.base.clone();
    tokio::task::spawn_blocking(move || {
        let dir = tempfile::tempdir().unwrap();
        let o = Command::new(env!("CARGO_BIN_EXE_nai-ops"))
            .current_dir(dir.path())
            .args(["get", "global_daily_anlas"])
            .env("ADMIN_API_BASE", format!("{base}/admin/api"))
            .env("ADMIN_PASSWORD", "wrong-synthetic-password")
            .output()
            .unwrap();
        assert_eq!(o.status.code(), Some(1));
        let err = String::from_utf8_lossy(&o.stderr);
        assert!(err.contains("密码不对"));
        assert!(!err.contains("wrong-synthetic-password"));
    })
    .await
    .unwrap();
}
