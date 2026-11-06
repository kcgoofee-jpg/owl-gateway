//! 上游读数和运行保护任务共用的 I/O。

use super::JobError;
use crate::state::AppState;
use gate_core::quota::AccountAllowance;
use serde_json::Value;

/// 上游账号 V5 缓存里的读数（`allowance.snapshot(pool)["accounts"]`；只读缓存，不发请求）。
async fn allowance_snapshot(state: &AppState) -> Result<Vec<AccountAllowance>, JobError> {
    let ids: Vec<String> = state
        .nai
        .tokens()
        .await
        .iter()
        .map(|t| t.token_id.clone())
        .collect();
    let snap = state
        .allowance
        .snapshot(&ids)
        .await
        .map_err(|e| JobError::new("AllowanceError", e.to_string()))?;
    let accounts = snap
        .get("accounts")
        .and_then(Value::as_array)
        .map(|a| a.as_slice())
        .unwrap_or(&[]);
    Ok(accounts
        .iter()
        .map(|a| AccountAllowance {
            percent: a.get("percent").and_then(Value::as_f64),
            recharge_per_day: a.get("recharge_per_day").and_then(Value::as_f64),
        })
        .collect())
}

/// 对每个可用的上游账号主动读一次订阅信息（失败忽略）；至少一个成功返回 true。
pub(crate) async fn refresh_allowance(state: &AppState) -> bool {
    use gate_upstream::client::Allowance;
    let now = state.now();
    let mut ok = false;
    for t in state.nai.tokens().await {
        if t.usable(now) && state.allowance.with(&state.nai).resolve(&t).await.is_ok() {
            ok = true;
        }
    }
    ok
}

/// 账号 V5 剩余 % 与恢复 %/天。刚重启时缓存里还没有读数，主动读一次（读失败也不报错）。
pub(crate) async fn allowance_accounts(
    state: &AppState,
) -> Result<Vec<AccountAllowance>, JobError> {
    let mut accounts = allowance_snapshot(state).await?;
    if accounts.iter().all(|a| a.percent.is_none()) {
        refresh_allowance(state).await;
        accounts = allowance_snapshot(state).await?;
    }
    Ok(accounts)
}
