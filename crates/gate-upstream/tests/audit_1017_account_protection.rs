//! 2026-10-17 逐行挑战的回归测试：上游账号保护的三条边界。
//!
//! 1. 标签补全（`suggest-tags`）成功不能清掉「连续 403」的连击计数——否则被上游风控的账号
//!    会因为成员输入提示词时触发的 tags 请求而永远凑不满 3 次，既不隔离也不告警。
//! 2. 标签补全同样是「打上游账号」，必须受账号保护（熔断 / 安静时段 / 上限）约束。
//! 3. 生图 / 图片工具是非幂等 POST：上游返回 3xx 时绝不自动跟随（307/308 会把请求体重发一次）。
//! 4. 403 / 429 冷却要能跨重启（M-4）：变化时报给钩子存库，新进程恢复后接着冷却、退避档位接着算。

mod common;

use bytes::Bytes;
use common::TestHooks;
use gate_testkit::{MockUpstream, Reply};
use gate_upstream::client::{Callbacks, Config, Hooks, NaiClient, NoObserver, RequestOpts};
use gate_upstream::pool::{Cooldown, decode_cooldowns, encode_cooldowns};
use serde_json::json;

const TOKEN: &str = "pst-synthetic-0000000000000000";

fn client(up: &MockUpstream, hooks: &TestHooks) -> NaiClient<TestHooks> {
    NaiClient::new(
        Config {
            image_host: up.url(),
            proxy: None,
            pace: false,
        },
        hooks.clone(),
        vec![(TOKEN.into(), 0, false)],
    )
    .unwrap()
}

fn image_opts<'a>(up: &MockUpstream, body: &'a serde_json::Value) -> RequestOpts<'a> {
    let mut o = RequestOpts::post(format!("{}/ai/generate-image", up.url()), body);
    o.image_lane = true;
    o.image_count = 1;
    o.max_response_bytes = Some(4 << 20);
    o
}

/// 与 `routes/image/tags.rs:120-127` 相同的形态：GET、image_lane、不等待出图间隔。
fn tags_opts(up: &MockUpstream) -> RequestOpts<'static> {
    let mut o = RequestOpts::get(format!(
        "{}/ai/generate-image/suggest-tags?prompt=1girl&model=nai-diffusion-4-5-full",
        up.url()
    ));
    o.accept = "application/json";
    o.image_lane = true;
    o.wait_for_image_slot = false;
    o
}

fn image_body() -> serde_json::Value {
    json!({"input": "1girl", "model": "nai-diffusion-4-5-full", "action": "generate",
           "parameters": {"width": 832, "height": 1216, "steps": 28, "n_samples": 1, "scale": 5.0}})
}

/// tags 的 200 不能把「连续 403」打断：三次生图 403 之间各夹一次 tags 200，仍应在第 3 次隔离。
#[tokio::test]
async fn tags_success_does_not_reset_forbidden_streak() {
    let up = MockUpstream::start().await;
    let hooks = TestHooks::default();
    let c = client(&up, &hooks);
    let body = image_body();

    for _ in 0..2 {
        up.push("/ai/generate-image", Reply::account_403());
        let e = c
            .request(
                image_opts(&up, &body),
                &Callbacks::default(),
                None,
                &NoObserver,
            )
            .await
            .unwrap_err();
        assert_eq!(e.status, 403);
        // 成员客户端输入提示词时触发的旁路请求：200，不应被当成「账号恢复」
        let r = c
            .request(tags_opts(&up), &Callbacks::default(), None, &NoObserver)
            .await
            .expect("tags 正常返回");
        assert_eq!(r.status, 200);
    }
    up.push("/ai/generate-image", Reply::account_403());
    let e = c
        .request(
            image_opts(&up, &body),
            &Callbacks::default(),
            None,
            &NoObserver,
        )
        .await
        .unwrap_err();
    assert_eq!(e.status, 403);

    let f = c.tokens().await[0].fields();
    assert_eq!(
        (f.forbidden_trips, f.forbidden_streak),
        (1, 0),
        "第 3 次连续 403 必须触发隔离（tags 成功不能清连击）"
    );
    assert!(f.blocked_until > hooks.now() + 299.0, "第一轮冷却 5 分钟");
    assert!(
        hooks
            .0
            .events
            .lock()
            .unwrap()
            .iter()
            .any(|k| k == "upstream_403_cooldown"),
        "应发出 upstream_403_cooldown 告警事件"
    );
}

/// 账号保护拦住时，tags 也不能发往上游。
#[tokio::test]
async fn tags_are_blocked_by_account_guard() {
    let up = MockUpstream::start().await;
    let hooks = TestHooks::default();
    *hooks.0.block.lock().unwrap() = Some((
        "现在是安静时段（3:00–7:00），暂停出图".into(),
        "quiet_cap".into(),
    ));
    let c = client(&up, &hooks);

    let e = c
        .request(tags_opts(&up), &Callbacks::default(), None, &NoObserver)
        .await
        .unwrap_err();
    assert_eq!(e.status, 429);
    assert_eq!(e.code, "quiet_cap");
    assert_eq!(
        up.requests().len(),
        0,
        "被账号保护拦住时不得向上游发任何请求"
    );
}

/// 生图 POST 遇 307 不得自动跟随：一次成员请求只能对应一次上游请求，且不重发请求体。
#[tokio::test]
async fn image_post_does_not_follow_redirects() {
    let up = MockUpstream::start().await;
    up.push(
        "/ai/generate-image",
        Reply::Status {
            status: 307,
            headers: vec![("location".into(), "/ai/generate-image".into())],
            body: Bytes::new(),
        },
    );
    let hooks = TestHooks::default();
    let c = client(&up, &hooks);
    let body = image_body();

    let e = c
        .request(
            image_opts(&up, &body),
            &Callbacks::default(),
            None,
            &NoObserver,
        )
        .await
        .unwrap_err();
    assert_eq!(e.status, 502);
    assert_eq!(
        up.requests_to("/ai/generate-image").len(),
        1,
        "307 不得被自动跟随（否则 307/308 会把生图请求体重发到新地址）"
    );
}

async fn image_403(c: &NaiClient<TestHooks>, up: &MockUpstream) {
    up.push("/ai/generate-image", Reply::account_403());
    let body = image_body();
    let e = c
        .request(
            image_opts(up, &body),
            &Callbacks::default(),
            None,
            &NoObserver,
        )
        .await
        .unwrap_err();
    assert_eq!(e.status, 403);
}

/// 冷却跨重启：第一个进程 3 次 403 → 冷却 5 分钟并报给钩子；新进程恢复后仍在冷却，
/// 退避档位也接着算（下一轮 15 分钟，而不是从 5 分钟重新算）。
#[tokio::test]
async fn forbidden_cooldown_survives_restart() {
    let up = MockUpstream::start().await;
    let hooks = TestHooks::default();
    let c = client(&up, &hooks);
    for _ in 0..3 {
        image_403(&c, &up).await;
    }
    let (id, saved) = hooks.0.cooldowns.lock().unwrap().last().cloned().unwrap();
    assert_eq!(id, c.tokens().await[0].token_id);
    assert_eq!(saved.forbidden_trips, 1);
    assert!(saved.blocked_until > hooks.now() + 299.0);

    // 「重启」：新客户端，从存库格式恢复
    let raw = encode_cooldowns(&[(id.clone(), saved)].into_iter().collect());
    let restarted = TestHooks::default();
    let c2 = client(&up, &restarted);
    let t = c2.tokens().await[0].clone();
    assert!(t.usable(restarted.now()), "恢复前是新状态");
    t.restore_cooldown(decode_cooldowns(Some(&raw))[&id], restarted.now());
    assert!(!t.usable(restarted.now()), "恢复后仍在冷却，不立刻试探");

    // 403 退避档位接着算：下一轮按第 2 档（15 分钟），不是从 5 分钟重来
    assert_eq!(t.cooldown().forbidden_trips, 1);
    assert_eq!(t.cooldown().blocked_until, saved.blocked_until);
}

#[test]
fn cooldown_store_format_and_merge() {
    let now = 1_000_000.0;
    let a = Cooldown {
        blocked_until: now + 600.0,
        forbidden_trips: 2,
        forbidden_last: now - 10.0,
    };
    let map = [("tok-a".to_string(), a)].into_iter().collect();
    let back = decode_cooldowns(Some(&encode_cooldowns(&map)));
    assert_eq!(back["tok-a"], a);

    // 坏数据一律当没有
    assert!(decode_cooldowns(None).is_empty());
    assert!(decode_cooldowns(Some("not json")).is_empty());
    assert!(decode_cooldowns(Some(r#"{"x": [1, "2", 3]}"#)).is_empty());

    // 合并：冷却取更晚的，档位以最近一次 403 为准
    let older = Cooldown {
        blocked_until: now + 900.0,
        forbidden_trips: 1,
        forbidden_last: now - 100.0,
    };
    let m = older.merge(a);
    assert_eq!(m.blocked_until, now + 900.0);
    assert_eq!((m.forbidden_trips, m.forbidden_last), (2, now - 10.0));
    assert_eq!(a.merge(older), m, "合并与顺序无关");

    // 过期判断：冷却已过、403 在 24 小时前 → 存库时丢掉
    assert!(a.live(now));
    let stale = Cooldown {
        blocked_until: now - 1.0,
        forbidden_trips: 3,
        forbidden_last: now - 86_401.0,
    };
    assert!(!stale.live(now));

    // 10-22～10-23 的版本多存了第 4、5 项（出图间隔 AIMD，已删除）：读到也忽略；新写的只有三项
    let old = decode_cooldowns(Some(r#"{"tok-a": [1000600.0, 2, 999990.0, 60.0, 5.0]}"#));
    assert_eq!(old["tok-a"], a);
    let map = [("tok-b".to_string(), a)].into_iter().collect();
    let text = encode_cooldowns(&map);
    assert_eq!(decode_cooldowns(Some(&text))["tok-b"], a);
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["tok-b"].as_array().unwrap().len(), 3, "{text}");
}
