//! 出图调度（站长 10-22）：选「最早能开始」的账号、排队中换到空出来的账号、
//! 出完后歇 N～M 秒（出图节奏的唯一旋钮，站长 10-23）、占了槽却没发出去时不歇。

mod common;

use common::TestHooks;
use gate_testkit::{MockUpstream, Reply};
use gate_upstream::client::{
    Callbacks, Config, Hooks, NaiClient, NoObserver, Observer, RequestOpts, UpstreamError,
};
use gate_upstream::pool::Cooldown;
use serde_json::json;
use std::sync::Mutex;
use std::time::{Duration, Instant};

const T1: &str = "pst-aaaaaaaaaaaaaaaaaaaa";
const T2: &str = "pst-bbbbbbbbbbbbbbbbbbbb";

/// `rest` = 每张出完后这个账号歇几秒（账号保护的最少 = 最多 = `rest`）；0 = 测试环境，不歇。
fn config(url: &str, hooks: &TestHooks, rest: f64) -> Config {
    *hooks.0.post.lock().unwrap() = (rest, rest);
    Config {
        image_host: url.into(),
        proxy: None,
        pace: rest > 0.0,
    }
}

fn image_body() -> serde_json::Value {
    json!({"input": "1girl", "model": "nai-diffusion-4-5-full", "action": "generate",
           "parameters": {"width": 832, "height": 1216, "steps": 28, "n_samples": 1, "scale": 5.0}})
}

fn image_opts<'a>(url: &str, body: &'a serde_json::Value) -> RequestOpts<'a> {
    let mut o = RequestOpts::post(format!("{url}/ai/generate-image"), body);
    o.image_lane = true;
    o.image_count = 1;
    o.max_response_bytes = Some(64 << 20);
    o
}

fn bearer(t: &str) -> String {
    format!("Bearer {t}")
}

/// 发往上游的每张图用的是哪把 Token（按发出顺序）。
fn used(up: &MockUpstream) -> Vec<String> {
    up.requests_to("/ai/generate-image")
        .iter()
        .map(|r| r.header("authorization").unwrap_or_default().to_string())
        .collect()
}

/// 记下选号 / 换号和「还要等多久」。
#[derive(Default)]
struct Seen {
    picked: Mutex<Vec<String>>,
    queued: Mutex<Vec<(String, f64)>>,
}

impl Observer for Seen {
    fn picked(&self, token_id: &str) {
        self.picked.lock().unwrap().push(token_id.to_string());
    }
    fn queued(&self, token_id: &str, wait: f64) {
        self.queued
            .lock()
            .unwrap()
            .push((token_id.to_string(), wait));
    }
}

/// 资格照旧：要花 Anlas 的图只给允许 Anlas 的账号，哪怕它慢、另一个空着。
#[tokio::test]
async fn pick_respects_eligibility_over_speed() {
    let up = MockUpstream::start().await;
    let hooks = TestHooks::default();
    let c = NaiClient::new(
        config(&up.url(), &hooks, 0.0),
        hooks.clone(),
        vec![(T1.into(), 0, false), (T2.into(), 0, true)],
    )
    .unwrap();
    let body = image_body();
    for _ in 0..2 {
        let mut o = image_opts(&up.url(), &body);
        o.requires_anlas = true;
        c.request(o, &Callbacks::default(), None, &NoObserver)
            .await
            .unwrap();
    }
    assert_eq!(used(&up), [bearer(T2), bearer(T2)]);
}

/// 排队中：别的账号空出来了就挪过去，不在原来的账号后面干等。
#[tokio::test]
async fn waiting_request_moves_to_an_account_that_frees_up() {
    let up = MockUpstream::start().await;
    let hooks = TestHooks::default();
    let c = NaiClient::new(
        config(&up.url(), &hooks, 6.0),
        hooks.clone(),
        vec![(T1.into(), 0, false), (T2.into(), 0, false)],
    )
    .unwrap();
    // T2 还在 429 冷却里（1 秒后恢复）：一开始只有 T1 能用
    let now = hooks.now();
    c.tokens().await[1].restore_cooldown(
        Cooldown {
            blocked_until: now + 1.0,
            ..Default::default()
        },
        now,
    );
    let body = image_body();
    c.request(
        image_opts(&up.url(), &body),
        &Callbacks::default(),
        None,
        &NoObserver,
    )
    .await
    .unwrap();
    // 第二张只能排在 T1 后面（要等 6 秒）；T2 冷却一结束就换过去
    let seen = Seen::default();
    let t0 = Instant::now();
    c.request(
        image_opts(&up.url(), &body),
        &Callbacks::default(),
        None,
        &seen,
    )
    .await
    .unwrap();
    let took = t0.elapsed();
    assert!(
        took < Duration::from_secs(4),
        "应当换号而不是等满间隔：{took:?}"
    );
    assert_eq!(used(&up), [bearer(T1), bearer(T2)]);
    let (id1, id2) = (
        gate_upstream::headers::token_id(T1),
        gate_upstream::headers::token_id(T2),
    );
    assert_eq!(*seen.picked.lock().unwrap(), [id1.clone(), id2]);
    // 排队期间报过「还要等多久」（在 T1 上，约 6 秒）
    let queued = seen.queued.lock().unwrap().clone();
    assert!(
        queued.iter().any(|(t, w)| *t == id1 && *w > 3.0),
        "{queued:?}"
    );
    // 换走以后 T1 上没有残留的排队票和并发槽
    let t1 = &c.tokens().await[0];
    assert_eq!(t1.waiting_len(), 0);
    assert_eq!(t1.image_slots.active(), 0);
}

/// 换号也要守资格：要花 Anlas 的图不会挪到不允许 Anlas 的账号上。
#[tokio::test]
async fn move_respects_eligibility() {
    let up = MockUpstream::start().await;
    let hooks = TestHooks::default();
    let c = NaiClient::new(
        config(&up.url(), &hooks, 2.0),
        hooks.clone(),
        vec![(T1.into(), 0, true), (T2.into(), 0, false)],
    )
    .unwrap();
    let now = hooks.now();
    c.tokens().await[1].restore_cooldown(
        Cooldown {
            blocked_until: now + 0.5,
            ..Default::default()
        },
        now,
    );
    let body = image_body();
    for _ in 0..2 {
        let mut o = image_opts(&up.url(), &body);
        o.requires_anlas = true;
        c.request(o, &Callbacks::default(), None, &NoObserver)
            .await
            .unwrap();
    }
    assert_eq!(used(&up), [bearer(T1), bearer(T1)]);
}

/// 免费 V5 换号时 V5 预留跟着走：旧账号释放、新账号预留，出完只记在新账号上。
#[tokio::test]
async fn move_carries_the_v5_reservation() {
    let up = MockUpstream::start().await;
    let hooks = TestHooks::default();
    let c = NaiClient::new(
        config(&up.url(), &hooks, 6.0),
        hooks.clone(),
        vec![(T1.into(), 5, false), (T2.into(), 5, false)],
    )
    .unwrap();
    let now = hooks.now();
    c.tokens().await[1].restore_cooldown(
        Cooldown {
            blocked_until: now + 1.0,
            ..Default::default()
        },
        now,
    );
    let body = image_body();
    let v5 = || {
        let mut o = image_opts(&up.url(), &body);
        o.v5_free = true;
        o
    };
    c.request(v5(), &Callbacks::default(), None, &NoObserver)
        .await
        .unwrap();
    c.request(v5(), &Callbacks::default(), None, &NoObserver)
        .await
        .unwrap();
    assert_eq!(used(&up), [bearer(T1), bearer(T2)]);
    let tokens = c.tokens().await;
    assert_eq!(tokens[0].fields().pending_v5, 0);
    assert_eq!(tokens[1].fields().pending_v5, 0);
    let counts = hooks.0.v5.lock().unwrap().clone();
    assert_eq!(counts.get(&tokens[0].token_id), Some(&1));
    assert_eq!(counts.get(&tokens[1].token_id), Some(&1));
}

/// 占到了槽、发出前被拒（这里是 before_dispatch）：不算出了一张，下一张不用歇。
#[tokio::test]
async fn refused_after_acquiring_does_not_burn_the_interval() {
    let up = MockUpstream::start().await;
    let hooks = TestHooks::default();
    let c = NaiClient::new(
        config(&up.url(), &hooks, 5.0),
        hooks.clone(),
        vec![(T1.into(), 0, false)],
    )
    .unwrap();
    let body = image_body();
    let refuse = Callbacks {
        before_dispatch: Some(Box::new(|| {
            Box::pin(async { Err(UpstreamError::new(503, "全站冷却中")) })
        })),
        ..Default::default()
    };
    let e = c
        .request(image_opts(&up.url(), &body), &refuse, None, &NoObserver)
        .await
        .unwrap_err();
    assert_eq!(e.status, 503);
    assert!(up.requests().is_empty());
    let t0 = Instant::now();
    c.request(
        image_opts(&up.url(), &body),
        &Callbacks::default(),
        None,
        &NoObserver,
    )
    .await
    .unwrap();
    assert!(
        t0.elapsed() < Duration::from_secs(2),
        "没发出去的那张不该让账号歇 5 秒：{:?}",
        t0.elapsed()
    );
    // 真发出去的照常歇
    assert!(c.pacing(&c.tokens().await[0]).next_start_in > 3.0);
}

/// 上游 429：只让这个账号冷却，节奏不变（10-23 起没有 AIMD）；冷却存库的格式只有三项。
#[tokio::test]
async fn a_429_cools_the_account_but_keeps_the_interval() {
    let up = MockUpstream::start().await;
    up.push("/ai/generate-image", Reply::rate_limited("5"));
    let hooks = TestHooks::default();
    let c = NaiClient::new(
        config(&up.url(), &hooks, 15.0),
        hooks.clone(),
        vec![(T1.into(), 0, false), (T2.into(), 0, false)],
    )
    .unwrap();
    let body = image_body();
    let e = c
        .request(
            image_opts(&up.url(), &body),
            &Callbacks::default(),
            None,
            &NoObserver,
        )
        .await
        .unwrap_err();
    assert_eq!(e.status, 429);
    let tokens = c.tokens().await;
    // 第一张按轮询给了 T2
    let (hit, other) = (&tokens[1], &tokens[0]);
    let p = c.pacing(hit);
    assert_eq!(
        p.cycle, 15.0,
        "周期 = 出完后歇的 15 秒（还没有出图耗时样本）"
    );
    assert!(p.last_429 > 0.0);
    assert!(!hit.usable(hooks.now()), "429 后冷却");
    assert_eq!(c.pacing(other).cycle, 15.0);
    assert_eq!(p.locked_today, 0, "真 429 不算并发锁");
    let (id, saved) = hooks.0.cooldowns.lock().unwrap().last().cloned().unwrap();
    assert_eq!(id, hit.token_id);
    assert!(saved.blocked_until > hooks.now());
}

/// 排队上限按「排队超时内真能开始几张」：出完歇 15 秒（还没有出图耗时样本）、超时 90 秒 → 每个账号 6 张。
#[tokio::test]
async fn queue_caps_follow_each_accounts_cycle() {
    let up = MockUpstream::start().await;
    let hooks = TestHooks::default();
    let c = NaiClient::new(
        config(&up.url(), &hooks, 15.0),
        hooks.clone(),
        vec![(T1.into(), 0, false), (T2.into(), 0, false)],
    )
    .unwrap();
    assert_eq!(c.queue_caps(90.0).await, [Some(6), Some(6)]);
    *hooks.0.post.lock().unwrap() = (30.0, 30.0);
    assert_eq!(c.queue_caps(90.0).await, [Some(3), Some(3)]);
    // 测试环境不歇：估不出周期，不限
    let c0 = NaiClient::new(
        config(&up.url(), &hooks, 0.0),
        hooks,
        vec![(T1.into(), 0, false)],
    )
    .unwrap();
    assert_eq!(c0.queue_caps(90.0).await, [None]);
}

/// 等了一半排队超时：别的账号虽然还没空，但明显能更早开始（早 2 秒以上），也挪过去，免得排到超时。
#[tokio::test]
async fn after_half_the_timeout_a_clearly_earlier_account_wins() {
    #[derive(Default)]
    struct Timed(Mutex<Vec<(String, Instant)>>);
    impl Observer for Timed {
        fn picked(&self, token_id: &str) {
            self.0
                .lock()
                .unwrap()
                .push((token_id.to_string(), Instant::now()));
        }
    }
    let up = MockUpstream::start().await;
    let hooks = TestHooks::default();
    let c = NaiClient::new(
        config(&up.url(), &hooks, 6.0),
        hooks.clone(),
        vec![(T1.into(), 0, false), (T2.into(), 0, false)],
    )
    .unwrap();
    let body = image_body();
    let send = |timeout: f64, obs: std::sync::Arc<dyn Observer>| {
        let (c, url, body) = (c.clone(), up.url(), body.clone());
        tokio::spawn(async move {
            let mut o = image_opts(&url, &body);
            o.queue_timeout = timeout;
            c.request(o, &Callbacks::default(), None, obs.as_ref())
                .await
        })
    };
    let none = || std::sync::Arc::new(NoObserver) as std::sync::Arc<dyn Observer>;
    let t0 = Instant::now();
    // 两个账号各出一张：都要 6 秒后才能出下一张
    send(30.0, none()).await.unwrap().unwrap();
    send(30.0, none()).await.unwrap().unwrap();
    let pause = || tokio::time::sleep(Duration::from_millis(80));
    // T2（一样快，按轮询）。排队超时比 r5 的短一点：r3 先「等久了」，公平排队里一直排在 r5 前面
    // （否则 r5 先过了一半超时会在 T2 上排到 r3 前面，就不用挪了）
    let r3 = send(6.9, none());
    pause().await;
    let r4 = send(30.0, none()); // T1
    pause().await;
    let timed = std::sync::Arc::new(Timed::default());
    let r5 = send(7.0, timed.clone()); // 两边都排第二（约 12 秒），按轮询排到 T2
    pause().await;
    r4.abort(); // T1 上前面那张走了：T1 约 6 秒就能开始，r5 在 T2 上还要约 12 秒
    let _ = r4.await;
    r5.await.unwrap().unwrap();
    r3.await.unwrap().unwrap();
    let picks = timed.0.lock().unwrap().clone();
    let (id1, id2) = (
        gate_upstream::headers::token_id(T1),
        gate_upstream::headers::token_id(T2),
    );
    assert_eq!(
        picks.iter().map(|(t, _)| t.clone()).collect::<Vec<_>>(),
        [id2, id1]
    );
    // 在 T1 真空出来（约 6 秒）之前就挪了：按「明显更早」挪，不是按「空出来」
    let moved_at = picks[1].1 - t0;
    assert!(moved_at < Duration::from_millis(5500), "{moved_at:?}");
}

// ---------------------------------------------------------------- 上游并发锁（站长 10-22）

/// 记下状态码、并发锁、什么时候发出。
#[derive(Default)]
struct Watch {
    share: i64,
    statuses: Mutex<Vec<u16>>,
    locked: Mutex<Vec<String>>,
    sent_at: Mutex<Option<Instant>>,
}

impl Watch {
    fn with_share(share: i64) -> Self {
        Self {
            share,
            ..Default::default()
        }
    }
}

impl Observer for Watch {
    fn status(&self, code: u16) {
        self.statuses.lock().unwrap().push(code);
    }
    fn locked(&self, token_id: &str) {
        self.locked.lock().unwrap().push(token_id.to_string());
    }
    fn sent(&self) {
        self.sent_at.lock().unwrap().get_or_insert(Instant::now());
    }
    fn share(&self) -> i64 {
        self.share
    }
}

/// 回调记一下有没有被当成限流（全站冷却）。
fn counting_callbacks(hits: &std::sync::Arc<std::sync::atomic::AtomicUsize>) -> Callbacks<'static> {
    let hits = hits.clone();
    Callbacks {
        on_rate_limited: Some(Box::new(move |_| {
            hits.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Box::pin(async {})
        })),
        ..Default::default()
    }
}

/// 并发锁不是限流：等约 2 秒在同一个账号上重试成功；间隔不翻倍、账号不冷却、不回调限流、不报 429，
/// 后台按账号单独数一次。
#[tokio::test]
async fn concurrency_lock_is_retried_on_the_same_account_without_aimd() {
    let up = MockUpstream::start().await;
    up.push("/ai/generate-image", Reply::concurrency_locked(429));
    let hooks = TestHooks::default();
    let c = NaiClient::new(
        config(&up.url(), &hooks, 0.0),
        hooks.clone(),
        vec![(T1.into(), 0, false)],
    )
    .unwrap();
    let body = image_body();
    let hits = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let obs = Watch::default();
    let t0 = Instant::now();
    let resp = c
        .request(
            image_opts(&up.url(), &body),
            &counting_callbacks(&hits),
            None,
            &obs,
        )
        .await
        .unwrap();
    assert_eq!(resp.status, 200);
    let took = t0.elapsed();
    assert!(
        took >= Duration::from_millis(1900) && took < Duration::from_secs(5),
        "{took:?}"
    );
    assert_eq!(used(&up), [bearer(T1), bearer(T1)], "同一个账号上重试");
    let t = &c.tokens().await[0];
    let p = c.pacing(t);
    assert_eq!(p.last_429, 0.0, "不算 429");
    assert_eq!(p.locked_today, 1);
    assert!(t.usable(hooks.now()), "账号不冷却");
    assert_eq!(
        hits.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "不触发全站限流冷却"
    );
    assert!(hooks.0.cooldowns.lock().unwrap().is_empty());
    assert_eq!(*obs.statuses.lock().unwrap(), [200], "不报 429 状态码");
    assert_eq!(obs.locked.lock().unwrap().len(), 1);
    assert_eq!(hooks.0.images.lock().unwrap().values().sum::<i64>(), 1);
}

/// 重试次数受排队超时限制：超时 3 秒时只够等一次（2 秒），第二次（4 秒）等不到，以 upstream_locked 结束。
/// 409 + 同样的正文也是并发锁。
#[tokio::test]
async fn concurrency_lock_gives_up_within_the_queue_timeout() {
    let up = MockUpstream::start().await;
    for _ in 0..3 {
        up.push("/ai/generate-image", Reply::concurrency_locked(409));
    }
    let hooks = TestHooks::default();
    let c = NaiClient::new(
        config(&up.url(), &hooks, 0.0),
        hooks.clone(),
        vec![(T1.into(), 0, false)],
    )
    .unwrap();
    let body = image_body();
    let mut o = image_opts(&up.url(), &body);
    o.queue_timeout = 3.0;
    let obs = Watch::default();
    let e: UpstreamError = c
        .request(o, &Callbacks::default(), None, &obs)
        .await
        .unwrap_err();
    assert_eq!(e.code, "upstream_locked");
    assert_eq!(e.status, 503);
    assert!(e.message.contains("上一张图仍未释放"), "{}", e.message);
    assert_eq!(up.requests_to("/ai/generate-image").len(), 2);
    let t = &c.tokens().await[0];
    assert_eq!(c.pacing(t).locked_today, 2);
    assert!(obs.statuses.lock().unwrap().is_empty());
    assert_eq!(t.image_slots.active(), 0, "并发槽归还");
}

/// 流式出图同样认并发锁（正文要先读出来才分得清）；普通 429 仍然是限流。
#[tokio::test]
async fn stream_retries_a_concurrency_lock_but_not_a_rate_limit() {
    let up = MockUpstream::start().await;
    up.push("/ai/generate-image-stream", Reply::concurrency_locked(429));
    let hooks = TestHooks::default();
    let c = NaiClient::new(
        config(&up.url(), &hooks, 0.0),
        hooks.clone(),
        vec![(T1.into(), 0, false)],
    )
    .unwrap();
    let mut body = image_body();
    body["parameters"]["stream"] = json!("msgpack");
    let url = format!("{}/ai/generate-image-stream", up.url());
    let open = |c: NaiClient<TestHooks>| {
        let (url, body) = (url.clone(), body.clone());
        async move {
            c.image_stream(
                &url,
                &body,
                1,
                false,
                false,
                30.0,
                &Callbacks::default(),
                None,
                &NoObserver,
            )
            .await
        }
    };
    let s = open(c.clone()).await.unwrap();
    s.finish().await;
    assert_eq!(up.requests_to("/ai/generate-image-stream").len(), 2);
    let t = c.tokens().await[0].clone();
    assert_eq!(c.pacing(&t).locked_today, 1);
    assert_eq!(c.pacing(&t).last_429, 0.0);
    // 真限流：账号冷却（出图间隔不变，10-23 起没有自动翻倍）
    up.push("/ai/generate-image-stream", Reply::rate_limited("5"));
    let e = open(c.clone()).await.err().unwrap();
    assert_eq!(e.status, 429);
    assert!(c.pacing(&t).last_429 > 0.0);
    assert!(!t.usable(hooks.now()), "真 429 照常冷却");
    assert_eq!(c.pacing(&t).locked_today, 1);
}

// ---------------------------------------------------------------- 公平排队（站长 10-22）

/// 一个账号上正在出一张；先来的重度用户（今天 150 张）和后来的轻度用户（今天 3 张）在等：
/// 空出来时轻度用户先上。
#[tokio::test]
async fn a_light_user_overtakes_a_heavy_one_on_the_same_account() {
    let up = MockUpstream::start().await;
    up.push(
        "/ai/generate-image",
        Reply::Chunks {
            status: 200,
            content_type: "binary/octet-stream".into(),
            chunks: vec![(
                Duration::from_millis(600),
                bytes::Bytes::from(gate_testkit::zip_of_pngs(1)),
            )],
        },
    );
    let hooks = TestHooks::default();
    let c = NaiClient::new(
        config(&up.url(), &hooks, 0.0),
        hooks.clone(),
        vec![(T1.into(), 0, false)],
    )
    .unwrap();
    let body = image_body();
    let send = |obs: std::sync::Arc<Watch>, timeout: f64| {
        let (c, url, body) = (c.clone(), up.url(), body.clone());
        tokio::spawn(async move {
            let mut o = image_opts(&url, &body);
            o.queue_timeout = timeout;
            c.request(o, &Callbacks::default(), None, obs.as_ref())
                .await
        })
    };
    let first = send(std::sync::Arc::new(Watch::with_share(0)), 30.0);
    tokio::time::sleep(Duration::from_millis(100)).await;
    let heavy = std::sync::Arc::new(Watch::with_share(150));
    let h = send(heavy.clone(), 30.0);
    tokio::time::sleep(Duration::from_millis(100)).await;
    let light = std::sync::Arc::new(Watch::with_share(3));
    let l = send(light.clone(), 30.0);
    for r in [first, h, l] {
        r.await.unwrap().unwrap();
    }
    let (ht, lt) = (
        heavy.sent_at.lock().unwrap().unwrap(),
        light.sent_at.lock().unwrap().unwrap(),
    );
    assert!(lt < ht, "轻度用户应先发出：light={lt:?} heavy={ht:?}");
}

/// 防饿死：重度用户等过了排队超时的一半，就按先来后到排在最前，后来的轻度用户不能再插队。
#[tokio::test]
async fn a_long_waiting_heavy_user_is_no_longer_overtaken() {
    let up = MockUpstream::start().await;
    up.push(
        "/ai/generate-image",
        Reply::Chunks {
            status: 200,
            content_type: "binary/octet-stream".into(),
            chunks: vec![(
                Duration::from_millis(1500),
                bytes::Bytes::from(gate_testkit::zip_of_pngs(1)),
            )],
        },
    );
    let hooks = TestHooks::default();
    let c = NaiClient::new(
        config(&up.url(), &hooks, 0.0),
        hooks.clone(),
        vec![(T1.into(), 0, false)],
    )
    .unwrap();
    let body = image_body();
    let send = |obs: std::sync::Arc<Watch>, timeout: f64| {
        let (c, url, body) = (c.clone(), up.url(), body.clone());
        tokio::spawn(async move {
            let mut o = image_opts(&url, &body);
            o.queue_timeout = timeout;
            c.request(o, &Callbacks::default(), None, obs.as_ref())
                .await
        })
    };
    let first = send(std::sync::Arc::new(Watch::with_share(0)), 30.0);
    tokio::time::sleep(Duration::from_millis(50)).await;
    // 排队超时 2 秒：约 1 秒后就算「等久了」
    let heavy = std::sync::Arc::new(Watch::with_share(150));
    let h = send(heavy.clone(), 2.0);
    tokio::time::sleep(Duration::from_millis(1150)).await;
    let light = std::sync::Arc::new(Watch::with_share(3));
    let l = send(light.clone(), 30.0);
    for r in [first, h, l] {
        r.await.unwrap().unwrap();
    }
    let (ht, lt) = (
        heavy.sent_at.lock().unwrap().unwrap(),
        light.sent_at.lock().unwrap().unwrap(),
    );
    assert!(
        ht < lt,
        "等久了的重度用户应先发出：heavy={ht:?} light={lt:?}"
    );
}

/// 成员看到的「还要等多久」按公平顺序：轻度用户排在重度用户前面，估计更早开始。
#[tokio::test]
async fn queue_estimates_follow_the_fair_order() {
    #[derive(Default)]
    struct Eta(i64, Mutex<Vec<f64>>);
    impl Observer for Eta {
        fn queued(&self, _t: &str, wait: f64) {
            self.1.lock().unwrap().push(wait);
        }
        fn share(&self) -> i64 {
            self.0
        }
    }
    let up = MockUpstream::start().await;
    let hooks = TestHooks::default();
    let c = NaiClient::new(
        config(&up.url(), &hooks, 5.0),
        hooks.clone(),
        vec![(T1.into(), 0, false)],
    )
    .unwrap();
    let body = image_body();
    // 先出一张：之后 5 秒内这个账号不能开始
    c.request(
        image_opts(&up.url(), &body),
        &Callbacks::default(),
        None,
        &NoObserver,
    )
    .await
    .unwrap();
    let send = |obs: std::sync::Arc<Eta>| {
        let (c, url, body) = (c.clone(), up.url(), body.clone());
        tokio::spawn(async move {
            let o = image_opts(&url, &body);
            c.request(o, &Callbacks::default(), None, obs.as_ref())
                .await
        })
    };
    let heavy = std::sync::Arc::new(Eta(150, Mutex::default()));
    let h = send(heavy.clone());
    tokio::time::sleep(Duration::from_millis(100)).await;
    let light = std::sync::Arc::new(Eta(3, Mutex::default()));
    let l = send(light.clone());
    tokio::time::sleep(Duration::from_millis(1200)).await;
    let last = |e: &Eta| *e.1.lock().unwrap().last().unwrap();
    // 轻度用户排第一（约 4 秒后），重度用户被挤到第二（再加一个 5 秒周期）
    assert!(last(&light) < 5.0, "{:?}", light.1.lock().unwrap());
    assert!(
        last(&heavy) > last(&light) + 3.0,
        "{:?}",
        heavy.1.lock().unwrap()
    );
    h.abort();
    l.abort();
}

#[tokio::test]
async fn connection_failure_and_interrupted_response_describe_different_billing_outcomes() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let closed = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let closed_url = format!("http://{}", closed.local_addr().unwrap());
    drop(closed);
    let hooks = TestHooks::default();
    let c = NaiClient::new(
        config(&closed_url, &hooks, 0.0),
        hooks,
        vec![(T1.into(), 0, false)],
    )
    .unwrap();
    let body = image_body();
    let rejected = c
        .request(
            image_opts(&closed_url, &body),
            &Callbacks::default(),
            None,
            &NoObserver,
        )
        .await
        .unwrap_err();
    assert!(!rejected.billing_uncertain);
    assert!(rejected.message.contains("未执行、未扣费"));

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let response = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = vec![0; 8192];
        let _ = socket.read(&mut request).await.unwrap();
        socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\nContent-Type: application/zip\r\nConnection: close\r\n\r\nshort").await.unwrap();
        socket.shutdown().await.unwrap();
    });
    let hooks = TestHooks::default();
    let c = NaiClient::new(
        config(&url, &hooks, 0.0),
        hooks,
        vec![(T1.into(), 0, false)],
    )
    .unwrap();
    let unknown = c
        .request(
            image_opts(&url, &body),
            &Callbacks::default(),
            None,
            &NoObserver,
        )
        .await
        .unwrap_err();
    response.await.unwrap();
    assert!(unknown.billing_uncertain);
    assert!(unknown.message.contains("执行结果不明"));
    assert!(unknown.message.contains("额度暂时保留"));
    assert!(!unknown.message.contains("未扣费"));
}
