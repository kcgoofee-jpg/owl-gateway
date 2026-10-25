mod common;

use common::{Admin, Call, TOKEN, setup};
use gate_testkit::Reply;
use serde_json::{Value, json};

fn image_body(model: &str) -> Value {
    json!({
        "input": "1girl, solo", "model": model, "action": "generate",
        "parameters": {"width": 832, "height": 1216, "steps": 28, "scale": 5,
                       "sampler": "k_euler_ancestral", "n_samples": 1, "seed": 7},
    })
}

async fn generate(state: &gate_server::AppState, model: &str) -> (u16, Vec<u8>) {
    let (status, _, body) = Call::new("POST", "/ai/generate-image")
        .bearer(TOKEN)
        .json(image_body(model))
        .send(state)
        .await;
    (status.as_u16(), body)
}

async fn disabled(state: &gate_server::AppState, id: &str) -> bool {
    state
        .nai
        .tokens()
        .await
        .into_iter()
        .find(|t| t.token_id == id)
        .unwrap()
        .fields()
        .disabled
}

#[tokio::test]
async fn image_401_stays_isolated_after_restart_while_other_account_remains_usable() {
    let e = setup(&[("NAI_TOKENS", "pst-synthetic-first,pst-synthetic-second")]).await;
    let ids: Vec<_> = e
        .state
        .nai
        .tokens()
        .await
        .iter()
        .map(|t| t.token_id.clone())
        .collect();
    e.mock.push(
        "/ai/generate-image",
        Reply::status(401, "synthetic unauthorized"),
    );
    assert_eq!(generate(&e.state, "nai-diffusion-4-5-full").await.0, 502);
    let first_request = e.mock.requests_to("/ai/generate-image");
    let failed_token = first_request[0]
        .headers
        .iter()
        .find(|(name, _)| name == "authorization")
        .unwrap()
        .1
        .trim_start_matches("Bearer ");
    let failed_id = gate_upstream::headers::token_id(failed_token);
    let available_id = ids.iter().find(|id| **id != failed_id).unwrap();
    assert!(disabled(&e.state, &failed_id).await);
    assert!(
        e.state
            .nai
            .tokens()
            .await
            .into_iter()
            .find(|token| token.token_id == failed_id)
            .unwrap()
            .fields()
            .admin_enabled
    );

    let stored = e
        .state
        .db
        .get_setting(gate_server::hooks::INVALID_TOKENS_KEY)
        .await
        .unwrap()
        .unwrap();
    assert!(stored.contains(&failed_id));
    assert!(!stored.contains("pst-synthetic"));

    let restarted = e.restart().await;
    assert!(disabled(&restarted, &failed_id).await);
    assert!(!disabled(&restarted, available_id).await);
    let (status, body) = generate(&restarted, "nai-diffusion-4-5-full").await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    let sent = e.mock.requests_to("/ai/generate-image");
    assert_eq!(sent.len(), 2);
    let replacement_auth =
        if available_id == &gate_upstream::headers::token_id("pst-synthetic-second") {
            "Bearer pst-synthetic-second"
        } else {
            "Bearer pst-synthetic-first"
        };
    assert!(
        sent[1]
            .headers
            .iter()
            .any(|(name, value)| name == "authorization" && value == replacement_auth)
    );
}

#[tokio::test]
async fn successfully_verified_replacement_clears_only_the_replaced_fingerprint() {
    let e = setup(&[("NAI_TOKENS", "pst-synthetic-first,pst-synthetic-second")]).await;
    let tokens = e.state.nai.tokens().await;
    let old_ids: Vec<_> = tokens.iter().map(|t| t.token_id.clone()).collect();
    e.mock.push(
        "/ai/generate-image",
        Reply::status(401, "synthetic unauthorized"),
    );
    assert_eq!(generate(&e.state, "nai-diffusion-4-5-full").await.0, 502);
    let failed_request = e.mock.requests_to("/ai/generate-image");
    let failed_token = failed_request[0]
        .headers
        .iter()
        .find(|(name, _)| name == "authorization")
        .unwrap()
        .1
        .trim_start_matches("Bearer ");
    let old_id = gate_upstream::headers::token_id(failed_token);
    let replacement_id = gate_upstream::headers::token_id("pst-synthetic-valid-replacement");
    e.state
        .db
        .set_setting(
            gate_server::hooks::INVALID_TOKENS_KEY,
            &json!([old_id.clone(), replacement_id.clone()]).to_string(),
        )
        .await
        .unwrap();

    let admin = Admin::login(&e.state).await;
    let replaced = admin
        .put(
            &format!("/upstream-tokens/{old_id}"),
            json!({"token": "pst-synthetic-valid-replacement"}),
        )
        .await;
    assert_eq!(replaced.status, 200, "{}", replaced.text());
    let new_id = replacement_id;
    let new = e
        .state
        .nai
        .tokens()
        .await
        .into_iter()
        .find(|token| token.token_id == new_id)
        .unwrap();
    assert_ne!(new.token_id, old_id);
    assert!(!new.fields().disabled);
    assert!(old_ids.contains(&old_id));

    let stored = e
        .state
        .db
        .get_setting(gate_server::hooks::INVALID_TOKENS_KEY)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored, "[]");
    let restarted = e.restart().await;
    assert!(!disabled(&restarted, &new.token_id).await);
}

#[tokio::test]
async fn subscription_401_isolates_before_generation_but_zero_percent_and_402_do_not() {
    let e = setup(&[]).await;
    e.state
        .db
        .run_script(vec![
            "UPDATE api_keys SET image_model_scope='all', daily_v5=10 WHERE id=1".into(),
        ])
        .await
        .unwrap();
    let id = e.state.nai.tokens().await[0].token_id.clone();
    e.mock.push(
        "/user/subscription",
        Reply::status(401, "synthetic unauthorized"),
    );
    let (status, body) = generate(&e.state, "nai-diffusion-5-full").await;
    assert_ne!(status, 200, "{}", String::from_utf8_lossy(&body));
    assert!(disabled(&e.state, &id).await);
    assert!(e.mock.requests_to("/ai/generate-image").is_empty());

    let balance = setup(&[]).await;
    let balance_id = balance.state.nai.tokens().await[0].token_id.clone();
    balance.mock.push(
        "/ai/generate-image",
        Reply::status(402, "synthetic insufficient Anlas"),
    );
    let _ = generate(&balance.state, "nai-diffusion-4-5-full").await;
    assert!(!disabled(&balance.state, &balance_id).await);

    let model_forbidden = setup(&[]).await;
    let forbidden_id = model_forbidden.state.nai.tokens().await[0].token_id.clone();
    model_forbidden.mock.push(
        "/ai/generate-image",
        Reply::status(403, "synthetic model restriction"),
    );
    let _ = generate(&model_forbidden.state, "nai-diffusion-4-5-full").await;
    assert!(!disabled(&model_forbidden.state, &forbidden_id).await);

    let local_failure = setup(&[
        ("NAI_TOKENS", "pst-synthetic-other"),
        ("NAI_IMAGE_HOST", "http://127.0.0.1:9"),
    ])
    .await;
    local_failure
        .state
        .db
        .run_script(vec![
            "UPDATE api_keys SET image_model_scope='all', daily_v5=10 WHERE id=1".into(),
        ])
        .await
        .unwrap();
    let other_id = local_failure.state.nai.tokens().await[0].token_id.clone();
    let _ = generate(&local_failure.state, "nai-diffusion-5-full").await;
    assert!(!disabled(&local_failure.state, &other_id).await);

    let zero = setup(&[]).await;
    zero.mock.set_v5_percent(0);
    zero.state
        .db
        .run_script(vec![
            "UPDATE api_keys SET image_model_scope='all', daily_v5=10 WHERE id=1".into(),
        ])
        .await
        .unwrap();
    let zero_id = zero.state.nai.tokens().await[0].token_id.clone();
    let _ = generate(&zero.state, "nai-diffusion-5-full").await;
    assert!(!disabled(&zero.state, &zero_id).await);
}

#[tokio::test]
async fn information_401_persists_isolation_without_treating_other_errors_as_401() {
    let e = setup(&[]).await;
    let id = e.state.nai.tokens().await[0].token_id.clone();
    let admin = Admin::login(&e.state).await;
    let status = admin.get_ok("/admin/api/upstream-information").await;
    let csrf = status["csrf_token"].as_str().unwrap();
    e.mock.push(
        "/user/information",
        Reply::status(401, "synthetic unauthorized"),
    );
    let refreshed = admin
        .raw(
            "POST",
            "/admin/api/upstream-information",
            Some(json!({})),
            &[("x-nai-admin-csrf", csrf)],
        )
        .await;
    assert_eq!(refreshed.status, 200, "{}", refreshed.text());
    assert_eq!(
        refreshed.json()["accounts"][0]["last_error"],
        "上游凭据被拒绝"
    );
    assert!(disabled(&e.state, &id).await);

    let normal = setup(&[]).await;
    let normal_id = normal.state.nai.tokens().await[0].token_id.clone();
    normal.mock.push(
        "/user/information",
        Reply::status(503, "synthetic unavailable"),
    );
    let admin = Admin::login(&normal.state).await;
    let state = admin.get_ok("/admin/api/upstream-information").await;
    let csrf = state["csrf_token"].as_str().unwrap();
    admin
        .raw(
            "POST",
            "/admin/api/upstream-information",
            Some(json!({})),
            &[("x-nai-admin-csrf", csrf)],
        )
        .await;
    assert!(!disabled(&normal.state, &normal_id).await);
}
