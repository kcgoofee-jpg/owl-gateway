//! 容量相关的定时任务「上游 V5 快照」（「每小时上限自动调整」10-22 删除）。
//! 真实 SQLite + 假上游 + 冻结时钟；公开仓库，全部是合成数据。
//! 10-24 起每个上游账号各记一行（`token_id` 标明归属）。

mod common;
use common::*;
use gate_testkit::Reply;
use serde_json::{Value, json};

/// 2026-10-14 12:00:00 +08:00
const NOW: f64 = 1_791_950_400.0;

async fn snapshots(e: &Env) -> Vec<Value> {
    query(
        &e.state.db,
        "SELECT ts, token_id, v5_percent, v5_rate, anlas, fresh FROM upstream_snapshots ORDER BY ts",
    )
    .await
}

// ------------------------------------------------------------------ 上游 V5 快照

#[tokio::test]
async fn snapshot_at_most_once_an_hour() {
    let _clock = clock(Some(NOW)).await;
    let e = setup(&[]).await;
    // 假上游（一个账号）：剩余 80%，每 3600 秒恢复 1%（= 24%/天），Anlas 余额 10000（读不到时写 NULL，不写 0）
    let s = run_job(&e.state, "upstream_snapshot").await.unwrap();
    assert_eq!(s, "已记录 1 个账号：#1 剩余 80%（恢复 24%/天）");
    assert_eq!(
        snapshots(&e).await,
        [json!({
            "ts": NOW,
            "token_id": "token-12d24769535a51a7",
            "v5_percent": 80.0,
            "v5_rate": 24.0,
            "anlas": 10000.0,
            "fresh": 1
        })]
    );
    for later in [60.0, 1800.0, 3599.0] {
        gate_server::time::freeze(Some(NOW + later));
        assert_eq!(
            run_job(&e.state, "upstream_snapshot").await.unwrap(),
            "距上次不到 1 小时"
        );
    }
    assert_eq!(snapshots(&e).await.len(), 1);
    gate_server::time::freeze(Some(NOW + 3600.0));
    run_job(&e.state, "upstream_snapshot").await.unwrap();
    let rows = snapshots(&e).await;
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[1]["ts"], json!(NOW + 3600.0));
}

#[tokio::test]
async fn a_snapshot_without_a_reading_is_still_recorded() {
    let _clock = clock(Some(NOW)).await;
    // 订阅读失败：这一行没有读数，fresh=0（不能拿来测恢复速度）
    let e = setup(&[]).await;
    e.mock
        .push("/user/subscription", Reply::status(503, "synthetic"));
    let s = run_job(&e.state, "upstream_snapshot").await.unwrap();
    assert_eq!(s, "已记录 1 个账号：#1 没有读数");
    assert_eq!(
        snapshots(&e).await,
        [json!({
            "ts": NOW,
            "token_id": "token-12d24769535a51a7",
            "v5_percent": null,
            "v5_rate": null,
            "anlas": null,
            "fresh": 0
        })]
    );
    // 没有可用的上游账号：同样记一行无读数的占位（token_id 为空），不报错
    let e = setup(&[("NAI_TOKENS", "")]).await;
    e.mock.clear();
    run_job(&e.state, "upstream_snapshot").await.unwrap();
    let rows = snapshots(&e).await;
    assert_eq!(rows[0]["v5_percent"], Value::Null);
    assert_eq!(rows[0]["token_id"], json!(""));
    assert!(e.mock.requests_to("/user/subscription").is_empty());
}

// 「每小时上限自动调整」（hourly_cap）10-22 删除：每小时上限没了，账号节奏改由出图间隔 AIMD 管
// （见 gate-upstream 的 tests/scheduling.rs）。

#[test]
fn the_hourly_cap_job_is_gone() {
    assert!(gate_server::jobs::find("hourly_cap").is_none());
}
