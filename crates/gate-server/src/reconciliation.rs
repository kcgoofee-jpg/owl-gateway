//! Anlas 手动核对：查询官方余额，与本站记账对比。
//!
//! 核对时要拿到「没有进行中的出图、新出图也不能开始」的一刻：先等 `state.budget` 里没有额度预留，
//! 持着预算锁（挡住新的准入）去读官方余额，读完再放。每次核对（含失败）都先写 60 秒冷却，
//! 官方 429 则按 Retry-After 写更长的冷却——冷却存在数据库里，重启不会清零。
//! 官方的响应 / 异常里可能带账号信息，所以失败时只给固定文案，从不回显。

use crate::error::{ApiError, InternalError};
use crate::state::AppState;
use gate_core::{primitives, values};
use gate_db::DbError;
use gate_upstream::client::Token;
use serde_json::{Map, Value, json};
use std::sync::Arc;
use std::time::Duration;
use tokio::time::Instant;

pub const COOLDOWN_SETTING: &str = "anlas_reconciliation_retry_at";
/// 等进行中的图片任务结算的最长时间。
const WAIT_TIMEOUT: Duration = Duration::from_secs(30);
/// 查询全部账号的总时限。
const QUERY_TIMEOUT: Duration = Duration::from_secs(30);
/// 单个账号的查询时限。
const ACCOUNT_TIMEOUT: Duration = Duration::from_secs(8);
/// 官方订阅接口的响应体上限。
const MAX_BODY: usize = 65536;
/// 页面上保留的历史条数。
const HISTORY: i64 = 20;

/// 核对失败：`status` 是回给后台的 HTTP 状态码，`retry_after > 0` 时带 `Retry-After` 头。
#[derive(Debug, Clone)]
pub struct ReconError {
    pub message: String,
    pub status: u16,
    pub retry_after: i64,
}

impl ReconError {
    fn new(message: impl Into<String>, status: u16, retry_after: i64) -> Self {
        Self {
            message: message.into(),
            status,
            retry_after,
        }
    }

    fn unreadable() -> Self {
        Self::new("官方余额未能完整读取；上次结果已保留", 503, 0)
    }
}

/// 核对失败：要么是业务上的拒绝 / 失败（固定文案），要么是内部错误（500）。
#[derive(Debug)]
pub enum RunError {
    Refused(ReconError),
    Internal(ApiError),
}

impl From<ReconError> for RunError {
    fn from(e: ReconError) -> Self {
        RunError::Refused(e)
    }
}

impl From<DbError> for RunError {
    fn from(e: DbError) -> Self {
        RunError::Internal(e.into())
    }
}

fn internal(msg: impl Into<String>) -> RunError {
    RunError::Internal(InternalError(msg.into()).into())
}

// ------------------------------------------------------------------ 数值

/// 这些数只给后台页面和核对任务读，读取方都按 f64（页面按 number）；数值在 1e12 以内，f64 能精确表示。
/// 新写入快照里的整数会带小数点（`10.0`），库里的旧快照照样能读。
fn round4(x: f64) -> f64 {
    primitives::round(x, 4)
}

/// `amount(value)`：必须是有限的数字（bool 不算），且在 0～1e12 之间。
fn amount(v: &Value) -> Option<f64> {
    let f = v.as_f64()?;
    (f.is_finite() && (0.0..=1e12).contains(&f)).then_some(f)
}

/// `balance(data)`：官方订阅信息 → `{"fixed", "purchased", "expires_at"}`。读不懂返回 None。
pub fn balance(data: &Value) -> Option<Value> {
    let obj = data.as_object()?;
    if obj.get("active") != Some(&Value::Bool(true)) {
        return None;
    }
    let steps = obj.get("trainingStepsLeft")?.as_object()?;
    let fixed = amount(steps.get("fixedTrainingStepsLeft")?)?;
    let purchased = amount(steps.get("purchasedTrainingSteps")?)?;
    // 用订阅到期时间判断是否进入了新的计费周期
    let expires_at = match obj.get("expiresAt") {
        None | Some(Value::Null) => Value::Null,
        Some(v) => values::float(amount(v)?),
    };
    Some(json!({
        "fixed": values::float(fixed),
        "purchased": values::float(purchased),
        "expires_at": expires_at,
    }))
}

/// 两个值是否相等（数字按数值比较，1 == 1.0；缺失只和缺失相等）。
fn value_eq(a: Option<&Value>, b: Option<&Value>) -> bool {
    match (a, b) {
        (Some(x), Some(y)) => values::equal(x, y),
        (None, None) => true,
        _ => false,
    }
}

/// 两次快照的对比。`previous` 是库里存的上一条；旧快照缺字段时按「存档损坏」报错。
pub fn comparison(previous: Option<&Value>, current: &Value) -> Result<Value, String> {
    let Some(previous) = previous else {
        return Ok(json!({"reason": "first"}));
    };
    let missing = |k: &str| format!("对账快照缺少有效字段: {k}");
    let obj = |v: &Value, k: &str| -> Result<Map<String, Value>, String> {
        v.get(k)
            .and_then(Value::as_object)
            .cloned()
            .ok_or_else(|| missing(k))
    };
    let (old, new) = (obj(previous, "accounts")?, obj(current, "accounts")?);
    if old.len() != new.len() || old.keys().any(|k| !new.contains_key(k)) {
        return Ok(json!({"reason": "accounts_changed"}));
    }
    if old
        .iter()
        .any(|(k, o)| !value_eq(o.get("expires_at"), new[k].get("expires_at")))
    {
        return Ok(json!({"reason": "cycle_changed"}));
    }
    for (k, o) in &old {
        for part in ["fixed", "purchased"] {
            let (Some(a), Some(b)) = (
                new[k].get(part).and_then(Value::as_f64),
                o.get(part).and_then(Value::as_f64),
            ) else {
                return Err(missing(part));
            };
            if a > b {
                return Ok(json!({"reason": "balance_increased"}));
            }
        }
    }
    let (before, after) = (obj(previous, "ledger")?, obj(current, "ledger")?);
    for (k, b) in &before {
        let (Some(a), Some(b)) = (after.get(k).and_then(Value::as_f64), b.as_f64()) else {
            return Err(missing(k));
        };
        if a < b {
            return Ok(json!({"reason": "ledger_reset"}));
        }
    }
    let n = |v: &Value, k: &str| v.get(k).and_then(Value::as_f64).ok_or_else(|| missing(k));
    let ledger = |m: &Map<String, Value>, k: &str| {
        m.get(k).and_then(Value::as_f64).ok_or_else(|| missing(k))
    };
    let decrease = round4(n(previous, "balance")? - n(current, "balance")?);
    // 成员计数之外，账号自己付的站付 V5 也是本站用掉的（旧快照没有这一项时按 0 算，见 run_with 补起点）
    let site = |m: &Map<String, Value>| m.get("site_paid").and_then(Value::as_f64).unwrap_or(0.0);
    let site_paid = round4(site(&after) - site(&before));
    let recorded = round4(ledger(&after, "anlas")? - ledger(&before, "anlas")? + site_paid);
    let since = previous
        .get("checked_at")
        .cloned()
        .ok_or_else(|| missing("checked_at"))?;
    Ok(json!({
        "reason": null,
        "since": since,
        "balance_decrease": values::float(decrease),
        "recorded": values::float(recorded),
        "site_paid": values::float(site_paid),
        "difference": values::float(round4(decrease - recorded)),
        "unconfirmed_anlas": values::float(round4(
            ledger(&after, "unconfirmed_anlas")? - ledger(&before, "unconfirmed_anlas")?,
        )),
        "unconfirmed_requests": values::float(
            ledger(&after, "unconfirmed_requests")? - ledger(&before, "unconfirmed_requests")?,
        ),
    }))
}

// ------------------------------------------------------------------ 官方查询

/// 官方 429 后的冷却秒数：Retry-After（整数秒或 HTTP 日期），至少 5 分钟、至多 1 天。读不懂就 5 分钟。
fn retry_delay(header: &str, now: f64) -> i64 {
    let base = 300_i64;
    let seconds: Option<f64> = if !header.is_empty() && header.bytes().all(|b| b.is_ascii_digit()) {
        header.parse::<f64>().ok()
    } else {
        jiff::fmt::rfc2822::parse(header)
            .ok()
            .map(|z| z.timestamp().as_second() as f64 - now)
    };
    match seconds {
        Some(s) if s.is_finite() => base.max(86_400.min(s.ceil() as i64)),
        _ => base,
    }
}

async fn query(state: &AppState, token: &Token) -> Result<Value, RunError> {
    let url = format!("{}/user/subscription", state.nai.image_host());
    let fetched = tokio::time::timeout(
        ACCOUNT_TIMEOUT,
        state.nai.get_json_with(
            &token.token,
            token.profile_index(),
            &url,
            ACCOUNT_TIMEOUT,
            MAX_BODY,
            false,
        ),
    )
    .await;
    // 官方的响应 / 异常里可能带账号信息或凭据：只给固定文案
    let Ok(Ok((status, headers, body))) = fetched else {
        return Err(ReconError::unreadable().into());
    };
    if status == 401 {
        state.nai.isolate_unauthorized(token).await;
    }
    if status == 429 {
        let header = headers
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        let delay = retry_delay(header, state.now());
        state
            .db
            .set_setting(
                COOLDOWN_SETTING,
                &primitives::float_text(state.now() + delay as f64),
            )
            .await?;
        return Err(ReconError::new("官方查询限流，请稍后重试；上次结果已保留", 429, delay).into());
    }
    if status != 200 {
        return Err(ReconError::unreadable().into());
    }
    let data: Value = serde_json::from_slice(&body).map_err(|_| ReconError::unreadable())?;
    balance(&data).ok_or_else(|| ReconError::unreadable().into())
}

// ------------------------------------------------------------------ 状态与执行

/// 还要等多少秒才能再核对（冷却存在数据库里）。
pub async fn retry_after(state: &AppState) -> Result<i64, RunError> {
    let value = match state.db.get_setting(COOLDOWN_SETTING).await? {
        None => 0.0,
        Some(raw) => primitives::parse_float(&raw)
            .ok_or_else(|| internal(format!("{COOLDOWN_SETTING} 不是数字")))?,
    };
    Ok(((value - state.now()).ceil() as i64).max(0))
}

/// 页面用的状态：最近 20 次结果、冷却剩余秒数、是否正在核对。
pub async fn status(state: &AppState) -> Result<Value, RunError> {
    let history = state.db.reconciliation_history(HISTORY).await?;
    let running = state.ops.reconcile.try_lock().is_err();
    Ok(json!({
        "history": history,
        "retry_after": retry_after(state).await?,
        "running": running,
    }))
}

/// 令牌池里的账号（含已停用的：它们的用量仍在账本里），按 token_id 去重。
async fn accounts(state: &AppState) -> Vec<(String, Arc<Token>)> {
    let mut out: Vec<(String, Arc<Token>)> = Vec::new();
    for t in state.nai.tokens().await {
        match out.iter_mut().find(|(id, _)| *id == t.token_id) {
            Some(slot) => slot.1 = t,
            None => out.push((t.token_id.clone(), t)),
        }
    }
    out
}

/// 执行一次核对，成功返回 [`status`]。
pub async fn run(state: &AppState) -> Result<Value, RunError> {
    run_with(state, WAIT_TIMEOUT).await
}

/// 同 [`run`]，等图片任务结算的最长时间可调（测试用）。
pub async fn run_with(state: &AppState, wait_timeout: Duration) -> Result<Value, RunError> {
    {
        let shared = &state.ops;
        let Ok(_exclusive) = shared.reconcile.try_lock() else {
            return Err(ReconError::new("已有核对正在进行，请稍后查看结果", 409, 0).into());
        };
        let retry = retry_after(state).await?;
        if retry != 0 {
            return Err(ReconError::new(format!("请在 {retry} 秒后重试"), 429, retry).into());
        }
        // 等所有进行中的图片任务结算完，并持着预算锁（挡住新的准入）读官方余额
        let deadline = Instant::now() + wait_timeout;
        let busy = || ReconError::new("图片任务仍在处理，请稍后核对；尚未查询官方", 409, 0);
        let _budget = loop {
            let guard = tokio::time::timeout_at(deadline, state.budget.lock())
                .await
                .map_err(|_| busy())?;
            if !guard.reservations.values().any(|r| r.active) {
                break guard;
            }
            // 先登记等待再放锁：预留清空的通知不会落在两者之间
            let idle = state.budget_idle.notified();
            drop(guard);
            tokio::time::timeout_at(deadline, idle)
                .await
                .map_err(|_| busy())?;
        };
        let pool = accounts(state).await;
        if pool.is_empty() {
            return Err(ReconError::new("尚未配置可查询的上游账号", 503, 0).into());
        }
        if pool.iter().any(|(_, t)| t.fields().disabled) {
            return Err(
                ReconError::new("有上游账号已失效，请先检查令牌池；尚未查询官方", 503, 0).into(),
            );
        }
        let blocked = pool
            .iter()
            .map(|(_, t)| t.fields().blocked_until)
            .fold(f64::NEG_INFINITY, f64::max)
            - state.now();
        if blocked > 0.0 {
            return Err(ReconError::new(
                "上游账号正在冷却，请稍后核对",
                429,
                blocked.ceil() as i64,
            )
            .into());
        }
        // 先把冷却写进库，失败的尝试也算
        state
            .db
            .set_setting(
                COOLDOWN_SETTING,
                &primitives::float_text(state.now() + 60.0),
            )
            .await?;
        let queried = tokio::time::timeout(QUERY_TIMEOUT, async {
            let mut rows = Map::new();
            for (identity, token) in &pool {
                rows.insert(identity.clone(), query(state, token).await?);
            }
            Ok::<_, RunError>(rows)
        })
        .await;
        let rows = match queried {
            Err(_) => return Err(ReconError::new("本次核对超时；上次结果已保留", 503, 0).into()),
            Ok(rows) => rows?,
        };
        let now_ids: Vec<String> = accounts(state)
            .await
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        if now_ids.len() != pool.len() || pool.iter().any(|(id, _)| !now_ids.contains(id)) {
            return Err(ReconError::new("查询期间账号列表有变化，请重新查询", 503, 0).into());
        }
        let ledger = state.db.reconciliation_totals().await?;
        let mut balance = 0.0_f64;
        for row in rows.values() {
            let part = |k: &str| row.get(k).and_then(Value::as_f64).unwrap_or(0.0);
            balance += part("fixed") + part("purchased");
        }
        let mut current = Map::new();
        current.insert("checked_at".into(), values::float(state.now()));
        current.insert("accounts".into(), Value::Object(rows));
        current.insert("ledger".into(), ledger);
        current.insert("balance".into(), values::float(balance));
        let current = Value::Object(current);
        let mut previous = state.db.reconciliation_history(1).await?;
        // 旧快照没记站付 V5：补上它那一刻的累计，不然这次会把以前所有站付都算进来
        if let Some(p) = previous.first_mut()
            && let Some(at) = p.get("checked_at").and_then(Value::as_f64)
            && let Some(l) = p.get_mut("ledger").and_then(Value::as_object_mut)
            && !l.contains_key("site_paid")
        {
            let before = state.db.site_paid_before(at).await?;
            l.insert("site_paid".into(), values::float(before));
        }
        let cmp = comparison(previous.first(), &current).map_err(internal)?;
        let mut snapshot = current;
        if let Some(m) = snapshot.as_object_mut() {
            m.insert("comparison".into(), cmp);
        }
        // 保存要做完：调用方断开也不能留下半条
        let db = state.db.clone();
        let saved = tokio::spawn(async move { db.save_reconciliation(&snapshot).await })
            .await
            .map_err(|e| internal(e.to_string()))?;
        if saved.is_err() {
            return Err(ReconError::new("核对结果未能保存；上次结果已保留", 503, 0).into());
        }
    }
    status(state).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn site_paid_counts_as_recorded_and_old_snapshots_start_from_zero() {
        let snap = |fixed: i64, anlas: f64, site: Option<f64>| {
            let mut ledger = json!({"anlas": anlas, "unconfirmed_anlas": 0, "unconfirmed_requests": 0, "last_log_id": 1});
            if let Some(s) = site {
                ledger["site_paid"] = json!(s);
            }
            json!({"checked_at": 1.0, "balance": fixed,
                   "accounts": {"t": {"fixed": fixed, "purchased": 0, "expires_at": 9}},
                   "ledger": ledger})
        };
        // 成员记了 2440，站付 2664：本站一共 5104，官方少了 5411 → 只差 307
        let c = comparison(
            Some(&snap(9766, 100.0, Some(50.0))),
            &snap(4355, 2540.0, Some(2714.0)),
        )
        .unwrap();
        assert_eq!(c["recorded"], json!(5104.0));
        assert_eq!(c["site_paid"], json!(2664.0));
        assert_eq!(c["difference"], json!(307.0));
        // 上一份没有 site_paid（旧快照，run_with 会补；这里没补就按 0）
        let c = comparison(
            Some(&snap(9766, 100.0, None)),
            &snap(4355, 2540.0, Some(2664.0)),
        )
        .unwrap();
        assert_eq!(c["recorded"], json!(5104.0));
    }

    #[test]
    fn balance_validation_matches_python() {
        let ok = json!({"active": true, "expiresAt": 1900000000,
            "trainingStepsLeft": {"fixedTrainingStepsLeft": 1000, "purchasedTrainingSteps": 100}});
        assert_eq!(
            balance(&ok).unwrap(),
            json!({"fixed": 1000.0, "purchased": 100.0, "expires_at": 1900000000.0})
        );
        // 没有 expiresAt → null；一律按 f64 输出（整数带小数点）
        let f = json!({"active": true, "trainingStepsLeft": {"fixedTrainingStepsLeft": 1.5, "purchasedTrainingSteps": 0}});
        assert_eq!(
            balance(&f).unwrap(),
            json!({"fixed": 1.5, "purchased": 0.0, "expires_at": null})
        );
        for bad in [
            json!(null),
            json!(-1),
            json!(true),
            json!("100"),
            json!(1e13),
            json!([1]),
        ] {
            let d = json!({"active": true, "trainingStepsLeft": {"fixedTrainingStepsLeft": bad, "purchasedTrainingSteps": 0}});
            assert!(balance(&d).is_none(), "{bad}");
        }
        // active 必须恰好是 true
        let inactive = json!({"active": 1, "trainingStepsLeft": {"fixedTrainingStepsLeft": 1, "purchasedTrainingSteps": 0}});
        assert!(balance(&inactive).is_none());
        assert!(balance(&json!([])).is_none());
    }

    fn snap(fixed: i64, expires: i64, anlas: Value, unconfirmed: Value) -> Value {
        json!({"checked_at": 100.5,
            "accounts": {"token-a": {"fixed": fixed, "purchased": 100, "expires_at": expires}},
            "ledger": {"anlas": anlas, "unconfirmed_anlas": unconfirmed, "unconfirmed_requests": 0, "last_log_id": 5},
            "balance": fixed + 100})
    }

    #[test]
    fn comparison_cases_match_python() {
        let first = snap(1000, 1900000000, json!(0), json!(0));
        assert_eq!(
            comparison(None, &first).unwrap(),
            json!({"reason": "first"})
        );
        // 余额减少 10、账本记 7、未确认 3（数值一律按 f64 输出）
        let mut next = snap(990, 1900000000, json!(7.0), json!(3.0));
        next["ledger"]["unconfirmed_requests"] = json!(1);
        let c = comparison(Some(&first), &next).unwrap();
        assert_eq!(
            c,
            json!({"reason": null, "since": 100.5, "balance_decrease": 10.0, "recorded": 7.0,
                   "site_paid": 0.0, "difference": 3.0, "unconfirmed_anlas": 3.0, "unconfirmed_requests": 1.0})
        );
        // 序列化时整数值也带小数点
        let text = String::from_utf8(gate_core::json::wire(&c)).unwrap();
        assert!(
            text.contains("\"balance_decrease\":10.0,") && text.contains("\"recorded\":7.0,"),
            "{text}"
        );
        // 不连续 → 重新记基线
        let cycle = snap(990, 1900086400, json!(0), json!(0));
        assert_eq!(
            comparison(Some(&first), &cycle).unwrap(),
            json!({"reason": "cycle_changed"})
        );
        let credit = snap(1200, 1900000000, json!(0), json!(0));
        assert_eq!(
            comparison(Some(&first), &credit).unwrap(),
            json!({"reason": "balance_increased"})
        );
        let ledger = snap(990, 1900000000, json!(-1), json!(0));
        assert_eq!(
            comparison(Some(&snap(990, 1900000000, json!(5), json!(0))), &ledger).unwrap(),
            json!({"reason": "ledger_reset"})
        );
        let mut other = snap(990, 1900000000, json!(0), json!(0));
        other["accounts"] =
            json!({"token-b": {"fixed": 1, "purchased": 1, "expires_at": 1900000000}});
        assert_eq!(
            comparison(Some(&first), &other).unwrap(),
            json!({"reason": "accounts_changed"})
        );
    }

    #[test]
    fn rate_limit_delay_like_python() {
        assert_eq!(retry_delay("600", 0.0), 600);
        assert_eq!(retry_delay("5", 0.0), 300);
        assert_eq!(retry_delay("999999", 0.0), 86_400);
        assert_eq!(retry_delay("invalid", 0.0), 300);
        assert_eq!(retry_delay("", 0.0), 300);
        // HTTP 日期：2100 年远超一天 → 封顶
        assert_eq!(retry_delay("Fri, 01 Jan 2100 00:00:00 GMT", 0.0), 86_400);
        // 日期在 10 分钟后
        assert_eq!(retry_delay("Thu, 01 Jan 1970 00:10:00 GMT", 0.0), 600);
    }
}
