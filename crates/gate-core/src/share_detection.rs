//! 分享检测健康度。心跳在线不等于消息已经核对并提交。
use serde::{Deserialize, Serialize};

pub const GRACE_SECONDS: f64 = 300.0;

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Report {
    pub connected: bool,
    pub last_complete_at: f64,
    pub complete_day_start: f64,
    /// Earliest fully reconciled service-day boundary; absent old reports fail open.
    #[serde(default)]
    pub coverage_start: Option<f64>,
    pub pending: usize,
    pub oldest_failure_at: Option<f64>,
    pub error: Option<String>,
}

impl Report {
    pub fn valid(&self, now: f64) -> bool {
        [self.last_complete_at, self.complete_day_start]
            .into_iter()
            .chain(self.oldest_failure_at)
            .chain(self.coverage_start)
            .all(|t| t.is_finite() && t >= 0.0 && t <= now + 120.0)
            && self.error.as_ref().is_none_or(|s| s.len() <= 160)
    }

    /// 最近一次完整核对仍在五分钟窗口内。异常期间保留窗口，过期自动放行。
    pub fn enforceable(&self, seen_at: f64, now: f64, day_start: f64) -> bool {
        self.valid(now)
            && seen_at <= now + 120.0
            && now - seen_at < GRACE_SECONDS
            && self.last_complete_at > 0.0
            && now - self.last_complete_at < GRACE_SECONDS
            && self.complete_day_start == day_start
            && self
                .coverage_start
                .is_some_and(|at| at == day_start - 86400.0)
    }

    pub fn healthy(&self, seen_at: f64, now: f64, day_start: f64) -> bool {
        self.enforceable(seen_at, now, day_start)
            && self.connected
            && self.pending == 0
            && self.error.is_none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heartbeat_cannot_replace_a_complete_sweep_or_extend_its_grace() {
        let mut r = Report {
            connected: true,
            last_complete_at: 87400.0,
            complete_day_start: 87300.0,
            coverage_start: Some(87300.0 - 86400.0),
            ..Default::default()
        };
        assert!(r.healthy(87400.0, 87401.0, 87300.0));
        r.pending = 1;
        assert!(!r.healthy(87400.0, 87401.0, 87300.0));
        assert!(r.enforceable(87400.0, 87699.0, 87300.0));
        assert!(!r.enforceable(87700.0, 87700.0, 87300.0));
        assert!(!r.enforceable(87400.0, 87401.0, 87401.0));
        r.last_complete_at = f64::NAN;
        assert!(!r.enforceable(87400.0, 87401.0, 87300.0));
    }
}
