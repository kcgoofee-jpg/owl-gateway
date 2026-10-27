//! 公开页面、实时看板、状态、客户端视图和首页首屏数据。
//! 期望值来自旧版 Python 网关实际跑出来的响应（2.x 的演示环境）与旧 pytest 用例的断言（都在标签 py-final 里）。

mod common;
use axum::body::{Body, BodyDataStream};
use axum::http::{HeaderMap, Request, StatusCode};
use common::*;
use futures::StreamExt;
use gate_db::repo::usage::LogFilter;
use gate_server::AppState;
use gate_server::key::Key;
use gate_server::session;
use http_body_util::BodyExt;
use serde_json::{Value, json};
use std::time::Duration;
use tower::ServiceExt;

const MEMBER_ID: &str = "900000000000000001";

/// 配了 Discord 注册服务（member_state 才会去查领取的 Key）+ 站点地址 + 邀请链接。
fn reg_env() -> Vec<(&'static str, &'static str)> {
    vec![
        ("DISCORD_CLIENT_ID", "1"),
        ("DISCORD_CLIENT_SECRET", "synthetic-client-secret"),
        ("DISCORD_BOT_TOKEN", "synthetic-bot-token"),
        (
            "REGISTRATION_BRIDGE_SECRET",
            "dev-bridge-secret-0123456789abcdefghij",
        ),
        ("DISCORD_GUILD_ID", "1"),
        ("SITE_URL", "https://gate.example.top"),
        ("DISCORD_INVITE_URL", "https://discord.gg/abc"),
    ]
}

/// 把测试 Key 登记成 Discord 成员，返回 `owl_member=...` Cookie。
async fn register_member(state: &AppState) -> String {
    state
        .db
        .write(|tx| {
            tx.execute(
                "INSERT INTO discord_registrations (discord_id, key_id, created_at, username, display_name, avatar) \
                 VALUES (?,?,?,?,?,?)",
                [MEMBER_ID, "1", "1.0", "member_one", "成员甲", "avatarhash"],
            )
        })
        .await
        .unwrap();
    let row = state.db.get_key_by_token(TOKEN).await.unwrap().unwrap();
    let tag = session::key_tag(Some(&Key(row)));
    let exp = state.now() as i64 + 86400;
    format!(
        "owl_member={}",
        session::sign_member(state, MEMBER_ID, exp, &tag)
    )
}

fn admin_cookie(state: &AppState) -> String {
    format!("nai_gate_admin={}", session::make_admin_cookie(state))
}

fn json_of(body: &[u8]) -> Value {
    serde_json::from_slice(body)
        .unwrap_or_else(|_| panic!("不是 JSON：{}", String::from_utf8_lossy(body)))
}

fn keys(v: &Value) -> Vec<&str> {
    v.as_object().unwrap().keys().map(String::as_str).collect()
}

/// 任意方法 / 任意请求体。
async fn raw(
    state: &AppState,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: &[u8],
) -> (StatusCode, HeaderMap, Vec<u8>) {
    let mut b = Request::builder().method(method).uri(path);
    for (k, v) in headers {
        b = b.header(*k, *v);
    }
    let resp = gate_server::app(state.clone())
        .oneshot(b.body(Body::from(body.to_vec())).unwrap())
        .await
        .unwrap();
    let (status, headers) = (resp.status(), resp.headers().clone());
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (status, headers, bytes.to_vec())
}

async fn get_json(
    state: &AppState,
    path: &str,
    headers: &[(&str, &str)],
) -> (StatusCode, HeaderMap, Value) {
    let (s, h, b) = raw(state, "GET", path, headers, b"").await;
    (s, h, json_of(&b))
}

async fn get_with_auth(state: &AppState, path: &str, token: &str) -> (StatusCode, Vec<u8>) {
    let (status, _, body) = raw(
        state,
        "GET",
        path,
        &[("authorization", &format!("Bearer {token}"))],
        b"",
    )
    .await;
    (status, body)
}

/// SSE 读取器：按空行切事件块。
struct Sse {
    stream: BodyDataStream,
    buf: String,
}

impl Sse {
    async fn open(
        state: &AppState,
        path: &str,
        headers: &[(&str, &str)],
    ) -> (StatusCode, HeaderMap, Sse) {
        let mut b = Request::builder().method("GET").uri(path);
        for (k, v) in headers {
            b = b.header(*k, *v);
        }
        let resp = gate_server::app(state.clone())
            .oneshot(b.body(Body::empty()).unwrap())
            .await
            .unwrap();
        let (status, headers) = (resp.status(), resp.headers().clone());
        (
            status,
            headers,
            Sse {
                stream: resp.into_body().into_data_stream(),
                buf: String::new(),
            },
        )
    }

    /// 下一个事件块（`data: ...` 或保活注释 `: ...`）；超时或流结束返回 None。
    async fn next(&mut self, within: Duration) -> Option<String> {
        loop {
            if let Some(i) = self.buf.find("\n\n") {
                let block = self.buf[..i].to_string();
                self.buf.drain(..i + 2);
                return Some(block);
            }
            match tokio::time::timeout(within, self.stream.next()).await {
                Ok(Some(Ok(chunk))) => self.buf.push_str(&String::from_utf8_lossy(&chunk)),
                _ => return None,
            }
        }
    }

    /// 下一个 `data:` 事件的 JSON（跳过保活注释）。
    async fn next_data(&mut self, within: Duration) -> Option<Value> {
        let deadline = tokio::time::Instant::now() + within;
        loop {
            let left = deadline.saturating_duration_since(tokio::time::Instant::now());
            let block = self.next(left).await?;
            if let Some(d) = block.strip_prefix("data: ") {
                return Some(json_of(d.as_bytes()));
            }
        }
    }
}

fn without_t(mut v: Value) -> Value {
    v.as_object_mut().unwrap().shift_remove("t");
    v
}

// ------------------------------------------------------------------ 条款 / 隐私

/// 「本站说明」页（/terms、/privacy）已下线（站长 10-22）：两个地址都不再有路由。
#[tokio::test]
async fn terms_and_privacy_pages_are_gone() {
    let e = setup(&[("SITE_URL", "https://gate.example.top/")]).await;
    for path in ["/terms", "/privacy"] {
        let (status, _, _) = Call::new("GET", path).send(&e.state).await;
        assert_eq!(status, 404, "{path} 应已下线");
    }
}

/// 站长公告（首页沙箱 iframe、`/announcement`、`has_announcement`）10-23 删除。
#[tokio::test]
async fn announcement_is_gone() {
    let e = setup(&[]).await;
    std::fs::write(
        e.state.env.data_dir.join("announcement.html"),
        "<h1>hi</h1>",
    )
    .unwrap();
    let (status, _, _) = Call::new("GET", "/announcement").send(&e.state).await;
    assert_eq!(status, 404);
    let (_, _, status_body) = get_json(&e.state, "/public/status", &[]).await;
    assert!(status_body.get("has_announcement").is_none());
    let (_, _, live) = get_json(&e.state, "/public/live", &[]).await;
    assert!(live.get("has_announcement").is_none());
}

// ------------------------------------------------------------------ /public/live

#[tokio::test]
async fn public_live_shape_and_key_order() {
    let e = setup(&reg_env()).await;
    e.state.live_note("ok");
    e.state.live_note("rejected");
    let (status, headers, body) = raw(&e.state, "GET", "/public/live", &[], b"").await;
    assert_eq!(status, 200);
    assert_eq!(headers["cache-control"], "no-store");
    assert_eq!(headers["content-type"], "application/json");
    let text = String::from_utf8(body.clone()).unwrap();
    assert!(text.starts_with("{\"t\":"), "{text}");
    assert!(text.ends_with(",\"me\":null}"), "{text}");
    let v = json_of(&body);
    // 键顺序与旧版实际响应一致：live.build → meta → me
    assert_eq!(
        keys(&v),
        [
            "t",
            "mode",
            "auth",
            "quota",
            "queue",
            "guard",
            "members",
            "cycle",
            "rules",
            "site",
            "discord_invite",
            "algo_notice",
            "key_inactivity_delete_days",
            "web_login",
            "registration",
            "me"
        ]
    );
    assert_eq!(v["mode"], "fifo");
    assert_eq!(v["auth"], json!({"requests": 2, "rejected": 1}));
    assert_eq!(
        keys(&v["quota"]),
        // 10-27 起图例只显示「今天各生成了多少张」；images_cap 只是弧线的比例尺（账号上限合计）
        ["images_today", "images_cap", "v5_today"]
    );
    assert_eq!(v["quota"]["images_today"], 0);
    // 单账号、上限 2400：比例尺 = 2400
    assert_eq!(v["quota"]["images_cap"], 2400);
    assert_eq!(
        v["queue"],
        json!({"waiting": 0, "running": 0, "full": false})
    );
    // 排队时间估算已删除：guard 只剩冷却与全站拦截原因
    assert_eq!(keys(&v["guard"]), ["cooldown", "blocked"]);
    assert_eq!(v["guard"]["cooldown"], json!(0));
    assert_eq!(v["guard"]["blocked"], Value::Null);
    // 注册服务已配置：members 是 {max, active, open}
    assert_eq!(v["members"], json!({"max": 10, "active": 0, "open": false}));
    // 一个账号平均多久开始一张：还没有出图耗时样本（测试环境也不歇）→ 默认 10 秒
    assert_eq!(v["cycle"], json!(10.0));
    // 保底 `base`、空闲借用 `borrow_open`、Anlas 自动分配占位 `anlas`、`daily_cap` / `quota` 已删除
    assert_eq!(keys(&v["rules"]), ["key_queue", "queue_cap", "rest"]);
    let rules = &v["rules"];
    // 全站排队上限 = min(账号排队 5, 排队超时内真能开始的张数)：测试环境还估不出周期 → 只按 5
    assert_eq!(rules["queue_cap"], 5);
    // 出图节奏只剩一个手动旋钮：每张出完后歇 1～3 秒
    assert_eq!(rules["rest"], json!([1, 3]));
    assert_eq!(v["site"], "https://gate.example.top");
    assert_eq!(v["discord_invite"], "https://discord.gg/abc");
    assert_eq!(v["key_inactivity_delete_days"], 3);
    assert_eq!(v["web_login"], true, "网页登录常开");
    assert!(
        v.get("home_join_open").is_none(),
        "首页加入按钮跟着 registration.open"
    );
    assert!(v.get("audit_notice").is_none(), "记录说明已删除");
    assert_eq!(
        v["registration"],
        json!({"configured": true, "open": false, "slots_left": 10})
    );
    assert_eq!(v["me"], Value::Null);
    // 不泄露任何标识
    for secret in [TOKEN, "pst-synthetic-upstream-token", "token_id"] {
        assert!(!text.contains(secret), "看板里不能有 {secret}");
    }
}

#[tokio::test]
async fn public_live_without_registration_service() {
    let e = setup(&[]).await;
    let (_, _, v) = get_json(&e.state, "/public/live", &[]).await;
    assert_eq!(v["members"], json!({}));
    assert_eq!(
        v["registration"],
        json!({"configured": false, "open": false})
    );
    let (_, _, s) = get_json(&e.state, "/public/status", &[]).await;
    assert_eq!(
        s["registration"],
        json!({"open": false, "slots_left": null})
    );
}

#[tokio::test]
async fn public_live_reflects_site_settings_after_meta_cache() {
    let e = setup(&reg_env()).await;
    let long = "额".repeat(400);
    for (k, v) in [
        ("algo_notice", long.as_str()),
        ("register_open", "1"),
        ("register_max_users", "3"),
    ] {
        e.state.db.set_setting(k, v).await.unwrap();
    }
    e.state.live.invalidate();
    let (_, _, v) = get_json(&e.state, "/public/live", &[]).await;
    assert_eq!(v["web_login"], true);
    assert_eq!(v["algo_notice"].as_str().unwrap().chars().count(), 300);
    assert_eq!(
        v["registration"],
        json!({"configured": true, "open": true, "slots_left": 3})
    );
    assert_eq!(v["members"], json!({"max": 3, "active": 0, "open": true}));

    // 30 秒子缓存：改了设置，不 invalidate 就还是旧值
    e.state
        .db
        .set_setting("algo_notice", "新说明")
        .await
        .unwrap();
    let (_, _, again) = get_json(&e.state, "/public/live", &[]).await;
    assert_eq!(again["algo_notice"].as_str().unwrap().chars().count(), 300);
    e.state.live.invalidate();
    let (_, _, fresh) = get_json(&e.state, "/public/live", &[]).await;
    assert_eq!(fresh["algo_notice"], "新说明");
    // 登录开关已删除：库里残留 web_login_paused=1 也不影响
    e.state
        .db
        .set_setting("web_login_paused", "1")
        .await
        .unwrap();
    e.state.live.invalidate();
    let (_, _, fresh) = get_json(&e.state, "/public/live", &[]).await;
    assert_eq!(fresh["web_login"], true);
}

#[tokio::test]
async fn public_rules_announce_restricted_v5_without_exposing_subjects() {
    let e = setup(&reg_env()).await;
    e.state
        .db
        .set_setting(gate_server::v5_access::MODE_KEY, "restricted")
        .await
        .unwrap();
    e.state
        .db
        .set_setting(
            gate_server::v5_access::SUBJECTS_KEY,
            r#"["900000000000000081"]"#,
        )
        .await
        .unwrap();
    e.state.live.invalidate();
    let (_, _, live) = get_json(&e.state, "/public/live", &[]).await;
    // 10-27 起看板不带访问策略字段：策略通过 meta 的 algo_notice 一句话说清
    assert!(
        live["algo_notice"]
            .as_str()
            .unwrap()
            .contains("V5 当前仅授权成员可用")
    );
    assert!(!live.to_string().contains("900000000000000081"));
    assert!(
        !live.to_string().contains("v5_access_mode"),
        "shared live must not carry the policy fields"
    );
}

/// 今天出满的账号（单号每日上限）虽然「可用」，但不再接图：全站所有账号出满时，首页说明原因。
#[tokio::test]
async fn public_live_marks_accounts_at_their_daily_cap() {
    let e = setup(&reg_env()).await;
    let id = e.state.nai.tokens().await[0].token_id.clone();
    e.state
        .db
        .bump_upstream_image_counter(&id, &e.state.day(), 2400)
        .await
        .unwrap();
    e.state.live.invalidate();
    let (_, _, v) = get_json(&e.state, "/public/live", &[]).await;
    assert_eq!(v["guard"]["blocked"]["code"], "daily_cap");
}

#[tokio::test]
async fn public_live_blocked_reason_and_queue() {
    let e = setup(&[]).await;
    let now = e.state.now();
    // 全站熔断：所有可用账号都被站点级规则拦下，首页要说清原因
    e.state
        .guard
        .with(|g| g.trip_breaker(300.0, "", now))
        .unwrap();
    // 有人在排队
    e.state.guard.with(|g| g.admit_image(77, None, now));
    e.state.live.invalidate();
    let (_, _, v) = get_json(&e.state, "/public/live", &[]).await;
    assert_eq!(
        v["queue"],
        json!({"waiting": 1, "running": 0, "full": false})
    );
    assert_eq!(
        v["guard"]["blocked"],
        json!({"code": "breaker", "text": "NovelAI 连续出错，全站暂停出图 300 秒，未扣费。到时自动恢复。"})
    );
    // 没登录的人看不到别人的排队位置
    assert_eq!(v["me"], Value::Null);
}

#[tokio::test]
async fn public_live_me_for_logged_in_member_only() {
    let e = setup(&reg_env()).await;
    let cookie = register_member(&e.state).await;
    let now = e.state.now();
    // 实测每张 2.5 秒（测试环境出完不歇）：周期 2.5 秒，排队超时 5 秒内能开始 ⌈5 ÷ 2.5⌉ = 2 张
    e.state.nai.tokens().await[0].set_gen_secs(2.5);
    e.state.guard.with(|g| g.admit_image(77, None, now - 5.0)); // 别人先到
    e.state.guard.with(|g| g.admit_image(1, None, now)); // 测试成员排第 2
    e.state.live.invalidate();

    let (_, _, guest) = get_json(&e.state, "/public/live", &[]).await;
    assert_eq!(guest["me"], Value::Null);
    // 排队上限是 2：两张就满了
    assert_eq!(
        guest["queue"],
        json!({"waiting": 2, "running": 0, "full": true})
    );

    let (status, headers, v) = get_json(&e.state, "/public/live", &[("cookie", &cookie)]).await;
    assert_eq!(status, 200);
    assert_eq!(headers["cache-control"], "no-store");
    // 排第 2（10-27 起不再给时间估算）
    assert_eq!(
        v["me"],
        json!({"mine": [{"state": "waiting", "position": 2}], "waiting": 2, "running": 0})
    );
    // 共享快照没有被某个成员的 me 污染（旧版红线）
    let (_, _, guest_again) = get_json(&e.state, "/public/live", &[]).await;
    assert_eq!(guest_again["me"], Value::Null);

    // Key 被停用：不再显示「我的排队」
    let fields = json!({"enabled": false});
    e.state
        .db
        .update_key(1, fields.as_object().unwrap())
        .await
        .unwrap();
    let (_, _, off) = get_json(&e.state, "/public/live", &[("cookie", &cookie)]).await;
    assert_eq!(off["me"], Value::Null);
}

// ------------------------------------------------------------------ /public/status

#[tokio::test]
async fn public_status_exposes_only_safe_fields() {
    let e = setup(&reg_env()).await;
    let (status, headers, v) = get_json(&e.state, "/public/status", &[]).await;
    assert_eq!(status, 200);
    assert_eq!(headers["cache-control"], "no-store");
    // 键集合与键顺序都与旧版实际响应一致
    assert_eq!(
        keys(&v),
        [
            "site",
            "upstream",
            "registration",
            "default_features",
            "web_login",
            "algo_notice",
            "discord_invite",
            "key_inactivity_delete_days",
            "limits",
            "image_jobs",
            "stability"
        ]
    );
    assert_eq!(v["site"], "https://gate.example.top");
    assert_eq!(v["discord_invite"], "https://discord.gg/abc");
    assert_eq!(
        v["upstream"],
        json!({"status": "idle", "image_cooldown_seconds": 0})
    );
    assert_eq!(v["registration"], json!({"open": false, "slots_left": 10}));
    assert_eq!(
        v["default_features"],
        json!([{"id": "image", "label": "文生图"}])
    );
    assert_eq!(
        v["limits"],
        // 保底 `base_daily_images` 已删除
        json!({"key_image_queue": 2})
    );
    assert_eq!(v["image_jobs"], 0);
    assert_eq!(v["stability"], json!({"ok": 0, "error": 0}));
    assert_eq!(v["web_login"], true);
    assert_eq!(v["key_inactivity_delete_days"], 3);

    // 8 秒缓存；invalidate 后看到新值
    for (k, val) in [
        ("register_features", "*"),
        ("register_open", "1"),
        ("register_max_users", "5"),
    ] {
        e.state.db.set_setting(k, val).await.unwrap();
    }
    let (_, _, cached) = get_json(&e.state, "/public/status", &[]).await;
    assert_eq!(cached, v);
    e.state.live.invalidate();
    let _cookie = register_member(&e.state).await; // 占 1 个名额
    let (_, _, v2) = get_json(&e.state, "/public/status", &[]).await;
    assert_eq!(v2["registration"], json!({"open": true, "slots_left": 4}));
    let ids: Vec<&str> = v2["default_features"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["image", "upscale", "augment", "vibe", "tags"]);

    // 上游状态
    for _ in 0..4 {
        e.state.record_upstream(false);
    }
    e.state.cooldown.block(30.0, e.state.now());
    e.state.live.invalidate();
    let (_, _, v3) = get_json(&e.state, "/public/status", &[]).await;
    assert_eq!(v3["upstream"]["status"], "degraded");
    assert!(v3["upstream"]["image_cooldown_seconds"].as_i64().unwrap() > 0);
    // 只有 status 和冷却秒数，没有次数
    assert_eq!(keys(&v3["upstream"]), ["status", "image_cooldown_seconds"]);
}

// ------------------------------------------------------------------ /queue-status

#[tokio::test]
async fn queue_status_shape() {
    let e = setup(&[]).await;
    let (status, _, body) = raw(&e.state, "GET", "/queue-status", &[], b"").await;
    assert_eq!(status, 200);
    assert_eq!(
        String::from_utf8(body).unwrap(),
        r#"{"global":{"active":0,"waiting":0,"concurrency":1},"image_next_slot_in":0.0,"image_cooldown_remaining":0,"queue_timeout":5}"#
    );
    // 上游 429 之后冷却秒数出现
    e.state.cooldown.block(30.0, e.state.now());
    let (_, _, v) = get_json(&e.state, "/queue-status", &[]).await;
    assert!(v["image_cooldown_remaining"].as_i64().unwrap() > 0);
}

// ------------------------------------------------------------------ 客户端兼容视图

#[tokio::test]
async fn user_compat_routes_require_key_and_keep_panel_schema() {
    let e = setup(&[]).await;
    let now = e.state.now();
    for prefix in ["", "/ai", "/v1"] {
        for leaf in [
            "subscription",
            "data",
            "priority",
            "keystore",
            "information",
        ] {
            if prefix == "/v1" && !matches!(leaf, "subscription" | "information") {
                continue;
            }
            let path = format!("{prefix}/user/{leaf}");
            let (status, _, body) = Call::new("GET", &path).send(&e.state).await;
            assert_eq!(status, 401, "{path}");
            assert_eq!(
                json_of(&body)["message"],
                "猫头鹰公益站提醒：未收到 API Key。请在客户端中填入本站发放的 Key。开发者参考：放在 Authorization: Bearer 请求头中",
                "{path}"
            );
            let (status, _, body) = Call::new("GET", &path)
                .bearer("nai-wrong")
                .send(&e.state)
                .await;
            assert_eq!(status, 401, "{path}");
            assert_eq!(
                json_of(&body)["error"]["message"],
                "猫头鹰公益站提醒：API Key 无效。请检查客户端中的 Key 是否完整，或在 Discord 用 /resetkey 重新获取。"
            );
        }
    }
    let get = |path: &'static str| {
        let state = e.state.clone();
        async move {
            let (s, _, b) = Call::new("GET", path).bearer(TOKEN).send(&state).await;
            (s, b)
        }
    };

    for path in [
        "/user/subscription",
        "/ai/user/subscription",
        "/v1/user/subscription",
    ] {
        let (status, body) = get(path).await;
        assert_eq!(status, 200, "{path}");
        let text = String::from_utf8(body.clone()).unwrap();
        assert!(!text.contains(TOKEN), "订阅视图不能回显 Key");
        let v = json_of(&body);
        assert_eq!(
            keys(&v),
            [
                "tier",
                "active",
                "expiresAt",
                "trainingStepsLeft",
                "perks",
                "naiGate"
            ]
        );
        assert_eq!(v["tier"], 3);
        assert_eq!(v["active"], true);
        let expires = v["expiresAt"].as_f64().unwrap();
        assert!(
            (expires - (now + 3650.0 * 86400.0)).abs() < 86400.0,
            "没有到期时间 = 十年后"
        );
        assert_eq!(
            v["trainingStepsLeft"],
            json!({"fixedTrainingStepsLeft": 0, "purchasedTrainingSteps": 0})
        );
        assert_eq!(
            v["perks"],
            json!({"imageGeneration": true, "unlimitedImageGeneration": false, "voiceGeneration": false, "contextTokens": 0})
        );
        assert_eq!(
            v["naiGate"],
            json!({
                "imageModelScope": "legacy",
                "quotaLimits": {"images": {"mode": "inherit"}, "v5": {"mode": "inherit"}, "daily_anlas": {"mode": "inherit"}, "monthly_anlas": {"mode": "unlimited"}},
                "v45DailyLimit": 150, "v45Unlimited": false, "dailyAnlasLimit": 50.0, "dailyAnlasUnlimited": false,
                "anlasEnabled": false, "anlasLeft": 0, "anlasMonthlyLimit": 0, "anlasUnlimited": true,
                "v5LeftToday": 150, "v5DailyLimit": 150, "v5Unlimited": false, "v5Paused": false, "v5Allowed": true, "account": null
            })
        );
    }

    // 用掉的 V5 体现在 v5LeftToday 里
    common::add_usage(&e.state.db, 1, &e.state.day(), 3, 0.0).await;
    let (_, body) = get("/user/subscription").await;
    assert_eq!(json_of(&body)["naiGate"]["v5LeftToday"], 147);

    for path in ["/user/data", "/ai/user/data"] {
        let (status, body) = get(path).await;
        assert_eq!(status, 200, "{path}");
        let v = json_of(&body);
        assert_eq!(
            keys(&v),
            [
                "subscription",
                "priority",
                "trainingStepsLeft",
                "anlas",
                "v5Access"
            ]
        );
        assert_eq!(
            keys(&v["subscription"]),
            ["tier", "active", "expiresAt", "trainingStepsLeft", "perks"],
            "naiGate 不暴露在 data 里"
        );
        assert_eq!(
            v["v5Access"],
            json!({"mode": "legacy", "allowed": true, "paused": false})
        );
        assert_eq!(
            keys(&v["priority"]),
            ["maxPriorityActions", "nextRefillAt", "taskPriority"]
        );
        assert_eq!(
            v["trainingStepsLeft"],
            v["subscription"]["trainingStepsLeft"]
        );
        assert_eq!(v["anlas"], 0);
    }

    for path in ["/user/priority", "/ai/user/priority"] {
        let (status, body) = get(path).await;
        assert_eq!(status, 200, "{path}");
        let v = json_of(&body);
        assert_eq!(v["maxPriorityActions"], 0);
        assert_eq!(v["taskPriority"], 10);
        let next = v["nextRefillAt"].as_i64().unwrap();
        assert_eq!(next % 3600, 0, "下个整点刷新");
        assert!(next as f64 > now && next as f64 <= now + 3601.0);
    }

    for path in [
        "/user/information",
        "/ai/user/information",
        "/v1/user/information",
    ] {
        let (status, body) = get(path).await;
        assert_eq!(status, 200, "{path}");
        let v = json_of(&body);
        assert_eq!(keys(&v), ["tier", "active", "username", "expiresAt"]);
        assert_eq!(v["username"], "测试成员");
        assert_eq!(v["tier"], 3);
        assert!(v["expiresAt"].is_i64());
    }

    // 密钥库：照官方对持久令牌的说法回 403
    for path in ["/user/keystore", "/ai/user/keystore"] {
        let (status, body) = get(path).await;
        assert_eq!(status, 403, "{path}");
        let v = json_of(&body);
        assert_eq!(v["statusCode"], 403);
        assert_eq!(
            v["message"],
            "猫头鹰公益站提醒：该功能仅限 NovelAI 官网使用，本站不提供，未扣费。开发者参考：NovelAI 官方对持久令牌同样返回 403"
        );
        assert_eq!(v["error"]["status"], 403);
        assert_eq!(v["error"]["message"], v["message"]);
    }
    // 403 被记成一条「rejected」（旧版 GateError 处理器的统一记账）
    settle().await;
    let rows = e
        .state
        .db
        .list_logs(50, 0, &LogFilter::default())
        .await
        .unwrap();
    assert!(
        rows.iter()
            .any(|r| r["status"] == "rejected" && r["kind"] == "account"),
        "{rows:?}"
    );
}

#[tokio::test]
async fn user_data_and_subscription_report_restricted_v5_permission() {
    let e = setup(&reg_env()).await;
    e.state
        .db
        .run_script(vec![
            "UPDATE api_keys SET discord_id='900000000000000091' WHERE id=1".into(),
        ])
        .await
        .unwrap();
    e.state
        .db
        .set_setting(gate_server::v5_access::MODE_KEY, "restricted")
        .await
        .unwrap();
    e.state
        .db
        .set_setting(
            gate_server::v5_access::SUBJECTS_KEY,
            r#"["900000000000000092"]"#,
        )
        .await
        .unwrap();
    let (_, subscription) = get_with_auth(&e.state, "/user/subscription", TOKEN).await;
    let subscription = json_of(&subscription);
    assert_eq!(subscription["naiGate"]["v5Allowed"], false);
    assert_eq!(subscription["naiGate"]["v5Unlimited"], false);
    assert_eq!(subscription["naiGate"]["v5LeftToday"], 0);
    let (_, data) = get_with_auth(&e.state, "/user/data", TOKEN).await;
    assert_eq!(
        json_of(&data)["v5Access"],
        json!({"mode":"restricted", "allowed":false, "paused":false})
    );
}

#[tokio::test]
async fn user_information_uses_key_expiry() {
    let e = setup(&[]).await;
    let fields = json!({"expires_at": 1_900_000_000.9});
    e.state
        .db
        .update_key(1, fields.as_object().unwrap())
        .await
        .unwrap();
    let (_, _, body) = Call::new("GET", "/user/information")
        .bearer(TOKEN)
        .send(&e.state)
        .await;
    assert_eq!(json_of(&body)["expiresAt"], 1_900_000_000);
    let (_, _, body) = Call::new("GET", "/user/subscription")
        .bearer(TOKEN)
        .send(&e.state)
        .await;
    let v = json_of(&body);
    assert_eq!(v["expiresAt"], 1_900_000_000);
    assert_eq!(v["naiGate"].get("isAdmin"), None);
}

/// 设备指纹 id 由服务端按各项算：同样的项（顺序、数字 / 字符串写法不同）得到同一个 id；
/// 前端自带的 id 一律不认；别的网站发来的不收。
#[tokio::test]
async fn device_fp_id_is_computed_by_the_server() {
    let e = setup(&[]).await;
    let same_origin = [
        ("host", "gate.example.top"),
        ("origin", "https://gate.example.top"),
        ("content-type", "application/json"),
    ];
    let post = |body: Value| {
        let state = e.state.clone();
        async move {
            let (s, _, b) = raw(
                &state,
                "POST",
                "/public/device-fp",
                &same_origin,
                body.to_string().as_bytes(),
            )
            .await;
            (s, b)
        }
    };
    let (s, b) =
        post(json!({"data": {"ua": "UA", "tz": -480, "hw": 8, "canvas": "c1", "webgl": "w1"}}))
            .await;
    assert_eq!(s, 200);
    let id = json_of(&b)["id"].as_str().unwrap().to_string();
    assert_eq!(id.len(), 32);
    assert!(
        id.chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
        "{id}"
    );
    assert!(e.state.db.has_device_fingerprint(&id).await.unwrap());
    // 同样的项换个顺序、数字写成字符串，还是同一个 id；前端自带的 id 被忽略
    let (s, b) = post(json!({"id": "f".repeat(32), "data": {"webgl": "w1", "canvas": "c1", "hw": "8", "tz": "-480", "ua": "UA"}})).await;
    assert_eq!(s, 200);
    assert_eq!(json_of(&b)["id"], json!(id));
    assert!(
        !e.state
            .db
            .has_device_fingerprint(&"f".repeat(32))
            .await
            .unwrap()
    );
    // 任何一项不同就是另一个 id
    let (_, b) =
        post(json!({"data": {"ua": "UA", "tz": -480, "hw": 8, "canvas": "c2", "webgl": "w1"}}))
            .await;
    assert_ne!(json_of(&b)["id"], json!(id));
    // 只带 id、没有可用的项：204，不存
    let (s, b) = post(json!({"id": "a".repeat(32)})).await;
    assert_eq!((s, b.is_empty()), (StatusCode::NO_CONTENT, true));
    let (s, _) = post(json!({"data": {"x": ""}})).await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    assert!(
        !e.state
            .db
            .has_device_fingerprint(&"a".repeat(32))
            .await
            .unwrap()
    );
    // 别的网站发来的：204，不存
    let (s, _, _) = raw(
        &e.state,
        "POST",
        "/public/device-fp",
        &[
            ("host", "gate.example.top"),
            ("origin", "https://evil.example"),
        ],
        json!({"data": {"ua": "evil"}}).to_string().as_bytes(),
    )
    .await;
    assert_eq!(s, StatusCode::NO_CONTENT);
}

/// 浏览器标记 owl_bid：没有就新发一个（32 位小写十六进制）；Cookie 在就沿用并续期；
/// 没有 Cookie 时认请求体里前端 localStorage 存的 bid；格式不对的一律不认、另发新的。
#[tokio::test]
async fn device_fp_sets_and_keeps_browser_cookie() {
    let e = setup(&[]).await;
    let post = |cookie: Option<String>, body: Value| {
        let state = e.state.clone();
        async move {
            let mut headers = vec![
                ("host", "gate.example.top".to_string()),
                ("origin", "https://gate.example.top".to_string()),
                ("content-type", "application/json".to_string()),
            ];
            if let Some(c) = cookie {
                headers.push(("cookie", c));
            }
            let headers: Vec<(&str, &str)> =
                headers.iter().map(|(k, v)| (*k, v.as_str())).collect();
            let (s, h, b) = raw(
                &state,
                "POST",
                "/public/device-fp",
                &headers,
                body.to_string().as_bytes(),
            )
            .await;
            let set: Vec<String> = h
                .get_all("set-cookie")
                .iter()
                .map(|v| v.to_str().unwrap().to_string())
                .collect();
            (s, set, b)
        }
    };
    let data = json!({"ua": "UA", "canvas": "c1"});
    let bid_of = |set: &[String]| -> String {
        assert_eq!(set.len(), 1, "{set:?}");
        let c = &set[0];
        assert_eq!(
            c.split_once(';').unwrap().1,
            " HttpOnly; Max-Age=34560000; Path=/; SameSite=lax; Secure",
            "{c}"
        );
        c.strip_prefix("owl_bid=")
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_string()
    };
    let is_bid =
        |b: &str| b.len() == 32 && b.bytes().all(|c| matches!(c, b'0'..=b'9' | b'a'..=b'f'));
    // 第一次：新发
    let (s, set, b) = post(None, json!({"data": data})).await;
    assert_eq!(s, 200);
    let bid = bid_of(&set);
    assert!(is_bid(&bid), "{bid}");
    assert_eq!(json_of(&b)["bid"], json!(bid));
    // Cookie 在：沿用（请求体里的 bid 被忽略）
    let other = "0123456789abcdef0123456789abcdef";
    let (_, set, b) = post(
        Some(format!("owl_bid={bid}")),
        json!({"data": data, "bid": other}),
    )
    .await;
    assert_eq!(
        (bid_of(&set), json_of(&b)["bid"].clone()),
        (bid.clone(), json!(bid))
    );
    // 只清了 Cookie：认 localStorage 带回来的 bid
    let (_, set, b) = post(None, json!({"data": data, "bid": other})).await;
    assert_eq!(
        (bid_of(&set), json_of(&b)["bid"].clone()),
        (other.to_string(), json!(other))
    );
    // 格式不对（大写 / 太短 / 非十六进制 / 不是字符串）：不认，另发新的
    for bad in [
        json!(other.to_uppercase()),
        json!("abc"),
        json!("z".repeat(32)),
        json!(12345),
    ] {
        let (_, set, _) = post(None, json!({"data": data, "bid": bad})).await;
        let got = bid_of(&set);
        assert!(is_bid(&got) && got != other && got != bid, "{got}");
    }
    // Cookie 格式不对：同样不认
    let (_, set, _) = post(Some("owl_bid=XYZ".into()), json!({"data": data})).await;
    let got = bid_of(&set);
    assert!(is_bid(&got) && got != "XYZ", "{got}");
}

// ------------------------------------------------------------------ SSE

#[tokio::test]
async fn live_stream_first_event_changes_and_keepalive() {
    let e = setup(&[]).await;
    e.state.live.set_keepalive(Duration::from_millis(150));
    let (status, headers, mut sse) = Sse::open(&e.state, "/public/live/stream", &[]).await;
    assert_eq!(status, 200);
    assert_eq!(headers["content-type"], "text/event-stream");
    assert_eq!(headers["cache-control"], "no-store");
    assert_eq!(headers["x-accel-buffering"], "no");
    assert!(headers.get("x-request-id").is_some());
    assert_eq!(
        e.state
            .live
            .open_streams(gate_server::live::StreamKind::Public),
        1
    );

    // 连上立刻推第一条：和 /public/live 同一份数据（t 除外）
    let first = sse
        .next_data(Duration::from_secs(5))
        .await
        .expect("第一条事件");
    let (_, _, polled) = get_json(&e.state, "/public/live", &[]).await;
    assert_eq!(keys(&first), keys(&polled));
    assert_eq!(first["me"], Value::Null);
    assert_eq!(without_t(first.clone()), without_t(polled));

    // 内容没变就不推，只有保活注释（`: keep-alive`）
    let block = sse
        .next(Duration::from_secs(3))
        .await
        .expect("没等到保活注释");
    assert_eq!(block, ": keep-alive", "内容没变就不该推事件");

    // 有变化：下一秒的快照推过来
    e.state.live_note("rejected");
    let changed = sse
        .next_data(Duration::from_secs(5))
        .await
        .expect("变化后的事件");
    assert_eq!(changed["auth"], json!({"requests": 1, "rejected": 1}));

    // 关停：流结束，后台任务退出
    e.state.shutdown.cancel();
    loop {
        match sse.next(Duration::from_secs(3)).await {
            None => break,
            Some(_) => continue, // 在途的保活 / 事件
        }
    }
    e.state.tasks.close();
    tokio::time::timeout(Duration::from_secs(5), e.state.tasks.wait())
        .await
        .expect("后台任务应随 shutdown 退出");
    drop(sse);
    assert_eq!(
        e.state
            .live
            .open_streams(gate_server::live::StreamKind::Public),
        0
    );
}

#[tokio::test]
async fn live_stream_carries_my_queue_per_connection() {
    let e = setup(&reg_env()).await;
    let cookie = register_member(&e.state).await;
    let (_, _, mut member) =
        Sse::open(&e.state, "/public/live/stream", &[("cookie", &cookie)]).await;
    let (_, _, mut guest) = Sse::open(&e.state, "/public/live/stream", &[]).await;
    let m1 = member.next_data(Duration::from_secs(5)).await.unwrap();
    let g1 = guest.next_data(Duration::from_secs(5)).await.unwrap();
    assert_eq!(m1["me"], json!({"mine": [], "waiting": 0, "running": 0}));
    assert_eq!(g1["me"], Value::Null);

    let now = e.state.now();
    e.state.guard.with(|g| g.admit_image(1, None, now));
    let m2 = member.next_data(Duration::from_secs(5)).await.unwrap();
    assert_eq!(
        m2["me"],
        json!({"mine": [{"state": "waiting", "position": 1}], "waiting": 1, "running": 0})
    );
    // 访客那条流看到的是全站数字，没有任何人的位置
    let g2 = guest.next_data(Duration::from_secs(5)).await.unwrap();
    assert_eq!(
        g2["queue"],
        json!({"waiting": 1, "running": 0, "full": false})
    );
    assert_eq!(g2["me"], Value::Null);
}

#[tokio::test]
async fn admin_events_requires_admin_session() {
    let e = setup(&[]).await;
    let (status, _, body) = raw(&e.state, "GET", "/admin/api/events", &[], b"").await;
    assert_eq!(status, 401);
    assert_eq!(json_of(&body), json!({"detail": "未登录或会话已过期"}));
    let (status, _, _) = raw(
        &e.state,
        "GET",
        "/admin/api/events",
        &[("cookie", "nai_gate_admin=1.deadbeef")],
        b"",
    )
    .await;
    assert_eq!(status, 401);
    // 成员 / Key 都不是后台会话
    let (status, _, _) = raw(
        &e.state,
        "GET",
        "/admin/api/events",
        &[("authorization", &format!("Bearer {TOKEN}"))],
        b"",
    )
    .await;
    assert_eq!(status, 401);
    assert_eq!(
        e.state
            .live
            .open_streams(gate_server::live::StreamKind::Admin),
        0
    );
}

#[tokio::test]
async fn admin_events_stream_snapshot_and_badges() {
    let e = setup(&[]).await;
    e.state.live.set_keepalive(Duration::from_millis(150));
    let cookie = admin_cookie(&e.state);
    let (status, headers, mut sse) =
        Sse::open(&e.state, "/admin/api/events", &[("cookie", &cookie)]).await;
    assert_eq!(status, 200);
    assert_eq!(headers["content-type"], "text/event-stream");
    assert_eq!(headers["cache-control"], "no-store");
    assert_eq!(headers["x-accel-buffering"], "no");
    assert_eq!(headers["x-content-type-options"], "nosniff");

    let first = sse
        .next_data(Duration::from_secs(5))
        .await
        .expect("第一条事件");
    assert_eq!(keys(&first), ["live", "badges"]);
    assert_eq!(first["badges"], json!({}));
    // live 是同一份看板快照（不含任何成员的 me）
    let (_, _, polled) = get_json(&e.state, "/public/live", &[]).await;
    let live = without_t(first["live"].clone());
    assert!(live.get("me").is_none());
    let mut want = without_t(polled);
    want.as_object_mut().unwrap().shift_remove("me");
    assert_eq!(live, want);

    // 内容没变：只有保活注释，不会让后台页面白白刷新
    match sse.next(Duration::from_secs(2)).await {
        Some(b) => assert!(b.starts_with(':'), "没有变化却推了：{b}"),
        None => panic!("应该有保活"),
    }

    // 真有变化（队列里多了一张图）才推，隔至少 15 秒（后台每次刷新会重拉几页数据）
    let t0 = tokio::time::Instant::now();
    let now = e.state.now();
    e.state.guard.with(|g| g.admit_image(9, None, now));
    let second = sse
        .next_data(Duration::from_secs(20))
        .await
        .expect("变化后的事件");
    assert_eq!(
        second["live"]["queue"],
        json!({"waiting": 1, "running": 0, "full": false})
    );
    assert!(t0.elapsed() < Duration::from_secs(20));

    // 退出登录 / 改密码使会话失效：流结束，重连会拿到 401
    e.state
        .db
        .set_setting("admin_password_hash", "x")
        .await
        .unwrap();
    *e.state.admin_pw_hash.write().unwrap() = Some(session::hash_password("another-password"));
    loop {
        match sse.next(Duration::from_secs(4)).await {
            None => break,
            Some(_) => continue,
        }
    }
    assert_eq!(
        e.state
            .live
            .open_streams(gate_server::live::StreamKind::Admin),
        0
    );
}

// ------------------------------------------------------------------ 首页首屏数据

/// 最小的 landing.html（含 `<!--owl-boot-->` 占位）：`web/dist` 在测试里不一定构建过。
const LANDING_HTML: &str =
    "<!doctype html><html><head><!--owl-boot--></head><body><div id=\"app\"></div></body></html>";

fn boot_of(html: &str) -> Value {
    let open = "<script type=\"application/json\" id=\"owl-boot\">";
    let start = html.find(open).expect("没有注入 owl-boot") + open.len();
    let end = html[start..].find("</script>").unwrap() + start;
    // 不能出现能提前闭合标签的字符
    let raw = &html[start..end];
    assert!(!raw.contains('<') && !raw.contains('>') && !raw.contains('&'));
    json_of(raw.as_bytes())
}

#[tokio::test]
async fn landing_boot_for_guest() {
    let p = setup_with_static(&reg_env(), Some(LANDING_HTML)).await;
    let (status, headers, body) = raw(&p.state, "GET", "/", &[], b"").await;
    assert_eq!(status, 200);
    let html = String::from_utf8(body).unwrap();
    // 页面头与不注入时一样：严格 CSP、按登录态区分
    assert_eq!(headers["content-type"], "text/html; charset=utf-8");
    assert!(
        headers["content-security-policy"]
            .to_str()
            .unwrap()
            .contains("script-src 'self'")
    );
    assert_eq!(headers["vary"], "Cookie");
    assert_eq!(headers["cache-control"], "no-cache");
    let boot = boot_of(&html);
    assert_eq!(keys(&boot), ["me", "live", "status"]);
    assert_eq!(boot["me"], json!({"logged_in": false}));
    // live / status 与对应接口同形状
    let (_, _, live) = get_json(&p.state, "/public/live", &[]).await;
    assert_eq!(keys(&boot["live"]), keys(&live));
    assert_eq!(boot["live"]["me"], Value::Null);
    assert_eq!(boot["live"]["site"], "https://gate.example.top");
    assert_eq!(boot["live"]["quota"]["images_today"], 0);
    assert_eq!(boot["status"]["site"], "https://gate.example.top");
    assert_eq!(boot["status"]["web_login"], true);
    assert_eq!(
        boot["status"]["registration"],
        json!({"open": false, "slots_left": 10})
    );
}

#[tokio::test]
async fn landing_boot_for_member_with_key() {
    let p = setup_with_static(&reg_env(), Some(LANDING_HTML)).await;
    let cookie = register_member(&p.state).await;
    let now = p.state.now();
    p.state.guard.with(|g| g.admit_image(1, None, now));
    let (status, _, body) = raw(&p.state, "GET", "/", &[("cookie", &cookie)], b"").await;
    assert_eq!(status, 200);
    let boot = boot_of(&String::from_utf8(body).unwrap());
    let me = &boot["me"];
    assert_eq!(
        keys(me),
        [
            "logged_in",
            "discord",
            "has_key",
            "key",
            "name",
            "expires_at",
            "image_scope",
            "subject_roles",
            "today",
            "queue",
            "coupons",
            "timing"
        ]
    );
    assert_eq!(me["logged_in"], true);
    assert_eq!(
        me["discord"],
        json!({"id": MEMBER_ID, "username": "member_one", "display_name": "成员甲", "avatar": "avatarhash"})
    );
    assert_eq!(me["has_key"], true);
    assert_eq!(me["key"], TOKEN);
    assert_eq!(me["name"], "测试成员");
    assert_eq!(me["expires_at"], Value::Null);
    assert_eq!(me["image_scope"], "legacy");
    assert_eq!(me["subject_roles"], json!({"owner": false, "admin": false}));
    assert_eq!(
        me["today"],
        json!({
            "images": 0, "v5": 0, "legacy": 0, "daily_images": 100, "daily_v5": 10,
            "quota_limits": {"images": {"mode": "inherit"}, "v5": {"mode": "inherit"}, "daily_anlas": {"mode": "inherit"}, "monthly_anlas": {"mode": "unlimited"}},
            "quota_limits_valid": true,
            "site_v5_left": 150, "site_v5_cap": 150, "upstream_v5_free": true, "v5_paused": false,
            "v5_access_allowed": true, "v5_access_reason": "legacy policy",
            "anlas": 0, "daily_anlas": 50, "reset_hour": 7
        })
    );
    assert_eq!(me["queue"], json!([{"state": "waiting", "position": 1}]));
    assert_eq!(me["coupons"], json!([]));
    assert_eq!(
        me["timing"],
        json!({"count": 0, "avg_dur_s": null, "avg_wait_s": null, "site_avg_dur_s": 9.0})
    );
    // 看板里带我的排队
    assert_eq!(
        boot["live"]["me"],
        json!({"mine": [{"state": "waiting", "position": 1}], "waiting": 1, "running": 0})
    );
    // 全站 V5 池停用（global_daily_v5=0，10-27 语义）：首页 V5 圆环显示 0 / 0
    p.state
        .db
        .set_setting("global_daily_v5", "0")
        .await
        .unwrap();
    let (_, _, body) = raw(&p.state, "GET", "/", &[("cookie", &cookie)], b"").await;
    let today = &boot_of(&String::from_utf8(body).unwrap())["me"]["today"];
    assert_eq!(
        (&today["site_v5_left"], &today["site_v5_cap"]),
        (&json!(0), &json!(0))
    );
}

#[tokio::test]
async fn landing_boot_for_member_without_key_and_bad_session() {
    let p = setup_with_static(&reg_env(), Some(LANDING_HTML)).await;
    // 登录了但没有领 Key：会话里的 Key 指纹是 "-"
    let exp = p.state.now() as i64 + 86400;
    let cookie = format!(
        "owl_member={}",
        session::sign_member(&p.state, MEMBER_ID, exp, "-")
    );
    let (_, _, body) = raw(&p.state, "GET", "/", &[("cookie", &cookie)], b"").await;
    let boot = boot_of(&String::from_utf8(body).unwrap());
    assert_eq!(
        boot["me"],
        json!({"logged_in": true, "discord": {"id": MEMBER_ID}, "has_key": false})
    );
    assert_eq!(boot["live"]["me"], Value::Null);

    // 伪造的会话 = 访客
    let (_, _, body) = raw(
        &p.state,
        "GET",
        "/",
        &[("cookie", "owl_member=900000000000000001:1:-.deadbeef")],
        b"",
    )
    .await;
    assert_eq!(
        boot_of(&String::from_utf8(body).unwrap())["me"],
        json!({"logged_in": false})
    );
}

#[tokio::test]
async fn landing_boot_escapes_markup() {
    let p = setup_with_static(&[], Some(LANDING_HTML)).await;
    p.state
        .db
        .set_setting("algo_notice", "</script><script>alert(1)</script> & <b>")
        .await
        .unwrap();
    p.state.live.invalidate();
    let (_, _, body) = raw(&p.state, "GET", "/", &[], b"").await;
    let html = String::from_utf8(body).unwrap();
    assert!(!html.contains("<script>alert"));
    let boot = boot_of(&html);
    assert_eq!(
        boot["status"]["algo_notice"],
        "</script><script>alert(1)</script> & <b>"
    );
}

// ------------------------------------------------------------------ 404 分流

/// 浏览器（Accept: text/html）拿到可读的 HTML 404；API / 客户端仍拿到原来的 JSON 文案。
#[tokio::test]
async fn browser_gets_html_404_but_clients_still_get_json() {
    let e = setup(&[]).await;
    let browser = "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8";

    // 旧链接（/terms 已下线）：浏览器不再看到 JSON
    let (status, headers, body) = raw(&e.state, "GET", "/terms", &[("accept", browser)], b"").await;
    assert_eq!(status, 404);
    assert_eq!(headers["content-type"], "text/html; charset=utf-8");
    let html = String::from_utf8(body).unwrap();
    assert!(html.contains("猫头鹰公益站"), "{html}");
    assert!(html.contains(r#"href="/""#), "{html}");
    assert!(html.contains(r#"href="/status""#), "{html}");
    // CSP 与首页一致：不允许内联样式 / 脚本，所以 404 页用系统默认样式
    assert!(
        headers["content-security-policy"]
            .to_str()
            .unwrap()
            .contains("style-src 'self'")
    );
    assert_eq!(headers["x-content-type-options"], "nosniff");
    assert!(!html.contains(" style="), "404 页不应有 style 属性");
    assert!(!html.contains("<style"), "404 页不应有 <style>");
    assert!(!html.contains("<script"), "404 页不应有脚本");

    // API / 客户端：保持 JSON 契约与中文提示
    for (path, accept, hint) in [
        ("/v1/models", "application/json", "不兼容 OpenAI 格式"),
        ("/v1", "*/*", "不兼容 OpenAI 格式"),
        (
            "/ai/generate",
            "application/json",
            "文本生成和语音接口已停用",
        ),
        ("/nope", "application/json", "Not Found"),
    ] {
        let (status, headers, body) = raw(&e.state, "GET", path, &[("accept", accept)], b"").await;
        assert_eq!(status, 404, "{path}");
        assert_eq!(headers["content-type"], "application/json", "{path}");
        let detail = gate_server::routes::not_found_message(path);
        assert!(detail.contains(hint), "{path}: {detail}");
        assert_eq!(
            json_of(&body),
            json!({"detail": detail}),
            "{path} 的 JSON 文案不能变"
        );
    }
}
