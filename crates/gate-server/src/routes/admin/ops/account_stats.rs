//! 后台「上游」页的每账号统计：最近 1 小时 / 24 小时的请求成败、上游错误码、今日张数、
//! 最近 1 小时 / 24 小时请求窗口、今日张数、负载与余额占比，以及由对账快照推算的 Anlas 余额与续费倒计时。
//! 只读，不向上游发任何请求；成员接口看不到这里的任何数据。

use super::upstream::pool_status;
use crate::body::read_json_body;
use crate::error::{ApiError, ApiResult, HttpError, ok_json};
use crate::session::AdminSession;
use crate::state::AppState;
use axum::Router;
use axum::extract::{Request, State};
use axum::routing::{get, post};
use gate_core::anlas_estimate::{self, Projection, SnapshotAccount, Usage};
use gate_core::values;
use gate_db::repo::account_stats::AccountLogRow;
use serde_json::{Value, json};

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/admin/api/sitepaid-hints", get(sitepaid_hints))
        .route("/admin/api/sitepaid-hints/dismiss", post(sitepaid_dismiss))
}

/// 每个上游账号一行的本站观测：1 小时 / 24 小时请求窗口、负载与余额占比、
/// 由对账快照推算的 Anlas 余额（`balance`）。顺序与传入的 `pool` 一致；
/// 「上游」页的聚合接口把它和令牌池状态、官方读数按行合并。
pub(super) async fn account_rows(
    state: &AppState,
    pool: &[Value],
    now: f64,
) -> Result<Vec<Value>, ApiError> {
    let ids: Vec<String> = pool
        .iter()
        .map(|p| p["token_id"].as_str().unwrap_or_default().to_string())
        .collect();
    let single = ids.len() == 1;

    let rows_24h = state.db.account_log_rows(now - DAY).await?;
    let ok_24h_total = rows_24h
        .iter()
        .filter(|r| r.token_id.is_some() && r.status == "ok")
        .count() as f64;

    // 最近一次真实核对：每个账号取最新一份含它的快照
    let history = state.db.reconciliation_history(20).await?;
    let used_3d = state.db.anlas_by_token(0, now - 3.0 * DAY).await?;

    let mut accounts: Vec<Value> = Vec::new();
    let mut balances: Vec<Option<f64>> = Vec::new();
    for (i, _p) in pool.iter().enumerate() {
        let id = ids[i].as_str();
        let all_24h: Vec<&AccountLogRow> = rows_24h
            .iter()
            .filter(|r| r.token_id.as_deref() == Some(id))
            .collect();
        let last_1h: Vec<&AccountLogRow> = all_24h
            .iter()
            .copied()
            .filter(|r| r.ts > now - HOUR)
            .collect();
        let ok_24h = all_24h.iter().filter(|r| r.status == "ok").count() as f64;

        let (balance, est) =
            match account_estimate(state, &history, &used_3d, id, single, now).await? {
                None => (Value::Null, None),
                Some((e, un)) => (estimate_json(&e, un), Some(e)),
            };
        balances.push(est.as_ref().and_then(|e| e.balance));
        accounts.push(json!({
            "token_id": id,
            "w1h": window_stats(&last_1h),
            "w24h": window_stats(&all_24h),
            "load_share_24h": pct(ok_24h, ok_24h_total),
            "balance": balance,
        }));
    }
    // 余额占比：只在每个账号都有可用推算时才给
    let total_balance: f64 = balances.iter().flatten().sum();
    let all_known = !balances.is_empty() && balances.iter().all(Option::is_some);
    for (a, b) in accounts.iter_mut().zip(&balances) {
        a["balance_share"] = match (all_known, b) {
            (true, Some(b)) => pct(*b, total_balance).map_or(Value::Null, |v| json!(v)),
            _ => Value::Null,
        };
    }
    Ok(accounts)
}

const HOUR: f64 = 3600.0;
const DAY: f64 = 86400.0;

/// 一个时间窗内某个账号的请求统计。
fn window_stats(rows: &[&AccountLogRow]) -> Value {
    let count = |s: &str| rows.iter().filter(|r| r.status == s).count() as i64;
    let up = |code: i64| rows.iter().filter(|r| r.up_status == code).count() as i64;
    json!({
        "ok": count("ok"),
        "error": count("error"),
        "rejected": count("rejected"),
        "up_429": up(429),
        "up_403": up(403),
        "up_401": up(401),
    })
}

fn pct(part: f64, total: f64) -> Option<i64> {
    (total > 0.0).then(|| (part / total * 100.0).round() as i64)
}

fn estimate_json(e: &anlas_estimate::Estimate, unattributed: f64) -> Value {
    let (kind, value) = match e.projection {
        Projection::Unknown => ("unknown", Value::Null),
        Projection::RunsOut { days } => ("runs_out", json!(days)),
        Projection::Waste { anlas } => ("waste", json!(anlas)),
    };
    json!({
        "cycle_rolled": e.cycle_rolled,
        "balance": e.balance,
        "fixed": e.fixed,
        "purchased": e.purchased,
        "expires_at": e.expires_at,
        "days_left": e.days_left,
        "avg_daily": e.avg_daily,
        "projection": kind,
        "projection_value": value,
        "snapshot_age_days": e.snapshot_age_days,
        "stale": e.stale,
        "approximate": e.approximate,
        "unattributed_anlas": unattributed,
    })
}

/// 一个账号的 Anlas 推算：最近一份含它的对账快照 + 快照之后的账本用量。没有快照返回 None。
/// 返回值里的 f64 是没法确定属于这个账号的用量（多账号且日志没记账号时）。
async fn account_estimate(
    state: &AppState,
    history: &[Value],
    used_3d: &[(Option<String>, f64)],
    id: &str,
    single: bool,
    now: f64,
) -> Result<Option<(anlas_estimate::Estimate, f64)>, ApiError> {
    let sum_of = |rows: &[(Option<String>, f64)], id: Option<&str>| -> f64 {
        rows.iter()
            .filter(|(t, _)| t.as_deref() == id)
            .map(|(_, a)| *a)
            .sum()
    };
    let snapshot = history.iter().find_map(|h| {
        let a = h.get("accounts")?.get(id)?;
        let acct = SnapshotAccount {
            checked_at: h.get("checked_at").and_then(Value::as_f64)?,
            fixed: a.get("fixed").and_then(Value::as_f64)?,
            purchased: a.get("purchased").and_then(Value::as_f64)?,
            expires_at: a.get("expires_at").and_then(Value::as_f64),
        };
        let last_id = h
            .get("ledger")
            .and_then(|l| l.get("last_log_id"))
            .and_then(Value::as_i64)
            .unwrap_or(0);
        Some((acct, last_id))
    });
    let Some((acct, last_id)) = snapshot else {
        return Ok(None);
    };
    let untagged_3d = sum_of(used_3d, None);
    let after = state.db.anlas_by_token(last_id, 0.0).await?;
    let untagged_after = sum_of(&after, None);
    let tagged_after = sum_of(&after, Some(id));
    let tagged_3d = sum_of(used_3d, Some(id));
    let usage = Usage {
        since_snapshot: tagged_after + if single { untagged_after } else { 0.0 },
        last_3d: tagged_3d + if single { untagged_3d } else { 0.0 },
        approximate: !single && (untagged_after > 0.0 || untagged_3d > 0.0),
    };
    let e = anlas_estimate::estimate(now, acct, usage);
    let un = if single { 0.0 } else { untagged_after };
    Ok(Some((e, un)))
}

// ------------------------------------------------------------------ 站付 V5 到期建议

/// 离订阅到期不超过这么多天才提醒。
const HINT_DAYS: f64 = 3.0;
/// 官方读到的固定 Anlas 剩余超过这个数才值得提醒。
const HINT_MIN_ANLAS: f64 = 300.0;
/// 已关闭的提醒：`{token_id: 当时的到期时间}`，同一个号同一个到期日只提醒一次。
const DISMISSED_KEY: &str = "sitepaid_hint_dismissed";

async fn dismissed_map(state: &AppState) -> Result<serde_json::Map<String, Value>, ApiError> {
    let raw = state.db.get_setting(DISMISSED_KEY).await?;
    Ok(raw
        .and_then(|r| serde_json::from_str::<Value>(&r).ok())
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default())
}

/// `GET /admin/api/sitepaid-hints`：站付 V5 的「建议」提醒（手动：只提醒，不会自动开）。
///
/// 条件：某个上游号离订阅到期 ≤ 3 天（还没到期）、按对账快照推算的固定 Anlas 剩余 > 300、
/// 站付 V5 现在没有对它生效、且这个到期日的提醒没被关闭过。到期日和余额来自 Anlas 对账快照（只读，不查上游）。
/// 每条提示：`token_id`、`expires_at`、`days_left`、`fixed`（剩余 Anlas）、`stale`（快照偏旧）、
/// `message`（写好的提示语）、`manual: true`、`setting_key`（要设置的站付 V5 项，用 `POST /admin/api/ops/manual` 改）。
async fn sitepaid_hints(State(state): State<AppState>, _s: AdminSession) -> ApiResult {
    let now = state.now();
    let pool = pool_status(&state).await?;
    let single = pool.len() == 1;
    let history = state.db.reconciliation_history(20).await?;
    let used_3d = state.db.anlas_by_token(0, now - 3.0 * DAY).await?;
    let dismissed = dismissed_map(&state).await?;
    let raw_paid = state.db.get_setting(gate_core::paid_v5::KEY).await?;
    let mut hints: Vec<Value> = Vec::new();
    for p in &pool {
        let id = p["token_id"].as_str().unwrap_or_default();
        if gate_core::paid_v5::grant(raw_paid.as_deref(), id, now).is_some() {
            continue;
        }
        let Some((e, _)) = account_estimate(&state, &history, &used_3d, id, single, now).await?
        else {
            continue;
        };
        let (Some(expires_at), Some(days_left), Some(fixed)) = (e.expires_at, e.days_left, e.fixed)
        else {
            continue;
        };
        if e.cycle_rolled || !(0.0..=HINT_DAYS).contains(&days_left) || fixed <= HINT_MIN_ANLAS {
            continue;
        }
        if dismissed.get(id).and_then(Value::as_f64) == Some(expires_at) {
            continue;
        }
        let when = state.clock.format(expires_at, "%m-%d %H:%M");
        hints.push(json!({
            "token_id": id,
            "expires_at": expires_at,
            "days_left": days_left,
            "fixed": fixed,
            "stale": e.stale,
            "manual": true,
            "setting_key": gate_core::paid_v5::KEY,
            "message": format!(
                "上游账号 {id} 将在 {when} 到期，按对账推算还剩约 {} Anlas，可以考虑手动开站付 V5 把它用掉（手动）",
                fixed.round() as i64
            ),
        }));
    }
    Ok(ok_json(&json!({
        "generated_at": values::float(now),
        "hints": hints,
        "min_anlas": HINT_MIN_ANLAS,
        "within_days": HINT_DAYS,
    })))
}

/// `POST /admin/api/sitepaid-hints/dismiss`：关闭某个号这个到期日的提醒（到期日变了会再提醒）。
/// 请求体 `{"token_id": "...", "expires_at": 到期 Unix 秒}`。
async fn sitepaid_dismiss(
    State(state): State<AppState>,
    _s: AdminSession,
    req: Request,
) -> ApiResult {
    let (parts, body) = req.into_parts();
    let body = read_json_body(&parts.headers, body, 4096).await?;
    let token_id = body.get("token_id").and_then(Value::as_str).unwrap_or("");
    let expires_at = body.get("expires_at").and_then(Value::as_f64);
    let (false, Some(expires_at)) = (token_id.is_empty(), expires_at) else {
        return Err(HttpError::new(422, "需要 token_id 和 expires_at").into());
    };
    let mut map = dismissed_map(&state).await?;
    map.insert(token_id.to_string(), json!(expires_at));
    // 只留 50 个号，防止无限增长
    while map.len() > 50 {
        let Some(k) = map.keys().next().cloned() else {
            break;
        };
        map.remove(&k);
    }
    state
        .db
        .set_setting(DISMISSED_KEY, &Value::Object(map).to_string())
        .await?;
    Ok(ok_json(&json!({"ok": true})))
}
