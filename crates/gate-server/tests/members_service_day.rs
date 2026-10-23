//! 成员页「今日重置」按**服务日**（每天 7 点切换）统计：凌晨 0–7 点仍属于前一个服务日。
//! 单独一个测试文件，因为要冻结全进程的时钟（`time::freeze`），不能和别的测试并行。

mod common;
use common::{Admin, setup};
use gate_db::repo::keys::NewKey;
use serde_json::json;

/// 2026-10-15 06:30（+08:00）：服务日还是 10-14，起点 10-14 07:00。
const NOW: f64 = 1_792_017_000.0;
const DAY_START: f64 = 1_791_932_400.0; // 2026-10-14 07:00

#[tokio::test]
async fn reset_today_counts_from_service_day_start_before_7am() {
    gate_server::time::freeze(Some(NOW));
    let e = setup(&[]).await;
    let a = Admin::login(&e.state).await;
    let k: NewKey = serde_json::from_value(json!({
        "name": "凌晨成员", "token": "nai-test-service-day-000000000000", "daily_images": 50,
        "monthly_anlas": 0.0, "daily_text_tokens": 1000, "rpm": 30, "daily_v5": 5, "daily_anlas": 0.0,
    }))
    .unwrap();
    let id = e.state.db.create_key(&k, NOW).await.unwrap()["id"]
        .as_i64()
        .unwrap();
    let target = format!("Key #{id}");
    let db = &e.state.db;
    // 上一个服务日（10-14 07:00 之前）的一次：不算
    db.add_admin_action("后台", "重置今日额度", &target, "", true, DAY_START - 60.0)
        .await
        .unwrap();
    // 本服务日的两次（10-14 夜里、10-15 凌晨 6:00）：都算；失败的那次不算
    db.add_admin_action(
        "后台",
        "重置今日额度",
        &target,
        "",
        true,
        DAY_START + 3600.0,
    )
    .await
    .unwrap();
    db.add_admin_action("后台", "重置今日额度", &target, "", true, NOW - 1800.0)
        .await
        .unwrap();
    db.add_admin_action("后台", "重置今日额度", &target, "", false, NOW - 60.0)
        .await
        .unwrap();

    let r = a.get("/admin/api/members").await;
    assert_eq!(r.status, 200, "{}", r.text());
    let v = r.json();
    let m = v["members"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["id"] == id)
        .expect("成员在列表里");
    assert_eq!(
        m["reset_today"],
        json!({"count": 2, "last_ts": NOW - 1800.0}),
        "凌晨 6:30 看到的是从昨天 7 点起的两次重置"
    );
    gate_server::time::freeze(None);
}
