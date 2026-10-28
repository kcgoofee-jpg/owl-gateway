mod common;
use common::*;
use gate_testkit::Reply;
use serde_json::{Value, json};

async fn context(e: &Env) -> (Admin, String, String) {
    let api = Admin::login(&e.state).await;
    let csrf = api.get_ok("/admin/api/upstream").await["csrf_info"]
        .as_str()
        .unwrap()
        .to_owned();
    let id = e.state.nai.tokens().await[0].token_id.clone();
    (api, format!("/admin/api/upstream/{id}/details"), csrf)
}
fn script(e: &Env, info: Value) {
    e.mock.push("/user/information", Reply::json(200, &info));
    e.mock.push("/user/subscription",Reply::json(200,&json!({"tier":3,"paymentProcessorData":{"canary":"private-billing-canary"},"unknownCredit":0})));
    e.mock.push(
        "/user/priority",
        Reply::json(
            200,
            &json!({"taskPriority":0,"maxPriorityActions":1000,"nextRefillAt":0}),
        ),
    );
}

#[tokio::test]
async fn full_details_require_owner_origin_csrf_and_an_existing_account() {
    let _clock = clock(None).await;
    let e = setup(&[]).await;
    let (api, path, csrf) = context(&e).await;
    assert_eq!(
        Call::new("POST", &path)
            .json(json!({}))
            .send(&e.state)
            .await
            .0,
        401
    );
    assert_eq!(api.post(&path, json!({})).await.status, 403);
    assert_eq!(
        api.raw(
            "POST",
            &path,
            Some(json!({})),
            &[
                ("origin", "https://other.invalid"),
                ("x-nai-admin-csrf", &csrf)
            ]
        )
        .await
        .status,
        403
    );
    assert_eq!(
        api.raw(
            "POST",
            &path,
            Some(json!({"url":"https://other.invalid"})),
            &[("x-nai-admin-csrf", &csrf)]
        )
        .await
        .status,
        400
    );
    assert_eq!(
        api.raw(
            "POST",
            "/admin/api/upstream/missing/details",
            Some(json!({})),
            &[("x-nai-admin-csrf", &csrf)]
        )
        .await
        .status,
        404
    );
    assert!(e.mock.requests_to("/user/information").is_empty());
}

#[tokio::test]
async fn full_unknown_metadata_is_returned_without_persisting_raw_values_or_secrets() {
    let _clock = clock(None).await;
    let e = setup(&[("NAI_TOKENS", "pst-synthetic-one,pst-synthetic-two")]).await;
    let (api, path, csrf) = context(&e).await;
    script(
        &e,
        json!({"plainTextEmail":"private-email-canary@example.invalid","future":{"zero":0,"enabled":false,"echo":"pst-synthetic-one pst-synthetic-two"},"accessToken":"another-private-secret","keystore":"private-key-material","settings":"private-settings"}),
    );
    let r = api
        .raw(
            "POST",
            &path,
            Some(json!({})),
            &[("x-nai-admin-csrf", &csrf)],
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(r.headers["cache-control"], "no-store");
    let data = r.json();
    assert_eq!(
        data["information"]["data"]["plainTextEmail"],
        "private-email-canary@example.invalid"
    );
    assert_eq!(data["information"]["data"]["future"]["zero"], 0);
    assert_eq!(data["information"]["data"]["future"]["enabled"], false);
    assert_eq!(
        data["subscription"]["data"]["paymentProcessorData"]["canary"],
        "private-billing-canary"
    );
    assert_eq!(data["priority"]["data"]["taskPriority"], 0);
    for secret in [
        "pst-synthetic-one",
        "pst-synthetic-two",
        "another-private-secret",
        "private-key-material",
        "private-settings",
    ] {
        assert!(!r.text().contains(secret));
    }
    for endpoint in ["information", "subscription", "priority"] {
        assert_eq!(e.mock.requests_to(&format!("/user/{endpoint}")).len(), 1);
    }
    let tables = query(
        &e.state.db,
        "SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'",
    )
    .await;
    for table in tables {
        let name = table["name"].as_str().unwrap().replace('"', "\"\"");
        let rows = query(&e.state.db, &format!("SELECT * FROM \"{name}\"")).await;
        let stored = serde_json::to_string(&rows).unwrap();
        assert!(
            !stored.contains("private-email-canary") && !stored.contains("private-billing-canary"),
            "raw metadata persisted in {name}"
        );
    }
    assert!(
        !api.get_ok("/admin/api/upstream")
            .await
            .to_string()
            .contains("private-email-canary")
    );
}

#[tokio::test]
async fn concurrent_reads_have_one_attempt_per_account_and_no_global_cooldown() {
    let _clock = clock(None).await;
    let e = setup(&[("NAI_TOKENS", "pst-synthetic-one,pst-synthetic-two")]).await;
    let (api, path, csrf) = context(&e).await;
    script(&e, json!({"banStatus":"not_banned"}));
    let headers = [("x-nai-admin-csrf", csrf.as_str())];
    let (first, second) = tokio::join!(
        api.raw("POST", &path, Some(json!({})), &headers),
        api.raw("POST", &path, Some(json!({})), &headers)
    );
    let mut statuses = [first.status, second.status];
    statuses.sort();
    assert_eq!(statuses, [200, 429]);
    let throttled = if first.status == 429 { first } else { second };
    assert_eq!(throttled.headers["cache-control"], "no-store");
    assert!(
        throttled.headers["retry-after"]
            .to_str()
            .unwrap()
            .parse::<u64>()
            .unwrap()
            <= 60
    );
    assert_eq!(e.mock.requests_to("/user/information").len(), 1);
    let second_id = e.state.nai.tokens().await[1].token_id.clone();
    script(&e, json!({"banStatus":"not_banned"}));
    assert_eq!(
        api.raw(
            "POST",
            &format!("/admin/api/upstream/{second_id}/details"),
            Some(json!({})),
            &headers
        )
        .await
        .status,
        200
    );
    assert_eq!(e.mock.requests_to("/user/information").len(), 2);
    assert_eq!(e.state.ops.account_details_attempts.lock().await.len(), 2);
}

#[tokio::test]
async fn endpoint_failure_keeps_other_results_and_never_echoes_error_payload() {
    let _clock = clock(None).await;
    let e = setup(&[]).await;
    let (api, path, csrf) = context(&e).await;
    e.mock.push(
        "/user/information",
        Reply::status(503, "private-upstream-error-canary"),
    );
    e.mock.push(
        "/user/subscription",
        Reply::json(200, &json!({"active":false})),
    );
    e.mock.push(
        "/user/priority",
        Reply::status(200, "not JSON private-upstream-error-canary"),
    );
    let r = api
        .raw(
            "POST",
            &path,
            Some(json!({})),
            &[("x-nai-admin-csrf", &csrf)],
        )
        .await;
    assert_eq!(r.status, 200);
    assert_eq!(r.json()["information"]["status"], 503);
    assert_eq!(r.json()["subscription"]["data"]["active"], false);
    assert_eq!(r.json()["priority"]["data"], Value::Null);
    assert!(!r.text().contains("private-upstream-error-canary"));
}

#[tokio::test]
async fn per_account_cooldown_reopens_at_exactly_sixty_seconds() {
    const START: f64 = 1_793_000_000.0;
    let _clock = clock(Some(START)).await;
    let e = setup(&[]).await;
    let (api, path, csrf) = context(&e).await;
    script(&e, json!({"banStatus":"not_banned"}));
    let headers = [("x-nai-admin-csrf", csrf.as_str())];
    assert_eq!(
        api.raw("POST", &path, Some(json!({})), &headers)
            .await
            .status,
        200
    );
    gate_server::time::freeze(Some(START + 59.999));
    let denied = api.raw("POST", &path, Some(json!({})), &headers).await;
    assert_eq!(denied.status, 429);
    assert_eq!(denied.headers["retry-after"], "1");
    gate_server::time::freeze(Some(START + 60.0));
    script(&e, json!({"banStatus":"not_banned"}));
    assert_eq!(
        api.raw("POST", &path, Some(json!({})), &headers)
            .await
            .status,
        200
    );
    assert_eq!(e.mock.requests_to("/user/information").len(), 2);
    gate_server::time::freeze(None);
}

#[tokio::test]
async fn unauthorized_detail_isolates_only_its_account_and_survives_restart() {
    let _clock = clock(None).await;
    let e = setup(&[("NAI_TOKENS", "pst-synthetic-one,pst-synthetic-two")]).await;
    let (api, path, csrf) = context(&e).await;
    e.mock.push(
        "/user/information",
        Reply::status(401, "private-unauthorized-canary"),
    );
    let result = api
        .raw(
            "POST",
            &path,
            Some(json!({})),
            &[("x-nai-admin-csrf", &csrf)],
        )
        .await;
    assert_eq!(result.status, 200);
    assert!(!result.text().contains("private-unauthorized-canary"));
    let tokens = e.state.nai.tokens().await;
    assert!(tokens[0].fields().disabled);
    assert!(!tokens[1].fields().disabled);
    let restarted = e.restart().await;
    let tokens = restarted.nai.tokens().await;
    assert!(tokens[0].fields().disabled);
    assert!(!tokens[1].fields().disabled);
}

#[tokio::test]
async fn removed_account_does_not_return_its_late_sensitive_response() {
    let _clock = clock(None).await;
    let e = setup(&[("NAI_TOKENS", "pst-synthetic-one,pst-synthetic-two")]).await;
    let (api, path, csrf) = context(&e).await;
    let id = e.state.nai.tokens().await[0].token_id.clone();
    e.mock.push(
        "/user/information",
        Reply::Chunks {
            status: 200,
            content_type: "application/json".into(),
            chunks: vec![(
                std::time::Duration::from_millis(500),
                bytes::Bytes::from_static(
                    br#"{"plainTextEmail":"late-private-canary@example.invalid"}"#,
                ),
            )],
        },
    );
    let headers = [("x-nai-admin-csrf", csrf.as_str())];
    let (response, ()) = tokio::join!(api.raw("POST", &path, Some(json!({})), &headers), async {
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            while e.mock.requests_to("/user/information").is_empty() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(e.state.nai.remove_token(&id).await.unwrap());
    });
    assert_eq!(response.status, 409);
    assert!(!response.text().contains("late-private-canary"));
}
