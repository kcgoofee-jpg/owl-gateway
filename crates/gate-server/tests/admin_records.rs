//! 后台请求日志、操作日志和全站重置。
//!
//! 逐字节快照见 `admin_snapshot.rs`；这里按接口写成可读的用例（鉴权、校验文案、返回结构、写进库的行、操作日志、导出内容）。

mod common;
use common::{query, ready};
use serde_json::{Value, json};

const A: &str = "/admin/api";

/// 8×8 的合成 PNG。

#[tokio::test]
async fn usage_logs_filter_and_paginate() {
    let (e, a) = ready().await;
    // 两把 Key 的日志 + 各种功能
    a.sql("INSERT INTO api_keys(id, name, token, created_at) VALUES (2, '二号', 'nai-t-2', 1.0);")
        .await;
    let mut sql = String::new();
    for i in 0..45 {
        let (kid, kind) = match i % 3 {
            0 => (1, "image"),
            1 => (2, "upscale"),
            _ => (1, "chat"),
        };
        let model = if i % 2 == 0 {
            "nai-diffusion-4-5-full"
        } else {
            "nai-diffusion-5-full"
        };
        sql.push_str(&format!(
            "INSERT INTO usage_log(ts, key_id, key_name, kind, model, status, images, rid) VALUES ({}.0, {kid}, 'k{kid}', '{kind}', '{model}', 'ok', 1, 'ab{:02x}');",
            1000 + i,
            i
        ));
    }
    a.sql(&sql).await;
    let r = a.get(&format!("{A}/logs")).await;
    assert_eq!(r.status, 200, "{}", r.text());
    let v = r.json();
    assert_eq!(
        (
            v["page"].clone(),
            v["per_page"].clone(),
            v["total"].clone(),
            v["pages"].clone()
        ),
        (json!(1), json!(20), json!(45), json!(3))
    );
    assert_eq!(v["logs"].as_array().unwrap().len(), 20);
    assert_eq!(v["logs"][0]["id"], 45, "新的在前");
    let cols: Vec<&str> = v["logs"][0]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        &cols[..6],
        ["id", "ts", "key_id", "key_name", "kind", "model"]
    );
    // 原因短说明和原因下拉选项由服务端给
    assert!(v["logs"][0]["reason_label"].is_string());
    let reasons = v["reasons"].as_array().unwrap();
    assert_eq!(reasons.len(), gate_core::reasons::LABELS.len());
    assert_eq!(
        reasons[0],
        json!({"code": gate_core::reasons::LABELS[0].0, "label": gate_core::reasons::LABELS[0].1})
    );
    // 页码夹取：越界回最后一页（5 条），0 / 负数回第一页
    assert_eq!(
        a.get(&format!("{A}/logs?page=99")).await.json()["logs"]
            .as_array()
            .unwrap()
            .len(),
        5
    );
    assert_eq!(a.get(&format!("{A}/logs?page=-3")).await.json()["page"], 1);
    // 每页条数只认 20 / 50 / 100
    assert_eq!(
        a.get(&format!("{A}/logs?per_page=50")).await.json()["pages"],
        1
    );
    assert_eq!(
        a.get(&format!("{A}/logs?per_page=7")).await.json()["per_page"],
        20
    );
    // 筛选
    assert_eq!(
        a.get(&format!("{A}/logs?key_id=2")).await.json()["total"],
        15
    );
    assert_eq!(
        a.get(&format!("{A}/logs?key_id=0")).await.json()["total"],
        45,
        "0 = 不筛"
    );
    assert_eq!(
        a.get(&format!("{A}/logs?feature=upscale")).await.json()["total"],
        15
    );
    assert_eq!(
        a.get(&format!("{A}/logs?feature=image&model_version=v45"))
            .await
            .json()["total"],
        8
    );
    assert_eq!(
        a.get(&format!("{A}/logs?feature=image&model_version=v5"))
            .await
            .json()["total"],
        7
    );
    // 文本生成 / 语音已删：不能再按它们筛选；旧的 chat / text 记录仍在全部列表里
    for f in ["text", "voice"] {
        a.get(&format!("{A}/logs?feature={f}"))
            .await
            .assert_detail(422, "未知功能");
    }
    assert_eq!(
        a.get(&format!("{A}/logs?feature=")).await.json()["total"],
        45
    );
    assert_eq!(
        a.get(&format!("{A}/logs?rid=%20AB0F%2Czz%20")).await.json()["total"],
        1,
        "rid 只留十六进制字符"
    );
    assert_eq!(
        a.get(&format!("{A}/logs?rid=zz")).await.json()["total"],
        45,
        "过滤后为空 = 不筛"
    );
    assert_eq!(
        v["summary"],
        json!({"total": 45, "ok": 45, "rejected": 0, "error": 0})
    );
    // 状态 / 来源 IP / 原因码 / 多把 Key；行里带 UA（client）
    a.sql(
        "INSERT INTO usage_log(ts, key_id, key_name, kind, model, status, reason, src, client, token_id) VALUES \
         (2000.0, 1, 'k1', 'image', 'm', 'rejected', 'queue_full', '203.0.113.9', 'axios/1.7', NULL), \
         (2001.0, 2, 'k2', 'image', 'm', 'rejected', 'key_rate', '203.0.113.9', 'curl/8', NULL), \
         (2002.0, 3, 'k3', 'image', 'm', 'error', '', '198.51.100.4', 'Mozilla/5.0', 'tok-a');",
    )
    .await;
    let v = a.get(&format!("{A}/logs?status=rejected")).await.json();
    assert_eq!(v["total"], 2);
    assert_eq!(
        v["summary"],
        json!({"total": 48, "ok": 45, "rejected": 2, "error": 1}),
        "summary 不受 status 自己影响"
    );
    assert_eq!(v["logs"][0]["client"], "curl/8");
    assert_eq!(v["logs"][0]["src"], "203.0.113.9");
    assert_eq!(
        a.get(&format!("{A}/logs?reason=queue_full")).await.json()["total"],
        1
    );
    let v = a.get(&format!("{A}/logs?status=error")).await.json();
    assert_eq!(
        (v["total"].clone(), v["logs"][0]["token_id"].clone()),
        (json!(1), json!("tok-a"))
    );
    assert_eq!(
        a.get(&format!("{A}/logs?key_id=1,3&status=rejected"))
            .await
            .json()["total"],
        1
    );
    assert_eq!(
        a.get(&format!("{A}/logs?key_id=1,2")).await.json()["total"],
        47
    );
    for (q, msg) in [
        ("status=oops", "未知状态"),
        ("model_version=v3", "未知文生图版本"),
        ("feature=upscale&model_version=v5", "未知文生图版本"),
        ("reason=Queue-Full", "未知原因码"),
        ("key_id=1,x", "Key 编号无效"),
        ("key_id=1,", "Key 编号无效"),
    ] {
        a.get(&format!("{A}/logs?{q}"))
            .await
            .assert_detail(422, msg);
    }
    // 校验
    a.get(&format!("{A}/logs?feature=nope"))
        .await
        .assert_detail(422, "未知功能");
    a.get(&format!("{A}/logs?key_id=a&page=b"))
        .await
        .assert_detail(422, "key_id 必须是整数");
    // 只认规范写法：小数、下划线都不接受
    for q in ["page=1.0", "page=1_0"] {
        assert_eq!(a.get(&format!("{A}/logs?{q}")).await.status, 422, "{q}");
    }
    drop(e);
}

#[tokio::test]
async fn admin_actions_filters() {
    let (_e, a) = ready().await;
    a.sql(
        "INSERT INTO admin_actions(ts, actor, action, target, detail, ok) VALUES \
         (1.0, '站长', '添加黑名单', '', '', 1), (2.0, '系统', '私信成员', '', '', 1), \
         (3.0, 'Discord:900000000000000001', '成员自助重置', '', '', 1), (4.0, '后台 *.*.0.1', '重置今日额度', '', '', 0);",
    )
    .await;
    let all = a.get(&format!("{A}/actions")).await.json();
    assert_eq!(
        (
            all["per_page"].clone(),
            all["total"].clone(),
            all["pages"].clone()
        ),
        (json!(30), json!(5), json!(1))
    );
    assert_eq!(
        all["action_kinds"],
        json!([
            "成员自助重置",
            "添加黑名单",
            "登录后台",
            "私信成员",
            "重置今日额度"
        ]),
        "按字节序去重"
    );
    let names = |v: Value| -> Vec<String> {
        v["actions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["action"].as_str().unwrap().to_string())
            .collect()
    };
    assert_eq!(
        names(a.get(&format!("{A}/actions?actor=admin")).await.json()),
        ["重置今日额度", "添加黑名单", "登录后台"]
    );
    assert_eq!(
        names(a.get(&format!("{A}/actions?actor=system")).await.json()),
        ["私信成员"]
    );
    assert_eq!(
        names(a.get(&format!("{A}/actions?actor=member")).await.json()),
        ["成员自助重置"]
    );
    assert_eq!(
        names(a.get(&format!("{A}/actions?actor=zzz")).await.json()).len(),
        5,
        "未知类别不筛"
    );
    let q = "action=%E6%B7%BB%E5%8A%A0%E9%BB%91%E5%90%8D%E5%8D%95%7C%20%E7%A7%81%E4%BF%A1%E6%88%90%E5%91%98%20%7C%7C";
    assert_eq!(
        names(a.get(&format!("{A}/actions?{q}")).await.json()),
        ["私信成员", "添加黑名单"],
        "| 分隔多个操作名"
    );
    assert_eq!(
        a.get(&format!("{A}/actions?page=50")).await.json()["page"],
        1
    );
    let first = &all["actions"][0];
    assert_eq!(first["action"], "重置今日额度");
    a.get(&format!("{A}/actions?page=q"))
        .await
        .assert_detail(422, "page 必须是整数");
}

#[tokio::test]
async fn bug_tracker_lists_resolves_and_names_keys() {
    let (_e, a) = ready().await;
    // 10-27 起 Bug 追踪整个删除：列表 / 标记接口都不在了
    assert_eq!(a.get(&format!("{A}/errors")).await.status, 404);
    assert_eq!(
        a.post(&format!("{A}/errors/resolve-all"), json!({}))
            .await
            .status,
        405
    );
}

#[tokio::test]
async fn donations_are_gone() {
    let (_e, a) = ready().await;
    a.get(&format!("{A}/donations"))
        .await
        .assert_detail(404, "Not Found");
}

#[tokio::test]
async fn reset_all_keys_disables_everyone_including_invites() {
    let (e, a) = ready().await;
    a.sql(
        "INSERT INTO api_keys(id, name, token, created_at) VALUES (2, '普通号2', 'nai-t-2', 1.0);\
         INSERT INTO api_keys(id, name, token, created_at) VALUES (3, '邀请号', 'nai-t-3', 1.0), (4, '普通号', 'nai-t-4', 1.0);\
         INSERT INTO key_tags(key_id, tag, note, by, ts) VALUES (3, '邀请', '', '站长', 1.0), (3, '观察', '', '站长', 1.0), (4, '观察', '', '站长', 1.0), (2, '常驻', '', '站长', 1.0);\
         INSERT INTO discord_registrations(discord_id, key_id, created_at) VALUES ('900000000000000003', 3, 1.0), ('900000000000000004', 4, 1.0), ('900000000000000001', 1, 1.0);\
         INSERT INTO share_state(key_id, paused_until) VALUES (4, 9.0);",
    )
    .await;
    a.post(&format!("{A}/ops/reset-all-keys"), json!({}))
        .await
        .assert_detail(400, "安全确认未通过：必须传入 confirm='确认重置'");
    a.post(
        &format!("{A}/ops/reset-all-keys"),
        json!({"confirm": "确认"}),
    )
    .await
    .assert_detail(400, "安全确认未通过：必须传入 confirm='确认重置'");
    let r = a
        .post(
            &format!("{A}/ops/reset-all-keys"),
            json!({"confirm": "确认重置"}),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(
        r.json(),
        json!({"ok": true, "disabled_count": 4, "message": "已失效 4 把 Key 并解除 Discord 绑定。"})
    );
    let enabled = query(&e.state.db, "SELECT id, enabled FROM api_keys ORDER BY id").await;
    assert_eq!(
        enabled,
        [
            json!({"id": 1, "enabled": 0}),
            json!({"id": 2, "enabled": 0}),
            json!({"id": 3, "enabled": 0}),
            json!({"id": 4, "enabled": 0})
        ]
    );
    assert_eq!(
        query(&e.state.db, "SELECT discord_id FROM discord_registrations").await,
        [] as [Value; 0]
    );
    let tags = query(
        &e.state.db,
        "SELECT key_id, tag FROM key_tags ORDER BY key_id, tag",
    )
    .await;
    assert!(tags.is_empty(), "所有凭据标签一起清除");
    assert!(
        query(&e.state.db, "SELECT 1 FROM share_state")
            .await
            .is_empty()
    );
    // 成功那次是处理函数自己记的日志（站长 / 全站重置 Key），中间件不重复记；确认没过的两次记失败
    let acts = a.actions().await;
    assert_eq!(
        acts.iter()
            .filter(|(act, _, _, ok)| act == "全站杀头重置" && !*ok)
            .count(),
        2,
        "{acts:?}"
    );
    assert_eq!(
        acts.iter()
            .filter(|(act, _, _, ok)| act == "全站杀头重置" && *ok)
            .count(),
        0,
        "{acts:?}"
    );
    assert!(acts.iter().any(|(act, t, d, ok)| act == "全站重置 Key"
        && t == "全部 Key"
        && d == "已失效 4 把 Key 并清空 Discord 绑定"
        && *ok));
}

#[tokio::test]
async fn creative_content_records_are_removed() {
    let (e, a) = ready().await;
    for path in [
        "/prompts",
        "/audit",
        "/audit/1/image",
        "/audit/1/thumb",
        "/audit/export?key_id=1",
        "/ops/audit",
    ] {
        assert_eq!(a.get(&format!("{A}{path}")).await.status, 404, "{path}");
    }
    gate_server::jobs::purge(&e.state).await.unwrap();
    let rows = query(
        &e.state.db,
        "SELECT name FROM sqlite_master WHERE name IN ('generation_audit','audit_request_links')",
    )
    .await;
    assert!(rows.is_empty());
}
