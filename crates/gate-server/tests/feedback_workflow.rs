mod common;

use common::*;
use gate_db::repo::usage::NewLog;
use serde_json::json;

#[tokio::test]
async fn feedback_metadata_accepts_own_and_deleted_key_rids_but_rejects_unknown_or_other_member() {
    use gate_server::feedback::{Metadata, SubmitError, submit, submit_with_metadata};

    let e = setup(&[]).await;
    e.state
        .db
        .run_script(vec![
            "INSERT INTO api_keys(name,token,discord_id,created_at) VALUES \
             ('owner','synthetic-owner-key','owner-id',100), \
             ('other','synthetic-other-key','other-id',100)"
                .into(),
        ])
        .await
        .unwrap();
    let rows = common::query(
        &e.state.db,
        "SELECT id, discord_id FROM api_keys WHERE token LIKE 'synthetic-%-key' ORDER BY id",
    )
    .await;
    let owner = rows[0]["id"].as_i64().unwrap();
    let other = rows[1]["id"].as_i64().unwrap();
    for (key_id, rid) in [(owner, "1a2b3c4d"), (other, "9a8b7c6d")] {
        e.state
            .db
            .add_log(
                &NewLog {
                    key_id: Some(key_id),
                    rid: rid.into(),
                    status: "error".into(),
                    ..NewLog::default()
                },
                200.0,
            )
            .await
            .unwrap();
    }

    // A deleted/replaced Key retains its owner in deleted_key_owners.
    e.state.db.delete_key(owner, 250.0).await.unwrap();
    assert!(
        e.state
            .db
            .feedback_request_belongs_to("owner-id", "1a2b3c4d")
            .await
            .unwrap()
    );

    submit_with_metadata(
        &e.state.db,
        "owner-id",
        "owner",
        None,
        "Please inspect this request",
        Metadata {
            client: " 柏宝绘 ",
            version: " 1.2.3 ",
            request_id: "1A2B3C4D",
        },
        300.0,
    )
    .await
    .unwrap();
    let rows = e.state.db.list_feedback(10).await.unwrap();
    let answers: serde_json::Value =
        serde_json::from_str(rows[0]["answers"].as_str().unwrap()).unwrap();
    assert_eq!(answers["text"], "Please inspect this request");
    assert_eq!(answers["client"], "柏宝绘");
    assert_eq!(answers["version"], "1.2.3");
    assert_eq!(answers["request_id"], "1a2b3c4d");

    let malformed = submit_with_metadata(
        &e.state.db,
        "other-id",
        "other",
        Some(other),
        "Bad request ID format",
        Metadata {
            client: "",
            version: "",
            request_id: "not-a-rid",
        },
        1000.0,
    )
    .await;
    assert!(
        matches!(malformed, Err(SubmitError::Rejected(message)) if message == "请求编号无效，或不属于你的账号。")
    );
    let too_long_client = submit_with_metadata(
        &e.state.db,
        "other-id",
        "other",
        Some(other),
        "Oversized client metadata",
        Metadata {
            client: &"x".repeat(81),
            version: "",
            request_id: "",
        },
        1000.0,
    )
    .await;
    assert!(
        matches!(too_long_client, Err(SubmitError::Rejected(message)) if message.contains("80 字"))
    );

    for rid in ["9a8b7c6d", "00000000"] {
        let result = submit_with_metadata(
            &e.state.db,
            "owner-id",
            "owner",
            None,
            "Could not access this request",
            Metadata {
                client: "",
                version: "",
                request_id: rid,
            },
            1000.0,
        )
        .await;
        assert!(
            matches!(result, Err(SubmitError::Rejected(message)) if message == "请求编号无效，或不属于你的账号。")
        );
    }
    let too_long = submit_with_metadata(
        &e.state.db,
        "other-id",
        "other",
        Some(other),
        "legacy free text remains accepted",
        Metadata::default(),
        1000.0,
    )
    .await;
    assert!(too_long.is_ok());
    // The original Discord text-only API still stores the original shape.
    submit(
        &e.state.db,
        "third-id",
        "third",
        None,
        "Legacy Discord text",
        2000.0,
    )
    .await
    .unwrap();
    let rows = e.state.db.list_feedback(10).await.unwrap();
    let legacy: serde_json::Value =
        serde_json::from_str(rows[0]["answers"].as_str().unwrap()).unwrap();
    assert_eq!(legacy, json!({"text": "Legacy Discord text"}));
}

#[tokio::test]
async fn feedback_review_status_is_persisted_via_admin_session_and_csrf() {
    let e = setup(&[]).await;
    e.state
        .db
        .insert_feedback(
            10.0,
            "synthetic-member",
            "Member",
            None,
            r#"{"text":"A report","client":"智绘姬","version":"2.0","request_id":"aabbccdd"}"#,
        )
        .await
        .unwrap();
    let api = Admin::login(&e.state).await;
    let list = api.get_ok("/admin/api/feedback").await;
    assert_eq!(list["items"][0]["client"], "智绘姬");
    assert_eq!(list["items"][0]["request_id"], "aabbccdd");
    assert_eq!(list["items"][0]["reviewed_at"], serde_json::Value::Null);
    let csrf = list["csrf_token"].as_str().unwrap();
    let id = list["items"][0]["id"].as_i64().unwrap();
    api.fails(
        "PUT",
        &format!("/admin/api/feedback/{id}/review"),
        Some(json!({"reviewed": true})),
        403,
        "会话校验失败，请刷新后台后重试",
    )
    .await;
    let reviewed = api
        .raw(
            "PUT",
            &format!("/admin/api/feedback/{id}/review"),
            Some(json!({"reviewed": true})),
            &[("x-nai-admin-csrf", csrf)],
        )
        .await;
    assert_eq!(reviewed.status, 200, "{}", reviewed.text());
    let list = api.get_ok("/admin/api/feedback").await;
    assert!(list["items"][0]["reviewed_at"].as_f64().is_some());
    assert!(!list["items"][0]["reviewed_by"].as_str().unwrap().is_empty());

    let anonymous = Call::new("PUT", &format!("/admin/api/feedback/{id}/review"))
        .json(json!({"reviewed": false}))
        .send(&e.state)
        .await;
    assert_eq!(anonymous.0.as_u16(), 401);
    let reopened = api
        .raw(
            "PUT",
            &format!("/admin/api/feedback/{id}/review"),
            Some(json!({"reviewed": false})),
            &[("x-nai-admin-csrf", csrf)],
        )
        .await;
    assert_eq!(reopened.status, 200, "{}", reopened.text());
    assert_eq!(
        api.get_ok("/admin/api/feedback").await["items"][0]["reviewed_at"],
        serde_json::Value::Null
    );
}

#[tokio::test]
async fn member_feedback_route_uses_discord_session_and_rejects_non_owned_rids_safely() {
    let e = setup(&[]).await;
    let member = "900000000000000001";
    let cookie_value = gate_server::session::sign_member(
        &e.state,
        member,
        e.state.now() as i64 + 3600,
        &gate_server::session::key_tag(None),
    );
    let cookie = format!("owl_member={cookie_value}");
    e.state
        .db
        .run_script(vec![
            format!(
                "INSERT INTO api_keys(name,token,discord_id,created_at) VALUES ('owner','synthetic-owner-key','{member}',100)"
            ),
        ])
        .await
        .unwrap();
    let key_id = common::query(
        &e.state.db,
        "SELECT id FROM api_keys WHERE token='synthetic-owner-key'",
    )
    .await[0]["id"]
        .as_i64()
        .unwrap();
    e.state
        .db
        .add_log(
            &NewLog {
                key_id: Some(key_id),
                rid: "a1b2c3d4".into(),
                ..NewLog::default()
            },
            100.0,
        )
        .await
        .unwrap();

    let accepted = Call::new("POST", "/member/feedback")
        .header("cookie", cookie.clone())
        .json(json!({"text":"Need help","client":"智绘姬","version":"3.2","request_id":"a1b2c3d4"}))
        .send(&e.state)
        .await;
    assert_eq!(
        accepted.0.as_u16(),
        200,
        "{}",
        String::from_utf8_lossy(&accepted.2)
    );

    for request_id in ["00000000", "ffffffff"] {
        let rejected = Call::new("POST", "/member/feedback")
            .header("cookie", cookie.clone())
            .json(json!({"text":"Need help","request_id":request_id}))
            .send(&e.state)
            .await;
        assert_eq!(rejected.0.as_u16(), 400);
        let body: serde_json::Value = serde_json::from_slice(&rejected.2).unwrap();
        assert_eq!(body["detail"], "请求编号无效，或不属于你的账号。");
    }
    let anonymous = Call::new("POST", "/member/feedback")
        .json(json!({"text":"No session"}))
        .send(&e.state)
        .await;
    assert_eq!(anonymous.0.as_u16(), 401);
}
