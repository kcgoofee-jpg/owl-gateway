//! 后台里属于定时任务的接口：额度记录（只显示）。
//!
//! | 接口 | 说明 |
//! |---|---|
//! | `GET /admin/api/quota-records` | 记录：各上游账号的 V5 电量 / 恢复速度 / Anlas 余额、昨天和今天每个账号出了多少、还在生效的手动临时调整 |
//!
//! 站长 10-23 删掉了全部自动额度算法：`/admin/api/quota-algo`（结果、参数、每日复盘）随之删除；额度的手动值在
//! `/admin/api/settings`。
//!
//! 「系统」页的定时任务卡片 10-23 删除（Claude 从日志 / 库里的 `job_last` 看运行记录），`GET /admin/api/jobs` 随之删除。

use crate::error::{ApiError, ok_json};
use crate::session::AdminSession;
use crate::state::AppState;
use axum::Router;
use axum::extract::State;
use axum::response::Response;
use axum::routing::get;
use gate_core::{pyval, quota};
use serde_json::{Value, json};

pub fn router() -> Router<AppState> {
    Router::new().route("/admin/api/quota-records", get(quota_records))
}

/// 记录（只显示，不参与任何限额）：每个上游账号的 V5 电量读数、恢复速度（官方读数 + 安静时段实测）、
/// Anlas 余额，昨天 / 今天的出图张数；账号只给编号，不给 Token。
async fn quota_records(
    State(state): State<AppState>,
    _s: AdminSession,
) -> Result<Response, ApiError> {
    let now = state.now();
    let rows = state.nai.status_rows().await;
    let ids: Vec<String> = rows.iter().map(|(t, _)| t.token_id.clone()).collect();
    let snap = state.allowance.snapshot(&ids).await?;
    let readings = snap
        .get("accounts")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let today = state.day();
    let mut accounts = Vec::new();
    for (i, (t, f)) in rows.iter().enumerate() {
        let r = readings.get(i).cloned().unwrap_or(Value::Null);
        let pick = |k: &str| r.get(k).cloned().unwrap_or(Value::Null);
        let c_today = state.db.get_upstream_counter(&t.token_id, &today).await?;
        let c_yday = state
            .db
            .get_upstream_counter(&t.token_id, &state.clock.day(now - 86400.0))
            .await?;
        // 实测恢复速度按账号估（该账号自己的快照序列 + 自己的 V5 出图条数）
        let quiet =
            quota::quiet_recovery(&state.db.quota_quiet_inputs_for(&t.token_id, now).await?)
                .map_or(Value::Null, |q| q.to_value());
        accounts.push(json!({
            "position": f.position,
            "v5_percent": pick("percent"),
            "v5_recharge_per_day": pick("recharge_per_day"),
            "anlas": state.allowance.anlas_of(&t.token_id),
            "quiet_rate": quiet,
            "today": {"images": c_today.images, "v5": c_today.v5},
            "yesterday": {"images": c_yday.images, "v5": c_yday.v5},
        }));
    }
    Ok(ok_json(&json!({
        "accounts": accounts,
        "manual": manual_raises(&state).await?,
    })))
}

/// 此刻还在生效的手动临时调整（站长 / Claude 经 `nai-ops manual` 定的）。
/// 账号只给编号（和「上游」页一致），不给 Token。
async fn manual_raises(state: &AppState) -> Result<Value, ApiError> {
    let now = state.now();
    let rows = state.nai.status_rows().await;
    let position = |id: &str| {
        rows.iter()
            .find(|(t, _)| t.token_id == id)
            .map_or(Value::Null, |(_, f)| json!(f.position))
    };
    let (base, caps) = state
        .guard
        .with(|g| (g.values["account_daily_cap"], g.token_daily_caps.clone()));
    let caps: Vec<Value> = caps
        .iter()
        .filter(|(_, (_, until))| now < *until)
        .map(|(id, (cap, until))| {
            json!({"position": position(id), "cap": cap, "base": base, "until": pyval::float(*until)})
        })
        .collect();
    let raw_paid = state.db.get_setting(gate_core::paid_v5::KEY).await?;
    let mut paid = Vec::new();
    for (t, f) in &rows {
        if let Some(g) = gate_core::paid_v5::grant(raw_paid.as_deref(), &t.token_id, now) {
            let spent = state
                .db
                .site_paid_spent(&t.token_id, gate_core::paid_v5::MARKER)
                .await?;
            paid.push(json!({
                "position": f.position,
                "max_anlas": pyval::float(g.max_anlas),
                "spent": pyval::float(spent),
                "until": pyval::float(g.until),
            }));
        }
    }
    Ok(json!({"account_caps": caps, "paid_v5": paid}))
}
