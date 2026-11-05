//! Synthetic requests matching the two supported third-party clients.
mod common;
use common::{Admin, Call, SUBJECT, TOKEN, settle, setup};
use serde_json::{Value, json};

fn request(model: &str) -> Value {
    json!({"input":"comic, two panels, forest","model":model,"action":"generate","parameters":{
        "params_version":4,"width":1536,"height":1024,"steps":23,"scale":6.5,
        "sampler":"k_dpmpp_2m","noise_schedule":"exponential","cfg_rescale":0.45,
        "skip_cfg_above_sigma":null,"n_samples":1,"seed":12345,"stream":"msgpack",
        "negative_prompt":"lowres","characterPrompts":[],
        "v4_prompt":{"caption":{"base_caption":"comic, two panels, forest","char_captions":[
            {"char_caption":"woman, 2::red jacket::, standing","centers":[{"x":0.5,"y":0.5}]},
            {"char_caption":"man, 2::dot nose::, expressionless, -2::eyes::","centers":[{"x":0.5,"y":0.5}]}]},"use_coords":false,"use_order":true},
        "v4_negative_prompt":{"caption":{"base_caption":"lowres","char_captions":[
            {"char_caption":"blue jacket","centers":[{"x":0.5,"y":0.5}]},
            {"char_caption":"smile","centers":[{"x":0.5,"y":0.5}]}]},"legacy_uc":false}
    }})
}

#[tokio::test]
async fn owner_identity_is_bound_and_full_access_preserves_limits_across_restart() {
    let e = setup(&[]).await;
    let a = Admin::login_bare(&e.state).await;
    let path = format!("/members/{SUBJECT}/image-access");
    assert_eq!(
        a.json("PUT", &path, Some(json!({"enabled":true}))).await.0,
        403
    );
    e.state.db.bind_image_owner(SUBJECT).await.unwrap();
    assert!(
        e.state
            .db
            .bind_image_owner("900000000000000002")
            .await
            .is_err()
    );
    assert_eq!(
        a.json(
            "PUT",
            &path,
            Some(json!({"enabled":true,"subject":"900000000000000002"}))
        )
        .await
        .0,
        422
    );
    assert_eq!(
        a.json(
            "PUT",
            "/members/900000000000000002/image-access",
            Some(json!({"enabled":true}))
        )
        .await
        .0,
        403
    );
    assert_eq!(
        Call::new("PUT", &format!("/admin/api{path}"))
            .bearer(TOKEN)
            .json(json!({"enabled":true}))
            .send(&e.state)
            .await
            .0,
        401
    );
    assert_eq!(
        a.json("PUT", &path, Some(json!({"enabled":true}))).await.0,
        200
    );
    let full = e.state.db.member_policy(SUBJECT).await.unwrap();
    assert!(full.effective.paid_parameters && full.effective.augment && full.effective.img2img);
    assert_eq!(full.effective.daily_images, 100);
    assert_eq!(full.effective.rpm, 60);
    assert_eq!(
        e.restart().await.db.member_policy(SUBJECT).await.unwrap(),
        full
    );
    assert_eq!(
        a.json("PUT", &path, Some(json!({"enabled":false}))).await.0,
        200
    );
    let mut defaults = e.state.db.member_defaults().await.unwrap();
    defaults.daily_images = 77;
    e.state.db.set_member_defaults(&defaults).await.unwrap();
    assert_eq!(
        e.state.db.member_policy(SUBJECT).await.unwrap().effective,
        defaults
    );
    assert_eq!(
        e.state
            .db
            .member_policy_directory()
            .await
            .unwrap()
            .for_subject(Some(SUBJECT))
            .unwrap()
            .effective,
        defaults
    );
    assert!(
        !e.state
            .db
            .member_policy("900000000000000002")
            .await
            .unwrap()
            .effective
            .paid_parameters
    );
    assert_eq!(
        a.json("PUT", &path, Some(json!({"enabled":true}))).await.0,
        200
    );
    assert_eq!(
        e.state.db.member_policy(SUBJECT).await.unwrap().effective,
        full.effective
    );
}

#[tokio::test]
async fn plugin_captions_parameters_and_response_formats_survive_gateway() {
    for (model, path) in [
        ("nai-diffusion-5-full", "/ai/generate-image"),
        ("nai-diffusion-4-5-full", "/ai/generate-image-stream"),
    ] {
        let e = setup(&[]).await;
        e.state.db.bind_image_owner(SUBJECT).await.unwrap();
        e.state
            .db
            .set_owner_image_enabled(SUBJECT, true)
            .await
            .unwrap();
        e.state
            .db
            .set_setting(
                gate_core::features::GLOBAL_KEY,
                &json!({"image":false,"vibe":false,"tags":false,"upscale":false,"augment":false})
                    .to_string(),
            )
            .await
            .unwrap();
        e.state
            .db
            .set_setting("v5_medium_mode", "on")
            .await
            .unwrap();
        let original = request(model);
        let (status, headers, body) = Call::new("POST", path)
            .bearer(TOKEN)
            .json(original.clone())
            .send(&e.state)
            .await;
        assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
        if path.ends_with("-stream") {
            assert!(
                headers["content-type"]
                    .to_str()
                    .unwrap()
                    .contains("msgpack")
            );
            assert!(!body.is_empty());
        } else {
            assert!(body.starts_with(b"PK"));
        }
        let sent = e.mock.requests_to(path);
        assert_eq!(sent.len(), 1);
        let sent_body: Value = serde_json::from_slice(&sent[0].body).unwrap();
        assert_eq!(sent_body["model"], model);
        for field in [
            "width",
            "height",
            "steps",
            "scale",
            "sampler",
            "cfg_rescale",
            "noise_schedule",
            "v4_prompt",
            "v4_negative_prompt",
        ] {
            assert_eq!(
                sent_body["parameters"][field], original["parameters"][field],
                "{field}"
            );
        }
        let (_, _, sub) = Call::new("GET", "/user/subscription")
            .bearer(TOKEN)
            .send(&e.state)
            .await;
        let sub: Value = serde_json::from_slice(&sub).unwrap();
        assert_eq!(sub["naiGate"]["ownerImageFull"], true);
        assert_eq!(sub["naiGate"]["features"]["img2img"], true);
        assert!(sub.get("usage").is_none());
    }
}

#[tokio::test]
async fn ordinary_default_still_clamps_large_plugin_requests() {
    let e = setup(&[]).await;
    let (status, _, _) = Call::new("POST", "/ai/generate-image")
        .bearer(TOKEN)
        .json(request("nai-diffusion-5-full"))
        .send(&e.state)
        .await;
    assert_eq!(status, 200);
    let sent = e.mock.requests_to("/ai/generate-image");
    let sent_body: Value = serde_json::from_slice(&sent[0].body).unwrap();
    assert_eq!(sent_body["parameters"]["width"], 1216);
    assert_eq!(sent_body["parameters"]["height"], 832);
    assert_eq!(sent_body["parameters"]["steps"], 23);
}

#[tokio::test]
async fn queued_full_access_requests_stop_after_switch_without_dispatch_or_counter_reset() {
    for path in ["/ai/generate-image", "/ai/generate-image-stream"] {
        let e = setup(&[]).await;
        e.state.db.bind_image_owner(SUBJECT).await.unwrap();
        e.state
            .db
            .set_owner_image_enabled(SUBJECT, true)
            .await
            .unwrap();
        e.mock.push(
            "/ai/generate-image",
            gate_testkit::Reply::Chunks {
                status: 200,
                content_type: "application/zip".into(),
                chunks: vec![(
                    std::time::Duration::from_millis(650),
                    bytes::Bytes::from(gate_testkit::zip_of_pngs(1)),
                )],
            },
        );
        let state = e.state.clone();
        let first = tokio::spawn(async move {
            Call::new("POST", "/ai/generate-image")
                .bearer(TOKEN)
                .json({
                    let mut b = request("nai-diffusion-5-full");
                    b["parameters"]["width"] = json!(832);
                    b["parameters"]["height"] = json!(1216);
                    b
                })
                .send(&state)
                .await
        });
        for _ in 0..200 {
            if !e.mock.requests_to("/ai/generate-image").is_empty() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(3)).await;
        }
        assert_eq!(e.mock.requests_to("/ai/generate-image").len(), 1);
        let state = e.state.clone();
        let queued = tokio::spawn(async move {
            Call::new("POST", path)
                .bearer(TOKEN)
                .json(request("nai-diffusion-5-full"))
                .send(&state)
                .await
        });
        let mut waiting = false;
        for _ in 0..100 {
            if e.state.guard.with(|g| g.queue_view(Some(1)))["waiting"]
                .as_i64()
                .unwrap_or(0)
                > 0
            {
                waiting = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(3)).await;
        }
        assert!(waiting);
        e.state
            .db
            .set_owner_image_enabled(SUBJECT, false)
            .await
            .unwrap();
        assert_eq!(first.await.unwrap().0, 200);
        assert_eq!(queued.await.unwrap().0, 403);
        settle().await;
        assert_eq!(e.mock.requests_to("/ai/generate-image").len(), 1);
        assert!(e.mock.requests_to("/ai/generate-image-stream").is_empty());
        assert_eq!(
            e.state.db.get_counter(1, &e.state.day()).await.unwrap().v5,
            1
        );
        assert_eq!(e.state.budget.lock().await.reservations.len(), 0);
    }
}

#[tokio::test]
async fn full_access_keeps_personal_and_shared_budgets_and_v5_pause() {
    let e = setup(&[]).await;
    e.state.db.bind_image_owner(SUBJECT).await.unwrap();
    e.state
        .db
        .set_owner_image_enabled(SUBJECT, true)
        .await
        .unwrap();
    let mut shared = e.state.db.shared_budgets().await.unwrap();
    shared.daily_anlas = 0;
    e.state.db.set_shared_budgets(&shared).await.unwrap();
    assert_eq!(
        Call::new("POST", "/ai/generate-image")
            .bearer(TOKEN)
            .json(request("nai-diffusion-5-full"))
            .send(&e.state)
            .await
            .0,
        403
    );
    assert!(e.mock.requests_to("/ai/generate-image").is_empty());
    shared.daily_anlas = 200;
    e.state.db.set_shared_budgets(&shared).await.unwrap();
    e.state
        .db
        .set_member_overrides(
            SUBJECT,
            &gate_core::member_policy::MemberOverrides {
                daily_anlas: Some(0),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(
        Call::new("POST", "/ai/generate-image")
            .bearer(TOKEN)
            .json(request("nai-diffusion-5-full"))
            .send(&e.state)
            .await
            .0,
        403
    );
    assert!(e.mock.requests_to("/ai/generate-image").is_empty());
    e.state
        .db
        .set_member_overrides(
            SUBJECT,
            &gate_core::member_policy::MemberOverrides {
                daily_anlas: Some(100),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    shared.daily_v5 = 0;
    e.state.db.set_shared_budgets(&shared).await.unwrap();
    assert_eq!(
        Call::new("POST", "/ai/generate-image")
            .bearer(TOKEN)
            .json(request("nai-diffusion-5-full"))
            .send(&e.state)
            .await
            .0,
        403
    );
    assert!(e.mock.requests_to("/ai/generate-image").is_empty());
    shared.daily_v5 = 150;
    e.state.db.set_shared_budgets(&shared).await.unwrap();
    e.state
        .db
        .set_member_overrides(
            SUBJECT,
            &gate_core::member_policy::MemberOverrides {
                daily_anlas: Some(100),
                daily_v5: Some(0),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(
        Call::new("POST", "/ai/generate-image")
            .bearer(TOKEN)
            .json(request("nai-diffusion-5-full"))
            .send(&e.state)
            .await
            .0,
        403
    );
    assert!(e.mock.requests_to("/ai/generate-image").is_empty());
    e.state
        .db
        .set_member_overrides(
            SUBJECT,
            &gate_core::member_policy::MemberOverrides {
                daily_anlas: Some(100),
                daily_v5: Some(10),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    e.state.db.set_setting("v5_paused", "true").await.unwrap();
    assert_eq!(
        Call::new("POST", "/ai/generate-image")
            .bearer(TOKEN)
            .json(request("nai-diffusion-5-full"))
            .send(&e.state)
            .await
            .0,
        403
    );
    assert!(e.mock.requests_to("/ai/generate-image").is_empty());
}

#[tokio::test]
async fn full_access_unlocks_image_tools_but_keeps_output_and_cost_validation() {
    use base64::Engine;
    let e = setup(&[]).await;
    e.state.db.bind_image_owner(SUBJECT).await.unwrap();
    e.state
        .db
        .set_owner_image_enabled(SUBJECT, true)
        .await
        .unwrap();
    let image = base64::engine::general_purpose::STANDARD.encode(gate_testkit::tiny_png());
    for (path, body) in [
        ("/ai/upscale", json!({"image":image,"width":64,"height":64})),
        (
            "/ai/augment-image",
            json!({"image":image,"width":64,"height":64,"req_type":"lineart"}),
        ),
        (
            "/ai/encode-vibe",
            json!({"image":image,"model":"nai-diffusion-4-5-full","information_extracted":1.0}),
        ),
    ] {
        let (status, _, response) = Call::new("POST", path)
            .bearer(TOKEN)
            .json(body)
            .send(&e.state)
            .await;
        assert_eq!(
            status,
            200,
            "{path}: {}",
            String::from_utf8_lossy(&response)
        );
        assert!(!response.is_empty());
    }
    settle().await;
    assert!(
        e.state
            .db
            .get_counter(1, &e.state.day())
            .await
            .unwrap()
            .anlas
            > 0.0
    );
    e.state
        .db
        .set_owner_image_enabled(SUBJECT, false)
        .await
        .unwrap();
    assert_eq!(
        Call::new("POST", "/ai/augment-image")
            .bearer(TOKEN)
            .json(json!({"image":image,"req_type":"lineart"}))
            .send(&e.state)
            .await
            .0,
        403
    );
}

#[tokio::test]
async fn exhausted_owner_v5_uses_personal_budget_and_records_unknown_delivery() {
    let e = setup(&[]).await;
    e.state.db.bind_image_owner(SUBJECT).await.unwrap();
    e.state
        .db
        .set_owner_image_enabled(SUBJECT, true)
        .await
        .unwrap();
    e.mock.set_v5_percent(0);
    let mut body = request("nai-diffusion-5-full");
    body["parameters"]["width"] = json!(832);
    body["parameters"]["height"] = json!(1216);
    let (status, _, reply) = Call::new("POST", "/ai/generate-image")
        .bearer(TOKEN)
        .json(body)
        .send(&e.state)
        .await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&reply));
    settle().await;
    let counter = e.state.db.get_counter(1, &e.state.day()).await.unwrap();
    assert!(counter.anlas > 0.0);
    assert_eq!(counter.v5, 1);
    e.mock.push(
        "/ai/generate-image",
        gate_testkit::Reply::json(500, &json!({"message":"synthetic failure"})),
    );
    e.mock.push(
        "/ai/generate-image",
        gate_testkit::Reply::json(500, &json!({"message":"synthetic failure"})),
    );
    let (status, _, reply) = Call::new("POST", "/ai/generate-image")
        .bearer(TOKEN)
        .json({
            let mut b = request("nai-diffusion-5-full");
            b["parameters"]["width"] = json!(512);
            b["parameters"]["height"] = json!(768);
            b
        })
        .send(&e.state)
        .await;
    assert!(status.is_server_error());
    let reply: Value = serde_json::from_slice(&reply).unwrap();
    assert_eq!(reply["error"]["execution_unknown"], true);
    assert_eq!(reply["error"]["retryable"], false);
    settle().await;
    assert!(!e.state.budget.lock().await.reservations.is_empty());
}

#[tokio::test]
async fn owner_img2img_infill_and_precise_references_reach_the_upstream() {
    use base64::Engine;
    for operation in ["img2img", "infill", "reference"] {
        let e = setup(&[]).await;
        e.state.db.bind_image_owner(SUBJECT).await.unwrap();
        e.state
            .db
            .set_owner_image_enabled(SUBJECT, true)
            .await
            .unwrap();
        let image = base64::engine::general_purpose::STANDARD.encode(gate_testkit::tiny_png());
        let mut body = request(if operation == "infill" {
            "nai-diffusion-4-5-full-inpainting"
        } else {
            "nai-diffusion-4-5-full"
        });
        body["parameters"]["width"] = json!(832);
        body["parameters"]["height"] = json!(1216);
        if operation == "reference" {
            body["parameters"]["director_reference_images"] = json!([image]);
            body["parameters"]["director_reference_descriptions"] = json!([{"caption":{"base_caption":"character","char_captions":[]},"legacy_uc":false}]);
            body["parameters"]["director_reference_information_extracted"] = json!([1]);
            body["parameters"]["director_reference_strength_values"] = json!([1]);
            body["parameters"]["director_reference_secondary_strength_values"] = json!([0]);
        } else {
            body["action"] = json!(operation);
            body["parameters"]["image"] = json!(image);
            body["parameters"]["strength"] = json!(0.5);
            body["parameters"]["noise"] = json!(0);
            if operation == "infill" {
                body["parameters"]["mask"] = json!(image);
                body["parameters"]["img2img"] = json!({"strength":0.5});
            }
        }
        let (status, _, reply) = Call::new("POST", "/ai/generate-image")
            .bearer(TOKEN)
            .json(body)
            .send(&e.state)
            .await;
        assert_eq!(
            status,
            200,
            "{operation}: {}",
            String::from_utf8_lossy(&reply)
        );
        assert_eq!(e.mock.requests_to("/ai/generate-image").len(), 1);
    }
}
