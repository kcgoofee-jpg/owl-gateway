//! 流式出图事件的快照：msgpack / SSE 编码、信封、严格 base64、unpackb、事件跟踪器。

mod common;

use common::{Tally, b64, bytes_of, depth_of, tag};
use gate_core::image_events::{
    ImageEventTracker, WireFormat, b64decode_strict, encode_image_event, image_envelope,
};
use gate_core::pymsgpack;
use gate_snapshot::Case;
use gate_snapshot::snapshot::check;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const M: &str = env!("CARGO_MANIFEST_DIR");

/// 线上格式名：只有恰好等于 `"msgpack"` 才走 msgpack，其余（包括拼错的）都走 SSE。
fn wire_of(c: &Case) -> WireFormat {
    if c.get("wire").and_then(Value::as_str).unwrap() == "msgpack" {
        WireFormat::Msgpack
    } else {
        WireFormat::Sse
    }
}

#[test]
fn encode_image_event_snapshot() {
    let mut tally = Tally::default();
    check(M, "image_events/encode", |c| {
        let wire = wire_of(c);
        Some(match c.get("fn")?.as_str()? {
            "encode_image_event" => match encode_image_event(&c.raw["payload"], wire) {
                Ok(bytes) => {
                    tally.add("ok");
                    json!({"$b": b64(&bytes)})
                }
                Err(e) => {
                    tally.add("err");
                    json!({"err": format!("{e:?}")})
                }
            },
            // 大负载只记长度和 SHA-256，不存字节
            "encode_image_event_big" => {
                tally.add("big");
                let size = c.get("size")?.as_u64()? as usize;
                let payload = match c.get("kind")?.as_str()? {
                    "str" => json!({"event_type": "big", "v": "x".repeat(size)}),
                    _ => json!({"event_type": "big", "v": vec![Value::Null; size]}),
                };
                let bytes = encode_image_event(&payload, wire).unwrap();
                json!({"len": bytes.len(), "sha256": hex::encode(Sha256::digest(&bytes))})
            }
            other => panic!("未知函数 {other}"),
        })
    });
    tally.assert_at_least(&[("ok", 30), ("err", 3), ("big", 2)]);
}

#[test]
fn envelope_snapshot() {
    let mut tally = Tally::default();
    check(M, "image_events/envelope", |c| {
        let got = image_envelope(&bytes_of(c.arg(0)));
        tally.add(if got { "true" } else { "false" });
        Some(json!(got))
    });
    tally.assert_at_least(&[("true", 10), ("false", 10)]);
}

#[test]
fn strict_base64_snapshot() {
    let mut tally = Tally::default();
    check(M, "image_events/b64", |c| {
        Some(match b64decode_strict(c.arg(0).as_str()?) {
            Some(bytes) => {
                tally.add("ok");
                json!({"$b": b64(&bytes)})
            }
            None => {
                tally.add("rejected");
                Value::Null
            }
        })
    });
    tally.assert_at_least(&[("ok", 10), ("rejected", 10)]);
}

/// 深度超过这个值的结果只记深度（`tag` 是递归实现，只用于深度不大的结果）。
const TAG_MAX_DEPTH: usize = 64;

#[test]
fn unpackb_snapshot() {
    let mut tally = Tally::default();
    check(M, "image_events/unpackb", |c| {
        Some(match pymsgpack::unpackb(&bytes_of(c.get("raw")?)) {
            Ok(obj) => {
                let depth = depth_of(&obj);
                if depth > TAG_MAX_DEPTH {
                    tally.add("deep");
                    json!({"depth": depth})
                } else {
                    tally.add("ok");
                    tag(&obj)
                }
            }
            Err(e) => {
                tally.add("rejected");
                json!({"err": format!("{e:?}")})
            }
        })
    });
    tally.assert_at_least(&[("ok", 15), ("rejected", 10), ("deep", 1)]);
}

/// 一串 frames / feed / finish：每步记转发的帧、每帧后的已完成张数、是否报错、failed、completed、
/// error_detail 和第一张图。
#[test]
fn tracker_snapshot() {
    let mut tally = Tally::default();
    check(M, "image_events/tracker", |c| {
        let expected = c.get("expected")?.as_i64()?;
        if c.get("fn")?.as_str()? == "tracker_new" {
            tally.add("new");
            return Some(json!({"ok": ImageEventTracker::new(expected, WireFormat::Sse).is_ok()}));
        }
        let max_event = c.get("max_event")?.as_u64()?;
        let max_stream = c.get("max_stream")?.as_u64()?;
        let mut t =
            ImageEventTracker::with_limits(expected, wire_of(c), max_event, max_stream).unwrap();
        let steps: Vec<Value> = c
            .get("ops")?
            .as_array()?
            .iter()
            .map(|op| {
                let (frames, err) = match op["op"].as_str().unwrap() {
                    "frames" => {
                        let r = t.frames(&bytes_of(&op["chunk"]));
                        (r.frames, r.error.is_some())
                    }
                    "feed" => (vec![], t.frames(&bytes_of(&op["chunk"])).error.is_some()),
                    "finish" => {
                        t.finish();
                        (vec![], false)
                    }
                    other => panic!("未知操作 {other}"),
                };
                tally.add_if(!frames.is_empty(), "frames");
                tally.add_if(err, "error");
                tally.add_if(t.failed(), "failed");
                tally.add_if(!t.error_detail().is_empty(), "detail");
                let mut step = serde_json::Map::new();
                step.insert("completed".into(), json!(t.completed_images()));
                step.insert("failed".into(), json!(t.failed()));
                step.insert("error".into(), json!(err));
                step.insert(
                    "frames".into(),
                    Value::Array(
                        frames
                            .iter()
                            .map(|f| json!({"$b": b64(&f.bytes)}))
                            .collect(),
                    ),
                );
                step.insert(
                    "completed_after".into(),
                    Value::Array(frames.iter().map(|f| json!(f.completed_after)).collect()),
                );
                step.insert("detail".into(), json!(t.error_detail()));
                step.insert(
                    "first_image".into(),
                    t.first_image()
                        .map_or(Value::Null, |b| json!({"$b": b64(b)})),
                );
                Value::Object(step)
            })
            .collect();
        tally.add("trace");
        Some(Value::Array(steps))
    });
    tally.assert_at_least(&[
        ("trace", 10),
        ("new", 2),
        ("frames", 10),
        ("error", 5),
        ("failed", 5),
        ("detail", 3),
    ]);
}

#[test]
fn constants() {
    use gate_core::image_events as ev;
    assert_eq!(ev::MAX_EVENT_BYTES, 33_554_432);
    assert_eq!(ev::MAX_STREAM_BYTES, 134_217_728);
    assert_eq!(
        ev::ImageStreamProtocolError.to_string(),
        "NovelAI 返回的图片数据格式错误或过大，未完成的图未扣费。请稍后重试。"
    );
    assert_eq!(WireFormat::Sse.media_type(), "text/event-stream");
    assert_eq!(WireFormat::Msgpack.media_type(), "application/x-msgpack");
}
