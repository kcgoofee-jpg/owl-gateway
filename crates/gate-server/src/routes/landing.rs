//! 首页首屏与成员个人状态。
use crate::error::InternalError;
use crate::{key::Key, live, session, state::AppState};
use axum::http::HeaderMap;
use serde_json::{Map, Value, json};

fn quota_rows_json(rows: &[gate_core::landing_quota::Row]) -> Value {
    Value::Array(
        rows.iter()
            .map(|row| {
                let mut value = json!({
                    "id": row.id,
                    "used": row.used,
                    "cap": row.cap,
                    "left": row.left,
                    "paused": row.paused,
                });
                if !row.reason.is_empty() {
                    value["reason"] = json!(row.reason);
                }
                if row.reason == "site_paused" {
                    value["hint"] = json!(gate_core::quota::V5_PAUSED_HINT);
                }
                value
            })
            .collect(),
    )
}

/// 登录成员的个人状态（`/public/me` 的内容）。
pub(super) async fn me_payload(
    state: &AppState,
    discord_id: &str,
    key: Option<&Key>,
) -> Result<Value, InternalError> {
    let discord = if state.env.registration.is_some() {
        match state.db.discord_profile_row(discord_id).await? {
            None => json!({"id": discord_id}),
            Some((username, display_name, avatar)) => {
                json!({"id": discord_id, "username": username, "display_name": display_name, "avatar": avatar})
            }
        }
    } else {
        Value::Null
    };
    let mut out = Map::new();
    out.insert("logged_in".into(), json!(true));
    out.insert("discord".into(), discord);
    // 站长 10-24：「网页登录一次」闸门生效时，首页要能提示受影响成员，而不是等出图 403。
    if crate::auth::requires_web_login(state, discord_id).await {
        out.insert("requires_web_login".into(), json!(true));
    }
    let Some(key) = key.filter(|k| k.enabled()) else {
        out.insert("has_key".into(), json!(false));
        return Ok(Value::Object(out));
    };
    let day = state.day();
    let c = state.db.get_counter(key.id(), &day).await?;
    let mut queue = state
        .guard
        .with(|g| g.queue_view(Some(key.id())))
        .get("mine")
        .cloned()
        .unwrap_or(json!([]));
    // 预计开始时刻只给实时看板（`live::mine` 换算成 eta），这里保持原来的字段
    if let Value::Array(items) = &mut queue {
        for item in items.iter_mut().filter_map(Value::as_object_mut) {
            item.remove("start_at");
        }
    }
    // 全站今天还剩多少张 V5。圆环用 `rows`，不再拿这个数和个人剩余自己取较小值。
    // 10-27 起全站池 0 = 停用（0 剩余），和「今天用完」不是同一件事。
    let budgets = state.db.shared_budgets().await?;
    let g = i64::from(budgets.daily_v5);
    let paused = crate::ops::v5_paused(&state.db).await?;
    let allowed = key.policy.allows("v5");
    let used = state.db.day_v5_total(&day).await?;
    let site_v5_left = if paused || !allowed {
        0
    } else {
        (g - used).max(0)
    };
    // 全站今天的 Anlas 池还剩多少：和「全站今日 Anlas 额度已用完」的拒绝口径一致。
    let anlas_budget = f64::from(budgets.daily_anlas);
    let site_anlas_left = (anlas_budget - state.db.day_anlas_total(&day).await?).max(0.0);
    let daily_anlas = live::member_anlas_cap(state, key).await?;
    let upstream_free = live::upstream_v5_free(state).await;
    let quota_rows = quota_rows_json(&gate_core::landing_quota::rows(
        &gate_core::landing_quota::Input {
            legacy: Some(c.legacy_free_images),
            images: c.images,
            v5_used: c.v5,
            daily_images: key.daily_images(),
            image_on: key.policy.image,
            daily_v5: key.daily_v5(),
            v5_on: allowed,
            v5_paused: paused,
            upstream_v5_free: upstream_free,
            site_v5_cap: g,
            site_v5_used: used,
            anlas_used: gate_core::primitives::round0(c.anlas),
            daily_anlas,
            site_anlas_cap: anlas_budget.trunc() as i64,
            site_anlas_left: site_anlas_left.trunc() as i64,
        },
    ));
    let now = state.now();
    out.insert("has_key".into(), json!(true));
    out.insert("key".into(), json!(key.token()));
    out.insert("name".into(), json!(key.name()));
    out.insert("expires_at".into(), json!(key.expires_at()));
    out.insert("policy".into(), json!(&key.policy));
    if key.owner_image_full {
        out.insert("owner_image_full".into(), json!(true));
    }
    if crate::share_lock::is_locked(state, key).await {
        out.insert(
            "share_lock".into(),
            json!({
                "message": crate::share_lock::MESSAGE,
                "channel_url": crate::share_lock::channel_url(),
            }),
        );
    }
    out.insert(
        "today".into(),
        json!({
            "images": c.images, "v5": c.v5, "legacy": c.legacy_free_images,
            "daily_images": key.daily_images(), "daily_v5": key.daily_v5(),
            "site_v5_left": site_v5_left, "site_v5_cap": g,
            "upstream_v5_free": upstream_free && !paused,
            "v5_paused": paused,
            "v5_access_allowed": allowed,
            "v5_access_reason": if allowed { "" } else { "此成员未开放 V5" },
            "anlas": gate_core::primitives::round0(c.anlas),
            "daily_anlas": daily_anlas,
            "site_anlas_left": site_anlas_left.trunc() as i64,
            "site_anlas_cap": anlas_budget.trunc() as i64,
            "reset_hour": state.guard.with(|g| g.reset_hour),
            "rows": quota_rows,
        }),
    );
    out.insert("queue".into(), queue);
    out.insert(
        "coupons".into(),
        json!(state.db.list_coupons(key.id(), true, now).await?),
    );
    Ok(Value::Object(out))
}

/// 首页 HTML 里的 `<script type="application/json" id="owl-boot">`：`{me, live, status}`，
/// 形状与 `/public/me`、`/public/live`、`/public/status` 的响应一样（web/src/pages/landing/store.ts 的 `Boot`）。
/// 页面拿到就不用再等三个请求；任何一块取不到就缺省，页面自己走接口。
pub async fn landing_boot(state: &AppState, headers: &HeaderMap) -> Option<Value> {
    let mut boot = Map::new();
    let session = session::member_state(state, headers).await;
    let mut key_id = None;
    match &session {
        Err(e) => tracing::warn!(target: "boot", "首屏会话读取失败：{e}"),
        Ok(None) => {
            boot.insert("me".into(), json!({"logged_in": false}));
        }
        Ok(Some((discord_id, key))) => {
            key_id = key.as_ref().filter(|k| k.enabled()).map(Key::id);
            match me_payload(state, discord_id, key.as_ref()).await {
                Ok(me) => {
                    boot.insert("me".into(), me);
                }
                Err(e) => tracing::warn!(target: "boot", "首屏 me 取数失败：{}", e.0),
            }
        }
    }
    match state.live.current(state).await {
        Ok(snap) => {
            boot.insert(
                "live".into(),
                snap.value(key_id.map(|id| live::mine(state, id))),
            );
        }
        Err(e) => tracing::warn!(target: "boot", "首屏 live 取数失败：{}", e.0),
    }
    match state.live.status(state).await {
        Ok(status) => {
            boot.insert("status".into(), (*status).clone());
        }
        Err(e) => tracing::warn!(target: "boot", "首屏 status 取数失败：{}", e.0),
    }
    (!boot.is_empty()).then_some(Value::Object(boot))
}
