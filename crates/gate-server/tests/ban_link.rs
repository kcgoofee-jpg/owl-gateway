//! 封禁号关联：和已封禁账号同浏览器 / 同（非苹果）设备网页登录过的账号领不到 Key；苹果设备只提醒；
//! 站长在待审里忽略那一对 = 放行，确认照样拦；无关账号不受影响；涉及账号全部封禁的信号自动标成已确认；
//! 待审接口列出每个涉及账号（含没有 Key 的）。合成数据：Discord ID 一律 9000000000000004xx。

mod common;
use common::{Admin, query, run_job, setup_registration};
use gate_server::ban_link::{self, BLOCK_NOTICE};
use gate_server::registration::{DiscordUser, Issued, RegErr};
use serde_json::{Value, json};

const GUILD: &str = "900000000000000001";
const BANNED: &str = "900000000000000401";
const X: &str = "900000000000000402";
const Y: &str = "900000000000000403";
const NVIDIA: &str = r#"{"gpuRenderer":"ANGLE (NVIDIA RTX 3060)","platform":"Win32"}"#;
const APPLE: &str = r#"{"gpuRenderer":"Apple GPU","platform":"iPhone"}"#;

async fn env() -> common::Env {
    // 发 Key 不需要找 Discord（没配成员身份组）；地址指向一个不会连上的端口
    let e = setup_registration("http://127.0.0.1:9", &[]).await;
    common::claim_for(&e.state, json!({"open":true})).await;
    let now = e.state.now();
    e.state
        .db
        .run_script(vec![format!(
            "INSERT INTO discord_bans(discord_id, created_at, reason, display_name) VALUES ('{BANNED}', {now}, '倒卖 Key', '坏人')"
        )])
        .await
        .unwrap();
    e
}

fn user(id: &str) -> DiscordUser {
    DiscordUser {
        id: id.into(),
        username: "bob".into(),
        global_name: "Bob".into(),
        avatar: String::new(),
    }
}

async fn issue(e: &common::Env, id: &str) -> Result<Issued, RegErr> {
    issue_with(e, id, "").await
}

async fn issue_with(e: &common::Env, id: &str, invite: &str) -> Result<Issued, RegErr> {
    e.state
        .registration
        .as_deref()
        .unwrap()
        .issue_direct(id, GUILD, &user(id), invite)
        .await
}

fn blocked(r: &Result<Issued, RegErr>) -> bool {
    *r == Err(RegErr::Denied(BLOCK_NOTICE.into()))
}

async fn browser(e: &common::Env, who: &[&str], bid: &str) {
    let now = e.state.now();
    for d in who {
        e.state
            .db
            .record_browser_login(d, bid, "203.0.113.9", now)
            .await
            .unwrap();
    }
}

async fn device(e: &common::Env, who: &[&str], dev: &str, data: &str) {
    let now = e.state.now();
    e.state
        .db
        .upsert_device_fingerprint(dev, data, now)
        .await
        .unwrap();
    for d in who {
        e.state
            .db
            .record_device_login(d, dev, "203.0.113.9", now)
            .await
            .unwrap();
    }
}

async fn signal(e: &common::Env, x: &str) -> Option<Value> {
    query(
        &e.state.db,
        &format!(
            "SELECT id, score, status, detail FROM abuse_signals WHERE kind='banned_link' AND subject='{}'",
            ban_link::subject(x, BANNED)
        ),
    )
    .await
    .into_iter()
    .next()
}

#[tokio::test]
async fn same_browser_blocks_register_and_invite_and_alerts() {
    let e = env().await;
    browser(&e, &[BANNED, X], "0123456789abcdef0123456789abcdef").await;
    browser(&e, &[Y], "fedcba9876543210fedcba9876543210").await;
    assert!(blocked(&issue(&e, X).await));
    // 邀请 Key 也不绑
    assert!(blocked(&issue_with(&e, X, "nai-whatever").await));
    // 无关账号照常领
    assert!(matches!(issue(&e, Y).await, Ok(Issued::Provisioned(_))));

    // 登录时写进待审；同一对再登录不重复记
    ban_link::on_login(&e.state, X).await;
    ban_link::on_login(&e.state, X).await;
    let s = signal(&e, X).await.expect("banned_link");
    assert_eq!(
        (s["score"].as_f64(), s["status"].as_str()),
        (Some(95.0), Some("open"))
    );
    let d: Value = serde_json::from_str(s["detail"].as_str().unwrap()).unwrap();
    assert_eq!(
        d["evidence"],
        json!([{"k": "关联的封禁号", "v": "坏人"}, {"k": "方式", "v": "同浏览器"}, {"k": "封禁理由", "v": "倒卖 Key"}])
    );
}

#[tokio::test]
async fn devices_only_notify_regardless_of_platform() {
    let e = env().await;
    device(&e, &[BANNED, X], "dev-pc", NVIDIA).await;
    device(&e, &[BANNED, Y], "dev-iphone", APPLE).await;
    assert!(matches!(issue(&e, X).await, Ok(Issued::Provisioned(_))));
    assert!(matches!(issue(&e, Y).await, Ok(Issued::Provisioned(_))));

    // 封禁号自己又登录：反过来给两个关联账号都记上
    ban_link::on_login(&e.state, BANNED).await;
    let x = signal(&e, X).await.unwrap();
    assert_eq!(x["score"], 60.0);
    let y = signal(&e, Y).await.unwrap();
    assert_eq!(
        (y["score"].as_f64(), y["status"].as_str()),
        (Some(60.0), Some("open"))
    );
    assert!(y["detail"].as_str().unwrap().contains("同设备（苹果）"));
    // Y 已有的 Key 不受影响
    let k = query(&e.state.db, &format!("SELECT k.enabled FROM api_keys k JOIN discord_registrations r ON r.key_id=k.id WHERE r.discord_id='{Y}'")).await;
    assert_eq!(k, vec![json!({"enabled": 1})]);
}

#[tokio::test]
async fn historical_device_block_is_recomputed_without_losing_review() {
    let e = env().await;
    device(&e, &[BANNED, X], "dev-pc", NVIDIA).await;
    e.state
        .db
        .upsert_abuse_signal(
            ban_link::KIND,
            &ban_link::subject(X, BANNED),
            "",
            95.0,
            "{}",
            e.state.now(),
        )
        .await
        .unwrap();
    assert!(!ban_link::blocked(&e.state.db, X).await.unwrap());
    let id = signal(&e, X).await.unwrap()["id"].as_i64().unwrap();
    e.state.db.set_abuse_status(id, "confirmed").await.unwrap();
    run_job(&e.state, "abuse_scan").await.unwrap();
    let updated = signal(&e, X).await.unwrap();
    assert_eq!(updated["score"], 60.0);
    assert_eq!(updated["status"], "confirmed");
}

#[tokio::test]
async fn dismiss_unblocks_confirm_keeps_blocking() {
    let e = env().await;
    let a = Admin::login(&e.state).await;
    browser(&e, &[BANNED, X], "0123456789abcdef0123456789abcdef").await;
    ban_link::on_login(&e.state, X).await;
    let id = signal(&e, X).await.unwrap()["id"].as_i64().unwrap();

    let r = a
        .post(
            &format!("/admin/api/abuse/{id}"),
            json!({"action": "confirm"}),
        )
        .await;
    assert_eq!(r.json()["status"], "confirmed");
    assert!(blocked(&issue(&e, X).await));

    let r = a
        .post(
            &format!("/admin/api/abuse/{id}"),
            json!({"action": "dismiss"}),
        )
        .await;
    assert_eq!(r.json()["status"], "dismissed");
    // 再登录、再扫描也不会把已忽略的这一对重新打开（分数没变）
    ban_link::on_login(&e.state, X).await;
    run_job(&e.state, "abuse_scan").await.unwrap();
    assert_eq!(signal(&e, X).await.unwrap()["status"], "dismissed");
    assert!(matches!(issue(&e, X).await, Ok(Issued::Provisioned(_))));
}

#[tokio::test]
async fn scan_writes_banned_link_but_skips_keyless_signals() {
    let e = env().await;
    let other_banned = "900000000000000404";
    let now = e.state.now();
    e.state
        .db
        .run_script(vec![format!(
            "INSERT INTO discord_bans(discord_id, created_at, reason) VALUES ('{other_banned}', {now}, '')"
        )])
        .await
        .unwrap();
    // 两个封禁号同一个浏览器：没有新信息；X 和封禁号同设备：提醒
    browser(
        &e,
        &[BANNED, other_banned],
        "0123456789abcdef0123456789abcdef",
    )
    .await;
    device(&e, &[BANNED, X], "dev-pc", NVIDIA).await;
    let summary = run_job(&e.state, "abuse_scan").await.unwrap();
    assert!(summary.contains("banned_link 1"), "{summary}");
    let rows = query(
        &e.state.db,
        "SELECT kind, subject, status FROM abuse_signals ORDER BY kind",
    )
    .await;
    // 这几个账号都没有 Key：只留 banned_link（领 Key 拦截靠它的忽略状态放行），其余「无 Key」信号不写
    assert_eq!(
        rows,
        vec![
            json!({"kind": "banned_link", "subject": ban_link::subject(X, BANNED), "status": "open"}),
        ]
    );
}

/// 待审接口：每条信号附上涉及的每个账号（没有 Key 的也在），带 Key、封禁、账号注册时间、在共用浏览器上的首次 / 最近登录；不含 IP。
#[tokio::test]
async fn admin_list_carries_linked_members() {
    let e = env().await;
    let a = Admin::login(&e.state).await;
    let Ok(Issued::Provisioned(_)) = issue(&e, Y).await else {
        panic!("Y 应能领 Key")
    };
    let bid = "0123456789abcdef0123456789abcdef";
    let t0 = e.state.now() - 3600.0;
    e.state
        .db
        .record_browser_login(X, bid, "203.0.113.9", t0)
        .await
        .unwrap();
    browser(&e, &[BANNED, X, Y], bid).await;
    e.state
        .db
        .upsert_web_profile(X, "xuser", "小X", "abc", t0)
        .await
        .unwrap();
    run_job(&e.state, "abuse_scan").await.unwrap();
    let v = a.get("/admin/api/abuse").await.json();
    let same = v["signals"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["kind"] == "device_cluster")
        .expect("device_cluster");
    let m = same["members"].as_array().unwrap();
    assert_eq!(m.len(), 3);
    let by = |id: &str| m.iter().find(|x| x["discord_id"] == id).unwrap().clone();
    let (b, x, y) = (by(BANNED), by(X), by(Y));
    assert_eq!(
        (
            b["banned"].as_bool(),
            b["ban_reason"].as_str(),
            b["display_name"].as_str()
        ),
        (Some(true), Some("倒卖 Key"), Some("坏人"))
    );
    assert_eq!(
        (x["display_name"].as_str(), x["username"].as_str()),
        (Some("小X"), Some("xuser"))
    );
    assert_eq!(
        x["avatar_url"],
        format!("https://cdn.discordapp.com/avatars/{X}/abc.png?size=64")
    );
    assert_eq!(x["key_id"], Value::Null);
    assert_eq!(x["first_login"].as_f64(), Some(t0));
    assert!(x["last_login"].as_f64().unwrap() > t0);
    assert!(x["created_at"].as_f64().is_some());
    assert!(y["key_id"].as_i64().is_some());
    assert_eq!(y["banned"], false);
    assert!(!v.to_string().contains("203.0.113.9"), "不能带 IP");
    // 封禁号关联那条也带账号
    let link = v["signals"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["kind"] == "banned_link")
        .unwrap();
    assert_eq!(link["kind_label"], "封禁号关联");
    assert_eq!(link["members"].as_array().unwrap().len(), 2);
}
