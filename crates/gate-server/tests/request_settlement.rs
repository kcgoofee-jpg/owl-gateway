mod common;
use common::{Call, TOKEN, settle, setup};
use gate_db::repo::usage::LogFilter;
use gate_db::repo::usage::Success;
use gate_testkit::Reply;
use serde_json::{Value, json};

fn body(n: i64) -> Value {
    json!({"input":"cat", "model":"nai-diffusion-4-5-full", "action":"generate", "parameters":{"width":832,"height":1216,"steps":28,"n_samples":n,"scale":5,"sampler":"k_euler_ancestral"}})
}

#[tokio::test]
async fn restart_commits_result_saved_without_replaying_upstream_post() {
    let e = setup(&[]).await;
    let key_id = 1;
    e.state
        .db
        .reserve_request_settlement(
            "saved-only",
            "rid-saved",
            key_id,
            &e.state.day(),
            &e.state.month(),
            false,
            0.0,
            0,
            1,
            e.state.now(),
        )
        .await
        .unwrap();
    e.state
        .db
        .dispatch_request_settlement(
            "saved-only",
            "ABC123",
            "11deadbeef",
            &e.state.day(),
            &e.state.month(),
            false,
            0.0,
            0,
            1,
            e.state.now(),
        )
        .await
        .unwrap();
    let success = Success {
        key_id,
        key_name: "测试成员".into(),
        kind: "image".into(),
        model: "nai-diffusion-4-5-full".into(),
        day: e.state.day(),
        images: 1,
        legacy_free_images: 1,
        rid: "rid-saved".into(),
        ..Default::default()
    };
    let payload = serde_json::to_string(&success).unwrap();
    assert!(
        e.state
            .db
            .save_request_result("saved-only", &payload, e.state.now(), e.state.now())
            .await
            .unwrap()
    );
    let restarted = e.restart().await;
    assert_eq!(e.mock.requests_to("/ai/generate-image").len(), 0);
    assert_eq!(
        restarted
            .db
            .get_counter(key_id, &restarted.day())
            .await
            .unwrap()
            .images,
        1
    );
    let status: String = restarted
        .db
        .read(|c| {
            c.query_row(
                "SELECT status FROM request_settlements WHERE request_id='saved-only'",
                [],
                |r| r.get(0),
            )
        })
        .await
        .unwrap();
    assert_eq!(status, "committed");
}

#[tokio::test]
async fn success_and_known_rejection_release_live_budget_once() {
    let e = setup(&[]).await;
    let (status, _, _) = Call::new("POST", "/ai/generate-image")
        .bearer(TOKEN)
        .json(body(1))
        .send(&e.state)
        .await;
    assert_eq!(status, 200);
    settle().await;
    assert!(e.state.budget.lock().await.reservations.is_empty());
    assert_eq!(e.mock.requests_to("/ai/generate-image").len(), 1);
    let counters = e.state.db.get_counter(1, &e.state.day()).await.unwrap();
    assert_eq!(counters.images, 1);
    let status: String = e
        .state
        .db
        .read(|c| c.query_row("SELECT status FROM request_settlements", [], |r| r.get(0)))
        .await
        .unwrap();
    assert_eq!(status, "committed");

    let e = setup(&[]).await;
    e.mock
        .push("/ai/generate-image", Reply::status(401, "invalid token"));
    let _ = Call::new("POST", "/ai/generate-image")
        .bearer(TOKEN)
        .json(body(1))
        .send(&e.state)
        .await;
    settle().await;
    assert!(e.state.budget.lock().await.reservations.is_empty());
    let status: String = e
        .state
        .db
        .read(|c| c.query_row("SELECT status FROM request_settlements", [], |r| r.get(0)))
        .await
        .unwrap();
    assert_eq!(status, "released");
    assert_eq!(e.mock.requests_to("/ai/generate-image").len(), 1);
}

#[tokio::test]
async fn stream_missing_final_frame_holds_unknown_request_without_replay() {
    let e = setup(&[]).await;
    e.mock.push(
        "/ai/generate-image-stream",
        Reply::Chunks {
            status: 200,
            content_type: "text/event-stream".into(),
            chunks: gate_testkit::image_stream(0, "sse"),
        },
    );
    let (status, _, _) = Call::new("POST", "/ai/generate-image-stream")
        .bearer(TOKEN)
        .json(body(2))
        .send(&e.state)
        .await;
    assert_eq!(status, 200);
    settle().await;
    let logs = e
        .state
        .db
        .list_logs(10, 0, &LogFilter::default())
        .await
        .unwrap();
    assert!(logs.iter().all(|r| r["status"] != "ok"));
    let counters = e.state.db.get_counter(1, &e.state.day()).await.unwrap();
    assert_eq!(counters.images, 0);
    let held = e.state.budget.lock().await.reservations.clone();
    assert_eq!(held.len(), 1, "logs={logs:?}, held={held:?}");
    assert!(
        held.values().next().unwrap().anlas > 0.0
            || held.values().next().unwrap().v5 > 0
            || held.values().next().unwrap().legacy > 0
    );
    let status: String = e
        .state
        .db
        .read(|c| c.query_row("SELECT status FROM request_settlements", [], |r| r.get(0)))
        .await
        .unwrap();
    assert_eq!(status, "uncertain");
    assert_eq!(e.mock.requests_to("/ai/generate-image-stream").len(), 1);
}

/// 账本只留未决行：已终结（committed / released / released_manual）的行过期后清掉，
/// uncertain 永远留着，指向被清行的 usage_request_links 一并删掉。
#[tokio::test]
async fn pruning_keeps_only_unresolved_settlements() {
    let e = setup(&[]).await;
    let now = e.state.now();
    for (n, (id, status)) in [
        ("old-committed", "committed"),
        ("old-released", "released"),
        ("old-manual", "released_manual"),
        ("still-uncertain", "uncertain"),
    ]
    .into_iter()
    .enumerate()
    {
        e.state
            .db
            .reserve_request_settlement(
                id,
                "rid",
                1,
                &e.state.day(),
                &e.state.month(),
                false,
                0.0,
                0,
                1,
                now,
            )
            .await
            .unwrap();
        e.state
            .db
            .write({
                let (id, status) = (id.to_string(), status.to_string());
                move |tx| {
                    tx.execute(
                        "UPDATE request_settlements SET status=?, updated_at=? WHERE request_id=?",
                        rusqlite::params![status, now - 7200.0, id],
                    )?;
                    Ok(())
                }
            })
            .await
            .unwrap();
        e.state
            .db
            .write({
                let id = id.to_string();
                let usage_log_id = n as i64 + 1;
                move |tx| {
                    tx.execute(
                        "INSERT INTO usage_request_links(request_id, usage_log_id) VALUES(?,?)",
                        rusqlite::params![id, usage_log_id],
                    )?;
                    Ok(())
                }
            })
            .await
            .unwrap();
    }
    let deleted = e
        .state
        .db
        .prune_settled_requests(now - gate_db::repo::settlement::SETTLED_KEEP_SECS)
        .await
        .unwrap();
    assert_eq!(deleted, 3);
    let left: Vec<String> = e
        .state
        .db
        .read(|c| {
            let mut s =
                c.prepare("SELECT request_id FROM request_settlements ORDER BY request_id")?;
            s.query_map([], |r| r.get(0))?.collect()
        })
        .await
        .unwrap();
    assert_eq!(left, vec!["still-uncertain"]);
    let links: Vec<String> = e
        .state
        .db
        .read(|c| {
            let mut s = c.prepare("SELECT request_id FROM usage_request_links")?;
            s.query_map([], |r| r.get(0))?.collect()
        })
        .await
        .unwrap();
    assert_eq!(links, vec!["still-uncertain"]);
}

#[tokio::test]
async fn transient_local_accounting_failure_retries_without_resending_upstream() {
    let e = setup(&[]).await;
    e.state.db.run_script(vec!["CREATE TRIGGER reject_retry BEFORE INSERT ON usage_log BEGIN SELECT RAISE(ABORT, 'synthetic failure'); END".into()]).await.unwrap();
    let state = e.state.clone();
    let request = tokio::spawn(async move {
        Call::new("POST", "/ai/generate-image")
            .bearer(TOKEN)
            .json(body(1))
            .send(&state)
            .await
            .0
    });
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            let saved = e.state.db.read(|c| c.query_row("SELECT EXISTS(SELECT 1 FROM request_settlements WHERE status='result_saved')", [], |r| r.get::<_, i64>(0))).await.unwrap();
            if saved == 1 { break; }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    }).await.expect("success payload was not persisted");
    e.state
        .db
        .run_script(vec!["DROP TRIGGER reject_retry".into()])
        .await
        .unwrap();
    assert_eq!(request.await.unwrap().as_u16(), 200);
    settle().await;
    assert_eq!(e.mock.requests_to("/ai/generate-image").len(), 1);
    assert_eq!(
        e.state
            .db
            .get_counter(1, &e.state.day())
            .await
            .unwrap()
            .images,
        1
    );
    assert!(e.state.budget.lock().await.reservations.is_empty());
}

#[tokio::test]
async fn partial_stream_holds_paid_remainder_without_second_free_image() {
    let e = setup(&[]).await;
    common::policy_for(
        &e.state,
        1,
        json!({"paid_parameters": true, "daily_anlas": 5000, "monthly_anlas": 99999}),
    )
    .await;
    common::budgets_for(
        &e.state,
        json!({"daily_anlas": 5000, "monthly_anlas": 99999}),
    )
    .await;
    e.mock.push(
        "/ai/generate-image-stream",
        Reply::Chunks {
            status: 200,
            content_type: "text/event-stream".into(),
            chunks: gate_testkit::image_stream(1, "sse"),
        },
    );
    let (status, _, response) = Call::new("POST", "/ai/generate-image-stream")
        .bearer(TOKEN)
        .json(body(3))
        .send(&e.state)
        .await;
    assert_eq!(status, 200);
    let text = String::from_utf8_lossy(&response);
    assert!(text.contains("执行结果未知"), "{text}");
    settle().await;
    let sent = e.mock.requests_to("/ai/generate-image-stream");
    assert_eq!(sent.len(), 1);
    let full = sent[0].json();
    assert_eq!(full["parameters"]["n_samples"], 3);
    let mut completed = full.clone();
    completed["parameters"]["n_samples"] = json!(1);
    let total = gate_core::policy::estimate_image_cost(&full, true, false)
        .unwrap()
        .anlas;
    let done = gate_core::policy::estimate_image_cost(&completed, true, false)
        .unwrap()
        .anlas;
    let held = e.state.budget.lock().await.reservations.clone();
    assert_eq!(held.len(), 1);
    assert_eq!(held.values().next().unwrap().anlas, (total - done) as f64);
    assert!(held.values().next().unwrap().anlas > 0.0);
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
async fn owner_v5_stream_counts_each_completed_image_toward_v5_quota() {
    let e = setup(&[]).await;
    e.state.db.bind_image_owner(common::SUBJECT).await.unwrap();
    assert!(
        e.state
            .db
            .set_owner_image_enabled(common::SUBJECT, true)
            .await
            .unwrap()
    );
    common::policy_for(
        &e.state,
        1,
        json!({"daily_v5":10,"daily_anlas":5000,"monthly_anlas":99999}),
    )
    .await;
    common::budgets_for(
        &e.state,
        json!({"daily_v5":100,"daily_anlas":5000,"monthly_anlas":99999}),
    )
    .await;
    let mut request_body = body(3);
    request_body["model"] = json!("nai-diffusion-5-full");
    let (status, _, response) = Call::new("POST", "/ai/generate-image-stream")
        .bearer(TOKEN)
        .json(request_body)
        .send(&e.state)
        .await;
    assert_eq!(
        status.as_u16(),
        200,
        "{}",
        String::from_utf8_lossy(&response)
    );
    settle().await;
    let c = e.state.db.get_counter(1, &e.state.day()).await.unwrap();
    assert_eq!(c.images, 3);
    assert_eq!(c.v5, 3);
}

#[tokio::test]
async fn partial_owner_v5_counts_completed_and_holds_only_remaining_v5() {
    let e = setup(&[]).await;
    e.state.db.bind_image_owner(common::SUBJECT).await.unwrap();
    assert!(
        e.state
            .db
            .set_owner_image_enabled(common::SUBJECT, true)
            .await
            .unwrap()
    );
    common::policy_for(
        &e.state,
        1,
        json!({"daily_v5":10,"daily_anlas":5000,"monthly_anlas":99999}),
    )
    .await;
    common::budgets_for(
        &e.state,
        json!({"daily_v5":100,"daily_anlas":5000,"monthly_anlas":99999}),
    )
    .await;
    e.mock.push(
        "/ai/generate-image-stream",
        Reply::Chunks {
            status: 200,
            content_type: "text/event-stream".into(),
            chunks: gate_testkit::image_stream(1, "sse"),
        },
    );
    let mut payload = body(3);
    payload["model"] = json!("nai-diffusion-5-full");
    let (status, _, _) = Call::new("POST", "/ai/generate-image-stream")
        .bearer(TOKEN)
        .json(payload)
        .send(&e.state)
        .await;
    assert_eq!(status.as_u16(), 200);
    settle().await;
    let counter = e.state.db.get_counter(1, &e.state.day()).await.unwrap();
    assert_eq!((counter.images, counter.v5), (1, 1));
    let held = e.state.budget.lock().await.reservations.clone();
    assert_eq!(held.len(), 1);
    assert_eq!(held.values().next().unwrap().v5, 2);
    let restarted = e.restart().await;
    let recovered = restarted.budget.lock().await.reservations.clone();
    assert_eq!(recovered.values().next().unwrap().v5, 2);
    assert_eq!(e.mock.requests_to("/ai/generate-image-stream").len(), 1);
}

#[tokio::test]
async fn partial_stream_keeps_full_reservation_if_completed_image_cannot_be_recorded() {
    let e = setup(&[]).await;
    common::policy_for(
        &e.state,
        1,
        json!({"paid_parameters":true,"daily_anlas":5000,"monthly_anlas":99999}),
    )
    .await;
    common::budgets_for(&e.state, json!({"daily_anlas":5000,"monthly_anlas":99999})).await;
    e.state.db.run_script(vec!["CREATE TRIGGER reject_partial BEFORE INSERT ON usage_log BEGIN SELECT RAISE(ABORT, 'synthetic failure'); END".into()]).await.unwrap();
    e.mock.push(
        "/ai/generate-image-stream",
        Reply::Chunks {
            status: 200,
            content_type: "text/event-stream".into(),
            chunks: gate_testkit::image_stream(1, "sse"),
        },
    );
    let (status, _, _) = Call::new("POST", "/ai/generate-image-stream")
        .bearer(TOKEN)
        .json(body(3))
        .send(&e.state)
        .await;
    assert_eq!(status.as_u16(), 200);
    settle().await;
    let sent = e.mock.requests_to("/ai/generate-image-stream");
    assert_eq!(sent.len(), 1);
    let full = gate_core::policy::estimate_image_cost(&sent[0].json(), true, false).unwrap();
    let held = e.state.budget.lock().await.reservations.clone();
    assert_eq!(held.len(), 1);
    assert_eq!(held.values().next().unwrap().anlas, full.anlas as f64);
    assert_eq!(
        e.state
            .db
            .get_counter(1, &e.state.day())
            .await
            .unwrap()
            .images,
        0
    );
    e.state
        .db
        .run_script(vec!["DROP TRIGGER reject_partial".into()])
        .await
        .unwrap();
    let restarted = e.restart().await;
    assert_eq!(
        restarted
            .db
            .get_counter(1, &restarted.day())
            .await
            .unwrap()
            .images,
        1
    );
    assert_eq!(e.mock.requests_to("/ai/generate-image-stream").len(), 1);
}

#[tokio::test]
async fn upstream_5xx_never_resends_and_keeps_the_result_unknown() {
    let e = setup(&[]).await;
    e.mock.push(
        "/ai/generate-image",
        Reply::status(500, "synthetic upstream error"),
    );
    let (status, _, response) = Call::new("POST", "/ai/generate-image")
        .bearer(TOKEN)
        .json(body(1))
        .send(&e.state)
        .await;
    assert_eq!(status.as_u16(), 500);
    let json: Value = serde_json::from_slice(&response).unwrap();
    assert_eq!(json["error"]["execution_unknown"], true);
    assert_eq!(json["error"]["retryable"], false);
    settle().await;
    assert_eq!(e.mock.requests_to("/ai/generate-image").len(), 1);
    assert_eq!(
        e.state
            .db
            .get_counter(1, &e.state.day())
            .await
            .unwrap()
            .images,
        0
    );
    let held = e.state.budget.lock().await.reservations.clone();
    assert_eq!(held.len(), 1);
    assert_eq!(held.values().next().unwrap().legacy, 1);
    let restarted = e.restart().await;
    assert_eq!(restarted.budget.lock().await.reservations.len(), 1);
    assert_eq!(e.mock.requests_to("/ai/generate-image").len(), 1);
}

#[tokio::test]
async fn streaming_upstream_5xx_is_unknown_and_not_retried() {
    let e = setup(&[]).await;
    e.mock.push(
        "/ai/generate-image-stream",
        Reply::status(500, "synthetic upstream error"),
    );
    let (status, _, response) = Call::new("POST", "/ai/generate-image-stream")
        .bearer(TOKEN)
        .json(body(1))
        .send(&e.state)
        .await;
    assert_eq!(status.as_u16(), 500);
    let v: Value = serde_json::from_slice(&response).unwrap();
    assert_eq!(v["error"]["execution_unknown"], true);
    assert_eq!(v["error"]["retryable"], false);
    assert!(!v["message"].as_str().unwrap().contains("未扣费"));
    settle().await;
    assert_eq!(e.mock.requests_to("/ai/generate-image-stream").len(), 1);
    assert_eq!(e.state.budget.lock().await.reservations.len(), 1);
}

#[tokio::test]
async fn unknown_free_v5_respects_single_upstream_daily_limit() {
    let e = setup(&[]).await;
    common::policy_for(&e.state, 1, json!({"daily_v5":10,"v5":true})).await;
    let id = e.state.nai.tokens().await[0].token_id.clone();
    e.state.db.set_upstream_token_limit(&id, 1).await.unwrap();
    assert!(e.state.nai.set_v5_daily_limit(&id, 1).await);
    e.mock
        .push("/ai/generate-image", Reply::status(500, "synthetic"));
    let mut payload = body(1);
    payload["model"] = json!("nai-diffusion-5-full");
    let first = Call::new("POST", "/ai/generate-image")
        .bearer(TOKEN)
        .json(payload.clone())
        .send(&e.state)
        .await;
    assert_eq!(first.0.as_u16(), 500);
    settle().await;
    let second = Call::new("POST", "/ai/generate-image")
        .bearer(TOKEN)
        .json(payload.clone())
        .send(&e.state)
        .await;
    assert_ne!(
        second.0.as_u16(),
        200,
        "unknown first request must keep upstream allowance reserved"
    );
    assert_eq!(e.mock.requests_to("/ai/generate-image").len(), 1);
    let restarted = e.restart().await;
    let third = Call::new("POST", "/ai/generate-image")
        .bearer(TOKEN)
        .json(payload.clone())
        .send(&restarted)
        .await;
    assert_ne!(
        third.0.as_u16(),
        200,
        "restart must retain the account hold"
    );
    let held = restarted
        .db
        .list_unknown_request_settlements()
        .await
        .unwrap();
    assert_eq!(held.len(), 1);
    assert!(
        restarted
            .db
            .manually_release_unknown_request(
                &held[0].request_id,
                "test",
                "confirmed not executed by synthetic test",
                restarted.now()
            )
            .await
            .unwrap()
    );
    gate_server::admission::release_reservations(&restarted, &held[0].request_id).await;
    let fourth = Call::new("POST", "/ai/generate-image")
        .bearer(TOKEN)
        .json(payload)
        .send(&restarted)
        .await;
    assert_eq!(fourth.0.as_u16(), 200);
    settle().await;
    assert_eq!(
        restarted
            .db
            .upstream_v5_used_and_held(&id, &restarted.day())
            .await
            .unwrap(),
        1
    );
}

#[tokio::test]
async fn free_v5_transfers_from_memory_to_durable_hold_without_double_counting() {
    let e = setup(&[]).await;
    common::policy_for(&e.state, 1, json!({"daily_v5":10,"v5":true})).await;
    let token = e.state.nai.tokens().await.remove(0);
    assert!(e.state.nai.set_v5_daily_limit(&token.token_id, 2).await);
    e.mock.push(
        "/ai/generate-image",
        Reply::Chunks {
            status: 200,
            content_type: "application/zip".into(),
            chunks: vec![(
                std::time::Duration::from_millis(600),
                gate_testkit::zip_of_pngs(1).into(),
            )],
        },
    );
    let state = e.state.clone();
    let mut payload = body(1);
    payload["model"] = json!("nai-diffusion-5-full");
    let task = tokio::spawn(async move {
        Call::new("POST", "/ai/generate-image")
            .bearer(TOKEN)
            .json(payload)
            .send(&state)
            .await
            .0
    });
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            if !e.mock.requests_to("/ai/generate-image").is_empty() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(token.fields().pending_v5, 0);
    assert_eq!(
        e.state
            .db
            .upstream_v5_used_and_held(&token.token_id, &e.state.day())
            .await
            .unwrap(),
        1
    );
    assert_eq!(task.await.unwrap().as_u16(), 200);
    settle().await;
    assert_eq!(
        e.state
            .db
            .get_upstream_counter(&token.token_id, &e.state.day())
            .await
            .unwrap()
            .v5,
        1
    );
    assert_eq!(
        e.state
            .db
            .upstream_v5_used_and_held(&token.token_id, &e.state.day())
            .await
            .unwrap(),
        1
    );
}

#[tokio::test]
async fn free_v5_known_rejection_releases_the_account_hold() {
    let e = setup(&[]).await;
    common::policy_for(&e.state, 1, json!({"daily_v5":10,"v5":true})).await;
    let id = e.state.nai.tokens().await[0].token_id.clone();
    assert!(e.state.nai.set_v5_daily_limit(&id, 1).await);
    e.mock.push(
        "/ai/generate-image",
        Reply::status(400, "synthetic rejection"),
    );
    let mut payload = body(1);
    payload["model"] = json!("nai-diffusion-5-full");
    let first = Call::new("POST", "/ai/generate-image")
        .bearer(TOKEN)
        .json(payload.clone())
        .send(&e.state)
        .await;
    assert_eq!(first.0.as_u16(), 400);
    settle().await;
    assert_eq!(
        e.state
            .db
            .upstream_v5_used_and_held(&id, &e.state.day())
            .await
            .unwrap(),
        0
    );
    let second = Call::new("POST", "/ai/generate-image")
        .bearer(TOKEN)
        .json(payload)
        .send(&e.state)
        .await;
    assert_eq!(second.0.as_u16(), 200);
    settle().await;
    assert_eq!(
        e.state
            .db
            .get_upstream_counter(&id, &e.state.day())
            .await
            .unwrap()
            .v5,
        1
    );
    assert_eq!(
        e.state
            .db
            .upstream_v5_used_and_held(&id, &e.state.day())
            .await
            .unwrap(),
        1
    );
}

#[tokio::test]
async fn free_v5_is_not_sent_if_durable_account_hold_cannot_be_written() {
    let e = setup(&[]).await;
    common::policy_for(&e.state, 1, json!({"daily_v5":10,"v5":true})).await;
    e.state.db.run_script(vec!["CREATE TRIGGER reject_v5_hold BEFORE INSERT ON upstream_v5_reservations BEGIN SELECT RAISE(ABORT, 'synthetic write failure'); END".into()]).await.unwrap();
    let mut payload = body(1);
    payload["model"] = json!("nai-diffusion-5-full");
    let (status, _, _) = Call::new("POST", "/ai/generate-image")
        .bearer(TOKEN)
        .json(payload)
        .send(&e.state)
        .await;
    assert_eq!(status.as_u16(), 500);
    settle().await;
    assert!(e.mock.requests_to("/ai/generate-image").is_empty());
    assert!(e.state.budget.lock().await.reservations.is_empty());
}

#[tokio::test]
async fn mixed_v5_batch_must_not_use_disabled_free_upstream_quota() {
    let e = setup(&[]).await;
    common::policy_for(&e.state, 1, json!({"paid_parameters":true,"daily_v5":10,"daily_anlas":5000,"monthly_anlas":99999,"v5":true})).await;
    common::budgets_for(
        &e.state,
        json!({"daily_v5":100,"daily_anlas":5000,"monthly_anlas":99999}),
    )
    .await;
    let token = e.state.nai.tokens().await.remove(0);
    assert!(e.state.nai.set_v5_daily_limit(&token.token_id, 0).await);
    let mut payload = body(3);
    payload["model"] = json!("nai-diffusion-5-full");
    let (status, _, _) = Call::new("POST", "/ai/generate-image")
        .bearer(TOKEN)
        .json(payload)
        .send(&e.state)
        .await;
    settle().await;
    assert_eq!(
        status.as_u16(),
        200,
        "paid fallback should remain available"
    );
    let sent = e.mock.requests_to("/ai/generate-image");
    assert_eq!(sent.len(), 1);
    let full_cost = gate_core::policy::estimate_image_cost(&sent[0].json(), true, false)
        .unwrap()
        .anlas;
    let counter = e.state.db.get_counter(1, &e.state.day()).await.unwrap();
    assert_eq!(counter.anlas, full_cost as f64);
    assert_eq!(
        e.state
            .db
            .upstream_v5_used_and_held(&token.token_id, &e.state.day())
            .await
            .unwrap(),
        0
    );
    let mut stream_body = body(3);
    stream_body["model"] = json!("nai-diffusion-5-full");
    let (stream_status, _, _) = Call::new("POST", "/ai/generate-image-stream")
        .bearer(TOKEN)
        .json(stream_body)
        .send(&e.state)
        .await;
    assert_eq!(stream_status.as_u16(), 200);
    settle().await;
    let stream_sent = e.mock.requests_to("/ai/generate-image-stream");
    assert_eq!(stream_sent.len(), 1);
    let stream_cost = gate_core::policy::estimate_image_cost(&stream_sent[0].json(), true, false)
        .unwrap()
        .anlas;
    assert_eq!(
        e.state
            .db
            .get_counter(1, &e.state.day())
            .await
            .unwrap()
            .anlas,
        (full_cost + stream_cost) as f64
    );
    assert_eq!(
        e.state
            .db
            .upstream_v5_used_and_held(&token.token_id, &e.state.day())
            .await
            .unwrap(),
        0
    );
}
