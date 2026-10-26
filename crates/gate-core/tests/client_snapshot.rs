mod common;

use common::Tally;
use gate_core::client_views::{SubscriptionData, SubscriptionError, subscription_payload};
use gate_snapshot::as_f64;
use gate_snapshot::snapshot::check;
use serde_json::json;

const M: &str = env!("CARGO_MANIFEST_DIR");

/// 订阅信息：成功是整份响应；缺字段（Python 的 KeyError）记 `{"err": "KeyError"}`。
#[test]
fn subscription_payload_snapshot() {
    let mut tally = Tally::default();
    check(M, "client/subscription", |c| {
        let d = c.get("data")?;
        let data = SubscriptionData {
            month_anlas_used: as_f64(&d["month_anlas_used"])?,
            daily_v5_used: d["daily_v5_used"].as_i64()?,
            global_v5_limit: d["global_v5_limit"].as_i64()?,
            global_v5_total: d["global_v5_total"].as_i64()?,
            v5_paused: false,
            v5_access_allowed: true,
            global_quota_modes: gate_core::quota_limits::GlobalLimits::from_legacy([
                150.0,
                0.0,
                d["global_v5_limit"].as_i64()? as f64,
                200.0,
                50.0,
                10000.0,
            ]),
        };
        let got = subscription_payload(c.get("key")?, as_f64(c.get("now")?)?, &data);
        tally.add("all");
        Some(match got {
            Ok(v) => v,
            Err(SubscriptionError::MissingKey(k)) => {
                tally.add("missing");
                json!({"err": "KeyError", "key": k})
            }
            Err(SubscriptionError::BadNumber(k)) => {
                tally.add("bad_number");
                json!({"err": "BadNumber", "key": k})
            }
            Err(SubscriptionError::BadQuotaModes) => json!({"err": "BadQuotaModes"}),
        })
    });
    tally.assert_at_least(&[("all", 30), ("missing", 2)]);
}
