//! 后台登录 / 会话 / 来源校验 / 操作日志。

mod common;
use common::*;
use serde_json::{Value, json};

/// 后台全部接口：(方法, 路径)。路径不带 `/admin/api` 前缀，路径参数用占位值。
const ENDPOINTS: &[(&str, &str)] = &[
    // 成员 / 邀请 / 黑名单
    ("GET", "/keys"),
    ("POST", "/keys/invite"),
    ("POST", "/keys/1/regenerate"),
    ("PATCH", "/keys/1"),
    ("POST", "/keys/1/reset-daily-image-quota"),
    ("GET", "/keys/1/coupons"),
    ("POST", "/keys/1/coupons"),
    ("DELETE", "/keys/1"),
    ("PUT", "/keys/1/tags"),
    ("DELETE", "/keys/1/tags/x"),
    ("POST", "/keys/1/share-clear"),
    ("GET", "/members"),
    ("GET", "/member-rules"),
    ("PUT", "/member-rules"),
    ("GET", "/members/900000000000000001/rules"),
    ("PUT", "/members/900000000000000001/rules"),
    ("GET", "/bans"),
    ("POST", "/bans"),
    ("DELETE", "/bans/9"),
    // 日志 / 记录 / 导出
    ("GET", "/logs"),
    ("GET", "/actions"),
    ("POST", "/ops/reset-all-keys"),
    // 待审（防滥用）
    ("GET", "/abuse"),
    ("POST", "/abuse/1"),
    // 运营 / 监控
    ("GET", "/overview"),
    ("GET", "/status"),
    ("GET", "/settings"),
    ("PUT", "/settings"),
    ("GET", "/runtime-limits"),
    ("PUT", "/runtime-limits"),
    ("GET", "/upstream"),
    ("POST", "/upstream-tokens"),
    ("PUT", "/upstream-tokens/token-x"),
    ("DELETE", "/upstream-tokens/token-x"),
    ("PUT", "/upstream-tokens/token-x/v5-limit"),
    ("GET", "/quota-records"),
    ("PUT", "/upstream-tokens/token-x/enabled"),
    ("POST", "/alerts/test"),
    ("GET", "/ops"),
    ("PUT", "/ops/registration"),
    ("PUT", "/ops/features"),
    ("PUT", "/ops/log-retention"),
    ("GET", "/guard"),
    ("PUT", "/guard"),
    ("GET", "/discord-bot"),
    ("GET", "/sitepaid-hints"),
    ("POST", "/reconciliation"),
];

#[tokio::test]
async fn login_session_and_action_log() {
    let e = setup(&[]).await;
    let (status, _, body) = Call::new("POST", "/admin/api/login")
        .header("host", "owl.example")
        .header("origin", "https://owl.example")
        .json(json!({"password": "wrong"}))
        .send(&e.state)
        .await;
    assert_eq!(status, 401);
    assert_eq!(
        serde_json::from_slice::<Value>(&body).unwrap(),
        json!({"detail": "密码错误"})
    );

    let (status, headers, _) = Call::new("POST", "/admin/api/login")
        .header("host", "owl.example")
        .header("origin", "https://owl.example")
        .json(json!({"password": ADMIN_PASSWORD}))
        .send(&e.state)
        .await;
    assert_eq!(status, 200);
    let set = headers["set-cookie"].to_str().unwrap().to_string();
    assert!(
        set.starts_with("nai_gate_admin=")
            && set.contains("HttpOnly")
            && set.contains("SameSite=strict"),
        "{set}"
    );
    let cookie = set.split(';').next().unwrap().to_string();

    let (status, headers, _) = Call::new("GET", "/admin/api/me")
        .header("cookie", cookie.clone())
        .send(&e.state)
        .await;
    assert_eq!(status, 200);
    assert_eq!(headers["cache-control"], "no-store");
    let (status, _, _) = Call::new("GET", "/admin/api/me").send(&e.state).await;
    assert_eq!(status, 401);

    // 跨站写请求被拒
    let (status, _, body) = Call::new("POST", "/admin/api/logout")
        .header("host", "owl.example")
        .header("origin", "https://evil.example")
        .header("cookie", cookie.clone())
        .send(&e.state)
        .await;
    assert_eq!(
        status,
        403,
        "跨站 logout 必须拒绝: {}",
        String::from_utf8_lossy(&body)
    );
    // 同源 logout 仍然可用
    let (status, headers, _) = Call::new("POST", "/admin/api/logout")
        .header("host", "owl.example")
        .header("origin", "https://owl.example")
        .header("cookie", cookie.clone())
        .send(&e.state)
        .await;
    assert_eq!(status, 200);
    assert!(
        headers["set-cookie"]
            .to_str()
            .unwrap()
            .contains("Max-Age=0")
    );

    settle().await;
    let rows = common::admin_actions(&e.state.db, 10).await;
    let actions: Vec<(String, bool)> = rows
        .iter()
        .map(|r| {
            (
                r["action"].as_str().unwrap().to_string(),
                r["ok"].as_i64() == Some(1),
            )
        })
        .collect();
    assert!(actions.contains(&("登录后台".into(), false)), "{actions:?}");
    assert!(actions.contains(&("登录后台".into(), true)), "{actions:?}");
    assert!(actions.contains(&("退出登录".into(), true)), "{actions:?}");
}

#[tokio::test]
async fn cross_origin_write_rejected() {
    let e = setup(&[]).await;
    let (status, _, body) = Call::new("POST", "/admin/api/login")
        .header("host", "owl.example")
        .header("origin", "https://owl.example.evil.net")
        .json(json!({"password": ADMIN_PASSWORD}))
        .send(&e.state)
        .await;
    assert_eq!(status, 403);
    assert_eq!(
        serde_json::from_slice::<Value>(&body).unwrap(),
        json!({"detail": "来源校验失败"})
    );
}

/// 每个后台接口：没登录 401（不进操作日志）；带着有效会话的跨站写请求 403，各记一条失败（外加登录那一条）；读请求不校验来源。
/// 各组接口自己的参数校验（422）留在各自的测试文件里。
#[tokio::test]
async fn every_endpoint_requires_login_and_same_origin() {
    let e = setup(&[]).await;
    for (method, path) in ENDPOINTS {
        let body = (*method != "GET").then(|| json!({}));
        send(
            &e.state,
            method,
            &format!("/admin/api{path}"),
            None,
            body,
            &[],
        )
        .await
        .assert_detail(401, "未登录或会话已过期");
    }
    settle().await;
    assert!(
        admin_actions(&e.state.db, 500).await.is_empty(),
        "没登录的请求不进操作日志（防止被刷）"
    );

    let a = Admin::login(&e.state).await;
    let mut writes = 0;
    for (method, path) in ENDPOINTS.iter().filter(|(m, _)| *m != "GET") {
        writes += 1;
        a.raw(
            method,
            path,
            Some(json!({})),
            &[("origin", "https://evil.example")],
        )
        .await
        .assert_detail(403, "来源校验失败");
    }
    // 读请求不校验来源
    let r = a
        .raw(
            "GET",
            "/settings",
            None,
            &[("origin", "https://evil.example")],
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    let acts = a.actions().await;
    assert_eq!(acts.len(), writes + 1, "{acts:?}");
    assert_eq!(
        acts.iter()
            .filter(|(_, _, d, ok)| d == "失败（403）：来源校验失败" && !ok)
            .count(),
        writes,
        "{acts:?}"
    );
}
