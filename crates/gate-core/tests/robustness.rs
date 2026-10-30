//! 健壮性：任意字节喂进解码器 / 解析器都不能 panic；校验结果与切块方式无关（限额足够大时）。
//! （逐条用例在 `*_snapshot.rs`；这里是 proptest 补充。）

mod common;

use common::b64;
use gate_core::image_events::{
    ImageEventTracker, WireFormat, b64decode_strict, encode_image_event, image_envelope,
};
use gate_core::json_body;
use gate_core::sse::encode_sse;
use proptest::prelude::*;
use serde_json::json;

fn png() -> Vec<u8> {
    let mut v = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
    v.extend_from_slice(&[0, 0, 0, 2, 0, 0, 0, 2, 8, 6, 0, 0, 0, 1, 2, 3, 4]);
    v.extend_from_slice(b"\0\0\0\0IEND\xaeB`\x82");
    v
}

/// 一段「事件」：有效 final / 进度 / 错误 / 垃圾。
fn piece(kind: u8, sample: i64, wire: WireFormat, noise: &[u8]) -> Vec<u8> {
    let final_ev = json!({"event_type": "final", "samp_ix": sample, "image": b64(&png())});
    match (kind % 5, wire) {
        (0, _) => encode_image_event(&final_ev, wire).unwrap(),
        (1, _) => encode_image_event(
            &json!({"event_type": "intermediate", "samp_ix": 0, "step_ix": 3}),
            wire,
        )
        .unwrap(),
        (2, _) => encode_image_event(
            &json!({"event_type": "error", "message": "boom  now"}),
            wire,
        )
        .unwrap(),
        (3, WireFormat::Sse) => encode_sse(noise, b""),
        (3, WireFormat::Msgpack) => noise.to_vec(),
        (_, WireFormat::Sse) => b": heartbeat\n\n".to_vec(),
        (_, WireFormat::Msgpack) => encode_image_event(
            &json!({"event_type": "final", "samp_ix": 99, "image": "AAAA"}),
            wire,
        )
        .unwrap(),
    }
}

type Outcome = (Vec<Vec<u8>>, bool, usize, bool, String);

fn run_tracker(wire: WireFormat, stream: &[u8], cuts: &[usize]) -> Outcome {
    let mut t = ImageEventTracker::new(3, wire).unwrap();
    let mut frames = Vec::new();
    let mut errored = false;
    let mut prev = 0;
    for &cut in cuts.iter().chain(std::iter::once(&stream.len())) {
        let cut = cut.clamp(prev, stream.len());
        let out = t.frames(&stream[prev..cut]);
        frames.extend(out.frames.into_iter().map(|f| f.bytes));
        errored |= out.error.is_some();
        prev = cut;
    }
    t.finish();
    (
        frames,
        t.failed(),
        t.completed_images(),
        errored,
        t.error_detail().to_string(),
    )
}

proptest! {
    #[test]
    fn json_parsers_never_panic(raw in proptest::collection::vec(any::<u8>(), 0..300)) {
        let _ = json_body::parse_json(&raw);
        let _ = json_body::parse_json_body(&raw);
    }

    #[test]
    fn json_parser_handles_structured_noise(
        parts in proptest::collection::vec(
            prop_oneof![
                Just("{".to_string()), Just("}".to_string()), Just("[".to_string()), Just("]".to_string()),
                Just(",".to_string()), Just(":".to_string()), Just("\"a\"".to_string()), Just("-0".to_string()),
                Just("1e999".to_string()), Just("NaN".to_string()), Just("\"\\ud800\"".to_string()),
                Just("12345678901234567890123".to_string()), Just(" ".to_string()), Just("null".to_string()),
                Just("\"\\u00e9\"".to_string()), Just("0.5".to_string()), Just("true".to_string()),
            ],
            0..40,
        )
    ) {
        let text = parts.concat();
        let a = serde_json::from_str::<serde_json::Value>(&text);
        let b = json_body::parse_json(text.as_bytes());
        // 能被严格解析的，一定也能被宽松解析
        if b.is_ok() {
            prop_assert!(a.is_ok());
        }
    }

    #[test]
    fn msgpack_and_friends_never_panic(raw in proptest::collection::vec(any::<u8>(), 0..300)) {
        let _ = rmpv::decode::read_value_with_max_depth(&mut std::io::Cursor::new(&raw), 64);
        let _ = image_envelope(&raw);
        let s = String::from_utf8_lossy(&raw);
        let _ = b64decode_strict(&s);
    }

    #[test]
    fn tracker_never_panics_on_garbage(
        chunks in proptest::collection::vec(proptest::collection::vec(any::<u8>(), 0..80), 0..8),
        sse in any::<bool>(),
    ) {
        let wire = if sse { WireFormat::Sse } else { WireFormat::Msgpack };
        let mut t = ImageEventTracker::with_limits(2, wire, 64, 400).unwrap();
        for c in &chunks {
            let _ = t.frames(c);
        }
        t.finish();
        prop_assert!(t.completed_images() <= 2);
    }

    /// 限额足够大时，校验结果（转发的帧、是否失败、完成张数、是否报错、上游原话）与怎么切块无关。
    #[test]
    fn tracker_is_chunking_invariant(
        kinds in proptest::collection::vec((0u8..5, 0i64..4, proptest::collection::vec(any::<u8>(), 0..8)), 0..8),
        sse in any::<bool>(),
        cuts in proptest::collection::vec(0usize..2000, 0..10),
    ) {
        let wire = if sse { WireFormat::Sse } else { WireFormat::Msgpack };
        let stream: Vec<u8> = kinds.iter().flat_map(|(k, s, n)| piece(*k, *s, wire, n)).collect();
        let mut sorted = cuts.clone();
        sorted.sort_unstable();
        let whole = run_tracker(wire, &stream, &[]);
        let split = run_tracker(wire, &stream, &sorted);
        prop_assert_eq!(&whole, &split);
        let all: Vec<usize> = (0..stream.len()).collect();
        prop_assert_eq!(&whole, &run_tracker(wire, &stream, &all));
    }
}

#[test]
fn smea_with_dynamic_sigma_is_refused_before_upstream() {
    // 10-31：线上 7 天 60 次上游 500 里有一部分是 sm + sm_dyn 同时打开（官方直接 500）。
    let both = serde_json::json!({
        "model": "nai-diffusion-4-5-full",
        "action": "generate",
        "parameters": {"sm": true, "sm_dyn": true, "width": 832, "height": 1216}
    });
    let msg = gate_core::policy::upstream_parameter_problem(&both).expect("应当拦下");
    assert!(msg.contains("SMEA"), "{msg}");
    assert!(msg.contains("未扣费"), "{msg}");

    for (sm, dyn_) in [(true, false), (false, true), (false, false)] {
        let ok = serde_json::json!({
            "model": "nai-diffusion-4-5-full",
            "action": "generate",
            "parameters": {"sm": sm, "sm_dyn": dyn_, "width": 832, "height": 1216}
        });
        assert_eq!(gate_core::policy::upstream_parameter_problem(&ok), None);
    }
}
