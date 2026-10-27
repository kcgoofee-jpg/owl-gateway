//! 出图请求通路的端到端测试：真实的 AppState + 路由 + 记账，上游换成本地假服务。

mod common;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use common::{Call, TOKEN, settle, setup};
use futures::StreamExt;
use gate_db::repo::usage::{LogFilter, Success};
use gate_server::AppState;
use gate_testkit::Reply;
use rusqlite::OptionalExtension;
use serde_json::{Value, json};
use tower::ServiceExt;

fn image_body(model: &str, n: i64) -> Value {
    json!({
        "input": "1girl, solo", "model": model, "action": "generate",
        "parameters": {"width": 832, "height": 1216, "steps": 28, "scale": 5, "sampler": "k_euler_ancestral",
                       "n_samples": n, "seed": 1234, "negative_prompt": "lowres"},
    })
}

async fn call(
    state: &AppState,
    method: &str,
    path: &str,
    auth: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, axum::http::HeaderMap, Vec<u8>) {
    let mut c = Call::new(method, path);
    if let Some(a) = auth {
        c = c.bearer(a);
    }
    if let Some(v) = body {
        c = c.json(v);
    }
    c.send(state).await
}

/// 错误响应里给成员看的那句话（OpenAI 风格的 `error.message` 或顶层 `message`）。
fn err_message(body: &[u8]) -> String {
    let v: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
    v["error"]["message"]
        .as_str()
        .or(v["message"].as_str())
        .unwrap_or_default()
        .to_string()
}

/// 假上游收到的请求里的 `authorization` 头（用来看是哪个上游账号在出图）。
fn auth_of(r: &gate_testkit::Recorded) -> String {
    r.headers
        .iter()
        .find(|(k, _)| k == "authorization")
        .map(|(_, v)| v.clone())
        .unwrap_or_default()
}

fn correlation_of(r: &gate_testkit::Recorded) -> String {
    r.headers
        .iter()
        .find(|(k, _)| k == "x-correlation-id")
        .map(|(_, v)| v.clone())
        .unwrap_or_default()
}

fn valid_correlation_id(id: &str) -> bool {
    id.len() == 6 && id.bytes().all(|b| b.is_ascii_alphanumeric())
}

/// 成员自己的 Key（id=1）累计花掉的 Anlas。
async fn member_anlas(state: &AppState) -> f64 {
    state
        .db
        .read(|c| {
            c.query_row(
                "SELECT COALESCE(SUM(anlas),0) FROM counters WHERE key_id=1",
                [],
                |r| r.get(0),
            )
        })
        .await
        .unwrap()
}

async fn logs(state: &AppState) -> Vec<Value> {
    // 记账在后台任务里：等它们写完
    settle().await;
    state
        .db
        .list_logs(50, 0, &LogFilter::default())
        .await
        .unwrap()
}

#[tokio::test]
async fn generate_image_ok_records_usage() {
    let e = setup(&[]).await;
    let (status, headers, body) = call(
        &e.state,
        "POST",
        "/ai/generate-image",
        Some(TOKEN),
        Some(image_body("nai-diffusion-4-5-full", 1)),
    )
    .await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    assert!(body.starts_with(b"PK"));
    assert!(headers.get("x-request-id").is_some());
    let sent = e.mock.requests_to("/ai/generate-image");
    assert_eq!(sent.len(), 1);
    let correlation_id = correlation_of(&sent[0]);
    assert!(valid_correlation_id(&correlation_id), "{correlation_id}");
    let (request_id, bound_rid, bound_correlation): (String, String, String) = e
        .state
        .db
        .read({
            let correlation_id = correlation_id.clone();
            move |c| {
                c.query_row(
            "SELECT request_id,rid,correlation_id FROM request_settlements WHERE correlation_id=?",
            [correlation_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
            }
        })
        .await
        .unwrap();
    assert_eq!(request_id.len(), 32);
    assert_eq!(bound_rid, headers["x-request-id"]);
    assert_eq!(bound_correlation, correlation_id);
    let owned = e.state.db.member_logs(1, 0.0, 0, "", 10).await.unwrap();
    assert_eq!(owned[0]["correlation_id"], correlation_id);
    assert!(
        e.state
            .db
            .member_logs(2, 0.0, 0, "", 10)
            .await
            .unwrap()
            .is_empty()
    );
    let rows = logs(&e.state).await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["status"], "ok");
    assert_eq!(rows[0]["kind"], "image");
    assert_eq!(rows[0]["images"], 1);
    let detail = rows[0]["detail"].as_str().unwrap();
    assert!(detail.starts_with("832x1216/28step 免费"), "{detail}");
    let c = e.state.db.get_counter(1, &e.state.day()).await.unwrap();
    assert_eq!((c.images, c.legacy_free_images), (1, 1));
}

#[tokio::test]
async fn image_tool_sends_and_persists_support_correlation_id() {
    use base64::Engine;
    let e = setup(&[]).await;
    e.state
        .db
        .run_script(vec![
            "UPDATE api_keys SET allow_img2img=1, allow_anlas=1, daily_anlas=10, features='image,upscale,augment,vibe' WHERE id=1"
                .into(),
        ])
        .await
        .unwrap();
    let image = image::GrayImage::from_fn(64, 64, |x, y| image::Luma([((x * 4) ^ y) as u8]));
    let mut png = std::io::Cursor::new(Vec::new());
    image.write_to(&mut png, image::ImageFormat::Png).unwrap();
    let source = base64::engine::general_purpose::STANDARD.encode(png.into_inner());
    let (status, _, body) = call(
        &e.state,
        "POST",
        "/ai/upscale",
        Some(TOKEN),
        Some(json!({"image": source})),
    )
    .await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    let sent = e.mock.requests_to("/ai/upscale");
    assert_eq!(sent.len(), 1);
    let correlation_id = correlation_of(&sent[0]);
    assert!(valid_correlation_id(&correlation_id), "{correlation_id}");
    let (request_id, bound_correlation): (String, String) = e
        .state
        .db
        .read({
            let correlation_id = correlation_id.clone();
            move |c| {
                c.query_row(
            "SELECT request_id,correlation_id FROM request_settlements WHERE correlation_id=?",
            [correlation_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
            }
        })
        .await
        .unwrap();
    assert_eq!(request_id.len(), 32);
    assert_eq!(bound_correlation, correlation_id);
}

#[tokio::test]
async fn nonstream_body_drop_keeps_success_charge_and_marks_interrupted() {
    let e = setup(&[]).await;
    let req = Request::builder()
        .method("POST")
        .uri("/ai/generate-image")
        .header("authorization", format!("Bearer {TOKEN}"))
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::to_vec(&image_body("nai-diffusion-4-5-full", 1)).unwrap(),
        ))
        .unwrap();
    let response = gate_server::app(e.state.clone())
        .oneshot(req)
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let rid = response.headers()["x-request-id"]
        .to_str()
        .unwrap()
        .to_string();
    drop(response.into_body()); // Body is never polled by the caller.
    let rows = logs(&e.state).await;
    assert_eq!(e.mock.requests_to("/ai/generate-image").len(), 1);
    assert_eq!(rows.iter().filter(|r| r["status"] == "ok").count(), 1);
    let delivery: String = e.state.db.read(move |c| c.query_row(
        "SELECT s.delivery_state FROM usage_log l JOIN usage_request_links u ON u.usage_log_id=l.id JOIN request_settlements s ON s.request_id=u.request_id WHERE l.rid=?",
        [rid], |r| r.get(0),
    )).await.unwrap();
    assert_eq!(delivery, "transport_interrupted");
    assert_eq!(
        e.state
            .db
            .get_counter(1, &e.state.day())
            .await
            .unwrap()
            .images,
        1
    );
}

#[tokio::test]
async fn disconnect_before_generation_finishes_still_commits_once_and_marks_interrupted() {
    let e = setup(&[]).await;
    e.mock.push(
        "/ai/generate-image",
        Reply::Chunks {
            status: 200,
            content_type: "application/zip".into(),
            chunks: vec![(
                std::time::Duration::from_millis(180),
                bytes::Bytes::from(gate_testkit::zip_of_pngs(1)),
            )],
        },
    );
    let req = Request::builder()
        .method("POST")
        .uri("/ai/generate-image")
        .header("authorization", format!("Bearer {TOKEN}"))
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::to_vec(&image_body("nai-diffusion-4-5-full", 1)).unwrap(),
        ))
        .unwrap();
    let task = tokio::spawn(gate_server::app(e.state.clone()).oneshot(req));
    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        while e.mock.requests_to("/ai/generate-image").is_empty() {
            tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        }
    })
    .await
    .unwrap();
    task.abort();
    let _ = task.await;
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let rows = logs(&e.state).await;
    assert_eq!(e.mock.requests_to("/ai/generate-image").len(), 1);
    assert_eq!(
        rows.iter().filter(|r| r["status"] == "ok").count(),
        1,
        "{rows:?}"
    );
    assert_eq!(
        e.state
            .db
            .get_counter(1, &e.state.day())
            .await
            .unwrap()
            .images,
        1
    );
    let rid = rows.iter().find(|r| r["status"] == "ok").unwrap()["rid"]
        .as_str()
        .unwrap()
        .to_string();
    let delivery: String = e.state.db.read(move |c| c.query_row(
        "SELECT s.delivery_state FROM usage_log l JOIN usage_request_links u ON u.usage_log_id=l.id JOIN request_settlements s ON s.request_id=u.request_id WHERE l.rid=?",
        [rid], |r| r.get(0),
    )).await.unwrap();
    assert_eq!(delivery, "transport_interrupted");
}

#[tokio::test]
async fn stream_disconnect_after_final_keeps_single_image_charge_and_marks_interrupted() {
    let e = setup(&[]).await;
    let req = Request::builder()
        .method("POST")
        .uri("/ai/generate-image-stream")
        .header("authorization", format!("Bearer {TOKEN}"))
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::to_vec(&image_body("nai-diffusion-4-5-full", 1)).unwrap(),
        ))
        .unwrap();
    let response = gate_server::app(e.state.clone())
        .oneshot(req)
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let rid = response.headers()["x-request-id"]
        .to_str()
        .unwrap()
        .to_string();
    let mut body = response.into_body().into_data_stream();
    let mut saw_final = false;
    while let Some(frame) = body.next().await {
        let frame = frame.unwrap();
        if String::from_utf8_lossy(&frame).contains("final") {
            saw_final = true;
            break;
        }
    }
    assert!(
        saw_final,
        "the client observed the final image event before disconnecting"
    );
    drop(body);
    let rows = logs(&e.state).await;
    let sent = e.mock.requests_to("/ai/generate-image-stream");
    assert_eq!(sent.len(), 1);
    let correlation_id = correlation_of(&sent[0]);
    assert!(valid_correlation_id(&correlation_id), "{correlation_id}");
    assert_eq!(rows.iter().filter(|r| r["status"] == "ok").count(), 1);
    assert_eq!(rows.iter().find(|r| r["rid"] == rid).unwrap()["images"], 1);
    let (delivery, bound_correlation): (String, String) = e.state.db.read(move |c| c.query_row(
        "SELECT s.delivery_state,s.correlation_id FROM usage_log l JOIN usage_request_links u ON u.usage_log_id=l.id JOIN request_settlements s ON s.request_id=u.request_id WHERE l.rid=?",
        [rid], |r| Ok((r.get(0)?, r.get(1)?)),
    )).await.unwrap();
    assert_eq!(delivery, "transport_interrupted");
    assert_eq!(bound_correlation, correlation_id);
    assert_eq!(
        e.state
            .db
            .get_counter(1, &e.state.day())
            .await
            .unwrap()
            .images,
        1
    );
}

/// 每人只有每日上限：2.x 的每人注水（底仓 30 张、每小时补 4 张，一天最多约 126 张）已删除。
/// 上限 200、今天已用 150 张的算法管理成员照样能出图，不会被 `paced` 拦下；
/// 用满上限才是「今日 V4.5 免费图额度已用完」。
#[tokio::test]
async fn no_per_member_pacing_below_the_daily_cap() {
    let e = setup(&[]).await;
    e.state.db.set_setting("quota_modes_v1", r#"{"register_daily_images":{"mode":"limit","value":200},"register_daily_v5":{"mode":"unlimited"},"global_daily_v5":{"mode":"unlimited"},"global_daily_anlas":{"mode":"unlimited"},"member_daily_anlas":{"mode":"unlimited"},"global_monthly_anlas":{"mode":"unlimited"},"public_vibe_enabled":false}"#).await.unwrap();
    let used = |n: i64| Success {
        key_id: 1,
        kind: "image".into(),
        model: "nai-diffusion-4-5-full".into(),
        day: e.state.day(),
        images: n,
        legacy_free_images: n,
        ..Default::default()
    };
    e.state
        .db
        .record_success(&used(150), e.state.now())
        .await
        .unwrap();
    let (status, _, body) = call(
        &e.state,
        "POST",
        "/ai/generate-image",
        Some(TOKEN),
        Some(image_body("nai-diffusion-4-5-full", 1)),
    )
    .await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    // 用满 200 张：个人额度用完（403），不是「出图速度过快」
    e.state
        .db
        .record_success(&used(49), e.state.now())
        .await
        .unwrap();
    let (status, _, body) = call(
        &e.state,
        "POST",
        "/ai/generate-image",
        Some(TOKEN),
        Some(image_body("nai-diffusion-4-5-full", 1)),
    )
    .await;
    assert_eq!(status, 403, "{}", String::from_utf8_lossy(&body));
    let text = String::from_utf8_lossy(&body);
    assert!(text.contains("今日 V4.5 额度已用完，每天 200 张"), "{text}");
    assert!(!text.contains("每小时自动恢复"), "{text}");
}

#[tokio::test]
async fn explicit_member_limit_zero_rejects_generation_before_upstream() {
    let e = setup(&[]).await;
    e.state.db.run_script(vec![
        r#"UPDATE api_keys SET quota_limits='{"images":{"mode":"limit","value":0},"v5":{"mode":"unlimited"},"daily_anlas":{"mode":"inherit"},"monthly_anlas":{"mode":"unlimited"}}' WHERE id=1"#.into(),
    ]).await.unwrap();
    let (status, _, body) = call(
        &e.state,
        "POST",
        "/ai/generate-image",
        Some(TOKEN),
        Some(image_body("nai-diffusion-4-5-full", 1)),
    )
    .await;
    assert_eq!(status, 403, "{}", String::from_utf8_lossy(&body));
    assert!(err_message(&body).contains("今日 V4.5 额度已用完"));
    assert!(e.mock.requests_to("/ai/generate-image").is_empty());
}

#[tokio::test]
async fn explicit_global_v5_limit_zero_rejects_generation_before_upstream() {
    let e = setup(&[]).await;
    e.state.db.set_setting("quota_modes_v1", r#"{"register_daily_images":{"mode":"unlimited"},"register_daily_v5":{"mode":"unlimited"},"global_daily_v5":{"mode":"limit","value":0},"global_daily_anlas":{"mode":"unlimited"},"member_daily_anlas":{"mode":"unlimited"},"global_monthly_anlas":{"mode":"unlimited"},"public_vibe_enabled":false}"#).await.unwrap();
    e.state
        .db
        .run_script(vec![
            "UPDATE api_keys SET image_model_scope='all' WHERE id=1".into(),
        ])
        .await
        .unwrap();
    let (status, _, body) = call(
        &e.state,
        "POST",
        "/ai/generate-image",
        Some(TOKEN),
        Some(image_body("nai-diffusion-5-full", 1)),
    )
    .await;
    assert_eq!(status, 403, "{}", String::from_utf8_lossy(&body));
    assert!(
        err_message(&body).contains("全站今日 V5 额度已用完"),
        "{}",
        String::from_utf8_lossy(&body)
    );
    assert!(e.mock.requests_to("/ai/generate-image").is_empty());
}

fn vibe_payload() -> Value {
    use base64::Engine;
    let image = image::GrayImage::from_fn(64, 64, |x, y| image::Luma([((x * 4) ^ y) as u8]));
    let mut png = std::io::Cursor::new(Vec::new());
    image.write_to(&mut png, image::ImageFormat::Png).unwrap();
    json!({
        "image": base64::engine::general_purpose::STANDARD.encode(png.into_inner()),
        "model": "nai-diffusion-4-5-full",
        "informationExtracted": 0.5
    })
}

async fn configure_public_vibe(e: &common::Env, enabled: bool, pool: Value, allow_anlas: bool) {
    e.state
        .db
        .set_setting(
            "features_global",
            r#"{"image":true,"upscale":true,"augment":true,"vibe":true,"tags":true}"#,
        )
        .await
        .unwrap();
    e.state
        .db
        .set_setting(
            "quota_modes_v1",
            &json!({
                "register_daily_images":{"mode":"unlimited"},
                "register_daily_v5":{"mode":"unlimited"},
                "global_daily_v5":{"mode":"unlimited"},
                "global_daily_anlas":pool,
                "member_daily_anlas":{"mode":"unlimited"},
                "global_monthly_anlas":{"mode":"unlimited"},
                "public_vibe_enabled":enabled
            })
            .to_string(),
        )
        .await
        .unwrap();
    e.state
        .db
        .run_script(vec![format!(
            "UPDATE api_keys SET allow_anlas={} WHERE id=1",
            i32::from(allow_anlas)
        )])
        .await
        .unwrap();
}

fn encoded_vibe_reply() -> Reply {
    Reply::Status {
        status: 200,
        headers: vec![("content-type".into(), "application/octet-stream".into())],
        body: bytes::Bytes::from_static(b"encoded-vibe"),
    }
}

#[tokio::test]
async fn public_vibe_uses_canonical_flag_and_anlas_pool_at_the_real_route() {
    let e = setup(&[]).await;
    configure_public_vibe(&e, true, json!({"mode":"unlimited"}), false).await;
    e.mock.push("/ai/encode-vibe", encoded_vibe_reply());
    let (status, _, body) = call(
        &e.state,
        "POST",
        "/ai/encode-vibe",
        Some(TOKEN),
        Some(vibe_payload()),
    )
    .await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    assert_eq!(e.mock.requests_to("/ai/encode-vibe").len(), 1);

    let e = setup(&[]).await;
    configure_public_vibe(&e, true, json!({"mode":"limit","value":0}), false).await;
    e.mock.push("/ai/encode-vibe", encoded_vibe_reply());
    let (status, _, body) = call(
        &e.state,
        "POST",
        "/ai/encode-vibe",
        Some(TOKEN),
        Some(vibe_payload()),
    )
    .await;
    assert_eq!(status, 403, "{}", String::from_utf8_lossy(&body));
    assert!(err_message(&body).contains("全站今日 Vibe 额度已用完"));
    assert!(e.mock.requests_to("/ai/encode-vibe").is_empty());

    let e = setup(&[]).await;
    configure_public_vibe(&e, false, json!({"mode":"unlimited"}), false).await;
    e.mock.push("/ai/encode-vibe", encoded_vibe_reply());
    let (status, _, body) = call(
        &e.state,
        "POST",
        "/ai/encode-vibe",
        Some(TOKEN),
        Some(vibe_payload()),
    )
    .await;
    assert_eq!(status, 403, "{}", String::from_utf8_lossy(&body));
    assert!(
        err_message(&body).contains("本站未开放"),
        "{}",
        String::from_utf8_lossy(&body)
    );
    assert!(e.mock.requests_to("/ai/encode-vibe").is_empty());

    // Explicit Anlas access remains independent of the free public-Vibe switch.
    let e = setup(&[]).await;
    configure_public_vibe(&e, false, json!({"mode":"unlimited"}), true).await;
    e.mock.push("/ai/encode-vibe", encoded_vibe_reply());
    let (status, _, body) = call(
        &e.state,
        "POST",
        "/ai/encode-vibe",
        Some(TOKEN),
        Some(vibe_payload()),
    )
    .await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    assert_eq!(e.mock.requests_to("/ai/encode-vibe").len(), 1);
}

#[tokio::test]
async fn malformed_canonical_quota_fails_closed_instead_of_legacy_zero_unlimited() {
    let e = setup(&[]).await;
    e.state.db.run_script(vec![
        r#"UPDATE api_keys SET daily_images=0, quota_auto=-1, quota_limits='{"images":{"mode":"limit","value":-1},"v5":{"mode":"unlimited"},"daily_anlas":{"mode":"inherit"},"monthly_anlas":{"mode":"unlimited"}}' WHERE id=1"#.into(),
    ]).await.unwrap();
    let (status, _, body) = call(
        &e.state,
        "POST",
        "/ai/generate-image",
        Some(TOKEN),
        Some(image_body("nai-diffusion-4-5-full", 1)),
    )
    .await;
    assert_eq!(status, 500, "{}", String::from_utf8_lossy(&body));
    assert!(e.mock.requests_to("/ai/generate-image").is_empty());
}

/// 每把 Key 的限速只剩排队与 RPM 两层：
/// - 同一把 Key 同时只生成 1 张：后面的排队、等前一张出完才发往上游（即使还有空着的上游账号）——
///   这是排队本身做的，旧版靠每把 Key 一个的信号量；
/// - 每人最多同时排 2 张（`queue_per_member`，正在生成的不算）；再发当场拒绝（`member_queue_full`，
///   10-22 之前是 `key_busy`），不再有「同时进行中的请求过多」（`key_rate`）；
/// - 前一张出完，下一张马上能发，没有 15 秒的每 Key 出图间隔。
#[tokio::test]
async fn one_image_per_key_at_a_time_without_per_key_interval() {
    let e = setup(&[("NAI_TOKENS", "pst-synthetic-one,pst-synthetic-two")]).await;
    // 账号级的随机抖动（0～5 秒）与这里无关，关掉免得计时不稳
    e.state
        .guard
        .with(|g| g.values.insert("interval_jitter".into(), 0));
    e.mock.push(
        "/ai/generate-image",
        Reply::Chunks {
            status: 200,
            content_type: "binary/octet-stream".into(),
            chunks: vec![(
                std::time::Duration::from_millis(600),
                bytes::Bytes::from(gate_testkit::zip_of_pngs(1)),
            )],
        },
    );
    let spawn = |state: AppState| {
        tokio::spawn(async move {
            call(
                &state,
                "POST",
                "/ai/generate-image",
                Some(TOKEN),
                Some(image_body("nai-diffusion-4-5-full", 1)),
            )
            .await
        })
    };
    let first = spawn(e.state.clone());
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    let second = spawn(e.state.clone());
    let third = spawn(e.state.clone());
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    // 第一张在上游，后两张在排队：第二个上游账号空着也不发
    assert_eq!(e.mock.requests_to("/ai/generate-image").len(), 1);
    let view = e.state.guard.with(|g| g.queue_view(Some(1)));
    assert_eq!(
        (view["running"].as_i64(), view["waiting"].as_i64()),
        (Some(1), Some(2))
    );
    // 第四、五张：每人排队满了（生成 1 张 + 排队 2 张）
    for _ in 0..2 {
        let (status, _, body) = call(
            &e.state,
            "POST",
            "/ai/generate-image",
            Some(TOKEN),
            Some(image_body("nai-diffusion-4-5-full", 1)),
        )
        .await;
        let text = String::from_utf8_lossy(&body).to_string();
        assert_eq!(status, 429, "{text}");
        assert!(
            text.contains("已有 2 张在排队，请等前面完成后再发"),
            "{text}"
        );
    }
    let (s1, _, _) = first.await.unwrap();
    let (s2, _, _) = second.await.unwrap();
    let (s3, _, _) = third.await.unwrap();
    assert_eq!(
        (s1, s2, s3),
        (StatusCode::OK, StatusCode::OK, StatusCode::OK)
    );
    assert_eq!(e.mock.requests_to("/ai/generate-image").len(), 3);
    // 前一张出完，下一张立刻能发（没有每 Key 15 秒间隔）
    let t0 = std::time::Instant::now();
    let (status, _, _) = call(
        &e.state,
        "POST",
        "/ai/generate-image",
        Some(TOKEN),
        Some(image_body("nai-diffusion-4-5-full", 1)),
    )
    .await;
    assert_eq!(status, 200);
    assert!(
        t0.elapsed() < std::time::Duration::from_secs(3),
        "{:?}",
        t0.elapsed()
    );
    let rows = logs(&e.state).await;
    let reasons: Vec<&str> = rows
        .iter()
        .filter(|r| r["status"] == "rejected")
        .map(|r| r["reason"].as_str().unwrap_or(""))
        .collect();
    assert_eq!(
        reasons,
        ["member_queue_full", "member_queue_full"],
        "{rows:?}"
    );
}

/// 「V5 续杯」已删除：旧版 `anlas_auto` 的 Key 当天个人 V5 用完、
/// 账号 V5 也见底时会改用 Anlas 出同规格 V5；现在个人 V5 用完就是 403，不会悄悄花 Anlas。
#[tokio::test]
async fn no_v5_refill_with_anlas() {
    let e = setup(&[]).await;
    e.state.db.run_script(vec![r#"UPDATE api_keys SET allow_anlas=1, quota_auto=-1, quota_limits='{"images":{"mode":"unlimited"},"v5":{"mode":"limit","value":1},"daily_anlas":{"mode":"limit","value":500},"monthly_anlas":{"mode":"unlimited"}}', image_model_scope='all' WHERE id=1"#.into()]).await.unwrap();
    e.state
        .db
        .set_setting("quota_algo_last", r#"{"v5": {"percent": 0}}"#)
        .await
        .unwrap();
    e.state
        .db
        .record_success(
            &Success {
                key_id: 1,
                kind: "image".into(),
                model: "nai-diffusion-5-full".into(),
                day: e.state.day(),
                images: 1,
                v5: 1,
                ..Default::default()
            },
            e.state.now(),
        )
        .await
        .unwrap();
    let (status, _, body) = call(
        &e.state,
        "POST",
        "/ai/generate-image",
        Some(TOKEN),
        Some(image_body("nai-diffusion-5-full", 1)),
    )
    .await;
    let text = String::from_utf8_lossy(&body).to_string();
    assert_eq!(status, 403, "{text}");
    assert!(text.contains("今日 V5 额度已用完，每天 1 张"), "{text}");
    assert!(e.mock.requests_to("/ai/generate-image").is_empty());
}

#[tokio::test]
async fn restricted_v5_denial_does_not_dispatch_or_consume_quota() {
    let e = setup(&[]).await;
    e.state.db.run_script(vec![
        "UPDATE api_keys SET image_model_scope='all', discord_id='900000000000000041' WHERE id=1".into(),
    ]).await.unwrap();
    e.state
        .db
        .set_setting(gate_server::v5_access::MODE_KEY, "restricted")
        .await
        .unwrap();
    e.state
        .db
        .set_setting(
            gate_server::v5_access::SUBJECTS_KEY,
            r#"["900000000000000042"]"#,
        )
        .await
        .unwrap();
    let (status, _, body) = call(
        &e.state,
        "POST",
        "/ai/generate-image",
        Some(TOKEN),
        Some(image_body("nai-diffusion-5-full", 1)),
    )
    .await;
    assert_eq!(status, 403, "{}", String::from_utf8_lossy(&body));
    assert!(e.mock.requests_to("/ai/generate-image").is_empty());
    assert_eq!(
        e.state.db.get_counter(1, &e.state.day()).await.unwrap().v5,
        0
    );
    let logs = e
        .state
        .db
        .list_logs(1, 0, &gate_db::repo::usage::LogFilter::default())
        .await
        .unwrap();
    assert_eq!(logs[0]["reason"], gate_core::reasons::FEATURE_OFF);
}

#[tokio::test]
async fn queued_v5_is_rechecked_after_subject_revocation() {
    let e = setup(&[]).await;
    e.state.db.run_script(vec![
        "UPDATE api_keys SET image_model_scope='all', discord_id='900000000000000061' WHERE id=1".into(),
    ]).await.unwrap();
    e.state
        .db
        .set_setting(gate_server::v5_access::MODE_KEY, "restricted")
        .await
        .unwrap();
    e.state
        .db
        .set_setting(
            gate_server::v5_access::SUBJECTS_KEY,
            r#"["900000000000000061"]"#,
        )
        .await
        .unwrap();
    e.mock.push(
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
    let state = e.state.clone();
    let first = tokio::spawn(async move {
        call(
            &state,
            "POST",
            "/ai/generate-image",
            Some(TOKEN),
            Some(image_body("nai-diffusion-5-full", 1)),
        )
        .await
    });
    for _ in 0..200 {
        if e.mock.requests_to("/ai/generate-image").len() == 1 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    assert_eq!(
        e.mock.requests_to("/ai/generate-image").len(),
        1,
        "first request should be in flight"
    );
    let state = e.state.clone();
    let second = tokio::spawn(async move {
        call(
            &state,
            "POST",
            "/ai/generate-image",
            Some(TOKEN),
            Some(image_body("nai-diffusion-5-full", 1)),
        )
        .await
    });
    let mut queued = false;
    for _ in 0..200 {
        let view = e.state.guard.with(|g| g.queue_view(Some(1)));
        if view["mine"]
            .as_array()
            .is_some_and(|items| items.iter().any(|item| item["state"] == "waiting"))
        {
            queued = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    assert!(queued, "second image must be waiting before revocation");
    e.state
        .db
        .set_setting(gate_server::v5_access::SUBJECTS_KEY, "[]")
        .await
        .unwrap();
    assert_eq!(first.await.unwrap().0, StatusCode::OK);
    let (status, _, body) = second.await.unwrap();
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "{}",
        String::from_utf8_lossy(&body)
    );
    assert_eq!(e.mock.requests_to("/ai/generate-image").len(), 1);
    assert_eq!(
        e.state.db.get_counter(1, &e.state.day()).await.unwrap().v5,
        1
    );
    let logs = e
        .state
        .db
        .list_logs(1, 0, &gate_db::repo::usage::LogFilter::default())
        .await
        .unwrap();
    assert_eq!(logs[0]["reason"], gate_core::reasons::FEATURE_OFF);
}

#[tokio::test]
async fn owner_identity_survives_member_key_rotation() {
    let e = setup(&[]).await;
    e.state
        .db
        .run_script(vec![
            "UPDATE api_keys SET discord_id='900000000000000071' WHERE id=1".into(),
        ])
        .await
        .unwrap();
    e.state
        .db
        .set_setting(gate_server::v5_access::MODE_KEY, "restricted")
        .await
        .unwrap();
    e.state
        .db
        .set_setting(
            gate_server::v5_access::SUBJECTS_KEY,
            r#"["900000000000000071"]"#,
        )
        .await
        .unwrap();
    e.state
        .db
        .set_setting(
            gate_server::v5_access::OWNERS_KEY,
            r#"["900000000000000071"]"#,
        )
        .await
        .unwrap();
    assert!(
        e.state
            .db
            .rotate_key_token(1, "synthetic-rotated-member-key")
            .await
            .unwrap()
    );
    let key = gate_server::key::Key(e.state.db.get_key(1).await.unwrap().unwrap());
    let access = gate_server::v5_access::evaluate(&e.state, Some(key.str("discord_id")))
        .await
        .unwrap();
    assert!(access.decision.allowed);
    assert!(access.owner);
}

/// V5 也在入口预检：同一把 Key 的前一张 V5 已经拿到预留、正在出图，而今天只剩这 1 张时，
/// 后一张立即 403，不排队等前一张出完（10-21 线上：第二张排了 47 秒才被告知额度用完）。
#[tokio::test]
async fn second_v5_is_refused_at_once_when_the_first_holds_the_last_slot() {
    let e = setup(&[]).await;
    e.state
        .db
        .run_script(vec![
            r#"UPDATE api_keys SET image_model_scope='all', daily_v5=2, quota_auto=-1, quota_limits='{"images":{"mode":"unlimited"},"v5":{"mode":"limit","value":2},"daily_anlas":{"mode":"inherit"},"monthly_anlas":{"mode":"unlimited"}}' WHERE id=1"#.into(),
        ])
        .await
        .unwrap();
    e.state
        .db
        .record_success(
            &Success {
                key_id: 1,
                kind: "image".into(),
                model: "nai-diffusion-5-full".into(),
                day: e.state.day(),
                images: 1,
                v5: 1,
                ..Default::default()
            },
            e.state.now(),
        )
        .await
        .unwrap();
    // 第一张在上游慢慢出（5 秒），期间一直拿着最后 1 张的预留
    e.mock.push(
        "/ai/generate-image",
        Reply::Chunks {
            status: 200,
            content_type: "binary/octet-stream".into(),
            chunks: vec![(
                std::time::Duration::from_millis(5000),
                bytes::Bytes::from(gate_testkit::zip_of_pngs(1)),
            )],
        },
    );
    let state = e.state.clone();
    let first = tokio::spawn(async move {
        call(
            &state,
            "POST",
            "/ai/generate-image",
            Some(TOKEN),
            Some(image_body("nai-diffusion-5-full", 1)),
        )
        .await
    });
    // 等第一张真的发到上游（此时它已拿到预留），不靠固定的等待时间
    for _ in 0..100 {
        if !e.mock.requests_to("/ai/generate-image").is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert_eq!(e.mock.requests_to("/ai/generate-image").len(), 1);

    let (status, _, body) = call(
        &e.state,
        "POST",
        "/ai/generate-image",
        Some(TOKEN),
        Some(image_body("nai-diffusion-5-full", 1)),
    )
    .await;
    let text = String::from_utf8_lossy(&body).to_string();
    assert_eq!(status, 403, "{text}");
    assert!(text.contains("今日 V5 额度已用完，每天 2 张"), "{text}");
    // 第二张在第一张出完之前就被拒（修复前它要排到第一张出完才拿到 403）
    assert!(!first.is_finished(), "第二张应在第一张出完前就被拒");
    // 没有进排队，也没有发到上游
    let view = e.state.guard.with(|g| g.queue_view(Some(1)));
    assert_eq!(view["waiting"].as_i64(), Some(0));

    let (s1, _, _) = first.await.unwrap();
    assert_eq!(s1, StatusCode::OK);
    assert_eq!(e.mock.requests_to("/ai/generate-image").len(), 1);
    let rows = logs(&e.state).await;
    let reasons: Vec<&str> = rows
        .iter()
        .filter(|r| r["status"] == "rejected")
        .map(|r| r["reason"].as_str().unwrap_or(""))
        .collect();
    assert_eq!(reasons, ["member_quota"], "{rows:?}");
}

#[tokio::test]
async fn auth_errors_have_novelai_shape() {
    let e = setup(&[]).await;
    let (status, _, body) = call(
        &e.state,
        "POST",
        "/ai/generate-image",
        None,
        Some(image_body("x", 1)),
    )
    .await;
    assert_eq!(status, 401);
    let v: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        v["message"],
        "猫头鹰公益站提醒：未收到 API Key。请在客户端中填入本站发放的 Key。开发者参考：放在 Authorization: Bearer 请求头中"
    );
    assert_eq!(v["statusCode"], 401);
    let (status, _, body) = call(
        &e.state,
        "POST",
        "/ai/generate-image",
        Some("nai-wrong"),
        Some(image_body("x", 1)),
    )
    .await;
    assert_eq!(status, 401);
    let v: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        v["error"]["message"],
        "猫头鹰公益站提醒：API Key 无效。请检查客户端中的 Key 是否完整，或在 Discord 用 /resetkey 重新获取。"
    );
    assert!(e.mock.requests().is_empty());
}

/// 停用的 Key：按停用原因给出说明。全站重置停掉的要告诉成员去重新领。
#[tokio::test]
async fn disabled_key_message_depends_on_reason() {
    let e = setup(&[]).await;
    let reason = |state: gate_server::AppState| async move {
        state
            .db
            .get_key(1)
            .await
            .unwrap()
            .unwrap()
            .get("disabled_reason")
            .cloned()
            .unwrap_or(Value::Null)
    };
    let request = || {
        call(
            &e.state,
            "POST",
            "/ai/generate-image",
            Some(TOKEN),
            Some(image_body("x", 1)),
        )
    };

    // 全站重置
    e.state.db.reset_all_keys().await.unwrap();
    assert_eq!(reason(e.state.clone()).await, "site_reset");
    let (status, _, body) = request().await;
    assert_eq!(status, 403);
    assert!(
        err_message(&body).contains("全站重置") && err_message(&body).contains("/register"),
        "{}",
        err_message(&body)
    );

    // 重新启用：原因清掉；站长手动停用：另一种说明
    let on = json!({"enabled": true});
    e.state
        .db
        .update_key(1, on.as_object().unwrap())
        .await
        .unwrap();
    assert_eq!(reason(e.state.clone()).await, Value::Null);
    let off = json!({"enabled": false});
    e.state
        .db
        .update_key(1, off.as_object().unwrap())
        .await
        .unwrap();
    assert_eq!(reason(e.state.clone()).await, "admin");
    let (status, _, body) = request().await;
    assert_eq!(status, 403);
    assert!(
        err_message(&body).ends_with("这把 Key 已被停用。如有疑问，请在 Discord 服务器联系站长。"),
        "{}",
        err_message(&body)
    );
    assert!(e.mock.requests().is_empty());
}

/// 被临时暂停的 Key：没写原因时只说「已被临时暂停，到点自动恢复」（防分享体系已删，不再提「异常多地使用」）；
/// 写了原因（自动驾驶的限流）就用原因。
#[tokio::test]
async fn paused_key_message_without_reason_is_neutral() {
    let e = setup(&[]).await;
    let request = || {
        call(
            &e.state,
            "POST",
            "/ai/generate-image",
            Some(TOKEN),
            Some(image_body("x", 1)),
        )
    };
    let until = e.state.now() + 600.0;

    e.state.pauses.set(1, until, String::new());
    let (status, _, body) = request().await;
    assert_eq!(status, 403);
    let m = err_message(&body);
    assert!(
        m.contains("这把 Key 已被临时暂停，") && m.ends_with(" 自动恢复。"),
        "{m}"
    );
    assert!(!m.contains("多地") && !m.contains("仅限本人"), "{m}");

    e.state.pauses.set(
        1,
        until,
        "你的 Key 短时间内被大量拒绝，已暂停 1 小时".into(),
    );
    let (status, _, body) = request().await;
    assert_eq!(status, 403);
    let m = err_message(&body);
    assert!(
        m.contains("你的 Key 短时间内被大量拒绝，已暂停 1 小时，") && m.ends_with(" 自动恢复。"),
        "{m}"
    );
    assert!(e.mock.requests().is_empty());
}

/// 「每位成员到网页登录一次」：开关打开后，这之后没在网页登录过的成员 Key 先拦下；网页登录一次（哪怕没算出设备指纹）
/// 立即放行；没绑 Discord 的 Key（这里的默认 Key）不拦；开关关掉就都不拦。
/// 放行按永久保留的 web_logins：30 天清理 device_logins 后不会又拦回去。
#[tokio::test]
async fn web_login_required_blocks_until_the_member_logs_in_once() {
    let e = setup(&[]).await;
    let did = "900000000000000777";
    common::member_key(&e.state, 1, &[("discord_id", json!(did))]).await;
    let member = "nai-synthetic-0001";
    let ask = |token: &'static str| {
        let st = e.state.clone();
        async move {
            call(
                &st,
                "POST",
                "/ai/generate-image",
                Some(token),
                Some(image_body("x", 1)),
            )
            .await
        }
    };
    let set_since = |v: String| {
        let st = e.state.clone();
        async move {
            st.db
                .set_setting("web_login_required_since", &v)
                .await
                .unwrap();
            // 开关值缓存 15 秒：测试里改完立即生效
            gate_server::auth::clear_web_login_cache(st.db.path());
        }
    };
    let now = e.state.now();
    // 开关关着时先出一张图：缓存里记下「不要求」，打开后清缓存必须立即生效
    let (_, _, body) = ask(member).await;
    assert!(!err_message(&body).contains("用 Discord 登录一次"));
    set_since(format!("{}", now - 10.0)).await;

    let (status, _, body) = ask(member).await;
    assert_eq!(status, 403);
    assert!(
        err_message(&body).contains("用 Discord 登录一次"),
        "{}",
        err_message(&body)
    );
    // 没绑 Discord 的 Key 不拦
    let (_, _, body) = ask(TOKEN).await;
    assert!(!err_message(&body).contains("用 Discord 登录一次"));

    // 开关之前的登录不算；之后登录一次（没有指纹也算）就放行（没登录过的结果不缓存）
    e.state
        .db
        .record_web_login(did, "none", "1.2.3.4", now - 3600.0)
        .await
        .unwrap();
    let (status, _, _) = ask(member).await;
    assert_eq!(status, 403);
    // 只记 device_logins 不算（开关只认 web_logins）
    e.state
        .db
        .record_device_login(did, "abc", "1.2.3.4", now)
        .await
        .unwrap();
    let (status, _, _) = ask(member).await;
    assert_eq!(status, 403);
    e.state
        .db
        .record_web_login(did, "none", "1.2.3.4", now)
        .await
        .unwrap();
    let (_, _, body) = ask(member).await;
    assert!(
        !err_message(&body).contains("用 Discord 登录一次"),
        "{}",
        err_message(&body)
    );
    // device_logins 被清空后不会又拦回去（清缓存后照样从库里查得到 web_logins）
    e.state
        .db
        .write(|tx| tx.execute("DELETE FROM device_logins", []))
        .await
        .unwrap();
    gate_server::auth::clear_web_login_cache(e.state.db.path());
    let (_, _, body) = ask(member).await;
    assert!(
        !err_message(&body).contains("用 Discord 登录一次"),
        "{}",
        err_message(&body)
    );
    // 开始时刻往后挪：之前的登录不再算，放行缓存跟着作废
    set_since(format!("{}", now + 10.0)).await;
    let (status, _, _) = ask(member).await;
    assert_eq!(status, 403);

    // 关掉开关（0）：另一个没登录过的成员也不拦
    common::member_key(&e.state, 2, &[("discord_id", json!("900000000000000778"))]).await;
    set_since("0".into()).await;
    let (_, _, body) = ask("nai-synthetic-0002").await;
    assert!(!err_message(&body).contains("用 Discord 登录一次"));
}

#[tokio::test]
async fn unknown_model_is_rejected_and_logged() {
    let e = setup(&[]).await;
    let (status, _, body) = call(
        &e.state,
        "POST",
        "/ai/generate-image",
        Some(TOKEN),
        Some(image_body("not-a-model", 1)),
    )
    .await;
    assert_eq!(status, 400, "{}", String::from_utf8_lossy(&body));
    let rows = logs(&e.state).await;
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0]["status"], "rejected");
    assert_eq!(rows[0]["reason"], "feature_off", "{rows:?}");
    assert!(e.mock.requests_to("/ai/generate-image").is_empty());
    let correlated_dispatches: i64 = e
        .state
        .db
        .read(|c| {
            c.query_row(
                "SELECT COUNT(*) FROM request_settlements WHERE correlation_id!=''",
                [],
                |r| r.get(0),
            )
        })
        .await
        .unwrap();
    assert_eq!(
        correlated_dispatches, 0,
        "rejected requests are not correlated as sent"
    );
}

/// 参数校验挡下的拒绝（这条路径记日志时没带原因码）：写日志时就归类成 param，不等重启回填。
#[tokio::test]
async fn bad_weight_is_rejected_with_param_reason() {
    let e = setup(&[]).await;
    let mut body = image_body("nai-diffusion-4-5-full", 1);
    body["input"] = json!("1girl, 2025::solo::");
    let (status, _, resp) = call(
        &e.state,
        "POST",
        "/ai/generate-image",
        Some(TOKEN),
        Some(body),
    )
    .await;
    assert_eq!(status, 400, "{}", String::from_utf8_lossy(&resp));
    let rows = logs(&e.state).await;
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0]["status"], "rejected");
    assert_eq!(rows[0]["reason"], "param", "{rows:?}");
    assert!(e.mock.requests_to("/ai/generate-image").is_empty());
}

#[tokio::test]
async fn upstream_429_pauses_all_image_generation() {
    let e = setup(&[]).await;
    e.mock.push("/ai/generate-image", Reply::rate_limited("30"));
    let (status, _, body) = call(
        &e.state,
        "POST",
        "/ai/generate-image",
        Some(TOKEN),
        Some(image_body("nai-diffusion-4-5-full", 1)),
    )
    .await;
    assert_eq!(status, 429, "{}", String::from_utf8_lossy(&body));
    assert!(e.state.cooldown.remaining(e.state.now()) >= 59);
    let (status, _, body) = call(
        &e.state,
        "POST",
        "/ai/generate-image",
        Some(TOKEN),
        Some(image_body("nai-diffusion-4-5-full", 1)),
    )
    .await;
    assert_eq!(status, 429);
    let v: Value = serde_json::from_slice(&body).unwrap();
    assert!(
        v["message"].as_str().unwrap().contains("全站暂停出图约"),
        "{v}"
    );
    let rows = logs(&e.state).await;
    let reasons: Vec<&str> = rows
        .iter()
        .map(|r| r["reason"].as_str().unwrap_or(""))
        .collect();
    assert!(reasons.contains(&"cooldown"), "{rows:?}");
    assert_eq!(e.mock.requests_to("/ai/generate-image").len(), 1);
}

#[tokio::test]
async fn stream_sse_ok() {
    let e = setup(&[]).await;
    let (status, headers, body) = call(
        &e.state,
        "POST",
        "/ai/generate-image-stream",
        Some(TOKEN),
        Some(image_body("nai-diffusion-4-5-full", 1)),
    )
    .await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    assert_eq!(headers["content-type"], "text/event-stream");
    let text = String::from_utf8_lossy(&body);
    assert!(text.contains("final"), "{text}");
    let rows = logs(&e.state).await;
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0]["kind"], "image_stream");
    assert_eq!(rows[0]["status"], "ok");
    assert_eq!(rows[0]["images"], 1);
    let rid = rows[0]["rid"].as_str().unwrap().to_string();
    let delivery = tokio::time::timeout(std::time::Duration::from_secs(1), async {
        loop {
            let rid = rid.clone();
            let state: Option<String> = e.state.db.read(move |c| c.query_row(
                "SELECT s.delivery_state FROM usage_log l JOIN usage_request_links u ON u.usage_log_id=l.id JOIN request_settlements s ON s.request_id=u.request_id WHERE l.rid=?",
                [rid], |r| r.get(0),
            ).optional()).await.unwrap();
            if state.as_deref() == Some("transport_complete") { break state.unwrap(); }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    }).await.unwrap();
    assert_eq!(delivery, "transport_complete");
}

#[tokio::test]
async fn stream_bad_wire_format_is_400_json() {
    let e = setup(&[]).await;
    let mut body = image_body("nai-diffusion-4-5-full", 1);
    body["parameters"]["stream"] = json!("xml");
    let (status, _, out) = call(
        &e.state,
        "POST",
        "/ai/generate-image-stream",
        Some(TOKEN),
        Some(body),
    )
    .await;
    assert_eq!(status, 400);
    let v: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(
        v["message"],
        "猫头鹰公益站提醒：本站不支持该流式格式，未扣费。请更新或更换客户端。给客户端作者：stream 只支持 sse 或 msgpack"
    );
}

#[tokio::test]
async fn stream_upstream_failure_before_start_is_plain_json() {
    let e = setup(&[]).await;
    e.mock.push(
        "/ai/generate-image-stream",
        Reply::json(500, &json!({"message": "boom"})),
    );
    let (status, headers, out) = call(
        &e.state,
        "POST",
        "/ai/generate-image-stream",
        Some(TOKEN),
        Some(image_body("nai-diffusion-4-5-full", 1)),
    )
    .await;
    assert!(status.is_server_error(), "{status}");
    assert_eq!(headers["content-type"], "application/json");
    let body: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(body["error"]["message"], body["message"], "{body}");
    assert_eq!(body["error"]["execution_unknown"], true, "{body}");
    assert_eq!(body["error"]["retryable"], false, "{body}");
    assert_eq!(headers["cache-control"], "no-store");
    assert_eq!(headers["x-accel-buffering"], "no");
    let rows = logs(&e.state).await;
    assert!(rows.iter().any(|r| r["status"] == "error"), "{rows:?}");
    // 客户端拿到了 JSON 错误，不是「断开」
}

#[tokio::test]
async fn suggest_tags_get() {
    let e = setup(&[]).await;
    let (status, _, out) = call(
        &e.state,
        "GET",
        "/ai/generate-image/suggest-tags?model=nai-diffusion-4-5-full&prompt=1g",
        Some(TOKEN),
        None,
    )
    .await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&out));
    let sent = e.mock.requests_to("/ai/generate-image/suggest-tags");
    assert_eq!(sent[0].query, "prompt=1g&model=nai-diffusion-4-5-full");
    let rows = logs(&e.state).await;
    assert_eq!(rows[0]["kind"], "tags");
    assert_eq!(rows[0]["status"], "ok");
}

#[tokio::test]
async fn v1_alias_and_root_post_route_to_generate() {
    let e = setup(&[]).await;
    let (status, _, _) = call(
        &e.state,
        "POST",
        "/v1/ai/generate-image",
        Some(TOKEN),
        Some(image_body("nai-diffusion-4-5-full", 1)),
    )
    .await;
    assert_eq!(status, 200);
    let (status, _, _) = call(
        &e.state,
        "POST",
        "/",
        Some(TOKEN),
        Some(image_body("nai-diffusion-4-5-full", 1)),
    )
    .await;
    assert_eq!(status, 200);
}

#[tokio::test]
async fn cors_preflight_allows_authorization_and_fallbacks_are_json() {
    let e = setup(&[]).await;
    let req = Request::builder()
        .method("OPTIONS")
        .uri("/ai/generate-image")
        .header("origin", "https://client.example")
        .header("access-control-request-method", "POST")
        .header(
            "access-control-request-headers",
            "authorization,content-type",
        )
        .body(Body::empty())
        .unwrap();
    let resp = gate_server::app(e.state.clone())
        .oneshot(req)
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let h = resp.headers();
    assert_eq!(h["access-control-allow-origin"], "*");
    assert!(
        h["access-control-allow-headers"]
            .to_str()
            .unwrap()
            .contains("authorization")
    );
    let (status, _, body) = call(&e.state, "GET", "/no-such-path", None, None).await;
    assert_eq!(
        (status.as_u16(), String::from_utf8_lossy(&body).to_string()),
        (404, r#"{"detail":"Not Found"}"#.to_string())
    );
    let (status, _, body) = call(&e.state, "GET", "/ai/generate-image", Some(TOKEN), None).await;
    assert_eq!(
        (status.as_u16(), String::from_utf8_lossy(&body).to_string()),
        (405, r#"{"detail":"Method Not Allowed"}"#.to_string())
    );
}

/// 记账恒等式（原「观测」「容量」模块在线上核对的，现在由测试守住）：每个成功的出图请求，
/// 今日计数器、用量日志、生成记录三处的张数一致；内存里的每小时计数等于本小时日志里的张数，且不超过上限。
#[tokio::test]
async fn accounting_identity_across_counters_logs_and_audit() {
    let e = setup(&[]).await;
    e.state.db.set_setting("audit_prompts", "1").await.unwrap();
    e.state.db.set_setting("audit_thumbs", "1").await.unwrap();
    const UA: &str = "DSH-Tavern-Image/1.2 (Windows NT 10.0)";
    for n in [1, 2] {
        let mut body = image_body("nai-diffusion-4-5-full", n);
        body["parameters"]["ucPresetId"] = json!(1);
        let (status, _, body) = Call::new("POST", "/ai/generate-image")
            .bearer(TOKEN)
            .header("user-agent", UA)
            .header("x-correlation-id", "synthetic")
            .header("x-initiated-at", "1")
            .json(body)
            .send(&e.state)
            .await;
        assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    }
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    // 10-18 起记下原始 User-Agent（不「识别」客户端，只存原文供站长筛查；保留天数按后台设置，默认永久）
    let clients = common::query(&e.state.db, "SELECT DISTINCT client FROM usage_log").await;
    assert_eq!(clients, [json!({"client": UA})]);
    // 开了记录原图：原图确实存下来了
    assert!(e.state.db.audit_image_bytes().await.unwrap() > 0);
    let counter = e
        .state
        .db
        .get_counter(1, &e.state.day())
        .await
        .unwrap()
        .images;
    let logged = common::query(
        &e.state.db,
        "SELECT COALESCE(SUM(images), 0) AS n FROM usage_log WHERE status='ok' AND kind LIKE 'image%'",
    )
    .await[0]["n"]
        .as_i64()
        .unwrap();
    let audited = common::query(
        &e.state.db,
        "SELECT COUNT(*) AS n FROM generation_audit WHERE status='ok'",
    )
    .await[0]["n"]
        .as_i64()
        .unwrap();
    // 每张成功出图一条生成记录（一次请求多张时网关按实际出的张数记）
    assert!(logged >= 2, "{logged}");
    assert_eq!((counter, audited), (logged, logged));
}

// ---------------------------------------------------------------- V5 Medium 降档

/// 让测试成员能出 V5。
async fn allow_v5(state: &AppState) {
    state
        .db
        .run_script(vec![
            "UPDATE api_keys SET image_model_scope='all', daily_v5=10 WHERE id=1".into(),
        ])
        .await
        .unwrap();
}

async fn set_mode(state: &AppState, mode: &str) {
    state.db.set_setting("v5_medium_mode", mode).await.unwrap();
}

async fn v5_post(e: &common::Env, body: Value) -> (StatusCode, Vec<u8>) {
    let (status, _, bytes) = call(
        &e.state,
        "POST",
        "/ai/generate-image",
        Some(TOKEN),
        Some(body),
    )
    .await;
    (status, bytes)
}

fn last_sent(e: &common::Env) -> Value {
    e.mock
        .requests_to("/ai/generate-image")
        .last()
        .expect("上游收到了请求")
        .json()
}

#[tokio::test]
async fn medium_mode_off_leaves_v5_full_untouched() {
    let e = setup(&[]).await;
    allow_v5(&e.state).await;
    let (status, body) = v5_post(&e, image_body("nai-diffusion-5-full", 1)).await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    let sent = last_sent(&e);
    assert_eq!(sent["model"], "nai-diffusion-5-full");
    assert_eq!(sent["parameters"]["steps"], 28);
    let rows = logs(&e.state).await;
    let detail = rows[0]["detail"].as_str().unwrap();
    assert!(!detail.contains("Medium"), "{detail}");
}

#[tokio::test]
async fn medium_mode_on_rewrites_full_and_still_consumes_v5_quota() {
    let e = setup(&[]).await;
    allow_v5(&e.state).await;
    set_mode(&e.state, "on").await;
    // 大小写和空白不同的写法也认
    let (status, body) = v5_post(&e, image_body(" NAI-Diffusion-5-Full ", 1)).await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    let sent = last_sent(&e);
    assert_eq!(sent["model"], "nai-diffusion-5-full-medium");
    assert_eq!(sent["parameters"]["steps"], 14);
    assert_eq!(
        sent["parameters"]["negative_prompt"],
        gate_core::policy::V5_MEDIUM_UC
    );
    let rows = logs(&e.state).await;
    assert_eq!(rows[0]["model"], "nai-diffusion-5-full-medium");
    let detail = rows[0]["detail"].as_str().unwrap();
    // 按 Medium 估价、仍占 V5 免费额度，并带上说明
    assert!(detail.starts_with("832x1216/14step V5额度+1"), "{detail}");
    assert!(detail.contains("站长已临时切到 V5 Medium"), "{detail}");
    let c = e.state.db.get_counter(1, &e.state.day()).await.unwrap();
    assert_eq!(c.v5, 1);
}

#[tokio::test]
async fn medium_mode_on_leaves_other_models_alone() {
    let e = setup(&[]).await;
    allow_v5(&e.state).await;
    set_mode(&e.state, "on").await;
    // 已经是 Medium：不再提示
    let (status, _) = v5_post(&e, image_body("nai-diffusion-5-full-medium", 1)).await;
    assert_eq!(status, 200);
    assert_eq!(last_sent(&e)["model"], "nai-diffusion-5-full-medium");
    // Curated 与 V4.5 不动
    let (status, _) = v5_post(&e, image_body("nai-diffusion-5-curated", 1)).await;
    assert_eq!(status, 200);
    assert_eq!(last_sent(&e)["model"], "nai-diffusion-5-curated");
    let (status, _) = v5_post(&e, image_body("nai-diffusion-4-5-full", 1)).await;
    assert_eq!(status, 200);
    assert_eq!(last_sent(&e)["model"], "nai-diffusion-4-5-full");
    let rows = logs(&e.state).await;
    assert!(
        rows.iter().all(|r| !r["detail"]
            .as_str()
            .unwrap_or("")
            .contains("站长已临时切到")),
        "没被改写的请求不该带降档说明"
    );
}

#[tokio::test]
async fn v5_beyond_free_spec_is_refused_even_with_anlas() {
    // 站长 10-22：V5 只用免费额度；多张等要花 Anlas 的 V5 直接拒，即使这把 Key 开了 Anlas
    let e = setup(&[]).await;
    allow_v5(&e.state).await;
    e.state
        .db
        .run_script(vec!["UPDATE api_keys SET allow_anlas=1 WHERE id=1".into()])
        .await
        .unwrap();
    let before = e.mock.requests_to("/ai/generate-image").len();
    let (status, body) = v5_post(&e, image_body("nai-diffusion-5-full", 2)).await;
    assert_eq!(status, 403, "{}", String::from_utf8_lossy(&body));
    assert!(String::from_utf8_lossy(&body).contains("V5 仅支持免费规格"));
    assert_eq!(
        e.mock.requests_to("/ai/generate-image").len(),
        before,
        "不该发到上游"
    );
}

/// A single oversized V5 image has no free allowance in its estimate at all, so it never reaches the
/// V5 confirmation step: it is refused at the door instead of being billed to the member's Anlas.
#[tokio::test]
async fn single_oversized_v5_is_refused_before_queueing() {
    let e = setup(&[]).await;
    allow_v5(&e.state).await;
    e.state
        .db
        .run_script(vec![
            "UPDATE api_keys SET allow_anlas=1, daily_anlas=5000 WHERE id=1".into(),
        ])
        .await
        .unwrap();
    let mut body = image_body("nai-diffusion-5-full", 1);
    body["parameters"]["width"] = json!(1216);
    body["parameters"]["height"] = json!(1600);
    let (status, body) = v5_post(&e, body).await;
    let text = String::from_utf8_lossy(&body).to_string();
    assert_eq!(status, 403, "{text}");
    assert!(text.contains("V5 仅支持免费规格"), "{text}");
    assert!(e.mock.requests_to("/ai/generate-image").is_empty());
    assert_eq!(member_anlas(&e.state).await, 0.0);
}

/// 自动档 10-23 删除：库里存着 `auto` 的按关处理，余量再低也不降档。
#[tokio::test]
async fn medium_auto_is_gone_and_counts_as_off() {
    let e = setup(&[]).await;
    allow_v5(&e.state).await;
    set_mode(&e.state, "auto").await;
    e.mock.set_v5_percent(1);
    for _ in 0..2 {
        let (status, _) = v5_post(&e, image_body("nai-diffusion-5-full", 1)).await;
        assert_eq!(status, 200);
        assert_eq!(last_sent(&e)["model"], "nai-diffusion-5-full");
    }
    assert!(!gate_server::v5_medium::active(&e.state).await);
}

// ---------------------------------------------------------------- 连接测试请求拦截

const TEST_MESSAGE: &str =
    "连接正常，Key 有效。本站不处理连接测试请求，以免占用共享额度。请使用正常参数生图。";

fn probe_body(model: &str, prompt: &str, steps: i64, w: i64, h: i64) -> Value {
    json!({
        "input": prompt, "model": model, "action": "generate",
        "parameters": {"width": w, "height": h, "steps": steps, "scale": 5, "sampler": "k_euler_ancestral",
                       "n_samples": 1, "seed": 7},
    })
}

/// 连接测试：400 + 说明、上游零调用、所有计数不变、用量日志记 rejected / test_request。
#[tokio::test]
async fn connection_test_is_answered_without_touching_upstream_or_quota() {
    let e = setup(&[]).await;
    allow_v5(&e.state).await;
    let cases = [
        // SillyTavern 预设（V4.5 / V5）和另一个客户端的探测；普通接口、流式接口、/v1 别名都要拦
        (
            "/ai/generate-image",
            "nai-diffusion-4-5-full",
            "simple circle, white background",
            4,
            512,
            512,
        ),
        (
            "/ai/generate-image-stream",
            "nai-diffusion-5-full",
            "simple circle, white background",
            4,
            512,
            512,
        ),
        (
            "/v1/ai/generate-image",
            "nai-diffusion-4-5-full",
            "test",
            1,
            64,
            64,
        ),
    ];
    for (path, model, prompt, steps, w, h) in cases {
        let (status, _, body) = call(
            &e.state,
            "POST",
            path,
            Some(TOKEN),
            Some(probe_body(model, prompt, steps, w, h)),
        )
        .await;
        assert_eq!(status, 400, "{path} {}", String::from_utf8_lossy(&body));
        let v: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["statusCode"], 400);
        assert_eq!(v["message"], format!("猫头鹰公益站提醒：{TEST_MESSAGE}"));
    }
    assert!(e.mock.requests().is_empty(), "上游不该收到任何请求");
    let rows = logs(&e.state).await;
    assert_eq!(rows.len(), 3, "{rows:?}");
    for r in &rows {
        assert_eq!(r["status"], "rejected");
        assert_eq!(r["reason"], "test_request");
        assert_eq!(r["images"], 0);
    }
    let c = e.state.db.get_counter(1, &e.state.day()).await.unwrap();
    assert_eq!(
        (c.images, c.legacy_free_images, c.v5, c.anlas),
        (0, 0, 0, 0.0)
    );
    let ok = common::query(
        &e.state.db,
        "SELECT COUNT(*) AS n FROM usage_log WHERE status='ok'",
    )
    .await[0]["n"]
        .as_i64()
        .unwrap();
    assert_eq!(ok, 0);
}

/// 同样的提示词、正常参数（28 步 832x1216）照常出图。
#[tokio::test]
async fn same_prompt_with_normal_params_still_generates() {
    let e = setup(&[]).await;
    let (status, _, body) = call(
        &e.state,
        "POST",
        "/ai/generate-image",
        Some(TOKEN),
        Some(probe_body(
            "nai-diffusion-4-5-full",
            "simple circle, white background",
            28,
            832,
            1216,
        )),
    )
    .await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    assert_eq!(e.mock.requests_to("/ai/generate-image").len(), 1);
    let c = e.state.db.get_counter(1, &e.state.day()).await.unwrap();
    assert_eq!(c.images, 1);
}

/// Key 无效 / 停用时仍然是原来的 401 / 403：测试请求才能如实证明 Key 有效。
#[tokio::test]
async fn invalid_key_still_gets_401_on_a_test_request() {
    let e = setup(&[]).await;
    for auth in [None, Some("nai-wrong")] {
        let (status, _, body) = call(
            &e.state,
            "POST",
            "/ai/generate-image",
            auth,
            Some(probe_body("nai-diffusion-4-5-full", "test", 1, 64, 64)),
        )
        .await;
        assert_eq!(status, 401);
        assert!(!String::from_utf8_lossy(&body).contains("连接正常"));
    }
    e.state
        .db
        .run_script(vec!["UPDATE api_keys SET enabled=0 WHERE id=1".into()])
        .await
        .unwrap();
    let (status, _, body) = call(
        &e.state,
        "POST",
        "/ai/generate-image",
        Some(TOKEN),
        Some(probe_body("nai-diffusion-4-5-full", "test", 1, 64, 64)),
    )
    .await;
    assert!(
        status == 401 || status == 403,
        "{status} {}",
        String::from_utf8_lossy(&body)
    );
    assert!(!String::from_utf8_lossy(&body).contains("连接正常"));
    assert!(e.mock.requests().is_empty());
}

/// 用量日志记请求的 User-Agent 和来源 IP（站长 10-18：筛脚本撞限速 / 转售）：
/// 直连对端是受信任的代理（测试环境信任 127.0.0.1）时，取 X-Forwarded-For 里的真实客户端地址；
/// 直连对端不受信任时，伪造的 X-Forwarded-For 不算，记直连地址。
#[tokio::test]
async fn usage_log_records_user_agent_and_real_client_ip() {
    let e = setup(&[]).await;
    let send = |peer: &'static str, xff: &'static str, ua: &'static str| {
        let state = e.state.clone();
        async move {
            let mut req = Request::builder()
                .method("POST")
                .uri("/ai/generate-image-stream")
                .header("authorization", format!("Bearer {TOKEN}"))
                .header("content-type", "application/json")
                .header("x-forwarded-for", xff)
                .header("user-agent", ua)
                // 不支持的模型：快速被拒（也会写一条拒绝日志）
                .body(Body::from(
                    serde_json::to_vec(&image_body("no-such-model", 1)).unwrap(),
                ))
                .unwrap();
            req.extensions_mut().insert(axum::extract::ConnectInfo(
                format!("{peer}:9").parse::<std::net::SocketAddr>().unwrap(),
            ));
            let resp = gate_server::app(state).oneshot(req).await.unwrap();
            resp.headers()
                .get("x-request-id")
                .unwrap()
                .to_str()
                .unwrap()
                .to_string()
        }
    };
    let rid_proxy = send(
        "127.0.0.1",
        "203.0.113.50, 127.0.0.1",
        "Aaalice_NAI_Launcher (axios/1.18.0)",
    )
    .await;
    let rid_spoof = send("198.51.100.4", "1.2.3.4", "curl/8.0").await;
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let read = |rid: String| {
        let db = e.state.db.clone();
        async move {
            db.write(move |tx| {
                tx.query_row(
                    "SELECT client, src FROM usage_log WHERE rid=?",
                    [rid.clone()],
                    |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
                )
            })
            .await
            .unwrap()
        }
    };
    assert_eq!(
        read(rid_proxy).await,
        (
            "Aaalice_NAI_Launcher (axios/1.18.0)".to_string(),
            "203.0.113.50".to_string()
        )
    );
    assert_eq!(
        read(rid_spoof).await,
        ("curl/8.0".to_string(), "198.51.100.4".to_string()),
        "不受信任的直连对端带来的 X-Forwarded-For 不算数"
    );
}

/// Site-paid V5: once the account's free V5 meter is empty, an account listed in `v5_paid_tokens`
/// pays from its own Anlas (logged, not charged to the member) until its budget runs out.
#[tokio::test]
async fn exhausted_v5_is_paid_by_a_listed_account_within_budget() {
    let e = setup(&[]).await;
    e.state
        .db
        .run_script(vec![
            "UPDATE api_keys SET daily_v5=10, image_model_scope='all' WHERE id=1".into(),
        ])
        .await
        .unwrap();
    e.mock.set_v5_percent(0);
    let token_id = e.state.nai.tokens().await[0].token_id.clone();
    let v5 = || Some(image_body("nai-diffusion-5-full", 1));

    // not listed: refused, nothing sent
    let (status, _, body) = call(&e.state, "POST", "/ai/generate-image", Some(TOKEN), v5()).await;
    let text = String::from_utf8_lossy(&body).to_string();
    assert_eq!(status, 403, "{text}");
    assert!(text.contains("V5 暂停"), "{text}");
    assert!(e.mock.requests_to("/ai/generate-image").is_empty());

    // listed with room for one 30-Anlas image (V5 costs 1.5x)
    let grant = json!({ &token_id: {"until": e.state.now() + 3600.0, "max_anlas": 45} });
    e.state
        .db
        .set_setting(gate_core::paid_v5::KEY, &grant.to_string())
        .await
        .unwrap();
    let (status, _, body) = call(&e.state, "POST", "/ai/generate-image", Some(TOKEN), v5()).await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    let rows = logs(&e.state).await;
    let ok = rows.iter().find(|r| r["status"] == "ok").expect("ok row");
    assert_eq!(ok["anlas"], json!(30.0), "{ok}");
    assert!(
        ok["detail"]
            .as_str()
            .unwrap()
            .contains(gate_core::paid_v5::MARKER),
        "{ok}"
    );
    assert_eq!(
        member_anlas(&e.state).await,
        0.0,
        "the member is not charged"
    );

    // budget left (15) is less than one image: refused again
    let (status, _, _) = call(&e.state, "POST", "/ai/generate-image", Some(TOKEN), v5()).await;
    assert_eq!(status, 403);
    assert_eq!(e.mock.requests_to("/ai/generate-image").len(), 1);
}

/// Two accounts, one with its free V5 meter empty: a V5 image that lands on the empty account is
/// moved to the other one and stays free, and later V5 images skip the empty account for a while.
#[tokio::test]
async fn exhausted_v5_account_hands_the_image_to_one_with_free_v5() {
    let e = setup(&[("NAI_TOKENS", "pst-synthetic-one,pst-synthetic-two")]).await;
    e.state
        .guard
        .with(|g| g.values.insert("interval_jitter".into(), 0));
    allow_v5(&e.state).await;
    let empty = json!({
        "tier": 3, "active": true,
        "usage": {"percent": 0, "isNegative": true, "timeUntilNextPercent": 3600}
    });
    e.mock.push("/user/subscription", Reply::json(200, &empty));
    for _ in 0..2 {
        let (status, body) = v5_post(&e, image_body("nai-diffusion-5-full", 1)).await;
        assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    }
    let subs = e.mock.requests_to("/user/subscription");
    let drained = auth_of(&subs[0]);
    let sent: Vec<String> = e
        .mock
        .requests_to("/ai/generate-image")
        .iter()
        .map(auth_of)
        .collect();
    assert_eq!(sent.len(), 2);
    assert!(sent.iter().all(|a| *a != drained), "{sent:?} vs {drained}");
    let rows = logs(&e.state).await;
    let oks: Vec<&Value> = rows.iter().filter(|r| r["status"] == "ok").collect();
    assert_eq!(oks.len(), 2, "{rows:?}");
    for r in oks {
        assert_eq!(r["anlas"], json!(0.0), "{r}");
        assert!(r["detail"].as_str().unwrap().contains("V5额度+1"), "{r}");
    }
}

/// An exhausted account with a site-paid budget pays for the V5 image itself even when the member
/// may spend Anlas: the member is not charged and the image is not moved to another account.
#[tokio::test]
async fn site_paid_budget_beats_member_anlas_and_switching() {
    let e = setup(&[("NAI_TOKENS", "pst-synthetic-one,pst-synthetic-two")]).await;
    e.state
        .guard
        .with(|g| g.values.insert("interval_jitter".into(), 0));
    allow_v5(&e.state).await;
    e.state
        .db
        .run_script(vec![
            "UPDATE api_keys SET allow_anlas=1, daily_anlas=500 WHERE id=1".into(),
        ])
        .await
        .unwrap();
    let until = e.state.now() + 3600.0;
    let mut grant = serde_json::Map::new();
    for t in e.state.nai.tokens().await {
        grant.insert(
            t.token_id.clone(),
            json!({"until": until, "max_anlas": 1000}),
        );
    }
    e.state
        .db
        .set_setting(gate_core::paid_v5::KEY, &Value::Object(grant).to_string())
        .await
        .unwrap();
    let empty = json!({
        "tier": 3, "active": true,
        "usage": {"percent": 0, "isNegative": true, "timeUntilNextPercent": 3600}
    });
    e.mock.push("/user/subscription", Reply::json(200, &empty));
    let (status, body) = v5_post(&e, image_body("nai-diffusion-5-full", 1)).await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    let drained = auth_of(&e.mock.requests_to("/user/subscription")[0]);
    let sent = e.mock.requests_to("/ai/generate-image");
    assert_eq!(sent.len(), 1);
    assert_eq!(auth_of(&sent[0]), drained, "stays on the paying account");
    let rows = logs(&e.state).await;
    let ok = rows.iter().find(|r| r["status"] == "ok").expect("ok row");
    assert!(
        ok["detail"]
            .as_str()
            .unwrap()
            .contains(gate_core::paid_v5::MARKER),
        "{ok}"
    );
    assert_eq!(
        member_anlas(&e.state).await,
        0.0,
        "the member is not charged"
    );
}

fn token_with_auth(
    tokens: &[std::sync::Arc<gate_upstream::client::Token>],
    auth: &str,
) -> gate_upstream::client::TokenFields {
    tokens
        .iter()
        .find(|t| format!("Bearer {}", t.token) == auth)
        .expect("token behind this authorization")
        .fields()
}

/// Upstream 402 on a free V5 image: the account is marked Anlas-out and free-V5-out, and the image
/// is retried on the other account (nothing was generated or charged), staying free.
#[tokio::test]
async fn upstream_402_on_free_v5_switches_to_other_account_and_marks_out() {
    let e = setup(&[("NAI_TOKENS", "pst-synthetic-one,pst-synthetic-two")]).await;
    e.state
        .guard
        .with(|g| g.values.insert("interval_jitter".into(), 0));
    allow_v5(&e.state).await;
    e.mock.push(
        "/ai/generate-image",
        Reply::json(
            402,
            &json!({"statusCode": 402, "message": "Insufficient Anlas"}),
        ),
    );
    let (status, body) = v5_post(&e, image_body("nai-diffusion-5-full", 1)).await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    let sent: Vec<String> = e
        .mock
        .requests_to("/ai/generate-image")
        .iter()
        .map(auth_of)
        .collect();
    assert_eq!(sent.len(), 2, "{sent:?}");
    assert_ne!(sent[0], sent[1], "retried on the other account");
    let tokens = e.state.nai.tokens().await;
    let now = e.state.now();
    let broke = token_with_auth(&tokens, &sent[0]);
    assert!(broke.anlas_out_until > now, "{broke:?}");
    assert!(broke.v5_out_until > now, "{broke:?}");
    let other = token_with_auth(&tokens, &sent[1]);
    assert!(other.anlas_out_until <= now && other.v5_out_until <= now);
    let rows = logs(&e.state).await;
    let oks: Vec<&Value> = rows.iter().filter(|r| r["status"] == "ok").collect();
    assert_eq!(oks.len(), 1, "{rows:?}");
    assert_eq!(oks[0]["anlas"], json!(0.0), "{}", oks[0]);
    assert!(oks[0]["detail"].as_str().unwrap().contains("V5额度+1"));
    assert_eq!(member_anlas(&e.state).await, 0.0);

    // the stream route does the same (fresh site: here the first account is still marked out)
    let e = setup(&[("NAI_TOKENS", "pst-synthetic-one,pst-synthetic-two")]).await;
    e.state
        .guard
        .with(|g| g.values.insert("interval_jitter".into(), 0));
    allow_v5(&e.state).await;
    e.mock.push(
        "/ai/generate-image-stream",
        Reply::json(
            402,
            &json!({"statusCode": 402, "message": "Insufficient Anlas"}),
        ),
    );
    let (status, _, body) = call(
        &e.state,
        "POST",
        "/ai/generate-image-stream",
        Some(TOKEN),
        Some(image_body("nai-diffusion-5-full", 1)),
    )
    .await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    let sent: Vec<String> = e
        .mock
        .requests_to("/ai/generate-image-stream")
        .iter()
        .map(auth_of)
        .collect();
    assert_eq!(sent.len(), 2, "{sent:?}");
    assert_ne!(sent[0], sent[1]);
}

/// An exhausted account whose last-read Anlas balance cannot cover the image does not pay for it
/// (neither site-paid nor from the member's Anlas): refused on the spot, nothing sent upstream.
#[tokio::test]
async fn paid_v5_refuses_when_account_anlas_is_short() {
    let e = setup(&[]).await;
    allow_v5(&e.state).await;
    e.state
        .db
        .run_script(vec![
            "UPDATE api_keys SET allow_anlas=1, daily_anlas=500 WHERE id=1".into(),
        ])
        .await
        .unwrap();
    let token_id = e.state.nai.tokens().await[0].token_id.clone();
    let grant = json!({ &token_id: {"until": e.state.now() + 3600.0, "max_anlas": 1000} });
    e.state
        .db
        .set_setting(gate_core::paid_v5::KEY, &grant.to_string())
        .await
        .unwrap();
    let short = json!({
        "tier": 3, "active": true,
        "trainingStepsLeft": {"fixedTrainingStepsLeft": 5, "purchasedTrainingSteps": 0},
        "usage": {"percent": 0, "isNegative": true, "timeUntilNextPercent": 3600}
    });
    e.mock.push("/user/subscription", Reply::json(200, &short));
    let (status, body) = v5_post(&e, image_body("nai-diffusion-5-full", 1)).await;
    let text = err_message(&body);
    assert_ne!(status, 200, "{text}");
    assert!(text.contains("V5 暂停"), "{text}");
    assert!(e.mock.requests_to("/ai/generate-image").is_empty());
    assert_eq!(member_anlas(&e.state).await, 0.0);
}

/// Once the only account's free V5 allowance reads empty, free V5 is refused (10-27 起
/// `/public/live` 不再镜像这个状态，拒绝原因只在 429 / 403 原文里)。
#[tokio::test]
async fn free_v5_is_refused_once_upstream_allowance_reads_empty() {
    let e = setup(&[]).await;
    allow_v5(&e.state).await;
    e.mock.set_v5_percent(0);
    let (status, _) = v5_post(&e, image_body("nai-diffusion-5-full", 1)).await;
    assert_eq!(status, 403);
}

/// Raw `director_reference_images` (older third-party clients) reach the upstream in the official
/// cached shape; a JPEG is refused before anything is sent.
#[tokio::test]
async fn raw_precise_references_are_sent_in_the_cached_shape() {
    let e = setup(&[]).await;
    e.state
        .db
        .run_script(vec![
            "UPDATE api_keys SET features='image,vibe,tags' WHERE id=1".into(),
        ])
        .await
        .unwrap();
    let png = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAAB";
    let body = |img: &str| {
        let mut b = image_body("nai-diffusion-4-5-full", 1);
        let p = b["parameters"].as_object_mut().unwrap();
        p.insert("director_reference_images".into(), json!([img]));
        p.insert(
            "director_reference_descriptions".into(),
            json!([{"caption": {"base_caption": "character", "char_captions": []}, "legacy_uc": false}]),
        );
        p.insert(
            "director_reference_information_extracted".into(),
            json!([1]),
        );
        p.insert("director_reference_strength_values".into(), json!([1]));
        p.insert(
            "director_reference_secondary_strength_values".into(),
            json!([0]),
        );
        Some(b)
    };

    let (status, _, resp) = call(
        &e.state,
        "POST",
        "/ai/generate-image",
        Some(TOKEN),
        body("/9j/4AAQSkZJRgABAQAAAQABAAD"),
    )
    .await;
    let text = String::from_utf8_lossy(&resp).to_string();
    assert_eq!(status, 400, "{text}");
    assert!(text.contains("PNG"), "{text}");
    assert!(e.mock.requests_to("/ai/generate-image").is_empty());

    let (status, _, resp) = call(
        &e.state,
        "POST",
        "/ai/generate-image",
        Some(TOKEN),
        body(png),
    )
    .await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&resp));
    let sent = e.mock.requests_to("/ai/generate-image");
    assert_eq!(sent.len(), 1);
    let raw = String::from_utf8_lossy(&sent[0].body).to_string();
    assert!(raw.contains("director_reference_images_cached"), "{raw}");
    assert!(raw.contains("cache_secret_key"), "{raw}");
    assert!(!raw.contains("\"director_reference_images\""), "{raw}");
}

/// Clients such as nai-atelier read `/user/subscription` before every batch image: it must answer for a
/// member key in the official shape, with the member's own numbers, never the upstream account's.
#[tokio::test]
async fn member_subscription_has_the_shape_clients_read() {
    let e = setup(&[]).await;
    let (status, _, body) = call(&e.state, "GET", "/user/subscription", Some(TOKEN), None).await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    let v: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["tier"], json!(3));
    assert_eq!(v["active"], json!(true));
    let steps = &v["trainingStepsLeft"];
    assert!(
        steps["fixedTrainingStepsLeft"]
            .as_i64()
            .is_some_and(|n| n >= 0),
        "{v}"
    );
    assert_eq!(steps["purchasedTrainingSteps"], json!(0));
    assert!(
        e.mock.requests_to("/user/subscription").is_empty(),
        "never asks the upstream"
    );
    let (status, _, _) = call(
        &e.state,
        "GET",
        "/user/subscription",
        Some("nai-not-a-key"),
        None,
    )
    .await;
    assert_eq!(status, 401);
}

/// 站长 10-23：防一人多号 / 共用 Key，请求日志多记几项客户端特征；Authorization / Cookie 原文不落库。
#[tokio::test]
async fn client_hints_are_stored_with_the_usage_row() {
    let e = setup(&[]).await;
    let bid = "0123456789abcdef0123456789abcdef";
    let (status, _, body) = Call::new("POST", "/ai/generate-image")
        .bearer(TOKEN)
        .header("user-agent", "Mozilla/5.0 (Linux; Android 14; wv)")
        .header("x-requested-with", "com.example.tavern")
        .header("accept-language", "zh-CN,zh;q=0.9")
        .header("sec-ch-ua", "\"Chromium\";v=\"130\"")
        .header("sec-ch-ua-platform", "\"Android\"")
        .header("sec-ch-ua-mobile", "?1")
        .header("origin", "https://app.example/ignored-path")
        .header("cookie", format!("owl_bid={bid}; other=1"))
        .json(image_body("nai-diffusion-4-5-full", 1))
        .send(&e.state)
        .await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let rows = common::query(
        &e.state.db,
        "SELECT xrw, lang, chua, hfp, bid, origin FROM usage_log",
    )
    .await;
    assert_eq!(rows.len(), 1);
    let r = &rows[0];
    assert_eq!(r["xrw"], "com.example.tavern");
    assert_eq!(r["lang"], "zh-CN,zh;q=0.9");
    assert_eq!(r["chua"], "\"Chromium\";v=\"130\" | \"Android\" | ?1");
    assert_eq!(r["origin"], "https://app.example");
    let hfp = r["hfp"].as_str().unwrap();
    assert_eq!(hfp.len(), 16);
    let b = r["bid"].as_str().unwrap();
    assert_eq!(b.len(), 16);
    assert_ne!(b, &bid[..16], "只存哈希，不存 Cookie 原值");
    // 整行里找不到 Key 和 Cookie 原文
    let all = common::query(&e.state.db, "SELECT * FROM usage_log").await;
    let dump = serde_json::to_string(&all).unwrap();
    assert!(!dump.contains(TOKEN) && !dump.contains(bid));
}

/// 站长 10-23：参考图（图生图底图 / Vibe / 精确参考）记 sha256 和 dHash，不存图片；参数校验之前就记，被拒的也留证据。
#[tokio::test]
async fn reference_images_are_hashed_even_when_rejected() {
    use base64::Engine;
    let e = setup(&[]).await;
    let img = image::GrayImage::from_fn(64, 64, |x, y| image::Luma([((x * 4) ^ y) as u8]));
    let mut png = std::io::Cursor::new(Vec::new());
    img.write_to(&mut png, image::ImageFormat::Png).unwrap();
    let png = png.into_inner();
    let b64 = base64::engine::general_purpose::STANDARD.encode(&png);
    let mut body = image_body("nai-diffusion-4-5-full", 1);
    body["action"] = json!("img2img");
    body["parameters"]["image"] = json!(b64);
    body["parameters"]["strength"] = json!(0.5);
    body["parameters"]["noise"] = json!(0);
    let (status, _, _) = call(
        &e.state,
        "POST",
        "/ai/generate-image",
        Some(TOKEN),
        Some(body),
    )
    .await;
    assert_eq!(status, 400, "测试 Key 没开图生图");
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let rows = common::query(
        &e.state.db,
        "SELECT key_id, kind, sha256, length(dhash) AS dl, rid!='' AS has_rid FROM ref_images",
    )
    .await;
    use sha2::Digest;
    assert_eq!(
        rows,
        [
            json!({"key_id": 1, "kind": "img2img", "sha256": hex::encode(sha2::Sha256::digest(&png)),
                "dl": 16, "has_rid": 1})
        ]
    );
    let all = serde_json::to_string(&common::query(&e.state.db, "SELECT * FROM ref_images").await)
        .unwrap();
    assert!(!all.contains(&b64[..40]), "不存图片本身");
}
