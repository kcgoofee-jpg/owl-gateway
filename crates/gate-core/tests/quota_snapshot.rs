//! V5 电量记录的快照（`quota/sqlmath`）：安静时段测速和账号读数汇总。只用于后台「记录」显示，不参与任何限额。

mod common;

use common::Tally;
use gate_core::quota::*;
use gate_snapshot::snapshot::check;
use gate_snapshot::{as_f64, f};
use serde_json::{Value, json};

const M: &str = env!("CARGO_MANIFEST_DIR");

fn fv(x: Option<f64>) -> Value {
    x.map_or(Value::Null, f)
}

fn ints(v: &Value) -> Vec<i64> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_i64().unwrap())
        .collect()
}

#[test]
fn sqlmath_snapshot() {
    let mut tally = Tally::default();
    check(M, "quota/sqlmath", |c| {
        let name = c.get("fn")?.as_str()?;
        tally.add(name);
        Some(match name {
            // 安静时段测速，以及服务层该查的参数：快照下界、要数 V5 条数的相邻快照对
            "quiet_recovery" => {
                let now = as_f64(c.arg(0))?;
                let q = QuietInputs {
                    snaps: serde_json::from_value(c.arg(1).clone()).ok()?,
                    counts: ints(c.arg(2)),
                };
                let pairs: Vec<Value> = quiet_count_pairs(&q.snaps)
                    .into_iter()
                    .map(|i| json!([f(q.snaps[i].ts), f(q.snaps[i + 1].ts)]))
                    .collect();
                json!({
                    "result": quiet_recovery(&q).map_or(Value::Null, |x| x.to_value()),
                    "snaps_since": f(quiet_snaps_since(now)),
                    "pairs": pairs,
                })
            }
            "_allowance" => {
                let accounts: Vec<AccountAllowance> =
                    serde_json::from_value(c.arg(0).clone()).ok()?;
                let (pct, rate) = allowance_summary(&accounts);
                json!([fv(pct), fv(rate)])
            }
            other => panic!("未知函数 {other}"),
        })
    });
    tally.assert_at_least(&[("quiet_recovery", 20), ("_allowance", 20)]);
}
