//! 后台「成员」接口：Key / 成员 / 黑名单 / 券 / 标签 / 奖励。
//!
//! 逐字节快照见 `admin_snapshot.rs`；这里按接口写成可读的用例：
//! 鉴权、校验文案、返回结构、写进库的行、操作日志，注册服务（真实的 RegistrationService + 本机假 Discord）和动态额度占位内核。

mod admin_util;
mod common;
use admin_util::MockDiscord;
use common::{Admin, Env, TOKEN, mk_key, query, ready, send, setup_registration};
use serde_json::{Value, json};

const H: &str = "身份组只能是 regular（普通）或 invite（邀请）；站长 / 测试身份组已删除";
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
async fn keys_list_shape_roles_and_counters() {
    let (e, a) = ready().await;
    let id2 = mk_key(&e.state, "邀请-7", "nai-t-2").await;
    e.state
        .db
        .set_key_role(id2, "invite", e.state.now())
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
    let order: Vec<&str> = k.as_object().unwrap().keys().map(String::as_str).collect();
    assert_eq!(
        order,
        [
            "id",
            "name",
            "token",
            "enabled",
            "daily_images",
            "daily_anlas",
            "daily_v5",
            "monthly_anlas",
            "quota_limits",
            "quota_limits_valid",
            "rpm",
            "allow_anlas",
            "allow_img2img",
            "exclude_global_v5",
            "image_model_scope",
            "quota_auto",
            "role",
            "role_label",
            "features",
            "expires_at",
            "created_at",
            "last_used_at",
            "used"
        ]
    );
    assert_eq!(k["token"], TOKEN);
    assert_eq!(k["quota_auto"], true);
    assert_eq!(k["features"], Value::Null);
    assert_eq!(
        (k["role"].as_str(), k["role_label"].as_str()),
        (Some("regular"), Some("普通"))
    );
    assert_eq!(
        k["used"],
        json!({"images": 3, "generated_images_total": 6, "legacy_free_images": 1, "anlas": 1.24, "v5": 2,
               "requests": 4}),
        "不再返回文本 token 计数"
    );
    assert_eq!(keys[1]["role"], "invite");
    assert_eq!(keys[1]["role_label"], "邀请");
    assert_eq!(keys[0]["role"], "regular");
}

#[tokio::test]
async fn create_key_validates_then_stores_manual_quota() {
    let (e, a) = ready().await;
    for (body, detail) in [
        (
            json!({"daily_images": -1}),
            "daily_images 必须在 0～1000000 之间",
        ),
        (
            json!({"daily_images": "abc"}),
            "daily_images 必须是有效数字",
        ),
        (json!({"daily_images": null}), "daily_images 不能为空"),
        (json!({"daily_images": true}), "daily_images 不能为空"),
        (json!({"daily_images": 1.5}), "daily_images 必须是整数"),
        (
            json!({"monthly_anlas": 100001}),
            "monthly_anlas 必须在 0.0～100000.0 之间",
        ),
        (json!({"rpm": 0}), "rpm 必须在 1～600 之间"),
        (
            json!({"expires_days": 3651}),
            "expires_days 必须在 0～3650 之间",
        ),
        (
            json!({"allow_anlas": true, "daily_anlas": 0, "monthly_anlas": 0}),
            // 文案写「0 = 不限」，但两项都是 0 实际被拒——与旧版一致
            "开通 Anlas 时必须设置每日或每月 Anlas 上限（0 = 不限），不能两项都留空",
        ),
        (
            json!({"features": ["nope"]}),
            "features 必须是功能名列表：image,upscale,augment,vibe,tags",
        ),
        (json!({"role": "admin"}), H),
        (json!({"role": "test"}), H),
    ] {
        a.post(&format!("{A}/keys"), body)
            .await
            .assert_detail(422, detail);
    }
    let bad = send(
        &e.state,
        "POST",
        &format!("{A}/keys"),
        Some(&a.cookie),
        None,
        &[],
    )
    .await;
    bad.assert_detail(
        400,
        "请求格式错误，未处理。请更新或更换客户端。开发者参考：请求体不是合法 JSON",
    );

    let r = a
        .post(
            &format!("{A}/keys"),
            json!({"name": "  新成员  ", "daily_images": "12", "daily_v5": 3, "monthly_anlas": 10, "rpm": 7,
                   "role": "INVITE ", "features": ["vibe", "image"], "expires_days": 2, "image_model_scope": "all"}),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    let key = &r.json()["key"];
    assert_eq!(key["name"], "新成员");
    assert_eq!(key["daily_images"], 12);
    assert_eq!(key["monthly_anlas"], 10.0);
    assert_eq!(key["features"], json!(["image", "vibe"]));
    assert_eq!(key["image_model_scope"], "all");
    assert_eq!(key["role"], "invite");
    assert!(key["token"].as_str().unwrap().starts_with("nai-"));
    assert_eq!(key["token"].as_str().unwrap().len(), 36);
    let row = e
        .state
        .db
        .get_key(key["id"].as_i64().unwrap())
        .await
        .unwrap()
        .unwrap();
    // 后台手动建的 Key 标为手动额度：动态额度算法不覆盖
    assert_eq!(row["quota_auto"], -1);
    assert_eq!(row["anlas_auto"], 0, "3.0 已删除的列原样不动");
    assert!(row["expires_at"].as_f64().unwrap() > e.state.now() + 86400.0);

    // 没传 features：沿用「新成员默认功能」= 只有文生图；站长把默认设成 * 则不限
    let r = a.post(&format!("{A}/keys"), json!({})).await;
    assert_eq!(r.json()["key"]["name"], "未命名");
    assert_eq!(r.json()["key"]["features"], json!(["image"]));
    e.state
        .db
        .set_setting("register_features", "*")
        .await
        .unwrap();
    let r = a
        .post(&format!("{A}/keys"), json!({"name": "全功能"}))
        .await;
    assert_eq!(r.json()["key"]["features"], Value::Null);
    let r = a
        .post(&format!("{A}/keys"), json!({"name": "x", "features": null}))
        .await;
    assert_eq!(r.json()["key"]["features"], Value::Null);

    let acts = a.actions().await;
    // 对象取请求里的原始名字（没截断、没去空白）
    assert!(acts.iter().any(|(act, t, d, ok)| act == "新建 Key"
        && t == "  新成员  "
        && d.starts_with("{\"name\"")
        && *ok));
    assert!(
        acts.iter()
            .any(|(act, _, d, ok)| act == "新建 Key" && d.contains("daily_images") && !ok)
    );
}

/// 每日文本额度不再读也不再写：创建 / 修改里带这个字段被忽略（不校验范围），
/// 返回的 Key 没有这个字段；新建的 Key 在列里写 0（列保留，只增不减），已有的 Key 的列原样不动。
#[tokio::test]
async fn daily_text_tokens_is_ignored_and_never_returned() {
    let (e, a) = ready().await;
    let r = a
        .post(
            &format!("{A}/keys"),
            json!({"name": "x", "daily_text_tokens": 100_000_001}),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    let key = r.json()["key"].clone();
    assert!(key.get("daily_text_tokens").is_none());
    assert!(key["used"].get("text_tokens").is_none());
    let row = e
        .state
        .db
        .get_key(key["id"].as_i64().unwrap())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row["daily_text_tokens"], 0);

    let url = format!("{A}/keys/1");
    let before = e.state.db.get_key(1).await.unwrap().unwrap()["daily_text_tokens"].clone();
    let r = a.patch(&url, json!({"daily_text_tokens": -1})).await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert!(r.json()["key"].get("daily_text_tokens").is_none());
    assert_eq!(
        e.state.db.get_key(1).await.unwrap().unwrap()["daily_text_tokens"],
        before
    );
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
    // 额度读手动的每人额度、有效期读「领 Key」的设置、模型范围固定含 V5（没设过：150 张 / 0 张 V5 / 不过期），
    // 有没有配注册服务都一样
    assert_eq!(key["daily_images"], 150);
    assert_eq!(key["daily_v5"], 0);
    assert_eq!(key["image_model_scope"], "all");
    assert_eq!(key["daily_anlas"], 0.0);
    assert!(key["expires_at"].is_null());
    let id = key["id"].as_i64().unwrap();
    let tags = query(
        &e.state.db,
        &format!("SELECT tag, by FROM key_tags WHERE key_id={id}"),
    )
    .await;
    assert_eq!(tags, [json!({"tag": "邀请", "by": "站长"})]);
    // quota_auto 保持默认（额度交给算法）
    assert_eq!(
        e.state.db.get_key(id).await.unwrap().unwrap()["quota_auto"],
        1
    );
    let r2 = a.post(&format!("{A}/keys/invite"), json!({})).await;
    assert_eq!(r2.json()["key"]["name"], "邀请-6");
    // 列表里认出「邀请」身份组；操作日志只记处理函数那一条可读的（通用包装不再重复记原样请求体）
    let l = a.get(&format!("{A}/keys")).await.json();
    assert_eq!(l["keys"][0]["role"], "invite");
    let acts = a.actions().await;
    assert!(
        acts.iter()
            .any(|(act, t, ..)| act == "新建邀请 Key" && t == "邀请-5")
    );
    assert!(!acts.iter().any(|(act, ..)| act == "POST /keys/invite"));
}

#[tokio::test]
async fn invite_key_falls_back_to_defaults_when_the_stored_settings_are_junk() {
    let (e, a) = ready().await;
    for (k, v) in [
        ("register_daily_images", "abc"),
        ("register_daily_v5", "-5"),
        ("register_expires_days", "2.9"),
        ("register_image_scope", "all"),
        ("register_features", ""),
    ] {
        e.state.db.set_setting(k, v).await.unwrap();
    }
    let r = a.post(&format!("{A}/keys/invite"), json!({})).await;
    assert_eq!(r.status, 200, "{}", r.text());
    let key = &r.json()["key"];
    // 张数 / V5 读不出来就用默认（150 / 0）；有效期按整天取（2.9 → 2 天）；功能为空 = 一个都不开
    assert_eq!(
        (&key["daily_images"], &key["daily_v5"]),
        (&json!(150), &json!(0))
    );
    assert_eq!(key["image_model_scope"], "all");
    assert_eq!(key["features"], json!([]));
    let left = key["expires_at"].as_f64().unwrap() - e.state.now();
    assert!((left - 2.0 * 86400.0).abs() < 60.0, "{left}");
}

#[tokio::test]
async fn new_key_defaults_follow_the_registration_settings() {
    let (e, a) = ready().await;
    // 手动新建：什么都不填 → 「新成员默认」（库里没设过用 defaults 里的常量）
    let r = a.post(&format!("{A}/keys"), json!({"name": "默认"})).await;
    assert_eq!(r.status, 200, "{}", r.text());
    let key = &r.json()["key"];
    assert_eq!(
        (
            &key["daily_images"],
            &key["daily_v5"],
            &key["rpm"],
            &key["daily_anlas"],
            &key["monthly_anlas"]
        ),
        (
            &json!(150),
            &json!(0),
            &json!(10),
            &json!(0.0),
            &json!(2500.0)
        )
    );
    assert!(key["expires_at"].is_null(), "默认不过期");
    // 站长改了「领 Key」的设置：手动新建 Key 和一键邀请 Key 跟着变，不用另外改一处
    for (k, v) in [
        ("register_daily_images", "45"),
        ("register_daily_v5", "6"),
        ("register_expires_days", "14"),
        ("register_image_scope", "all"),
    ] {
        e.state.db.set_setting(k, v).await.unwrap();
    }
    let r = a.post(&format!("{A}/keys"), json!({"name": "跟随"})).await;
    let key = &r.json()["key"];
    assert_eq!(
        (&key["daily_images"], &key["daily_v5"]),
        (&json!(45), &json!(6))
    );
    let left = key["expires_at"].as_f64().unwrap() - e.state.now();
    assert!((left - 14.0 * 86400.0).abs() < 60.0, "{left}");
    let r = a.post(&format!("{A}/keys/invite"), json!({})).await;
    let key = &r.json()["key"];
    assert_eq!(
        (
            &key["daily_images"],
            &key["daily_v5"],
            &key["image_model_scope"]
        ),
        (&json!(45), &json!(6), &json!("all"))
    );
    // 请求里填了的字段照旧优先
    let r = a
        .post(
            &format!("{A}/keys"),
            json!({"name": "手填", "daily_images": 7, "expires_days": 0, "rpm": 3}),
        )
        .await;
    let key = &r.json()["key"];
    assert_eq!((&key["daily_images"], &key["rpm"]), (&json!(7), &json!(3)));
    assert!(key["expires_at"].is_null());
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
async fn patch_key_updates_fields_and_quota_ownership() {
    let (e, a) = ready().await;
    let url = format!("{A}/keys/1");
    a.patch(&format!("{A}/keys/999"), json!({}))
        .await
        .assert_detail(404, "key 不存在");
    for (body, detail) in [
        (
            json!({"quota": "x"}),
            "quota 只能是 auto（算法）或 manual（手动）",
        ),
        (json!({"daily_v5": 1.5}), "daily_v5 必须是整数"),
        (json!({"monthly_anlas": null}), "monthly_anlas 不能为空"),
        (json!({"role": "boss"}), H),
        (
            json!({"allow_anlas": true}),
            "开通 Anlas 时必须设置每日或每月 Anlas 上限（0 = 不限），不能两项都留空",
        ),
    ] {
        a.patch(&url, body).await.assert_detail(422, detail);
    }
    // 普通修改：名字、开关、额度；改了额度 → 转为手动（quota_auto = -1）
    let r = a
        .patch(&url, json!({"name": "  改名  ", "enabled": false, "daily_images": 200, "daily_anlas": 5, "rpm": 9}))
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    let key = r.json()["key"].clone();
    assert_eq!(
        (
            key["name"].as_str(),
            key["enabled"].clone(),
            key["daily_images"].clone()
        ),
        (Some("改名"), json!(false), json!(200))
    );
    assert_eq!(key["daily_anlas"], 5.0);
    assert_eq!(key["quota_auto"], false);
    // 从「仅 V4.5」开到 V5：没填 daily_v5 就给默认 15
    let r = a
        .patch(&url, json!({"image_model_scope": "all", "daily_v5": 0}))
        .await;
    assert_eq!(r.json()["key"]["daily_v5"], 0, "填了 daily_v5 就用填的");
    let r = a.patch(&url, json!({"image_model_scope": "all"})).await;
    assert_eq!(r.json()["key"]["daily_v5"], 15);
    // 3.0 已删除的请求字段被忽略，对应的列不动
    let r = a
        .patch(&url, json!({"anlas_mode": "bad", "v5_pinned": 9}))
        .await;
    assert_eq!(r.status, 200);
    let row = e.state.db.get_key(1).await.unwrap().unwrap();
    assert_eq!(
        (row["anlas_auto"].clone(), row["v5_pinned"].clone()),
        (json!(0), Value::Null)
    );
    // 有效期：天数 → 到期时间；0 = 长期
    let r = a.patch(&url, json!({"expires_days": 3})).await;
    assert!(r.json()["key"]["expires_at"].as_f64().unwrap() > e.state.now() + 2.9 * 86400.0);
    let first = a.patch(&url, json!({"expires_days": 30})).await.json()["key"]["expires_at"]
        .as_f64()
        .unwrap();
    let again = a.patch(&url, json!({"expires_days": 30})).await.json()["key"]["expires_at"]
        .as_f64()
        .unwrap();
    assert!(
        (0.0..60.0).contains(&(again - first)),
        "同样的天数再设一次：从现在重新算，不在原到期时间上叠加（{first} → {again}）"
    );
    let r = a.patch(&url, json!({"expires_days": 0})).await;
    assert_eq!(r.json()["key"]["expires_at"], Value::Null);
    // features：列表按功能顺序规整，null = 不限
    let r = a.patch(&url, json!({"features": ["vibe", "image"]})).await;
    assert_eq!(r.json()["key"]["features"], json!(["image", "vibe"]));
    // 文本生成 / 语音已不是功能：和别的未知名字一样 422
    a.patch(&url, json!({"features": ["image", "text"]}))
        .await
        .assert_detail(
            422,
            "features 必须是功能名列表：image,upscale,augment,vibe,tags",
        );
    let r = a.patch(&url, json!({"features": null})).await;
    assert_eq!(r.json()["key"]["features"], Value::Null);
    // 身份组随 PATCH 一起改；站长 / 测试身份组已删除
    a.patch(&url, json!({"role": "admin"}))
        .await
        .assert_detail(422, H);
    a.patch(&url, json!({"role": "test"}))
        .await
        .assert_detail(422, H);
    // 手动：quota_auto = -1，其它额度参数照常生效
    let r = a
        .patch(&url, json!({"quota": "manual", "daily_images": 15}))
        .await;
    assert_eq!(r.json()["key"]["daily_images"], 15);
    assert_eq!(
        e.state.db.get_key(1).await.unwrap().unwrap()["quota_auto"],
        -1
    );
    // 操作日志：修改成功一条通用记录；校验失败的记录失败原因
    let acts = a.actions().await;
    assert!(
        acts.iter()
            .any(|(act, t, ..)| act == "修改 Key" && t == "Key #1 测试成员")
    );
    assert!(acts.iter().any(|(act, _, d, ok)| act == "修改 Key"
        && d == "失败（422）：quota 只能是 auto（算法）或 manual（手动）"
        && !ok));
}

#[tokio::test]
async fn explicit_quota_modes_round_trip_with_limit_zero() {
    let (e, a) = ready().await;
    let value = json!({
        "images": {"mode":"limit", "value":0},
        "v5": {"mode":"unlimited"},
        "daily_anlas": {"mode":"limit", "value":0},
        "monthly_anlas": {"mode":"unlimited"}
    });
    let r = a
        .patch(&format!("{A}/keys/1"), json!({"quota_limits": value}))
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(r.json()["key"]["quota_limits"]["images"]["mode"], "limit");
    assert_eq!(r.json()["key"]["quota_limits"]["images"]["value"], 0.0);
    let stored = e.state.db.get_key(1).await.unwrap().unwrap();
    assert!(
        stored["quota_limits"]
            .as_str()
            .unwrap()
            .contains("\"value\":0")
    );
}

#[tokio::test]
async fn patch_quota_auto_hands_the_key_to_the_algorithm() {
    let (e, a) = ready().await;
    let before = e.state.db.get_key(1).await.unwrap().unwrap();
    assert_eq!(before["quota_auto"], 1);
    e.state.db.set_quota_auto(1, -1).await.unwrap();
    let r = a
        .patch(
            &format!("{A}/keys/1"),
            json!({"quota": "auto", "daily_images": 5, "allow_anlas": true, "features": ["image"], "enabled": true}),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    let key = &r.json()["key"];
    assert_eq!(key["quota_auto"], true);
    // 交回统一额度：请求里的额度字段被忽略（5 张不会生效）；Anlas 授权收回
    assert_ne!(key["daily_images"], 5);
    assert_eq!(
        (
            key["allow_anlas"].clone(),
            key["daily_anlas"].clone(),
            key["features"].clone()
        ),
        (json!(false), json!(0.0), Value::Null)
    );
    // 立即同步了一次：这把 Key 拿到手动的每人额度（没设过 150）
    assert_eq!(
        e.state.db.get_key(1).await.unwrap().unwrap()["daily_images"],
        150
    );
    // 算法出错时（比如读不到上游额度）请求本身不失败，只记 Bug：这条路径由 kernel 的测试覆盖
    assert!(
        query(
            &e.state.db,
            "SELECT 1 FROM error_events WHERE source='quota_algo'"
        )
        .await
        .is_empty()
    );
}

#[tokio::test]
async fn patch_never_dms_and_ignores_the_old_notify_flag() {
    let (_e, a, d) = ready_reg().await;
    a.post(
        &format!("{A}/keys/1/bind-discord"),
        json!({"discord_id": "900000000000000001"}),
    )
    .await;
    // 有可见变化时旧版会私信成员；现在不发，`notify: true / false` 都照常保存
    for notify in [json!(true), json!(false)] {
        let r = a
            .patch(
                &format!("{A}/keys/1"),
                json!({"enabled": false, "daily_images": 50, "daily_anlas": 0,
                       "image_model_scope": "all", "allow_anlas": false, "notify": notify}),
            )
            .await;
        assert_eq!(r.status, 200, "{}", r.text());
        assert_eq!(r.json()["key"]["enabled"], false);
        assert_eq!(r.json()["key"]["daily_images"], 50);
    }
    assert_no_dm(&d);
    assert!(a.actions().await.iter().all(|(act, ..)| act != "私信成员"));
}

#[tokio::test]
async fn role_reset_quota_bind_discord_and_tags() {
    let (e, a) = ready().await;
    a.post(&format!("{A}/keys/999/role"), json!({"role": "invite"}))
        .await
        .assert_detail(404, "key 不存在");
    for role in ["boss", "admin", "test"] {
        a.post(&format!("{A}/keys/1/role"), json!({"role": role}))
            .await
            .assert_detail(422, H);
    }
    let r = a
        .post(&format!("{A}/keys/1/role"), json!({"role": " Invite "}))
        .await;
    assert_eq!(
        r.json(),
        json!({"ok": true, "key_id": 1, "role": "invite", "role_label": "邀请"})
    );
    assert_eq!(
        query(&e.state.db, "SELECT tag FROM key_tags WHERE key_id=1").await,
        [json!({"tag": "邀请"})]
    );
    a.post(&format!("{A}/keys/1/role"), json!({"role": "regular"}))
        .await;
    assert!(
        query(&e.state.db, "SELECT tag FROM key_tags WHERE key_id=1")
            .await
            .is_empty()
    );
    assert!(
        a.actions()
            .await
            .iter()
            .any(|(act, t, d, _)| act == "修改账号身份组"
                && t == "Key #1 测试成员"
                && d == "切换为「普通」（纳入闲置回收）")
    );

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

    // 手动绑定 Discord
    let bind_url = format!("{A}/keys/1/bind-discord");
    let bind = |body: Value| a.post(&bind_url, body);
    bind(json!({"discord_id": "12a"}))
        .await
        .assert_detail(422, "Discord ID 必须是纯数字");
    a.post(
        &format!("{A}/keys/999/bind-discord"),
        json!({"discord_id": "9"}),
    )
    .await
    .assert_detail(404, "key 不存在");
    let other = mk_key(&e.state, "另一把", "nai-t-2").await;
    let r = bind(
        json!({"discord_id": " 900000000000000001 ", "username": " u1 ", "display_name": "一号"}),
    )
    .await;
    assert_eq!(r.status, 200, "{}", r.text());
    let v = r.json();
    assert_eq!(v["discord_id"], "900000000000000001");
    assert_eq!(v["discord"]["username"], "u1");
    assert_eq!(v["discord"]["display_name"], "一号");
    assert_eq!(
        v["discord"]["profile_url"],
        "https://discord.com/users/900000000000000001"
    );
    // 没有头像：按 (id >> 22) % 6 取默认头像
    assert_eq!(
        v["discord"]["avatar_url"],
        "https://cdn.discordapp.com/embed/avatars/1.png"
    );
    assert_eq!(
        e.state.db.get_key(1).await.unwrap().unwrap()["discord_id"],
        "900000000000000001"
    );
    a.post(
        &format!("{A}/keys/{other}/bind-discord"),
        json!({"discord_id": "900000000000000001"}),
    )
    .await
    .assert_detail(400, "该 Discord 账号已绑定到 Key #1");
    // 换绑：同一把 Key 改成新账号，保留领取时间
    let r = bind(json!({"discord_id": "900000000000000002"})).await;
    assert_eq!(
        r.json()["discord"]["display_name"],
        "",
        "显示名默认用用户名（这里为空）"
    );
    assert_eq!(
        query(&e.state.db, "SELECT discord_id FROM discord_registrations").await,
        [json!({"discord_id": "900000000000000002"})]
    );

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
    a.post(
        &format!("{A}/keys/{k3}/bind-discord"),
        json!({"discord_id": "900000000000000003"}),
    )
    .await;
    a.post(
        &format!("{A}/keys/{k4}/bind-discord"),
        json!({"discord_id": "900000000000000004", "username": "u4"}),
    )
    .await;
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
    a.post(
        &format!("{A}/keys/{k5}/bind-discord"),
        json!({"discord_id": "900000000000000005"}),
    )
    .await;
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
async fn bind_discord_reads_the_profile_through_the_bot() {
    let (e, a, d) = ready_reg().await;
    let k = mk_key(&e.state, "原名", "nai-t-2").await;
    let url = format!("{A}/keys/{k}/bind-discord");
    let reg_row = |id: &str| {
        format!(
            "SELECT username, display_name, avatar FROM discord_registrations WHERE discord_id='{id}'"
        )
    };
    // 只填 Discord ID：资料、头像、Key 名都以 Discord 为准（填的用户名被忽略）
    d.set_user(
        "900000000000000021",
        200,
        json!({"id": "900000000000000021", "username": "alice", "global_name": "Alice", "avatar": "a1"}),
    );
    let r = a
        .post(
            &url,
            json!({"discord_id": "900000000000000021", "username": "旧的"}),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    let v = r.json();
    assert_eq!(v["name"], "Alice (@alice)");
    assert_eq!(v["from_discord"], true);
    assert_eq!(v["discord"]["username"], "alice");
    assert_eq!(v["discord"]["display_name"], "Alice");
    assert_eq!(
        v["discord"]["avatar_url"],
        "https://cdn.discordapp.com/avatars/900000000000000021/a1.png?size=64"
    );
    assert_eq!(
        e.state.db.get_key(k).await.unwrap().unwrap()["name"],
        "Alice (@alice)"
    );
    assert_eq!(
        query(&e.state.db, &reg_row("900000000000000021")).await,
        [json!({"username": "alice", "display_name": "Alice", "avatar": "a1"})]
    );
    assert_eq!(
        d.calls(),
        [json!(["GET", "/users/900000000000000021", null])]
    );
    assert!(a.actions().await.iter().any(|(act, _, det, _)| act == "手动绑定 Discord"
        && det == "@alice (900000000000000021)；已从 Discord 读取资料，Key 名改为「Alice (@alice)」"));
    // 换绑：没有 global_name 时显示名用用户名，Key 名就是用户名
    d.set_user(
        "900000000000000022",
        200,
        json!({"id": "900000000000000022", "username": "bob", "global_name": null, "avatar": null}),
    );
    let v = a
        .post(&url, json!({"discord_id": "900000000000000022"}))
        .await
        .json();
    assert_eq!(
        (v["name"].clone(), v["discord"]["display_name"].clone()),
        (json!("bob"), json!("bob"))
    );
    assert_eq!(
        query(&e.state.db, &reg_row("900000000000000022")).await,
        [json!({"username": "bob", "display_name": "bob", "avatar": ""})]
    );
    // 读不到（404）：退回填写的用户名，Key 名不动，旧账号的头像不留
    d.set_user(
        "900000000000000021",
        200,
        json!({"id": "900000000000000021", "username": "alice", "avatar": "a1"}),
    );
    a.post(&url, json!({"discord_id": "900000000000000021"}))
        .await;
    d.set_user(
        "900000000000000023",
        404,
        json!({"message": "Unknown User"}),
    );
    let v = a
        .post(
            &url,
            json!({"discord_id": "900000000000000023", "username": "typed"}),
        )
        .await
        .json();
    assert_eq!(v["from_discord"], false);
    assert_eq!(v["name"], "alice");
    assert_eq!(
        query(&e.state.db, &reg_row("900000000000000023")).await,
        [json!({"username": "typed", "display_name": "typed", "avatar": ""})]
    );
    assert_eq!(
        e.state.db.get_key(k).await.unwrap().unwrap()["name"],
        "alice"
    );
    assert!(
        a.actions()
            .await
            .iter()
            .any(|(act, _, det, _)| act == "手动绑定 Discord"
                && det == "@typed (900000000000000023)")
    );
    assert_no_dm(&d);
}

#[tokio::test]
async fn banning_a_registered_account_also_revokes_its_key() {
    let (e, a, _d) = ready_reg().await;
    let k = mk_key(&e.state, "甲", "nai-t-2").await;
    a.post(
        &format!("{A}/keys/{k}/bind-discord"),
        json!({"discord_id": "900000000000000012", "username": "u12", "display_name": "十二"}),
    )
    .await;
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
    a.post(
        &format!("{A}/keys/{k2}/bind-discord"),
        json!({"discord_id": "900000000000000002", "username": "u2"}),
    )
    .await;
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
    let keys: Vec<&str> = m[1]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        [
            "id",
            "name",
            "enabled",
            "role",
            "role_label",
            "discord_id",
            "discord",
            "created_at",
            "last_used_at",
            "expires_at",
            "daily_images",
            "daily_v5",
            "quota_limits",
            "quota_limits_valid",
            "allow_anlas",
            "daily_anlas",
            "image_model_scope",
            "quota_auto",
            "today",
            "week",
            "total_images",
            "registered_at",
            "first_image_at",
            "last_image_at",
            "rejected_24h",
            "rejected_why",
            "share",
            "reset_today",
            "tags",
            "auto_tags",
            "key_count",
            "all_key_ids"
        ],
        "3.0 已删除的 anlas_auto / anlas_mode / v5_pinned / last_client 不再返回（旧日志里的 client 也不露出来）；hours24 前端 0 读取已删"
    );
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
