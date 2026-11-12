//! 临时调试记录：只对名单里的成员落提示词和图片，默认关闭。

mod common;
use common::{Admin, Call, SUBJECT, TOKEN, setup};
use gate_server::debug_record::SETTING_KEY;
use serde_json::{Value, json};

fn image_body() -> Value {
    json!({
        "input": "1girl, debug owl", "model": "nai-diffusion-4-5-full", "action": "generate",
        "parameters": {"width": 832, "height": 1216, "steps": 28, "scale": 5, "sampler": "k_euler_ancestral",
                       "n_samples": 1, "seed": 7, "negative_prompt": "lowres"},
    })
}

async fn generate(state: &gate_server::AppState) -> u16 {
    Call::new("POST", "/ai/generate-image")
        .bearer(TOKEN)
        .json(image_body())
        .send(state)
        .await
        .0
        .as_u16()
}

#[tokio::test]
async fn off_by_default_and_does_not_recreate_generation_audit() {
    let e = setup(&[]).await;
    assert_eq!(generate(&e.state).await, 200);
    let data = e.state.db.path().parent().unwrap();
    assert!(!data.join("debug_records").exists());
    assert!(!data.join("audit_images").exists());
    let names = common::query(
        &e.state.db,
        "SELECT name FROM sqlite_master WHERE name='generation_audit'",
    )
    .await;
    assert!(names.is_empty());
}

#[tokio::test]
async fn listed_subject_keeps_prompt_and_output_image() {
    let e = setup(&[]).await;
    e.state
        .db
        .set_setting(SETTING_KEY, &format!(r#"["{SUBJECT}"]"#))
        .await
        .unwrap();
    assert_eq!(generate(&e.state).await, 200);
    let root = e
        .state
        .db
        .path()
        .parent()
        .unwrap()
        .join("debug_records")
        .join(SUBJECT);
    let session = std::fs::read_dir(&root)
        .unwrap()
        .flatten()
        .next()
        .unwrap()
        .path();
    let req: Value =
        serde_json::from_slice(&std::fs::read(session.join("request.json")).unwrap()).unwrap();
    assert_eq!(req["input"], "1girl, debug owl");
    assert_eq!(req["parameters"]["negative_prompt"], "lowres");
    let outs: Vec<_> = std::fs::read_dir(&session)
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("out-"))
        .collect();
    assert_eq!(outs.len(), 1, "{session:?}");
    assert!(
        std::fs::read(outs[0].path())
            .unwrap()
            .starts_with(b"\x89PNG")
    );
}

#[tokio::test]
async fn admin_can_toggle_subjects() {
    let e = setup(&[]).await;
    let admin = Admin::login(&e.state).await;
    let (status, body) = admin
        .json(
            "PUT",
            "/ops/debug-record",
            Some(json!({"subjects": [SUBJECT]})),
        )
        .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["subjects"], json!([SUBJECT]));
    let (status, body) = admin.json("GET", "/ops/debug-record", None).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["subjects"], json!([SUBJECT]));
}
