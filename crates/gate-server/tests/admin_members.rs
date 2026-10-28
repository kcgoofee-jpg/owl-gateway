//! 后台「成员」接口：Key / 成员 / 黑名单 / 券 / 标签 / 奖励。
//!
//! 逐字节快照见 `admin_snapshot.rs`；这里按接口写成可读的用例：
//! 鉴权、校验文案、返回结构、写进库的行、操作日志，注册服务（真实的 RegistrationService + 本机假 Discord）和动态额度占位内核。

mod admin_util;
mod common;
use admin_util::MockDiscord;
use common::{Admin, Env, TOKEN, mk_key, query, ready, send, setup_registration};
use serde_json::{Value, json};

const A: &str = "/admin/api";

/// 配齐自助领取的网关：真实的 `RegistrationService`，Discord REST 指向本机假服务。
async fn ready_reg() -> (Env, Admin, MockDiscord) {
    let d = MockDiscord::start().await;
    let e = setup_registration(&d.url(), &[]).await;
    let a = Admin::login(&e.state).await;
    (e, a, d)
}

/// 机器人不私信成员：假 Discord 上不能出现「开私信频道」或「发消息」的请求。
fn assert_no_dm(d: &MockDiscord) {
    let dm: Vec<_> = d
        .calls()
        .into_iter()
        .filter(|c| {
            let path = c[1].as_str().unwrap_or("");
            path == "/users/@me/channels" || path.ends_with("/messages")
        })
        .collect();
    assert!(dm.is_empty(), "不该有私信请求：{dm:?}");
}

#[tokio::test]
async fn bad_path_id_is_checked_after_login() {
    let (e, a) = ready().await;
    // 先鉴权再校验参数：没登录的人拿到的是 401，登录后才是 422
    send(
        &e.state,
        "GET",
        &format!("{A}/keys/abc/coupons"),
        None,
        None,
        &[],
    )
    .await
    .assert_detail(401, "未登录或会话已过期");
    a.get(&format!("{A}/keys/abc/coupons"))
        .await
        .assert_detail(422, "key_id 必须是整数");
}

#[tokio::test]
async fn keys_list_shape_invitation_source_and_counters() {
    let (e, a) = ready().await;
    let id2 = mk_key(&e.state, "邀请-7", "nai-t-2").await;
    e.state
        .db
        .upsert_key_tag(id2, "邀请", "", "synthetic", e.state.now())
        .await
        .unwrap();
    mk_key(&e.state, "普通-3", "nai-t-3").await;
    a.sql(&format!(
        "INSERT INTO counters(key_id, day, images, anlas, text_tokens, requests, v5, legacy_free_images) \
         VALUES (1, '{}', 3, 1.239, 7, 4, 2, 1);\
         INSERT INTO usage_log(ts, key_id, key_name, kind, model, status, images) VALUES (1.0, 1, 'x', 'image', 'm', 'ok', 4), (2.0, 1, 'x', 'image_stream', 'm', 'ok', 2), (3.0, 1, 'x', 'chat', 'm', 'ok', 9);",
        e.state.day()
    ))
    .await;
    let r = a.get(&format!("{A}/keys")).await;
    assert_eq!(r.status, 200, "{}", r.text());
    let v = r.json();
    let keys = v["keys"].as_array().unwrap();
    assert_eq!(
        keys.iter()
            .map(|k| k["name"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["普通-3", "邀请-7", "测试成员"],
        "id 倒序"
    );
    let k = &keys[2];
    assert_eq!(k["token"], TOKEN);
    assert_eq!(k["policy"]["effective"]["daily_images"], 100);
    for removed in [
        "daily_images",
        "quota_auto",
        "quota_limits",
        "features",
        "image_model_scope",
        "exclude_global_v5",
    ] {
        assert!(k.get(removed).is_none(), "{k}");
    }
    assert_eq!(
        k["used"],
        json!({"images": 3, "generated_images_total": 6, "legacy_free_images": 1, "anlas": 1.24, "v5": 2,
               "requests": 4}),
        "不再返回文本 token 计数"
    );
    assert_eq!(keys[1]["invited"], true);
    assert_eq!(keys[0]["invited"], false);
    assert!(k.get("role").is_none() && k.get("role_label").is_none());
}

#[tokio::test]
async fn invite_keys_are_numbered_and_tagged() {
    let (e, a) = ready().await;
    mk_key(&e.state, "邀请-4 的朋友", "nai-t-2").await;
    mk_key(&e.state, "邀请-x", "nai-t-3").await;
    let r = a.post(&format!("{A}/keys/invite"), json!({})).await;
    assert_eq!(r.status, 200, "{}", r.text());
    let key = &r.json()["key"];
    assert_eq!(key["name"], "邀请-5");
    assert_eq!(key["policy"]["effective"]["daily_images"], 150);
    assert_eq!(key["policy"]["effective"]["v5"], false);
    assert!(key["expires_at"].is_null());
    let id = key["id"].as_i64().unwrap();
    let tags = query(
        &e.state.db,
        &format!("SELECT tag, by FROM key_tags WHERE key_id={id}"),
    )
    .await;
    assert_eq!(tags, [json!({"tag": "邀请", "by": "站长"})]);
    let r2 = a.post(&format!("{A}/keys/invite"), json!({})).await;
    assert_eq!(r2.json()["key"]["name"], "邀请-6");
    // 列表里认出「邀请」身份组；操作日志只记处理函数那一条可读的（通用包装不再重复记原样请求体）
    let l = a.get(&format!("{A}/keys")).await.json();
    assert_eq!(l["keys"][0]["invited"], true);
    let acts = a.actions().await;
    assert!(
        acts.iter()
            .any(|(act, t, ..)| act == "新建邀请 Key" && t == "邀请-5")
    );
    assert!(!acts.iter().any(|(act, ..)| act == "POST /keys/invite"));
}

#[tokio::test]
async fn regenerate_replaces_the_key_immediately() {
    let (e, a) = ready().await;
    let url = format!("{A}/keys/1/regenerate");
    a.post(&format!("{A}/keys/999/regenerate"), json!({}))
        .await
        .assert_detail(404, "key 不存在");
    let r = a.req("POST", &url, None).await;
    assert_eq!(r.status, 200);
    assert_eq!(r.headers["cache-control"], "no-store");
    let token = r.json()["token"].as_str().unwrap().to_string();
    assert_ne!(token, TOKEN);
    assert_eq!(
        e.state.db.get_key(1).await.unwrap().unwrap()["token"],
        token.as_str()
    );
    assert!(
        e.state.db.get_key_by_token(TOKEN).await.unwrap().is_none(),
        "旧 Key 立即失效"
    );
    let acts = a.actions().await;
    assert!(
        acts.iter()
            .any(|(act, t, ..)| act == "重新生成 Key" && t == "Key #1 测试成员")
    );
    assert!(
        acts.iter().all(|(act, ..)| act != "私信成员"),
        "没有注册服务：不私信、也不记私信日志"
    );
}

#[tokio::test]
async fn regenerate_shows_the_new_key_and_never_dms() {
    let (_e, a, d) = ready_reg().await;
    let url = format!("{A}/keys/1/regenerate");
    a.post(
        &format!("{A}/keys/1/bind-discord"),
        json!({"discord_id": "900000000000000001", "username": "u1"}),
    )
    .await;
    // 绑了 Discord 的成员：新 Key 只在响应里给站长（手动转交），机器人不私信；旧的 `notify` 参数被忽略、不报错
    for suffix in ["", "?notify=1", "?notify=0"] {
        let r = a.req("POST", &format!("{url}{suffix}"), None).await;
        assert_eq!(r.status, 200, "{suffix}: {}", r.text());
        assert!(r.json()["token"].as_str().unwrap().starts_with("nai-"));
        assert_eq!(r.headers["cache-control"], "no-store");
    }
    assert_no_dm(&d);
    let acts = a.actions().await;
    assert!(
        acts.iter().all(|(act, ..)| act != "私信成员"),
        "不再有「私信成员」操作记录：{acts:?}"
    );
    assert!(
        acts.iter().any(|(act, ..)| act == "重新生成 Key"),
        "{acts:?}"
    );
}

#[tokio::test]
async fn credential_metadata_is_strict_and_does_not_change_policy() {
    let (e, a) = ready().await;
    let url = format!("{A}/keys/1");
    let before = e.state.db.member_policy(common::SUBJECT).await.unwrap();
    for value in [
        json!({"daily_images":200}),
        json!({"role":"invite"}),
        json!({"quota_limits":{}}),
        json!({"notify":true}),
    ] {
        assert_eq!(a.patch(&url, value).await.status, 422);
    }
    for value in [
        json!({"name":""}),
        json!({"enabled":1}),
        json!({"expires_at":-1}),
    ] {
        assert_eq!(a.patch(&url, value).await.status, 422);
    }
    let expires = e.state.now() + 86400.0;
    let r = a
        .patch(
            &url,
            json!({"name":"改名","enabled":false,"expires_at":expires}),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(r.json()["key"]["name"], "改名");
    assert_eq!(r.json()["key"]["enabled"], false);
    assert_eq!(r.json()["key"]["expires_at"], expires);
    assert_eq!(
        serde_json::to_value(e.state.db.member_policy(common::SUBJECT).await.unwrap()).unwrap(),
        serde_json::to_value(before).unwrap()
    );
    assert!(a.patch(&url, json!({"expires_at":null})).await.json()["key"]["expires_at"].is_null());
}

#[tokio::test]
async fn removed_manual_creation_binding_and_role_routes_are_unavailable() {
    let (_e, a, d) = ready_reg().await;
    for path in ["/keys", "/keys/1/role", "/keys/1/bind-discord"] {
        assert_eq!(
            a.post(&format!("{A}{path}"), json!({})).await.status,
            405,
            "{path}"
        );
    }
    assert_no_dm(&d);
}

#[tokio::test]
async fn reset_quota_and_tags_do_not_change_member_policy() {
    let (e, a) = ready().await;
    // 重置今日额度：基线 = 当前累计；返回的 key 里 used 不再包含已重置的 V5 / Anlas
    a.post(&format!("{A}/keys/999/reset-daily-image-quota"), json!({}))
        .await
        .assert_detail(404, "key 不存在");
    a.sql(&format!(
        "INSERT INTO counters(key_id, day, images, anlas, text_tokens, requests, v5) VALUES (1, '{}', 5, 3.5, 0, 5, 4)",
        e.state.day()
    ))
    .await;
    let r = a
        .post(
            &format!("{A}/keys/1/reset-daily-image-quota"),
            json!({"scope": ["v5", "anlas"]}),
        )
        .await;
    assert_eq!(r.status, 200);
    assert_eq!(
        (
            r.json()["ok"].clone(),
            r.json()["key"]["used"]["v5"].clone(),
            r.json()["key"]["used"]["anlas"].clone()
        ),
        (json!(true), json!(0), json!(0.0))
    );
    let off = query(
        &e.state.db,
        "SELECT anlas, v5 FROM daily_quota_offsets WHERE key_id=1",
    )
    .await;
    assert_eq!(off, [json!({"anlas": 3.5, "v5": 4})]);
    // 不带范围（默认，站长 10-18）只重置 V4.5：之后新用的 V5 / Anlas 不被清掉，基线不动
    a.sql(&format!(
        "UPDATE counters SET v5=v5+2, anlas=anlas+1.0 WHERE key_id=1 AND day='{}'",
        e.state.day()
    ))
    .await;
    let r = a
        .post(&format!("{A}/keys/1/reset-daily-image-quota"), json!({}))
        .await;
    assert_eq!(r.status, 200);
    assert_eq!(
        (
            r.json()["key"]["used"]["v5"].clone(),
            r.json()["key"]["used"]["anlas"].clone()
        ),
        (json!(2), json!(1.0))
    );
    let off = query(
        &e.state.db,
        "SELECT anlas, v5 FROM daily_quota_offsets WHERE key_id=1",
    )
    .await;
    assert_eq!(off, [json!({"anlas": 3.5, "v5": 4})]);

    // 标签
    let t = format!("{A}/keys/1/tags");
    a.put(&format!("{A}/keys/999/tags"), json!({"tag": "x"}))
        .await
        .assert_detail(404, "key 不存在");
    a.put(&t, json!({"tag": "  "}))
        .await
        .assert_detail(422, "标签不能为空");
    let r = a
        .put(
            &t,
            json!({"tag": "多  空白\t标签很长很长很长很长", "note": "  备注 "}),
        )
        .await;
    assert_eq!(
        r.json(),
        json!({"ok": true, "tag": "多 空白 标签很长很长很"})
    );
    a.put(
        &t,
        json!({"tag": "多 空白 标签很长很长很", "note": "新备注", "by": "运维"}),
    )
    .await;
    let tags = query(
        &e.state.db,
        "SELECT tag, note, by FROM key_tags WHERE key_id=1",
    )
    .await;
    assert_eq!(
        tags,
        [json!({"tag": "多 空白 标签很长很长很", "note": "新备注", "by": "运维"})]
    );
    let del = format!(
        "{t}/{}",
        "%E5%A4%9A%20%E7%A9%BA%E7%99%BD%20%E6%A0%87%E7%AD%BE%E5%BE%88%E9%95%BF%E5%BE%88%E9%95%BF%E5%BE%88"
    );
    assert_eq!(a.delete(&del).await.json(), json!({"ok": true}));
    a.delete(&del).await.assert_detail(404, "没有这个标签");
}

#[tokio::test]
async fn coupons_list_and_issue() {
    let (e, a) = ready().await;
    let c = format!("{A}/keys/1/coupons");
    assert_eq!(a.get(&c).await.json(), json!({"coupons": []}));
    for (body, detail) in [
        (json!({"kind": "gold"}), "目前只有重置券（reset）"),
        (json!({"days": 0}), "有效期 days 必须在 1～30 天之间"),
        (json!({"days": 31}), "有效期 days 必须在 1～30 天之间"),
        (json!({"days": "7"}), "有效期 days 必须在 1～30 天之间"),
        (json!({"days": true}), "有效期 days 必须在 1～30 天之间"),
    ] {
        a.post(&c, body).await.assert_detail(422, detail);
    }
    a.post(&format!("{A}/keys/999/coupons"), json!({}))
        .await
        .assert_detail(404, "key 不存在");
    let r = a
        .post(
            &c,
            json!({"days": 12.5, "note": "唱得真好听", "by": "奶妹"}),
        )
        .await;
    let coupon = r.json()["coupon"].clone();
    assert_eq!(coupon["key_id"], 1);
    assert_eq!(coupon["kind"], "reset");
    assert_eq!(coupon["used_at"], Value::Null);
    let span = coupon["expires_at"].as_f64().unwrap() - coupon["created_at"].as_f64().unwrap();
    assert!((span - 12.5 * 86400.0).abs() < 0.01, "{span}");
    let r = a.post(&c, json!({})).await;
    assert_eq!(r.json()["coupon"]["note"], "");
    let list = a.get(&c).await.json();
    assert_eq!(list["coupons"].as_array().unwrap().len(), 2);
    assert_eq!(
        list["coupons"][0]["by"], "站长",
        "按过期时间升序：默认 7 天的在前"
    );
    assert_eq!(
        query(&e.state.db, "SELECT COUNT(*) AS n FROM coupons").await,
        [json!({"n": 2})]
    );
}

#[tokio::test]
async fn delete_key_without_registration_service() {
    let (e, a) = ready().await;
    a.delete(&format!("{A}/keys/999"))
        .await
        .assert_detail(404, "key 不存在");
    a.delete(&format!("{A}/keys/1?ban=zz"))
        .await
        .assert_detail(422, "ban 必须是布尔值");
    // 直接删（带标签一起清），ban 参数被忽略
    let k2 = mk_key(&e.state, "待删", "nai-t-2").await;
    a.put(&format!("{A}/keys/{k2}/tags"), json!({"tag": "x"}))
        .await;
    let r = a.delete(&format!("{A}/keys/{k2}?ban=true")).await;
    assert_eq!(r.json(), json!({"ok": true}));
    assert!(e.state.db.get_key(k2).await.unwrap().is_none());
    assert!(
        query(
            &e.state.db,
            &format!("SELECT 1 FROM key_tags WHERE key_id={k2}")
        )
        .await
        .is_empty()
    );
    assert!(
        query(&e.state.db, "SELECT 1 FROM discord_bans")
            .await
            .is_empty()
    );
    // ?ban=true 的删除，操作名换成「删除 Key 并永久禁止领取」（没有注册服务时 ban 其实没生效，旧版如此）
    let acts = a.actions().await;
    assert!(
        acts.iter()
            .any(|(act, t, ..)| act == "删除 Key 并永久禁止领取" && t == &format!("Key #{k2} 待删")),
        "{acts:?}"
    );
    let k3 = mk_key(&e.state, "丙", "nai-t-3").await;
    assert_eq!(
        a.delete(&format!("{A}/keys/{k3}")).await.json(),
        json!({"ok": true})
    );
    assert!(a.actions().await.iter().any(|(act, ..)| act == "删除 Key"));
}

#[tokio::test]
async fn delete_key_revokes_or_bans_through_the_registration_service() {
    let (e, a, d) = ready_reg().await;
    let k3 = mk_key(&e.state, "甲", "nai-t-3").await;
    let k4 = mk_key(&e.state, "乙", "nai-t-4").await;
    a.sql(&format!("UPDATE api_keys SET discord_id='900000000000000003' WHERE id={k3}; INSERT INTO discord_registrations(discord_id,key_id,created_at,username,display_name) VALUES('900000000000000003',{k3},1.0,'','');")).await;
    a.sql(&format!("UPDATE api_keys SET discord_id='900000000000000004' WHERE id={k4}; INSERT INTO discord_registrations(discord_id,key_id,created_at,username,display_name) VALUES('900000000000000004',{k4},1.0,'u4','');")).await;
    d.clear(); // 绑定时会读一次 Discord 资料，下面只看删除发出的请求
    // 撤销：删 Key 和领取记录；旧的 `notify` 参数被忽略
    let r = a.delete(&format!("{A}/keys/{k3}?notify=0")).await;
    assert_eq!(
        r.json(),
        json!({"ok": true, "discord_id": "900000000000000003", "banned": false})
    );
    assert!(e.state.db.get_key(k3).await.unwrap().is_none());
    assert!(
        e.state
            .db
            .registration_key_id("900000000000000003")
            .await
            .unwrap()
            .is_none()
    );
    assert!(d.calls().is_empty());
    // 封禁：写封禁表（带上领取记录里的资料）、撤销 Key；不私信
    let r = a
        .delete(&format!("{A}/keys/{k4}?ban=true&reason=%E5%88%B7%E9%87%8F"))
        .await;
    assert_eq!(
        r.json(),
        json!({"ok": true, "discord_id": "900000000000000004", "banned": true})
    );
    assert_no_dm(&d);
    let bans = query(
        &e.state.db,
        "SELECT discord_id, reason, username, banned_by FROM discord_bans",
    )
    .await;
    assert_eq!(
        bans,
        [
            json!({"discord_id": "900000000000000004", "reason": "刷量", "username": "u4", "banned_by": "站长"})
        ]
    );
    assert!(e.state.db.get_key(k4).await.unwrap().is_none());
    // 默认原因
    let k5 = mk_key(&e.state, "丙", "nai-t-5").await;
    a.sql(&format!("UPDATE api_keys SET discord_id='900000000000000005' WHERE id={k5}; INSERT INTO discord_registrations(discord_id,key_id,created_at) VALUES('900000000000000005',{k5},1.0);")).await;
    a.delete(&format!("{A}/keys/{k5}?ban=true&notify=0")).await;
    let reason = query(
        &e.state.db,
        "SELECT reason FROM discord_bans WHERE discord_id='900000000000000005'",
    )
    .await;
    assert_eq!(reason, [json!({"reason": "站长手动删除并封禁"})]);
    // 没有 Discord 绑定的 Key：不走注册服务
    let k6 = mk_key(&e.state, "丁", "nai-t-6").await;
    assert_eq!(
        a.delete(&format!("{A}/keys/{k6}")).await.json(),
        json!({"ok": true})
    );
    assert_no_dm(&d);
}

#[tokio::test]
async fn bans_list_add_remove() {
    let (e, a) = ready().await;
    a.post(&format!("{A}/bans"), json!({"discord_id": "abc"}))
        .await
        .assert_detail(422, "Discord ID 必须为纯数字");
    a.post(&format!("{A}/bans"), json!({}))
        .await
        .assert_detail(422, "Discord ID 必须为纯数字");
    let r = a
        .post(
            &format!("{A}/bans"),
            json!({"discord_id": " 900000000000000010 ", "reason": "  刷量 "}),
        )
        .await;
    assert_eq!(
        r.json(),
        json!({"ok": true, "discord_id": "900000000000000010"})
    );
    a.post(
        &format!("{A}/bans"),
        json!({"discord_id": "900000000000000011"}),
    )
    .await;
    let l = a.get(&format!("{A}/bans")).await.json();
    assert_eq!(l["total"], 2);
    assert_eq!(l["bans"][1]["reason"], "刷量");
    assert_eq!(l["bans"][0]["reason"], "站长手动封禁");
    assert_eq!(l["bans"][0]["banned_by"], "站长");
    assert_eq!(
        a.get(&format!("{A}/bans?q=%E5%88%B7")).await.json()["total"],
        1
    );
    assert_eq!(
        a.get(&format!("{A}/bans?q=zzz")).await.json(),
        json!({"bans": [], "total": 0})
    );
    a.delete(&format!("{A}/bans/900000000000000099"))
        .await
        .assert_detail(404, "该账号不在黑名单中");
    let r = a.delete(&format!("{A}/bans/900000000000000010")).await;
    assert_eq!(
        r.json(),
        json!({"ok": true, "discord_id": "900000000000000010"})
    );
    assert_eq!(
        query(&e.state.db, "SELECT COUNT(*) AS n FROM discord_bans").await,
        [json!({"n": 1})]
    );
    let acts = a.actions().await;
    assert!(acts.iter().any(|(act, t, d, _)| act == "添加黑名单"
        && t == "Discord:900000000000000010"
        && d == "刷量"));
    assert!(
        acts.iter()
            .any(|(act, _, d, _)| act == "移出黑名单" && d == "已解除封禁，允许重新领取 Key")
    );
}

#[tokio::test]
async fn banning_a_registered_account_also_revokes_its_key() {
    let (e, a, _d) = ready_reg().await;
    let k = mk_key(&e.state, "甲", "nai-t-2").await;
    a.sql(&format!("UPDATE api_keys SET discord_id='900000000000000012' WHERE id={k}; INSERT INTO discord_registrations(discord_id,key_id,created_at,username,display_name) VALUES('900000000000000012',{k},1.0,'u12','十二');")).await;
    a.post(
        &format!("{A}/bans"),
        json!({"discord_id": "900000000000000012", "reason": "x"}),
    )
    .await;
    assert!(e.state.db.get_key(k).await.unwrap().is_none());
    let bans = query(
        &e.state.db,
        "SELECT reason, username, display_name, banned_by FROM discord_bans",
    )
    .await;
    assert_eq!(
        bans,
        [json!({"reason": "x", "username": "u12", "display_name": "十二", "banned_by": "站长"})]
    );
}

#[tokio::test]
async fn members_view_shape_and_dropped_fields() {
    let (e, a) = ready().await;
    let k2 = mk_key(&e.state, "乙", "nai-t-2").await;
    a.sql(&format!("UPDATE api_keys SET discord_id='900000000000000002' WHERE id={k2}; INSERT INTO discord_registrations(discord_id,key_id,created_at,username,display_name) VALUES('900000000000000002',{k2},1.0,'u2','');")).await;
    a.put(
        &format!("{A}/keys/{k2}/tags"),
        json!({"tag": "观察", "note": "n"}),
    )
    .await;
    a.sql(&format!(
        "INSERT INTO usage_log(ts, key_id, key_name, kind, model, status, images, detail, client) VALUES ({0}, {k2}, '乙', 'image', 'm', 'rejected', 0, '请求过于频繁（每把 Key 每分钟最多 10 次）', ''), ({0}, {k2}, '乙', 'image', 'm', 'ok', 1, '', '柏宝绘');\
         INSERT INTO share_state(key_id, paused_until, pause_reason) VALUES ({k2}, {1}, '');",
        e.state.now() - 600.0,
        e.state.now() + 3600.0
    ))
    .await;
    let r = a.get(&format!("{A}/members")).await;
    assert_eq!(r.status, 200, "{}", r.text());
    let v = r.json();
    assert_eq!(v["inactivity_days"], 3);
    // 裁剪前的成员行数与「需要处理」的行数：默认 all 时 members 本身不改
    assert_eq!(
        (
            v["total"].clone(),
            v["attention"].clone(),
            v["filter"].clone()
        ),
        (json!(2), json!(1), json!("all")),
        "乙被限流暂停（bad）要处理；默认 filter=all 保持旧行为"
    );
    let m = &v["members"];
    // 没有 Discord 的在前，有 Discord 的在后
    assert_eq!(m[0]["name"], "测试成员");
    assert_eq!(m[1]["name"], "乙");
    assert_eq!(m[1]["policy"]["effective"]["daily_images"], 150);
    for removed in [
        "quota_auto",
        "quota_limits",
        "image_model_scope",
        "allow_anlas",
        "daily_images",
    ] {
        assert!(m[1].get(removed).is_none(), "{}", m[1]);
    }
    assert_eq!(m[1]["discord_id"], "900000000000000002");
    assert_eq!(m[1]["tags"][0]["tag"], "观察");
    assert_eq!(m[1]["auto_tags"], json!([{"tag": "请求过快", "count": 1}]));
    assert_eq!(m[1]["rejected_24h"], 1);
    assert_eq!(
        m[1]["share"]["paused_until"]
            .as_f64()
            .map(|p| p > e.state.now()),
        Some(true)
    );
    assert_eq!(
        (m[1]["key_count"].clone(), m[1]["all_key_ids"].clone()),
        (json!(1), json!([k2]))
    );
    assert!(
        m[0].get("hours24").is_none(),
        "hours24 是前端 0 读取的重字段（每轮 13.9KB），已从成员列表删掉"
    );
    assert_eq!(m[0]["share"], Value::Null);
}

/// `?filter=attention`：只回需要处理的行，`total`/`attention`/`filter` 三个键给出计数；
/// 缺省（不带 filter）与 `filter=all` 保持旧行为（全量）。口径与后台前端 `needsAttention` 一致：
/// 暂停中 / 已过期是 bad（点过「没问题」也豁免不了）；3 天内到期 / 疑似标签 / 今日重置 ≥2 是 need
/// （「已查看」24 小时内不算）；停用的 Key 不计入；「请求过快」自动标签不升级为要处理。
#[tokio::test]
async fn members_filter_attention_trims_rows_and_counts() {
    let (e, a) = ready().await;
    let now = e.state.now();
    let day = e.state.day();
    let k2 = mk_key(&e.state, "快到期", "nai-t-2").await;
    let k3 = mk_key(&e.state, "已过期", "nai-t-3").await;
    let k4 = mk_key(&e.state, "停用", "nai-t-4").await;
    let k5 = mk_key(&e.state, "疑似小号", "nai-t-5").await;
    let k6 = mk_key(&e.state, "已查看", "nai-t-6").await;
    let k7 = mk_key(&e.state, "请求过快", "nai-t-7").await;
    let k8 = mk_key(&e.state, "角色太多", "nai-t-8").await;
    let k9 = mk_key(&e.state, "重置两次", "nai-t-9").await;
    a.sql(&format!(
        "UPDATE api_keys SET expires_at={} WHERE id IN ({k2}, {k6});\
         UPDATE api_keys SET expires_at={} WHERE id IN ({k3}, {k4});\
         UPDATE api_keys SET enabled=0 WHERE id={k4};\
         INSERT INTO usage_log(ts, key_id, key_name, kind, model, status, images, detail, client)\
           WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i+1 FROM n WHERE i<60)\
           SELECT {now} - 60.0, {k7}, 'x', 'image', 'm', 'rejected', 0, '请求过于频繁（每把 Key 每分钟最多 10 次）', '' FROM n;\
         INSERT INTO usage_log(ts, key_id, key_name, kind, model, status, images, detail, client)\
           WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i+1 FROM n WHERE i<60)\
           SELECT {now} - 60.0, {k8}, 'x', 'image', 'm', 'rejected', 0, '角色太多（最多 6 个）', '' FROM n;\
         INSERT INTO admin_actions(ts, actor, action, target, detail, ok) VALUES \
           ({now} - 120.0, '站长', '重置今日额度', 'Key #{k9}', '', 1), \
           ({now} - 60.0, '站长', '重置今日额度', 'Key #{k9}', '', 1);",
        now + 2.0 * 86400.0,
        now - 10.0
    ))
    .await;
    // 「已查看」= 站长点过「没问题」：24 小时内 need 类提醒不算（快到期那条被豁免）
    a.put(
        &format!("{A}/keys/{k6}/tags"),
        json!({"tag": "已查看", "note": ""}),
    )
    .await;
    a.put(
        &format!("{A}/keys/{k5}/tags"),
        json!({"tag": "疑似小号", "note": "观察中"}),
    )
    .await;
    // 当日计数：批量取回后每把 Key 的 used 仍然正确
    a.sql(&format!(
        "INSERT INTO counters(key_id, day, images, anlas, text_tokens, requests, v5, legacy_free_images) VALUES \
         (1, '{day}', 2, 1.0, 0, 1, 1, 2);"
    ))
    .await;

    let all = a.get(&format!("{A}/members")).await.json();
    assert_eq!(
        (
            all["filter"].clone(),
            all["total"].clone(),
            all["attention"].clone()
        ),
        (json!("all"), json!(9), json!(5)),
        "k2 快到期、k3 已过期、k5 疑似、k8 角色太多、k9 重置两次 → 5 个要处理"
    );
    assert_eq!(
        all["members"].as_array().unwrap().len(),
        9,
        "缺省 filter 保持全量"
    );
    assert_eq!(
        a.get(&format!("{A}/members?filter=all")).await.json()["members"]
            .as_array()
            .unwrap()
            .len(),
        9
    );

    let att = a.get(&format!("{A}/members?filter=attention")).await.json();
    assert_eq!(
        (
            att["filter"].clone(),
            att["total"].clone(),
            att["attention"].clone()
        ),
        (json!("attention"), json!(9), json!(5)),
        "attention 模式只裁 members，total/attention 仍是全量口径"
    );
    let names: Vec<&str> = att["members"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        ["重置两次", "角色太多", "疑似小号", "已过期", "快到期"],
        "id 倒序；停用的 k4、已查看的 k6、只有「请求过快」的 k7 不在里面"
    );
    assert_eq!(att["members"][0]["today"]["images"], 0);
    assert_eq!(
        all["members"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["id"] == 1)
            .unwrap()["today"]["images"],
        2,
        "批量计数没串行/错位"
    );

    let bad = a.get(&format!("{A}/members?filter=bad")).await;
    assert_eq!(bad.status, 422, "{}", bad.text());
    assert_eq!(bad.json()["detail"], "filter 只能是 all 或 attention");
}

/// `/admin/api/keys` 的今日计数改成一次批量查询后，逐把 Key 的数字（含「重置额度」基线扣减）不变。
#[tokio::test]
async fn keys_list_uses_batched_counters_with_offsets() {
    let (e, a) = ready().await;
    let k2 = mk_key(&e.state, "乙", "nai-t-2").await;
    let day = e.state.day();
    a.sql(&format!(
        "INSERT INTO counters(key_id, day, images, anlas, text_tokens, requests, v5, legacy_free_images) VALUES \
         (1, '{day}', 3, 12.5, 7, 4, 2, 5), ({k2}, '{day}', 1, 2.0, 0, 1, 1, 1);\
         INSERT INTO daily_quota_offsets(key_id, day, anlas, v5, legacy) VALUES (1, '{day}', 10.0, 2, 5);"
    ))
    .await;
    let v = a.get(&format!("{A}/keys")).await.json();
    let keys = v["keys"].as_array().unwrap();
    let k1 = keys.iter().find(|k| k["id"] == 1).unwrap();
    assert_eq!(
        k1["used"],
        json!({"images": 3, "generated_images_total": 0, "legacy_free_images": 0,
               "anlas": 2.5, "v5": 0, "requests": 4}),
        "重置过额度：Anlas / V5 / V4.5 都扣基线（12.5-10 / 2-2 / 5-5），图片数不扣"
    );
    let other = keys.iter().find(|k| k["id"] == k2).unwrap();
    assert_eq!(
        other["used"],
        json!({"images": 1, "generated_images_total": 0, "legacy_free_images": 1,
               "anlas": 2.0, "v5": 1, "requests": 1}),
        "没有 offset 的 Key 原样返回"
    );
}

#[tokio::test]
async fn direct_member_reward_endpoint_is_removed() {
    let (_e, a) = ready().await;
    let response = a
        .post(
            &format!("{A}/gift"),
            json!({"discord_id": "900000000000000070"}),
        )
        .await;
    assert_ne!(response.status, 200);
}

#[tokio::test]
async fn share_clear_lifts_the_pause() {
    let (e, a) = ready().await;
    e.state
        .pauses
        .set(1, e.state.now() + 600.0, "客户端死循环重试".into());
    e.state
        .db
        .upsert_pause(1, e.state.now() + 600.0, "客户端死循环重试")
        .await
        .unwrap();
    let r = a.post(&format!("{A}/keys/1/share-clear"), json!({})).await;
    assert_eq!(r.json(), json!({"ok": true}));
    assert!(e.state.pauses.paused(1, e.state.now()).is_none());
    assert!(
        query(&e.state.db, "SELECT 1 FROM share_state")
            .await
            .is_empty()
    );
    // 没有暂停 / Key 不存在也一样成功
    assert_eq!(
        a.post(&format!("{A}/keys/999/share-clear"), json!({}))
            .await
            .status,
        200
    );
    assert!(
        a.actions()
            .await
            .iter()
            .any(|(act, t, ..)| act == "防分享清零（误判）" && t == "Key #1 测试成员")
    );
}

#[tokio::test]
async fn oversized_ids_are_rejected_with_422() {
    let (e, a) = ready().await;
    a.get(&format!("{A}/keys/99999999999999999999/coupons"))
        .await
        .assert_detail(422, "key_id 必须是整数");
    // 写请求的 422 进操作日志，记成失败
    a.post(
        &format!("{A}/keys/99999999999999999999/regenerate"),
        json!({}),
    )
    .await
    .assert_detail(422, "key_id 必须是整数");
    let acts = a.actions().await;
    assert!(
        acts.iter().any(|(act, _, detail, ok)| act == "重新生成 Key"
            && !ok
            && detail.starts_with("失败（422）")),
        "{acts:?}"
    );
    assert!(
        acts.iter()
            .all(|(act, _, _, ok)| act != "重新生成 Key" || !ok),
        "非法 id 不能记成成功：{acts:?}"
    );
    drop(e);
}
