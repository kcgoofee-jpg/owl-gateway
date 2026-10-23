//! 后台「成员」接口（旧版 `app/admin.py` 的 Key / 成员 / 黑名单 / 券 / 标签部分）。
//!
//! 照旧版逐条移植：状态码、`{"detail": ...}` 文案、JSON 键顺序、写进库的行都与旧版一致。
//! 3.0 已删除的功能不移植：`anlas_auto` 自动分配（含 `anlas_mode`）、`v5_pinned`（V5 基础值）、
//! 防分享证据；对应的数据库列原样留着，这里不读不写，也不再出现在返回的 JSON 里。

use super::mark_logged;
use super::num::{strict_float, strict_int};
use super::req::{AdminPath, AdminQuery, R, http, json_of, pstr, pstr_or};
use crate::action_log::log_action;
use crate::body::JsonObject;
use crate::defaults;
use crate::error::ApiError;
use crate::error::ok_json;
use crate::ops;
use crate::registration::{DiscordUser, clip, member_label};
use crate::session::AdminSession;
use crate::state::AppState;
use axum::Router;
use axum::extract::{Request, State};
use axum::routing::{delete, get, patch, post, put};
use gate_core::{features, pyc, pyext, pyval};
use gate_db::repo::counters::Counter;
use gate_db::repo::{
    INVITE_TAG, ROLE_INVITE, ROLE_LABELS, ROLE_REGULAR,
    admin_members::{BindOutcome, BindProfile, RegistrationRow, TagRow},
    keys::NewKey,
};
use serde_json::{Map, Value, json};
use std::collections::HashMap;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/admin/api/keys", get(list_keys).post(create_key))
        .route("/admin/api/keys/invite", post(create_invite_key))
        .route(
            "/admin/api/keys/{key_id}",
            patch(patch_key).delete(delete_key),
        )
        .route("/admin/api/keys/{key_id}/regenerate", post(regenerate_key))
        .route("/admin/api/keys/{key_id}/role", post(switch_key_role))
        .route(
            "/admin/api/keys/{key_id}/reset-daily-image-quota",
            post(reset_daily_image_quota),
        )
        .route("/admin/api/keys/{key_id}/bind-discord", post(bind_discord))
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
const ANLAS_CAP: &str = "开通 Anlas 时必须设置每日或每月 Anlas 上限（0 = 不限），不能两项都留空";
const FEATURES_HINT: &str = "image,upscale,augment,vibe,tags";
const ROLE_HINT: &str = "身份组只能是 regular（普通）或 invite（邀请）；站长 / 测试身份组已删除";

// ----------------------------------------------------------------------------- 小工具

fn jget(row: &Value, name: &str) -> Value {
    row.get(name).cloned().unwrap_or(Value::Null)
}

fn jflag(row: &Value, name: &str) -> bool {
    row.get(name).is_some_and(pyval::truthy)
}

fn role_label(role: &str) -> &'static str {
    ROLE_LABELS
        .iter()
        .find(|(r, _)| *r == role)
        .map_or("普通", |(_, l)| l)
}

fn valid_role(role: &str) -> bool {
    matches!(role, ROLE_INVITE | ROLE_REGULAR)
}

/// `str(x).strip().lower()`
fn norm_role(v: &str) -> String {
    pyc::strip(v).to_lowercase()
}

/// `gen_key("nai")`
pub(super) fn gen_key() -> String {
    gate_core::policy::gen_key("nai", &rand::random::<[u8; 24]>())
}

/// 旧版 `_features_field`：缺省 / null = 沿用旧行为（全局开启的都可用）；列表 = 仅允许所列功能。
fn features_field(body: &JsonObject) -> Result<Option<String>, ApiError> {
    let Some(value) = body.get("features").filter(|v| !v.is_null()) else {
        return Ok(None);
    };
    let ok = value.as_array().is_some_and(|a| {
        a.iter().all(|v| {
            v.as_str()
                .is_some_and(|s| features::FEATURES.iter().any(|(n, _)| *n == s))
        })
    });
    if !ok {
        return Err(http(
            422,
            format!("features 必须是功能名列表：{FEATURES_HINT}"),
        ));
    }
    let names: Vec<&str> = value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    Ok(features::dump(Some(names)))
}

/// 新建 Key 未指定功能时，沿用「新成员默认开通的功能」（默认只有文生图），而不是全部功能。
async fn new_key_features(state: &AppState, body: &JsonObject) -> Result<Option<String>, ApiError> {
    if body.contains_key("features") {
        return features_field(body);
    }
    Ok(ops::default_new_key_features(state).await?)
}

/// Discord 头像地址：有头像哈希用自己的，没有用 Discord 按 ID 分配的默认头像。
pub(crate) fn discord_avatar_url(id: &str, avatar: Option<&str>) -> String {
    match avatar.filter(|a| !a.is_empty()) {
        Some(a) => format!("https://cdn.discordapp.com/avatars/{id}/{a}.png?size=64"),
        None => {
            let n = pyext::py_int_str(id).unwrap_or(0);
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

/// 身份组：邀请 / 普通。`invite` 由调用方按各自的来源（邀请标签）判断。
fn role_of(invite: bool) -> &'static str {
    if invite { ROLE_INVITE } else { ROLE_REGULAR }
}

/// 旧版 `_key_json`。`role` 给了就覆盖（Key 列表按「邀请」标签识别身份组）；
/// 不给时按普通——旧版如此，所以修改 / 新建接口返回的 role 不会是「邀请」。
fn key_json(row: &Value, c: &Counter, generated: i64, role: Option<&str>) -> Value {
    let role = role.unwrap_or(ROLE_REGULAR);
    let anlas = c.anlas;
    json!({
        "id": jget(row, "id"),
        "name": jget(row, "name"),
        "token": jget(row, "token"),
        "enabled": jflag(row, "enabled"),
        "daily_images": jget(row, "daily_images"),
        "daily_anlas": jget(row, "daily_anlas"),
        "daily_v5": jget(row, "daily_v5"),
        "monthly_anlas": jget(row, "monthly_anlas"),
        "rpm": jget(row, "rpm"),
        "allow_anlas": jflag(row, "allow_anlas"),
        "allow_img2img": jflag(row, "allow_img2img"),
        "exclude_global_v5": jflag(row, "exclude_global_v5"),
        "image_model_scope": jget(row, "image_model_scope"),
        "quota_auto": pyext::num_eq_int(row.get("quota_auto"), 1),
        "role": role,
        "role_label": role_label(role),
        "features": features::key_features(row),
        "expires_at": jget(row, "expires_at"),
        "created_at": jget(row, "created_at"),
        "last_used_at": jget(row, "last_used_at"),
        "used": {
            "images": c.images,
            "generated_images_total": generated,
            "legacy_free_images": c.legacy_free_images,
            "anlas": pyval::float(pyc::round(anlas, 2)),
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
    Ok(key_json(row, &c, generated, None))
}

// ----------------------------------------------------------------------------- Key 列表 / 新建

async fn list_keys(State(state): State<AppState>, _s: AdminSession) -> R {
    let st = &state;
    let rows = st.db.list_keys().await?;
    let today = st.day();
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
        let c = st.db.get_counter(id, &today).await?;
        let role = role_of(invite.contains(&id));
        out.push(key_json(
            r,
            &c,
            totals.get(&id).copied().unwrap_or(0),
            Some(role),
        ));
    }
    Ok(ok_json(&json!({"keys": out})))
}

/// 一键新建邀请 Key（10-11 站长）：自动编号「邀请-N」、打「邀请」标签、额度交给算法，其余和 /register 发的一样。
/// 成员拿到后在 Discord 用 /register 填这把 Key 完成绑定，后台自动认出是谁。
async fn create_invite_key(State(state): State<AppState>, _s: AdminSession) -> R {
    let st = &state;
    // 和 /register 一样读「新成员默认」（后台「领 Key」的设置，没设过用 defaults 里的常量）
    let cfg = ops::registration_settings(st).await?;
    let (daily_images, daily_v5, features_list, image_scope, expires_days) = (
        cfg.daily_images,
        cfg.daily_v5,
        cfg.features,
        cfg.image_scope,
        cfg.expires_days,
    );
    // 旧版 `re.match(r"邀请-(\d+)", name)`：只看开头，数字可以是任何 Unicode 十进制数字
    let max_no = st
        .db
        .key_names()
        .await?
        .iter()
        .filter_map(|n| {
            let rest = n.strip_prefix("邀请-")?;
            let digits: String = rest.chars().take_while(|c| pyext::is_nd(*c)).collect();
            pyc::parse_int(&digits)
        })
        .max()
        .unwrap_or(0);
    let name = format!("邀请-{}", max_no + 1);
    let now = st.now();
    let new = NewKey {
        name: name.clone(),
        token: gen_key(),
        enabled: true,
        daily_images,
        daily_anlas: 0.0,
        daily_v5,
        monthly_anlas: 0.0,
        daily_text_tokens: 0,
        rpm: defaults::MANUAL_KEY_RPM,
        allow_anlas: false,
        allow_img2img: false,
        exclude_global_v5: false,
        image_model_scope: image_scope,
        expires_at: (expires_days > 0).then_some(now + expires_days as f64 * 86400.0),
        features: features::dump(features_list),
        discord_id: None,
    };
    let row = st.db.create_key(&new, now).await?;
    let id = row.get("id").and_then(Value::as_i64).unwrap_or(0);
    st.db
        .upsert_key_tag(
            id,
            INVITE_TAG,
            "站长发的邀请 Key：成员用 /register 填它完成绑定；只由站长手动删除",
            "站长",
            now,
        )
        .await?;
    log_action(&st.db, "站长", "新建邀请 Key", &name, "", true, st.now()).await;
    let row = st.db.get_key(id).await?.unwrap_or(Value::Null);
    let c = st.db.get_counter(id, &st.day()).await?;
    let resp = ok_json(&json!({"key": key_json(&row, &c, 0, None)}));
    Ok(mark_logged(resp))
}

async fn create_key(State(state): State<AppState>, _s: AdminSession, req: Request) -> R {
    let (_, body) = json_of(req).await?;
    let st = &state;
    // 没填的字段用「新成员默认」（后台「领 Key」的设置）和 defaults 里的常量；弹窗从 /admin/api/ops 取同一份
    let reg = ops::registration_settings(st).await?;
    let field = |name: &str, default: Value| body.get(name).cloned().unwrap_or(default);

    let daily_images = strict_int(
        &field("daily_images", json!(reg.daily_images)),
        0,
        1_000_000,
        "daily_images",
    )?;
    let monthly_anlas = strict_float(
        &field("monthly_anlas", json!(defaults::MANUAL_KEY_MONTHLY_ANLAS)),
        0.0,
        100_000.0,
        "monthly_anlas",
    )?;
    let daily_anlas = strict_float(
        &field("daily_anlas", json!(defaults::MANUAL_KEY_DAILY_ANLAS)),
        0.0,
        100_000.0,
        "daily_anlas",
    )?;
    let daily_v5 = strict_int(
        &field("daily_v5", json!(reg.daily_v5)),
        0,
        100_000,
        "daily_v5",
    )?;
    let rpm = strict_int(
        &field("rpm", json!(defaults::MANUAL_KEY_RPM)),
        1,
        600,
        "rpm",
    )?;
    let expires_days = strict_int(
        &field("expires_days", json!(reg.expires_days)),
        0,
        3650,
        "expires_days",
    )?;
    let now = st.now();
    let expires_at = (expires_days > 0).then_some(now + expires_days as f64 * 86400.0);
    let scope = if body.get("image_model_scope") == Some(&json!("all")) {
        "all"
    } else {
        "legacy"
    };
    let truthy = |name: &str| body.get(name).is_some_and(pyval::truthy);
    // 配额检查只认 > 0 的上限：allow_anlas 开着而日 / 月上限都是 0 = 个人 Anlas 完全无上限，
    // 可无限烧共享账号的付费点数（旧版的文案写「0 = 不限」，但实际 0 + 0 会被拒，照旧）
    if truthy("allow_anlas") && daily_anlas <= 0.0 && monthly_anlas <= 0.0 {
        return Err(http(422, ANLAS_CAP));
    }
    let role = match body.get("role") {
        Some(v) if pyval::truthy(v) => Some(norm_role(&pyext::py_str(v))),
        _ => None,
    };
    if role.as_deref().is_some_and(|r| !valid_role(r)) {
        return Err(http(422, ROLE_HINT));
    }
    let name = {
        let n = pyc::take_chars(pyc::strip(&pstr(&body, "name")), 60).to_string();
        if n.is_empty() {
            "未命名".to_string()
        } else {
            n
        }
    };
    let new = NewKey {
        name,
        token: gen_key(),
        enabled: true,
        daily_images,
        daily_anlas,
        daily_v5,
        monthly_anlas,
        // 文本接口已删除：列保留（只增不减），新 Key 写 0，不再读
        daily_text_tokens: 0,
        rpm,
        allow_anlas: truthy("allow_anlas"),
        allow_img2img: truthy("allow_img2img"),
        exclude_global_v5: truthy("exclude_global_v5"),
        image_model_scope: scope.to_string(),
        expires_at,
        features: new_key_features(st, &body).await?,
        discord_id: None,
    };
    let row = st.db.create_key(&new, now).await?;
    let id = row.get("id").and_then(Value::as_i64).unwrap_or(0);
    // 后台手动建的 Key 用站长填的额度和模型范围：标为手动，动态额度算法不覆盖（否则 10 分钟内会被改成算法默认值、
    // 连 V4.5-only 也会被开成 V5）。要交给算法，可在成员页切回「自动」。
    st.db.set_quota_auto(id, -1).await?;
    if let Some(role) = &role {
        st.db.set_key_role(id, role, st.now()).await?;
    }
    let row = st.db.get_key(id).await?.unwrap_or(Value::Null);
    let c = st.db.get_counter(id, &st.day()).await?;
    Ok(ok_json(
        &json!({"key": key_json(&row, &c, 0, role.as_deref())}),
    ))
}

// ----------------------------------------------------------------------------- 单把 Key 的操作

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

/// 从「仅 V4.5」开到 V5 时给一个默认日额度，和早期成员一致。
const QUICK_V5_DAILY: i64 = 15;

async fn patch_key(
    State(state): State<AppState>,
    _s: AdminSession,
    AdminPath(key_id): AdminPath<i64>,
    req: Request,
) -> R {
    let st = state.clone();
    if load_key(&st, key_id).await?.is_none() {
        return Err(http(404, NO_KEY));
    }
    let (_, mut body) = json_of(req).await?;
    let quota = body.get("quota").cloned().unwrap_or(Value::Null);
    let quota_s = quota.as_str();
    if !(quota.is_null() || matches!(quota_s, Some("auto" | "manual"))) {
        return Err(http(422, "quota 只能是 auto（算法）或 manual（手动）"));
    }
    if !quota.is_null() {
        // 2026-10-11 站长：额度只有两种——「算法」全部交给算法；「手动」填多少就是多少，不和算法混用
        if quota_s == Some("auto") {
            for k in [
                "daily_images",
                "daily_v5",
                "daily_anlas",
                "monthly_anlas",
                "allow_anlas",
                "exclude_global_v5",
                "image_model_scope",
                "features",
            ] {
                pyval::pop(&mut body, k);
            }
        }
    }
    let mut fields: Map<String, Value> = Map::new();
    if let Some(v) = body.get("name") {
        let n = pyc::take_chars(
            pyc::strip(&if pyval::truthy(v) {
                pyext::py_str(v)
            } else {
                String::new()
            }),
            60,
        )
        .to_string();
        fields.insert(
            "name".into(),
            Value::String(if n.is_empty() { "未命名".into() } else { n }),
        );
    }
    if let Some(v) = body.get("enabled") {
        fields.insert("enabled".into(), Value::Bool(pyval::truthy(v)));
    }
    if let Some(v) = body.get("daily_images") {
        fields.insert(
            "daily_images".into(),
            json!(strict_int(v, 0, 1_000_000, "daily_images")?),
        );
    }
    if let Some(v) = body.get("daily_anlas") {
        fields.insert(
            "daily_anlas".into(),
            pyval::float(strict_float(v, 0.0, 100_000.0, "daily_anlas")?),
        );
    }
    if let Some(v) = body.get("daily_v5") {
        fields.insert(
            "daily_v5".into(),
            json!(strict_int(v, 0, 100_000, "daily_v5")?),
        );
    }
    if let Some(v) = body.get("monthly_anlas") {
        fields.insert(
            "monthly_anlas".into(),
            pyval::float(strict_float(v, 0.0, 100_000.0, "monthly_anlas")?),
        );
    }
    if let Some(v) = body.get("rpm") {
        fields.insert("rpm".into(), json!(strict_int(v, 1, 600, "rpm")?));
    }
    for k in ["allow_anlas", "allow_img2img", "exclude_global_v5"] {
        if let Some(v) = body.get(k) {
            fields.insert(k.into(), Value::Bool(pyval::truthy(v)));
        }
    }
    if let Some(v) = body.get("role") {
        let role = norm_role(&if pyval::truthy(v) {
            pyext::py_str(v)
        } else {
            String::new()
        });
        if !valid_role(&role) {
            return Err(http(422, ROLE_HINT));
        }
        st.db.set_key_role(key_id, &role, st.now()).await?;
    }
    if let Some(v) = body.get("image_model_scope") {
        let scope = if v.as_str() == Some("all") {
            "all"
        } else {
            "legacy"
        };
        fields.insert("image_model_scope".into(), json!(scope));
    }
    if body.contains_key("features") {
        fields.insert("features".into(), json!(features_field(&body)?));
    }
    if body.get("image_model_scope") == Some(&json!("all")) && !body.contains_key("daily_v5") {
        let before_v5 = load_key(&st, key_id).await?;
        if before_v5.is_some_and(|b| !jflag(&b, "daily_v5")) {
            fields.insert("daily_v5".into(), json!(QUICK_V5_DAILY)); // 从「仅 V4.5」开到 V5 时给一个默认日额度
        }
    }
    if let Some(v) = body.get("expires_days") {
        let d = strict_int(v, 0, 3650, "expires_days")?;
        fields.insert(
            "expires_at".into(),
            if d > 0 {
                pyval::float(st.now() + d as f64 * 86400.0)
            } else {
                Value::Null
            },
        );
    }
    let before = load_key(&st, key_id).await?.unwrap_or(Value::Null);
    if quota_s == Some("auto") {
        // Anlas 也交给算法：先收回手动授权（Sonnet 审查 P1：否则手动的 150/天永久保留）
        fields.insert("exclude_global_v5".into(), json!(false));
        fields.insert("features".into(), Value::Null);
        fields.insert("allow_anlas".into(), json!(false));
        fields.insert("daily_anlas".into(), pyval::float(0.0));
    }
    // 同 create_key：开 Anlas 必须留一道上限，日 / 月全为 0 等于个人无上限（2026-10-13 审查）
    let pick = |name: &str| {
        fields
            .get(name)
            .or_else(|| before.get(name))
            .cloned()
            .unwrap_or(Value::Null)
    };
    let le0 = |v: Value| pyval::number_like(&v).is_some_and(|x| x <= 0.0);
    if pyval::truthy(&pick("allow_anlas")) && le0(pick("daily_anlas")) && le0(pick("monthly_anlas"))
    {
        return Err(http(422, ANLAS_CAP));
    }
    st.db.update_key(key_id, &fields).await?;
    let quota_mode_auto = quota_s == Some("auto");
    if quota_s == Some("manual") {
        st.db.set_quota_auto(key_id, -1).await?;
    } else {
        // 只有额度 / 模型的值真的变了才转为手动（2026-10-10：提交了一个没变的「模型 = 全部」就被冻结在 18 张）
        let quota_changed = ["daily_images", "daily_v5", "image_model_scope"]
            .iter()
            .any(|k| {
                fields
                    .get(*k)
                    .is_some_and(|v| !pyval::py_eq(v, before.get(*k).unwrap_or(&Value::Null)))
            });
        if quota_mode_auto || quota_changed {
            // 手动改了额度或模型 → 这把 Key 由站长管理（-1），动态额度算法不再覆盖；选「交给算法」恢复为 1 并立即重算
            st.db
                .set_quota_auto(key_id, if quota_mode_auto { 1 } else { -1 })
                .await?;
            if quota_mode_auto && let Err(e) = crate::jobs::quota::run_now(&st).await {
                let (ty, msg) = match &e {
                    crate::error::ApiError::Internal(i) => ("InternalError", i.0.clone()),
                    crate::error::ApiError::Http(h) => ("HTTPException", pyval::py_str(&h.detail)),
                    crate::error::ApiError::Gate(g) => ("GateError", g.message.clone()),
                };
                st.bugs.capture(crate::bugs::Report {
                    source: "quota_algo",
                    error: Some((ty, &msg)),
                    ..Default::default()
                });
            }
        }
    }
    let row = load_key(&st, key_id).await?.unwrap_or(Value::Null);
    Ok(ok_json(
        &json!({"key": full_key_json(&st, &row, key_id).await?}),
    ))
}

/// 手动切换账号身份组（站长 / 测试、邀请、普通）。
async fn switch_key_role(
    State(state): State<AppState>,
    _s: AdminSession,
    AdminPath(key_id): AdminPath<i64>,
    req: Request,
) -> R {
    let st = state.clone();
    let Some(key) = load_key(&st, key_id).await? else {
        return Err(http(404, NO_KEY));
    };
    let (_, body) = json_of(req).await?;
    let role = norm_role(&pstr(&body, "role"));
    if !valid_role(&role) {
        return Err(http(422, ROLE_HINT));
    }
    st.db.set_key_role(key_id, &role, st.now()).await?;
    let label = role_label(&role);
    let exemption = if role == ROLE_REGULAR {
        "（纳入闲置回收）"
    } else {
        "（豁免闲置回收）"
    };
    log_action(
        &st.db,
        "站长",
        "修改账号身份组",
        &format!("Key #{key_id} {}", pyval::py_str(&jget(&key, "name"))),
        &format!("切换为「{label}」{exemption}"),
        true,
        st.now(),
    )
    .await;
    let resp = ok_json(&json!({"ok": true, "key_id": key_id, "role": role, "role_label": label}));
    Ok(mark_logged(resp))
}

/// 请求里的 `scope`（如 `["v5", "anlas"]`）→ 重置范围。V4.5 一定重置；没传 / 不是数组 = 只重置 V4.5（站长 10-18）。
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

/// 换绑时向 Discord 读资料的超时（秒）：读不到就用站长填的用户名，不让后台等太久。
const BIND_FETCH_SECS: u64 = 5;

/// 管理员手动给指定 Key 绑定或换绑 Discord 账号。
///
/// 只需要 Discord ID：配了机器人就用它读这个账号的公开资料（和后台补全资料同一条 `GET /users/{id}`），
/// 用户名 / 显示名（global_name，没有就用用户名）/ 头像都以 Discord 为准，Key 名改成「显示名 (@用户名)」。
/// 读不到（没配机器人、网络错误、超时、查无此人）就退回请求里填的 `username` / `display_name`，Key 名不动。
async fn bind_discord(
    State(state): State<AppState>,
    _s: AdminSession,
    AdminPath(key_id): AdminPath<i64>,
    req: Request,
) -> R {
    let (_, body) = json_of(req).await?;
    let discord_id = pyc::strip(&pstr(&body, "discord_id")).to_string();
    if !gate_core::pyext::is_decimal(&discord_id) {
        return Err(http(422, "Discord ID 必须是纯数字"));
    }
    let username = pyc::take_chars(pyc::strip(&pstr(&body, "username")), 80).to_string();
    let display_name = pyc::take_chars(&pstr_or(&body, "display_name", &username), 80).to_string();
    let st = state.clone();
    let Some(row) = load_key(&st, key_id).await? else {
        return Err(http(404, NO_KEY));
    };
    // 没有用户名的响应（假服务 / 异常返回）不算读到
    let fetched = match st.registration.as_ref() {
        Some(reg) => reg
            .fetch_user(&discord_id, BIND_FETCH_SECS)
            .await
            .filter(|u| !u.username.is_empty()),
        None => None,
    };
    let profile = match &fetched {
        Some(u) => {
            let user = DiscordUser {
                id: discord_id.clone(),
                ..u.clone()
            };
            let username = clip(&user.username).unwrap_or_default();
            BindProfile {
                display_name: clip(&user.global_name).unwrap_or_else(|| username.clone()),
                username,
                avatar: Some(clip(&user.avatar).unwrap_or_default()),
                key_name: Some(member_label(&user)),
            }
        }
        None => BindProfile {
            username,
            display_name,
            avatar: None,
            key_name: None,
        },
    };
    let reg = match st
        .db
        .bind_discord(key_id, &discord_id, &profile, st.now())
        .await?
    {
        BindOutcome::Taken(other) => {
            return Err(http(400, format!("该 Discord 账号已绑定到 Key #{other}")));
        }
        BindOutcome::Bound(reg) => reg,
    };
    let old_name = pyval::py_str(&jget(&row, "name"));
    // 读不到资料时日志和以前一样；读到了再注明来源和改名
    let note = match &profile.key_name {
        Some(n) if *n != old_name => format!("；已从 Discord 读取资料，Key 名改为「{n}」"),
        Some(_) => "；已从 Discord 读取资料".to_string(),
        None => String::new(),
    };
    log_action(
        &st.db,
        "站长",
        "手动绑定 Discord",
        &format!("Key #{key_id} {old_name}"),
        &format!("@{} ({discord_id}){note}", profile.username),
        true,
        st.now(),
    )
    .await;
    let name = profile.key_name.clone().unwrap_or(old_name);
    let resp = ok_json(&json!({
        "ok": true,
        "discord_id": discord_id,
        "discord": reg.as_ref().map(discord_profile),
        "name": name,
        "from_discord": fetched.is_some(),
    }));
    Ok(mark_logged(resp))
}

// ----------------------------------------------------------------------------- 券

/// 给成员发一张券（现在只有重置券）：`{"kind": "reset", "days": 7, "note": "唱得真好听"}`。成员在首页自己使用。
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
    let note = pyc::take_chars(&pstr(&body, "note"), 200).to_string();
    let by = pyc::take_chars(&pstr_or(&body, "by", "站长"), 40).to_string();
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
    let discord_id = pyc::strip(&pstr(&body, "discord_id")).to_string();
    if !gate_core::pyext::is_decimal(&discord_id) {
        return Err(http(422, "Discord ID 必须为纯数字"));
    }
    let reason =
        pyc::take_chars(pyc::strip(&pstr_or(&body, "reason", "站长手动封禁")), 100).to_string();
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
    let discord_id = pyc::strip(&discord_id).to_string();
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
        .split(pyc::is_space)
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let tag = pyc::take_chars(&joined, TAG_MAX_LEN).to_string();
    if tag.is_empty() {
        return Err(http(422, "标签不能为空"));
    }
    let note = pyc::take_chars(pyc::strip(&pstr(&body, "note")), TAG_NOTE_MAX).to_string();
    let by = {
        let b = pyc::take_chars(pyc::strip(&pstr_or(&body, "by", "站长")), 20).to_string();
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

/// Python `a + b`：整数相加仍是整数，有浮点则是浮点。
fn num_add(a: &Value, b: &Value) -> Value {
    match (
        a.as_i64().filter(|_| !a.is_f64()),
        b.as_i64().filter(|_| !b.is_f64()),
    ) {
        (Some(x), Some(y)) => json!(x + y),
        _ => pyval::float(
            pyval::number_like(a).unwrap_or(0.0) + pyval::number_like(b).unwrap_or(0.0),
        ),
    }
}

/// `x or 0`
fn or0(v: &Value) -> Value {
    if pyval::truthy(v) {
        v.clone()
    } else {
        json!(0)
    }
}

fn fnum(v: &Value) -> f64 {
    pyval::number_like(v).unwrap_or(0.0)
}

/// 每位成员（Key）的今日 / 近 7 天 / 累计用量，以及来源。
async fn members(State(state): State<AppState>, _s: AdminSession) -> R {
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
    // 旧版 `paused if paused > time.time() else 0`：过期的是整数 0，没过期的是浮点
    let pause_map: HashMap<i64, Value> = st
        .db
        .pause_rows()
        .await?
        .into_iter()
        .map(|(k, paused)| {
            let v = if paused > now {
                pyval::float(paused)
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
    // 起点是**当前服务日**的 7 点。旧版用日历日期算起点，凌晨 0–7 点起点落在未来，这段时间一直显示「没重置过」。
    let day_start = st.clock.service_day_start(&today).unwrap_or(0.0);
    let mut reset_map: HashMap<i64, (i64, f64)> = HashMap::new();
    for (a_ts, target) in st.db.quota_reset_marks(day_start).await? {
        let Some(rest) = target.strip_prefix("Key #") else {
            continue;
        };
        let digits: String = rest.chars().take_while(|c| pyext::is_nd(*c)).collect();
        let Some(id) = pyc::parse_int(&digits) else {
            continue;
        };
        let e = reset_map.entry(id).or_insert((0, 0.0));
        e.0 += 1;
        if a_ts > e.1 {
            e.1 = a_ts;
        }
    }
    let today_counters: HashMap<i64, (i64, i64, f64, i64)> = st
        .db
        .counter_rows(&today)
        .await?
        .into_iter()
        .map(|(k, images, v5, anlas, _tokens, requests)| (k, (images, v5, anlas, requests)))
        .collect();

    // 生图速率：每人最近 24 小时逐小时成功生图数（前端画 24 根小时条；跨天的开头几格是昨天同时段）
    let hour0 = (now / 3600.0).floor() as i64 * 3600 - 23 * 3600;
    let mut rate_map: HashMap<i64, Vec<i64>> = HashMap::new();
    for (key_id, bucket, n) in st.db.hourly_image_counts(hour0).await? {
        let bars = rate_map.entry(key_id).or_insert_with(|| vec![0; 24]);
        if (0..24).contains(&bucket) {
            bars[bucket as usize] = n;
        }
    }

    let mut out: Vec<Obj> = Vec::new();
    for row in st.db.list_keys().await? {
        let id = row.get("id").and_then(Value::as_i64).unwrap_or(0);
        let (images, v5, anlas, requests) =
            today_counters.get(&id).copied().unwrap_or((0, 0, 0.0, 0));
        let w = week.get(&id).cloned().unwrap_or(Value::Null);
        let wi = |k: &str| pyval::number_like(&jget(&w, k)).map_or(0, |x| x as i64);
        let wf = |k: &str| pyval::number_like(&jget(&w, k)).unwrap_or(0.0);
        let my_tags = manual_tags.get(&id);
        let role = role_of(my_tags.is_some_and(|t| t.iter().any(|x| x.tag == INVITE_TAG)));
        let reg_row = reg.get(&id);
        let ms = milestones.get(&id);
        let mut m = Obj::new();
        let mut put = |k: &str, v: Value| {
            m.insert(k.to_string(), v);
        };
        put("id", jget(&row, "id"));
        put("name", jget(&row, "name"));
        put("enabled", json!(jflag(&row, "enabled")));
        put("role", json!(role));
        put("role_label", json!(role_label(role)));
        put("discord_id", json!(reg_row.map(|r| r.discord_id.clone())));
        put("discord", json!(reg_row.map(discord_profile)));
        put("created_at", jget(&row, "created_at"));
        put("last_used_at", jget(&row, "last_used_at"));
        put("expires_at", jget(&row, "expires_at"));
        put("daily_images", jget(&row, "daily_images"));
        put("daily_v5", jget(&row, "daily_v5"));
        put("allow_anlas", json!(jflag(&row, "allow_anlas")));
        put("daily_anlas", jget(&row, "daily_anlas"));
        put("image_model_scope", jget(&row, "image_model_scope"));
        put(
            "quota_auto",
            json!(pyext::num_eq_int(row.get("quota_auto"), 1)),
        );
        put(
            "today",
            json!({"images": images, "v5": v5, "anlas": pyval::float(pyc::round(anlas, 2)),
                   "requests": requests}),
        );
        put(
            "week",
            json!({"images": wi("images"), "v5": wi("v5"),
                   "anlas": pyval::float(pyc::round(wf("anlas"), 2)), "requests": wi("requests")}),
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
                |(c, t)| json!({"count": c, "last_ts": pyval::float(*t)}),
            ),
        );
        put(
            "tags",
            Value::Array(
                my_tags
                    .into_iter()
                    .flatten()
                    .map(|t| json!({"tag": t.tag, "note": t.note, "by": t.by, "ts": pyval::float(t.ts)}))
                    .collect(),
            ),
        );
        put(
            "auto_tags",
            Value::Array(auto_tags.get(&id).cloned().unwrap_or_default()),
        );
        put(
            "hours24",
            json!(rate_map.get(&id).cloned().unwrap_or_default()),
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
            if pyval::truthy(&other["last_image_at"]) {
                let a = or0(&head["last_image_at"]);
                let b = other["last_image_at"].clone();
                head.insert(
                    "last_image_at".into(),
                    if fnum(&b) > fnum(&a) { b } else { a },
                );
            }
            if pyval::truthy(&other["first_image_at"]) {
                let b = other["first_image_at"].clone();
                let a = if pyval::truthy(&head["first_image_at"]) {
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
            if other["hours24"].as_array().is_some_and(|a| !a.is_empty()) {
                let zeros: Vec<Value> = vec![json!(0); 24];
                let hv = head["hours24"]
                    .as_array()
                    .filter(|a| !a.is_empty())
                    .cloned()
                    .unwrap_or(zeros);
                let merged_bars: Vec<Value> = hv
                    .iter()
                    .zip(other["hours24"].as_array().into_iter().flatten())
                    .map(|(a, b)| num_add(&or0(a), &or0(b)))
                    .collect();
                head.insert("hours24".into(), Value::Array(merged_bars));
            }
        }
        merged.push(head);
    }
    singles.extend(merged);
    Ok(ok_json(&json!({
        "members": singles,
        "inactivity_days": st.env.key_inactivity_delete_days,
    })))
}
