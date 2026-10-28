mod common;
use common::{Admin, Call, SUBJECT, TOKEN, settle, setup};
use gate_server::AppState;
use gate_testkit::Reply;
use serde_json::{Value, json};

async fn generate(state: &AppState, path: &str) -> (u16, Value) {
    let (status, _, body) = Call::new("POST", path)
        .bearer(TOKEN)
        .json(json!({
            "input":"synthetic test","model":"nai-diffusion-4-5-full","action":"generate",
            "parameters":{"width":832,"height":1216,"steps":28,"scale":5,
                "sampler":"k_euler_ancestral","n_samples":1,"seed":1}
        }))
        .send(state)
        .await;
    (
        status.as_u16(),
        serde_json::from_slice(&body).unwrap_or(Value::Null),
    )
}

#[tokio::test]
async fn defaults_overrides_and_public_effective_policy_are_one_contract() {
    let env = setup(&[]).await;
    let admin = Admin::login_bare(&env.state).await;
    let path = format!("/members/{SUBJECT}/rules");
    assert_eq!(admin.json("PUT", &path, Some(json!({}))).await.0, 200);
    let mut defaults = admin.get_ok("/member-rules").await["defaults"].clone();
    defaults["daily_images"] = json!(250);
    assert_eq!(
        admin
            .json("PUT", "/member-rules", Some(defaults.clone()))
            .await
            .0,
        200
    );
    let (_, _, body) = Call::new("GET", "/v1/me")
        .bearer(TOKEN)
        .send(&env.state)
        .await;
    let view: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(view["policy"]["daily_images"], 250);
    assert_eq!(view["today"]["daily_images"], 250);
    assert!(view["today"].get("daily_text_tokens").is_none());
    assert!(view["today"].get("quota_limits").is_none());
    assert_eq!(
        admin
            .json("PUT", &path, Some(json!({"daily_images":0})))
            .await
            .0,
        200
    );
    defaults["daily_images"] = json!(300);
    assert_eq!(
        admin.json("PUT", "/member-rules", Some(defaults)).await.0,
        200
    );
    assert_eq!(admin.get_ok(&path).await["effective"]["daily_images"], 0);
    assert_eq!(generate(&env.state, "/ai/generate-image").await.0, 403);
    assert_eq!(
        admin
            .json("PUT", &path, Some(json!({"daily_images":null})))
            .await
            .0,
        200
    );
    assert_eq!(admin.get_ok(&path).await["effective"]["daily_images"], 300);
    assert_eq!(generate(&env.state, "/ai/generate-image").await.0, 200);
}

#[tokio::test]
async fn owner_labels_and_key_flags_do_not_bypass_explicit_rules() {
    let env = setup(&[]).await;
    let admin = Admin::login_bare(&env.state).await;
    env.state
        .db
        .set_setting("gateway_owner_discord_ids", &json!([SUBJECT]).to_string())
        .await
        .unwrap();
    env.state
        .db
        .set_setting("gateway_admin_discord_ids", &json!([SUBJECT]).to_string())
        .await
        .unwrap();
    assert_eq!(
        admin
            .json(
                "PUT",
                &format!("/members/{SUBJECT}/rules"),
                Some(json!({"image":false}))
            )
            .await
            .0,
        200
    );
    assert_eq!(generate(&env.state, "/ai/generate-image").await.0, 403);
    assert!(env.mock.requests_to("/ai/generate-image").is_empty());
    for body in [
        json!({"quota":"auto"}),
        json!({"daily_images":9999}),
        json!({"role":"invite"}),
    ] {
        assert_eq!(admin.json("PATCH", "/keys/1", Some(body)).await.0, 422);
    }
    for body in [
        json!({"is_admin":true}),
        json!({"daily_images":{"mode":"unlimited"}}),
        json!({"daily_images":-1}),
    ] {
        assert_eq!(
            admin
                .json("PUT", &format!("/members/{SUBJECT}/rules"), Some(body))
                .await
                .0,
            422
        );
    }
}

#[tokio::test]
async fn rule_endpoints_require_owner_session_and_cross_origin_writes_fail() {
    let env = setup(&[]).await;
    assert_eq!(
        Call::new("GET", "/admin/api/member-rules")
            .send(&env.state)
            .await
            .0,
        401
    );
    let admin = Admin::login_bare(&env.state).await;
    for subject in ["invalid", "01", "+1", "0", "18446744073709551616"] {
        let path = format!("/members/{subject}/rules");
        assert_eq!(admin.json("GET", &path, None).await.0, 422);
        assert_eq!(admin.json("PUT", &path, Some(json!({}))).await.0, 422);
    }

    let defaults = admin.get_ok("/member-rules").await["defaults"].clone();
    let status = Call::new("PUT", "/admin/api/member-rules")
        .header(
            "cookie",
            format!("{}={}", gate_server::session::ADMIN_COOKIE, admin.cookie),
        )
        .header("origin", "https://foreign.test")
        .json(defaults)
        .send(&env.state)
        .await
        .0;
    assert_eq!(status, 403);
}

#[tokio::test]
async fn normal_and_stream_generation_share_effective_accounting() {
    let env = setup(&[]).await;
    assert_eq!(generate(&env.state, "/ai/generate-image").await.0, 200);
    assert_eq!(
        generate(&env.state, "/ai/generate-image-stream").await.0,
        200
    );
    settle().await;
    assert_eq!(
        env.state
            .db
            .get_counter(1, &env.state.day())
            .await
            .unwrap()
            .images,
        2
    );
    let admin = Admin::login_bare(&env.state).await;
    assert_eq!(
        admin
            .json(
                "PUT",
                &format!("/members/{SUBJECT}/rules"),
                Some(json!({"daily_images":2}))
            )
            .await
            .0,
        200
    );
    for path in ["/ai/generate-image", "/ai/generate-image-stream"] {
        assert_eq!(generate(&env.state, path).await.0, 403);
    }
    assert_eq!(env.mock.requests_to("/ai/generate-image").len(), 1);
    assert_eq!(env.mock.requests_to("/ai/generate-image-stream").len(), 1);
}

#[tokio::test]
async fn v5_client_view_uses_member_permission_and_shared_budget_without_identity_roles() {
    let env = setup(&[]).await;
    let admin = Admin::login_bare(&env.state).await;
    let path = format!("/members/{SUBJECT}/rules");
    for allowed in [false, true] {
        assert_eq!(
            admin
                .json("PUT", &path, Some(json!({"v5":allowed,"daily_v5":4})))
                .await
                .0,
            200
        );
        let (status, _, body) = Call::new("GET", "/user/subscription")
            .bearer(TOKEN)
            .send(&env.state)
            .await;
        assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
        let view: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(view["naiGate"]["v5Allowed"], allowed);
        assert_eq!(view["naiGate"]["v5DailyLimit"], if allowed { 4 } else { 0 });
        assert!(view["naiGate"].get("quotaLimits").is_none());
        assert!(view["naiGate"].get("imageModelScope").is_none());
    }
    let mut runtime = admin.get_ok("/settings").await;
    assert!(runtime.get("gateway_owner_discord_ids").is_none());
    assert!(runtime.get("v5_access_mode").is_none());
    runtime["budgets"]["daily_v5"] = json!(0);
    assert_eq!(admin.json("PUT", "/settings", Some(runtime)).await.0, 200);
    let (_, _, body) = Call::new("GET", "/user/subscription")
        .bearer(TOKEN)
        .send(&env.state)
        .await;
    let view: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(view["naiGate"]["v5DailyLimit"], 0);
    assert_eq!(view["naiGate"]["v5LeftToday"], 0);
}

#[tokio::test]
async fn queued_requests_recheck_changed_defaults_before_dispatch_in_both_modes() {
    for path in ["/ai/generate-image", "/ai/generate-image-stream"] {
        let env = setup(&[]).await;
        let admin = Admin::login_bare(&env.state).await;
        assert_eq!(
            admin
                .json("PUT", &format!("/members/{SUBJECT}/rules"), Some(json!({})))
                .await
                .0,
            200
        );
        env.mock.push(
            "/ai/generate-image",
            Reply::Chunks {
                status: 200,
                content_type: "binary/octet-stream".into(),
                chunks: vec![(
                    std::time::Duration::from_millis(500),
                    bytes::Bytes::from(gate_testkit::zip_of_pngs(1)),
                )],
            },
        );
        let state = env.state.clone();
        let first = tokio::spawn(async move { generate(&state, "/ai/generate-image").await });
        for _ in 0..200 {
            if !env.mock.requests_to("/ai/generate-image").is_empty() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        assert_eq!(env.mock.requests_to("/ai/generate-image").len(), 1);
        let state = env.state.clone();
        let queued = tokio::spawn(async move { generate(&state, path).await });
        let mut waiting = false;
        for _ in 0..100 {
            if env.state.guard.with(|g| g.queue_view(Some(1)))["waiting"]
                .as_i64()
                .unwrap_or(0)
                > 0
            {
                waiting = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        }
        assert!(waiting);
        let mut defaults = admin.get_ok("/member-rules").await["defaults"].clone();
        defaults["daily_images"] = json!(0);
        assert_eq!(
            admin.json("PUT", "/member-rules", Some(defaults)).await.0,
            200
        );
        assert_eq!(first.await.unwrap().0, 200);
        assert_eq!(queued.await.unwrap().0, 403);
        settle().await;
        assert_eq!(env.mock.requests_to("/ai/generate-image").len(), 1);
        assert!(env.mock.requests_to("/ai/generate-image-stream").is_empty());
        assert_eq!(
            env.state
                .db
                .get_counter(1, &env.state.day())
                .await
                .unwrap()
                .images,
            1
        );
        let uncertain = env
            .state
            .db
            .read(|c| {
                c.query_row(
                    "SELECT count(*) FROM request_settlements WHERE status='uncertain'",
                    [],
                    |r| r.get::<_, i64>(0),
                )
            })
            .await
            .unwrap();
        assert_eq!(uncertain, 0);
    }
}
