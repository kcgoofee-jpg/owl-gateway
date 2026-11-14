//! 拼车网关：凭据入库、流式最终帧转 SSE、402 换到官方号、导演/标签不打拼车 Host。

mod common;
use common::*;
use gate_testkit::{MockUpstream, Reply, msgpack_frame, tiny_png};
use serde_json::{Value, json};

const SHARE_KEY: &str = "NAI-synthetic-share-key01";

fn v5_body() -> Value {
    json!({
        "input": "1girl, solo", "model": "nai-diffusion-5-full", "action": "generate",
        "parameters": {"width": 832, "height": 1216, "steps": 28, "scale": 5, "sampler": "k_euler_ancestral",
                       "n_samples": 1, "seed": 1234, "negative_prompt": "lowres"},
    })
}

async fn add_share(state: &gate_server::AppState, host: &str) {
    state
        .nai
        .add_token(SHARE_KEY.into(), true, None, Some(true), Some(host.into()))
        .await
        .unwrap();
}

#[tokio::test]
async fn share_crud_persists_and_stays_off_the_official_page() {
    let share = MockUpstream::start().await;
    let e = setup(&[]).await;
    let api = Admin::login_bare(&e.state).await;
    let host = share.url();

    api.fails(
        "POST",
        "/share-upstreams",
        Some(json!({"token": SHARE_KEY})),
        422,
        "拼车网关地址不对：需要 https:// 开头的主机名，不要带路径",
    )
    .await;
    api.fails(
        "POST",
        "/share-upstreams",
        Some(json!({"token": "pst-synthetic-not-a-share", "image_host": host})),
        422,
        "Key 格式不对：拼车网关的 Key 应以 NAI- 或 pk- 开头",
    )
    .await;
    api.fails(
        "POST",
        "/share-upstreams",
        Some(json!({"token": "pk-short", "image_host": host})),
        422,
        "Key 格式不对：拼车网关的 Key 应以 NAI- 或 pk- 开头",
    )
    .await;
    api.fails(
        "POST",
        "/share-upstreams",
        Some(json!({"token": SHARE_KEY, "image_host": format!("{host}/path")})),
        422,
        "拼车网关地址不对：需要 https:// 开头的主机名，不要带路径",
    )
    .await;

    share.push(
        "/user/subscription",
        Reply::json(200, &json!({"tier": 3, "active": false})),
    );
    api.fails(
        "POST",
        "/share-upstreams",
        Some(json!({"token": SHARE_KEY, "image_host": host, "allow_anlas": true})),
        422,
        "拼车网关说这份份额未生效",
    )
    .await;

    let (status, body) = api
        .json(
            "POST",
            "/share-upstreams",
            Some(json!({"token": SHARE_KEY, "image_host": host, "allow_anlas": true})),
        )
        .await;
    assert_eq!(status, 200, "{body}");
    assert!(e.mock.requests_to("/user/subscription").is_empty());
    assert!(!share.requests_to("/user/subscription").is_empty());

    let official = api.get_ok("/upstream").await;
    assert_eq!(official["pool_size"], json!(2));
    assert_eq!(official["accounts"].as_array().unwrap().len(), 1);
    assert!(
        official["accounts"][0]["token"]
            .as_str()
            .unwrap()
            .starts_with("pst-")
    );

    let listed = api.get_ok("/share-upstreams").await;
    assert_eq!(listed["accounts"].as_array().unwrap().len(), 1);
    let row = &listed["accounts"][0];
    assert_eq!(row["image_host"], json!(host.trim_end_matches('/')));
    assert_eq!(row["allow_anlas"], json!(true));
    assert!(!row["token"].as_str().unwrap().contains(SHARE_KEY));
    assert_eq!(row["usage_percent"], json!(80));
    assert_eq!(row["v5_used"], json!(0));
    assert_eq!(row["images_today"], json!(0));
    assert!(row["v5_daily_limit"].as_i64().unwrap() >= 0);
    assert_eq!(row["w24h"]["ok"], json!(0));
    assert_eq!(listed["fields"]["v5_daily_limit"]["min"], json!(0));

    let id = row["token_id"].as_str().unwrap().to_string();
    let (status, body) = api
        .json(
            "PUT",
            &format!("/upstream-tokens/{id}/v5-limit"),
            Some(json!({"v5_daily_limit": 8})),
        )
        .await;
    assert_eq!(status, 200, "{body}");
    let listed = api.get_ok("/share-upstreams").await;
    assert_eq!(listed["accounts"][0]["v5_daily_limit"], json!(8));

    let restarted = e.restart().await;
    assert!(
        restarted
            .nai
            .tokens()
            .await
            .iter()
            .any(|t| t.token_id == id && t.is_share())
    );

    let api = Admin::login_bare(&restarted).await;
    assert_eq!(
        api.text("DELETE", &format!("/share-upstreams/{id}"), None)
            .await
            .0,
        200
    );
    assert_eq!(
        api.get_ok("/share-upstreams").await["accounts"]
            .as_array()
            .unwrap()
            .len(),
        0
    );
}

#[tokio::test]
async fn donated_pk_key_is_accepted_as_share_credential() {
    let share = MockUpstream::start().await;
    let e = setup(&[]).await;
    let api = Admin::login_bare(&e.state).await;
    let key = "pk-synthetic-donated-share-key01";
    let (status, body) = api
        .json(
            "POST",
            "/share-upstreams",
            Some(json!({"token": key, "image_host": share.url(), "allow_anlas": false})),
        )
        .await;
    assert_eq!(status, 200, "{body}");
    let listed = api.get_ok("/share-upstreams").await;
    assert_eq!(listed["accounts"].as_array().unwrap().len(), 1);
    let row = &listed["accounts"][0];
    assert_eq!(row["image_host"], json!(share.url().trim_end_matches('/')));
    assert_eq!(row["allow_anlas"], json!(false));
    // 未绑定赞助席的拼车 Key 不是捐赠来源。
    assert_eq!(row["donated"], json!(false));
    let id = row["token_id"].as_str().unwrap().to_string();
    assert_eq!(id, gate_upstream::headers::token_id(key));
    let restarted = e.restart().await;
    assert!(
        restarted
            .nai
            .tokens()
            .await
            .iter()
            .any(|t| t.token_id == id && t.is_share())
    );
}

#[tokio::test]
async fn share_octet_stream_final_becomes_member_sse() {
    let share = MockUpstream::start().await;
    let e = setup(&[]).await;
    let official_id = e.state.nai.tokens().await[0].token_id.clone();
    add_share(&e.state, &share.url()).await;
    assert!(e.state.nai.set_admin_enabled(&official_id, false).await);

    let png = tiny_png();
    let frame = msgpack_frame(vec![
        ("event_type", "final".into()),
        ("samp_ix", 0.into()),
        ("image", rmpv::Value::Binary(png)),
    ]);
    share.push(
        "/ai/generate-image-stream",
        Reply::Status {
            status: 200,
            headers: vec![("content-type".into(), "application/octet-stream".into())],
            body: frame.into(),
        },
    );

    let (status, _, body) = Call::new("POST", "/ai/generate-image-stream")
        .bearer(TOKEN)
        .json(image_body())
        .send(&e.state)
        .await;
    let text = String::from_utf8_lossy(&body);
    assert_eq!(status, 200, "{text}");
    assert!(text.contains("event: final"), "{text}");
    assert!(e.mock.requests_to("/ai/generate-image-stream").is_empty());
    assert_eq!(share.requests_to("/ai/generate-image-stream").len(), 1);
}

#[tokio::test]
async fn share_402_on_free_v5_switches_to_official() {
    let share = MockUpstream::start().await;
    let e = setup(&[]).await;
    e.state
        .guard
        .with(|g| g.values.insert("interval_jitter".into(), 0));
    policy_for(&e.state, 1, json!({"daily_v5": 10, "v5": true})).await;
    add_share(&e.state, &share.url()).await;
    share.push(
        "/ai/generate-image-stream",
        Reply::json(
            402,
            &json!({"statusCode": 402, "message": "Insufficient Anlas"}),
        ),
    );

    let (status, _, body) = Call::new("POST", "/ai/generate-image-stream")
        .bearer(TOKEN)
        .json(v5_body())
        .send(&e.state)
        .await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    assert_eq!(share.requests_to("/ai/generate-image-stream").len(), 1);
    assert_eq!(e.mock.requests_to("/ai/generate-image-stream").len(), 1);
    settle().await;
    let tokens = e.state.nai.tokens().await;
    let official = tokens.iter().find(|t| !t.is_share()).unwrap();
    let shared = tokens.iter().find(|t| t.is_share()).unwrap();
    assert_eq!(
        e.state
            .db
            .upstream_v5_used_and_held(&official.token_id, &e.state.day())
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        e.state
            .db
            .upstream_v5_used_and_held(&shared.token_id, &e.state.day())
            .await
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn share_generate_image_drops_website_extras() {
    let share = MockUpstream::start().await;
    let e = setup(&[]).await;
    let official_id = e.state.nai.tokens().await[0].token_id.clone();
    add_share(&e.state, &share.url()).await;
    assert!(e.state.nai.set_admin_enabled(&official_id, false).await);

    let mut body = image_body();
    body["website_extra"] = json!(true);
    body["parameters"]["characterReferences"] = json!([]);
    body["parameters"]["autoSmea"] = json!(true);
    body["parameters"]["sm"] = json!(true);
    body["parameters"]["qualityToggle"] = json!(1);

    let (status, _, resp) = Call::new("POST", "/ai/generate-image")
        .bearer(TOKEN)
        .json(body)
        .send(&e.state)
        .await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&resp));
    let sent = share.requests_to("/ai/generate-image");
    assert_eq!(sent.len(), 1);
    let got = sent[0].json();
    assert!(got.get("website_extra").is_none(), "{got}");
    let p = got["parameters"].as_object().unwrap();
    assert!(p.get("characterReferences").is_none(), "{p:?}");
    assert_eq!(p.get("autoSmea"), Some(&json!(false)));
    assert_eq!(p.get("sm"), Some(&json!(false)));
    assert_eq!(p.get("qualityToggle"), Some(&json!(true)));
}

#[tokio::test]
async fn share_generate_image_fills_character_coords_and_string_uc() {
    let share = MockUpstream::start().await;
    let e = setup(&[]).await;
    let official_id = e.state.nai.tokens().await[0].token_id.clone();
    add_share(&e.state, &share.url()).await;
    assert!(e.state.nai.set_admin_enabled(&official_id, false).await);

    let mut body = image_body();
    body["parameters"]["negative_prompt"] = json!(12);
    body["parameters"]["v4_prompt"] = json!({
        "caption": {
            "base_caption": "1girl, solo",
            "char_captions": [{"char_caption": "owl girl", "centers": [{}]}]
        },
        "use_coords": false,
        "use_order": true
    });
    body["parameters"]["v4_negative_prompt"] = json!({
        "caption": {
            "base_caption": "12",
            "char_captions": [{"char_caption": ""}]
        },
        "legacy_uc": false
    });

    let (status, _, resp) = Call::new("POST", "/ai/generate-image")
        .bearer(TOKEN)
        .json(body)
        .send(&e.state)
        .await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&resp));
    let sent = share.requests_to("/ai/generate-image");
    assert_eq!(sent.len(), 1);
    let got = sent[0].json();
    assert_eq!(got["parameters"]["negative_prompt"], json!("12"));
    assert_eq!(
        got["parameters"]["v4_prompt"]["caption"]["char_captions"][0]["centers"],
        json!([{"x": 0.5, "y": 0.5}])
    );
}

#[tokio::test]
async fn share_422_unknown_fields_are_stripped_and_retried() {
    let share = MockUpstream::start().await;
    let e = setup(&[]).await;
    let official_id = e.state.nai.tokens().await[0].token_id.clone();
    add_share(&e.state, &share.url()).await;
    assert!(e.state.nai.set_admin_enabled(&official_id, false).await);
    share.push(
        "/ai/generate-image",
        Reply::json(
            422,
            &json!({
                "statusCode": 422,
                "message": "包含尚未支持的生成参数；请关闭 ControlNet、自动放大或其他未支持的扩展。 未知字段：foo。"
            }),
        ),
    );

    let mut body = image_body();
    body["parameters"]["foo"] = json!(1);
    let (status, _, resp) = Call::new("POST", "/ai/generate-image")
        .bearer(TOKEN)
        .json(body)
        .send(&e.state)
        .await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&resp));
    let sent = share.requests_to("/ai/generate-image");
    assert_eq!(sent.len(), 2);
    assert_eq!(sent[0].json()["parameters"]["foo"], json!(1));
    assert!(sent[1].json()["parameters"].get("foo").is_none());
}

#[tokio::test]
async fn director_and_tags_do_not_hit_share_host() {
    use base64::Engine;
    let share = MockUpstream::start().await;
    let e = setup(&[]).await;
    add_share(&e.state, &share.url()).await;
    policy_for(
        &e.state,
        1,
        json!({"daily_anlas": 20, "paid_parameters": true, "augment": true, "tags": true}),
    )
    .await;
    let image = base64::engine::general_purpose::STANDARD.encode(tiny_png());

    let (status, _, body) = Call::new("POST", "/ai/augment-image")
        .bearer(TOKEN)
        .json(json!({"image": image, "width": 64, "height": 64, "req_type": "lineart"}))
        .send(&e.state)
        .await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    assert!(share.requests_to("/ai/augment-image").is_empty());
    assert_eq!(e.mock.requests_to("/ai/augment-image").len(), 1);

    let (status, _, body) = Call::new(
        "GET",
        "/ai/generate-image/suggest-tags?prompt=1g&model=nai-diffusion-4-5-full",
    )
    .bearer(TOKEN)
    .send(&e.state)
    .await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    assert!(
        share
            .requests_to("/ai/generate-image/suggest-tags")
            .is_empty()
    );
    assert_eq!(
        e.mock.requests_to("/ai/generate-image/suggest-tags").len(),
        1
    );
}

#[tokio::test]
async fn share_settings_are_independent_validated_and_survive_restart_and_replacement() {
    let share = MockUpstream::start().await;
    let e = setup(&[]).await;
    let api = Admin::login_bare(&e.state).await;
    for key in [SHARE_KEY, "NAI-synthetic-second-share-key"] {
        assert_eq!(
            api.json(
                "POST",
                "/share-upstreams",
                Some(json!({"token":key,"image_host":share.url()}))
            )
            .await
            .0,
            200
        );
    }
    let id = gate_upstream::headers::token_id(SHARE_KEY);
    let settings = json!({"allow_anlas":true,"daily_anlas":25,"rpm":4,"concurrency":2,"post_wait_min":0,"post_wait_max":1});
    let url = format!("/share-upstreams/{id}/settings");
    for bad in [
        json!({"allow_anlas":true}),
        json!({"rpm":-1}),
        json!({"concurrency":0}),
        json!({"rpm":2.5}),
        json!({"post_wait_min":5}),
    ] {
        let mut merged = settings.clone();
        if bad.as_object().unwrap().len() == 1 && bad.get("allow_anlas").is_none() {
            for (k, v) in bad.as_object().unwrap() {
                merged[k] = v.clone();
            }
        } else {
            merged = bad;
        }
        assert_eq!(api.json("PUT", &url, Some(merged)).await.0, 422);
    }
    assert_eq!(api.json("PUT", &url, Some(settings.clone())).await.0, 200);
    let page = api.get_ok("/share-upstreams").await;
    assert_eq!(page["accounts"][0]["daily_anlas"], 25);
    // 没单独设置过的拼车号：默认不限（9999），不是 0
    assert_eq!(page["accounts"][1]["daily_anlas"], 9999);
    assert_eq!(page["accounts"][1]["allow_anlas"], false);
    assert_eq!(e.state.nai.tokens().await[0].image_slots.limit(), 1);
    let new_key = "NAI-synthetic-replaced-share-key";
    assert_eq!(
        api.json(
            "PUT",
            &format!("/share-upstreams/{id}"),
            Some(json!({"token":new_key}))
        )
        .await
        .0,
        200
    );
    let restarted = e.restart().await;
    let api = Admin::login_bare(&restarted).await;
    let page = api.get_ok("/share-upstreams").await;
    assert_eq!(page["accounts"][0]["v5_daily_limit"], 9999);
    assert_eq!(page["accounts"][1]["v5_daily_limit"], 9999);
    for (name, value) in settings.as_object().unwrap() {
        assert_eq!(&page["accounts"][0][name], value, "{name}");
    }
    let t = restarted
        .nai
        .tokens()
        .await
        .into_iter()
        .find(|t| t.token == new_key)
        .unwrap();
    assert_eq!(t.image_slots.limit(), 2);
}

async fn call(
    state: &gate_server::AppState,
    method: &str,
    path: &str,
    auth: Option<&str>,
    body: Option<Value>,
) -> (axum::http::StatusCode, axum::http::HeaderMap, Vec<u8>) {
    let mut req = Call::new(method, path);
    if let Some(token) = auth {
        req = req.bearer(token);
    }
    if let Some(body) = body {
        req = req.json(body);
    }
    req.send(state).await
}

#[tokio::test]
async fn share_paid_dispatch_enforces_cap_before_sending_and_releases_known_failures() {
    use base64::Engine;
    let share = MockUpstream::start().await;
    let e = setup(&[]).await;
    add_share(&e.state, &share.url()).await;
    policy_for(&e.state, 1, json!({"vibe":true,"daily_anlas":50})).await;
    budgets_for(&e.state, json!({"daily_anlas":50})).await;
    let api = Admin::login_bare(&e.state).await;
    let official = e.state.nai.tokens().await[0].token_id.clone();
    assert_eq!(
        api.json(
            "PUT",
            &format!("/upstream-tokens/{official}/enabled"),
            Some(json!({"enabled":false}))
        )
        .await
        .0,
        200
    );
    let id = gate_upstream::headers::token_id(SHARE_KEY);
    assert_eq!(api.json("PUT",&format!("/share-upstreams/{id}/settings"),Some(json!({"allow_anlas":true,"daily_anlas":3,"rpm":9999,"concurrency":2,"post_wait_min":0,"post_wait_max":0}))).await.0,200);
    let image = image::GrayImage::from_fn(64, 64, |x, y| image::Luma([((x * 4) ^ y) as u8]));
    let mut png = std::io::Cursor::new(Vec::new());
    image.write_to(&mut png, image::ImageFormat::Png).unwrap();
    let payload = json!({"image":base64::engine::general_purpose::STANDARD.encode(png.into_inner()),"model":"nai-diffusion-4-5-full","informationExtracted":0.5});
    share.push(
        "/ai/encode-vibe",
        Reply::json(400, &json!({"error":"known failure"})),
    );
    assert_eq!(
        call(
            &e.state,
            "POST",
            "/ai/encode-vibe",
            Some(TOKEN),
            Some(payload.clone())
        )
        .await
        .0,
        400
    );
    assert_eq!(
        e.state
            .db
            .upstream_anlas_used(&id, &e.state.day())
            .await
            .unwrap(),
        0.0
    );
    share.push(
        "/ai/encode-vibe",
        Reply::Status {
            status: 200,
            headers: vec![("content-type".into(), "application/octet-stream".into())],
            body: bytes::Bytes::from_static(b"encoded-vibe"),
        },
    );
    let (status, _, body) = call(
        &e.state,
        "POST",
        "/ai/encode-vibe",
        Some(TOKEN),
        Some(payload.clone()),
    )
    .await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    assert_eq!(
        e.state
            .db
            .upstream_anlas_used(&id, &e.state.day())
            .await
            .unwrap(),
        2.0
    );
    let (status, _, body) = call(
        &e.state,
        "POST",
        "/ai/encode-vibe",
        Some(TOKEN),
        Some(payload),
    )
    .await;
    assert_eq!(status, 429, "{}", String::from_utf8_lossy(&body));
    assert_eq!(share.requests_to("/ai/encode-vibe").len(), 2);
}
