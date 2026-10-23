//! Anlas 自动核对：每小时试一次和后台「Anlas 消耗核对」同样的核对（站长 10-22：要自动核、差距大要报警）。
//!
//! 核对要等到「没有进行中的出图」那一刻，忙的时候常常等不到：这时只记一句「这轮没核成」，下一轮再试，
//! 不算任务失败。核成了且差距偏大（官方多扣了本站没记的），记一条「注意」级 Bug，后台和巡检都能看到。

use crate::jobs::JobError;
use crate::reconciliation::{self, RunError};
use crate::state::AppState;
use serde_json::Value;

/// 差距超过这么多 Anlas、且超过官方减少量的这个比例才报警（估算和实扣本来就有几个百分点的出入）。
const ALERT_MIN_ANLAS: f64 = 300.0;
const ALERT_MIN_RATIO: f64 = 0.1;

pub async fn run(state: &AppState) -> Result<String, JobError> {
    let status = match reconciliation::run(state).await {
        Ok(v) => v,
        Err(RunError::Refused(e)) => return Ok(format!("这轮没核成：{}", e.message)),
        Err(RunError::Internal(e)) => {
            return Err(JobError::new("ReconcileError", format!("{e:?}")));
        }
    };
    let cmp = status
        .pointer("/history/0/comparison")
        .cloned()
        .unwrap_or(Value::Null);
    let num = |k: &str| cmp.get(k).and_then(Value::as_f64).unwrap_or(0.0);
    if let Some(reason) = cmp.get("reason").and_then(Value::as_str) {
        return Ok(format!("已记录余额（没法比较：{reason}）"));
    }
    let (decrease, recorded, diff) = (num("balance_decrease"), num("recorded"), num("difference"));
    let summary = format!(
        "官方减少 {:.0} · 本站记录 {:.0} · 相差 {:.0}",
        decrease, recorded, diff
    );
    if needs_alert(decrease, diff) {
        let detail = format!(
            "{summary}（其中站付 V5 {:.0}）。可能是有图按 Anlas 扣了但本站记成免费，或账号在别处被使用；到后台「上游 › Anlas 消耗核对」看历史。",
            num("site_paid")
        );
        state.bugs.capture(crate::bugs::Report {
            source: "job:anlas_reconcile",
            title: "Anlas 核对差距偏大",
            detail: &detail,
            level: "warn",
            ..Default::default()
        });
    }
    Ok(summary)
}

/// 官方多扣（diff > 0）超过阈值才报；本站记得比官方多是估算偏高，不报。
pub fn needs_alert(decrease: f64, diff: f64) -> bool {
    diff > ALERT_MIN_ANLAS && diff > decrease * ALERT_MIN_RATIO
}

#[cfg(test)]
mod tests {
    use super::needs_alert;

    #[test]
    fn alerts_only_on_large_unrecorded_spend() {
        assert!(needs_alert(5411.0, 2971.0));
        assert!(!needs_alert(5411.0, 307.0), "6% 以内是估算误差");
        assert!(!needs_alert(1000.0, 250.0), "绝对值太小");
        assert!(!needs_alert(5000.0, -800.0), "本站记多了不报");
    }
}
