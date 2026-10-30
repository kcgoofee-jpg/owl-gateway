//! 后台「成员」接口（Key / 成员 / 黑名单 / 券 / 标签）。
//!
//! 状态码、`{"detail": ...}` 文案、JSON 键顺序都是固定契约：后台前端按它们渲染，不要随手改。
//! 3.0 已删除的功能不移植：`anlas_auto` 自动分配（含 `anlas_mode`）、`v5_pinned`（V5 基础值）、
//! 防分享证据；对应的数据库列原样留着，这里不读不写，也不再出现在返回的 JSON 里。

pub(super) use super::mark_logged;
use super::member_ops;
use super::req::{AdminPath, AdminQuery, R, http, json_of};
use crate::error::ApiError;
use crate::error::ok_json;
use crate::session::AdminSession;
use crate::state::AppState;
use axum::Router;
use axum::extract::{Request, State};
use axum::routing::{delete, get, patch, post, put};
use gate_core::{primitives, validation, values};
use gate_db::repo::admin_members::RegistrationRow;
use gate_db::repo::counters::Counter;
use serde_json::{Value, json};

mod view;

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

pub(super) const NO_KEY: &str = "key 不存在";
// ----------------------------------------------------------------------------- 小工具

fn jget(row: &Value, name: &str) -> Value {
    row.get(name).cloned().unwrap_or(Value::Null)
}

fn jflag(row: &Value, name: &str) -> bool {
    row.get(name).is_some_and(values::enabled)
}

pub(super) fn gen_key() -> String {
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
pub(super) fn key_json(
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

pub(super) async fn load_key(state: &AppState, key_id: i64) -> Result<Option<Value>, ApiError> {
    Ok(state.db.get_key(key_id).await?)
}

/// 一把 Key 的完整 JSON（今日计数 + 累计生成张数）。
pub(super) async fn full_key_json(
    state: &AppState,
    row: &Value,
    key_id: i64,
) -> Result<Value, ApiError> {
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
    Ok(ok_json(&member_ops::list_keys_with_counters(&state).await?))
}

/// 一键新建邀请 Key（10-11 站长）：自动编号「邀请-N」、打「邀请」标签、额度交给算法，其余和 /register 发的一样。
/// 成员拿到后在 Discord 用 /register 填这把 Key 完成绑定，后台自动认出是谁。
async fn create_invite_key(State(state): State<AppState>, _s: AdminSession) -> R {
    let resp = ok_json(&member_ops::create_invite_key(&state).await?);
    Ok(mark_logged(resp))
}

async fn regenerate_key(
    State(state): State<AppState>,
    _s: AdminSession,
    AdminPath(key_id): AdminPath<i64>,
) -> R {
    let mut resp = ok_json(&member_ops::regenerate_key(&state, key_id).await?);
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
    Ok(ok_json(&member_ops::patch_key(&state, key_id, body).await?))
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
        .map(|(_, b)| member_ops::reset_scope_of(&b))
        .unwrap_or(gate_db::repo::counters::ResetScope::V45_ONLY);
    Ok(ok_json(
        &member_ops::reset_quota(&state, key_id, scope).await?,
    ))
}

/// 发一张重置券（目前只有 reset，有效期 1～30 天）。
async fn give_coupon(
    State(state): State<AppState>,
    _s: AdminSession,
    AdminPath(key_id): AdminPath<i64>,
    req: Request,
) -> R {
    let (_, body) = json_of(req).await?;
    Ok(ok_json(
        &member_ops::add_reset_coupon(&state, key_id, &body).await?,
    ))
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
    let reason = q.get("reason").unwrap_or("站长手动删除并封禁").to_string();
    Ok(ok_json(
        &member_ops::delete_member_key(&state, key_id, ban, &reason).await?,
    ))
}

/// 成员页：视图模型在 `members/view.rs`，这里只取参数。
async fn members(State(state): State<AppState>, _s: AdminSession, q: AdminQuery) -> R {
    Ok(ok_json(
        &view::members_view(&state, q.text("filter")).await?,
    ))
}

// ----------------------------------------------------------------------------- 黑名单

/// 获取黑名单列表，支持按 ID、用户名、原因搜索。
async fn list_bans(State(state): State<AppState>, _s: AdminSession, q: AdminQuery) -> R {
    Ok(ok_json(&member_ops::list_bans(&state, q.text("q")).await?))
}

/// 手动将某个 Discord ID 加入黑名单。
async fn create_ban(State(state): State<AppState>, _s: AdminSession, req: Request) -> R {
    let (_, body) = json_of(req).await?;
    let resp = ok_json(&member_ops::create_ban(&state, &body).await?);
    Ok(mark_logged(resp))
}

/// 将某个 Discord ID 移出黑名单（解封）。
async fn delete_ban(
    State(state): State<AppState>,
    _s: AdminSession,
    AdminPath(discord_id): AdminPath<String>,
) -> R {
    let resp = ok_json(&member_ops::delete_ban(&state, &discord_id).await?);
    Ok(mark_logged(resp))
}

// ----------------------------------------------------------------------------- 标签

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
    let (_, body) = json_of(req).await?;
    Ok(ok_json(
        &member_ops::set_key_tag(&state, key_id, &body).await?,
    ))
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
    Ok(ok_json(&member_ops::clear_pause(&state, key_id).await?))
}

// ----------------------------------------------------------------------------- 成员页
