//! Site-paid V5: when an upstream account's free V5 meter is exhausted, V5 images routed to an
//! account listed in the `v5_paid_tokens` setting may be paid from that account's own Anlas, until a
//! deadline and up to a budget. The member is not charged Anlas; their V5 per-person quota still counts.
//!
//! Setting shape: `{"token-id": {"until": 1792913433, "max_anlas": 9500}}`.

use serde_json::Value;

/// Setting key.
pub const KEY: &str = "v5_paid_tokens";
/// Marker written into the usage-log detail of site-paid images; spend is summed by it.
pub const MARKER: &str = "站付V5";

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Grant {
    pub until: f64,
    pub max_anlas: f64,
}

/// The live grant for `token_id` at `now`, if any. Malformed entries are ignored.
pub fn grant(raw: Option<&str>, token_id: &str, now: f64) -> Option<Grant> {
    let v: Value = serde_json::from_str(raw?).ok()?;
    let g = v.get(token_id)?;
    let until = g.get("until")?.as_f64()?;
    let max_anlas = g.get("max_anlas")?.as_f64().filter(|m| *m > 0.0)?;
    (now < until).then_some(Grant { until, max_anlas })
}

/// Whether one more image costing `cost` fits: `spent + cost <= max_anlas`.
pub fn fits(g: Grant, spent: f64, cost: f64) -> bool {
    cost > 0.0 && spent + cost <= g.max_anlas
}

#[cfg(test)]
mod tests {
    use super::*;

    const RAW: &str = r#"{"t1": {"until": 2000, "max_anlas": 100}, "t2": {"until": 2000}}"#;

    #[test]
    fn listed_live_account_gets_a_grant() {
        assert_eq!(
            grant(Some(RAW), "t1", 1000.0),
            Some(Grant {
                until: 2000.0,
                max_anlas: 100.0
            })
        );
    }

    #[test]
    fn unlisted_expired_or_malformed_get_nothing() {
        assert_eq!(grant(Some(RAW), "t3", 1000.0), None); // not listed
        assert_eq!(grant(Some(RAW), "t1", 2000.0), None); // expired
        assert_eq!(grant(Some(RAW), "t2", 1000.0), None); // no budget
        assert_eq!(grant(Some("not json"), "t1", 1000.0), None);
        assert_eq!(grant(None, "t1", 1000.0), None);
    }

    #[test]
    fn budget_is_a_hard_ceiling() {
        let g = Grant {
            until: 2000.0,
            max_anlas: 100.0,
        };
        assert!(fits(g, 80.0, 20.0));
        assert!(!fits(g, 81.0, 20.0));
        assert!(!fits(g, 0.0, 0.0));
    }
}
