mod common;

use common::*;
use gate_testkit::Reply;
use serde_json::json;

const PAGE: &str = "/admin/api/upstream";
const PATH: &str = "/admin/api/upstream/refresh-info";
const CACHE_KEY: &str = "upstream_account_information_projection";

/// 聚合页里每账号的官方信息行（`info`）。
async fn cached_info(api: &Admin) -> serde_json::Value {
    api.get_ok(PAGE).await["accounts"][0]["info"].clone()
}

#[tokio::test]
async fn information_status_is_admin_only_and_get_never_queries_upstream() {
    let e = setup(&[]).await;
    let (status, _, _) = Call::new("POST", PATH).send(&e.state).await;
    assert_eq!(status.as_u16(), 401);

    let api = Admin::login(&e.state).await;
    let data = cached_info(&api).await;
    assert_eq!(data["ban_status"], serde_json::Value::Null);
    assert_eq!(data["stale"], true);
    assert!(e.mock.requests_to("/user/information").is_empty());

    let bare = Admin::login_bare(&e.state).await;
    bare.fails("POST", PATH, Some(json!({})), 403, "请求来源校验失败")
        .await;
    assert!(e.mock.requests_to("/user/information").is_empty());
}

#[tokio::test]
async fn manual_refresh_stores_only_projection_and_failure_preserves_last_success() {
    let e = setup(&[]).await;
    let api = Admin::login(&e.state).await;
    let csrf = api.get_ok(PAGE).await["csrf_info"]
        .as_str()
        .unwrap()
        .to_string();
    let info = json!({
        "banStatus": "banned",
        "banMessage": "synthetic moderation reason",
        "emailVerified": true,
        "accountCreatedAt": 1700000000,
        "trialActivated": false,
        "trialActionsLeft": 0,
        "trialImagesLeft": 0,
        "plainTextEmail": "private@example.invalid",
        "paymentProcessorData": {"card": "synthetic-card"},
        "keystore": "synthetic-keystore"
    });
    e.mock.push("/user/information", Reply::json(200, &info));
    let refreshed = api
        .raw(
            "POST",
            PATH,
            Some(json!({})),
            &[("x-nai-admin-csrf", csrf.as_str())],
        )
        .await;
    assert_eq!(refreshed.status, 200, "{}", refreshed.text());
    let refreshed = refreshed.json();
    assert_eq!(refreshed["accounts"][0]["ban_status"], "banned");
    assert_eq!(refreshed["accounts"][0]["email_verified"], true);
    assert_eq!(
        refreshed["accounts"][0]["last_error"],
        serde_json::Value::Null
    );
    assert_eq!(e.mock.requests_to("/user/information").len(), 1);
    let stored = setting(&e.state, CACHE_KEY).await.unwrap();
    assert!(stored.contains("synthetic moderation reason"));
    assert!(!stored.contains("private@example.invalid"));
    assert!(!stored.contains("paymentProcessorData"));
    assert!(!stored.contains("synthetic-keystore"));

    // 页面读数仍是缓存投影，不再消耗剧本好的上游响应。
    let shown = cached_info(&api).await;
    assert_eq!(shown["ban_status"], "banned");
    assert_eq!(e.mock.requests_to("/user/information").len(), 1);

    // Move the persisted global cooldown into the past, then script a failed refresh.
    let mut cache: serde_json::Value = serde_json::from_str(&stored).unwrap();
    cache["last_attempt_at"] = json!(e.state.now() - 301.0);
    set_setting(&e, CACHE_KEY, &cache.to_string()).await;
    e.mock.push(
        "/user/information",
        Reply::status(503, "sensitive upstream detail"),
    );
    let refreshed = api
        .raw(
            "POST",
            PATH,
            Some(json!({})),
            &[("x-nai-admin-csrf", csrf.as_str())],
        )
        .await;
    assert_eq!(refreshed.status, 200, "{}", refreshed.text());
    let result = refreshed.json();
    assert_eq!(result["accounts"][0]["ban_status"], "banned");
    assert_eq!(
        result["accounts"][0]["last_error"],
        "上游账号信息暂时不可用"
    );
    assert!(result["accounts"][0]["last_error_at"].as_f64().is_some());
    let stored = setting(&e.state, CACHE_KEY).await.unwrap();
    assert!(!stored.contains("sensitive upstream detail"));
}

#[tokio::test]
async fn manual_refresh_is_csrf_protected_and_cooled_down() {
    let e = setup(&[]).await;
    let api = Admin::login(&e.state).await;
    let csrf = api.get_ok(PAGE).await["csrf_info"]
        .as_str()
        .unwrap()
        .to_string();
    api.fails(
        "POST",
        PATH,
        Some(json!({})),
        403,
        "会话校验失败，请刷新后台后重试",
    )
    .await;
    e.mock.push(
        "/user/information",
        Reply::json(
            200,
            &json!({
                "banStatus": "not_banned",
                "banMessage": null,
                "emailVerified": false,
                "accountCreatedAt": 1
            }),
        ),
    );
    let first = api
        .raw(
            "POST",
            PATH,
            Some(json!({})),
            &[("x-nai-admin-csrf", csrf.as_str())],
        )
        .await;
    assert_eq!(first.status, 200, "{}", first.text());
    let second = api
        .raw(
            "POST",
            PATH,
            Some(json!({})),
            &[("x-nai-admin-csrf", csrf.as_str())],
        )
        .await;
    assert_eq!(second.status, 429);
    assert_eq!(e.mock.requests_to("/user/information").len(), 1);
}
