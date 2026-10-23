//! `GET /v1/me` 的端到端测试：真实的 AppState + 路由 + 记账，上游换成本地假服务。
//!
//! 这个文件原先是文本生成 / 流式 / 语音 / OpenAI 兼容聊天路由的测试（`text_routes.rs`）。文本接口在 2026-10-15
//! 整套删除，只留下 `/v1/me` 的用例；`/v1/models`、`/v1` 随之删除，也不再测。
//! 期望值里的字节串来自旧 Python 网关，功能列表按 3.0 现状少了「文本」「语音」两项；`today` 里去掉了
//! `daily_images_base`（保底）与 `anlas_auto`（V5 续杯）。

mod common;
use axum::http::StatusCode;
use common::{Call, Env, TOKEN, image_body, settle, setup};
use gate_db::repo::keys::NewKey;
use serde_json::{Value, json};

type Reply3 = (StatusCode, axum::http::HeaderMap, Vec<u8>);

async fn call_get(e: &Env, path: &str, token: &str) -> Reply3 {
    Call::new("GET", path).bearer(token).send(&e.state).await
}

async fn call_get_anon(e: &Env, path: &str) -> Reply3 {
    Call::new("GET", path).send(&e.state).await
}

async fn add_key(e: &Env, name: &str, token: &str, extra: Value) {
    let mut v = json!({
        "name": name, "token": token, "daily_images": 100, "daily_v5": 10, "monthly_anlas": 0.0,
        "daily_text_tokens": 1000, "rpm": 1000,
    });
    for (k, x) in extra.as_object().unwrap() {
        v[k] = x.clone();
    }
    let key: NewKey = serde_json::from_value(v).unwrap();
    e.state.db.create_key(&key, e.state.now()).await.unwrap();
}

fn text(body: &[u8]) -> String {
    String::from_utf8(body.to_vec()).unwrap()
}

fn json_of(body: &[u8]) -> Value {
    serde_json::from_slice(body).unwrap_or_else(|_| panic!("不是 JSON：{}", text(body)))
}

fn ct(headers: &axum::http::HeaderMap) -> &str {
    headers["content-type"].to_str().unwrap()
}

/// 旧版 `GateError` 的响应形态：顶层 statusCode / message 与 NovelAI 官方一致，message 带站点前缀。
fn assert_gate_error(status: StatusCode, body: &[u8], code: u16, message: &str) {
    assert_eq!(status.as_u16(), code, "{}", text(body));
    let v = json_of(body);
    let full = format!("猫头鹰公益站提醒：{message}");
    assert_eq!(v["statusCode"], code);
    assert_eq!(v["message"], full);
    assert_eq!(v["error"]["message"], full);
    assert_eq!(v["error"]["status"], code);
    assert_eq!(v["error"]["request_id"].as_str().unwrap().len(), 8);
}

// ------------------------------------------------------------------ /v1/me

const FEATURES_ALL_ON: &str = r#"[{"id":"image","label":"文生图","on":true},{"id":"upscale","label":"放大","on":true},{"id":"augment","label":"导演工具（线稿、上色等）","on":true},{"id":"vibe","label":"Vibe 编码","on":true},{"id":"tags","label":"标签补全","on":true}]"#;

fn me_json(
    name: &str,
    tokens: i64,
    requests: i64,
    daily_text: i64,
    delete_days: i64,
    features: &str,
) -> String {
    format!(
        "{{\"name\":\"{name}\",\"generated_images_total\":0,\"today\":{{\"images\":0,\"daily_images\":100,\"legacy_free_images_today\":0,\"anlas_today\":0.0,\"daily_anlas\":0.0,\"v5_today\":0,\"daily_v5\":10,\"image_model_scope\":\"legacy\",\"anlas_month\":0.0,\"monthly_anlas\":0.0,\"text_tokens\":{tokens},\"daily_text_tokens\":{daily_text},\"requests\":{requests}}},\"expires_at\":null,\"inactivity_delete_days\":{delete_days},\"features\":{features}}}"
    )
}

#[tokio::test]
async fn me_matches_legacy_bytes() {
    let e = setup(&[]).await;
    let (status, headers, body) = call_get(&e, "/v1/me", TOKEN).await;
    assert_eq!(status, 200, "{}", text(&body));
    assert_eq!(ct(&headers), "application/json");
    assert_eq!(
        text(&body),
        me_json("测试成员", 0, 0, 1000, 3, FEATURES_ALL_ON)
    );
}

#[tokio::test]
async fn me_counts_usage_and_respects_feature_switches() {
    let e = setup(&[]).await;
    add_key(
        &e,
        "仅出图",
        "nai-test-feat-key-00000000000000000000",
        json!({"features": "image"}),
    )
    .await;
    // 出一张图：今日的张数和请求数跟着走
    let (status, _, body) = Call::new("POST", "/ai/generate-image")
        .bearer(TOKEN)
        .json(image_body())
        .send(&e.state)
        .await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    settle().await;
    let (_, _, body) = call_get(&e, "/v1/me", TOKEN).await;
    let v = json_of(&body);
    assert_eq!(v["today"]["images"], 1, "{}", text(&body));
    assert_eq!(v["today"]["requests"], 1, "{}", text(&body));
    // Key 自己的功能列表：只开了出图（Vibe 因为全站 Anlas 池开放，仍是可用）
    let (_, _, body) = call_get(&e, "/v1/me", "nai-test-feat-key-00000000000000000000").await;
    let v = json_of(&body);
    let on: Vec<(&str, bool)> = v["features"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| (f["id"].as_str().unwrap(), f["on"].as_bool().unwrap()))
        .collect();
    assert_eq!(
        on,
        [
            ("image", true),
            ("upscale", false),
            ("augment", false),
            ("vibe", true),
            ("tags", false),
        ]
    );
    // 站长全局关掉「放大」：成员看到 off；库里残留的 text / voice 开关（文本接口已删）不再有任何作用
    e.state
        .db
        .set_setting(
            "features_global",
            r#"{"upscale": false, "text": false, "voice": false}"#,
        )
        .await
        .unwrap();
    let (_, _, body) = call_get(&e, "/v1/me", TOKEN).await;
    let v = json_of(&body);
    assert_eq!(v["features"].as_array().unwrap().len(), 5);
    assert_eq!(
        v["features"][1],
        json!({"id": "upscale", "label": "放大", "on": false})
    );
}

#[tokio::test]
async fn me_auth_errors() {
    let e = setup(&[]).await;
    let (status, _, body) = call_get_anon(&e, "/v1/me").await;
    assert_gate_error(
        status,
        &body,
        401,
        "未收到 API Key。请在客户端中填入本站发放的 Key。开发者参考：放在 Authorization: Bearer 请求头中",
    );
    let (status, _, body) = call_get(&e, "/v1/me", "nai-wrong").await;
    assert_gate_error(
        status,
        &body,
        401,
        "API Key 无效。请检查客户端中的 Key 是否完整，或在 Discord 用 /resetkey 重新获取。",
    );
}

/// 文本生成 / 流式 / 语音 / OpenAI 兼容聊天 / 模型列表整套删除：
/// 这些地址回 404 并说明原因（OpenAI 格式 / 文本已停用），带不带 Key 都一样，更不会往上游发请求或写用量日志。
#[tokio::test]
async fn removed_text_endpoints_are_plain_404s() {
    let e = setup(&[]).await;
    let post_body =
        json!({"model": "llama-3-erato-v1", "input": "once upon", "parameters": {"max_length": 5}});
    for path in [
        "/ai/generate-stream",
        "/nai/ai/generate-stream",
        "/ai/generate",
        "/nai/ai/generate",
        "/v1/ai/generate",
        "/ai/generate-voice",
        "/nai/ai/generate-voice",
        "/v1/chat/completions",
    ] {
        for token in [Some(TOKEN), None] {
            let mut call = Call::new("POST", path).json(post_body.clone());
            if let Some(t) = token {
                call = call.bearer(t);
            }
            let (status, _, body) = call.send(&e.state).await;
            let detail = gate_server::routes::not_found_message(path);
            assert_ne!(detail, "Not Found", "{path} 应说明原因");
            assert_eq!(
                (status.as_u16(), text(&body)),
                (404, json!({"detail": detail}).to_string()),
                "POST {path}"
            );
        }
    }
    for path in ["/v1/models", "/v1"] {
        let (status, _, body) = call_get_anon(&e, path).await;
        let detail = gate_server::routes::not_found_message(path);
        assert!(detail.contains("不兼容 OpenAI 格式"), "{detail}");
        assert_eq!(
            (status.as_u16(), text(&body)),
            (404, json!({"detail": detail}).to_string()),
            "GET {path}"
        );
        let (status, _, _) = call_get(&e, path, TOKEN).await;
        assert_eq!(status, 404, "GET {path}（带 Key）");
    }
    settle().await;
    assert!(e.mock.requests().is_empty());
    let logs = e
        .state
        .db
        .list_logs(10, 0, &gate_db::repo::usage::LogFilter::default())
        .await
        .unwrap();
    assert!(logs.is_empty(), "{logs:?}");
    // /v1/me 还在
    let (status, _, _) = call_get(&e, "/v1/me", TOKEN).await;
    assert_eq!(status, 200);
}
