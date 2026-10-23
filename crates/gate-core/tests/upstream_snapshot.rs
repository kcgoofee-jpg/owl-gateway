//! `upstream_policy` / `upstream_errors` 的快照：输入固定，输出是当前实现的结果（更新命令见失败信息）。
//! 依赖时间的函数用固定的 `NOW`。

mod common;

use common::Tally;
use gate_core::upstream_errors::upstream_error_message;
use gate_core::upstream_policy as up;
use gate_snapshot::snapshot::check;
use gate_snapshot::{as_f64, f};
use serde_json::json;

const M: &str = env!("CARGO_MANIFEST_DIR");
const NOW: f64 = 1_791_900_000.0;

#[test]
fn clamp_retry_after_snapshot() {
    let mut n = 0;
    check(M, "upstream/clamp_retry_after", |c| {
        n += 1;
        Some(f(up::clamp_retry_after_f64(as_f64(c.arg(0))?)))
    });
    assert!(n >= 20, "用例太少：{n}");
}

#[test]
fn parse_retry_after_snapshot() {
    let mut tally = Tally::default();
    check(M, "upstream/parse_retry_after", |c| {
        let out = f(up::parse_retry_after(c.arg(0).as_str(), NOW));
        tally.add("all");
        tally.add_if(out == f(20.0), "default");
        Some(out)
    });
    tally.assert_at_least(&[("all", 30), ("default", 2)]);
}

#[test]
fn is_model_level_403_snapshot() {
    let mut tally = Tally::default();
    check(M, "upstream/is_model_level_403", |c| {
        let hit = up::is_model_level_403(c.arg(0).as_str().unwrap_or(""));
        tally.add(if hit { "hit" } else { "miss" });
        Some(json!(hit))
    });
    tally.assert_at_least(&[("hit", 5), ("miss", 5)]);
}

#[test]
fn upstream_error_message_snapshot() {
    let mut n = 0;
    check(M, "upstream/upstream_error_message", |c| {
        n += 1;
        Some(json!(upstream_error_message(c.arg(0).as_i64()?)))
    });
    assert!(n >= 20, "用例太少：{n}");
}
