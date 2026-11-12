//! Share policy, bridge acknowledgement and administrative control against real local routes.
mod common;
use axum::ServiceExt;
use common::*;
use gate_core::{bot_config, clock::Clock, share_detection::Report};
use gate_server::{AppState, key::Key, share_lock};
use serde_json::{Value, json};

const GUILD: &str = "900000000000000083";
const SECRET: &str = "synthetic-share-bridge-contract-secret-0001";
const THREAD: &str = "900000000000000082";

async fn report(state: &AppState, at: f64) {
    let detection = Report {
        connected: true,
        last_complete_at: at,
        complete_day_start: state.clock.day_start(at),
        ..Default::default()
    };
    let response = Call::new("POST", "/self-register/bot/report")
        .bearer(SECRET)
        .json(json!({"status":{"share_detection":detection}}))
        .send(state)
        .await;
    assert_eq!(response.0, 200, "{:?}", response.2);
}

async fn key(state: &AppState) -> Key {
    Key::load(&state.db, state.db.get_key(1).await.unwrap().unwrap())
        .await
        .unwrap()
}

async fn member_view(state: &AppState) -> Value {
    let key = key(state).await;
    let session = gate_server::session::sign_member(
        state,
        SUBJECT,
        state.now() as i64 + 3600,
        &gate_server::session::key_tag(Some(&key)),
    );
    let response = Call::new("GET", "/public/me")
        .header("cookie", format!("owl_member={session}"))
        .send(state)
        .await;
    assert_eq!(response.0, 200);
    serde_json::from_slice(&response.2).unwrap()
}

async fn share(state: &AppState, at: f64, extra: Value) -> axum::http::StatusCode {
    let id = ((((at * 1000.0) as u64 - 1_420_070_400_000) << 22) + 1).to_string();
    let mut body = json!({"discord_id":SUBJECT, "guild_id":GUILD,
        "message_id":id, "channel_id":THREAD, "parent_id":share_lock::FORUM_CHANNEL_ID,"at":at});
    body.as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    Call::new("POST", "/self-register/bot/share")
        .bearer(SECRET)
        .json(body)
        .send(state)
        .await
        .0
}

#[tokio::test]
async fn share_commit_unlocks_the_next_request_and_survives_key_rotation_and_restart() {
    let now = share_lock::starts_at() + 3600.0;
    let _clock = clock(Some(now)).await;
    let e = setup_registration(
        "http://127.0.0.1:9",
        &[
            ("DISCORD_GUILD_ID", GUILD),
            ("REGISTRATION_BRIDGE_SECRET", SECRET),
        ],
    )
    .await;
    e.state
        .db
        .save_registration(&gate_db::repo::registration::NewRegistration {
            discord_id: SUBJECT,
            key_id: 1,
            now,
            username: None,
            display_name: None,
            avatar: None,
            key_name: "synthetic",
            member_role: true,
            expires_at: None,
        })
        .await
        .unwrap();
    report(&e.state, now).await;
    let yesterday = e.state.clock.day(e.state.clock.day_start(now) - 1.0);
    add_usage(&e.state.db, 1, &yesterday, 10, 0.0).await;
    assert!(share_lock::is_locked(&e.state, &key(&e.state).await).await);
    assert!(member_view(&e.state).await.get("share_lock").is_some());
    let member_admin = Admin::login_bare(&e.state).await;
    assert_eq!(
        member_admin.get_ok("/members").await["members"][0]["share_locked"],
        true
    );

    for path in [
        "/ai/generate-image",
        "/ai/generate-image-stream",
        "/ai/augment-image",
        "/ai/upscale",
        "/ai/encode-vibe",
    ] {
        let r = Call::new("POST", path)
            .bearer(TOKEN)
            .json(json!({}))
            .send(&e.state)
            .await;
        assert_eq!(r.0, 403, "{path} {:?}", r.2);
        assert!(String::from_utf8_lossy(&r.2).contains("share_lock"));
    }
    // Yesterday's share cannot unlock today.
    assert_eq!(share(&e.state, now - 86400.0, json!({})).await, 200);
    assert!(share_lock::is_locked(&e.state, &key(&e.state).await).await);
    // The real bot uses mocked Discord REST and POSTs to the real gateway HTTP router.
    let discord_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let discord_addr = discord_listener.local_addr().unwrap();
    let discord = axum::Router::new()
        .fallback(|| async { axum::Json(json!({"parent_id":share_lock::FORUM_CHANNEL_ID})) });
    tokio::spawn(async move {
        axum::serve(discord_listener, discord).await.unwrap();
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = gate_server::app(e.state.clone());
    tokio::spawn(async move {
        axum::serve(listener, app.into_make_service())
            .await
            .unwrap();
    });
    let bot = gate_discord::naimei::Naimei::with_clock(
        gate_discord::config::NaimeiConfig {
            token: "synthetic".into(),
            guild_id: GUILD.parse().unwrap(),
            backend: format!("http://{addr}"),
            site: "https://site.test".into(),
            secret: SECRET.into(),
            chat: None,
            owner_ids: vec![],
            chat_log: None,
        },
        Some(discord_addr.to_string()),
        Box::new(move || now),
    );
    let message = json!({"id":((((now*1000.0) as u64-1_420_070_400_000)<<22)+1).to_string(),
        "channel_id":THREAD,"guild_id":GUILD,"author":{"id":SUBJECT,"username":"synthetic"},
        "attachments":[{"id":"1","filename":"image.png","content_type":"image/png"}]});
    bot.handle_message(serde_json::from_value(message).unwrap())
        .await;
    assert_eq!(bot.share_detection_report().pending, 0);
    assert_eq!(share(&e.state, now, json!({})).await, 200);
    assert_eq!(share(&e.state, now, json!({})).await, 200);
    assert!(!share_lock::is_locked(&e.state, &key(&e.state).await).await);
    assert!(member_view(&e.state).await.get("share_lock").is_none());
    assert!(
        member_admin.get_ok("/members").await["members"][0]
            .get("share_locked")
            .is_none()
    );

    let r = Call::new("POST","/ai/generate-image").bearer(TOKEN).json(json!({
        "input":"synthetic", "model":"nai-diffusion-4-5-full", "action":"generate",
        "parameters":{"width":832,"height":1216,"steps":28,"scale":5,"sampler":"k_euler_ancestral","n_samples":1,"seed":1234}
    })).send(&e.state).await;
    assert_eq!(r.0, 200, "{}", String::from_utf8_lossy(&r.2));
    let admin = Admin::login_bare(&e.state).await;
    assert_eq!(
        admin.post("/keys/1/regenerate", json!({})).await.status,
        200
    );
    assert!(!share_lock::is_locked(&e.state, &key(&e.state).await).await);
    let restarted = e.restart().await;
    assert!(!share_lock::is_locked(&restarted, &key(&restarted).await).await);
    assert_eq!(
        query(&e.state.db, "SELECT count(*) AS n FROM share_posts").await[0]["n"],
        2
    );
    assert!(key(&e.state).await.enabled());
}

#[tokio::test]
async fn switch_is_strict_logged_persistent_and_does_not_remove_other_restrictions() {
    let now = share_lock::starts_at() + 3600.0;
    let _clock = clock(Some(now)).await;
    let e = setup_registration("http://127.0.0.1:9", &[]).await;
    let admin = Admin::login_bare(&e.state).await;
    assert_eq!(admin.get_ok("/ops/share-lock").await["enabled"], true);
    for body in [
        json!({}),
        json!({"enabled":1}),
        json!({"enabled":"false"}),
        json!({"enabled":false,"extra":1}),
    ] {
        assert_eq!(admin.put("/ops/share-lock", body).await.status, 422);
    }
    assert_eq!(
        Call::new("PUT", "/admin/api/ops/share-lock")
            .json(json!({"enabled":false}))
            .send(&e.state)
            .await
            .0,
        401
    );
    assert_eq!(
        admin
            .put("/ops/share-lock", json!({"enabled":false}))
            .await
            .status,
        200
    );
    assert_eq!(
        setting(&e.state, share_lock::ENABLED_KEY).await.as_deref(),
        Some("0")
    );
    assert!(
        admin_actions(&e.state.db, 50)
            .await
            .iter()
            .any(|a| a["action"] == "分享限制" && a["detail"] == "关闭" && a["ok"] == 1)
    );
    let restarted = e.restart().await;
    assert_eq!(
        share_lock::detection(&restarted).await.unwrap()["enabled"],
        false
    );
    e.state
        .db
        .write(|tx| {
            tx.execute("UPDATE api_keys SET enabled=0 WHERE id=1", [])?;
            Ok(())
        })
        .await
        .unwrap();
    assert_eq!(
        Call::new("POST", "/ai/generate-image")
            .bearer(TOKEN)
            .json(json!({}))
            .send(&e.state)
            .await
            .0,
        403
    );
}

#[tokio::test]
async fn unhealthy_or_stale_detection_fails_open_and_reset_requires_a_new_full_sweep() {
    let now = share_lock::starts_at() + 3600.0;
    let _clock = clock(Some(now)).await;
    let e = setup_registration(
        "http://127.0.0.1:9",
        &[
            ("DISCORD_GUILD_ID", GUILD),
            ("REGISTRATION_BRIDGE_SECRET", SECRET),
        ],
    )
    .await;
    let yesterday = e.state.clock.day(e.state.clock.day_start(now) - 1.0);
    add_usage(&e.state.db, 1, &yesterday, 10, 0.0).await;
    assert!(!share_lock::is_locked(&e.state, &key(&e.state).await).await);
    report(&e.state, now - 299.0).await;
    assert!(share_lock::is_locked(&e.state, &key(&e.state).await).await);
    report(&e.state, now - 300.0).await;
    assert!(!share_lock::is_locked(&e.state, &key(&e.state).await).await);
    // Fresh heartbeat alone cannot renew the completion timestamp.
    let raw = e
        .state
        .db
        .get_setting(bot_config::STATUS_KEY)
        .await
        .unwrap()
        .unwrap();
    let mut value: Value = serde_json::from_str(&raw).unwrap();
    value["seen_at"] = json!(now);
    e.state
        .db
        .set_setting(bot_config::STATUS_KEY, &value.to_string())
        .await
        .unwrap();
    assert!(!share_lock::is_locked(&e.state, &key(&e.state).await).await);
    report(&e.state, now).await;
    assert!(share_lock::is_locked(&e.state, &key(&e.state).await).await);
    gate_server::time::freeze(Some(e.state.clock.day_start(now) + 86400.0));
    assert!(!share_lock::is_locked(&e.state, &key(&e.state).await).await);
    assert_eq!(Clock::service().local_hour(e.state.now()), 7);
}

#[tokio::test]
async fn forged_time_wrong_guild_and_wrong_forum_never_create_a_share() {
    let now = share_lock::starts_at() + 3600.0;
    let _clock = clock(Some(now)).await;
    let e = setup_registration(
        "http://127.0.0.1:9",
        &[
            ("DISCORD_GUILD_ID", GUILD),
            ("REGISTRATION_BRIDGE_SECRET", SECRET),
        ],
    )
    .await;
    assert_eq!(share(&e.state, now, json!({"at":now+1.0})).await, 422);
    assert_eq!(
        share(&e.state, now, json!({"guild_id":"900000000000000084"})).await,
        403
    );
    assert_eq!(
        share(&e.state, now, json!({"parent_id":"900000000000000085"})).await,
        403
    );
    assert_eq!(
        query(&e.state.db, "SELECT count(*) AS n FROM share_posts").await[0]["n"],
        0
    );
}

#[tokio::test]
async fn queued_request_rechecks_the_reset_and_current_switch_before_dispatch() {
    use gate_server::{
        admission::{DispatchRequirements, Est, reserve_image_budget},
        reqctx::ReqCtx,
    };
    let reset = share_lock::starts_at();
    let _clock = clock(Some(reset - 10.0)).await;
    let e = setup_registration(
        "http://127.0.0.1:9",
        &[
            ("DISCORD_GUILD_ID", GUILD),
            ("REGISTRATION_BRIDGE_SECRET", SECRET),
        ],
    )
    .await;
    add_usage(&e.state.db, 1, &e.state.day(), 10, 0.0).await;
    let ctx = ReqCtx::new();
    let reserved = reserve_image_budget(
        &e.state,
        &ctx,
        &key(&e.state).await,
        &Est::default(),
        1,
        DispatchRequirements::feature("image"),
    )
    .await
    .unwrap();
    gate_server::time::freeze(Some(reset));
    report(&e.state, reset).await;
    let error = reserved
        .prepare_dispatch("abc123", "1100000001")
        .await
        .unwrap_err();
    assert!(matches!(error,gate_server::error::ApiError::Gate(ref g) if g.code=="share_lock"));
    let admin = Admin::login_bare(&e.state).await;
    assert_eq!(
        admin
            .put("/ops/share-lock", json!({"enabled":false}))
            .await
            .status,
        200
    );
    reserved
        .prepare_dispatch("abc123", "1100000001")
        .await
        .unwrap();
    assert!(e.mock.requests_to("/ai/generate-image").is_empty());
    assert_eq!(
        e.state
            .db
            .get_counter(1, &e.state.day())
            .await
            .unwrap()
            .images,
        0
    );
}
