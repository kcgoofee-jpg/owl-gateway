//! 闲置回收的口径：只有真正出图（`image` / `image_stream`）成功才算「用过」。

use gate_db::Db;
use gate_db::repo::keys::NewKey;
use gate_db::repo::usage::{NewLog, Success};
use serde_json::{Value, json};

async fn last_image_at(db: &Db, kid: i64) -> Value {
    db.get_key(kid).await.unwrap().unwrap()["last_image_at"].clone()
}

#[tokio::test]
async fn only_successful_images_refresh_last_image_at() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(dir.path().join("gate.db")).unwrap();
    let key: NewKey = serde_json::from_value(json!({
        "name": "m", "token": "nai-synthetic-000000000000000000000002", "daily_images": 5,
        "monthly_anlas": 0.0, "daily_text_tokens": 0, "rpm": 5,
    }))
    .unwrap();
    let kid = db.create_key(&key, 1.0).await.unwrap()["id"]
        .as_i64()
        .unwrap();
    let ok = |kind: &str| Success {
        key_id: kid,
        kind: kind.into(),
        day: "2030-03-15".into(),
        images: 1,
        ..Default::default()
    };
    db.record_success(&ok("image"), 1000.0).await.unwrap();
    assert_eq!(last_image_at(&db, kid).await, json!(1000.0));
    // 放大、导演工具、Vibe 编码、标签补全、文本：都不算出图（旧版前三种也算）
    for (i, kind) in [
        "upscale",
        "augment-image",
        "vibe_encode",
        "tags",
        "text",
        "imagex",
    ]
    .into_iter()
    .enumerate()
    {
        db.record_success(&ok(kind), 2000.0 + i as f64)
            .await
            .unwrap();
        assert_eq!(last_image_at(&db, kid).await, json!(1000.0), "{kind}");
    }
    // 被拒 / 失败的出图也不算
    db.add_log(
        &NewLog {
            key_id: Some(kid),
            kind: "image".into(),
            status: "rejected".into(),
            ..Default::default()
        },
        3000.0,
    )
    .await
    .unwrap();
    assert_eq!(last_image_at(&db, kid).await, json!(1000.0));
    // 流式出图算
    db.record_success(&ok("image_stream"), 4000.0)
        .await
        .unwrap();
    assert_eq!(last_image_at(&db, kid).await, json!(4000.0));
}
