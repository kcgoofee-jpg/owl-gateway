//! 赞助席：独立门户、插队、捐赠 Token 管理（捐赠号共享入池，不独占选号）。

mod common;
use common::{Admin, Call, SUBJECT, TOKEN, generate, policy_for, send, settle, setup};
use gate_core::lane::Lane;
use gate_core::policy::mask_token;
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
    assert_eq!(body["donated_account"], false);
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
async fn patrons_skip_the_site_cap_and_keep_the_per_member_queue() {
    let e = setup(&[]).await;
    let admin = Admin::login(&e.state).await;
    e.state
        .db
        .save_patron(
            None,
            PatronRecord {
                subject: SUBJECT.into(),
                enabled: true,
                queue_jump: true,
                self_service: true,
                bound_token_id: None,
                password_hash: hash_password(PORTAL),
                portal_mode: "basic".into(),
            },
        )
        .await
        .unwrap()
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
        assert_eq!(g.admit_image(10, cap, 1.0, Lane::Normal), None);
        assert_eq!(g.admit_image(11, cap, 1.0, Lane::Normal), None);
        assert!(g.queue_full(cap));
        assert_eq!(
            g.admit_image(12, cap, 1.0, Lane::Normal)
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
async fn donated_token_is_pooled_for_community_and_seat_does_not_pin() {
    let e = setup(&[("NAI_TOKENS", &format!("{SHARED},{DONATED}"))]).await;
    let admin = Admin::login(&e.state).await;
    let donated_id = token_id(DONATED);
    let shared_id = token_id(SHARED);
    grant_seat(&admin, SUBJECT, true, Some(&donated_id), true).await;
    // 一把捐赠号同一时刻只能绑给一个启用的席位。
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

    // 社区路人成员（不是赞助席）：普通免费规则。
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
                ..Default::default()
            },
        )
        .await
        .unwrap();

    // 捐赠号进全站共享池：只留它可用时，社区流量必须能选到它（旧独占机制下这里会 503）。
    admin
        .json(
            "PUT",
            &format!("/upstream-tokens/{shared_id}/enabled"),
            Some(json!({"enabled": false})),
        )
        .await;
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
    let first = &e.mock.requests_to("/ai/generate-image")[0];
    assert!(first.header("authorization").unwrap().contains(DONATED));

    // 赞助席本人不被钉死在捐赠号上：捐赠号停用、另一把健康号可用时，照常改派出图。
    admin
        .json(
            "PUT",
            &format!("/upstream-tokens/{shared_id}/enabled"),
            Some(json!({"enabled": true})),
        )
        .await;
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
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    let second = &e.mock.requests_to("/ai/generate-image")[1];
    assert!(second.header("authorization").unwrap().contains(SHARED));

    // 后台来源标签：捐赠号标 donated（仅标签，不影响选号）。
    let (s, page) = admin.json("GET", "/upstream", None).await;
    assert_eq!(s, 200);
    let donated_row = page["accounts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["token_id"] == donated_id)
        .unwrap();
    assert_eq!(donated_row["donated"], true);
    settle().await;
}

/// 赞助席绑定免费拼车号（allow_anlas=false）、本人没开高规格开关时仍套免费钳制：
/// 超大 v4.5 被收到免费小图。捐赠号是共享容量，不额外授予成员高规格权限。
#[tokio::test]
async fn bound_free_share_still_clamps_to_free_small_image() {
    let share = gate_testkit::MockUpstream::start().await;
    // 池里只留这把捐赠拼车号（无官方号），出图必然走它，便于核对钳制后的参数。
    let e = setup(&[("NAI_TOKENS", "")]).await;
    let admin = Admin::login(&e.state).await;
    let id = add_share(&admin, &share.url(), SHARE_A).await;
    policy_for(
        &e.state,
        1,
        json!({"daily_anlas": 5000, "paid_parameters": false}),
    )
    .await;
    seat_mode(&admin, SUBJECT, Some(&id), "full", true, true).await;

    let (status, _, body) = Call::new("POST", "/ai/generate-image")
        .bearer(TOKEN)
        .json(json!({
            "input": "1girl, solo", "model": "nai-diffusion-4-5-full", "action": "generate",
            "parameters": {"width": 1536, "height": 1536, "steps": 36, "scale": 5,
                "sampler": "k_euler_ancestral", "n_samples": 2, "seed": 1}
        }))
        .send(&e.state)
        .await;
    assert_eq!(status.as_u16(), 200, "{}", String::from_utf8_lossy(&body));
    let sent = share.requests_to("/ai/generate-image");
    assert_eq!(sent.len(), 1);
    assert!(sent[0].header("authorization").unwrap().contains(SHARE_A));
    let p = &sent[0].json()["parameters"];
    assert_eq!(p["n_samples"], 1, "{p}");
    assert!(p["steps"].as_i64().unwrap() <= 28, "{p}");
    let pixels = p["width"].as_i64().unwrap() * p["height"].as_i64().unwrap();
    assert!(pixels <= 1024 * 1024, "{p}");
    settle().await;
}

// ---------------------------------------------------------------- 捐赠 Key 自助

const SHARE_A: &str = "pk-synthetic-donated-share01";
const SHARE_B: &str = "pk-synthetic-donated-share02";
const SHARE_NEW: &str = "pk-synthetic-rotated-share03";

async fn add_share(api: &Admin, host: &str, key: &str) -> String {
    let (status, body) = api
        .json(
            "POST",
            "/share-upstreams",
            Some(json!({"token": key, "image_host": host, "allow_anlas": false})),
        )
        .await;
    assert_eq!(status, 200, "{body}");
    token_id(key)
}

async fn share_concurrency(api: &Admin, id: &str, concurrency: i64) {
    let (status, body) = api
        .json(
            "PUT",
            &format!("/share-upstreams/{id}/settings"),
            Some(json!({
                "allow_anlas": false, "daily_anlas": 0, "rpm": 12,
                "concurrency": concurrency, "post_wait_min": 1, "post_wait_max": 3,
            })),
        )
        .await;
    assert_eq!(status, 200, "{body}");
}

async fn seat_mode(
    admin: &Admin,
    subject: &str,
    bound: Option<&str>,
    mode: &str,
    self_service: bool,
    reset: bool,
) -> Value {
    let (status, body) = admin
        .json(
            "PUT",
            &format!("/members/{subject}/patron"),
            Some(json!({
                "enabled": true,
                "queue_jump": true,
                "self_service": self_service,
                "bound_token_id": bound.unwrap_or(""),
                "portal_mode": mode,
                "reset_password": reset,
            })),
        )
        .await;
    assert_eq!(status, 200, "{body}");
    body
}

fn patron_cookie(cookie: &str) -> String {
    format!("{PATRON_COOKIE}={cookie}")
}

#[tokio::test]
async fn patron_me_shows_the_bound_share_token_in_plaintext_but_hides_the_host() {
    let share = gate_testkit::MockUpstream::start().await;
    let e = setup(&[]).await;
    let admin = Admin::login(&e.state).await;
    let id = add_share(&admin, &share.url(), SHARE_A).await;
    share_concurrency(&admin, &id, 3).await;
    let (s, b) = admin
        .json(
            "PUT",
            &format!("/upstream-tokens/{id}/v5-limit"),
            Some(json!({"v5_daily_limit": 0})),
        )
        .await;
    assert_eq!(s, 200, "{b}");
    let granted = seat_mode(&admin, SUBJECT, Some(&id), "full", true, true).await;
    let password = granted["portal_password"].as_str().unwrap().to_string();
    let cookie = patron_login(&e.state, &password).await;
    let me = send(
        &e.state,
        "GET",
        "/patron/api/me",
        None,
        None,
        &[("cookie", &patron_cookie(&cookie))],
    )
    .await;
    assert_eq!(me.status, 200, "{}", me.text());
    let body = me.json();
    assert_eq!(body["portal_mode"], "full");
    assert_eq!(body["donated_account"], true);
    let token = &body["token"];
    assert_eq!(token["is_share"], true);
    assert_eq!(token["enabled"], true);
    assert_eq!(token["admin_disabled"], false);
    assert_eq!(token["isolated"], false);
    assert_eq!(token["concurrency"], 3);
    assert_eq!(token["active"], 0);
    assert_eq!(token["images_today"], 0);
    assert_eq!(token["v5_used"], 0);
    assert_eq!(token["v5_daily_limit"], 0);
    assert_eq!(token["fails"], 0);
    assert_eq!(token["blocked_for"], 0);
    assert_eq!(token["fingerprint"], json!(mask_token(SHARE_A, 6)));
    // 捐赠者本人门户明文显示完整 Key（站长 11-13：赞助席没必要隐藏 key）；网关地址仍不下发。
    assert_eq!(token["token"], json!(SHARE_A));
    let text = me.text();
    assert!(text.contains(SHARE_A), "本人门户应明文显示完整 Key: {text}");
    assert!(!text.contains(&share.url()), "网关地址泄漏: {text}");

    // 不绑捐赠号的席位：token 块为 null，门户模式默认 basic。
    let other = seat_mode(&admin, SUBJECT2, None, "basic", true, true).await;
    let pw2 = other["portal_password"].as_str().unwrap().to_string();
    let cookie2 = patron_login(&e.state, &pw2).await;
    let me2 = send(
        &e.state,
        "GET",
        "/patron/api/me",
        None,
        None,
        &[("cookie", &patron_cookie(&cookie2))],
    )
    .await;
    assert_eq!(me2.status, 200, "{}", me2.text());
    assert_eq!(me2.json()["token"], Value::Null);
    assert_eq!(me2.json()["portal_mode"], "basic");
    assert_eq!(me2.json()["donated_account"], false);
}

#[tokio::test]
async fn patron_token_self_service_auth_matrix() {
    let share = gate_testkit::MockUpstream::start().await;
    let e = setup(&[]).await;
    let admin = Admin::login(&e.state).await;
    let id = add_share(&admin, &share.url(), SHARE_A).await;
    let id2 = add_share(&admin, &share.url(), SHARE_B).await;

    // 未登录 401。
    send(
        &e.state,
        "PUT",
        "/patron/api/me/token/enabled",
        None,
        Some(json!({"enabled": false})),
        &[],
    )
    .await
    .assert_detail(401, "未登录或会话已过期");
    send(
        &e.state,
        "PUT",
        "/patron/api/me/token/rotate",
        None,
        Some(json!({"token": SHARE_NEW})),
        &[],
    )
    .await
    .assert_detail(401, "未登录或会话已过期");

    // basic 模式：能看不能用。
    let granted = seat_mode(&admin, SUBJECT, Some(&id), "basic", true, true).await;
    let password = granted["portal_password"].as_str().unwrap().to_string();
    let cookie = patron_login(&e.state, &password).await;
    let hdrs: &[(&str, &str)] = &[("cookie", &patron_cookie(&cookie))];
    send(
        &e.state,
        "PUT",
        "/patron/api/me/token/enabled",
        None,
        Some(json!({"enabled": false})),
        hdrs,
    )
    .await
    .assert_detail(403, "此席位未开放 Key 自助管理，请联系站长");
    send(
        &e.state,
        "PUT",
        "/patron/api/me/token/rotate",
        None,
        Some(json!({"token": SHARE_NEW})),
        hdrs,
    )
    .await
    .assert_detail(403, "此席位未开放 Key 自助管理，请联系站长");

    // full 但自助总开关关闭。
    seat_mode(&admin, SUBJECT, Some(&id), "full", false, false).await;
    send(
        &e.state,
        "PUT",
        "/patron/api/me/token/enabled",
        None,
        Some(json!({"enabled": false})),
        hdrs,
    )
    .await
    .assert_detail(403, "此席位未开放自助管理，请联系站长");

    // full + 自助，但没绑捐赠号。
    seat_mode(&admin, SUBJECT, None, "full", true, false).await;
    send(
        &e.state,
        "PUT",
        "/patron/api/me/token/enabled",
        None,
        Some(json!({"enabled": false})),
        hdrs,
    )
    .await
    .assert_detail(403, "此席位没有绑定捐赠 Key");

    // 席位被站长关闭：会话立刻失效（401，而不是 403）。
    let (s, _) = admin
        .json(
            "PUT",
            &format!("/members/{SUBJECT}/patron"),
            Some(json!({"enabled": false})),
        )
        .await;
    assert_eq!(s, 200);
    send(
        &e.state,
        "PUT",
        "/patron/api/me/token/enabled",
        None,
        Some(json!({"enabled": false})),
        hdrs,
    )
    .await
    .assert_detail(401, "未登录或会话已过期");

    // 恢复 full 席位：body 校验与 Key 前缀校验。
    seat_mode(&admin, SUBJECT, Some(&id), "full", true, false).await;
    send(
        &e.state,
        "PUT",
        "/patron/api/me/token/enabled",
        None,
        Some(json!({})),
        hdrs,
    )
    .await
    .assert_detail(422, "enabled 必须是布尔值");
    send(
        &e.state,
        "PUT",
        "/patron/api/me/token/enabled",
        None,
        Some(json!({"enabled": "yes"})),
        hdrs,
    )
    .await
    .assert_detail(422, "enabled 必须是布尔值");
    for bad in ["pst-synthetic-official-0001", "pk-short", ""] {
        send(
            &e.state,
            "PUT",
            "/patron/api/me/token/rotate",
            None,
            Some(json!({"token": bad})),
            hdrs,
        )
        .await
        .assert_detail(422, "Key 格式不对：应以 NAI- 或 pk- 开头");
    }

    // 只能动自己绑的那把：另一个席位的捐赠号不受影响。
    seat_mode(&admin, SUBJECT2, Some(&id2), "full", true, true).await;
    let off = send(
        &e.state,
        "PUT",
        "/patron/api/me/token/enabled",
        None,
        Some(json!({"enabled": false})),
        hdrs,
    )
    .await;
    assert_eq!(off.status, 200, "{}", off.text());
    let tokens = e.state.nai.tokens().await;
    let mine = tokens.iter().find(|t| t.token_id == id).unwrap();
    let theirs = tokens.iter().find(|t| t.token_id == id2).unwrap();
    assert!(!mine.fields().admin_enabled);
    assert!(theirs.fields().admin_enabled);
}

#[tokio::test]
async fn patron_full_seat_toggles_and_rotates_the_bound_share_key() {
    let share = gate_testkit::MockUpstream::start().await;
    let e = setup(&[]).await;
    let admin = Admin::login(&e.state).await;
    let id = add_share(&admin, &share.url(), SHARE_A).await;
    share_concurrency(&admin, &id, 3).await;
    let (s, b) = admin
        .json(
            "PUT",
            &format!("/upstream-tokens/{id}/v5-limit"),
            Some(json!({"v5_daily_limit": 0})),
        )
        .await;
    assert_eq!(s, 200, "{b}");
    let granted = seat_mode(&admin, SUBJECT, Some(&id), "full", true, true).await;
    let password = granted["portal_password"].as_str().unwrap().to_string();
    let cookie = patron_login(&e.state, &password).await;
    let hdrs: &[(&str, &str)] = &[("cookie", &patron_cookie(&cookie))];

    // 自助停用：与后台开关同一条路径，列表同步。
    let off = send(
        &e.state,
        "PUT",
        "/patron/api/me/token/enabled",
        None,
        Some(json!({"enabled": false})),
        hdrs,
    )
    .await;
    assert_eq!(off.status, 200, "{}", off.text());
    assert_eq!(off.json()["token"]["enabled"], false);
    assert_eq!(off.json()["token"]["admin_disabled"], true);
    let listed = admin.get_ok("/share-upstreams").await;
    let row = listed["accounts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["token_id"] == id)
        .unwrap();
    assert_eq!(row["admin_enabled"], false);
    let on = send(
        &e.state,
        "PUT",
        "/patron/api/me/token/enabled",
        None,
        Some(json!({"enabled": true})),
        hdrs,
    )
    .await;
    assert_eq!(on.status, 200, "{}", on.text());
    assert_eq!(on.json()["token"]["enabled"], true);

    // 换到池里另一把 Key：409 冲突，席位绑定不动。
    add_share(&admin, &share.url(), SHARE_B).await;
    send(
        &e.state,
        "PUT",
        "/patron/api/me/token/rotate",
        None,
        Some(json!({"token": SHARE_B})),
        hdrs,
    )
    .await
    .assert_detail(409, "这把 Token 已经在令牌池的另一个位置");
    assert_eq!(
        e.state
            .db
            .patron(SUBJECT)
            .await
            .unwrap()
            .unwrap()
            .bound_token_id
            .as_deref(),
        Some(id.as_str())
    );

    // 正常换 Key：验活 → 替换 → 席位重绑新指纹；并发与 V5 限额延续。
    let rotated = send(
        &e.state,
        "PUT",
        "/patron/api/me/token/rotate",
        None,
        Some(json!({"token": SHARE_NEW})),
        hdrs,
    )
    .await;
    assert_eq!(rotated.status, 200, "{}", rotated.text());
    let body = rotated.json();
    let new_id = token_id(SHARE_NEW);
    assert_eq!(
        body["token"]["fingerprint"],
        json!(mask_token(SHARE_NEW, 6))
    );
    assert_eq!(body["token"]["concurrency"], 3);
    assert_eq!(body["token"]["v5_daily_limit"], 0);
    assert_eq!(body["token"]["enabled"], true);
    assert!(
        rotated.text().contains(SHARE_NEW),
        "本人门户应明文显示更换后的完整 Key"
    );
    assert!(!rotated.text().contains(&share.url()), "网关地址泄漏");
    assert_eq!(
        e.state
            .db
            .patron(SUBJECT)
            .await
            .unwrap()
            .unwrap()
            .bound_token_id
            .as_deref(),
        Some(new_id.as_str())
    );
    let donated = e.state.db.donated_token_ids().await.unwrap();
    assert!(donated.contains(&new_id) && !donated.contains(&id));
    let listed = admin.get_ok("/share-upstreams").await;
    let rows = listed["accounts"].as_array().unwrap();
    let ids: Vec<&str> = rows
        .iter()
        .map(|a| a["token_id"].as_str().unwrap())
        .collect();
    assert!(!ids.contains(&id.as_str()) && ids.contains(&new_id.as_str()));
    // 来源标签：绑定到启用赞助席的新 Key 是捐赠（donated），池里另一把未绑定的是拼车平台。
    let new_row = rows.iter().find(|a| a["token_id"] == new_id).unwrap();
    assert_eq!(new_row["donated"], json!(true));
    let b_row = rows
        .iter()
        .find(|a| a["token_id"] == token_id(SHARE_B))
        .unwrap();
    assert_eq!(b_row["donated"], json!(false));

    // 席位成员仍能正常出图：捐赠号共享入池，不再钉死路由（可能走池里任意一把健康号）。
    let (status, body) = generate(&e.state, TOKEN).await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    settle().await;
}
