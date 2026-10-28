//! 后台「成员」接口（Key / 成员 / 黑名单 / 券 / 标签）。
//!
//! 状态码、`{"detail": ...}` 文案、JSON 键顺序都是固定契约：后台前端按它们渲染，不要随手改。
//! 3.0 已删除的功能不移植：`anlas_auto` 自动分配（含 `anlas_mode`）、`v5_pinned`（V5 基础值）、
//! 防分享证据；对应的数据库列原样留着，这里不读不写，也不再出现在返回的 JSON 里。

use super::mark_logged;
use super::req::{AdminPath, AdminQuery, R, http, json_of, pstr, pstr_or};
use crate::action_log::log_action;
use crate::body::JsonObject;
use crate::error::ApiError;
use crate::error::ok_json;
use crate::session::AdminSession;
use crate::state::AppState;
use axum::Router;
use axum::extract::{Request, State};
use axum::routing::{delete, get, patch, post, put};
use gate_core::{primitives, validation, values};
use gate_db::repo::counters::Counter;
use gate_db::repo::{
    INVITE_TAG,
    admin_members::{RegistrationRow, TagRow},
    keys::NewKey,
};
use serde_json::{Map, Value, json};
use std::collections::HashMap;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/admin/api/keys", get(list_keys))
        .route("/admin/api/keys/invite", post(create_invite_key))
        .route(
            "/admin/api/keys/{key_id}",
            patch(patch_key).delete(delete_key),
        )
        .route("/admin/api/keys/{key_id}/regenerate", post(regenerate_key))
        .route(
            "/admin/api/keys/{key_id}/reset-daily-image-quota",
            post(reset_daily_image_quota),
        )
        .route(
            "/admin/api/keys/{key_id}/coupons",
            get(list_key_coupons).post(give_coupon),
        )
        .route("/admin/api/keys/{key_id}/tags", put(put_key_tag))
        .route(
            "/admin/api/keys/{key_id}/tags/{tag}",
            delete(delete_key_tag),
        )
        .route(
            "/admin/api/keys/{key_id}/share-clear",
            post(key_share_clear),
        )
        .route("/admin/api/members", get(members))
        .route("/admin/api/bans", get(list_bans).post(create_ban))
        .route("/admin/api/bans/{discord_id}", delete(delete_ban))
}

const NO_KEY: &str = "key 不存在";
// ----------------------------------------------------------------------------- 小工具

fn jget(row: &Value, name: &str) -> Value {
    row.get(name).cloned().unwrap_or(Value::Null)
}

fn jflag(row: &Value, name: &str) -> bool {
    row.get(name).is_some_and(values::enabled)
}

fn gen_key() -> String {
    gate_core::policy::gen_key("nai", &rand::random::<[u8; 24]>())
}

/// `features` 字段：缺省 / null = 沿用全局开关；列表 = 仅允许所列功能。
pub(crate) fn discord_avatar_url(id: &str, avatar: Option<&str>) -> String {
    match avatar.filter(|a| !a.is_empty()) {
        Some(a) => format!("https://cdn.discordapp.com/avatars/{id}/{a}.png?size=64"),
        None => {
            let n = validation::integer_string(id).unwrap_or(0);
            format!(
                "https://cdn.discordapp.com/embed/avatars/{}.png",
                (n >> 22).rem_euclid(6)
            )
        }
    }
}

/// 成员的 Discord 资料：显示名、用户名、头像地址、个人资料链接（点开可直接私信）。
fn discord_profile(row: &RegistrationRow) -> Value {
    let id = &row.discord_id;
    let avatar_url = discord_avatar_url(id, row.avatar.as_deref());
    json!({
        "id": id,
        "username": row.username,
        "display_name": row.display_name,
        "avatar_url": avatar_url,
        "profile_url": format!("https://discord.com/users/{id}"),
    })
}

/// 后台凭据视图；邀请来源仅作标签，不参与权限计算。
fn key_json(
    row: &Value,
    c: &Counter,
    generated: i64,
    invited: bool,
    policy: &gate_db::repo::member_policy::PolicySnapshot,
) -> Value {
    let anlas = c.anlas;
    json!({
        "id": jget(row, "id"),
        "name": jget(row, "name"),
        "token": jget(row, "token"),
        "enabled": jflag(row, "enabled"),
        "policy": policy,
        "invited": invited,
        "expires_at": jget(row, "expires_at"),
        "created_at": jget(row, "created_at"),
        "last_used_at": jget(row, "last_used_at"),
        "used": {
            "images": c.images,
            "generated_images_total": generated,
            "legacy_free_images": c.legacy_free_images,
            "anlas": values::float(primitives::round(anlas, 2)),
            "v5": c.v5,
            "requests": c.requests,
        },
    })
}

async fn load_key(state: &AppState, key_id: i64) -> Result<Option<Value>, ApiError> {
    Ok(state.db.get_key(key_id).await?)
}

/// 一把 Key 的完整 JSON（今日计数 + 累计生成张数）。
async fn full_key_json(state: &AppState, row: &Value, key_id: i64) -> Result<Value, ApiError> {
    let c = state.db.get_counter(key_id, &state.day()).await?;
    let totals = state.db.generated_image_totals(Some(key_id)).await?;
    let generated = totals
        .iter()
        .find(|(k, _)| *k == key_id)
        .map_or(0, |(_, n)| *n);
    let policies = state.db.member_policy_directory().await?;
    let policy = policies.for_subject(row.get("discord_id").and_then(Value::as_str))?;
    Ok(key_json(
        row,
        &c,
        generated,
        state.db.invite_key_ids().await?.contains(&key_id),
        &policy,
    ))
}

// ----------------------------------------------------------------------------- Key 列表 / 新建

async fn list_keys(State(state): State<AppState>, _s: AdminSession) -> R {
    let st = &state;
    let rows = st.db.list_keys().await?;
    let policies = st.db.member_policy_directory().await?;
    let today = st.day();
    // 一次批量取回今日计数（以前逐把 get_counter：370 把 = 370 次串行读库）
    let counters = st.db.counter_map(&today).await?;
    let zero = Counter::default();
    let totals: HashMap<i64, i64> = st
        .db
        .generated_image_totals(None)
        .await?
        .into_iter()
        .collect();
    let invite: std::collections::HashSet<i64> =
        st.db.invite_key_ids().await?.into_iter().collect();
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
    Ok(ok_json(&json!({"keys": out})))
}

/// 一键新建邀请 Key（10-11 站长）：自动编号「邀请-N」、打「邀请」标签、额度交给算法，其余和 /register 发的一样。
/// 成员拿到后在 Discord 用 /register 填这把 Key 完成绑定，后台自动认出是谁。
async fn create_invite_key(State(state): State<AppState>, _s: AdminSession) -> R {
    let st = &state;
    // 和 /register 一样读「新成员默认」（后台「领 Key」的设置，没设过用 defaults 里的常量）
    // 「邀请-N」编号：只看名字开头，数字可以是任何 Unicode 十进制数字
    let max_no = st
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
    let now = st.now();
    let new = NewKey {
        name: name.clone(),
        token: gen_key(),
        enabled: true,
        expires_at: None,
        discord_id: None,
    };
    let row = st.db.create_key(&new, now).await?;
    let id = row.get("id").and_then(Value::as_i64).unwrap_or(0);
    st.db
        .upsert_key_tag(id, INVITE_TAG, "邀请领取", "站长", now)
        .await?;
    log_action(&st.db, "站长", "新建邀请 Key", &name, "", true, st.now()).await;
    let row = st.db.get_key(id).await?.unwrap_or(Value::Null);
    let resp = ok_json(&json!({"key": full_key_json(st,&row,id).await?}));
    Ok(mark_logged(resp))
}

async fn regenerate_key(
    State(state): State<AppState>,
    _s: AdminSession,
    AdminPath(key_id): AdminPath<i64>,
) -> R {
    let token = gen_key();
    if !state.db.rotate_key_token(key_id, &token).await? {
        return Err(http(404, NO_KEY));
    }
    let mut resp = ok_json(&json!({"token": token}));
    resp.headers_mut().insert(
        "cache-control",
        axum::http::HeaderValue::from_static("no-store"),
    );
    Ok(resp)
}

async fn patch_key(
    State(state): State<AppState>,
    _session: AdminSession,
    AdminPath(key_id): AdminPath<i64>,
    request: Request,
) -> R {
    let (_, body) = json_of(request).await?;
    if body
        .keys()
        .any(|name| !["name", "enabled", "expires_at"].contains(&name.as_str()))
    {
        return Err(http(
            422,
            "Key 只允许修改名称、启用状态和到期时间；权限请修改成员规则",
        ));
    }
    if load_key(&state, key_id).await?.is_none() {
        return Err(http(404, NO_KEY));
    }
    let mut fields = Map::new();
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
    let row = load_key(&state, key_id)
        .await?
        .ok_or_else(|| http(404, NO_KEY))?;
    Ok(ok_json(
        &json!({"key":full_key_json(&state,&row,key_id).await?}),
    ))
}

fn reset_scope_of(body: &JsonObject) -> gate_db::repo::counters::ResetScope {
    let has = |k: &str| {
        body.get("scope")
            .and_then(Value::as_array)
            .is_some_and(|a| a.iter().any(|v| v.as_str() == Some(k)))
    };
    gate_db::repo::counters::ResetScope {
        v5: has("v5"),
        anlas: has("anlas"),
    }
}

/// 重置指定 Key 今日的额度计数，保留审计日志。V4.5 一定重置，V5 / Anlas 看请求里的 `scope`（默认不重置）。
async fn reset_daily_image_quota(
    State(state): State<AppState>,
    _s: AdminSession,
    AdminPath(key_id): AdminPath<i64>,
    req: Request,
) -> R {
    // 请求体可以没有（旧前端 / 脚本）：没有就只重置 V4.5
    let scope = json_of(req)
        .await
        .map(|(_, b)| reset_scope_of(&b))
        .unwrap_or(gate_db::repo::counters::ResetScope::V45_ONLY);
    let st = state.clone();
    let Some(row) = load_key(&st, key_id).await? else {
        return Err(http(404, NO_KEY));
    };
    st.db
        .reset_daily_image_quota(key_id, &st.day(), scope)
        .await?;
    let key = full_key_json(&st, &row, key_id).await?;
    Ok(ok_json(&json!({"ok": true, "key": key})))
}

/// 管理员手动给指定 Key 绑定或换绑 Discord 账号。
///
/// 只需要 Discord ID：配了机器人就用它读这个账号的公开资料（和后台补全资料同一条 `GET /users/{id}`），
/// 用户名 / 显示名（global_name，没有就用用户名）/ 头像都以 Discord 为准，Key 名改成「显示名 (@用户名)」。
/// 读不到（没配机器人、网络错误、超时、查无此人）就退回请求里填的 `username` / `display_name`，Key 名不动。
async fn give_coupon(
    State(state): State<AppState>,
    _s: AdminSession,
    AdminPath(key_id): AdminPath<i64>,
    req: Request,
) -> R {
    let (_, body) = json_of(req).await?;
    let kind = body.get("kind").cloned().unwrap_or_else(|| json!("reset"));
    let days = body.get("days").cloned().unwrap_or_else(|| json!(7));
    if kind.as_str() != Some("reset") {
        return Err(http(422, "目前只有重置券（reset）"));
    }
    // `type(days) not in (int, float)`：布尔、字符串都不行
    let days_f = match &days {
        Value::Number(n) => n.as_f64(),
        _ => None,
    };
    let Some(days_f) = days_f.filter(|d| 0.0 < *d && *d <= 30.0) else {
        return Err(http(422, "有效期 days 必须在 1～30 天之间"));
    };
    let st = state.clone();
    if load_key(&st, key_id).await?.is_none() {
        return Err(http(404, NO_KEY));
    }
    let note = primitives::take_chars(&pstr(&body, "note"), 200).to_string();
    let by = primitives::take_chars(&pstr_or(&body, "by", "站长"), 40).to_string();
    let scope = reset_scope_of(&body).canonical();
    let c = st
        .db
        .add_coupon(key_id, "reset", days_f, &note, &by, &scope, st.now())
        .await?;
    Ok(ok_json(&json!({"ok": true, "coupon": c})))
}

async fn list_key_coupons(
    State(state): State<AppState>,
    _s: AdminSession,
    AdminPath(key_id): AdminPath<i64>,
) -> R {
    let rows = state.db.list_coupons(key_id, false, state.now()).await?;
    Ok(ok_json(&json!({"coupons": rows})))
}

// ----------------------------------------------------------------------------- 删除 Key

/// 删除成员 / Key。Discord 自助领取的成员会同时清掉领取记录并摘除身份组；`ban=true` 则永久禁止该账号再次领取。
async fn delete_key(
    State(state): State<AppState>,
    _s: AdminSession,
    AdminPath(key_id): AdminPath<i64>,
    q: AdminQuery,
) -> R {
    let ban = q.flag("ban")?;
    let st = state.clone();
    if load_key(&st, key_id).await?.is_none() {
        return Err(http(404, NO_KEY));
    }
    let registrar = st.registration.clone();
    let discord_id = match &registrar {
        Some(_) => st
            .db
            .registration_for_key(key_id)
            .await?
            .map(|r| r.discord_id),
        None => None,
    };
    if let (Some(reg), Some(discord_id)) = (registrar, discord_id) {
        let reason = q.get("reason").unwrap_or("站长手动删除并封禁").to_string();
        if ban {
            reg.ban(&discord_id, &reason, "站长").await?;
        } else {
            reg.revoke(&discord_id, true).await?;
        }
        return Ok(ok_json(
            &json!({"ok": true, "discord_id": discord_id, "banned": ban}),
        ));
    }
    st.db.delete_key(key_id, st.now()).await?;
    Ok(ok_json(&json!({"ok": true})))
}

// ----------------------------------------------------------------------------- 黑名单

/// 获取黑名单列表，支持按 ID、用户名、原因搜索。
async fn list_bans(State(state): State<AppState>, _s: AdminSession, q: AdminQuery) -> R {
    let bans = state.db.list_bans(q.text("q")).await?;
    Ok(ok_json(&json!({"bans": bans, "total": bans.len()})))
}

/// 手动将某个 Discord ID 加入黑名单。
async fn create_ban(State(state): State<AppState>, _s: AdminSession, req: Request) -> R {
    let (_, body) = json_of(req).await?;
    let st = state.clone();
    let discord_id = primitives::strip(&pstr(&body, "discord_id")).to_string();
    if !gate_core::validation::is_decimal(&discord_id) {
        return Err(http(422, "Discord ID 必须为纯数字"));
    }
    let reason = primitives::take_chars(
        primitives::strip(&pstr_or(&body, "reason", "站长手动封禁")),
        100,
    )
    .to_string();
    match st.registration.clone() {
        Some(reg) => reg.ban(&discord_id, &reason, "站长").await?,
        None => {
            let info = gate_db::repo::discord::BanInfo {
                reason: reason.clone(),
                banned_by: "站长".into(),
                ..Default::default()
            };
            st.db.ban_discord(&discord_id, &info, st.now()).await?;
        }
    }
    log_action(
        &st.db,
        "站长",
        "添加黑名单",
        &format!("Discord:{discord_id}"),
        &reason,
        true,
        st.now(),
    )
    .await;
    let resp = ok_json(&json!({"ok": true, "discord_id": discord_id}));
    Ok(mark_logged(resp))
}

/// 将某个 Discord ID 移出黑名单（解封）。
async fn delete_ban(
    State(state): State<AppState>,
    _s: AdminSession,
    AdminPath(discord_id): AdminPath<String>,
) -> R {
    let st = state.clone();
    let discord_id = primitives::strip(&discord_id).to_string();
    if !st.db.unban_discord(&discord_id).await? {
        return Err(http(404, "该账号不在黑名单中"));
    }
    log_action(
        &st.db,
        "站长",
        "移出黑名单",
        &format!("Discord:{discord_id}"),
        "已解除封禁，允许重新领取 Key",
        true,
        st.now(),
    )
    .await;
    let resp = ok_json(&json!({"ok": true, "discord_id": discord_id}));
    Ok(mark_logged(resp))
}

// ----------------------------------------------------------------------------- 标签

const TAG_MAX_LEN: usize = 12;
const TAG_NOTE_MAX: usize = 200;

/// 自动标签：近 24 小时因这些「成员自己能改」的原因被拒 / 失败（按 detail 识别；顺序即优先级）。
const AUTO_TAG_PATTERNS: [(&str, &str); 2] = [
    ("角色超 6 个", "%角色太多%"),
    ("请求过快", "%请求过于频繁%"),
];

/// 给成员打 / 改手动标签（只在后台显示）。body: `{tag, note}`
async fn put_key_tag(
    State(state): State<AppState>,
    _s: AdminSession,
    AdminPath(key_id): AdminPath<i64>,
    req: Request,
) -> R {
    let st = state.clone();
    if load_key(&st, key_id).await?.is_none() {
        return Err(http(404, NO_KEY));
    }
    let (_, body) = json_of(req).await?;
    // `" ".join(str(...).split())[:12]`：任何空白折成一个空格
    let joined = pstr(&body, "tag")
        .split(primitives::is_space)
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let tag = primitives::take_chars(&joined, TAG_MAX_LEN).to_string();
    if tag.is_empty() {
        return Err(http(422, "标签不能为空"));
    }
    let note =
        primitives::take_chars(primitives::strip(&pstr(&body, "note")), TAG_NOTE_MAX).to_string();
    let by = {
        let b = primitives::take_chars(primitives::strip(&pstr_or(&body, "by", "站长")), 20)
            .to_string();
        if b.is_empty() {
            "站长".to_string()
        } else {
            b
        }
    };
    st.db
        .upsert_key_tag(key_id, &tag, &note, &by, st.now())
        .await?;
    Ok(ok_json(&json!({"ok": true, "tag": tag})))
}

async fn delete_key_tag(
    State(state): State<AppState>,
    _s: AdminSession,
    AdminPath((key_id, tag)): AdminPath<(i64, String)>,
) -> R {
    if !state.db.delete_key_tag(key_id, &tag).await? {
        return Err(http(404, "没有这个标签"));
    }
    Ok(ok_json(&json!({"ok": true})))
}

/// 误判：解除暂停（风险分、证据在 3.0 已随防分享一起删除，只剩 Key 暂停闸门）。
async fn key_share_clear(
    State(state): State<AppState>,
    _s: AdminSession,
    AdminPath(key_id): AdminPath<i64>,
) -> R {
    state.pauses.remove(key_id);
    state.db.clear_pause(key_id).await?;
    Ok(ok_json(&json!({"ok": true})))
}

// ----------------------------------------------------------------------------- 成员页

type Obj = Map<String, Value>;

fn as_obj(v: &Value) -> &Obj {
    static EMPTY: std::sync::LazyLock<Obj> = std::sync::LazyLock::new(Map::new);
    v.as_object().unwrap_or(&EMPTY)
}

fn num_add(a: &Value, b: &Value) -> Value {
    match (
        a.as_i64().filter(|_| !a.is_f64()),
        b.as_i64().filter(|_| !b.is_f64()),
    ) {
        (Some(x), Some(y)) => json!(x + y),
        _ => values::float(values::number(a).unwrap_or(0.0) + values::number(b).unwrap_or(0.0)),
    }
}

/// `x or 0`
fn or0(v: &Value) -> Value {
    if values::enabled(v) {
        v.clone()
    } else {
        json!(0)
    }
}

fn fnum(v: &Value) -> f64 {
    values::number(v).unwrap_or(0.0)
}

/// 站长点过「没问题」打的标签：24 小时内可确认的提醒不再算「要处理」。
const ACK_TAG: &str = "已查看";
const DAY_SECS: f64 = 86400.0;

/// 与 `web/src/pages/admin/sections/members/logic.ts` 的 `acked` 一致。
fn acked_24h(m: &Obj, now: f64) -> bool {
    m.get("tags").and_then(Value::as_array).is_some_and(|tags| {
        tags.iter().any(|t| {
            t.get("tag").and_then(Value::as_str) == Some(ACK_TAG)
                && now - t.get("ts").and_then(Value::as_f64).unwrap_or(0.0) < DAY_SECS
        })
    })
}

/// 「需要站长处理」：与 `web/src/pages/admin/sections/members/logic.ts` 的 `needsAttention` 同口径
/// （两处要一起改）。bad（限流暂停中 / 已过期）点过「没问题」也不豁免；其余（3 天内到期、
/// 自动标签刷屏、疑似/待查/观察标签、今日被重置 ≥2 次）在「已查看」后 24 小时内不算。
/// 停用的 Key 前端列表不显示，也不计入。
fn attention_needed(m: &Obj, now: f64) -> bool {
    if !m.get("enabled").and_then(Value::as_bool).unwrap_or(false) {
        return false;
    }
    let paused = m
        .get("share")
        .and_then(|s| s.get("paused_until"))
        .and_then(Value::as_f64)
        .is_some_and(|until| until > now);
    let expires_at = m.get("expires_at").and_then(Value::as_f64);
    let expired = expires_at.is_some_and(|at| at < now);
    if paused || expired {
        return true;
    }
    if acked_24h(m, now) {
        return false;
    }
    // 3 天内到期
    if expires_at.is_some_and(|at| at - now < 3.0 * DAY_SECS) {
        return true;
    }
    // 自动标签：单纯客户端短时连点「请求过快」不升级；有备注或近 24h ≥ 50 次才要处理
    let loud_auto_tag = m
        .get("auto_tags")
        .and_then(Value::as_array)
        .is_some_and(|tags| {
            tags.iter().any(|t| {
                let tag = t.get("tag").and_then(Value::as_str).unwrap_or("");
                let count = t.get("count").and_then(Value::as_i64).unwrap_or(0);
                let noted = t
                    .get("note")
                    .and_then(Value::as_str)
                    .is_some_and(|n| !n.is_empty());
                tag != "请求过快" && (noted || count >= 50)
            })
        });
    if loud_auto_tag {
        return true;
    }
    // 站长手动打的「疑似 / 待查 / 观察」标签（「已查看」不算）
    let flagged = m.get("tags").and_then(Value::as_array).is_some_and(|tags| {
        tags.iter().any(|t| {
            let tag = t.get("tag").and_then(Value::as_str).unwrap_or("");
            tag != ACK_TAG && ["疑似", "待查", "观察"].iter().any(|w| tag.contains(w))
        })
    });
    if flagged {
        return true;
    }
    // 今天被手动重置额度 ≥ 2 次
    m.get("reset_today")
        .and_then(|r| r.get("count"))
        .and_then(Value::as_i64)
        .is_some_and(|n| n >= 2)
}

/// 每位成员（Key）的今日 / 近 7 天 / 累计用量，以及来源。
///
/// `?filter=all|attention`（缺省 = `all`，完全旧行为）：`attention` 只返回需要站长处理的行，
/// 同时给出 `total`（裁剪前成员行数）/ `attention`（需要处理的行数）/ `filter`（实际生效值）。
/// 前端 `web/src/pages/admin/sections/members/logic.ts` 的 `needsAttention` 与本文件的
/// [`attention_needed`] 是同一条口径，改一边要同时改另一边。
async fn members(State(state): State<AppState>, _s: AdminSession, q: AdminQuery) -> R {
    let policies = state.db.member_policy_directory().await?;
    let filter = match q.text("filter") {
        "" | "all" => "all",
        "attention" => "attention",
        _ => return Err(http(422, "filter 只能是 all 或 attention")),
    };
    let st = state.clone();
    let now = st.now();
    let today = st.day();
    let week_start = st.clock.last_days(now, 7).remove(0);
    let week: HashMap<i64, Value> = st.db.member_usage(&week_start).await?.into_iter().collect();
    let totals: HashMap<i64, i64> = st
        .db
        .generated_image_totals(None)
        .await?
        .into_iter()
        .collect();
    let reg: HashMap<i64, RegistrationRow> = st
        .db
        .registration_rows()
        .await?
        .into_iter()
        .map(|r| (r.key_id, r))
        .collect();
    let since = now - 24.0 * 3600.0;
    let milestones: HashMap<i64, _> = st
        .db
        .member_milestones(since, &|c, d| gate_core::reasons::reason_label(c, d))
        .await?
        .into_iter()
        .collect();
    // 暂停已过期的输出整数 0，没过期的输出浮点剩余秒数
    let pause_map: HashMap<i64, Value> = st
        .db
        .pause_rows()
        .await?
        .into_iter()
        .map(|(k, paused)| {
            let v = if paused > now {
                values::float(paused)
            } else {
                json!(0)
            };
            (k, v)
        })
        .collect();

    // 标签：手动（key_tags，站长 / 运维打的）+ 自动（since 之后因已知的成员侧原因被拒 / 失败）
    let mut manual_tags: HashMap<i64, Vec<TagRow>> = HashMap::new();
    for t in st.db.key_tag_rows().await? {
        manual_tags.entry(t.key_id).or_default().push(t);
    }
    let mut auto_tags: HashMap<i64, Vec<Value>> = HashMap::new();
    for (kid, label, n) in st.db.auto_tag_counts(since, &AUTO_TAG_PATTERNS).await? {
        auto_tags
            .entry(kid)
            .or_default()
            .push(json!({"tag": label, "count": n}));
    }

    // 今日「重置今日额度」痕迹：按服务日起点统计次数与最近一次（从操作日志取，只读）。
    // 起点是**当前服务日**的 7 点（不能用日历日期算，凌晨 0–7 点起点会落在未来）。
    let day_start = st.clock.service_day_start(&today).unwrap_or(0.0);
    let mut reset_map: HashMap<i64, (i64, f64)> = HashMap::new();
    for (a_ts, target) in st.db.quota_reset_marks(day_start).await? {
        let Some(rest) = target.strip_prefix("Key #") else {
            continue;
        };
        let digits: String = rest.chars().take_while(|c| validation::is_nd(*c)).collect();
        let Some(id) = primitives::parse_int(&digits) else {
            continue;
        };
        let e = reset_map.entry(id).or_insert((0, 0.0));
        e.0 += 1;
        if a_ts > e.1 {
            e.1 = a_ts;
        }
    }
    let today_counters = st.db.counter_map(&today).await?;
    let mut out: Vec<Obj> = Vec::new();
    for row in st.db.list_keys().await? {
        let id = row.get("id").and_then(Value::as_i64).unwrap_or(0);
        let counter = today_counters.get(&id).cloned().unwrap_or_default();
        let (images, v5, anlas, requests) =
            (counter.images, counter.v5, counter.anlas, counter.requests);
        let w = week.get(&id).cloned().unwrap_or(Value::Null);
        let wi = |k: &str| values::number(&jget(&w, k)).map_or(0, |x| x as i64);
        let wf = |k: &str| values::number(&jget(&w, k)).unwrap_or(0.0);
        let my_tags = manual_tags.get(&id);
        let invited = my_tags.is_some_and(|t| t.iter().any(|x| x.tag == INVITE_TAG));
        let reg_row = reg.get(&id);
        let ms = milestones.get(&id);
        let mut m = Obj::new();
        let mut put = |k: &str, v: Value| {
            m.insert(k.to_string(), v);
        };
        put("id", jget(&row, "id"));
        put("name", jget(&row, "name"));
        put("enabled", json!(jflag(&row, "enabled")));
        put("invited", json!(invited));
        put("discord_id", json!(reg_row.map(|r| r.discord_id.clone())));
        put("discord", json!(reg_row.map(discord_profile)));
        put("created_at", jget(&row, "created_at"));
        put("last_used_at", jget(&row, "last_used_at"));
        put("expires_at", jget(&row, "expires_at"));
        put(
            "policy",
            serde_json::to_value(
                policies.for_subject(row.get("discord_id").and_then(Value::as_str))?,
            )
            .map_err(|_| crate::error::InternalError("成员规则编码失败".into()))?,
        );
        put(
            "today",
            json!({"images": images, "legacy_free_images":counter.legacy_free_images,"v5": v5, "anlas": values::float(primitives::round(anlas, 2)),
                   "requests": requests}),
        );
        put(
            "week",
            json!({"images": wi("images"), "v5": wi("v5"),
                   "anlas": values::float(primitives::round(wf("anlas"), 2)), "requests": wi("requests")}),
        );
        put("total_images", json!(totals.get(&id).copied().unwrap_or(0)));
        // 筛选用：领取时间（Discord 领取优先，否则建 Key 时间）、首次 / 最近成功出图、24h 被拒次数
        put(
            "registered_at",
            match reg_row {
                Some(r) => json!(r.created_at),
                None => jget(&row, "created_at"),
            },
        );
        put("first_image_at", json!(ms.and_then(|m| m.first_image_at)));
        put("last_image_at", json!(ms.and_then(|m| m.last_image_at)));
        put("rejected_24h", json!(ms.map_or(0, |m| m.rejected_24h)));
        put(
            "rejected_why",
            json!(ms.and_then(|m| m.rejected_why.clone()).unwrap_or_default()),
        );
        // 字段名沿用 2.x 的 "share"（前端和 admin_snapshot 按这个名字读）；内容只剩 Key 临时暂停的到期时间
        put(
            "share",
            pause_map
                .get(&id)
                .map_or(Value::Null, |p| json!({"paused_until": p})),
        );
        put(
            "reset_today",
            reset_map.get(&id).map_or(
                Value::Null,
                |(c, t)| json!({"count": c, "last_ts": values::float(*t)}),
            ),
        );
        put(
            "tags",
            Value::Array(
                my_tags
                    .into_iter()
                    .flatten()
                    .map(|t| json!({"tag": t.tag, "note": t.note, "by": t.by, "ts": values::float(t.ts)}))
                    .collect(),
            ),
        );
        put(
            "auto_tags",
            Value::Array(auto_tags.get(&id).cloned().unwrap_or_default()),
        );
        out.push(m);
    }

    // 成员为中心合并（10-14 站长）：同一 Discord 成员重置 / 换 Key 留下的多把 Key 归并成一条，
    // 用量与统计跨 Key 汇总；key_count / all_key_ids 供前端展示与跨 Key 查生成记录。
    let mut order: Vec<String> = Vec::new();
    let mut by_discord: HashMap<String, Vec<Obj>> = HashMap::new();
    let mut singles: Vec<Obj> = Vec::new();
    for e in out {
        match e
            .get("discord_id")
            .and_then(Value::as_str)
            .map(str::to_string)
        {
            None => {
                // 没有 Discord 绑定的 Key（站长手建 / 邀请 Key）也要带 key_count / all_key_ids：
                // 前端成员列表与「生成记录」按这两个字段取数，缺了会 TypeError（2026-10-17 审查）。
                let mut e = e;
                let id = e["id"].clone();
                e.insert("key_count".into(), json!(1));
                e.insert("all_key_ids".into(), json!([id]));
                singles.push(e);
            }
            Some(did) => {
                if !by_discord.contains_key(&did) {
                    order.push(did.clone());
                }
                by_discord.entry(did).or_default().push(e);
            }
        }
    }
    let mut merged: Vec<Obj> = Vec::new();
    for did in order {
        let mut entries = by_discord.remove(&did).unwrap_or_default();
        if entries.len() == 1 {
            let mut e = entries.remove(0);
            let id = e["id"].clone();
            e.insert("key_count".into(), json!(1));
            e.insert("all_key_ids".into(), json!([id]));
            singles.push(e);
            continue;
        }
        // 已启用的在前，同组里最近用过的在前（稳定排序）
        entries.sort_by(|a, b| {
            let key = |e: &Obj| {
                (
                    !e["enabled"].as_bool().unwrap_or(false),
                    -fnum(&or0(&e["last_used_at"])),
                )
            };
            let (ka, kb) = (key(a), key(b));
            ka.0.cmp(&kb.0)
                .then(ka.1.partial_cmp(&kb.1).unwrap_or(std::cmp::Ordering::Equal))
        });
        let mut head = entries[0].clone();
        head.insert("key_count".into(), json!(entries.len()));
        head.insert(
            "all_key_ids".into(),
            Value::Array(entries.iter().map(|e| e["id"].clone()).collect()),
        );
        for other in &entries[1..] {
            let enabled = head["enabled"].as_bool().unwrap_or(false)
                || other["enabled"].as_bool().unwrap_or(false);
            head.insert("enabled".into(), json!(enabled));
            for f in ["today", "week"] {
                let mut h = as_obj(&head[f]).clone();
                for (k, v) in as_obj(&other[f]) {
                    if v.is_number() && h.get(k).is_some_and(Value::is_number) {
                        let sum = num_add(&h[k], v);
                        h.insert(k.clone(), sum);
                    }
                }
                head.insert(f.into(), Value::Object(h));
            }
            head.insert(
                "total_images".into(),
                num_add(&head["total_images"], &other["total_images"]),
            );
            head.insert(
                "rejected_24h".into(),
                num_add(&head["rejected_24h"], &other["rejected_24h"]),
            );
            // max(a or 0, b or 0)：b 严格更大才取 b
            let (a, b) = (or0(&head["last_used_at"]), or0(&other["last_used_at"]));
            head.insert(
                "last_used_at".into(),
                if fnum(&b) > fnum(&a) { b } else { a },
            );
            if values::enabled(&other["last_image_at"]) {
                let a = or0(&head["last_image_at"]);
                let b = other["last_image_at"].clone();
                head.insert(
                    "last_image_at".into(),
                    if fnum(&b) > fnum(&a) { b } else { a },
                );
            }
            if values::enabled(&other["first_image_at"]) {
                let b = other["first_image_at"].clone();
                let a = if values::enabled(&head["first_image_at"]) {
                    head["first_image_at"].clone()
                } else {
                    b.clone()
                };
                head.insert(
                    "first_image_at".into(),
                    if fnum(&b) < fnum(&a) { b } else { a },
                );
            }
            let seen: Vec<Value> = head["tags"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|t| t["tag"].clone())
                .collect();
            let mut tags: Vec<Value> = head["tags"].as_array().cloned().unwrap_or_default();
            tags.extend(
                other["tags"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|t| !seen.contains(&t["tag"]))
                    .cloned(),
            );
            head.insert("tags".into(), Value::Array(tags));
        }
        merged.push(head);
    }
    singles.extend(merged);
    // 需要站长处理的行：口径与后台前端 logic.ts 的 needsAttention 一致（含「已查看」24 小时豁免；
    // 停用的 Key 前端列表本来就不显示，也不计入）。`total` 是裁剪前的成员行数。
    let total = singles.len();
    let attention = singles.iter().filter(|m| attention_needed(m, now)).count();
    let members: Vec<Obj> = if filter == "attention" {
        singles
            .into_iter()
            .filter(|m| attention_needed(m, now))
            .collect()
    } else {
        singles
    };
    Ok(ok_json(&json!({
        "members": members,
        "inactivity_days": st.env.key_inactivity_delete_days,
        "total": total,
        "attention": attention,
        "filter": filter,
    })))
}
