//! 后台成员操作的服务层：重置额度、发券、删 Key / 封禁、打标签。
//!
//! 路由只负责鉴权、解析输入和拼响应；业务流程（校验、写库、操作日志、通知注册服务）都在这里，
//! 这样同一套动作既能被 HTTP 路由调用，也能被 `nai-ops` 之类的运维命令复用。

use super::members::{NO_KEY, full_key_json, load_key};
use super::req::{http, pstr, pstr_or};
use crate::body::JsonObject;
use crate::error::ApiError;
use crate::state::AppState;
use gate_core::primitives;
use gate_db::repo::counters::ResetScope;
use serde_json::{Value, json};

/// 请求体里的 `scope: ["v5", "anlas"]` → 重置范围。
pub(super) fn reset_scope_of(body: &JsonObject) -> ResetScope {
    let has = |k: &str| {
        body.get("scope")
            .and_then(Value::as_array)
            .is_some_and(|a| a.iter().any(|v| v.as_str() == Some(k)))
    };
    ResetScope {
        v5: has("v5"),
        anlas: has("anlas"),
    }
}

/// 重置指定 Key 今日的额度计数，保留审计日志。V4.5 一定重置，V5 / Anlas 看 `scope`。
pub(super) async fn reset_quota(
    state: &AppState,
    key_id: i64,
    scope: ResetScope,
) -> Result<Value, ApiError> {
    let Some(row) = load_key(state, key_id).await? else {
        return Err(http(404, NO_KEY));
    };
    state
        .db
        .reset_daily_image_quota(key_id, &state.day(), scope)
        .await?;
    let key = full_key_json(state, &row, key_id).await?;
    Ok(json!({"ok": true, "key": key}))
}

/// 给指定 Key 发一张重置券（目前只有 `reset`，有效期 1～30 天）。
pub(super) async fn add_reset_coupon(
    state: &AppState,
    key_id: i64,
    body: &JsonObject,
) -> Result<Value, ApiError> {
    let kind = body.get("kind").cloned().unwrap_or_else(|| json!("reset"));
    if kind.as_str() != Some("reset") {
        return Err(http(422, "目前只有重置券（reset）"));
    }
    let days = body.get("days").cloned().unwrap_or_else(|| json!(7));
    // `type(days) not in (int, float)`：布尔、字符串都不行
    let days_f = match &days {
        Value::Number(n) => n.as_f64(),
        _ => None,
    };
    let Some(days_f) = days_f.filter(|d| 0.0 < *d && *d <= 30.0) else {
        return Err(http(422, "有效期 days 必须在 1～30 天之间"));
    };
    if load_key(state, key_id).await?.is_none() {
        return Err(http(404, NO_KEY));
    }
    let note = primitives::take_chars(&pstr(body, "note"), 200).to_string();
    let by = primitives::take_chars(&pstr_or(body, "by", "站长"), 40).to_string();
    let scope = reset_scope_of(body).canonical();
    let coupon = state
        .db
        .add_coupon(key_id, "reset", days_f, &note, &by, &scope, state.now())
        .await?;
    Ok(json!({"ok": true, "coupon": coupon}))
}

/// 删除成员 / Key。Discord 自助领取的成员同时清掉领取记录并摘身份组；`ban` 则永久禁止再次领取。
pub(super) async fn delete_member_key(
    state: &AppState,
    key_id: i64,
    ban: bool,
    reason: &str,
) -> Result<Value, ApiError> {
    if load_key(state, key_id).await?.is_none() {
        return Err(http(404, NO_KEY));
    }
    let registrar = state.registration.clone();
    let discord_id = match &registrar {
        Some(_) => state
            .db
            .registration_for_key(key_id)
            .await?
            .map(|r| r.discord_id),
        None => None,
    };
    if let (Some(reg), Some(discord_id)) = (registrar, discord_id) {
        if ban {
            reg.ban(&discord_id, reason, "站长").await?;
        } else {
            reg.revoke(&discord_id, true).await?;
        }
        return Ok(json!({"ok": true, "discord_id": discord_id, "banned": ban}));
    }
    state.db.delete_key(key_id, state.now()).await?;
    Ok(json!({"ok": true}))
}

/// 给成员打 / 改手动标签（只在后台显示）。body: `{tag, note, by}`
pub(super) async fn set_key_tag(
    state: &AppState,
    key_id: i64,
    body: &JsonObject,
) -> Result<Value, ApiError> {
    const TAG_MAX_LEN: usize = 12;
    const TAG_NOTE_MAX: usize = 200;
    if load_key(state, key_id).await?.is_none() {
        return Err(http(404, NO_KEY));
    }
    // `" ".join(str(...).split())[:12]`：任何空白折成一个空格
    let joined = pstr(body, "tag")
        .split(primitives::is_space)
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let tag = primitives::take_chars(&joined, TAG_MAX_LEN).to_string();
    if tag.is_empty() {
        return Err(http(422, "标签不能为空"));
    }
    let note =
        primitives::take_chars(primitives::strip(&pstr(body, "note")), TAG_NOTE_MAX).to_string();
    let by = {
        let b =
            primitives::take_chars(primitives::strip(&pstr_or(body, "by", "站长")), 20).to_string();
        if b.is_empty() {
            "站长".to_string()
        } else {
            b
        }
    };
    state
        .db
        .upsert_key_tag(key_id, &tag, &note, &by, state.now())
        .await?;
    Ok(json!({"ok": true, "tag": tag}))
}

/// 误判解除暂停（风险分、证据在 3.0 已随防分享一起删除，只剩 Key 暂停闸门）。
pub(super) async fn clear_pause(state: &AppState, key_id: i64) -> Result<Value, ApiError> {
    state.pauses.remove(key_id);
    state.db.clear_pause(key_id).await?;
    Ok(json!({"ok": true}))
}
