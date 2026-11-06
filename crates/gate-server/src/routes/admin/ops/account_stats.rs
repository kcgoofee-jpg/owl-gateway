//! 后台「上游」页的每账号统计：最近 1 小时 / 24 小时的请求成败、上游错误码、负载与余额占比，
//! 以及最近一条上游快照里的真实余额读数。只读，不向上游发任何请求。
//!
//! 原来的「按对账快照推算 Anlas 余额 / 续费倒计时 / 站付 V5 到期建议」已删除：
//! 那是推断，余额直接看 `upstream_snapshots` 的实测读数就够了。

use crate::error::ApiError;
use crate::state::AppState;
use gate_core::values;
use gate_db::repo::account_stats::AccountLogRow;
use serde_json::{Value, json};
use std::collections::HashMap;

/// 每个上游账号一行的本站观测：1 小时 / 24 小时请求窗口、负载占比、以及最近一条快照的余额。
/// 顺序与传入的 `pool` 一致；「上游」页的聚合接口把它和令牌池状态、官方读数按行合并。
pub(super) async fn account_rows(
    state: &AppState,
    pool: &[Value],
    now: f64,
) -> Result<Vec<Value>, ApiError> {
    let ids: Vec<String> = pool
        .iter()
        .map(|p| p["token_id"].as_str().unwrap_or_default().to_string())
        .collect();

    let rows_24h = state.db.account_log_rows(now - DAY).await?;
    let ok_24h_total = rows_24h
        .iter()
        .filter(|r| r.token_id.is_some() && r.status == "ok")
        .count() as f64;

    // 每个账号最近一条快照的实测余额（不推算、不查上游）
    let snaps: HashMap<String, Value> = state
        .db
        .latest_snapshots()
        .await?
        .into_iter()
        .map(|(id, ts, anlas, v5)| {
            let age_days = (now - ts) / DAY;
            (
                id,
                json!({
                    "balance": anlas,
                    "fixed": anlas,
                    "v5_percent": v5,
                    "at": values::float(ts),
                    "snapshot_age_days": values::float(age_days),
                    "stale": age_days > 1.0,
                }),
            )
        })
        .collect();

    let mut accounts: Vec<Value> = Vec::new();
    let mut balances: Vec<Option<f64>> = Vec::new();
    for id in &ids {
        let all_24h: Vec<&AccountLogRow> = rows_24h
            .iter()
            .filter(|r| r.token_id.as_deref() == Some(id.as_str()))
            .collect();
        let last_1h: Vec<&AccountLogRow> = all_24h
            .iter()
            .copied()
            .filter(|r| r.ts > now - HOUR)
            .collect();
        let ok_24h = all_24h.iter().filter(|r| r.status == "ok").count() as f64;

        balances.push(
            snaps
                .get(id)
                .and_then(|s| s.get("balance"))
                .and_then(Value::as_f64),
        );
        accounts.push(json!({
            "token_id": id,
            "w1h": window_stats(&last_1h),
            "w24h": window_stats(&all_24h),
            "load_share_24h": pct(ok_24h, ok_24h_total),
            "balance": snaps.get(id).cloned().unwrap_or(Value::Null),
        }));
    }
    // 余额占比：只在每个账号都有读数时才给
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

fn window_stats(rows: &[&AccountLogRow]) -> Value {
    let count = |s: &str| rows.iter().filter(|r| r.status == s).count() as i64;
    let up = |code: i64| rows.iter().filter(|r| r.up_status == code).count() as i64;
    let samples: Vec<_> = rows
        .iter()
        .filter(|r| {
            r.status == "ok" && matches!(r.kind.as_str(), "image" | "image_stream") && r.dur_ms > 0
        })
        .collect();
    let mut waits: Vec<_> = samples.iter().map(|r| r.wait_ms).collect();
    waits.sort_unstable();
    let average = |total: f64| (!samples.is_empty()).then(|| total / samples.len() as f64);
    let avg_wait = average(samples.iter().map(|r| r.wait_ms as f64).sum());
    let avg_dur = average(samples.iter().map(|r| r.dur_ms as f64).sum());
    let p90_wait = (!waits.is_empty()).then(|| waits[(waits.len() * 9).div_ceil(10) - 1]);
    json!({
        "ok": count("ok"),
        "error": count("error"),
        "rejected": count("rejected"),
        "up_429": up(429),
        "up_403": up(403),
        "up_401": up(401),
        "timing_samples": samples.len(),
        "avg_wait_ms": avg_wait,
        "p90_wait_ms": p90_wait,
        "avg_dur_ms": avg_dur,
    })
}

fn pct(part: f64, total: f64) -> Option<i64> {
    (total > 0.0).then(|| (part / total * 100.0).round() as i64)
}
