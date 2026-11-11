//! 赞助席：独立门户、插队、专属 Token。

mod common;
use common::{Admin, Call, SUBJECT, TOKEN, policy_for, send, settle, setup};
use gate_db::repo::keys::NewKey;
use gate_db::repo::patron::PatronRecord;
use gate_server::session::{PATRON_COOKIE, hash_password};
use gate_upstream::headers::token_id;
use serde_json::{Value, json};

const SUBJECT2: &str = "900000000000000002";
const TOKEN2: &str = "nai-test-member-key-000000000000000002";
const PORTAL: &str = "patron-portal-pass-1";
const SHARED: &str = "pst-synthetic-shared-token";
const DONATED: &str = "pst-synthetic-donated-token";

fn cookie_value(set: &str, name: &str) -> String {
    set.split(';')
        .next()
        .unwrap()
        .trim_start_matches(&format!("{name}="))
        .to_string()
}

async fn grant_seat(
    admin: &Admin,
    subject: &str,
    jump: bool,
    bound: Option<&str>,
    reset: bool,
) -> Value {
    let (status, body) = admin
        .json(
            "PUT",
            &format!("/members/{subject}/patron"),
            Some(json!({
                "enabled": true,
                "queue_jump": jump,
                "self_service": true,
                "bound_token_id": bound.unwrap_or(""),
                "reset_password": reset,
            })),
        )
        .await;
    assert_eq!(status, 200, "{body}");
    body
}

async fn patron_login(state: &gate_server::AppState, password: &str) -> String {
    let r = send(
        state,
        "POST",
        "/patron/api/login",
        None,
        Some(json!({"password": password})),
        &[],
    )
    .await;
    assert_eq!(r.status, 200, "{}", r.text());
    let set = r.headers["set-cookie"].to_str().unwrap();
    assert!(
        set.starts_with("nai_gate_patron=")
            && set.contains("HttpOnly")
            && set.contains("SameSite=strict")
            && set.contains("Path=/patron"),
        "{set}"
    );
    cookie_value(set, PATRON_COOKIE)
}

#[tokio::test]
async fn patron_portal_is_separate_from_admin_and_password_change_drops_the_session() {
    let e = setup(&[]).await;
    let admin = Admin::login(&e.state).await;
    let granted = grant_seat(&admin, SUBJECT, true, None, true).await;
    let password = granted["portal_password"].as_str().unwrap().to_string();
    assert!(!password.is_empty());
    assert!(
        admin
            .actions()
            .await
            .iter()
            .any(|(name, _, detail, ok)| name == "设置赞助席"
                && *ok
                && !detail.contains(&password)),
        "{:?}",
        admin.actions().await
    );

    send(
        &e.state,
        "POST",
        "/patron/api/login",
        None,
        Some(json!({"password": "wrong-password-xx"})),
        &[],
    )
    .await
    .assert_detail(401, "密码错误");

    let cookie = patron_login(&e.state, &password).await;
    let me = send(
        &e.state,
        "GET",
        "/patron/api/me",
        None,
        None,
        &[("cookie", &format!("{PATRON_COOKIE}={cookie}"))],
    )
    .await;
    assert_eq!(me.status, 200, "{}", me.text());
    let body = me.json();
    assert_eq!(body["queue_jump"], true);
    assert_eq!(body["exclusive_account"], false);
    assert!(body["welcome"].as_str().unwrap().contains("席位"));
    assert_eq!(body["features"]["image"], true);

    send(
        &e.state,
        "GET",
        "/admin/api/me",
        None,
        None,
        &[("cookie", &format!("{PATRON_COOKIE}={cookie}"))],
    )
    .await
    .assert_detail(401, "未登录或会话已过期");

    send(
        &e.state,
        "GET",
        "/patron/api/me",
        Some(&admin.cookie),
        None,
        &[],
    )
    .await
    .assert_detail(401, "未登录或会话已过期");

    let put = send(
        &e.state,
        "PUT",
        "/patron/api/me/features",
        None,
        Some(json!({"paid_parameters": true})),
        &[("cookie", &format!("{PATRON_COOKIE}={cookie}"))],
    )
    .await;
    assert_eq!(put.status, 200, "{}", put.text());
    assert_eq!(put.json()["features"]["paid_parameters"], true);
    let off = send(
        &e.state,
        "PUT",
        "/patron/api/me/features",
        None,
        Some(json!({"image": false})),
        &[("cookie", &format!("{PATRON_COOKIE}={cookie}"))],
    )
    .await;
    assert_eq!(off.status, 200, "{}", off.text());
    let (status, _, body) = Call::new("POST", "/ai/generate-image")
        .bearer(TOKEN)
        .json(json!({
            "input": "1girl, solo", "model": "nai-diffusion-4-5-full", "action": "generate",
            "parameters": {"width": 832, "height": 1216, "steps": 28, "scale": 5,
                "sampler": "k_euler_ancestral", "n_samples": 1, "seed": 1}
        }))
        .send(&e.state)
        .await;
    assert_eq!(status.as_u16(), 403, "{}", String::from_utf8_lossy(&body));
    let on = send(
        &e.state,
        "PUT",
        "/patron/api/me/features",
        None,
        Some(json!({"image": true})),
        &[("cookie", &format!("{PATRON_COOKIE}={cookie}"))],
    )
    .await;
    assert_eq!(on.status, 200, "{}", on.text());

    let change = send(
        &e.state,
        "POST",
        "/patron/api/me/password",
        None,
        Some(json!({"current": password, "new": "brand-new-portal"})),
        &[("cookie", &format!("{PATRON_COOKIE}={cookie}"))],
    )
    .await;
    assert_eq!(change.status, 200, "{}", change.text());
    send(
        &e.state,
        "GET",
        "/patron/api/me",
        None,
        None,
        &[("cookie", &format!("{PATRON_COOKIE}={cookie}"))],
    )
    .await
    .assert_detail(401, "未登录或会话已过期");
}

#[tokio::test]
async fn patrons_reuse_the_fast_lane_and_keep_the_per_member_queue() {
    let e = setup(&[]).await;
    let admin = Admin::login(&e.state).await;
    e.state
        .db
        .upsert_patron(PatronRecord {
            subject: SUBJECT.into(),
            enabled: true,
            queue_jump: true,
            self_service: true,
            bound_token_id: None,
            password_hash: hash_password(PORTAL),
        })
        .await
        .unwrap();
    let key: NewKey = serde_json::from_value(json!({
        "name": "第二位", "token": TOKEN2, "discord_id": SUBJECT2,
    }))
    .unwrap();
    e.state.db.create_key(&key, e.state.now()).await.unwrap();
    e.state
        .db
        .set_member_overrides(
            SUBJECT2,
            &gate_core::member_policy::MemberOverrides {
                daily_images: Some(100),
                rpm: Some(60),
                image: Some(true),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    grant_seat(&admin, SUBJECT2, true, None, true).await;

    e.state.guard.with(|g| {
        g.values.insert("queue_per_account".into(), 2);
        let cap = g.queue_cap(&[None]);
        assert_eq!(cap, Some(2));
        assert_eq!(g.admit_image(10, cap, 1.0, false), None);
        assert_eq!(g.admit_image(11, cap, 1.0, false), None);
        assert!(g.queue_full(cap));
        assert_eq!(
            g.admit_image(12, cap, 1.0, false)
                .expect("ordinary key blocked")
                .code(),
            gate_core::reasons::QUEUE_FULL
        );
    });

    for token in [TOKEN, TOKEN2] {
        let (status, _, body) = Call::new("POST", "/ai/generate-image")
            .bearer(token)
            .json(json!({
                "input": "1girl, solo", "model": "nai-diffusion-4-5-full", "action": "generate",
                "parameters": {"width": 832, "height": 1216, "steps": 28, "scale": 5,
                    "sampler": "k_euler_ancestral", "n_samples": 1, "seed": 1}
            }))
            .send(&e.state)
            .await;
        assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    }
    assert_eq!(e.mock.requests_to("/ai/generate-image").len(), 2);
}

#[tokio::test]
async fn exclusive_token_is_hidden_from_the_site_pool_and_does_not_fall_back() {
    let e = setup(&[("NAI_TOKENS", &format!("{SHARED},{DONATED}"))]).await;
    let admin = Admin::login(&e.state).await;
    let donated_id = token_id(DONATED);
    grant_seat(&admin, SUBJECT, true, Some(&donated_id), true).await;
    let taken = admin
        .json(
            "PUT",
            &format!("/members/{SUBJECT2}/patron"),
            Some(json!({
                "enabled": true,
                "queue_jump": true,
                "bound_token_id": donated_id,
            })),
        )
        .await;
    assert_eq!(taken.0, 409, "{}", taken.1);
    policy_for(
        &e.state,
        1,
        json!({"daily_anlas": 500, "paid_parameters": false}),
    )
    .await;
    assert!(e.state.nai.exclusive_tokens().await.contains(&donated_id));

    let key: NewKey = serde_json::from_value(json!({
        "name": "路人", "token": TOKEN2, "discord_id": SUBJECT2,
    }))
    .unwrap();
    e.state.db.create_key(&key, e.state.now()).await.unwrap();
    e.state
        .db
        .set_member_overrides(
            SUBJECT2,
            &gate_core::member_policy::MemberOverrides {
                daily_images: Some(100),
                rpm: Some(60),
                image: Some(true),
                paid_parameters: Some(true),
                ..Default::default()
            },
        )
        .await
        .unwrap();

    let (status, _, body) = Call::new("POST", "/ai/generate-image")
        .bearer(TOKEN)
        .json(json!({
            "input": "1girl, solo", "model": "nai-diffusion-4-5-full", "action": "generate",
            "parameters": {"width": 1536, "height": 1536, "steps": 36, "scale": 5,
                "sampler": "k_euler_ancestral", "n_samples": 2, "seed": 1}
        }))
        .send(&e.state)
        .await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    let first = &e.mock.requests_to("/ai/generate-image")[0];
    assert!(first.header("authorization").unwrap().contains(DONATED));
    assert_eq!(first.json()["parameters"]["n_samples"], 2);
    assert_eq!(first.json()["parameters"]["width"], 1536);

    let (status, _, body) = Call::new("POST", "/ai/generate-image")
        .bearer(TOKEN2)
        .json(json!({
            "input": "1girl, solo", "model": "nai-diffusion-4-5-full", "action": "generate",
            "parameters": {"width": 832, "height": 1216, "steps": 28, "scale": 5,
                "sampler": "k_euler_ancestral", "n_samples": 1, "seed": 1}
        }))
        .send(&e.state)
        .await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    let second = &e.mock.requests_to("/ai/generate-image")[1];
    assert!(second.header("authorization").unwrap().contains(SHARED));
    assert!(!second.header("authorization").unwrap().contains(DONATED));

    let (s, page) = admin.json("GET", "/upstream", None).await;
    assert_eq!(s, 200);
    let donated_row = page["accounts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["token_id"] == donated_id)
        .unwrap();
    assert_eq!(donated_row["exclusive"], true);

    admin
        .json(
            "PUT",
            &format!("/upstream-tokens/{donated_id}/enabled"),
            Some(json!({"enabled": false})),
        )
        .await;
    let (status, _, body) = Call::new("POST", "/ai/generate-image")
        .bearer(TOKEN)
        .json(json!({
            "input": "1girl, solo", "model": "nai-diffusion-4-5-full", "action": "generate",
            "parameters": {"width": 832, "height": 1216, "steps": 28, "scale": 5,
                "sampler": "k_euler_ancestral", "n_samples": 1, "seed": 1}
        }))
        .send(&e.state)
        .await;
    assert_eq!(status.as_u16(), 503, "{}", String::from_utf8_lossy(&body));
    let text = String::from_utf8_lossy(&body);
    assert!(text.contains("专属账号"), "{text}");
    assert!(!text.contains(SHARED));
    settle().await;
}
