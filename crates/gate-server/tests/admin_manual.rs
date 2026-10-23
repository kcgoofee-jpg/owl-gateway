//! 手动 / 自动的区分（站长 10-20）：`POST /admin/api/ops/manual`（运维改临时设置，同一事务写「手动（Claude）」记录）、
//! nai-ops 请求头 → 操作者、操作日志的 `kind` 与 `actor=manual|auto` 筛选、`/quota-records` 的 `manual` 段。
//! 公开仓库：全部是合成数据。
mod common;
use common::*;
use serde_json::{Value, json};

/// 2026-10-14 12:00:00 +08:00
const NOW: f64 = 1_791_950_400.0;

async fn token_id(e: &Env) -> String {
    e.state.nai.status_rows().await[0].0.token_id.clone()
}

#[tokio::test]
async fn manual_changes_are_written_with_their_record() {
    let _clock = clock(Some(NOW)).await;
    let e = setup(&[]).await;
    let api = Admin::login_bare(&e.state).await;
    let tid = token_id(&e).await;
    let until = NOW + 86_400.0;

    // 账号日上限临时调整：写库 + 记录 + 网关内存即时生效（不用重启）
    let (s, v) = api
        .json(
            "POST",
            "/ops/manual",
            Some(json!({
                "key": "guard_token_daily_caps",
                "value": {tid.clone(): {"cap": 2200, "until": until}},
                "why": "站长批准：到期前多出图",
            })),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["before"], Value::Null);
    assert!(
        setting(&e.state, "guard_token_daily_caps")
            .await
            .unwrap()
            .contains("2200")
    );
    assert_eq!(e.state.guard.with(|g| g.daily_cap_for(&tid, NOW)), 2200);
    let a = &admin_actions(&e.state.db, 1).await[0];
    assert_eq!(a["action"], "手动：账号日上限临时调整");
    assert!(a["actor"].as_str().unwrap().starts_with("后台"), "{a}");
    assert!(a["detail"].as_str().unwrap().contains("站长批准"), "{a}");

    // 不认识的账号、过期、没写原因、不支持的键：422，不写
    for body in [
        json!({"key": "guard_token_daily_caps", "value": {"token-nope": {"cap": 1, "until": until}}, "why": "x"}),
        json!({"key": "guard_token_daily_caps", "value": {tid.clone(): {"cap": 1, "until": NOW - 1.0}}, "why": "x"}),
        json!({"key": "guard_token_daily_caps", "value": {tid.clone(): {"cap": 1, "until": until}}}),
        // V5 临时加量 / 改当天 V5 方案随 V5 自动方案删除（站长 10-23）
        json!({"key": "quota_v5_bonus", "value": {"daily": 400, "until": until}, "why": "x"}),
        json!({"key": "v5_plan", "value": {"global": 0}, "why": "x"}),
        json!({"key": "guard_account_daily_cap", "value": {}, "why": "x"}),
        json!({"why": "x"}),
    ] {
        let (s, _) = api.json("POST", "/ops/manual", Some(body.clone())).await;
        assert_eq!(s, 422, "{body}");
    }
    assert_eq!(setting(&e.state, "quota_v5_bonus").await, None);

    // nai-ops 带的请求头：操作者记「手动（Claude）」
    let body = json!({"key": "v5_paid_tokens", "value": {tid.clone(): {"max_anlas": 500, "until": until}}, "why": "站付"});
    let s = api
        .raw(
            "POST",
            "/ops/manual",
            Some(body),
            &[("x-owl-actor", "claude")],
        )
        .await
        .status;
    assert_eq!(s, 200);
    let a = &admin_actions(&e.state.db, 1).await[0];
    assert_eq!(
        (a["actor"].as_str(), a["action"].as_str()),
        (Some("手动（Claude）"), Some("手动：站付 V5"))
    );

    // 只补记一笔
    let (s, v) = api
        .json(
            "POST",
            "/ops/manual",
            Some(json!({"action": "重启网关", "why": "让新上限生效"})),
        )
        .await;
    assert_eq!((s, v["recorded"].clone()), (200, json!(true)));
    assert_eq!(admin_actions(&e.state.db, 1).await[0]["action"], "重启网关");

    // 删除：value = null
    let (s, v) = api
        .json(
            "POST",
            "/ops/manual",
            Some(json!({"key": "guard_token_daily_caps", "value": null, "why": "收回"})),
        )
        .await;
    assert_eq!((s, v["after"].clone()), (200, Value::Null));
    let base = e.state.guard.with(|g| g.values["account_daily_cap"]);
    assert_eq!(e.state.guard.with(|g| g.daily_cap_for(&tid, NOW)), base);

    // /quota-records 的 manual 段：只列还在生效的；账号给编号不给 Token
    let (_, q) = api.json("GET", "/quota-records", None).await;
    assert_eq!(q["manual"]["account_caps"], json!([]));
    assert_eq!(q["manual"]["paid_v5"][0]["position"], 1);
    assert_eq!(q["manual"]["paid_v5"][0]["max_anlas"], 500.0);
}

#[tokio::test]
async fn action_log_kind_and_filter() {
    let e = setup(&[]).await;
    let api = Admin::login_bare(&e.state).await;
    e.state
        .db
        .write(|tx| {
            tx.execute_batch(
                "INSERT INTO admin_actions (ts, actor, action, target, detail, ok) VALUES \
                 (1.0, '站长', '添加黑名单', '', '', 1), (2.0, '系统', '动态额度调整', '', '', 1), \
                 (3.0, 'Discord:900000000000000001', '领取 Key', '', '', 1), \
                 (4.0, '手动（Claude）', 'V5 不设限试验', '', '', 1), (5.0, '自动驾驶', '节约模式', '', '', 1), \
                 (6.0, '站长指示', '成员标签', '', '', 1), (7.0, '网页:*.*.0.1', '收到捐赠 Key', '', '', 1);",
            )
        })
        .await
        .unwrap();
    let rows = |v: &Value| -> Vec<(String, String)> {
        v["actions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| {
                (
                    r["action"].as_str().unwrap().to_string(),
                    r["kind"].as_str().unwrap().to_string(),
                )
            })
            .collect()
    };
    let (_, all) = api.json("GET", "/actions", None).await;
    let all = rows(&all);
    let kind_of = |a: &str| all.iter().find(|(n, _)| n == a).unwrap().1.clone();
    assert_eq!(kind_of("添加黑名单"), "manual");
    assert_eq!(kind_of("登录后台"), "manual");
    assert_eq!(kind_of("V5 不设限试验"), "manual");
    assert_eq!(kind_of("成员标签"), "manual");
    assert_eq!(kind_of("动态额度调整"), "auto");
    assert_eq!(kind_of("节约模式"), "auto");
    assert_eq!(kind_of("领取 Key"), "member");
    assert_eq!(kind_of("收到捐赠 Key"), "member");

    let names = |v: Value| -> Vec<String> { rows(&v).into_iter().map(|r| r.0).collect() };
    // 登录在插入之前（id 最小），最新的在前
    assert_eq!(
        names(api.json("GET", "/actions?actor=manual", None).await.1),
        ["成员标签", "V5 不设限试验", "添加黑名单", "登录后台"]
    );
    assert_eq!(
        names(api.json("GET", "/actions?actor=auto", None).await.1),
        ["节约模式", "动态额度调整"]
    );
}
