//! Discord 自助领 Key：机器人桥接、成员网页登录、领取与绑定。
//!
//! 真实的 AppState + 路由 + 数据库；Discord REST 换成本机的假服务（`DISCORD_API_BASE`），它记录每个请求，
//! 所以也能断言「哪些请求发出去了、哪些没有」。期望值来自旧 Python（`registration_routes.py` 在同样的场景下
//! 的真实输出），合成数据：Discord ID 一律 9000000000000000xx。

mod common;
use axum::body::{Body, Bytes};
use axum::extract::{Request, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use common::{Call, Env, set_setting, setup, setup_registration};
use gate_server::registration::{DiscordUser, Issued, RegErr};
use serde_json::{Value, json};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

const GUILD: &str = "900000000000000001";
const MEMBER: &str = "900000000000000005";
const OTHER: &str = "900000000000000006";
const SECRET: &str = "contract-secret-contract-secret-0001";

// ------------------------------------------------------------------ 假 Discord

#[derive(Clone, Debug)]
struct Seen {
    method: String,
    path: String,
    auth: String,
    reason: String,
    ctype: String,
    body: Bytes,
}

impl Seen {
    fn form(&self) -> Vec<(String, String)> {
        String::from_utf8_lossy(&self.body)
            .split('&')
            .filter_map(|p| p.split_once('='))
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }
}

struct Scripted {
    method: &'static str,
    path: String,
    status: u16,
    body: String,
    once: bool,
}

#[derive(Default)]
struct DState {
    seen: Mutex<Vec<Seen>>,
    scripts: Mutex<Vec<Scripted>>,
}

struct Discord {
    addr: SocketAddr,
    state: Arc<DState>,
    _shutdown: tokio::sync::oneshot::Sender<()>,
}

fn reply(status: u16, body: &str) -> Response {
    let mut r = Response::new(Body::from(body.to_string()));
    *r.status_mut() = StatusCode::from_u16(status).unwrap();
    r.headers_mut()
        .insert("content-type", "application/json".parse().unwrap());
    r
}

async fn discord_handler(State(st): State<Arc<DState>>, req: Request) -> Response {
    let (parts, body) = req.into_parts();
    let bytes = axum::body::to_bytes(body, 1 << 20).await.unwrap();
    let h = |n: &str| {
        parts
            .headers
            .get(n)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string()
    };
    let (method, path) = (parts.method.to_string(), parts.uri.path().to_string());
    st.seen.lock().unwrap().push(Seen {
        method: method.clone(),
        path: path.clone(),
        auth: h("authorization"),
        reason: h("x-audit-log-reason"),
        ctype: h("content-type"),
        body: bytes,
    });
    {
        let mut scripts = st.scripts.lock().unwrap();
        if let Some(i) = scripts
            .iter()
            .position(|s| s.method == method && s.path == path)
        {
            let (status, body) = (scripts[i].status, scripts[i].body.clone());
            if scripts[i].once {
                scripts.remove(i);
            }
            return reply(status, &body);
        }
    }
    let segs: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    match (method.as_str(), segs.as_slice()) {
        ("GET", ["guilds", _, "members", _]) => reply(200, r#"{"roles": []}"#),
        ("PUT" | "DELETE", ["guilds", _, "members", _, "roles", _]) => reply(204, ""),
        ("POST", ["users", "@me", "channels"]) => reply(200, r#"{"id": "dm-1"}"#),
        ("POST", ["channels", _, "messages"]) => reply(200, "{}"),
        ("POST", ["oauth2", "token"]) => reply(200, r#"{"access_token": "tok"}"#),
        ("GET", ["users", "@me", "guilds"]) => reply(
            200,
            r#"[{"id": "900000000000000091", "name": "Owl"}, {"id": "900000000000000092", "name": "Art"}]"#,
        ),
        ("GET", ["users", "@me", "connections"]) => reply(
            200,
            r#"[{"type": "steam", "id": "900000000000000088", "name": "alice_s", "verified": true}, {"type": "github", "id": "42"}]"#,
        ),
        ("GET", ["users", "@me"]) => reply(
            200,
            &json!({"id": MEMBER, "username": "alice", "global_name": "Alice", "avatar": "a1"})
                .to_string(),
        ),
        ("GET", ["users", id]) => reply(
            200,
            &json!({"id": id, "username": "profile-user", "global_name": "Profile", "avatar": "av1"})
                .to_string(),
        ),
        _ => reply(404, "{}"),
    }
}

impl Discord {
    async fn start() -> Self {
        let state = Arc::new(DState::default());
        let app = axum::Router::new()
            .fallback(discord_handler)
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = rx.await;
                })
                .await;
        });
        Self {
            addr,
            state,
            _shutdown: tx,
        }
    }
    fn url(&self) -> String {
        format!("http://{}", self.addr)
    }
    fn script(&self, method: &'static str, path: &str, status: u16, body: Value) {
        self.push(method, path, status, body, false);
    }
    fn push(&self, method: &'static str, path: &str, status: u16, body: Value, once: bool) {
        self.state.scripts.lock().unwrap().push(Scripted {
            method,
            path: path.to_string(),
            status,
            body: body.to_string(),
            once,
        });
    }
    fn clear_scripts(&self) {
        self.state.scripts.lock().unwrap().clear();
    }
    fn seen(&self) -> Vec<Seen> {
        self.state.seen.lock().unwrap().clone()
    }
    fn calls(&self) -> Vec<String> {
        self.seen()
            .iter()
            .map(|s| format!("{} {}", s.method, s.path))
            .collect()
    }
}

// ------------------------------------------------------------------ 公共小件

async fn setup_reg(extra: &[(&str, &str)]) -> (Env, Discord) {
    setup_reg_with(extra, true).await
}

/// `legacy_expiry`：多数用例按「有效 30 天」写的，所以默认把后台「领 Key」的有效天数设成 30；
/// 实际默认值是 0（不过期），对应的环境变量已删除。
async fn setup_reg_with(extra: &[(&str, &str)], legacy_expiry: bool) -> (Env, Discord) {
    let d = Discord::start().await;
    let base = d.url();
    let e = setup_registration(&base, extra).await;
    if legacy_expiry {
        common::claim_for(&e.state, json!({"expires_days":30})).await;
    }
    (e, d)
}

/// 写一条后台设置（`site_settings`）。
async fn set_db(e: &Env, key: &str, value: &str) {
    set_setting(e, key, value).await;
}

/// 只有拥有这个身份组的成员能领 Key（本服务器里的身份组）。原来是环境变量 `DISCORD_ROLE_ID`，
/// 现在只来自后台「领 Key」的限定身份组设置。
async fn require_role(e: &Env, role: &str) {
    common::claim_for(
        &e.state,
        json!({"roles":[{"guild":GUILD,"role":role,"note":""}]}),
    )
    .await;
}

fn who(id: &str, actor: &str) -> Value {
    json!({"discord_id": id, "guild_id": GUILD, "actor_id": actor})
}

type Resp = (u16, HeaderMap, Value);

fn parse(r: (StatusCode, HeaderMap, Vec<u8>)) -> Resp {
    let v = serde_json::from_slice(&r.2).unwrap_or(Value::Null);
    (r.0.as_u16(), r.1, v)
}

async fn bridge(e: &Env, path: &str, body: Value) -> Resp {
    parse(
        Call::new("POST", path)
            .bearer(SECRET)
            .json(body)
            .send(&e.state)
            .await,
    )
}

fn detail(r: &Resp) -> &str {
    r.2["detail"].as_str().unwrap_or("")
}

fn keys(v: &Value) -> Vec<&str> {
    v.as_object().unwrap().keys().map(String::as_str).collect()
}

async fn open_registration(e: &Env) {
    common::claim_for(&e.state, json!({"open":true})).await;
}

async fn issue(e: &Env, id: &str) -> Resp {
    bridge(
        e,
        "/self-register/issue",
        json!({"discord_id": id, "guild_id": GUILD, "username": "alice", "global_name": "Alice",
               "avatar": "a1b2", "name": "alice"}),
    )
    .await
}

fn svc(e: &Env) -> &gate_server::registration::RegistrationService {
    e.state.registration.as_deref().expect("已配置")
}

fn user(id: &str) -> DiscordUser {
    DiscordUser {
        id: id.into(),
        username: "bob".into(),
        global_name: "Bob".into(),
        avatar: "bb".into(),
    }
}

async fn actions(e: &Env) -> Vec<Value> {
    let mut v = common::admin_actions(&e.state.db, 200).await;
    v.reverse();
    v
}

fn action_tuple(a: &Value) -> (String, String, String, String, i64) {
    let s = |k: &str| a[k].as_str().unwrap_or("").to_string();
    (
        s("actor"),
        s("action"),
        s("target"),
        s("detail"),
        a["ok"].as_i64().unwrap_or(-1),
    )
}

// ------------------------------------------------------------------ 桥接：鉴权与请求体校验

#[tokio::test]
async fn bridge_is_503_when_not_configured() {
    let e = setup(&[]).await;
    assert!(e.state.registration.is_none());
    let r = bridge(&e, "/self-register/resetkey", who(MEMBER, MEMBER)).await;
    assert_eq!(r.0, 503);
    assert_eq!(r.2, json!({"detail": "自助领 Key 尚未配置"}));
    // 网页登录同样没配置
    let (s, _, b) = Call::new("GET", "/login").send(&e.state).await;
    assert_eq!(s, 503);
    assert_eq!(
        serde_json::from_slice::<Value>(&b).unwrap(),
        json!({"detail": "登录尚未配置"})
    );
}

#[tokio::test]
async fn bridge_checks_body_first_then_secret_then_guild() {
    let (e, d) = setup_reg(&[]).await;
    let good = who(MEMBER, MEMBER);
    // 密钥：没带 / 不对 → 401「未授权」
    let none = parse(
        Call::new("POST", "/self-register/resetkey")
            .json(good.clone())
            .send(&e.state)
            .await,
    );
    assert_eq!((none.0, none.2), (401, json!({"detail": "未授权"})));
    let wrong = parse(
        Call::new("POST", "/self-register/resetkey")
            .bearer("nope")
            .json(good.clone())
            .send(&e.state)
            .await,
    );
    assert_eq!(wrong.0, 401);
    // 请求体错误先于鉴权（旧版 FastAPI 先校验请求体）：缺字段的 422 用 pydantic 的形状
    let partial = json!({"discord_id": MEMBER});
    let miss = parse(
        Call::new("POST", "/self-register/resetkey")
            .bearer("nope")
            .json(partial.clone())
            .send(&e.state)
            .await,
    );
    assert_eq!(miss.0, 422);
    assert_eq!(miss.2, json!({"detail":"guild_id: Field required"}));
    let no_body = parse(
        Call::new("POST", "/self-register/resetkey")
            .bearer(SECRET)
            .send(&e.state)
            .await,
    );
    assert_eq!(
        no_body.2,
        json!({"detail":gate_core::json_body::MSG_INVALID_JSON})
    );
    let list = bridge(&e, "/self-register/resetkey", json!([1])).await;
    assert_eq!(
        list.2,
        json!({"detail":gate_core::json_body::MSG_NOT_OBJECT})
    );
    let num = bridge(
        &e,
        "/self-register/resetkey",
        json!({"discord_id": 5, "guild_id": GUILD}),
    )
    .await;
    assert_eq!(
        num.2,
        json!({"detail":"discord_id: Input should be a valid string"})
    );
    // 服务器不对 / ID 不是数字 → 403
    let wrong_guild = bridge(
        &e,
        "/self-register/resetkey",
        json!({"discord_id": MEMBER, "guild_id": "2"}),
    )
    .await;
    assert_eq!(
        (wrong_guild.0, wrong_guild.2),
        (
            403,
            json!({"detail": "请在猫头鹰公益站的 Discord 服务器里使用这个命令。"})
        )
    );
    let bad_id = bridge(
        &e,
        "/self-register/resetkey",
        json!({"discord_id": "abc", "guild_id": GUILD}),
    )
    .await;
    assert_eq!(bad_id.0, 403);
    // 以上没有任何一个请求去过 Discord
    assert!(d.seen().is_empty());
}

#[tokio::test]
async fn wrong_bridge_secret_counts_toward_auth_block() {
    let (e, _d) = setup_reg(&[]).await;
    let ip: SocketAddr = "203.0.113.9:4000".parse().unwrap();
    for _ in 0..e.state.env.auth_fail_max {
        let mut req = Request::builder()
            .method("POST")
            .uri("/self-register/resetkey")
            .header("authorization", "Bearer nope")
            .header("content-type", "application/json")
            .body(Body::from(who(MEMBER, MEMBER).to_string()))
            .unwrap();
        req.extensions_mut().insert(axum::extract::ConnectInfo(ip));
        let resp = tower::ServiceExt::oneshot(gate_server::app(e.state.clone()), req)
            .await
            .unwrap();
        assert_eq!(resp.status(), 401);
    }
    assert!(e.state.auth_guard.blocked("203.0.113.9", e.state.now()) > 0);
    assert_eq!(e.state.auth_guard.blocked("203.0.113.10", e.state.now()), 0);
}

// ------------------------------------------------------------------ 桥接：封禁

#[tokio::test]
async fn ban_revokes_the_key_and_blocks_reissue_until_unban() {
    // 机器人的管理员命令（/ban /unban）已删除：封禁只剩后台和服务方法
    let (e, _d) = setup_reg(&[]).await;
    open_registration(&e).await;
    assert_eq!(issue(&e, MEMBER).await.0, 200);
    svc(&e).ban(MEMBER, "", "").await.unwrap();
    assert!(
        e.state
            .db
            .key_row_for_discord(MEMBER)
            .await
            .unwrap()
            .is_none()
    );
    let bans = e.state.db.list_bans("").await.unwrap();
    assert_eq!(bans.len(), 1);
    assert_eq!(bans[0]["reason"], "站长手动封禁");
    assert_eq!(bans[0]["banned_by"], "站长");
    assert_eq!(bans[0]["username"], "alice");
    assert_eq!(bans[0]["display_name"], "Alice");
    let again = issue(&e, MEMBER).await;
    assert_eq!(
        (again.0, detail(&again)),
        (403, "这个 Discord 账号已被站长停用，无法领取 Key。")
    );
    assert!(e.state.db.unban_discord(MEMBER).await.unwrap());
    assert!(!e.state.db.unban_discord(MEMBER).await.unwrap());
    assert_eq!(issue(&e, MEMBER).await.0, 200);
}

/// 闲置回收拉黑的人再来领：说清原因（连续几天没出图、Key 已被自动回收），不说「站长停用」，也不提解封。
#[tokio::test]
async fn idle_recycled_member_is_told_why_when_registering_again() {
    let (e, _d) = setup_reg(&[]).await;
    open_registration(&e).await;
    let info = gate_db::repo::discord::BanInfo {
        reason: gate_core::idle::ban_reason(3),
        banned_by: "系统".into(),
        ..Default::default()
    };
    e.state
        .db
        .ban_discord(MEMBER, &info, e.state.now())
        .await
        .unwrap();
    let again = issue(&e, MEMBER).await;
    assert_eq!(
        (again.0, detail(&again)),
        (
            403,
            "你之前的 Key 连续 3 天没有出图，已被自动回收，这个 Discord 账号不能再领取 Key。"
        )
    );
}

// ------------------------------------------------------------------ 桥接：发 Key

/// 没配 `REGISTER_EXPIRES_DAYS`：新 Key 不过期，欢迎私信里也没有「有效期」那一行
#[tokio::test]
async fn issued_key_never_expires_by_default() {
    let (e, _d) = setup_reg_with(&[], false).await;
    open_registration(&e).await;
    let r = issue(&e, MEMBER).await;
    assert_eq!(r.0, 200, "{:?}", r.2);
    assert!(!r.2["message"].as_str().unwrap().contains("有效期"));
    let row = e
        .state
        .db
        .key_row_for_discord(MEMBER)
        .await
        .unwrap()
        .unwrap();
    assert!(row["expires_at"].is_null(), "{row:?}");
}

#[tokio::test]
async fn issue_mints_a_key_with_current_welcome_message() {
    let (e, d) = setup_reg(&[]).await;
    // 领 Key 默认关闭
    let closed = issue(&e, MEMBER).await;
    assert_eq!(
        (closed.0, closed.2),
        (
            403,
            json!({"detail": "领 Key 当前已关闭，开放时间看站长公告。"})
        )
    );
    open_registration(&e).await;
    let r = issue(&e, MEMBER).await;
    assert_eq!(r.0, 200, "{:?}", r.2);
    assert_eq!(r.1["cache-control"], "no-store");
    assert_eq!(keys(&r.2), ["key", "message"]);
    let key = r.2["key"].as_str().unwrap().to_string();
    assert!(key.starts_with("nai-") && key.len() == 36, "{key}");
    let message = r.2["message"].as_str().unwrap();
    assert!(message.contains(&key));
    assert!(message.contains("150"));
    assert!(message.contains("本站不保存提示词或生成图片"));
    assert!(!message.contains("9999"));
    // 落库
    let row = e
        .state
        .db
        .key_row_for_discord(MEMBER)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row["token"], key.as_str());
    assert_eq!(row["name"], "Alice (@alice)");
    assert_eq!(row["discord_id"], MEMBER);
    assert_eq!(row["enabled"], 1);
    let policy = e.state.db.member_policy(MEMBER).await.unwrap().effective;
    assert_eq!(policy.daily_images, 150);
    assert_eq!(policy.daily_v5, 8);
    assert!(!policy.v5 && !policy.paid_parameters && !policy.img2img);
    let left = row["expires_at"].as_f64().unwrap() - e.state.now();
    assert!((left - 30.0 * 86400.0).abs() < 60.0, "{left}");
    let p = e
        .state
        .db
        .discord_profile_row(MEMBER)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (p.0.as_deref(), p.1.as_deref(), p.2.as_deref()),
        (Some("alice"), Some("Alice"), Some("a1b2"))
    );
    // 再领 / 别的服务器 / 密钥不对
    let again = issue(&e, MEMBER).await;
    assert_eq!(
        (again.0, detail(&again)),
        (
            403,
            "这个 Discord 账号已经领取过 Key 了。额度在网页登录后查看；Key 找不到的话，用 /resetkey 换一把新的。"
        )
    );
    let wrong = bridge(
        &e,
        "/self-register/issue",
        json!({"discord_id": MEMBER, "guild_id": "1"}),
    )
    .await;
    assert_eq!(
        (wrong.0, detail(&wrong)),
        (403, "请在指定服务器使用 /register。")
    );
    let bad_auth = parse(
        Call::new("POST", "/self-register/issue")
            .header("authorization", "x".to_string())
            .json(json!({"discord_id": MEMBER, "guild_id": GUILD}))
            .send(&e.state)
            .await,
    );
    assert_eq!((bad_auth.0, bad_auth.2), (401, json!({"detail": "未授权"})));
    // 日志：成功与失败各记一条（失败带原因）
    let log: Vec<_> = actions(&e).await.iter().map(action_tuple).collect();
    let me = format!("Discord:{MEMBER}");
    assert!(log.contains(&(
        me.clone(),
        "领取 Key 失败（/register）".into(),
        String::new(),
        "领 Key 当前已关闭，开放时间看站长公告。".into(),
        1
    )));
    assert!(log.contains(&(
        me.clone(),
        "领取 Key（/register 直发）".into(),
        String::new(),
        String::new(),
        1
    )));
    // 发 Key 不私信、不碰 Discord（没有身份组门槛 / 成员身份组）
    assert!(d.seen().is_empty(), "{:?}", d.calls());
}

/// 闲置回收关闭（KEY_INACTIVITY_DELETE_DAYS=0）时，名额满的回复不再说「闲置的 Key 会被自动回收」。
#[tokio::test]
async fn full_message_does_not_promise_recycling_when_it_is_off() {
    let (e, _d) = setup_reg(&[("KEY_INACTIVITY_DELETE_DAYS", "0")]).await;
    common::claim_for(&e.state, json!({"max_users":1})).await;
    open_registration(&e).await;
    assert_eq!(issue(&e, MEMBER).await.0, 200);
    let full = issue(&e, OTHER).await;
    assert_eq!(
        (full.0, detail(&full)),
        (
            403,
            "名额已满（上限 1 人），请过段时间再试。空出的名额先到先得。"
        )
    );
}

#[tokio::test]
async fn issue_enforces_capacity_age_hourly_cap_and_expiry_rotation() {
    let (e, _d) = setup_reg(&[]).await;
    common::claim_for(&e.state, json!({"max_users":1})).await;
    open_registration(&e).await;
    assert_eq!(issue(&e, MEMBER).await.0, 200);
    // 名额满：先到先得
    let full = issue(&e, OTHER).await;
    assert_eq!(
        (full.0, detail(&full)),
        (
            403,
            "名额已满（上限 1 人），请过段时间再试。闲置的 Key 会被自动回收，空出的名额先到先得。"
        )
    );
    // 撤销释放名额
    assert!(svc(&e).revoke(MEMBER, true).await.unwrap());
    assert!(
        !svc(&e).revoke(MEMBER, true).await.unwrap(),
        "没有 Key 可撤销"
    );
    assert_eq!(issue(&e, OTHER).await.0, 200);
    // Key 过期：释放名额，成员可以重新领
    let row = e
        .state
        .db
        .key_row_for_discord(OTHER)
        .await
        .unwrap()
        .unwrap();
    let mut f = serde_json::Map::new();
    f.insert("expires_at".into(), json!(e.state.now() - 5.0));
    e.state
        .db
        .update_key(row["id"].as_i64().unwrap(), &f)
        .await
        .unwrap();
    assert_eq!(svc(&e).count_active().await.unwrap(), 0);
    let renewed = issue(&e, OTHER).await;
    assert_eq!(renewed.0, 200);
    assert!(
        actions(&e)
            .await
            .iter()
            .map(action_tuple)
            .any(|a| a.1 == "Key 到期释放名额" && a.4 == 1)
    );
    // 被站长停用的成员不会因为过期被放走
    let row = e
        .state
        .db
        .key_row_for_discord(OTHER)
        .await
        .unwrap()
        .unwrap();
    let mut f = serde_json::Map::new();
    f.insert("enabled".into(), json!(0));
    f.insert("expires_at".into(), json!(e.state.now() - 5.0));
    e.state
        .db
        .update_key(row["id"].as_i64().unwrap(), &f)
        .await
        .unwrap();
    let held = issue(&e, OTHER).await;
    assert_eq!(
        (held.0, detail(&held)),
        (
            403,
            "这个 Discord 账号已经领取过 Key 了。额度在网页登录后查看；Key 找不到的话，用 /resetkey 换一把新的。"
        )
    );
    // 每小时领取上限
    let (e2, _d2) = setup_reg(&[]).await;
    common::claim_for(&e2.state, json!({"max_users":0})).await;
    open_registration(&e2).await;
    e2.state
        .db
        .set_setting("issue_hourly_cap", "2")
        .await
        .unwrap();
    assert_eq!(issue(&e2, "900000000000000101").await.0, 200);
    assert_eq!(issue(&e2, "900000000000000102").await.0, 200);
    let limited = issue(&e2, "900000000000000103").await;
    assert_eq!(
        (limited.0, detail(&limited)),
        (
            403,
            "本小时领取人数较多，为保护服务稳定已暂时限流，请过几分钟再用 /register 领取。"
        )
    );
    e2.state
        .db
        .set_setting("issue_hourly_cap", "0")
        .await
        .unwrap();
    assert_eq!(issue(&e2, "900000000000000103").await.0, 200);
}

#[tokio::test]
async fn young_discord_accounts_are_refused() {
    let (e, _d) = setup_reg(&[]).await;
    open_registration(&e).await;
    let snowflake = |age_days: f64| {
        let ms = e.state.now() * 1000.0 - age_days * 86_400_000.0;
        (((ms as u64) - 1_420_070_400_000) << 22).to_string()
    };
    let young = issue(&e, &snowflake(1.0)).await;
    assert_eq!(
        (young.0, detail(&young)),
        (403, "Discord 账号创建满 7 天后才能领 Key，请稍后再来。")
    );
    assert_eq!(issue(&e, &snowflake(30.0)).await.0, 200);
}

/// 走一遍普通「登录」：`/login` 取 state，再不带 Cookie 回调（模拟丢 Cookie 的内置浏览器）。
/// 门槛配齐时这一步就会申请并核验身份组。
async fn verify_login(e: &Env) -> (u16, HeaderMap) {
    login_via(e, "/login").await
}

async fn login_via(e: &Env, start: &str) -> (u16, HeaderMap) {
    let (s, h, _) = get_raw(e, start, None).await;
    assert_eq!(s, 307);
    let loc = h["location"].to_str().unwrap();
    let st = loc.split("state=").nth(1).unwrap().to_string();
    let (s, h, _) = get_raw(e, &format!("/login/callback?code=ok&state={st}"), None).await;
    (s, h)
}

fn member_path(guild: &str) -> String {
    format!("/users/@me/guilds/{guild}/member")
}

#[tokio::test]
async fn role_gate_needs_oauth_verification_not_the_bot_token() {
    let (e, d) = setup_reg(&[]).await;
    require_role(&e, "900000000000000077").await;
    open_registration(&e).await;
    // 没核验过：拒绝，并给出验证链接；不发任何 Discord 请求（机器人不在那个服务器里）
    let r = issue(&e, MEMBER).await;
    assert_eq!(
        (r.0, detail(&r)),
        (
            403,
            "目前只开放给「指定身份组」：请先到 https://gate.test/login 用 Discord 登录（授权时会核验身份组），再用 /register 领取。"
        )
    );
    assert!(d.seen().is_empty());
    // 授权后没有身份组 → 仍不能领
    d.script("GET", &member_path(GUILD), 200, json!({"roles": ["1"]}));
    let (s, h) = verify_login(&e).await;
    assert_eq!((s, h["location"].to_str().unwrap()), (307, "/?verify=no"));
    assert_eq!(
        cookie_value(&h, "owl_member"),
        "",
        "没有 Key 又没身份组：不发会话"
    );
    assert_eq!(issue(&e, MEMBER).await.0, 403);
    // 有身份组 → 记下核验，能领
    d.clear_scripts();
    d.script(
        "GET",
        &member_path(GUILD),
        200,
        json!({"roles": ["900000000000000077"]}),
    );
    let (s, h) = verify_login(&e).await;
    assert_eq!((s, h["location"].to_str().unwrap()), (307, "/?verify=ok"));
    assert!(
        e.state
            .db
            .role_verified_since(MEMBER, GUILD, "900000000000000077", 0.0)
            .await
            .unwrap()
    );
    assert_eq!(issue(&e, MEMBER).await.0, 200);
    // 发请求用的是成员自己的授权，没有一次用机器人 Token 查成员
    assert!(
        d.seen()
            .iter()
            .filter(|s| s.path.ends_with("/member"))
            .all(|s| s.auth == "Bearer tok")
    );
    assert!(d.seen().iter().all(|s| s.auth != "Bot fake-bot-token"));
}

#[tokio::test]
async fn role_gate_can_target_another_community_via_settings() {
    let (e, d) = setup_reg(&[]).await;
    open_registration(&e).await;
    let other_guild = "900000000000000222";
    common::claim_for(
        &e.state,
        json!({"roles":[{"guild":other_guild,"role":"900000000000000333","note":"创作者"}]}),
    )
    .await;
    // 别的服务器 / 别的身份组的核验不算数
    e.state
        .db
        .upsert_role_verification(MEMBER, GUILD, "900000000000000333", 1.0)
        .await
        .unwrap();
    e.state
        .db
        .upsert_role_verification(MEMBER, other_guild, "1", 1.0)
        .await
        .unwrap();
    let r = issue(&e, MEMBER).await;
    assert_eq!(
        (r.0, detail(&r)),
        (
            403,
            "目前只开放给「创作者」：请先到 https://gate.test/login 用 Discord 登录（授权时会核验身份组），再用 /register 领取。"
        )
    );
    d.script(
        "GET",
        &member_path(other_guild),
        200,
        json!({"roles": ["900000000000000333"]}),
    );
    assert_eq!(verify_login(&e).await.0, 307);
    assert_eq!(issue(&e, MEMBER).await.0, 200);
}

/// 类脑「社区建设组」或棱镜「创作者」：主门槛 + 一条额外身份组，满足任意一条就能领（站长 10-18）。
const GUILD_B: &str = "900000000000000222";
const ROLE_A: &str = "900000000000000077";
const ROLE_B: &str = "900000000000000333";

async fn two_rules(e: &Env) {
    open_registration(e).await;
    common::claim_for(
        &e.state,
        json!({"roles":[
            {"guild":GUILD,"role":ROLE_A,"note":"类脑「社区建设组」"},
            {"guild":GUILD_B,"role":ROLE_B,"note":"棱镜「创作者」"}
        ]}),
    )
    .await;
}

#[tokio::test]
async fn role_gate_accepts_any_of_several_rules() {
    let (e, d) = setup_reg(&[]).await;
    two_rules(&e).await;
    // 都没核验：拒绝，说明里把各条的显示名用「 或 」连起来
    let r = issue(&e, MEMBER).await;
    assert_eq!(
        (r.0, detail(&r)),
        (
            403,
            "目前只开放给「类脑「社区建设组」 或 棱镜「创作者」」：请先到 https://gate.test/login 用 Discord 登录（授权时会核验身份组），再用 /register 领取。"
        )
    );
    // 在第一个服务器里没有身份组、在第二个服务器里有 → 通过，只记命中的那一条
    d.script("GET", &member_path(GUILD), 200, json!({"roles": ["1"]}));
    d.script(
        "GET",
        &member_path(GUILD_B),
        200,
        json!({"roles": [ROLE_B]}),
    );
    let (s, h) = verify_login(&e).await;
    assert_eq!((s, h["location"].to_str().unwrap()), (307, "/?verify=ok"));
    assert!(
        e.state
            .db
            .role_verified_since(MEMBER, GUILD_B, ROLE_B, 0.0)
            .await
            .unwrap()
    );
    assert!(
        !e.state
            .db
            .role_verified_since(MEMBER, GUILD, ROLE_A, 0.0)
            .await
            .unwrap()
    );
    assert_eq!(issue(&e, MEMBER).await.0, 200);
}

#[tokio::test]
async fn role_gate_main_rule_alone_still_passes_and_other_guild_is_not_required() {
    let (e, d) = setup_reg(&[]).await;
    two_rules(&e).await;
    // 第一个服务器有身份组；第二个服务器不在（404）→ 通过
    d.script("GET", &member_path(GUILD), 200, json!({"roles": [ROLE_A]}));
    d.script("GET", &member_path(GUILD_B), 404, json!({"code": 10007}));
    let (_, h) = verify_login(&e).await;
    assert_eq!(h["location"].to_str().unwrap(), "/?verify=ok");
    assert!(
        e.state
            .db
            .role_verified_since(MEMBER, GUILD, ROLE_A, 0.0)
            .await
            .unwrap()
    );
    assert!(
        !e.state
            .db
            .role_verified_since(MEMBER, GUILD_B, ROLE_B, 0.0)
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn role_gate_multi_rule_outcomes_never_blame_the_member_for_discord_failures() {
    let (e, d) = setup_reg(&[]).await;
    two_rules(&e).await;
    // 第一个服务器 404（不在）、第二个服务器 500：没命中，但有一边是 Discord 故障 → 不能说成「没有身份组」
    d.script("GET", &member_path(GUILD), 404, json!({"code": 10007}));
    d.script("GET", &member_path(GUILD_B), 500, json!({}));
    let (_, h) = verify_login(&e).await;
    assert_eq!(h["location"].to_str().unwrap(), "/?verify=error");
    // 两边都明确回答「不在 / 没有」→ 才是「没有身份组」
    d.clear_scripts();
    d.script("GET", &member_path(GUILD), 404, json!({"code": 10007}));
    d.script("GET", &member_path(GUILD_B), 200, json!({"roles": ["1"]}));
    let (_, h) = verify_login(&e).await;
    assert_eq!(h["location"].to_str().unwrap(), "/?verify=no");
    assert_eq!(issue(&e, MEMBER).await.0, 403);
}

#[tokio::test]
async fn role_gate_with_only_extra_rule_and_same_guild_checks_the_guild_once() {
    let (e, d) = setup_reg(&[]).await;
    open_registration(&e).await;
    // 没有主门槛，只有额外身份组；同一个服务器里两条身份组 → 只查一次
    common::claim_for(
        &e.state,
        json!({"roles":[
            {"guild":GUILD_B,"role":ROLE_B,"note":"创作者"},
            {"guild":GUILD_B,"role":"900000000000000444","note":"管理组"}
        ]}),
    )
    .await;
    d.script(
        "GET",
        &member_path(GUILD_B),
        200,
        json!({"roles": ["900000000000000444"]}),
    );
    let (_, h) = verify_login(&e).await;
    assert_eq!(h["location"].to_str().unwrap(), "/?verify=ok");
    assert!(
        e.state
            .db
            .role_verified_since(MEMBER, GUILD_B, "900000000000000444", 0.0)
            .await
            .unwrap()
    );
    assert_eq!(
        d.seen()
            .iter()
            .filter(|s| s.path == member_path(GUILD_B))
            .count(),
        1
    );
}

#[tokio::test]
async fn verify_outcomes_never_blame_the_member_for_discord_failures() {
    let (e, d) = setup_reg(&[]).await;
    require_role(&e, "900000000000000077").await;
    let cases: [(u16, Value, &str); 4] = [
        (404, json!({"code": 10007}), "/?verify=no"),
        (429, json!({"retry_after": 1}), "/?verify=error"),
        (500, json!({}), "/?verify=error"),
        (200, json!("not-an-object"), "/?verify=no"),
    ];
    for (status, body, want) in cases {
        d.clear_scripts();
        d.script("GET", &member_path(GUILD), status, body);
        let (s, h) = verify_login(&e).await;
        assert_eq!(
            (s, h["location"].to_str().unwrap()),
            (307, want),
            "{status}"
        );
        // 没有 Key 的人被门槛挡在登录这一步：不发会话
        assert_eq!(cookie_value(&h, "owl_member"), "", "{status}");
    }
    assert!(
        !e.state
            .db
            .role_verified_since(MEMBER, GUILD, "900000000000000077", 0.0)
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn keyed_members_keep_their_session_without_the_role() {
    let (e, d) = setup_reg(&[]).await;
    open_registration(&e).await;
    // 门槛上线前就已领过 Key 的成员
    assert_eq!(issue(&e, MEMBER).await.0, 200);
    require_role(&e, "900000000000000077").await;
    let cases: [(u16, Value); 4] = [
        (200, json!({"roles": ["1"]})),
        (404, json!({"code": 10007})),
        (429, json!({"retry_after": 1})),
        (500, json!({})),
    ];
    for (status, body) in cases {
        d.clear_scripts();
        d.script("GET", &member_path(GUILD), status, body);
        let (s, h) = verify_login(&e).await;
        // 照常登录进自己的额度页，不弹吓人的提示
        assert_eq!((s, h["location"].to_str().unwrap()), (307, "/"), "{status}");
        assert!(cookie_value(&h, "owl_member").contains(MEMBER), "{status}");
    }
    // 有身份组的老成员：同样直接进首页，也记下核验
    d.clear_scripts();
    d.script(
        "GET",
        &member_path(GUILD),
        200,
        json!({"roles": ["900000000000000077"]}),
    );
    let (s, h) = verify_login(&e).await;
    assert_eq!((s, h["location"].to_str().unwrap()), (307, "/"));
    assert!(cookie_value(&h, "owl_member").contains(MEMBER));
    assert!(
        e.state
            .db
            .role_verified_since(MEMBER, GUILD, "900000000000000077", 0.0)
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn verify_without_role_setting_passes_without_recording() {
    let (e, d) = setup_reg(&[]).await;
    let (s, h) = login_via(&e, "/login?verify=1").await;
    assert_eq!((s, h["location"].to_str().unwrap()), (307, "/"));
    assert!(cookie_value(&h, "owl_member").contains(MEMBER));
    assert!(!d.calls().iter().any(|c| c.ends_with("/member")));
}

#[tokio::test]
async fn unreachable_discord_never_mints_when_a_role_is_required() {
    // 门槛只查本地核验记录：Discord 连不上也不会放行没核验的人
    let (e, _d) = setup_reg(&[("DISCORD_API_BASE", "http://127.0.0.1:1")]).await;
    require_role(&e, "900000000000000077").await;
    open_registration(&e).await;
    let r = issue(&e, MEMBER).await;
    assert_eq!(r.0, 403);
    assert!(detail(&r).contains("https://gate.test/login "));
    assert!(
        e.state
            .db
            .key_row_for_discord(MEMBER)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn verify_scope_and_state_rules() {
    let (e, d) = setup_reg(&[]).await;
    let (_, h, _) = get_raw(&e, "/login?verify=1", None).await;
    let loc = h["location"].to_str().unwrap().to_string();
    assert!(
        loc.contains("&scope=identify+guilds+connections+guilds.members.read&state=v."),
        "{loc}"
    );
    // 门槛没配：普通登录不要 guilds.members.read
    let (_, h, _) = get_raw(&e, "/login", None).await;
    assert!(
        h["location"]
            .to_str()
            .unwrap()
            .contains("&scope=identify+guilds+connections&state=p.")
    );
    // 门槛配齐：普通「登录」一律带 guilds.members.read 并走验证模式；只配一半不算配齐
    set_db(&e, "register_role_guild", GUILD).await;
    let (_, h, _) = get_raw(&e, "/login", None).await;
    assert!(
        h["location"]
            .to_str()
            .unwrap()
            .contains("&scope=identify+guilds+connections&state=p."),
        "只配服务器、没配身份组 = 不限定"
    );
    require_role(&e, "900000000000000077").await;
    let (_, h, _) = get_raw(&e, "/login", None).await;
    let gated = h["location"].to_str().unwrap();
    assert!(
        gated.contains("&scope=identify+guilds+connections+guilds.members.read&state=v."),
        "{gated}"
    );
    // 改过的 state 不认；cookie 对得上的旧式 state 只能是普通登录，不会进入验证模式
    let st = loc.split("state=").nth(1).unwrap().to_string();
    let forged = st.replacen("v.", "p.", 1);
    let (_, h, _) = get_raw(&e, &format!("/login/callback?code=ok&state={forged}"), None).await;
    assert_eq!(h["location"], "/?login=failed");
    let (_, h, _) = get_raw(
        &e,
        "/login/callback?code=ok&state=v.legacy.1",
        Some("owl_login_state=v.legacy.1"),
    )
    .await;
    assert_eq!(h["location"], "/");
    assert!(!d.calls().iter().any(|c| c.ends_with("/member")));
}

#[tokio::test]
async fn legacy_cookie_only_state_is_plain_and_never_asks_discord_for_the_role() {
    let (e, d) = setup_reg(&[]).await;
    require_role(&e, "900000000000000077").await;
    // 没签名、只有 Cookie 对得上的旧式 state：当普通登录（授权时没申请 guilds.members.read），不核验也不拦
    let (s, h, _) = get_raw(
        &e,
        "/login/callback?code=ok&state=legacy.1",
        Some("owl_login_state=legacy.1"),
    )
    .await;
    assert_eq!((s, h["location"].to_str().unwrap()), (307, "/"));
    assert!(cookie_value(&h, "owl_member").contains(MEMBER));
    assert!(!d.calls().iter().any(|c| c.ends_with("/member")));
    // 但 /register 那边仍然只认核验记录
    open_registration(&e).await;
    assert_eq!(issue(&e, MEMBER).await.0, 403);
}

#[tokio::test]
async fn invite_key_binds_without_minting_and_obeys_the_closed_switch() {
    let (e, _d) = setup_reg(&[]).await;
    // 站长发的邀请 Key：带「邀请」标签、没绑定 Discord
    let invite: gate_db::repo::keys::NewKey = serde_json::from_value(json!({
        "name": "邀请-朋友", "token": "nai-invite-xyz", "daily_images": 1,
        "monthly_anlas": 0.0, "daily_text_tokens": 0, "rpm": 5,
    }))
    .unwrap();
    let row = e.state.db.create_key(&invite, e.state.now()).await.unwrap();
    let key_id = row["id"].as_i64().unwrap();
    e.state
        .db
        .write(move |tx| {
            tx.execute(
                "INSERT INTO key_tags(key_id, tag, note, by, ts) VALUES (?, '邀请', '', '站长', 0)",
                [key_id],
            )
            .map(|_| ())
        })
        .await
        .unwrap();
    assert!(svc(&e).is_unbound_invite(key_id).await.unwrap());
    let closed = bridge(
        &e,
        "/self-register/issue",
        json!({"discord_id":MEMBER,"guild_id":GUILD,"invite_key":"nai-invite-xyz"}),
    )
    .await;
    assert_eq!(closed.0, 403);
    assert!(svc(&e).is_unbound_invite(key_id).await.unwrap());
    open_registration(&e).await;
    // 写错的 Key
    let wrong = bridge(
        &e,
        "/self-register/issue",
        json!({"discord_id": MEMBER, "guild_id": GUILD, "username": "bob", "invite_key": "nai-wrong"}),
    )
    .await;
    assert_eq!(
        (wrong.0, detail(&wrong)),
        (
            403,
            "这不是一把待绑定的邀请 Key。请检查是否复制完整；普通领取请不填这一项。"
        )
    );
    // 领取开放后绑定已有凭据；回复里没有新 Key
    let ok = bridge(
        &e,
        "/self-register/issue",
        json!({"discord_id": MEMBER, "guild_id": GUILD, "username": "bob", "global_name": "Bob",
               "invite_key": "nai-invite-xyz"}),
    )
    .await;
    assert_eq!(ok.0, 200);
    assert_eq!(
        ok.2,
        json!({"message": "✅ 邀请 Key 已绑定到你的 Discord 账号，Key 不变，客户端里不用改。登录网站后可在右上角账户菜单复制 Key、查看额度；Key 泄露可用 /resetkey 重置。"})
    );
    assert_eq!(ok.1["cache-control"], "no-store");
    let bound = e
        .state
        .db
        .key_row_for_discord(MEMBER)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(bound["id"], key_id);
    assert_eq!(bound["name"], "Bob (@bob)");
    assert_eq!(bound["discord_id"], MEMBER);
    assert!(!svc(&e).is_unbound_invite(key_id).await.unwrap());
    // 别人不能再绑同一把；已绑过的人不能再绑
    let thief = bridge(
        &e,
        "/self-register/issue",
        json!({"discord_id": OTHER, "guild_id": GUILD, "invite_key": "nai-invite-xyz"}),
    )
    .await;
    assert_eq!(thief.0, 403);
    let twice = bridge(
        &e,
        "/self-register/issue",
        json!({"discord_id": MEMBER, "guild_id": GUILD, "invite_key": "nai-invite-xyz"}),
    )
    .await;
    assert_eq!(
        (twice.0, detail(&twice)),
        (
            403,
            "这个 Discord 账号已经领取过 Key 了。额度在网页登录后查看；Key 找不到的话，用 /resetkey 换一把新的。"
        )
    );
    assert!(e.state.db.get_key(key_id).await.unwrap().is_some());
    assert!(
        actions(&e)
            .await
            .iter()
            .map(action_tuple)
            .any(|a| a.1 == "绑定邀请 Key（/register）" && a.2 == format!("Key #{key_id}"))
    );
}

// ------------------------------------------------------------------ 桥接：resetkey / revoke（/quota、/help 10-23 删除）
#[tokio::test]
async fn resetkey_matches_legacy() {
    let (e, _d) = setup_reg(&[]).await;
    open_registration(&e).await;
    let r = issue(&e, MEMBER).await;
    let old_key = r.2["key"].as_str().unwrap().to_string();
    // 重置：id 不变、token 换新、旧 token 查不到
    let id = e
        .state
        .db
        .get_key_by_token(&old_key)
        .await
        .unwrap()
        .unwrap()["id"]
        .clone();
    let reset = bridge(&e, "/self-register/resetkey", who(MEMBER, MEMBER)).await;
    assert_eq!(reset.0, 200);
    assert_eq!(reset.1["cache-control"], "no-store");
    let new_key = reset.2["key"].as_str().unwrap();
    assert_ne!(new_key, old_key);
    assert!(
        e.state
            .db
            .get_key_by_token(&old_key)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        e.state.db.get_key_by_token(new_key).await.unwrap().unwrap()["id"],
        id
    );
    let none = bridge(&e, "/self-register/resetkey", who(OTHER, OTHER)).await;
    assert_eq!(none.0, 404);
    assert!(
        actions(&e)
            .await
            .iter()
            .map(action_tuple)
            .any(|a| a.1 == "成员重置 Key（/resetkey）" && a.2 == "Key #2 Alice (@alice)")
    );
}

// ------------------------------------------------------------------ 桥接：机器人配置

#[tokio::test]
async fn bot_report_keeps_only_the_heartbeat() {
    let (e, _d) = setup_reg(&[]).await;
    // 机器人拉「跑图分享设置」的接口已删除（跑图分享整个删了）
    let cfg = parse(
        Call::new("GET", "/self-register/bot/config")
            .bearer(SECRET)
            .send(&e.state)
            .await,
    );
    assert_eq!(cfg.0, 404);
    // 心跳：只留白名单字段，加上 seen_at；旧机器人带的 ai_ready / ai_today / 其它字段一律不存
    let report = bridge(
        &e,
        "/self-register/bot/report",
        json!({"status": {"user": "naimei", "guild": "测试服", "latency_ms": 42, "ready_at": 1.5,
                          "ai_ready": false, "ai_today": 0, "extra": 1}}),
    )
    .await;
    assert_eq!((report.0, report.2), (200, json!({"ok": true})));
    let stored: Value =
        serde_json::from_str(&e.state.db.get_setting("bot_status").await.unwrap().unwrap())
            .unwrap();
    assert_eq!(
        keys(&stored),
        ["user", "guild", "latency_ms", "ready_at", "seen_at"]
    );
    assert_eq!(stored["guild"], "测试服");
    // 没有 status 的上报同样 200，不动已存的心跳
    assert_eq!(
        bridge(&e, "/self-register/bot/report", json!({})).await.2,
        json!({"ok": true})
    );
    assert_eq!(
        serde_json::from_str::<Value>(
            &e.state.db.get_setting("bot_status").await.unwrap().unwrap()
        )
        .unwrap(),
        stored
    );
    // 旧机器人还会带 event：忽略，不再写 bot_events
    let ev = bridge(
        &e,
        "/self-register/bot/report",
        json!({"event": {"kind": "like", "title": "## 标题"}}),
    )
    .await;
    assert_eq!((ev.0, ev.2), (200, json!({"ok": true})));
    assert_eq!(e.state.db.get_setting("bot_events").await.unwrap(), None);
    // 鉴权不变：没带密钥 401；status 不是对象 422
    let anon = parse(
        Call::new("POST", "/self-register/bot/report")
            .json(json!({"status": {}}))
            .send(&e.state)
            .await,
    );
    assert_eq!((anon.0, anon.2), (401, json!({"detail": "未授权"})));
    let bad = bridge(&e, "/self-register/bot/report", json!({"status": "x"})).await;
    assert_eq!(
        bad.2,
        json!({"detail":"status: Input should be a valid dictionary"})
    );
}

// ------------------------------------------------------------------ 身份组 / 退群回收 / 资料补全 / 清空

#[tokio::test]
async fn member_role_is_granted_removed_and_retried() {
    let (e, d) = setup_reg(&[("DISCORD_MEMBER_ROLE_ID", "900000000000000055")]).await;
    open_registration(&e).await;
    let role_path = format!("/guilds/{GUILD}/members/{MEMBER}/roles/900000000000000055");
    assert_eq!(issue(&e, MEMBER).await.0, 200);
    let put = d.seen().into_iter().find(|s| s.method == "PUT").unwrap();
    assert_eq!(put.path, role_path);
    assert_eq!(put.auth, "Bot fake-bot-token");
    assert_eq!(put.reason, "NAI Gate key issued");
    let flag = |id: &'static str| {
        let db = e.state.db.clone();
        async move {
            db.read(move |c| {
                c.query_row(
                    "SELECT role_granted FROM discord_registrations WHERE discord_id=?",
                    [id],
                    |r| r.get::<_, i64>(0),
                )
            })
            .await
            .unwrap()
        }
    };
    assert_eq!(flag(MEMBER).await, 1);
    // 过期：同步时摘身份组；再同步是空操作（标志已清）
    let row = e
        .state
        .db
        .key_row_for_discord(MEMBER)
        .await
        .unwrap()
        .unwrap();
    let mut f = serde_json::Map::new();
    f.insert("expires_at".into(), json!(e.state.now() - 5.0));
    e.state
        .db
        .update_key(row["id"].as_i64().unwrap(), &f)
        .await
        .unwrap();
    assert_eq!(svc(&e).sync_roles().await.unwrap(), 1);
    let del = d
        .seen()
        .into_iter()
        .rfind(|s| s.method == "DELETE")
        .unwrap();
    assert_eq!(
        (del.path.as_str(), del.reason.as_str()),
        (role_path.as_str(), "NAI Gate key ended")
    );
    assert_eq!(svc(&e).sync_roles().await.unwrap(), 0);
    // 撤销时摘除失败 → 登记待办，下次同步重试
    common::claim_for(&e.state, json!({"open":true})).await;
    assert_eq!(issue(&e, OTHER).await.0, 200);
    d.script(
        "DELETE",
        &format!("/guilds/{GUILD}/members/{OTHER}/roles/900000000000000055"),
        429,
        json!({}),
    );
    assert!(svc(&e).revoke(OTHER, true).await.unwrap());
    assert_eq!(e.state.db.pending_role_removals().await.unwrap(), [OTHER]);
    d.clear_scripts();
    assert_eq!(svc(&e).sync_roles().await.unwrap(), 1);
    assert!(e.state.db.pending_role_removals().await.unwrap().is_empty());
}

#[tokio::test]
async fn sync_roles_removes_at_most_five_per_run() {
    // 待摘身份组一次堆起很多时（比如全站重置之后），每轮最多摘 5 个，剩下的留到下一轮
    let (e, d) = setup_reg(&[("DISCORD_MEMBER_ROLE_ID", "900000000000000055")]).await;
    let ids: Vec<String> = (0..7).map(|i| format!("90000000000000040{i}")).collect();
    for id in &ids {
        e.state
            .db
            .queue_role_removal(id, e.state.now())
            .await
            .unwrap();
    }
    let deletes = |d: &Discord| {
        d.calls()
            .iter()
            .filter(|c| c.starts_with("DELETE "))
            .count()
    };
    assert_eq!(svc(&e).sync_roles().await.unwrap(), 5);
    assert_eq!(deletes(&d), 5);
    assert_eq!(e.state.db.pending_role_removals().await.unwrap().len(), 2);
    assert_eq!(svc(&e).sync_roles().await.unwrap(), 2);
    assert_eq!(deletes(&d), 7);
    assert!(e.state.db.pending_role_removals().await.unwrap().is_empty());
    assert_eq!(svc(&e).sync_roles().await.unwrap(), 0);
}

#[tokio::test]
async fn failed_role_grant_does_not_block_registration() {
    let (e, d) = setup_reg(&[("DISCORD_MEMBER_ROLE_ID", "900000000000000055")]).await;
    open_registration(&e).await;
    d.script(
        "PUT",
        &format!("/guilds/{GUILD}/members/{MEMBER}/roles/900000000000000055"),
        403,
        json!({}),
    );
    assert_eq!(issue(&e, MEMBER).await.0, 200);
    // 标志先打上，之后过期同步一定会去摘
    let granted: i64 = e
        .state
        .db
        .read(|c| {
            c.query_row("SELECT role_granted FROM discord_registrations", (), |r| {
                r.get::<_, i64>(0)
            })
        })
        .await
        .unwrap();
    assert_eq!(granted, 1);
}

#[tokio::test]
async fn departed_sweep_only_revokes_confirmed_departures() {
    let (e, d) = setup_reg(&[]).await;
    common::claim_for(&e.state, json!({"max_users":0})).await;
    open_registration(&e).await;
    let ids = [
        "900000000000000201",
        "900000000000000202",
        "900000000000000203",
        "900000000000000204",
    ];
    for id in ids {
        assert_eq!(issue(&e, id).await.0, 200);
    }
    d.script(
        "GET",
        &format!("/guilds/{GUILD}/members/{}", ids[0]),
        404,
        json!({"code": 10007, "message": "Unknown Member"}),
    );
    d.script(
        "GET",
        &format!("/guilds/{GUILD}/members/{}", ids[1]),
        404,
        json!({"code": 10004, "message": "Unknown Guild"}),
    );
    d.script(
        "GET",
        &format!("/guilds/{GUILD}/members/{}", ids[2]),
        429,
        json!({"retry_after": 1}),
    );
    let now = e.state.now();
    let mut removed = 0;
    for _ in 0..4 {
        removed += svc(&e).sweep_departed(now, 1).await.unwrap();
    }
    assert_eq!(removed, 1);
    assert!(
        e.state
            .db
            .key_row_for_discord(ids[0])
            .await
            .unwrap()
            .is_none()
    );
    for id in &ids[1..] {
        assert!(e.state.db.key_row_for_discord(id).await.unwrap().is_some());
    }
    let checked: Vec<(String, f64)> = e
        .state
        .db
        .read(|c| {
            let mut s = c.prepare("SELECT discord_id, member_checked_at FROM discord_registrations ORDER BY discord_id")?;
            s.query_map((), |r| Ok((r.get(0)?, r.get(1)?)))?.collect()
        })
        .await
        .unwrap();
    let at = |id: &str| checked.iter().find(|(i, _)| i == id).unwrap().1;
    assert_eq!(at(ids[3]), now, "在服务器：今天不再查");
    assert_eq!(at(ids[1]), now - 86400.0 + 3600.0, "没查清：1 小时后重查");
    assert_eq!(at(ids[2]), now - 86400.0 + 3600.0);
    let log: Vec<_> = actions(&e).await.iter().map(action_tuple).collect();
    assert!(log.iter().any(|a| a.0 == "系统"
        && a.1 == "退群回收 Key"
        && a.2 == format!("Discord:{} Alice", ids[0])
        && a.3 == "已不在服务器，Key 已删除、名额释放"));
    // 没有私信，也没有别的请求
    assert!(d.calls().iter().all(|c| c.starts_with("GET /guilds/")));
    // 总开关
    d.clear_scripts();
    d.script(
        "GET",
        &format!("/guilds/{GUILD}/members/{}", ids[3]),
        404,
        json!({"code": 10007}),
    );
    e.state.db.set_setting("member_sweep", "0").await.unwrap();
    assert_eq!(
        svc(&e)
            .sweep_departed(now + 3.0 * 86400.0, 5)
            .await
            .unwrap(),
        0
    );
    assert!(
        e.state
            .db
            .key_row_for_discord(ids[3])
            .await
            .unwrap()
            .is_some()
    );
}

/// 邀请 Key 只由站长手动删：持有人退群时不回收，只留一条操作日志提醒。
#[tokio::test]
async fn departed_sweep_recycles_invites_using_the_same_lifecycle() {
    let (e, d) = setup_reg(&[]).await;
    common::claim_for(&e.state, json!({"max_users":0})).await;
    open_registration(&e).await;
    let invited = "900000000000000211";
    let regular = "900000000000000212";
    for id in [invited, regular] {
        assert_eq!(issue(&e, id).await.0, 200);
        d.script(
            "GET",
            &format!("/guilds/{GUILD}/members/{id}"),
            404,
            json!({"code": 10007, "message": "Unknown Member"}),
        );
    }
    let key = e
        .state
        .db
        .key_row_for_discord(invited)
        .await
        .unwrap()
        .unwrap();
    let (key_id, _key_name) = (
        key["id"].as_i64().unwrap(),
        key["name"].as_str().unwrap().to_string(),
    );
    e.state
        .db
        .run_script(vec![format!(
            "INSERT INTO key_tags (key_id, tag, note, by, ts) VALUES ({key_id}, '邀请', '', '站长', 1.0)"
        )])
        .await
        .unwrap();
    let now = e.state.now();
    let mut removed = 0;
    for _ in 0..3 {
        removed += svc(&e).sweep_departed(now, 5).await.unwrap();
    }
    assert_eq!(removed, 2, "普通和邀请凭据都回收");
    assert!(
        e.state
            .db
            .key_row_for_discord(regular)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        e.state
            .db
            .key_row_for_discord(invited)
            .await
            .unwrap()
            .is_none()
    );
    assert!(e.state.db.list_bans("").await.unwrap().is_empty());
    let log: Vec<_> = actions(&e).await.iter().map(action_tuple).collect();
    for member in [regular, invited] {
        assert!(
            log.iter()
                .any(|row| row.1 == "退群回收 Key" && row.2.contains(member)),
            "{log:?}"
        );
    }
}

#[tokio::test]
async fn sweep_treats_unreadable_404_as_unknown() {
    let (e, d) = setup_reg(&[]).await;
    let u = user("900000000000000250");
    open_registration(&e).await;
    svc(&e).issue_direct(&u.id, GUILD, &u, "").await.unwrap();
    let path = format!("/guilds/{GUILD}/members/{}", u.id);
    d.state.scripts.lock().unwrap().push(Scripted {
        method: "GET",
        path,
        status: 404,
        body: "<html>not json</html>".into(),
        once: false,
    });
    assert_eq!(svc(&e).sweep_departed(e.state.now(), 1).await.unwrap(), 0);
    assert!(
        e.state
            .db
            .key_row_for_discord(&u.id)
            .await
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn profile_backfill_names_default_keys() {
    let (e, d) = setup_reg(&[]).await;
    let bare = DiscordUser {
        id: "900000000000000260".into(),
        ..Default::default()
    };
    open_registration(&e).await;
    svc(&e)
        .issue_direct(&bare.id, GUILD, &bare, "")
        .await
        .unwrap();
    let before = e
        .state
        .db
        .key_row_for_discord(&bare.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(before["name"], "Discord:900000000000000260");
    assert_eq!(svc(&e).backfill_profiles(5).await.unwrap(), 1);
    let after = e
        .state
        .db
        .key_row_for_discord(&bare.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(after["name"], "Profile (@profile-user)");
    let p = e
        .state
        .db
        .discord_profile_row(&bare.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(p.0.as_deref(), Some("profile-user"));
    assert_eq!(
        svc(&e).backfill_profiles(5).await.unwrap(),
        0,
        "已补全的不再查"
    );
    assert_eq!(d.calls(), ["GET /users/900000000000000260"]);
}

#[tokio::test]
async fn background_jobs_do_not_panic_and_report_through_the_service() {
    let (e, d) = setup_reg(&[("DISCORD_MEMBER_ROLE_ID", "900000000000000055")]).await;
    open_registration(&e).await;
    let bare = DiscordUser {
        id: "900000000000000270".into(),
        ..Default::default()
    };
    svc(&e)
        .issue_direct(&bare.id, GUILD, &bare, "")
        .await
        .unwrap();
    gate_server::registration::registrar_jobs(&e.state).await;
    // 资料补全了；退群核对也做了（读成员，在服务器）
    let calls = d.calls();
    assert!(
        calls.contains(&"GET /users/900000000000000270".to_string()),
        "{calls:?}"
    );
    assert!(
        calls.contains(&format!("GET /guilds/{GUILD}/members/900000000000000270")),
        "{calls:?}"
    );
}

// ------------------------------------------------------------------ 机器人不私信成员

#[tokio::test]
async fn the_gateway_never_opens_a_dm_channel() {
    // 领 Key / 换 Key / 封禁 / 撤销全部走一遍：假 Discord 上不能出现私信相关的请求
    // （Discord 应用曾被标记（680009），成员私信出口整个删除）
    let (e, d) = setup_reg(&[]).await;
    open_registration(&e).await;
    assert_eq!(issue(&e, MEMBER).await.0, 200);
    assert_eq!(
        bridge(&e, "/self-register/resetkey", who(MEMBER, MEMBER))
            .await
            .0,
        200
    );
    let u = DiscordUser {
        id: "900000000000000150".into(),
        ..Default::default()
    };
    svc(&e).issue_direct(&u.id, GUILD, &u, "").await.unwrap();
    svc(&e).ban(&u.id, "", "").await.unwrap();
    assert!(svc(&e).revoke(MEMBER, true).await.unwrap());
    let seen = d.seen();
    assert!(
        seen.iter()
            .all(|s| s.path != "/users/@me/channels" && !s.path.ends_with("/messages")),
        "{seen:?}"
    );
}

// ------------------------------------------------------------------ 网页登录

fn set_cookies(h: &HeaderMap) -> Vec<String> {
    h.get_all("set-cookie")
        .iter()
        .map(|v| v.to_str().unwrap().to_string())
        .collect()
}

fn cookie_value(h: &HeaderMap, name: &str) -> String {
    set_cookies(h)
        .iter()
        .find_map(|c| c.strip_prefix(&format!("{name}=")))
        .map(|rest| rest.split(';').next().unwrap().to_string())
        .unwrap_or_default()
}

async fn get_raw(e: &Env, path: &str, cookie: Option<&str>) -> (u16, HeaderMap, Vec<u8>) {
    let mut c = Call::new("GET", path);
    if let Some(v) = cookie {
        c = c.header("cookie", v.to_string());
    }
    let r = c.send(&e.state).await;
    (r.0.as_u16(), r.1, r.2)
}

#[tokio::test]
async fn web_login_is_always_on() {
    let (e, _d) = setup_reg(&[]).await;
    // 暂停开关 web_login_paused 已删除：库里残留的值不再拦登录
    set_db(&e, "web_login_paused", "1").await;
    let (s, h, _) = get_raw(&e, "/login", None).await;
    assert_eq!(s, 307);
    assert!(
        h["location"]
            .to_str()
            .unwrap()
            .starts_with("https://discord.com/oauth2/authorize?")
    );
    assert!(!set_cookies(&h).is_empty());
}

#[tokio::test]
async fn web_login_flow_matches_legacy() {
    let (e, d) = setup_reg(&[]).await;
    // 起点：跳去 Discord 授权，带 state，并种下 state Cookie
    let (s, h, _) = get_raw(&e, "/login", None).await;
    assert_eq!(s, 307);
    let state = cookie_value(&h, "owl_login_state");
    assert!(state.starts_with("p."), "{state}");
    let loc = h["location"].to_str().unwrap().to_string();
    assert_eq!(
        loc,
        format!(
            "https://discord.com/oauth2/authorize?client_id=client-id&redirect_uri=https%3A%2F%2Fgate.test%2Flogin%2Fcallback&response_type=code&scope=identify+guilds+connections&state={state}"
        )
    );
    assert_eq!(
        set_cookies(&h),
        [format!(
            "owl_login_state={state}; HttpOnly; Max-Age=600; Path=/; SameSite=lax; Secure"
        )]
    );
    // 回调：缺参数 / state 不对 → 失败
    for path in [
        "/login/callback".to_string(),
        "/login/callback?code=c&state=zzz".to_string(),
        format!("/login/callback?state={state}"),
        "/login/callback?error=access_denied".to_string(),
    ] {
        let (s, h, _) = get_raw(&e, &path, Some(&format!("owl_login_state={state}"))).await;
        assert_eq!(
            (s, h["location"].to_str().unwrap()),
            (307, "/?login=failed"),
            "{path}"
        );
    }
    assert!(d.seen().is_empty(), "校验失败不去换授权码");
    // 成功：换授权码、查身份，种会话 Cookie、清 state Cookie
    let (s, h, _) = get_raw(
        &e,
        &format!("/login/callback?code=abc&state={state}"),
        Some(&format!("owl_login_state={state}")),
    )
    .await;
    assert_eq!((s, h["location"].to_str().unwrap()), (307, "/"));
    let cookies = set_cookies(&h);
    assert_eq!(cookies.len(), 2);
    assert!(
        cookies[0].starts_with("owl_member=")
            && cookies[0].ends_with("; HttpOnly; Max-Age=2592000; Path=/; SameSite=lax; Secure")
    );
    assert_eq!(
        cookies[1],
        "owl_login_state=\"\"; expires=Thu, 01 Jan 1970 00:00:00 GMT; Max-Age=0; Path=/; SameSite=lax"
    );
    let seen = d.seen();
    let mut calls = d.calls();
    let mut footprint = calls.split_off(2);
    footprint.sort();
    assert_eq!(calls, ["POST /oauth2/token", "GET /users/@me"]);
    // 站长 10-23：服务器列表和外部账号（scope guilds / connections）一起查，存进足迹表
    assert_eq!(
        footprint,
        ["GET /users/@me/connections", "GET /users/@me/guilds"]
    );
    let guilds = common::query(
        &e.state.db,
        "SELECT discord_id, guild_id, name FROM discord_user_guilds ORDER BY guild_id",
    )
    .await;
    assert_eq!(
        guilds,
        [
            json!({"discord_id": MEMBER, "guild_id": "900000000000000091", "name": "Owl"}),
            json!({"discord_id": MEMBER, "guild_id": "900000000000000092", "name": "Art"}),
        ]
    );
    let conns = common::query(
        &e.state.db,
        "SELECT type, ext_id, name, verified FROM discord_user_connections ORDER BY type",
    )
    .await;
    assert_eq!(
        conns,
        [
            json!({"type": "github", "ext_id": "42", "name": "", "verified": 0}),
            json!({"type": "steam", "ext_id": "900000000000000088", "name": "alice_s", "verified": 1}),
        ]
    );
    assert!(
        seen[0]
            .ctype
            .starts_with("application/x-www-form-urlencoded")
    );
    let form = seen[0].form();
    for (k, v) in [
        ("client_id", "client-id"),
        ("client_secret", "client-secret"),
        ("grant_type", "authorization_code"),
        ("code", "abc"),
        ("redirect_uri", "https%3A%2F%2Fgate.test%2Flogin%2Fcallback"),
    ] {
        assert!(form.contains(&(k.to_string(), v.to_string())), "{k}");
    }
    assert_eq!(seen[1].auth, "Bearer tok");
    // 登录记录：操作者是「昵称 (@用户名)」，对象是 Discord ID
    assert!(actions(&e).await.iter().map(action_tuple).any(|a| a
        == (
            "Alice (@alice)".into(),
            "网页登录".into(),
            format!("Discord:{MEMBER}"),
            String::new(),
            1
        )));
    // 会话 Cookie 能让 /public/me 认出人（没有 Key 时指纹是 "-"）；没领过 Key 也带着登录时记下的名字和头像
    let member_cookie = format!("owl_member={}", cookie_value(&h, "owl_member"));
    let me = parse(
        Call::new("GET", "/public/me")
            .header("cookie", member_cookie.clone())
            .send(&e.state)
            .await,
    );
    assert_eq!(
        me.2,
        json!({"logged_in": true, "discord": {"id": MEMBER, "username": "alice", "display_name": "Alice", "avatar": "a1"}, "has_key": false})
    );
    assert_eq!(me.1["cache-control"], "no-store");
    // 退出
    let out = Call::new("POST", "/logout").send(&e.state).await;
    assert_eq!(out.0, 200);
    assert_eq!(
        serde_json::from_slice::<Value>(&out.2).unwrap(),
        json!({"ok": true})
    );
    assert_eq!(
        set_cookies(&out.1),
        ["owl_member=\"\"; expires=Thu, 01 Jan 1970 00:00:00 GMT; Max-Age=0; Path=/; SameSite=lax"]
    );
}

/// 网页登录按带来的设备指纹分三种记进 web_logins：没带 = none；带了服务端没算过的 = unknown（疑似伪造，
/// 不进 device_logins）；/public/device-fp 算出来的 = ok（同时记 device_logins）。第一次登录时刻不变。
#[tokio::test]
async fn web_login_records_fingerprint_status() {
    let (e, _d) = setup_reg(&[]).await;
    let login = |dev: String| {
        let e = &e;
        async move {
            let path = if dev.is_empty() {
                "/login".to_string()
            } else {
                format!("/login?dev={dev}")
            };
            let (s, h, _) = get_raw(e, &path, None).await;
            assert_eq!(s, 307);
            let st = cookie_value(&h, "owl_login_state");
            let (s, h, _) = get_raw(
                e,
                &format!("/login/callback?code=abc&state={st}"),
                Some(&format!("owl_login_state={st}")),
            )
            .await;
            assert_eq!((s, h["location"].to_str().unwrap()), (307, "/"));
        }
    };
    let row = || {
        let db = e.state.db.clone();
        async move {
            db.read(|c| {
                let w = c.query_row(
                    "SELECT first_ts, last_ts, fp FROM web_logins WHERE discord_id=?",
                    [MEMBER],
                    |r| {
                        Ok((
                            r.get::<_, f64>(0)?,
                            r.get::<_, f64>(1)?,
                            r.get::<_, String>(2)?,
                        ))
                    },
                )?;
                let devs = c.query_row(
                    "SELECT COUNT(*) FROM device_logins WHERE discord_id=?",
                    [MEMBER],
                    |r| r.get::<_, i64>(0),
                )?;
                Ok((w, devs))
            })
            .await
            .unwrap()
        }
    };
    login(String::new()).await;
    let ((first, _, fp), devs) = row().await;
    assert_eq!((fp.as_str(), devs), ("none", 0));
    login("0123456789abcdef0123456789abcdef".into()).await;
    let ((first2, _, fp), devs) = row().await;
    assert_eq!((fp.as_str(), devs, first2), ("unknown", 0, first));
    let (s, _, b) = Call::new("POST", "/public/device-fp")
        .header("host", "gate.test")
        .header("origin", "https://gate.test")
        .json(json!({"data": {"ua": "UA", "canvas": "c1", "webgl": "w1"}}))
        .send(&e.state)
        .await;
    assert_eq!(s, 200);
    let id = serde_json::from_slice::<Value>(&b).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    login(id).await;
    let ((first3, _, fp), devs) = row().await;
    assert_eq!((fp.as_str(), devs, first3), ("ok", 1, first));
}

/// 站长 10-23：设备指纹的补充项和电量另存（指纹 id 不变）；登录时把同一设备 + 浏览器刚上报的电量记成这个账号的快照。
#[tokio::test]
async fn device_fp_extra_and_battery_are_stored_without_changing_the_id() {
    let (e, _d) = setup_reg(&[]).await;
    let bid = "0123456789abcdef0123456789abcdef";
    let post = |body: Value| {
        let e = &e;
        async move {
            let (s, _, b) = Call::new("POST", "/public/device-fp")
                .header("host", "gate.test")
                .header("origin", "https://gate.test")
                .header("cookie", format!("owl_bid={bid}"))
                .json(body)
                .send(&e.state)
                .await;
            assert_eq!(s, 200);
            serde_json::from_slice::<Value>(&b).unwrap()["id"]
                .as_str()
                .unwrap()
                .to_string()
        }
    };
    let data = json!({"ua": "UA", "canvas": "c1", "webgl": "w1"});
    let plain = post(json!({"data": data})).await;
    let with_extra = post(json!({
        "data": data,
        "extra": {"voices": "v1", "fonts": "f1", "gamut": "p3", "dpr": 2, "nested": {"x": 1}},
        "battery": {"level": 0.57, "charging": true},
    }))
    .await;
    assert_eq!(plain, with_extra, "补充项不影响指纹 id");
    let row = common::query(
        &e.state.db,
        "SELECT device_id, browser_id, data, length(xid) AS xl, battery_level, battery_charging FROM device_fp_extra",
    )
    .await;
    assert_eq!(
        row,
        [json!({
            "device_id": plain, "browser_id": bid,
            "data": r#"{"dpr":"2","fonts":"f1","gamut":"p3","voices":"v1"}"#, "xl": 16,
            "battery_level": 0.57, "battery_charging": 1,
        })]
    );
    // 登录：带着这个指纹和浏览器标记，电量记成这个账号的快照
    let (_, h, _) = get_raw(&e, &format!("/login?dev={plain}"), None).await;
    let st = cookie_value(&h, "owl_login_state");
    let (s, _, _) = get_raw(
        &e,
        &format!("/login/callback?code=abc&state={st}"),
        Some(&format!("owl_login_state={st}; owl_bid={bid}")),
    )
    .await;
    assert_eq!(s, 307);
    let bat = common::query(
        &e.state.db,
        "SELECT discord_id, level, charging FROM device_battery",
    )
    .await;
    assert_eq!(
        bat,
        [json!({"discord_id": MEMBER, "level": 0.57, "charging": 1})]
    );
    // 电量不合格（超出 0–1）就不记电量
    post(json!({"data": data, "battery": {"level": 7, "charging": false}})).await;
    let n = common::query(&e.state.db, "SELECT battery_level FROM device_fp_extra").await;
    assert_eq!(n, [json!({"battery_level": 0.57})], "不合格的电量不覆盖");
}

/// 带着浏览器标记 Cookie（owl_bid）登录：记进 browser_logins（同账号同浏览器一行，刷新时间）；
/// 格式不对的 Cookie 不记；定期清理按 90 天清掉旧记录。
#[tokio::test]
async fn web_login_records_browser_id() {
    let (e, _d) = setup_reg(&[]).await;
    let bid = "0123456789abcdef0123456789abcdef";
    let login = |extra: &str| {
        let e = &e;
        let extra = extra.to_string();
        async move {
            let (_, h, _) = get_raw(e, "/login", None).await;
            let st = cookie_value(&h, "owl_login_state");
            let (s, h, _) = get_raw(
                e,
                &format!("/login/callback?code=abc&state={st}"),
                Some(&format!("owl_login_state={st}; {extra}")),
            )
            .await;
            assert_eq!((s, h["location"].to_str().unwrap()), (307, "/"));
        }
    };
    let rows = || {
        let db = e.state.db.clone();
        async move {
            db.read(|c| {
                let mut st = c.prepare(
                    "SELECT discord_id, browser_id, ts FROM browser_logins ORDER BY browser_id",
                )?;
                let v = st
                    .query_map([], |r| {
                        Ok((
                            r.get::<_, String>(0)?,
                            r.get::<_, String>(1)?,
                            r.get::<_, f64>(2)?,
                        ))
                    })?
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(v)
            })
            .await
            .unwrap()
        }
    };
    login("").await;
    assert!(rows().await.is_empty());
    login("owl_bid=NOT-A-BID").await;
    assert!(rows().await.is_empty());
    login(&format!("owl_bid={bid}")).await;
    let r = rows().await;
    assert_eq!(r.len(), 1);
    assert_eq!((r[0].0.as_str(), r[0].1.as_str()), (MEMBER, bid));
    login(&format!("owl_bid={bid}")).await;
    assert_eq!(rows().await.len(), 1, "同账号同浏览器只留一行");
    // 定期清理：90 天内的留着，更早的清掉
    let now = e.state.now();
    e.state
        .db
        .record_browser_login(
            "900000000000000099",
            &"f".repeat(32),
            "",
            now - 91.0 * 86400.0,
        )
        .await
        .unwrap();
    assert_eq!(rows().await.len(), 2);
    // 后台记录一律留存（站长 10-22）：维护任务不再清浏览器登录记录
    gate_server::jobs::purge(&e.state).await.unwrap();
    assert_eq!(rows().await.len(), 2);
}

/// 网页登录时和封禁号用的是同一个浏览器：待审里出一条「封禁号关联」，/register 被拦。
#[tokio::test]
async fn web_login_next_to_a_banned_account_is_flagged_and_blocked() {
    let (e, _d) = setup_reg(&[]).await;
    open_registration(&e).await;
    let bid = "0123456789abcdef0123456789abcdef";
    let banned = "900000000000000098";
    let now = e.state.now();
    e.state
        .db
        .run_script(vec![format!(
            "INSERT INTO discord_bans(discord_id, created_at, reason) VALUES ('{banned}', {now}, '')"
        )])
        .await
        .unwrap();
    e.state
        .db
        .record_browser_login(banned, bid, "", now)
        .await
        .unwrap();
    let (_, h, _) = get_raw(&e, "/login", None).await;
    let st = cookie_value(&h, "owl_login_state");
    let (s, _, _) = get_raw(
        &e,
        &format!("/login/callback?code=abc&state={st}"),
        Some(&format!("owl_login_state={st}; owl_bid={bid}")),
    )
    .await;
    assert_eq!(s, 307);
    let rows = common::query(
        &e.state.db,
        "SELECT kind, subject, score, status FROM abuse_signals",
    )
    .await;
    assert_eq!(
        rows,
        vec![
            json!({"kind": "banned_link", "subject": format!("{MEMBER},{banned}"), "score": 95.0, "status": "open"})
        ]
    );
    assert_eq!(
        svc(&e).issue_direct(MEMBER, GUILD, &user(MEMBER), "").await,
        Err(RegErr::Denied(gate_server::ban_link::BLOCK_NOTICE.into()))
    );
}

#[tokio::test]
async fn web_login_rejections() {
    let (e, d) = setup_reg(&[]).await;
    require_role(&e, "900000000000000077").await;
    let ck = "owl_login_state=st";
    // 授权码被 Discord 拒绝
    d.script(
        "POST",
        "/oauth2/token",
        400,
        json!({"error": "invalid_grant"}),
    );
    let (s, h, _) = get_raw(&e, "/login/callback?code=bad&state=st", Some(ck)).await;
    assert_eq!(
        (s, h["location"].to_str().unwrap()),
        (307, "/?login=failed")
    );
    // 配了身份组门槛时，普通登录也不查成员、不拒绝（只有「验证身份组」才查）
    d.clear_scripts();
    let (s, h, _) = get_raw(&e, "/login/callback?code=ok&state=st", Some(ck)).await;
    assert_eq!((s, h["location"].to_str().unwrap()), (307, "/"));
    assert_eq!(set_cookies(&h).len(), 2);
    assert!(!d.calls().iter().any(|c| c.ends_with("/member")));
}

#[tokio::test]
async fn member_session_dies_when_the_key_is_reset() {
    let (e, _d) = setup_reg(&[]).await;
    open_registration(&e).await;
    issue(&e, MEMBER).await;
    let (_, h, _) = get_raw(
        &e,
        "/login/callback?code=ok&state=st",
        Some("owl_login_state=st"),
    )
    .await;
    let cookie = format!("owl_member={}", cookie_value(&h, "owl_member"));
    let me = |c: &str| {
        let c = c.to_string();
        let st = e.state.clone();
        async move {
            parse(
                Call::new("GET", "/public/me")
                    .header("cookie", c)
                    .send(&st)
                    .await,
            )
        }
    };
    let before = me(&cookie).await;
    assert_eq!(before.2["logged_in"], true);
    assert_eq!(before.2["has_key"], true);
    // /resetkey 换了 Key，旧会话立刻失效
    assert_eq!(
        bridge(&e, "/self-register/resetkey", who(MEMBER, MEMBER))
            .await
            .0,
        200
    );
    assert_eq!(me(&cookie).await.2, json!({"logged_in": false}));
    // 篡改过的 Cookie 也不行
    assert_eq!(
        me("owl_member=900000000000000005:9999999999:-.deadbeef")
            .await
            .2,
        json!({"logged_in": false})
    );
}

// ------------------------------------------------------------------ 个人状态 / 重置券

async fn login_member(e: &Env, id: &str) -> String {
    let exp = e.state.now() as i64 + 3600;
    let key = match e.state.db.key_row_for_discord(id).await.unwrap() {
        Some(row) => Some(gate_server::key::Key::load(&e.state.db, row).await.unwrap()),
        None => None,
    };
    let tag = gate_server::session::key_tag(key.as_ref());
    format!(
        "owl_member={}",
        gate_server::session::sign_member(&e.state, id, exp, &tag)
    )
}

#[tokio::test]
async fn public_me_matches_legacy_and_coupons_work() {
    let (e, _d) = setup_reg(&[]).await;
    open_registration(&e).await;
    let anon = parse(Call::new("GET", "/public/me").send(&e.state).await);
    assert_eq!((anon.0, anon.2), (200, json!({"logged_in": false})));
    assert_eq!(anon.1["cache-control"], "no-store");
    issue(&e, MEMBER).await;
    let cookie = login_member(&e, MEMBER).await;
    let me = parse(
        Call::new("GET", "/public/me")
            .header("cookie", cookie.clone())
            .send(&e.state)
            .await,
    );
    assert_eq!(me.0, 200);
    assert_eq!(
        keys(&me.2),
        [
            "logged_in",
            "discord",
            "has_key",
            "key",
            "name",
            "expires_at",
            "policy",
            "today",
            "queue",
            "coupons",
            "timing"
        ]
    );
    let key = e
        .state
        .db
        .key_row_for_discord(MEMBER)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(me.2["key"], key["token"]);
    assert_eq!(me.2["name"], "Alice (@alice)");
    assert_eq!(me.2["policy"]["daily_images"], 150);
    assert_eq!(me.2["policy"]["v5"], false);
    assert_eq!(
        me.2["discord"],
        json!({"id": MEMBER, "username": "alice", "display_name": "Alice", "avatar": "a1b2"})
    );
    assert_eq!(me.2["today"]["images"], 0);
    assert_eq!(me.2["today"]["daily_images"], 150);
    assert_eq!(me.2["today"]["daily_v5"], 8);
    assert_eq!(me.2["today"]["v5_access_allowed"], false);
    assert_eq!(me.2["today"]["reset_hour"], 7);
    assert!(me.2["today"].get("quota_limits").is_none());
    assert_eq!(me.2["queue"], json!([]));
    assert_eq!(me.2["coupons"], json!([]));
    assert_eq!(
        me.2["timing"],
        json!({"count": 0, "avg_dur_s": null, "avg_wait_s": null, "site_avg_dur_s": 9.0})
    );
    // 被停用的 Key：只说没有 Key
    // 券：别人的 / 过期的 / 用过的都不行
    let key_id = key["id"].as_i64().unwrap();
    let c = e
        .state
        .db
        .add_coupon(
            key_id,
            "reset",
            7.0,
            "唱得真好听",
            "奶妹",
            "v45,v5,anlas",
            e.state.now(),
        )
        .await
        .unwrap();
    let me = parse(
        Call::new("GET", "/public/me")
            .header("cookie", cookie.clone())
            .send(&e.state)
            .await,
    );
    assert_eq!(me.2["coupons"][0]["note"], "唱得真好听");
    assert_eq!(me.2["coupons"][0]["by"], "奶妹");
    let use_path = format!("/public/me/coupons/{}/use", c.id);
    // 先用掉一些额度，确认用券会重置
    common::add_usage(&e.state.db, key_id, &e.state.day(), 3, 9.0).await;
    let day = e.state.day();
    e.state
        .db
        .write(move |tx| {
            tx.execute(
                "UPDATE counters SET legacy_free_images=7 WHERE key_id=? AND day=?",
                (key_id, &day),
            )?;
            Ok(())
        })
        .await
        .unwrap();
    let anon = parse(Call::new("POST", &use_path).send(&e.state).await);
    assert_eq!(
        (anon.0, anon.2),
        (
            401,
            json!({"detail": "未登录或登录已过期，请刷新页面后重新登录。"})
        )
    );
    let used = parse(
        Call::new("POST", &use_path)
            .header("cookie", cookie.clone())
            .send(&e.state)
            .await,
    );
    assert_eq!((used.0, used.2), (200, json!({"ok": true})));
    assert_eq!(used.1["cache-control"], "no-store");
    let counter = e
        .state
        .db
        .get_counter(key_id, &e.state.day())
        .await
        .unwrap();
    assert_eq!(
        (counter.legacy_free_images, counter.v5, counter.anlas),
        (0, 0, 0.0),
        "用券重置今天的 V4.5、V5 与 Anlas"
    );
    let again = parse(
        Call::new("POST", &use_path)
            .header("cookie", cookie.clone())
            .send(&e.state)
            .await,
    );
    assert_eq!(
        (again.0, again.2),
        (409, json!({"detail": "该券已使用或已过期，额度未变更。"}))
    );
    // 默认的券（站长 10-18）只重置 V4.5：V5 和 Anlas 的用量留着
    common::add_usage(&e.state.db, key_id, &e.state.day(), 2, 1.0).await;
    let day = e.state.day();
    e.state
        .db
        .write(move |tx| {
            tx.execute(
                "UPDATE counters SET legacy_free_images=legacy_free_images+4 WHERE key_id=? AND day=?",
                (key_id, &day),
            )?;
            Ok(())
        })
        .await
        .unwrap();
    let only45 = e
        .state
        .db
        .add_coupon(key_id, "reset", 7.0, "", "站长", "v45", e.state.now())
        .await
        .unwrap();
    let c = e
        .state
        .db
        .get_counter(key_id, &e.state.day())
        .await
        .unwrap();
    assert_eq!((c.legacy_free_images, c.v5, c.anlas), (4, 2, 1.0));
    let used = parse(
        Call::new("POST", &format!("/public/me/coupons/{}/use", only45.id))
            .header("cookie", cookie.clone())
            .send(&e.state)
            .await,
    );
    assert_eq!((used.0, used.2), (200, json!({"ok": true})));
    let c = e
        .state
        .db
        .get_counter(key_id, &e.state.day())
        .await
        .unwrap();
    assert_eq!(
        (c.legacy_free_images, c.v5, c.anlas),
        (0, 2, 1.0),
        "只写 V4.5 的券不动 V5 和 Anlas"
    );
    let bad = parse(
        Call::new("POST", "/public/me/coupons/abc/use")
            .header("cookie", cookie.clone())
            .send(&e.state)
            .await,
    );
    assert_eq!(bad.2, json!({"detail":"coupon_id 必须是整数"}));
    let log: Vec<_> = actions(&e).await.iter().map(action_tuple).collect();
    assert!(log.contains(&(
        format!("Discord:{MEMBER}"),
        "成员使用重置券".into(),
        "Alice (@alice)".into(),
        "券 #1（唱得真好听）".into(),
        1
    )));
    // 没有 Key 的会话不能用券
    let nokey = login_member(&e, OTHER).await;
    let r = parse(
        Call::new("POST", &use_path)
            .header("cookie", nokey)
            .send(&e.state)
            .await,
    );
    assert_eq!((r.0, r.2), (404, json!({"detail": "你还没有可用的 Key"})));
    // Key 被停用后个人页只显示没有 Key
    let mut f = serde_json::Map::new();
    f.insert("enabled".into(), json!(0));
    e.state.db.update_key(key_id, &f).await.unwrap();
    let off = parse(
        Call::new("GET", "/public/me")
            .header("cookie", cookie)
            .send(&e.state)
            .await,
    );
    assert_eq!(
        off.2,
        json!({"logged_in": true, "discord": {"id": MEMBER, "username": "alice",
        "display_name": "Alice", "avatar": "a1b2"}, "has_key": false})
    );
}

// ------------------------------------------------------------------ 已移除的捐赠入口

#[tokio::test]
async fn public_donation_endpoint_is_removed() {
    let (e, _d) = setup_reg(&[]).await;
    let response = parse(
        Call::new("POST", "/public/donate")
            .json(json!({"token": "synthetic", "message": "synthetic"}))
            .send(&e.state)
            .await,
    );
    assert_eq!(response.0, 404);
}

// ------------------------------------------------------------------ 没有请求会发到真实 Discord

#[tokio::test]
async fn discord_base_comes_from_the_environment() {
    // 默认值是官方 v10；测试环境一律通过 DISCORD_API_BASE 换成本机假服务
    let plain =
        gate_server::env::EnvConfig::load(&std::collections::HashMap::<String, String>::new());
    assert_eq!(plain.discord_api_base, "https://discord.com/api/v10");
    // 跑一遍会碰 Discord 的流程：每个请求都落在假服务上
    let (e, d) = setup_reg(&[("DISCORD_MEMBER_ROLE_ID", "900000000000000055")]).await;
    require_role(&e, "900000000000000077").await;
    open_registration(&e).await;
    d.script(
        "GET",
        &member_path(GUILD),
        200,
        json!({"roles": ["900000000000000077"]}),
    );
    assert_eq!(verify_login(&e).await.0, 307);
    assert_eq!(issue(&e, MEMBER).await.0, 200);
    assert!(svc(&e).revoke(MEMBER, true).await.unwrap());
    let calls = d.calls();
    assert!(calls.len() >= 3, "{calls:?}");
    assert!(
        d.seen()
            .iter()
            .all(|s| s.auth == "Bot fake-bot-token" || s.auth == "Bearer tok" || s.auth.is_empty())
    );
}

#[tokio::test]
async fn issue_result_enum_is_provisioned_for_plain_requests() {
    let (e, _d) = setup_reg(&[]).await;
    open_registration(&e).await;
    let r = svc(&e)
        .issue_direct(MEMBER, GUILD, &user(MEMBER), "")
        .await
        .unwrap();
    assert!(matches!(r, Issued::Provisioned(_)));
    // 非数字 ID 直接拒绝，不查库
    let bad = svc(&e).issue_direct("12a", GUILD, &user("12a"), "").await;
    assert_eq!(
        bad,
        Err(RegErr::Denied("请在指定服务器使用 /register。".into()))
    );
}

// ------------------------------------------------------------------ 成员名 / 打包

/// 成员名的写法：显示名和用户名都有且不同 → `显示名 (@用户名)`；只有一个，或两个一样 → 就是它；
/// 都没有 → `Discord:<id>`；超过 60 个字符时从后面截断（显示名优先保留）。
#[test]
fn member_label_writing() {
    const ID: &str = "900000000000000001";
    let named = |username: &str, global_name: &str| DiscordUser {
        id: ID.into(),
        username: username.into(),
        global_name: global_name.into(),
        avatar: String::new(),
    };
    let cases = [
        (named("neo", "Neo"), "Neo (@neo)".to_string()),
        (named("neo", "neo"), "neo".to_string()),
        (named("neo", ""), "neo".to_string()),
        (named("", "Neo"), "Neo".to_string()),
        (named("", ""), format!("Discord:{ID}")),
        // 资料里压根没有这两个字段
        (
            DiscordUser {
                id: ID.into(),
                ..Default::default()
            },
            format!("Discord:{ID}"),
        ),
        // 超长：显示名占满 60 个字符，用户名被截掉
        (named(&"u".repeat(90), &"d".repeat(90)), "d".repeat(60)),
        (named(&"u".repeat(90), ""), "u".repeat(60)),
        // 按字符数而不是字节数截断：40 个字符的显示名 + ` (@` + 17 个字符的用户名，正好 60 个，右括号被截掉
        (
            named(&"猫".repeat(70), &"头鹰".repeat(20)),
            format!("{} (@{}", "头鹰".repeat(20), "猫".repeat(17)),
        ),
    ];
    assert_eq!(cases.len(), 9);
    for (user, want) in cases {
        assert_eq!(
            gate_server::registration::member_label(&user),
            want,
            "{user:?}"
        );
    }
}

// ------------------------------------------------------------------ 设置：名额 × V5 提示、校验、欢迎文案变体、记录开关（对拍旧版 ops.py）

#[tokio::test]
async fn registration_settings_use_only_claim_conditions_and_defaults_are_independent() {
    let (env, _discord) = setup_reg(&[]).await;
    let body = json!({"open":true,"max_users":50,"expires_days":0,"roles":[]});
    gate_server::ops::set_registration(&env.state, body.as_object().unwrap())
        .await
        .unwrap();
    common::defaults_for(&env.state, json!({"daily_images":100,"v5":false})).await;
    let issued = issue(&env, "900000000000000401").await;
    assert_eq!(issued.0, 200, "{:?}", issued.2);
    assert!(issued.2["message"].as_str().unwrap().contains("100 张"));
    let policy = env
        .state
        .db
        .member_policy("900000000000000401")
        .await
        .unwrap()
        .effective;
    assert_eq!(policy.daily_images, 100);
    assert!(!policy.v5);
    let stored = env.state.db.claim_rules().await.unwrap();
    assert!(stored.open);
    assert_eq!(stored.max_users, 50);
    assert!(
        gate_server::ops::set_registration(
            &env.state,
            json!({"role_guild":"bad"}).as_object().unwrap()
        )
        .await
        .is_err()
    );
}

#[tokio::test]
async fn trailing_slash_in_api_base_is_tolerated() {
    let d = Discord::start().await;
    let base = format!("{}/", d.url());
    let e = setup_registration(&base, &[]).await;
    require_role(&e, "900000000000000077").await;
    open_registration(&e).await;
    d.script(
        "GET",
        &member_path(GUILD),
        200,
        json!({"roles": ["900000000000000077"]}),
    );
    assert_eq!(verify_login(&e).await.0, 307);
    assert_eq!(issue(&e, MEMBER).await.0, 200);
    assert_eq!(
        d.calls()[..3],
        [
            "POST /oauth2/token".to_string(),
            "GET /users/@me".to_string(),
            format!("GET /users/@me/guilds/{GUILD}/member")
        ]
    );
}

#[tokio::test]
async fn public_meta_exposes_only_the_role_note_when_the_gate_is_configured() {
    async fn fetch(e: &Env, path: &str) -> Value {
        let r = Call::new("GET", path).send(&e.state).await;
        serde_json::from_slice::<Value>(&r.2).unwrap()
    }
    // status / live 的 meta 带短缓存，每种设置各用一个全新的环境
    // 没配门槛：registration 里没有这个字段
    let (e, _d) = setup_reg(&[]).await;
    assert!(
        fetch(&e, "/public/status").await["registration"]
            .get("role_note")
            .is_none()
    );
    assert!(
        fetch(&e, "/public/live").await["registration"]
            .get("role_note")
            .is_none()
    );
    // 只配一半不算门槛
    let (e, _d) = setup_reg(&[]).await;
    set_db(&e, "register_role_note", "创作者").await;
    set_db(&e, "register_role_guild", GUILD).await;
    assert!(
        fetch(&e, "/public/status").await["registration"]
            .get("role_note")
            .is_none()
    );
    assert!(
        fetch(&e, "/public/live").await["registration"]
            .get("role_note")
            .is_none()
    );
    // 配齐：只给名称，不给服务器 / 身份组 ID
    let (e, _d) = setup_reg(&[]).await;
    common::claim_for(
        &e.state,
        json!({"roles":[{"guild":GUILD,"role":"900000000000000077","note":"创作者"}]}),
    )
    .await;
    for path in ["/public/status", "/public/live"] {
        let body = fetch(&e, path).await;
        assert_eq!(body["registration"]["role_note"], "创作者", "{path}");
        let all = body.to_string();
        assert!(!all.contains("900000000000000077") && !all.contains(GUILD));
    }
}

// ------------------------------------------------------------------ 进服时间 / 成员标记位（防小号）

/// 2024-01-02T03:04:05.678Z
const JOINED_2024: f64 = 1_704_164_645.678;
/// 2023-06-01T00:00:00Z
const JOINED_2023: f64 = 1_685_577_600.0;

#[test]
fn member_join_parses_discord_member_objects() {
    use gate_server::registration::MemberJoin;
    let m = MemberJoin::from_member(
        &json!({"joined_at": "2024-01-02T03:04:05.678000+00:00", "flags": 3, "roles": []}),
    );
    assert_eq!(
        m,
        MemberJoin {
            joined_at: Some(JOINED_2024),
            flags: 3
        }
    );
    assert_eq!(
        MemberJoin::from_member(&json!({"joined_at": "2023-06-01T08:00:00+08:00"})).joined_at,
        Some(JOINED_2023)
    );
    // 缺字段 / 读不出：None 和 0
    assert_eq!(
        MemberJoin::from_member(&json!({"joined_at": null})),
        MemberJoin::default()
    );
    assert_eq!(
        MemberJoin::from_member(&json!({"joined_at": "yesterday", "flags": "x"})),
        MemberJoin::default()
    );
    let merged = MemberJoin {
        joined_at: Some(JOINED_2024),
        flags: 2,
    }
    .merge(MemberJoin {
        joined_at: Some(JOINED_2023),
        flags: 1,
    })
    .merge(MemberJoin::default());
    assert_eq!(
        merged,
        MemberJoin {
            joined_at: Some(JOINED_2023),
            flags: 3
        }
    );
}

#[tokio::test]
async fn verify_login_records_join_time_and_flags_for_registered_members() {
    let (e, d) = setup_reg(&[]).await;
    require_role(&e, ROLE_A).await;
    open_registration(&e).await;
    d.script(
        "GET",
        &member_path(GUILD),
        200,
        json!({"roles": [ROLE_A], "joined_at": "2024-01-02T03:04:05.678000+00:00", "flags": 1}),
    );
    // 还没领 Key：没有领取记录，什么都不存
    assert_eq!(verify_login(&e).await.0, 307);
    assert_eq!(e.state.db.get_member_join(MEMBER).await.unwrap(), None);
    assert_eq!(issue(&e, MEMBER).await.0, 200);
    assert_eq!(
        e.state.db.get_member_join(MEMBER).await.unwrap(),
        Some((None, 0))
    );
    // 领了 Key 之后再登录：记下进服时间和「重新加入」标记
    assert_eq!(verify_login(&e).await.0, 307);
    assert_eq!(
        e.state.db.get_member_join(MEMBER).await.unwrap(),
        Some((Some(JOINED_2024), 1))
    );
}

#[tokio::test]
async fn verify_login_keeps_the_earliest_join_across_two_guilds() {
    let (e, d) = setup_reg(&[]).await;
    two_rules(&e).await;
    e.state
        .db
        .upsert_role_verification(MEMBER, GUILD, ROLE_A, e.state.now())
        .await
        .unwrap();
    assert_eq!(issue(&e, MEMBER).await.0, 200);
    d.script(
        "GET",
        &member_path(GUILD),
        200,
        json!({"roles": [ROLE_A], "joined_at": "2024-01-02T03:04:05.678+00:00", "flags": 2}),
    );
    d.script(
        "GET",
        &member_path(GUILD_B),
        200,
        json!({"roles": [], "joined_at": "2023-06-01T00:00:00+00:00", "flags": 1}),
    );
    assert_eq!(verify_login(&e).await.0, 307);
    assert_eq!(
        e.state.db.get_member_join(MEMBER).await.unwrap(),
        Some((Some(JOINED_2023), 3)),
        "最早的进服时间、标记位按位或"
    );
    // 之后第二个服务器不在了（404）、第一个服务器的进服时间更晚：已存的最早时间不被改晚
    d.clear_scripts();
    d.script(
        "GET",
        &member_path(GUILD),
        200,
        json!({"roles": [ROLE_A], "joined_at": "2025-01-01T00:00:00+00:00", "flags": 0}),
    );
    d.script("GET", &member_path(GUILD_B), 404, json!({"code": 10007}));
    assert_eq!(verify_login(&e).await.0, 307);
    assert_eq!(
        e.state.db.get_member_join(MEMBER).await.unwrap(),
        Some((Some(JOINED_2023), 3))
    );
}

#[tokio::test]
/// 机器人巡查的是本站自己的服，不是门槛服：它的进服时间不能拿来算「进服多久」，一律不记。
async fn departed_sweep_does_not_record_the_bot_guild_join_time() {
    let (e, d) = setup_reg(&[]).await;
    common::claim_for(&e.state, json!({"max_users":0})).await;
    open_registration(&e).await;
    let ids = [
        "900000000000000301",
        "900000000000000302",
        "900000000000000303",
    ];
    for id in ids {
        assert_eq!(issue(&e, id).await.0, 200);
        assert_eq!(
            e.state.db.get_member_join(id).await.unwrap(),
            Some((None, 0))
        );
    }
    d.script(
        "GET",
        &format!("/guilds/{GUILD}/members/{}", ids[0]),
        200,
        json!({"roles": [], "joined_at": "2024-01-02T03:04:05.678000+00:00", "flags": 1}),
    );
    d.script(
        "GET",
        &format!("/guilds/{GUILD}/members/{}", ids[1]),
        200,
        json!({"roles": [], "joined_at": "2023-06-01T00:00:00+00:00", "flags": 0}),
    );
    d.script(
        "GET",
        &format!("/guilds/{GUILD}/members/{}", ids[2]),
        429,
        json!({"retry_after": 1}),
    );
    let now = e.state.now();
    for _ in 0..3 {
        svc(&e).sweep_departed(now, 1).await.unwrap();
    }
    let db = &e.state.db;
    for id in ids {
        assert_eq!(
            db.get_member_join(id).await.unwrap(),
            Some((None, 0)),
            "{id}"
        );
    }
    // 每人只读了一次成员，没有额外请求
    let calls = d.calls();
    assert_eq!(calls.len(), 3, "{calls:?}");
    assert!(calls.iter().all(|c| c.starts_with("GET /guilds/")));
}

/// 「我的请求」：只看得到自己上次重置以来的请求，不带 IP / 客户端 / 上游账号；站付的 Anlas 不算成员花的；
/// 拒绝说明里的状态码单独给出；能按状态筛、按 id 往回翻页。
#[tokio::test]
async fn my_requests_show_only_my_recent_rows_without_private_columns() {
    let (e, _d) = setup_reg(&[]).await;
    open_registration(&e).await;
    let anon = parse(Call::new("GET", "/public/me/requests").send(&e.state).await);
    assert_eq!(anon.0, 401);
    issue(&e, MEMBER).await;
    issue(&e, OTHER).await;
    let mine = e
        .state
        .db
        .key_row_for_discord(MEMBER)
        .await
        .unwrap()
        .unwrap()["id"]
        .as_i64()
        .unwrap();
    let theirs = e
        .state
        .db
        .key_row_for_discord(OTHER)
        .await
        .unwrap()
        .unwrap()["id"]
        .as_i64()
        .unwrap();
    let now = e.state.now();
    // 只看上次重置（服务日起点）以来的：可见的几条按比例落在「起点 → 现在」之间，起点前的看不到
    let ds = e.state.clock.day_start(now);
    let at = |f: f64| ds + (now - ds) * f;
    let rows: Vec<(i64, f64, &str, f64, &str, &str)> = vec![
        (mine, ds - 3600.0, "ok", 0.0, "", "old"),
        (mine, at(0.2), "ok", 30.0, "", "832x1216/28step est=30A"),
        (
            mine,
            at(0.4),
            "ok",
            30.0,
            "",
            "832x1216/28step; 站付V5 30A（账号 Anlas 付费，不计入成员）",
        ),
        (
            mine,
            at(0.6),
            "rejected",
            0.0,
            "member_quota",
            "402 你今天的 Anlas 额度不够这次用",
        ),
        (theirs, at(0.8), "ok", 0.0, "", "not mine"),
    ];
    for (key_id, ts, status, anlas, reason, detail) in rows {
        let (status, reason, detail) = (status.to_string(), reason.to_string(), detail.to_string());
        e.state
            .db
            .write(move |tx| {
                tx.execute(
                    "INSERT INTO usage_log (ts, key_id, key_name, kind, model, status, images, anlas, \
                     detail, reason, src, client, token_id, wait_ms, dur_ms, rid) \
                     VALUES (?,?,'k','image','nai-diffusion-5-full',?,1,?,?,?,'192.0.2.9','curl/8','token-x',1500,9000,'edae7aa9')",
                    (ts, key_id, &status, anlas, &detail, &reason),
                )?;
                Ok(())
            })
            .await
            .unwrap();
    }
    let cookie = login_member(&e, MEMBER).await;
    let get = |q: &str| {
        let (cookie, path) = (cookie.clone(), format!("/public/me/requests{q}"));
        let state = e.state.clone();
        async move {
            parse(
                Call::new("GET", &path)
                    .header("cookie", cookie)
                    .send(&state)
                    .await,
            )
        }
    };
    let r = get("").await;
    assert_eq!(r.0, 200, "{}", r.2);
    assert_eq!(r.1["cache-control"], "no-store");
    let items = r.2["items"].as_array().unwrap();
    assert_eq!(
        items.len(),
        3,
        "rows before the last reset and other members' rows are hidden"
    );
    for it in items {
        assert_eq!(
            it["rid"], "edae7aa9",
            "request id shown to the member: {it}"
        );
        for private in ["src", "client", "token_id", "key_name"] {
            assert!(it.get(private).is_none(), "{private} leaked: {it}");
        }
    }
    assert_eq!(items[0]["status"], "rejected");
    assert_eq!(items[0]["code"], 402);
    assert_eq!(items[0]["message"], "你今天的 Anlas 额度不够这次用");
    assert_eq!(items[1]["site_paid"], true);
    assert_eq!(items[1]["anlas"], 0.0);
    assert_eq!(items[2]["anlas"], 30.0);
    assert_eq!(items[2]["wait_s"], 1.5);
    assert_eq!(items[2]["dur_s"], 9.0);
    assert_eq!(r.2["next"], Value::Null);
    assert_eq!(
        r.2["summary"],
        json!({"total": 3, "ok": 2, "rejected": 1, "error": 0})
    );
    let only = get("?status=rejected").await;
    assert_eq!(only.2["items"].as_array().unwrap().len(), 1);
    let before = items[0]["id"].as_i64().unwrap();
    let older = get(&format!("?before={before}")).await;
    assert_eq!(older.2["items"].as_array().unwrap().len(), 2);
}
