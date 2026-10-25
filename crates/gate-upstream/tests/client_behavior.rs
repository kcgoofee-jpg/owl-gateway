//! 上游客户端行为测试（对假上游）：对照 app/nai.py 的分支逐条验证。

mod common;

use bytes::Bytes;
use common::TestHooks;
use gate_testkit::{MockUpstream, Reply};
use gate_upstream::client::{Callbacks, Config, Hooks, NaiClient, NoObserver, RequestOpts};
use serde_json::json;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// `pace` = 出完后歇不歇（歇几秒取自账号保护 `Hooks::post_jitter`）；false = 测试环境。
fn config(url: &str, pace: bool) -> Config {
    Config {
        image_host: url.into(),
        proxy: None,
        pace,
    }
}

const T1: &str = "pst-aaaaaaaaaaaaaaaaaaaa";
const T2: &str = "pst-bbbbbbbbbbbbbbbbbbbb";

fn image_body(n: i64) -> serde_json::Value {
    json!({"input": "1girl", "model": "nai-diffusion-4-5-full", "action": "generate",
           "parameters": {"width": 832, "height": 1216, "steps": 28, "n_samples": n, "scale": 5.0}})
}

fn image_opts<'a>(url: &str, body: &'a serde_json::Value, n: usize) -> RequestOpts<'a> {
    let mut o = RequestOpts::post(format!("{url}/ai/generate-image"), body);
    o.image_lane = true;
    o.image_count = n;
    o.max_response_bytes = Some(64 << 20);
    o
}

#[tokio::test]
async fn image_success_counts_and_body_is_python_json() {
    let up = MockUpstream::start().await;
    let hooks = TestHooks::default();
    let c = NaiClient::new(
        config(&up.url(), false),
        hooks.clone(),
        vec![(T1.into(), 0, false)],
    )
    .unwrap();
    let body = image_body(2);
    let resp = c
        .request(
            image_opts(&up.url(), &body, 2),
            &Callbacks::default(),
            None,
            &NoObserver,
        )
        .await
        .unwrap();
    assert_eq!(resp.status, 200);
    assert!(resp.body.starts_with(b"PK"));
    let req = &up.requests_to("/ai/generate-image")[0];
    // 发出的正文与 Python json.dumps(separators=(",", ":"), ensure_ascii=False) 逐字节一致
    assert_eq!(req.body.as_ref(), gate_core::pyjson::wire(&body).as_slice());
    assert_eq!(
        req.header("authorization"),
        Some(format!("Bearer {T1}").as_str())
    );
    assert_eq!(req.header("content-type"), Some("application/json"));
    // 结算：张数记到上游计数
    let tid = gate_upstream::headers::token_id(T1);
    assert_eq!(hooks.0.images.lock().unwrap().get(&tid), Some(&2));
}

#[tokio::test]
async fn failed_durable_dispatch_callback_prevents_upstream_post() {
    let up = MockUpstream::start().await;
    let c = NaiClient::new(
        config(&up.url(), false),
        TestHooks::default(),
        vec![(T1.into(), 0, false)],
    )
    .unwrap();
    let cb = Callbacks {
        on_dispatch: Some(Box::new(|| {
            Box::pin(async {
                Err(gate_upstream::client::UpstreamError::local(
                    500,
                    "persistence failed",
                    "",
                ))
            })
        })),
        ..Default::default()
    };
    let err = c
        .request(
            image_opts(&up.url(), &image_body(1), 1),
            &cb,
            None,
            &NoObserver,
        )
        .await
        .unwrap_err();
    assert_eq!(err.message, "persistence failed");
    assert!(up.requests_to("/ai/generate-image").is_empty());
}

/// 账号的「今天出了几张」（单号每日上限按它算）只记上游真的出成的图：上游 5xx、429、并发锁、
/// 发出前被拒、返回的图不完整都不算；并发锁后重试出成的那张只算一次。
#[tokio::test]
async fn only_produced_images_count_toward_the_account_today() {
    let up = MockUpstream::start().await;
    let hooks = TestHooks::default();
    let c = NaiClient::new(
        config(&up.url(), false),
        hooks.clone(),
        vec![(T1.into(), 0, false)],
    )
    .unwrap();
    let tid = gate_upstream::headers::token_id(T1);
    let today = || {
        hooks
            .0
            .images
            .lock()
            .unwrap()
            .get(&tid)
            .copied()
            .unwrap_or(0)
    };
    let body = image_body(1);
    let send = |cb: Callbacks<'static>| {
        let (c, url, body) = (c.clone(), up.url(), body.clone());
        async move {
            c.request(image_opts(&url, &body, 1), &cb, None, &NoObserver)
                .await
        }
    };
    // 上游 500：没出图
    up.push("/ai/generate-image", Reply::status(500, "boom"));
    let _ = send(Callbacks::default()).await;
    // 返回 200 但不是图：校验不过，按没出图
    up.push("/ai/generate-image", Reply::status(200, "not a zip"));
    let _ = send(Callbacks::default()).await;
    // 发出前被网关拒绝
    let refuse = Callbacks {
        before_dispatch: Some(Box::new(|| {
            Box::pin(async {
                Err(gate_upstream::client::UpstreamError::new(503, "全站冷却中"))
            })
        })),
        ..Default::default()
    };
    let _ = send(refuse).await;
    assert_eq!(today(), 0, "被拒 / 出错都不算");
    // 并发锁一次、重试出成：只算出成的那一张
    up.push("/ai/generate-image", Reply::concurrency_locked(429));
    assert_eq!(send(Callbacks::default()).await.unwrap().status, 200);
    assert_eq!(today(), 1);
    // 上游 429（限流）：不算
    up.push("/ai/generate-image", Reply::rate_limited("1"));
    let _ = send(Callbacks::default()).await;
    assert_eq!(today(), 1);
}

#[tokio::test]
async fn image_429_cools_token_and_calls_back() {
    let up = MockUpstream::start().await;
    up.push("/ai/generate-image", Reply::rate_limited("120"));
    let c = NaiClient::new(
        config(&up.url(), false),
        TestHooks::default(),
        vec![(T1.into(), 0, false)],
    )
    .unwrap();
    let seen = Arc::new(Mutex::new(None));
    let s2 = seen.clone();
    let cb = Callbacks {
        on_rate_limited: Some(Box::new(move |ra| {
            *s2.lock().unwrap() = Some(ra);
            Box::pin(async {})
        })),
        ..Default::default()
    };
    let body = image_body(1);
    let err = c
        .request(image_opts(&up.url(), &body, 1), &cb, None, &NoObserver)
        .await
        .unwrap_err();
    assert_eq!(
        (err.status, err.message.as_str()),
        (
            429,
            "NovelAI 正在限流，全站出图暂停冷却，未扣费。请稍后重试。"
        )
    );
    assert_eq!(*seen.lock().unwrap(), Some(120.0));
    // 冷却中：没有可用 Token，给出「冷却中、不是额度用完」的说明
    let err = c
        .request(
            image_opts(&up.url(), &body, 1),
            &Callbacks::default(),
            None,
            &NoObserver,
        )
        .await
        .unwrap_err();
    assert_eq!(err.status, 503);
    assert!(
        err.message.starts_with("本站 NovelAI 账号正在限流冷却"),
        "{}",
        err.message
    );
}

#[tokio::test]
async fn non_image_429_switches_token_once() {
    let up = MockUpstream::start().await;
    up.push("/ai/generate-image", Reply::rate_limited("30"));
    let c = NaiClient::new(
        config(&up.url(), false),
        TestHooks::default(),
        vec![(T1.into(), 0, false), (T2.into(), 0, false)],
    )
    .unwrap();
    let body = image_body(1);
    let opts = RequestOpts::post(format!("{}/ai/generate-image", up.url()), &body);
    let resp = c
        .request(opts, &Callbacks::default(), None, &NoObserver)
        .await
        .unwrap();
    assert_eq!(resp.status, 200);
    let reqs = up.requests_to("/ai/generate-image");
    assert_eq!(reqs.len(), 2);
    assert_ne!(
        reqs[0].header("authorization"),
        reqs[1].header("authorization")
    );
}

#[tokio::test]
async fn forbidden_model_is_400_and_account_403_trips_after_three() {
    let up = MockUpstream::start().await;
    let hooks = TestHooks::default();
    let c = NaiClient::new(
        config(&up.url(), false),
        hooks.clone(),
        vec![(T1.into(), 0, false)],
    )
    .unwrap();
    let body = image_body(1);
    up.push("/ai/generate-image", Reply::model_403("nai-glm-4-6"));
    let e = c
        .request(
            image_opts(&up.url(), &body, 1),
            &Callbacks::default(),
            None,
            &NoObserver,
        )
        .await
        .unwrap_err();
    assert_eq!(
        (e.status, e.message.as_str()),
        (
            400,
            "NovelAI 不支持该模型，未扣费。请改用本站支持的模型，如 nai-diffusion-4-5-full。"
        )
    );
    for _ in 0..3 {
        up.push("/ai/generate-image", Reply::account_403());
        let e = c
            .request(
                image_opts(&up.url(), &body, 1),
                &Callbacks::default(),
                None,
                &NoObserver,
            )
            .await
            .unwrap_err();
        assert_eq!(e.status, 403);
    }
    assert!(
        hooks
            .0
            .events
            .lock()
            .unwrap()
            .iter()
            .any(|k| k == "upstream_403_cooldown")
    );
    let f = c.tokens().await[0].fields();
    assert_eq!((f.forbidden_trips, f.forbidden_streak), (1, 0));
    assert!(f.blocked_until > hooks.now() + 299.0, "第一轮冷却 5 分钟");
}

#[tokio::test]
async fn unauthorized_disables_token() {
    let up = MockUpstream::start().await;
    up.push("/ai/generate-image", Reply::status(401, "nope"));
    let hooks = TestHooks::default();
    let c = NaiClient::new(
        config(&up.url(), false),
        hooks.clone(),
        vec![(T1.into(), 0, false)],
    )
    .unwrap();
    let body = image_body(1);
    let e = c
        .request(
            image_opts(&up.url(), &body, 1),
            &Callbacks::default(),
            None,
            &NoObserver,
        )
        .await
        .unwrap_err();
    assert_eq!(
        (e.status, e.message.as_str()),
        (502, "本站 NovelAI 账号登录失效，未扣费。请稍后重试。")
    );
    assert!(c.tokens().await[0].fields().disabled);
    assert!(
        hooks
            .0
            .events
            .lock()
            .unwrap()
            .iter()
            .any(|k| k == "upstream_401")
    );
}

/// 出图节奏只有一个旋钮（站长 10-23）：每张出完后这个账号歇 [最少, 最多] 秒（账号保护 `Hooks::post_jitter`）。
#[tokio::test]
async fn each_image_is_followed_by_a_rest() {
    let up = MockUpstream::start().await;
    let hooks = TestHooks::default();
    *hooks.0.post.lock().unwrap() = (0.4, 0.4);
    let c = NaiClient::new(config(&up.url(), true), hooks, vec![(T1.into(), 0, false)]).unwrap();
    let body = image_body(1);
    let t0 = Instant::now();
    for _ in 0..3 {
        c.request(
            image_opts(&up.url(), &body, 1),
            &Callbacks::default(),
            None,
            &NoObserver,
        )
        .await
        .unwrap();
    }
    assert!(
        t0.elapsed() >= Duration::from_millis(800),
        "三张图之间歇了两次：{:?}",
        t0.elapsed()
    );
}

/// 歇是从上一张**出完**算起（不是从开始算）。第一张要 0.6 秒才出完：
/// - 歇 1.8 秒：第二张最早 0.6 + 1.8 = 2.4 秒；
/// - 歇 1 秒（下限）：0.6 + 1 = 1.6 秒；
/// - 测试环境不歇：0.6 秒出完就接着发。
#[tokio::test]
async fn rest_counts_from_the_end_of_the_previous_image() {
    for (pace, post, lo, hi) in [
        (true, (1.8, 1.8), 2350, 3000),
        (true, (1.0, 1.0), 1550, 2000),
        (false, (1.0, 3.0), 550, 1000),
    ] {
        let up = MockUpstream::start().await;
        up.push(
            "/ai/generate-image",
            Reply::Chunks {
                status: 200,
                content_type: "binary/octet-stream".into(),
                chunks: vec![(
                    Duration::from_millis(600),
                    Bytes::from(gate_testkit::zip_of_pngs(1)),
                )],
            },
        );
        let hooks = TestHooks::default();
        *hooks.0.post.lock().unwrap() = post;
        let c =
            NaiClient::new(config(&up.url(), pace), hooks, vec![(T1.into(), 0, false)]).unwrap();
        let body = image_body(1);
        let t0 = Instant::now();
        for _ in 0..2 {
            c.request(
                image_opts(&up.url(), &body, 1),
                &Callbacks::default(),
                None,
                &NoObserver,
            )
            .await
            .unwrap();
        }
        let elapsed = t0.elapsed();
        assert!(
            elapsed >= Duration::from_millis(lo) && elapsed < Duration::from_millis(hi),
            "歇 {post:?}（pace={pace}）：{elapsed:?}"
        );
    }
}

#[tokio::test]
async fn guard_block_reason_is_429() {
    let up = MockUpstream::start().await;
    let hooks = TestHooks::default();
    *hooks.0.block.lock().unwrap() = Some(("本小时已到上限".into(), "hourly_cap".into()));
    let c = NaiClient::new(config(&up.url(), false), hooks, vec![(T1.into(), 0, false)]).unwrap();
    let body = image_body(1);
    let e = c
        .request(
            image_opts(&up.url(), &body, 1),
            &Callbacks::default(),
            None,
            &NoObserver,
        )
        .await
        .unwrap_err();
    assert_eq!(
        (e.status, e.message.as_str(), e.code.as_str()),
        (429, "本小时已到上限", "hourly_cap")
    );
    assert!(up.requests().is_empty(), "被账号保护挡住时不发上游");
}

/// 上游对某个号回 402（Anlas 不足）后，之后要花 Anlas 的图一段时间内都派给别的号；
/// 402 本身照原样回给调用方，不重试（不是免费 V5）。
#[tokio::test]
async fn requires_anlas_skips_account_after_402() {
    let up = MockUpstream::start().await;
    up.push(
        "/ai/generate-image",
        Reply::json(
            402,
            &json!({"statusCode": 402, "message": "Insufficient Anlas"}),
        ),
    );
    let c = NaiClient::new(
        config(&up.url(), false),
        TestHooks::default(),
        vec![(T1.into(), 0, true), (T2.into(), 0, true)],
    )
    .unwrap();
    let body = image_body(1);
    let paid = |url: &str| {
        let mut o = image_opts(url, &body, 1);
        o.requires_anlas = true;
        o
    };
    let resp = c
        .request(paid(&up.url()), &Callbacks::default(), None, &NoObserver)
        .await
        .unwrap();
    assert_eq!(resp.status, 402);
    let reqs = up.requests_to("/ai/generate-image");
    assert_eq!(reqs.len(), 1, "402 不自动重试");
    let broke = reqs[0].header("authorization").unwrap().to_string();
    let out: Vec<_> = c
        .tokens()
        .await
        .iter()
        .map(|t| t.fields().anlas_out_until > 0.0)
        .collect();
    assert_eq!(out.iter().filter(|o| **o).count(), 1, "{out:?}");
    // 轮询本来会轮回到 402 的号：现在两张都派给另一个
    for _ in 0..2 {
        let resp = c
            .request(paid(&up.url()), &Callbacks::default(), None, &NoObserver)
            .await
            .unwrap();
        assert_eq!(resp.status, 200);
    }
    let reqs = up.requests_to("/ai/generate-image");
    assert_eq!(reqs.len(), 3);
    assert!(
        reqs[1..]
            .iter()
            .all(|r| r.header("authorization") != Some(broke.as_str())),
        "要花 Anlas 的图不再派给刚回 402 的号"
    );
    // 免费的图不受影响：照常轮询到它
    for _ in 0..2 {
        c.request(
            image_opts(&up.url(), &body, 1),
            &Callbacks::default(),
            None,
            &NoObserver,
        )
        .await
        .unwrap();
    }
    let reqs = up.requests_to("/ai/generate-image");
    assert!(
        reqs[3..]
            .iter()
            .any(|r| r.header("authorization") == Some(broke.as_str()))
    );
}

#[tokio::test]
async fn v5_reservation_released_when_cancelled() {
    let up = MockUpstream::start().await;
    up.push("/ai/generate-image", Reply::Hang);
    let hooks = TestHooks::default();
    let c = NaiClient::new(
        config(&up.url(), false),
        hooks.clone(),
        vec![(T1.into(), 5, false)],
    )
    .unwrap();
    let body = image_body(1);
    let c2 = c.clone();
    let url = up.url();
    let b2 = body.clone();
    let task = tokio::spawn(async move {
        let mut o = image_opts(&url, &b2, 1);
        o.v5_free = true;
        c2.request(o, &Callbacks::default(), None, &NoObserver)
            .await
    });
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(c.tokens().await[0].fields().pending_v5, 1);
    task.abort();
    let _ = task.await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    let t = &c.tokens().await[0];
    assert_eq!(t.fields().pending_v5, 0, "取消后 V5 预留必须归还");
    assert_eq!(t.image_slots.active(), 0, "取消后并发槽必须归还");
    // 之后的请求照常
    let mut o = image_opts(&up.url(), &body, 1);
    o.v5_free = true;
    c.request(o, &Callbacks::default(), None, &NoObserver)
        .await
        .unwrap();
    let tid = gate_upstream::headers::token_id(T1);
    assert_eq!(hooks.0.v5.lock().unwrap().get(&tid), Some(&1));
}

#[tokio::test]
async fn image_stream_settles_completed_images() {
    let up = MockUpstream::start().await;
    let hooks = TestHooks::default();
    let c = NaiClient::new(
        config(&up.url(), false),
        hooks.clone(),
        vec![(T1.into(), 0, false)],
    )
    .unwrap();
    let mut body = image_body(2);
    body["parameters"]["stream"] = json!("msgpack");
    let mut s = c
        .image_stream(
            &format!("{}/ai/generate-image-stream", up.url()),
            &body,
            2,
            false,
            false,
            90.0,
            &Callbacks::default(),
            None,
            &NoObserver,
        )
        .await
        .unwrap();
    use futures::StreamExt;
    let mut bytes = 0;
    let mut stream = Box::pin(s.take_body().unwrap());
    while let Some(chunk) = stream.next().await {
        bytes += chunk.unwrap().len();
    }
    assert!(bytes > 0);
    let req = &up.requests_to("/ai/generate-image-stream")[0];
    assert_eq!(req.header("accept"), Some("application/x-msgpack"));
    s.completed_images = 2;
    s.finish().await;
    let tid = gate_upstream::headers::token_id(T1);
    assert_eq!(hooks.0.images.lock().unwrap().get(&tid), Some(&2));
}

// ---------------------------------------------------------------- 响应压缩

/// Chrome 的 Accept-Encoding 声明了这四种：上游用哪一种回，网关都要解开。
const ENCODINGS: [&str; 4] = ["gzip", "deflate", "br", "zstd"];

#[tokio::test]
async fn compressed_responses_are_decoded() {
    for enc in ENCODINGS {
        let up = MockUpstream::start().await;
        up.set_encoding(Some(enc));
        let c = NaiClient::new(
            config(&up.url(), false),
            TestHooks::default(),
            vec![(T1.into(), 0, false)],
        )
        .unwrap();
        // 出图 zip（读正文有上限的分支）
        let body = image_body(2);
        let resp = c
            .request(
                image_opts(&up.url(), &body, 2),
                &Callbacks::default(),
                None,
                &NoObserver,
            )
            .await
            .unwrap();
        assert_eq!(resp.body, gate_testkit::zip_of_pngs(2), "{enc}");
        assert!(resp.headers.get("content-encoding").is_none(), "{enc}");
        // Vibe 编码（不设上限的分支）
        let vibe = json!({"image": "AAAA", "model": "nai-diffusion-4-5-full"});
        let resp = c
            .request(
                RequestOpts::post(format!("{}/ai/encode-vibe", up.url()), &vibe),
                &Callbacks::default(),
                None,
                &NoObserver,
            )
            .await
            .unwrap();
        assert_eq!(resp.body.as_ref(), [7u8; 128].as_slice(), "{enc}");
        assert!(resp.headers.get("content-encoding").is_none(), "{enc}");
        // 标签补全（GET）
        let mut o = RequestOpts::get(format!(
            "{}/ai/generate-image/suggest-tags?model=m&prompt=p",
            up.url()
        ));
        o.accept = "application/json";
        let resp = c
            .request(o, &Callbacks::default(), None, &NoObserver)
            .await
            .unwrap();
        let tags: serde_json::Value = serde_json::from_slice(&resp.body).unwrap();
        assert_eq!(tags["tags"][0]["tag"], "1girl", "{enc}");
        // 订阅查询：Token 校验与额度查询
        assert_eq!(c.verify_token(T2, None).await, Ok(3), "{enc}");
        let (status, _, sub) = c
            .get_json_with(
                T1,
                0,
                &format!("{}/user/subscription", up.url()),
                Duration::from_secs(5),
                1 << 20,
                false,
            )
            .await
            .unwrap();
        assert_eq!(status, 200);
        let sub: serde_json::Value = serde_json::from_slice(&sub).unwrap();
        assert_eq!(sub["usage"]["percent"], 80, "{enc}");
        // 上面每个响应都真的压缩过；每个请求都按 Chrome 声明了四种编码
        let encoded = up.encoded_responses();
        for path in [
            "/ai/generate-image",
            "/ai/encode-vibe",
            "/ai/generate-image/suggest-tags",
            "/user/subscription",
        ] {
            assert!(
                encoded.iter().any(|(p, e)| p == path && e == enc),
                "{enc} {path} {encoded:?}"
            );
        }
        for r in up.requests() {
            assert_eq!(
                r.header("accept-encoding"),
                Some("gzip, deflate, br, zstd"),
                "{}",
                r.path
            );
        }
    }
}

/// 按 4 字节大端长度前缀切 msgpack 帧，返回每帧的 event_type。
fn msgpack_events(mut data: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    while data.len() >= 4 {
        let len = u32::from_be_bytes(data[..4].try_into().unwrap()) as usize;
        let v = rmpv::decode::read_value(&mut &data[4..4 + len]).unwrap();
        let kind = v
            .as_map()
            .unwrap()
            .iter()
            .find(|(k, _)| k.as_str() == Some("event_type"))
            .and_then(|(_, v)| v.as_str())
            .unwrap()
            .to_string();
        out.push(kind);
        data = &data[4 + len..];
    }
    assert!(data.is_empty(), "帧没有切齐");
    out
}

fn msgpack_body(n: i64) -> serde_json::Value {
    let mut body = image_body(n);
    body["parameters"]["stream"] = json!("msgpack");
    body
}

#[tokio::test]
async fn compressed_msgpack_stream_is_complete() {
    use futures::StreamExt;
    for enc in ENCODINGS {
        let up = MockUpstream::start().await;
        up.set_encoding(Some(enc));
        let c = NaiClient::new(
            config(&up.url(), false),
            TestHooks::default(),
            vec![(T1.into(), 0, false)],
        )
        .unwrap();
        let mut s = c
            .image_stream(
                &format!("{}/ai/generate-image-stream", up.url()),
                &msgpack_body(2),
                2,
                false,
                false,
                90.0,
                &Callbacks::default(),
                None,
                &NoObserver,
            )
            .await
            .unwrap();
        assert!(
            s.headers().unwrap().get("content-encoding").is_none(),
            "{enc}"
        );
        let mut data = Vec::new();
        let mut stream = Box::pin(s.take_body().unwrap());
        while let Some(chunk) = stream.next().await {
            data.extend_from_slice(&chunk.unwrap());
        }
        let events = msgpack_events(&data);
        assert_eq!(
            events.iter().filter(|e| *e == "final").count(),
            2,
            "{enc} {events:?}"
        );
        assert_eq!(up.encoded_responses().len(), 1, "{enc}");
        s.completed_images = 2;
        s.finish().await;
    }
}

/// 压缩的图片流仍是边收边转：上游先发一帧、一小时后才发下一块，整包缓冲的话第一帧永远等不到。
#[tokio::test]
async fn compressed_msgpack_stream_is_not_buffered() {
    use futures::StreamExt;
    let frame = gate_testkit::msgpack_frame(vec![
        ("event_type", "intermediate".into()),
        ("samp_ix", 0.into()),
        ("step_ix", 0.into()),
        ("image", rmpv::Value::Binary(gate_testkit::tiny_png())),
    ]);
    for enc in ENCODINGS {
        let up = MockUpstream::start().await;
        up.set_encoding(Some(enc));
        up.push(
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
        let c = NaiClient::new(
            config(&up.url(), false),
            TestHooks::default(),
            vec![(T1.into(), 0, false)],
        )
        .unwrap();
        let mut s = c
            .image_stream(
                &format!("{}/ai/generate-image-stream", up.url()),
                &msgpack_body(1),
                1,
                false,
                false,
                90.0,
                &Callbacks::default(),
                None,
                &NoObserver,
            )
            .await
            .unwrap();
        let mut stream = Box::pin(s.take_body().unwrap());
        let mut got = Vec::new();
        tokio::time::timeout(Duration::from_secs(10), async {
            while got.len() < frame.len() {
                got.extend_from_slice(&stream.next().await.unwrap().unwrap());
            }
        })
        .await
        .unwrap_or_else(|_| panic!("{enc}：第一帧没有在上游结束前转出来"));
        assert_eq!(got, frame, "{enc}");
        assert_eq!(msgpack_events(&got), ["intermediate"], "{enc}");
        drop(stream);
        s.finish().await;
    }
}
