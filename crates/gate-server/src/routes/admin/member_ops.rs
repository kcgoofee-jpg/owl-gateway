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
use gate_db::repo::INVITE_TAG;
use gate_db::repo::counters::{Counter, ResetScope};
use gate_db::repo::keys::NewKey;
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

/// 一键新建邀请 Key（10-11 站长）：自动编号「邀请-N」、打「邀请」标签，其余和 /register 发的一样。
pub(super) async fn create_invite_key(state: &AppState) -> Result<Value, ApiError> {
    // 「邀请-N」编号：只看名字开头，数字可以是任何 Unicode 十进制数字
    let max_no = state
        .db
        .key_names()
        .await?
        .iter()
        .filter_map(|n| {
            let rest = n.strip_prefix("邀请-")?;
            let digits: String = rest.chars().take_while(|c| validation::is_nd(*c)).collect();
            primitives::parse_int(&digits)
        })
        .max()
        .unwrap_or(0);
    let name = format!("邀请-{}", max_no + 1);
    let now = state.now();
    let new = NewKey {
        name: name.clone(),
        token: gen_key(),
        enabled: true,
        expires_at: None,
        discord_id: None,
    };
    let row = state.db.create_key(&new, now).await?;
    let id = row.get("id").and_then(Value::as_i64).unwrap_or(0);
    state
        .db
        .upsert_key_tag(id, INVITE_TAG, "邀请领取", "站长", now)
        .await?;
    log_action(
        &state.db,
        "站长",
        "新建邀请 Key",
        &name,
        "",
        true,
        state.now(),
    )
    .await;
    let row = state.db.get_key(id).await?.unwrap_or(Value::Null);
    Ok(json!({"key": full_key_json(state, &row, id).await?}))
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
    state.db.update_key(key_id, &fields).await?;
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
