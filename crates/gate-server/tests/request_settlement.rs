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
