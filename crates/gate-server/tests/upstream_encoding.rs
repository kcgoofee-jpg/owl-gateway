//! 上游响应压缩（br / zstd）时网关照常转发：Chrome 的 Accept-Encoding 带 br、zstd，上游可能用其中任何一种回。
//! 非流式出图的 zip 原样给成员；msgpack 图片流解压后仍是一帧一帧地转，不等上游结束。

mod common;
use axum::body::Body;
use axum::http::Request;
use bytes::Bytes;
use common::{TOKEN, setup};
use gate_testkit::Reply;
use http_body_util::BodyExt;
use serde_json::{Value, json};
use std::time::Duration;
use tower::ServiceExt;

fn image_body(n: i64, stream: Option<&str>) -> Value {
    let mut body = common::image_body();
    body["parameters"]["n_samples"] = json!(n);
    if let Some(s) = stream {
        body["parameters"]["stream"] = json!(s);
    }
    body
}

fn post(path: &str, body: &Value) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(path)
        .header("authorization", format!("Bearer {TOKEN}"))
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(body).unwrap()))
        .unwrap()
}

/// 按 4 字节大端长度前缀切 msgpack 帧，返回每帧的 event_type。
fn events(mut data: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    while data.len() >= 4 {
        let len = u32::from_be_bytes(data[..4].try_into().unwrap()) as usize;
        if data.len() < 4 + len {
            break;
        }
        let v = rmpv::decode::read_value(&mut &data[4..4 + len]).unwrap();
        let kind = v
            .as_map()
            .unwrap()
            .iter()
            .find(|(k, _)| k.as_str() == Some("event_type"))
            .and_then(|(_, v)| v.as_str())
            .unwrap_or("")
            .to_string();
        out.push(kind);
        data = &data[4 + len..];
    }
    out
}

#[tokio::test]
async fn compressed_zip_and_stream_are_forwarded() {
    for enc in ["br", "zstd"] {
        let e = setup(&[]).await;
        e.mock.set_encoding(Some(enc));
        // 非流式：zip 原样给成员
        let resp = gate_server::app(e.state.clone())
            .oneshot(post("/ai/generate-image", &image_body(1, None)))
            .await
            .unwrap();
        assert_eq!(resp.status(), 200, "{enc}");
        assert!(resp.headers().get("content-encoding").is_none(), "{enc}");
        let zip = resp.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(zip, gate_testkit::zip_of_pngs(1), "{enc}");
        // 流式 msgpack：中间图与最终图都完整到达（免费 Key 钳成单张）
        let resp = gate_server::app(e.state.clone())
            .oneshot(post(
                "/ai/generate-image-stream",
                &image_body(1, Some("msgpack")),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), 200, "{enc}");
        let data = resp.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(
            events(&data),
            ["intermediate", "intermediate", "final"],
            "{enc}"
        );
        let encoded = e.mock.encoded_responses();
        assert!(
            encoded
                .iter()
                .any(|(p, x)| p == "/ai/generate-image" && x == enc)
                && encoded
                    .iter()
                    .any(|(p, x)| p == "/ai/generate-image-stream" && x == enc),
            "{enc} {encoded:?}"
        );
    }
}

/// 上游先发一帧中间图、一小时后才发下一块：成员必须先收到这一帧（网关没有整包缓冲）。
#[tokio::test]
async fn compressed_stream_is_relayed_frame_by_frame() {
    let frame = gate_testkit::msgpack_frame(vec![
        ("event_type", "intermediate".into()),
        ("samp_ix", 0.into()),
        ("step_ix", 0.into()),
        ("image", rmpv::Value::Binary(gate_testkit::tiny_png())),
    ]);
    for enc in ["br", "zstd"] {
        let e = setup(&[]).await;
        e.mock.set_encoding(Some(enc));
        e.mock.push(
            "/ai/generate-image-stream",
            Reply::Chunks {
                status: 200,
                content_type: "application/x-msgpack".into(),
                chunks: vec![
                    (Duration::ZERO, Bytes::from(frame.clone())),
                    (Duration::from_secs(3600), Bytes::from_static(b"never")),
                ],
            },
        );
        let resp = gate_server::app(e.state.clone())
            .oneshot(post(
                "/ai/generate-image-stream",
                &image_body(1, Some("msgpack")),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), 200, "{enc}");
        let mut body = resp.into_body();
        let mut got = Vec::new();
        tokio::time::timeout(Duration::from_secs(10), async {
            while events(&got).is_empty() {
                let f = body.frame().await.unwrap().unwrap();
                if let Ok(d) = f.into_data() {
                    got.extend_from_slice(&d);
                }
            }
        })
        .await
        .unwrap_or_else(|_| panic!("{enc}：第一帧没有在上游结束前转给成员"));
        assert_eq!(events(&got), ["intermediate"], "{enc}");
    }
}
