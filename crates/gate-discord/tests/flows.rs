//! 命令流程：同样的交互进来，发给网关桥接的请求、发给 Discord 的请求（路径 / 查询 / JSON）和回给成员的文字
//! 必须是下面写的这样。网关和 Discord 都换成 `common::MockServer`（本机假服务），测试里不连任何真实的 Discord。
//!
//! 管理员命令（`/open /limit /ban /unban /slots /revoke`）和 @奶妹 的自动回复已删除，
//! `/quota`、`/help` 10-23 删除（额度看成员首页）。

mod common;

use std::time::Duration;

use common::{APP, BOT_ID, GUILD, MEMBER, MockServer, SECRET, bridge_paths, member, naimei};
use gate_discord::events::{Interaction, ReadyInfo, User};
use gate_discord::handlers::{
    GENERIC_ERROR, REGISTER_NOT_ELIGIBLE, REGISTER_UNAVAILABLE, REGISTER_WRONG_GUILD,
};
use serde_json::{Value, json};

/// 斜杠命令（在指定服务器里）。
fn slash(n: u32, name: &str, options: Value) -> Interaction {
    in_guild(n, name, options, GUILD)
}

fn in_guild(n: u32, name: &str, options: Value, guild: &str) -> Interaction {
    serde_json::from_value(json!({
        "id": format!("9000000000000003{n:02}"), "application_id": APP, "type": 2, "token": format!("tok-{n}"),
        "guild_id": guild, "channel_id": "900000000000000004", "member": member(),
        "data": {"id": "900000000000000050", "name": name, "type": 1, "guild_id": guild, "options": options},
    }))
    .unwrap()
}

fn callback_path(n: u32) -> String {
    format!("/interactions/9000000000000003{n:02}/tok-{n}/callback")
}

fn followup_path(n: u32) -> String {
    format!("/webhooks/{APP}/tok-{n}")
}

/// 成员看到的跟进消息：只有本人可见（flags 64）。
fn ephemeral(content: &str) -> Value {
    json!({"content": content, "tts": false, "flags": 64})
}

/// 一次正常的「先确认、再跟进」：返回跟进消息的内容，同时检查确认的请求形状。
fn expect_deferred_reply(server: &MockServer, n: u32) -> String {
    let calls = server.discord_log();
    assert_eq!(calls.len(), 2, "{calls:?}");
    assert_eq!(calls[0]["method"], "POST");
    assert_eq!(calls[0]["path"], callback_path(n));
    assert_eq!(calls[0]["query"], json!({"with_response": "true"}));
    // 斜杠命令：类型 5（正在思考），只有本人可见
    assert_eq!(calls[0]["json"], json!({"type": 5, "data": {"flags": 64}}));
    assert_eq!(calls[1]["method"], "POST");
    assert_eq!(calls[1]["path"], followup_path(n));
    assert_eq!(
        calls[1]["query"],
        json!({"wait": "true", "with_components": "false"})
    );
    assert_eq!(calls[1]["json"]["flags"], 64);
    assert_eq!(calls[1]["json"]["tts"], false);
    // 跟进消息和确认一样，不带机器人 Authorization（令牌即鉴权）
    assert!(calls[1]["authorization"].is_null(), "{}", calls[1]);
    calls[1]["json"]["content"].as_str().unwrap().to_owned()
}

// ------------------------------------------------------------------ /register

fn register_options(key: Option<&str>) -> Value {
    match key {
        Some(k) => json!([{"name": "key", "type": 3, "value": k}]),
        None => json!([]),
    }
}

#[tokio::test]
async fn register_hands_the_key_only_to_the_member() {
    let server = MockServer::start().await;
    let app = naimei(&server);
    server.reset(
        &json!({"/self-register/issue": {"status": 200, "json": {"key": "nai-x", "message": "🦉 欢迎！`nai-x`"}}}),
        &Value::Null,
    );
    app.handle_interaction(slash(1, "register", register_options(None)))
        .await;

    let bridge = server.bridge_log();
    assert_eq!(bridge.len(), 1, "{bridge:?}");
    assert_eq!(bridge[0]["method"], "POST");
    assert_eq!(bridge[0]["path"], "/self-register/issue");
    assert_eq!(bridge[0]["authorization"], format!("Bearer {SECRET}"));
    assert_eq!(bridge[0]["content_type"], "application/json");
    assert_eq!(
        bridge[0]["json"].to_string(),
        json!({"discord_id": MEMBER, "guild_id": GUILD, "name": "alice", "username": "alice",
               "global_name": "Alice", "avatar": "a1b2c3", "invite_key": ""})
        .to_string(),
        "键和顺序都固定"
    );
    assert_eq!(expect_deferred_reply(&server, 1), "🦉 欢迎！`nai-x`");
}

#[tokio::test]
async fn register_trims_and_truncates_the_invite_key() {
    let server = MockServer::start().await;
    let app = naimei(&server);
    server.reset(
        &json!({"/self-register/issue": {"status": 200, "json": {"message": "绑定好了"}}}),
        &Value::Null,
    );
    app.handle_interaction(slash(
        2,
        "register",
        register_options(Some(&format!("  nai-{}  ", "k".repeat(100)))),
    ))
    .await;
    let sent = server.bridge_log()[0]["json"]["invite_key"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(sent.chars().count(), 80);
    assert!(sent.starts_with("nai-k"), "{sent}");
    assert_eq!(expect_deferred_reply(&server, 2), "绑定好了");
}

#[tokio::test]
async fn register_replies_follow_the_gateways_answer() {
    let server = MockServer::start().await;
    let app = naimei(&server);
    let cases: Vec<(Value, &str)> = vec![
        // 网关说不行：原因原样给成员
        (
            json!({"status": 403, "json": {"detail": "Discord 账号创建满 7 天后才能领 Key，请稍后再来。"}}),
            "Discord 账号创建满 7 天后才能领 Key，请稍后再来。",
        ),
        (json!({"status": 403, "json": {}}), REGISTER_NOT_ELIGIBLE),
        // detail 不是文字（请求格式校验的错误列表）：不把内部结构给成员看
        (
            json!({"status": 403, "json": {"detail": [{"msg": "x"}]}}),
            REGISTER_NOT_ELIGIBLE,
        ),
        // 其他情况：领 Key 暂不可用
        (
            json!({"status": 401, "json": {"detail": "未授权"}}),
            REGISTER_UNAVAILABLE,
        ),
        (json!({"status": 500, "json": {}}), REGISTER_UNAVAILABLE),
        (
            json!({"status": 503, "json": {"detail": "x"}}),
            REGISTER_UNAVAILABLE,
        ),
        (
            json!({"status": 200, "text": "不是 JSON"}),
            REGISTER_UNAVAILABLE,
        ),
        (json!({"error": "connect"}), REGISTER_UNAVAILABLE),
    ];
    for (i, (spec, want)) in cases.into_iter().enumerate() {
        let n = 10 + i as u32;
        server.reset(&json!({"/self-register/issue": spec}), &Value::Null);
        app.handle_interaction(slash(n, "register", register_options(None)))
            .await;
        assert_eq!(expect_deferred_reply(&server, n), want, "第 {i} 种情况");
    }
}

#[tokio::test]
async fn register_in_another_server_is_refused_without_calling_the_gateway() {
    let server = MockServer::start().await;
    let app = naimei(&server);
    server.reset(&Value::Null, &Value::Null);
    app.handle_interaction(in_guild(
        3,
        "register",
        register_options(None),
        "900000000000000777",
    ))
    .await;
    assert!(bridge_paths(&server).is_empty());
    let calls = server.discord_log();
    assert_eq!(calls.len(), 1, "{calls:?}");
    assert_eq!(calls[0]["path"], callback_path(3));
    // 直接回复（类型 4），只有本人可见
    assert_eq!(
        calls[0]["json"],
        json!({"type": 4, "data": {"tts": false, "content": REGISTER_WRONG_GUILD, "flags": 64}})
    );
}

// ------------------------------------------------------------------ /resetkey

#[tokio::test]
async fn resetkey_shows_the_new_key_once() {
    let server = MockServer::start().await;
    let app = naimei(&server);
    server.reset(
        &json!({"/self-register/resetkey": {"status": 200, "json": {"key": "nai-new"}}}),
        &Value::Null,
    );
    app.handle_interaction(slash(13, "resetkey", json!([])))
        .await;
    assert_eq!(
        server.bridge_log()[0]["json"],
        json!({"discord_id": MEMBER, "guild_id": GUILD})
    );
    assert_eq!(
        expect_deferred_reply(&server, 13),
        "新的 Key（只有你能看到这条消息，请妥善保存）：\n`nai-new`\n旧 Key 已失效。"
    );
    // 还没领 Key
    server.reset(
        &json!({"/self-register/resetkey": {"status": 404, "json": {"detail": "你还没有领取 Key，请先使用 /register。"}}}),
        &Value::Null,
    );
    app.handle_interaction(slash(14, "resetkey", json!([])))
        .await;
    assert_eq!(
        expect_deferred_reply(&server, 14),
        "你还没有领取 Key，请先使用 /register。"
    );
}

#[tokio::test]
async fn removed_and_unknown_commands_get_the_generic_error_and_never_reach_the_gateway() {
    let server = MockServer::start().await;
    let app = naimei(&server);
    // 删掉的命令（旧的管理员命令；10-23 删的 /quota、/help）：服务器里的旧注册会在下次启动同步时被覆盖掉，
    // 还没同步前点到也是这个结果
    for (i, name) in [
        "open", "limit", "ban", "unban", "slots", "revoke", "quota", "help", "nope",
    ]
    .into_iter()
    .enumerate()
    {
        let n = 20 + i as u32;
        server.reset(&Value::Null, &Value::Null);
        app.handle_interaction(slash(n, name, json!([]))).await;
        assert!(bridge_paths(&server).is_empty(), "/{name}");
        let calls = server.discord_log();
        assert_eq!(calls.len(), 1, "/{name}：{calls:?}");
        assert_eq!(calls[0]["path"], callback_path(n));
        assert_eq!(
            calls[0]["json"],
            json!({"type": 4, "data": {"tts": false, "content": GENERIC_ERROR, "flags": 64}}),
            "/{name}"
        );
    }
}

#[tokio::test]
async fn discord_refusing_the_acknowledgement_still_tries_to_tell_the_member() {
    let server = MockServer::start().await;
    let app = naimei(&server);
    // 确认被 Discord 拒绝（比如超过 3 秒）：没有 followup 可用，改用直接回复告诉成员出错了
    server.reset(
        &json!({"/self-register/issue": {"status": 200, "json": {"message": "x"}}}),
        &json!({"callback": {"status": 404, "json": {"message": "Unknown interaction", "code": 10062}}}),
    );
    app.handle_interaction(slash(30, "register", register_options(None)))
        .await;
    let calls = server.discord_log();
    assert_eq!(calls.len(), 2, "{calls:?}");
    assert_eq!(calls[0]["json"]["type"], 5);
    assert_eq!(calls[1]["json"]["type"], 4);
    assert_eq!(calls[1]["json"]["data"]["content"], GENERIC_ERROR);
    assert!(bridge_paths(&server).is_empty(), "确认失败就不去领 Key");
}

#[tokio::test]
async fn a_failed_followup_is_reported_once_more_as_a_followup() {
    let server = MockServer::start().await;
    let app = naimei(&server);
    server.reset(
        &json!({"/self-register/resetkey": {"status": 200, "json": {"key": "nai-new"}}}),
        &json!({"followup": {"status": 500, "json": {"message": "boom"}}}),
    );
    app.handle_interaction(slash(31, "resetkey", json!([])))
        .await;
    let calls = server.discord_log();
    // 确认 + 跟进（失败）+ 再发一条「出了点问题」的跟进（也失败，只打日志）
    assert_eq!(calls.len(), 3, "{calls:?}");
    assert!(
        calls[1]["json"]["content"]
            .as_str()
            .unwrap()
            .contains("nai-new")
    );
    assert_eq!(calls[2]["json"], ephemeral(GENERIC_ERROR));
}

// ------------------------------------------------------------------ 心跳 / 启动

#[tokio::test]
async fn heartbeat_reports_account_server_latency_and_start_time() {
    let server = MockServer::start().await;
    let app = naimei(&server);
    app.set_latency(Some(Duration::from_millis(42)));
    server.reset(
        &json!({"/self-register/bot/report": {"status": 200, "json": {"ok": true}}}),
        &Value::Null,
    );
    assert!(app.sync_once(1_759_999_000.5).await);
    let bridge = server.bridge_log();
    assert_eq!(bridge.len(), 1, "{bridge:?}");
    assert_eq!(bridge[0]["method"], "POST");
    assert_eq!(bridge[0]["path"], "/self-register/bot/report");
    assert_eq!(bridge[0]["authorization"], format!("Bearer {SECRET}"));
    assert_eq!(
        bridge[0]["json"].to_string(),
        json!({"status": {"user": "naimei", "guild": "测试服", "latency_ms": 42, "ready_at": 1_759_999_000.5}})
            .to_string(),
        "只有在线状态，没有旧版的 AI 评论计数"
    );
    assert!(server.discord_log().is_empty(), "心跳不调用 Discord");
    // 服务器不在缓存里：guild 报空串；延迟还没有数据按 0 报
    let fresh = naimei(&server);
    fresh.remove_guild(GUILD);
    server.reset(
        &json!({"/self-register/bot/report": {"status": 200, "json": {"ok": true}}}),
        &Value::Null,
    );
    assert!(fresh.sync_once(1.0).await);
    assert_eq!(
        server.bridge_log()[0]["json"]["status"]["guild"],
        "",
        "{:?}",
        server.bridge_log()
    );
    assert_eq!(server.bridge_log()[0]["json"]["status"]["latency_ms"], 0);
}

#[tokio::test]
async fn heartbeat_failures_do_not_break_anything() {
    let server = MockServer::start().await;
    let app = naimei(&server);
    for spec in [
        json!({"error": "connect"}),
        json!({"status": 500, "json": {"detail": "x"}}),
        json!({"status": 200, "text": "不是 JSON"}),
    ] {
        server.reset(&json!({"/self-register/bot/report": spec}), &Value::Null);
        assert!(!app.sync_once(1.0).await);
    }
}

#[tokio::test]
async fn ready_syncs_the_commands_then_starts_the_heartbeat_once() {
    let server = MockServer::start().await;
    let app = naimei(&server);
    server.reset(
        &json!({"/self-register/bot/report": {"status": 200, "json": {"ok": true}}}),
        &Value::Null,
    );
    let ready = || ReadyInfo {
        user: User {
            id: BOT_ID.into(),
            username: "naimei".into(),
            discriminator: Some("0".into()),
            ..User::default()
        },
        application_id: APP.into(),
    };
    app.clone().on_ready(ready()).await;
    // 断线重连会再来一次 READY：命令再同步一次，心跳循环只有一个
    app.clone().on_ready(ready()).await;
    let puts: Vec<_> = server
        .discord_log()
        .into_iter()
        .filter(|c| c["method"] == "PUT")
        .collect();
    assert_eq!(puts.len(), 2);
    for put in &puts {
        assert_eq!(
            put["path"],
            format!("/applications/{APP}/guilds/{GUILD}/commands")
        );
        assert_eq!(put["json"].as_array().unwrap().len(), 2);
    }
    // 心跳循环的第一轮马上就发
    for _ in 0..50 {
        if !server.bridge_log().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(bridge_paths(&server), ["/self-register/bot/report"]);
}
