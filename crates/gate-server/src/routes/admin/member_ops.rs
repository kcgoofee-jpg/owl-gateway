//! 后台成员操作的服务层：重置额度、发券、删 Key / 封禁、打标签。
//!
//! 路由只负责鉴权、解析输入和拼响应；业务流程（校验、写库、操作日志、通知注册服务）都在这里，
//! 这样同一套动作既能被 HTTP 路由调用，也能被 `nai-ops` 之类的运维命令复用。

use super::members::{NO_KEY, full_key_json, gen_key, key_json, load_key};
use super::req::{http, pstr, pstr_or};
use crate::action_log::log_action;
use crate::body::JsonObject;
use crate::error::ApiError;
use crate::state::AppState;
use gate_core::primitives;
use gate_core::validation;
use gate_db::repo::counters::{Counter, ResetScope};
use serde_json::{Value, json};
use std::collections::HashMap;

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

/// 重置券有效期上限（天）。表单下限是 1；接口仍接受大于 0 且不超过这个上限的数。
pub(super) const COUPON_DAYS_MAX: f64 = 30.0;

pub(super) fn coupon_fields() -> Value {
    json!({
        "days": {"kind": "float", "min": 1, "max": COUPON_DAYS_MAX as i64}
    })
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
    let Some(days_f) = days_f.filter(|d| 0.0 < *d && *d <= COUPON_DAYS_MAX) else {
        return Err(http(
            422,
            format!("有效期 days 必须在 1～{} 天之间", COUPON_DAYS_MAX as i64),
        ));
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
    let row = load_key(state, key_id)
        .await?
        .ok_or_else(|| http(404, NO_KEY))?;
    let registrar = state.registration.clone();
    let discord_id = match &registrar {
        Some(_) => state
            .db
            .registration_for_key(key_id)
            .await?
            .map(|r| r.discord_id),
        None => None,
    };
    if ban {
        let subject = discord_id.or_else(|| {
            row.get("discord_id")
                .and_then(Value::as_str)
                .map(str::to_string)
        });
        let subject = subject
            .filter(|id| gate_core::member_policy::is_discord_id(id))
            .ok_or_else(|| http(422, "该 Key 没有可封禁的 Discord 身份；请使用删除操作。"))?;
        if let Some(reg) = registrar {
            reg.ban(&subject, reason, "站长").await?;
        } else {
            state
                .db
                .ban_discord(
                    &subject,
                    &gate_db::repo::discord::BanInfo {
                        reason: reason.to_string(),
                        banned_by: "站长".into(),
                        ..Default::default()
                    },
                    state.now(),
                )
                .await?;
            state.db.revoke_registration(&subject).await?;
        }
        // 目标可能是没有领取记录的旧 Key；封禁已先停用同身份的所有凭据。
        state.db.delete_key(key_id).await?;
        return Ok(json!({"ok": true, "discord_id": subject, "banned": true}));
    }
    if let (Some(reg), Some(discord_id)) = (registrar, discord_id) {
        reg.revoke(&discord_id, true).await?;
        return Ok(json!({"ok": true, "discord_id": discord_id, "banned": false}));
    }
    state.db.delete_key(key_id).await?;
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

/// 后台成员列表：一次批量取今日计数 / 累计张数 / 邀请 Key 集合 / 每人的生效规则。
pub(super) async fn list_keys_with_counters(state: &AppState) -> Result<Value, ApiError> {
    let rows = state.db.list_keys().await?;
    let policies = state.db.member_policy_directory().await?;
    let today = state.day();
    // 一次批量取回今日计数（以前逐把 get_counter：370 把 = 370 次串行读库）
    let counters = state.db.counter_map(&today).await?;
    let zero = Counter::default();
    let totals: HashMap<i64, i64> = state
        .db
        .generated_image_totals(None)
        .await?
        .into_iter()
        .collect();
    let invite: std::collections::HashSet<i64> =
        state.db.invite_key_ids().await?.into_iter().collect();
    let mut out = Vec::with_capacity(rows.len());
    for r in &rows {
        let id = r.get("id").and_then(Value::as_i64).unwrap_or(0);
        let c = counters.get(&id).unwrap_or(&zero);
        out.push(key_json(
            r,
            c,
            totals.get(&id).copied().unwrap_or(0),
            invite.contains(&id),
            &policies.for_subject(r.get("discord_id").and_then(Value::as_str))?,
        ));
    }
    Ok(json!({"keys": out}))
}

/// 换一把新 Token（旧 Token 立即失效）。
pub(super) async fn regenerate_key(state: &AppState, key_id: i64) -> Result<Value, ApiError> {
    let token = gen_key();
    if !state.db.rotate_key_token(key_id, &token).await? {
        return Err(http(404, NO_KEY));
    }
    Ok(json!({"token": token}))
}

/// 改名称 / 启用状态 / 到期时间（权限走成员规则，不在这里改）。
pub(super) async fn patch_key(
    state: &AppState,
    key_id: i64,
    body: crate::body::JsonObject,
) -> Result<Value, ApiError> {
    if body
        .keys()
        .any(|name| !["name", "enabled", "expires_at"].contains(&name.as_str()))
    {
        return Err(http(
            422,
            "Key 只允许修改名称、启用状态和到期时间；权限请修改成员规则",
        ));
    }
    if load_key(state, key_id).await?.is_none() {
        return Err(http(404, NO_KEY));
    }
    let mut fields = serde_json::Map::new();
    for (name, value) in body {
        let valid = match name.as_str() {
            "name" => value
                .as_str()
                .is_some_and(|text| !text.trim().is_empty() && text.chars().count() <= 60),
            "enabled" => value.is_boolean(),
            "expires_at" => {
                value.is_null() || value.as_f64().is_some_and(|n| n.is_finite() && n >= 0.0)
            }
            _ => false,
        };
        if !valid {
            return Err(http(422, format!("{name} 格式无效")));
        }
        fields.insert(name, value);
    }
    if !state.db.update_key(key_id, &fields).await? {
        return Err(http(
            409,
            "无法修改 Key 启用状态；请检查停用原因和封禁状态。",
        ));
    }
    let row = load_key(state, key_id)
        .await?
        .ok_or_else(|| http(404, NO_KEY))?;
    Ok(json!({"key": full_key_json(state, &row, key_id).await?}))
}

/// 黑名单列表（支持按 ID / 用户名 / 原因搜索）。
pub(super) async fn list_bans(state: &AppState, query: &str) -> Result<Value, ApiError> {
    let bans = state.db.list_bans(query).await?;
    Ok(json!({"bans": bans, "total": bans.len()}))
}

/// 手动把某个 Discord ID 加入黑名单：配了机器人走注册服务（顺带摘身份组），否则只写库。
pub(super) async fn create_ban(
    state: &AppState,
    body: &crate::body::JsonObject,
) -> Result<Value, ApiError> {
    let discord_id = primitives::strip(&pstr(body, "discord_id")).to_string();
    if !validation::is_decimal(&discord_id) {
        return Err(http(422, "Discord ID 必须为纯数字"));
    }
    let reason = primitives::take_chars(
        primitives::strip(&pstr_or(body, "reason", "站长手动封禁")),
        100,
    )
    .to_string();
    match state.registration.clone() {
        Some(reg) => reg.ban(&discord_id, &reason, "站长").await?,
        None => {
            let info = gate_db::repo::discord::BanInfo {
                reason: reason.clone(),
                banned_by: "站长".into(),
                ..Default::default()
            };
            state
                .db
                .ban_discord(&discord_id, &info, state.now())
                .await?;
        }
    }
    log_action(
        &state.db,
        "站长",
        "添加黑名单",
        &format!("Discord:{discord_id}"),
        &reason,
        true,
        state.now(),
    )
    .await;
    Ok(json!({"ok": true, "discord_id": discord_id}))
}

/// 解封：把 Discord ID 移出黑名单，允许重新领取 Key。
pub(super) async fn delete_ban(state: &AppState, discord_id: &str) -> Result<Value, ApiError> {
    let discord_id = primitives::strip(discord_id).to_string();
    if !state.db.unban_discord(&discord_id).await? {
        return Err(http(404, "该账号不在黑名单中"));
    }
    log_action(
        &state.db,
        "站长",
        "移出黑名单",
        &format!("Discord:{discord_id}"),
        "已解除封禁，允许重新领取 Key",
        true,
        state.now(),
    )
    .await;
    Ok(json!({"ok": true, "discord_id": discord_id}))
}
