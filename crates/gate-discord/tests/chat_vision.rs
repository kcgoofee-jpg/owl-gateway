//! The bot keeps vision input in-band in both reply modes; no real network services.
use axum::{Json, Router, body::Body, extract::State, response::Response, routing::post};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use gate_discord::chat::{Chat, ChatCfg, user_content};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

#[tokio::test]
async fn streaming_and_nonstreaming_requests_contain_decodable_inline_images() {
    let seen = Arc::new(Mutex::new(Vec::<Value>::new()));
    let app=Router::new().route("/v1/chat/completions",post(|State(seen):State<Arc<Mutex<Vec<Value>>>>,Json(body):Json<Value>|async move {
        let streaming=body["stream"]==true;seen.lock().unwrap().push(body);
        if streaming {
            Response::builder().header("content-type","text/event-stream").body(Body::from("data: {\"choices\":[{\"delta\":{\"content\":\"red\"}}]}\n\ndata: [DONE]\n\n")).unwrap()
        } else {
            Response::builder().header("content-type","application/json").body(Body::from(json!({"choices":[{"message":{"content":"red"}}]}).to_string())).unwrap()
        }
    })).with_state(seen.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let chat = Chat::new(ChatCfg {
        base_url: format!("http://{addr}"),
        api_key: "synthetic".into(),
        model: "synthetic-vision".into(),
        stream: false,
    });
    let mut png = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
        4,
        4,
        image::Rgba([255, 0, 0, 255]),
    ))
    .write_to(&mut png, image::ImageFormat::Png)
    .unwrap();
    let user = user_content(
        "synthetic color question",
        &[format!(
            "data:image/png;base64,{}",
            STANDARD.encode(png.into_inner())
        )],
    );
    assert_eq!(
        chat.reply("synthetic", &[], user.clone()).await.unwrap(),
        "red"
    );
    assert_eq!(
        chat.reply_stream("synthetic", &[], user, |_| {})
            .await
            .unwrap(),
        "red"
    );
    let requests = seen.lock().unwrap();
    assert_eq!(requests.len(), 2);
    for request in requests.iter() {
        let url = request["messages"][1]["content"][1]["image_url"]["url"]
            .as_str()
            .unwrap();
        assert!(url.starts_with("data:image/png;base64,"));
        let bytes = STANDARD.decode(url.split_once(',').unwrap().1).unwrap();
        assert_eq!(image::load_from_memory(&bytes).unwrap().width(), 4);
    }
}
