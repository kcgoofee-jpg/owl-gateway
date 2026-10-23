mod common;

use common::{Tally, b64, bytes_of};
use gate_core::sse::{SseDecoder, SseEvent, encode_sse};
use gate_snapshot::snapshot::check;
use proptest::prelude::*;
use serde_json::{Value, json};

const M: &str = env!("CARGO_MANIFEST_DIR");

fn events_json(events: &[SseEvent]) -> Value {
    Value::Array(
        events
            .iter()
            .map(|e| json!([{"$b": b64(&e.data)}, {"$b": b64(&e.event)}]))
            .collect(),
    )
}

/// 一串 feed / finish 操作：feed 记下解出的事件和是否超限出错，finish 记 `{}`。
#[test]
fn decode_snapshot() {
    let mut tally = Tally::default();
    check(M, "sse/decode", |c| {
        let event_limit = c.get("event_limit").and_then(Value::as_u64)?;
        let stream_limit = c.get("stream_limit").and_then(Value::as_u64)?;
        let mut dec = SseDecoder::new(event_limit, stream_limit);
        let results: Vec<Value> = c
            .get("ops")?
            .as_array()?
            .iter()
            .map(|op| match op["op"].as_str().unwrap() {
                "feed" => {
                    let feed = dec.feed(&bytes_of(&op["chunk"]));
                    tally.add_if(feed.error.is_some(), "error");
                    tally.add_if(!feed.events.is_empty(), "events");
                    json!({"events": events_json(&feed.events), "err": feed.error.is_some()})
                }
                "finish" => {
                    dec.finish();
                    json!({})
                }
                other => panic!("未知操作 {other}"),
            })
            .collect();
        tally.add("trace");
        Some(Value::Array(results))
    });
    // 快照要确实覆盖到「出事件」和「超限报错」两种情形
    tally.assert_at_least(&[("trace", 5), ("error", 3), ("events", 5)]);
}

#[test]
fn encode_snapshot() {
    let mut n = 0;
    check(M, "sse/encode", |c| {
        let data = bytes_of(c.arg(0));
        let event = c.args().get(1).map(bytes_of).unwrap_or_default();
        n += 1;
        Some(json!({"$b": b64(&encode_sse(&data, &event))}))
    });
    assert!(n >= 20, "编码用例太少：{n}");
}

/// 把流按给定切点切开喂进去，收集全部事件与是否出错。
fn run(
    stream: &[u8],
    cuts: &[usize],
    event_limit: u64,
    stream_limit: u64,
) -> (Vec<SseEvent>, bool) {
    let mut dec = SseDecoder::new(event_limit, stream_limit);
    let mut events = Vec::new();
    let mut failed = false;
    let mut prev = 0;
    for &cut in cuts.iter().chain(std::iter::once(&stream.len())) {
        let cut = cut.clamp(prev, stream.len());
        let feed = dec.feed(&stream[prev..cut]);
        events.extend(feed.events);
        failed |= feed.error.is_some();
        prev = cut;
    }
    (events, failed)
}

proptest! {
    /// 限额足够大时，解码结果与怎么切块无关（含半个 UTF-8、被拆开的 \r\n、空块）。
    #[test]
    fn decoding_is_chunking_invariant(
        parts in proptest::collection::vec(
            prop_oneof![
                Just(b"data: x".to_vec()), Just(b"data:y".to_vec()), Just(b"data".to_vec()),
                Just(b"event: e".to_vec()), Just(b": c".to_vec()), Just(b"id: 1".to_vec()),
                Just("data: 猫头鹰😀".as_bytes().to_vec()), Just(Vec::new()),
                proptest::collection::vec(any::<u8>(), 0..6),
            ],
            0..24,
        ),
        terms in proptest::collection::vec(0u8..3, 24),
        bom in any::<bool>(),
        cuts in proptest::collection::vec(0usize..400, 0..12),
    ) {
        let mut stream = if bom { b"\xef\xbb\xbf".to_vec() } else { Vec::new() };
        for (i, p) in parts.iter().enumerate() {
            // 噪声字节里可能自带换行，没关系：整体仍是确定的字节流
            stream.extend_from_slice(p);
            stream.extend_from_slice(match terms[i % terms.len()] { 0 => b"\n", 1 => b"\r\n", _ => b"\r" });
        }
        let mut sorted = cuts.clone();
        sorted.sort_unstable();
        let big = 1 << 40;
        let whole = run(&stream, &[], big, big);
        let split = run(&stream, &sorted, big, big);
        prop_assert_eq!(&whole, &split);
        // 逐字节喂也一样
        let all: Vec<usize> = (0..stream.len()).collect();
        prop_assert_eq!(&whole, &run(&stream, &all, big, big));
        // 流总量超限与怎么切无关
        let tiny = stream.len().saturating_sub(1) as u64;
        if !stream.is_empty() {
            prop_assert!(run(&stream, &sorted, big, tiny).1);
        }
    }
}
