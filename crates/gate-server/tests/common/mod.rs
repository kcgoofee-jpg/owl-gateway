//! 网关集成测试的公共部分：临时数据目录 + 假上游 + 一把成员 Key。
#![allow(dead_code)]

use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{HeaderMap, Request, StatusCode};
use gate_db::repo::keys::NewKey;
use gate_server::AppState;
use gate_server::env::EnvConfig;
use gate_server::web::Assets;
use gate_testkit::MockUpstream;
use http_body_util::BodyExt;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::net::SocketAddr;
use tower::ServiceExt;

pub const TOKEN: &str = "nai-test-member-key-000000000000000000";
pub const ADMIN_PASSWORD: &str = "dev-admin-password-123";

pub struct Env {
    pub state: AppState,
    pub mock: MockUpstream,
    vars: HashMap<String, String>,
    static_dir: Option<std::path::PathBuf>,
    _dir: tempfile::TempDir,
}

fn env_config(vars: &HashMap<String, String>) -> EnvConfig {
    let mut env = EnvConfig::load(vars);
    // 这两项是代码常量（不读环境变量），测试里直接改：出完不歇，排队 5 秒就算超时
    env.pace_images = false;
    env.queue_timeout = 5;
    env
}

impl Env {
    /// 模拟重启：用同一个数据目录、同样的环境变量重新建一份 `AppState`（内存里的缓存、计数都是空的）。
    pub async fn restart(&self) -> AppState {
        AppState::build(env_config(&self.vars), Assets::new(self.static_dir.clone()))
            .await
            .unwrap()
    }
}

pub async fn setup(extra: &[(&str, &str)]) -> Env {
    setup_with_static(extra, None).await
}

/// 同 [`setup`]；`landing_html` 给了就在临时目录里放一份 `landing.html` 当静态目录
/// （`web/dist` 在测试里不一定构建过）。
pub async fn setup_with_static(extra: &[(&str, &str)], landing_html: Option<&str>) -> Env {
    let mock = MockUpstream::start().await;
    let dir = tempfile::tempdir().unwrap();
    let mut vars: HashMap<String, String> = [
        ("DATA_DIR", dir.path().to_str().unwrap()),
        ("NAI_TOKENS", "pst-synthetic-upstream-token"),
        ("NAI_IMAGE_HOST", &mock.url()),
        ("SECRET_KEY", "test-secret-key-test-secret-key-0123456789"),
        ("ADMIN_PASSWORD", ADMIN_PASSWORD),
    ]
    .iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect();
    for (k, v) in extra {
        vars.insert(k.to_string(), v.to_string());
    }
    let static_dir = landing_html.map(|html| {
        let d = dir.path().join("static");
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("landing.html"), html).unwrap();
        d
    });
    let state = AppState::build(env_config(&vars), Assets::new(static_dir.clone()))
        .await
        .unwrap();
    let key: NewKey = serde_json::from_value(json!({
        "name": "测试成员", "token": TOKEN, "daily_images": 100, "daily_v5": 10,
        "monthly_anlas": 0.0, "daily_text_tokens": 1000, "rpm": 60,
    }))
    .unwrap();
    state.db.create_key(&key, state.now()).await.unwrap();
    Env {
        state,
        mock,
        vars,
        static_dir,
        _dir: dir,
    }
}

pub struct Call<'a> {
    pub method: &'a str,
    pub path: &'a str,
    pub headers: Vec<(&'a str, String)>,
    pub body: Option<Value>,
}

impl<'a> Call<'a> {
    pub fn new(method: &'a str, path: &'a str) -> Self {
        Self {
            method,
            path,
            headers: vec![],
            body: None,
        }
    }
    pub fn bearer(mut self, t: &str) -> Self {
        self.headers.push(("authorization", format!("Bearer {t}")));
        self
    }
    pub fn header(mut self, k: &'a str, v: impl Into<String>) -> Self {
        self.headers.push((k, v.into()));
        self
    }
    pub fn json(mut self, v: Value) -> Self {
        self.body = Some(v);
        self
    }
    pub async fn send(self, state: &AppState) -> (StatusCode, HeaderMap, Vec<u8>) {
        let mut b = Request::builder().method(self.method).uri(self.path);
        for (k, v) in &self.headers {
            b = b.header(*k, v);
        }
        let req = match self.body {
            Some(v) => b
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&v).unwrap()))
                .unwrap(),
            None => b.body(Body::empty()).unwrap(),
        };
        let resp = gate_server::app(state.clone()).oneshot(req).await.unwrap();
        let status = resp.status();
        let headers = resp.headers().clone();
        let bytes = resp
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec();
        (status, headers, bytes)
    }
}

/// 一张 V4.5 的出图请求体（832x1216、28 步、1 张）。
pub fn image_body() -> Value {
    json!({
        "input": "1girl, solo", "model": "nai-diffusion-4-5-full", "action": "generate",
        "parameters": {"width": 832, "height": 1216, "steps": 28, "scale": 5, "sampler": "k_euler_ancestral",
                       "n_samples": 1, "seed": 1234, "negative_prompt": "lowres"},
    })
}

/// 用成员 Key 提交一张 [`image_body`]，返回 (状态码, 响应体)。
pub async fn generate(state: &AppState, token: &str) -> (StatusCode, Vec<u8>) {
    let (status, _, body) = Call::new("POST", "/ai/generate-image")
        .bearer(token)
        .json(image_body())
        .send(state)
        .await;
    (status, body)
}

/// 后台任务（记账 / 操作日志）写完。
pub async fn settle() {
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
}

/// 执行一串 SQL（造测试数据）。
pub async fn run_sql(e: &Env, stmts: impl IntoIterator<Item = String>) {
    e.state
        .db
        .run_script(stmts.into_iter().collect())
        .await
        .unwrap();
}

/// 写一项设置。
pub async fn set_setting(e: &Env, k: &str, v: &str) {
    e.state.db.set_setting(k, v).await.unwrap();
}

/// 测试里直接查库：一条只读 SELECT，取全部行（每行一个 JSON 对象，列名为键）。
pub async fn query(db: &gate_db::Db, sql: &str) -> Vec<Value> {
    let sql = sql.to_string();
    db.read(move |c| {
        let mut stmt = c.prepare(&sql)?;
        stmt.query_map([], gate_db::value::row_json)?.collect()
    })
    .await
    .unwrap()
}

/// 后台操作记录，最新的在前，最多 `limit` 条。
pub async fn admin_actions(db: &gate_db::Db, limit: i64) -> Vec<Value> {
    query(
        db,
        &format!("SELECT * FROM admin_actions ORDER BY id DESC LIMIT {limit}"),
    )
    .await
}

/// 给某把 Key 某一天的计数器直接加上 V5 张数与 Anlas（准备「已经用掉一些额度」的场景）。
pub async fn add_usage(db: &gate_db::Db, key_id: i64, day: &str, v5: i64, anlas: f64) {
    let day = day.to_string();
    db.write(move |tx| {
        tx.execute(
            "INSERT INTO counters (key_id, day, requests, v5, anlas) VALUES (?, ?, 1, ?, ?) \
             ON CONFLICT(key_id, day) DO UPDATE SET requests = requests + 1, \
             v5 = v5 + excluded.v5, anlas = anlas + excluded.anlas",
            (key_id, &day, v5, anlas),
        )?;
        Ok(())
    })
    .await
    .unwrap();
}

// ------------------------------------------------------------------ 定时任务测试用

/// 时钟（`time::freeze`）是全进程共享的：用到它的测试先拿这把锁（每个测试文件各一把）。
static CLOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// 串行化用到时钟的测试并冻结到 `at`（None = 真实时间）。返回的锁要一直拿到测试结束。
pub async fn clock(at: Option<f64>) -> tokio::sync::MutexGuard<'static, ()> {
    let guard = CLOCK.lock().await;
    gate_server::time::freeze(at);
    guard
}

/// 立即跑一轮某个定时任务（和循环里一样：记 `job_last`，失败记 Bug）。
pub async fn run_job(state: &AppState, name: &str) -> Result<String, gate_server::jobs::JobError> {
    let job = gate_server::jobs::find(name).unwrap_or_else(|| panic!("没有任务 {name}"));
    gate_server::jobs::run_once(state, job).await
}

/// 读一项设置。
pub async fn setting(state: &AppState, key: &str) -> Option<String> {
    state.db.get_setting(key).await.unwrap()
}

/// 读一项 JSON 设置（没有或不是 JSON 时为 Null）。
pub async fn setting_json(state: &AppState, key: &str) -> Value {
    setting(state, key)
        .await
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(Value::Null)
}

/// 建一把合成的成员 Key（`nai-synthetic-<编号>`），可以再用 SQL 改列；返回库里的 id。
pub async fn member_key(state: &AppState, n: i64, extra: &[(&str, Value)]) -> i64 {
    let key: NewKey = serde_json::from_value(json!({
        "name": format!("成员{n:02}"), "token": format!("nai-synthetic-{n:04}"), "daily_images": 100,
        "daily_v5": 10, "monthly_anlas": 0.0, "daily_text_tokens": 1000, "rpm": 60,
    }))
    .unwrap();
    state.db.create_key(&key, state.now()).await.unwrap();
    let id = state
        .db
        .get_key_by_token(&format!("nai-synthetic-{n:04}"))
        .await
        .unwrap()
        .unwrap()["id"]
        .as_i64()
        .unwrap();
    let sql: Vec<String> = extra
        .iter()
        .map(|(col, v)| format!("UPDATE api_keys SET {col}={v} WHERE id={id}"))
        .collect();
    if !sql.is_empty() {
        state.db.run_script(sql).await.unwrap();
    }
    id
}

// ------------------------------------------------------------------ 后台接口调用

pub struct Resp {
    pub status: u16,
    pub headers: HeaderMap,
    pub body: Vec<u8>,
}

impl Resp {
    pub fn json(&self) -> Value {
        serde_json::from_slice(&self.body)
            .unwrap_or_else(|e| panic!("响应不是 JSON（{e}）：{}", self.text()))
    }
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
    /// (状态码, 响应头, 响应原文)。
    pub fn parts(self) -> (u16, HeaderMap, String) {
        let text = self.text();
        (self.status, self.headers, text)
    }
    /// 状态码 + `{"detail": 文案}`。
    #[track_caller]
    pub fn assert_detail(&self, status: u16, detail: &str) {
        assert_eq!(self.status, status, "{}", self.text());
        assert_eq!(self.json(), json!({"detail": detail}), "状态码 {status}");
    }
}

/// 请求体：没有 / JSON / 原文（原文不带 content-type，用来发坏 JSON）。
pub enum Payload {
    None,
    Json(Value),
    Raw(String),
}

impl From<Option<Value>> for Payload {
    fn from(v: Option<Value>) -> Self {
        v.map_or(Payload::None, Payload::Json)
    }
}

/// 请求长得像不像浏览器发来的：三个开关各管一件事，两套默认值见 `BROWSER` / `BARE`。
#[derive(Clone, Copy)]
pub struct Style {
    /// 默认带 `host` 和 `origin`（= 本站，过得了来源校验）。
    pub origin_host: bool,
    /// 注入 `ConnectInfo`（对端 127.0.0.1）。
    pub conn_info: bool,
    /// 非 GET 且没有请求体时也写 `content-length: 0`（操作日志中间件靠它决定要不要缓存请求体）。
    pub zero_len: bool,
}

impl Style {
    pub const BROWSER: Style = Style {
        origin_host: true,
        conn_info: true,
        zero_len: true,
    };
    pub const BARE: Style = Style {
        origin_host: false,
        conn_info: false,
        zero_len: false,
    };
}

/// 发一个请求。`cookie` 是后台会话的值（不带 `nai_gate_admin=`），None = 未登录；`headers` 覆盖默认头。
pub async fn send_with(
    style: Style,
    state: &AppState,
    method: &str,
    path: &str,
    cookie: Option<&str>,
    body: Payload,
    headers: &[(&str, &str)],
) -> Resp {
    let mut hdrs: Vec<(String, String)> = vec![];
    if style.origin_host {
        hdrs.push(("host".into(), "owl.example".into()));
        hdrs.push(("origin".into(), "https://owl.example".into()));
    }
    if let Some(c) = cookie {
        hdrs.push(("cookie".into(), format!("nai_gate_admin={c}")));
    }
    for (k, v) in headers {
        hdrs.retain(|(x, _)| x != k);
        hdrs.push(((*k).into(), (*v).into()));
    }
    let (bytes, ctype) = match body {
        Payload::None => (None, None),
        Payload::Json(v) => (
            Some(serde_json::to_vec(&v).unwrap()),
            Some("application/json"),
        ),
        Payload::Raw(s) => (Some(s.into_bytes()), None),
    };
    let mut b = Request::builder().method(method).uri(path);
    for (k, v) in &hdrs {
        b = b.header(k.as_str(), v.as_str());
    }
    if let Some(c) = ctype {
        b = b.header("content-type", c);
    }
    // 真实客户端都会带 Content-Length
    if let Some(bytes) = &bytes {
        b = b.header("content-length", bytes.len().to_string());
    } else if style.zero_len && method != "GET" {
        b = b.header("content-length", "0");
    }
    let mut req = b.body(Body::from(bytes.unwrap_or_default())).unwrap();
    if style.conn_info {
        req.extensions_mut()
            .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 123))));
    }
    let resp = gate_server::app(state.clone()).oneshot(req).await.unwrap();
    let status = resp.status().as_u16();
    let headers = resp.headers().clone();
    let body = resp
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes()
        .to_vec();
    Resp {
        status,
        headers,
        body,
    }
}

/// 浏览器风格的请求（来源 = 本站，对端 127.0.0.1）。
pub async fn send(
    state: &AppState,
    method: &str,
    path: &str,
    cookie: Option<&str>,
    body: Option<Value>,
    headers: &[(&str, &str)],
) -> Resp {
    send_with(
        Style::BROWSER,
        state,
        method,
        path,
        cookie,
        body.into(),
        headers,
    )
    .await
}

/// 已登录的后台会话。
pub struct Admin {
    pub state: AppState,
    /// 会话 Cookie 的值（不带 `nai_gate_admin=`）。
    pub cookie: String,
    style: Style,
}

/// 后台路径可以写完整的 `/admin/api/...`，也可以省掉前缀。
fn admin_path(path: &str) -> String {
    if path.starts_with("/admin/") {
        path.to_string()
    } else {
        format!("/admin/api{path}")
    }
}

impl Admin {
    /// 浏览器风格的会话（带 host / origin / ConnectInfo，没请求体的写请求带 `content-length: 0`）。
    pub async fn login(state: &AppState) -> Self {
        Self::login_as(state, Style::BROWSER).await
    }

    /// 什么默认头都不加的会话（来源校验、按 IP 计数之类的用例自己补头）。
    pub async fn login_bare(state: &AppState) -> Self {
        Self::login_as(state, Style::BARE).await
    }

    pub async fn login_as(state: &AppState, style: Style) -> Self {
        let r = send_with(
            style,
            state,
            "POST",
            "/admin/api/login",
            None,
            Payload::Json(json!({"password": ADMIN_PASSWORD})),
            &[],
        )
        .await;
        assert_eq!(r.status, 200, "{}", r.text());
        let set = r.headers["set-cookie"].to_str().unwrap();
        let cookie = set
            .split(';')
            .next()
            .unwrap()
            .trim_start_matches("nai_gate_admin=")
            .to_string();
        Self {
            state: state.clone(),
            cookie,
            style,
        }
    }

    /// 完整的 `Cookie` 请求头（自己拼请求时用）。
    pub fn cookie_header(&self) -> String {
        format!("nai_gate_admin={}", self.cookie)
    }

    /// 带会话发请求，可以追加 / 覆盖请求头。
    pub async fn raw(
        &self,
        method: &str,
        path: &str,
        body: Option<Value>,
        extra: &[(&str, &str)],
    ) -> Resp {
        send_with(
            self.style,
            &self.state,
            method,
            &admin_path(path),
            Some(&self.cookie),
            body.into(),
            extra,
        )
        .await
    }
    pub async fn req(&self, method: &str, path: &str, body: Option<Value>) -> Resp {
        self.raw(method, path, body, &[]).await
    }
    pub async fn get(&self, path: &str) -> Resp {
        self.req("GET", path, None).await
    }
    pub async fn post(&self, path: &str, body: Value) -> Resp {
        self.req("POST", path, Some(body)).await
    }
    pub async fn put(&self, path: &str, body: Value) -> Resp {
        self.req("PUT", path, Some(body)).await
    }
    pub async fn patch(&self, path: &str, body: Value) -> Resp {
        self.req("PATCH", path, Some(body)).await
    }
    pub async fn delete(&self, path: &str) -> Resp {
        self.req("DELETE", path, None).await
    }
    /// (状态码, 响应原文)。
    pub async fn text(&self, method: &str, path: &str, body: Option<Value>) -> (u16, String) {
        let r = self.req(method, path, body).await;
        (r.status, r.text())
    }
    /// (状态码, 解析后的 JSON；不是 JSON 时为 Null)。
    pub async fn json(&self, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
        let r = self.req(method, path, body).await;
        let v = serde_json::from_slice(&r.body).unwrap_or(Value::Null);
        (r.status, v)
    }
    /// GET 并要求 200，返回 JSON。
    pub async fn get_ok(&self, path: &str) -> Value {
        let r = self.get(path).await;
        assert_eq!(r.status, 200, "GET {path}: {}", r.text());
        r.json()
    }
    /// 期望失败：状态码与 `{"detail": ...}` 一致。
    pub async fn fails(
        &self,
        method: &str,
        path: &str,
        body: Option<Value>,
        status: u16,
        detail: &str,
    ) {
        self.req(method, path, body)
            .await
            .assert_detail(status, detail);
    }
    /// 操作日志（新→旧）里 (操作名, 对象, 详情, 是否成功)。写日志是后台任务，先等一下。
    pub async fn actions(&self) -> Vec<(String, String, String, bool)> {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        admin_actions(&self.state.db, 200)
            .await
            .iter()
            .map(|r| {
                (
                    r["action"].as_str().unwrap().to_string(),
                    r["target"].as_str().unwrap().to_string(),
                    r["detail"].as_str().unwrap().to_string(),
                    r["ok"].as_i64() == Some(1),
                )
            })
            .collect()
    }
    /// 执行一串 SQL（造测试数据）。
    pub async fn sql(&self, sql: &str) {
        let sql = sql.to_string();
        self.state
            .db
            .write(move |tx| tx.execute_batch(&sql))
            .await
            .unwrap();
    }
}

/// 配齐自助领取（真实注册服务）的环境变量；`base` = 假 Discord 的地址。
pub fn registration_env(base: &str) -> Vec<(String, String)> {
    [
        ("DISCORD_CLIENT_ID", "client-id"),
        ("DISCORD_CLIENT_SECRET", "client-secret"),
        ("DISCORD_BOT_TOKEN", "fake-bot-token"),
        (
            "REGISTRATION_BRIDGE_SECRET",
            "contract-secret-contract-secret-0001",
        ),
        ("DISCORD_GUILD_ID", "900000000000000001"),
        ("SITE_URL", "https://gate.test"),
        ("DISCORD_API_BASE", base),
    ]
    .iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect()
}

/// 建一把成员 Key（日 50 张，每分钟 30 次），返回库里的 id。
pub async fn mk_key(state: &AppState, name: &str, token: &str) -> i64 {
    let k: NewKey = serde_json::from_value(json!({
        "name": name, "token": token, "daily_images": 50, "monthly_anlas": 0.0,
        "daily_text_tokens": 1000, "rpm": 30, "daily_v5": 5, "daily_anlas": 0.0,
    }))
    .unwrap();
    let row = state.db.create_key(&k, state.now()).await.unwrap();
    row["id"].as_i64().unwrap()
}

/// 配好网关并登录后台。
pub async fn ready() -> (Env, Admin) {
    let e = setup(&[]).await;
    let a = Admin::login(&e.state).await;
    (e, a)
}

/// 配齐自助领取的网关；`extra` 追加 / 覆盖环境变量。
pub async fn setup_registration(base: &str, extra: &[(&str, &str)]) -> Env {
    let vars = registration_env(base);
    let mut refs: Vec<(&str, &str)> = vars.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    refs.extend_from_slice(extra);
    setup(&refs).await
}

/// 已经写进库的 Bug 的来源（等 Bug 写完再读）。
pub async fn bug_sources(state: &AppState) -> Vec<String> {
    state.bugs.idle().await;
    state
        .db
        .list_error_events(true, 100)
        .await
        .unwrap()
        .iter()
        .map(|r| r["source"].as_str().unwrap_or("").to_string())
        .collect()
}
