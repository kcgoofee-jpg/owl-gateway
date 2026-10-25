//! 首页实时图（`/public/live` 的数据与 meta）、`/public/status` 和公开页面用的进程内状态。
//!
//! 算出来的快照放进一个 `tokio::sync::watch` 通道，
//! JSON 接口与 SSE 共用同一份——
//! - HTTP 取数（[`Hub::current`]）：快照不到 1 秒新就直接用，否则单飞重算（没人访问就不算）；
//! - SSE 订阅（[`Hub::subscribe`]）：第一次订阅时起一个后台任务，每秒在有订阅者时重算一次；任务挂在 `state.tasks` 上，
//!   `state.shutdown` 取消时退出。
//!
//! 快照里没有任何人的 Key、提示词、IP 或 Discord ID；「我的排队」（`me`）按连接单独拼，不进共享快照
//! 红线：共享缓存对象绝不能被某个请求的 `me` 污染。

use crate::auth::site_flag;
use crate::error::InternalError;
use crate::state::AppState;
use gate_core::features::{self, FEATURES};
use gate_core::pyc;
use gate_core::pyval;
use gate_core::reasons::Reason;
use gate_core::site_flags::ALGO_NOTICE;
use serde_json::{Map, Value, json};

fn public_algo_notice(custom: &str, mode: gate_core::v5_access::Mode) -> String {
    let note = match mode {
        gate_core::v5_access::Mode::Restricted => "V5 当前仅授权成员可用。",
        gate_core::v5_access::Mode::Closed => "V5 当前已关闭。",
        _ => return pyc::take_chars(custom, 300).to_string(),
    };
    let prefix = pyc::take_chars(custom, 220);
    if prefix.trim().is_empty() {
        note.to_string()
    } else {
        format!("{prefix} {note}")
    }
}
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::watch;
use tokio::time::{Duration, Instant};

/// 「近 1 分钟」请求窗口（秒）。
pub const WINDOW: f64 = 60.0;
/// 快照新鲜度：1 秒。
const SNAPSHOT_TTL: Duration = Duration::from_secs(1);
/// meta（站点地址、注册开关……）变化很慢：30 秒子缓存。
const META_TTL: Duration = Duration::from_secs(30);
/// `/public/status` 缓存 8 秒，避免匿名请求放大数据库压力。
const STATUS_TTL: Duration = Duration::from_secs(8);
/// SSE 保活注释的间隔。
pub const KEEPALIVE: Duration = Duration::from_secs(15);

// ------------------------------------------------------------------ 近 1 分钟请求

/// (近 1 分钟总请求数, 其中被拒数)。顺手清掉窗口外的旧记录。
pub fn recent(state: &AppState, now: f64) -> (i64, i64) {
    let mut e = state.live_events.lock().expect("live");
    while e.front().is_some_and(|(t, _)| *t < now - WINDOW) {
        e.pop_front();
    }
    let rejected = e.iter().filter(|(_, s)| s == "rejected").count();
    (e.len() as i64, rejected as i64)
}

/// 从拒绝理由里取「约 N 分钟」的 N；没有就当无穷大（取最短等待时排在最后）。
fn wait_minutes(text: &str) -> i64 {
    let mut rest = text;
    while let Some(i) = rest.find("约 ") {
        rest = &rest[i + "约 ".len()..];
        let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
        if !digits.is_empty() && rest[digits.len()..].starts_with(" 分钟") {
            return digits.parse().unwrap_or(i64::MAX);
        }
    }
    1_000_000
}

// ------------------------------------------------------------------ 注册设置（只读部分）

/// 注册设置里首页用得到的三项。没配注册服务时没有这个东西（None）。
pub struct RegSettings {
    pub open: bool,
    pub max_users: i64,
    /// None = 全部功能（`register_features` 存的是 `*`，或环境变量里没配）。
    pub features: Option<Vec<String>>,
    /// 限定身份组的名称（后台 register_role_note）；只有身份组门槛配齐（服务器 + 身份组都有）时才是 `Some`。
    /// 首页登录被挡时要告诉成员「缺哪个身份组」，只暴露这一句名称，不暴露服务器 / 身份组 ID。
    pub role_note: Option<String>,
}

/// 生效的注册设置：数据库里保存的优先，其次是环境默认值。读法和后台 / 机器人共用 `ops::registration_settings`，
/// 这里只取首页用得到的三项。
pub async fn registration_settings(state: &AppState) -> Result<Option<RegSettings>, InternalError> {
    if state.env.registration.is_none() {
        return Ok(None);
    }
    let r = crate::ops::registration_settings(state).await?;
    let role_note = (!r.role_rules().is_empty()).then(|| r.role_note_text());
    Ok(Some(RegSettings {
        open: r.open,
        max_users: r.max_users,
        features: r.features,
        role_note,
    }))
}

/// 剩余名额：`max(0, max_users - 已占用)`；不限名额（max_users = 0）为 None。
async fn slots_left(state: &AppState, cfg: &RegSettings) -> Result<Option<i64>, InternalError> {
    if cfg.max_users == 0 {
        return Ok(None);
    }
    let active = state.db.registered_member_count(state.now()).await?;
    Ok(Some((cfg.max_users - active).max(0)))
}

// ------------------------------------------------------------------ 看板本体

/// 还没有出图耗时样本时，一个账号平均多久开始一张（秒）：实测出图约 7.6 秒 + 出完后平均歇 2 秒，取整 10。
pub const DEFAULT_CYCLE: f64 = 10.0;

/// 汇总数字（请求数、额度、排队数、账号保护、名额），不含 meta 与 me。键顺序固定，前端按它读。
pub async fn build(state: &AppState, now: f64) -> Result<Map<String, Value>, InternalError> {
    struct Acct {
        usable: bool,
        cooling: i64,
        next_in: f64,
        /// 这个账号两次开始出图的平均间距（出图耗时 + 出完后平均歇的秒数）。
        cycle: f64,
        /// 今天出满了（单号每日上限）：账号本身可用，但不会再接图。
        full: bool,
        /// 网关估计：现在再来一张，这个账号多久后能开始（含它前面排着的张数）。
        start_in: f64,
    }
    impl Acct {
        /// 能接新图：可用且今天没出满。
        fn open(&self) -> bool {
            self.usable && !self.full
        }
    }
    let pool = state.nai.tokens().await;
    let day = state.day();
    let mono = state.nai.mono();
    let mut accounts: Vec<Acct> = Vec::with_capacity(pool.len());
    let mut images_today = 0i64;
    for t in &pool {
        let c = state.db.get_upstream_counter(&t.token_id, &day).await?;
        images_today += c.images;
        let f = t.fields();
        let pacing = state.nai.pacing(t);
        accounts.push(Acct {
            usable: t.usable(now),
            cooling: if !f.disabled && f.admin_enabled {
                ((f.blocked_until - now) as i64).max(0)
            } else {
                0
            },
            next_in: pyc::round((f.image_next_at - mono).max(0.0), 1),
            cycle: pacing.cycle,
            full: state
                .guard
                .with(|g| g.at_daily_cap(&t.token_id, c.images, now)),
            start_in: pacing.next_start_in,
        });
    }
    let values = state.guard.with(|g| g.values.clone());
    let gv = |name: &str| values.get(name).copied().unwrap_or(0);
    let usable = (accounts.iter().filter(|a| a.usable).count() as i64).max(1);
    let v5_cap = crate::ops::global_daily_v5(&state.db).await?;
    let v5_access = crate::v5_access::evaluate(state, None).await?;
    let v5_paused = v5_access.paused;
    let v5_access_mode = v5_access.mode.as_str();
    // 10 分钟突发窗口已删除：`guard` 段不再有 `burst_cap` / `burst_wait`。
    // 出图间隔与间隔抖动 10-23 删除：每个账号的节奏 = 出图耗时 + 出完后歇 N～M 秒
    let (rest_min, rest_max) = (
        gv("post_jitter_min"),
        gv("post_jitter_max").max(gv("post_jitter_min")),
    );
    // 周期、开始时间都只看能接新图的账号（可用且今天没出满）：出满的账号不会再接图
    let cycles: Vec<f64> = accounts
        .iter()
        .filter(|a| a.open() && a.cycle > 0.0)
        .map(|a| a.cycle)
        .collect();
    let cycle = if cycles.is_empty() {
        DEFAULT_CYCLE
    } else {
        cycles.iter().sum::<f64>() / cycles.len() as f64
    };
    let next_in = accounts
        .iter()
        .filter(|a| a.open())
        .map(|a| a.next_in)
        .reduce(f64::min);
    // 网关自己的估计：现在再来一张，最早多久后能开始（选号用的同一个数，取整秒，免得每秒都推一次快照）
    let start_in = accounts
        .iter()
        .filter(|a| a.open())
        .map(|a| a.start_in)
        .reduce(f64::min)
        .map(|v| v.max(0.0).round() as i64);

    // 调度规则（首页「点一步看规则」用）：全是站点级参数和汇总，不含任何个人信息。
    // 额度都是站长手动定的（10-23 起没有自动额度算法）
    let quota = {
        let m = crate::jobs::quota::manual(state).await?;
        json!({"ceiling": m.daily_images, "v5_each": m.daily_v5, "v5_global": m.global_v5,
               "v5_paused": m.v5_paused, "v5_access_mode": v5_access_mode})
    };
    let token_ids: Vec<String> = pool
        .iter()
        .filter(|t| t.usable(now))
        .map(|t| t.token_id.clone())
        .collect();
    // 全站现在能不能出图：所有可用账号都被站点级规则拦下时，首页要说清原因（和成员拿到的 429 同一句话）
    let mut blocked = Value::Null;
    if !token_ids.is_empty() {
        let mut why: Vec<Option<Reason>> = Vec::with_capacity(token_ids.len());
        let mut failed = false;
        for tid in &token_ids {
            match state.guard.token_block_reason(tid, &day, now).await {
                Ok(r) => why.push(r),
                Err(_) => {
                    failed = true;
                    break;
                }
            }
        }
        if !failed
            && why
                .iter()
                .all(|r| r.as_ref().is_some_and(|r| !r.text().is_empty()))
        {
            let mut first: Option<&Reason> = None;
            for r in why.iter().flatten() {
                // Python `min(key=...)`：并列取先出现的
                if first.is_none_or(|f| wait_minutes(r.text()) < wait_minutes(f.text())) {
                    first = Some(r);
                }
            }
            if let Some(f) = first {
                blocked = json!({"code": f.code(), "text": f.text()});
            }
        }
    }

    let (requests, rejected) = recent(state, now);
    let mut members = json!({});
    if let Some(cfg) = registration_settings(state).await? {
        let active = state.db.registered_member_count(state.now()).await?;
        members = json!({"max": cfg.max_users, "active": active, "open": cfg.open});
    }
    // Each usable account's cap today: a temporary manual raise, else the automatic cap (<= the owner's value)
    let daily_cap_total = if gv("account_daily_cap") == 0 {
        0
    } else if token_ids.is_empty() {
        gv("account_daily_cap") * usable
    } else {
        state.guard.with(|g| g.total_daily_cap(&token_ids, now))
    };
    let mut body = Map::new();
    body.insert("t".into(), json!(now));
    body.insert("mode".into(), json!("fifo")); // 启用轮流出图后为 "drr"
    body.insert(
        "auth".into(),
        json!({"requests": requests, "rejected": rejected}),
    );
    body.insert(
        "quota".into(),
        json!({
            "images_today": images_today,
            // 节约模式的折算张数 `units_today` 已删除：每日上限就按 images_today 比
            "images_cap": daily_cap_total,
            "v5_today": state.db.day_v5_total(&day).await?,
            "v5_cap": v5_cap,
            // 动态额度把今天的 V5 暂停了：和「账号免费 V5 用完」同样显示暂停
            "upstream_v5_free": upstream_v5_free(state).await && !v5_paused,
            "v5_paused": v5_paused,
            "v5_access_mode": v5_access_mode,
        }),
    );
    // queue.full：网关此刻是否会以「排队满」拒绝新图（与 guard.admit_image 同一判断），首页直接用它，不在前端重算
    let queue_cap = crate::admission::queue_cap(state).await;
    body.insert(
        "queue".into(),
        state.guard.with(|g| {
            let mut q = g.queue_view(None);
            q["full"] = json!(g.queue_full(queue_cap));
            q
        }),
    );
    body.insert(
        "guard".into(),
        json!({
            // `min(..., default=0)`：没有可用账号时是整数 0
            "next_in": next_in.map_or(json!(0), pyval::float),
            // 没有能接新图的账号时为 null
            "start_in": start_in,
            "cooldown": state.cooldown.remaining(now),
            "blocked": blocked,
        }),
    );
    body.insert(
        "accounts".into(),
        Value::Array(
            accounts
                .iter()
                .map(|a| json!({"usable": a.usable, "cooling": a.cooling, "full": a.full}))
                .collect(),
        ),
    );
    body.insert("members".into(), members);
    // 一个账号平均多久开始一张（可用账号的平均周期）；首页排队估算用
    body.insert("cycle".into(), json!(pyc::round(cycle, 1)));
    body.insert(
        "rules".into(),
        // 3.0 删掉的字段：`base`（保底）、`borrow_open`（空闲借用）、
        // `anlas`（早已下线的 Anlas 自动分配占位）
        json!({
            // 全站排队上限：每个账号取 min(设置, 排队超时内真能开始的张数) 相加；null = 不限。
            // 每小时上限（10-22）、安静时段（10-23）删除
            // 字段名沿用旧的 key_queue（客户端兼容）；值是每人排队上限 queue_per_member
            "key_queue": gv("queue_per_member"),
            "queue_cap": queue_cap,
            "daily_cap": daily_cap_total,
            "rest": [rest_min, rest_max], // 每张出完后这个账号歇 N～M 秒
            "quota": quota, // 手动额度
        }),
    );
    Ok(body)
}

/// 上游账号里还有没有能免费出 V5 的（`/public/me` 的 `today.upstream_v5_free`、`/public/live` 的
/// `quota.upstream_v5_free`）。false = 全站免费 V5 暂时用完：网关此刻会改派不成、以 `v5_exhausted`
/// 拒绝免费 V5（同一个判断，见 `NaiClient::v5_free_open`）。查不到（库出错）按 true，不误报用完。
pub async fn upstream_v5_free(state: &AppState) -> bool {
    state.nai.v5_free_open().await.unwrap_or(true)
}

/// 这把 Key 的图现在排第几 / 是否在生成。
/// `eta`：已经排到账号上的用上游客户端的估计（`start_at`，按那个账号前面排了几张和它现在的间隔算）；
/// 还没排到的按 位次 × 全站每张的放行间隔（[`Snapshot::site_interval`]：单账号周期 ÷ 可用账号数）。
pub fn mine(state: &AppState, key_id: i64, snap: &Snapshot) -> Value {
    let mut view = state.guard.with(|g| g.queue_view(Some(key_id)));
    let interval = snap.site_interval();
    let mut items = view.get_mut("mine").map(Value::take).unwrap_or(json!([]));
    if let Value::Array(list) = &mut items {
        for item in list {
            let Some(o) = item.as_object_mut() else {
                continue;
            };
            let start_at = o.remove("start_at").and_then(|v| v.as_f64());
            if o.get("state").and_then(Value::as_str) == Some("waiting") {
                let pos = o.get("position").and_then(Value::as_i64).unwrap_or(0);
                let eta = match start_at {
                    Some(at) => (at - state.now()).max(0.0),
                    None => pos as f64 * interval,
                };
                o.insert("eta".into(), json!(eta.round() as i64));
            }
        }
    }
    json!({"mine": items, "waiting": view["waiting"], "running": view["running"]})
}

/// 变化很慢的站点信息（meta）。
async fn build_meta(state: &AppState) -> Result<Map<String, Value>, InternalError> {
    let env = &state.env;
    let mut reg = json!({"configured": false, "open": false}); // 没配注册服务 = 关闭（fail-closed）
    if let Some(cfg) = registration_settings(state).await? {
        reg = json!({"configured": true, "open": cfg.open});
        if let Some(note) = &cfg.role_note {
            reg["role_note"] = json!(note);
        }
        if let Some(left) = slots_left(state, &cfg).await? {
            reg["slots_left"] = json!(left);
        }
    }
    let algo = pyval::py_str(&site_flag(state, &ALGO_NOTICE).await?);
    let policy = crate::v5_access::evaluate(state, None).await?;
    let algo = public_algo_notice(&algo, policy.mode);
    let mut m = Map::new();
    m.insert("site".into(), json!(env.site_url.trim_end_matches('/')));
    m.insert("discord_invite".into(), json!(env.discord_invite_url));
    m.insert("algo_notice".into(), json!(pyc::take_chars(&algo, 300)));
    m.insert(
        "key_inactivity_delete_days".into(),
        json!(env.key_inactivity_delete_days),
    );
    // 网页登录常开（暂停开关 web_login_paused 已删除）；字段留着是因为已打开的旧页面还在读它
    m.insert("web_login".into(), json!(true));
    m.insert("registration".into(), reg);
    Ok(m)
}

// ------------------------------------------------------------------ /public/status

/// 首页「使用须知」里的排队规则；数值来自后台「账号保护与排队」（保底已删除：每人只有每日上限）。
fn public_limits(state: &AppState) -> Value {
    let v = state.guard.with(|g| g.values.clone());
    let get = |k: &str| v.get(k).copied().unwrap_or(0);
    // 字段名沿用旧的 key_image_queue（首页 / 客户端兼容）；值是每人排队上限 queue_per_member
    json!({"key_image_queue": get("queue_per_member")})
}

/// 状态页的公开响应：不含任何密钥、成员信息或计数细节。
pub async fn build_status(state: &AppState) -> Result<Value, InternalError> {
    let flags =
        features::parse_global_flags(state.db.get_setting(features::GLOBAL_KEY).await?.as_deref())?;
    let enabled = |n: &str| flags.get(n).copied().unwrap_or(false);
    let mut reg = json!({"open": false, "slots_left": null});
    let mut defaults: Vec<String> = if enabled("image") {
        vec!["image".into()]
    } else {
        vec![]
    };
    if let Some(cfg) = registration_settings(state).await? {
        reg = json!({"open": cfg.open, "slots_left": slots_left(state, &cfg).await?});
        if let Some(note) = &cfg.role_note {
            reg["role_note"] = json!(note);
        }
        let names: Vec<String> = match &cfg.features {
            None => FEATURES.iter().map(|(n, _)| n.to_string()).collect(),
            Some(l) => l.clone(),
        };
        defaults = names
            .into_iter()
            .filter(|n| enabled(n) && n != "voice")
            .collect();
    }
    let now = state.now();
    let (status, _, _) = state.health.status(now);
    let stability = match state.db.image_stability(now - 24.0 * 3600.0).await {
        Ok(s) => json!({"ok": s.ok, "error": s.error}),
        Err(_) => json!({"ok": 0, "error": 0}),
    };
    let image_jobs = state.budget.lock().await.reservations.len();
    let env = &state.env;
    let algo = pyval::py_str(&site_flag(state, &ALGO_NOTICE).await?);
    let policy = crate::v5_access::evaluate(state, None).await?;
    let algo = public_algo_notice(&algo, policy.mode);
    Ok(json!({
        "site": env.site_url.trim_end_matches('/'),
        "upstream": {"status": status, "image_cooldown_seconds": state.cooldown.remaining(now)},
        "registration": reg,
        "default_features": defaults.iter().map(|n| json!({
            "id": n,
            "label": FEATURES.iter().find(|(f, _)| f == n).map_or("", |(_, l)| *l),
        })).collect::<Vec<_>>(),
        "web_login": true,
        "algo_notice": pyc::take_chars(&algo, 300),
        "discord_invite": env.discord_invite_url,
        "key_inactivity_delete_days": env.key_inactivity_delete_days,
        "limits": public_limits(state),
        "image_jobs": image_jobs,
        "stability": stability,
    }))
}

/// 每人每日 Anlas 上限：Key 手动设置的优先，否则全站默认 `member_daily_anlas`。0 = 不限。
/// 都是今天的生效值。
pub async fn member_anlas_cap(
    state: &AppState,
    key: &crate::key::Key,
) -> Result<i64, InternalError> {
    Ok(crate::admission::member_anlas_cap_f64(state, key).await? as i64)
}

// ------------------------------------------------------------------ 快照与 Hub

/// 一份共享快照（live.build + meta，不含 me）。
pub struct Snapshot {
    pub t: f64,
    pub body: Map<String, Value>,
    json: String,
    /// 内容签名（不含 `t`）：SSE 只在它变了才推。
    pub sig: u64,
    /// 后台事件用：再去掉每秒倒计时类字段与滚动窗口计数。
    pub admin_sig: u64,
    made: Instant,
}

/// 与 Starlette `JSONResponse` 相同的紧凑 JSON 文本。
pub fn wire_text(v: &Value) -> String {
    String::from_utf8(gate_core::pyjson::wire(v)).unwrap_or_default()
}

fn hash_value(v: &Value) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    gate_core::pyjson::wire(v).hash(&mut h);
    h.finish()
}

fn content_sig(body: &Map<String, Value>, admin: bool) -> u64 {
    let mut m = Map::new();
    for (k, v) in body {
        if k == "t" || (admin && k == "auth") {
            continue;
        }
        if admin
            && k == "guard"
            && let Value::Object(g) = v
        {
            let g2: Map<String, Value> = g
                .iter()
                .filter(|(k, _)| !matches!(k.as_str(), "next_in" | "start_in" | "cooldown"))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            m.insert(k.clone(), Value::Object(g2));
            continue;
        }
        m.insert(k.clone(), v.clone());
    }
    hash_value(&Value::Object(m))
}

impl Snapshot {
    /// 全站平均每张图的放行间隔（秒）：`cycle`（单账号周期 = 出图耗时 + 出完后平均歇的秒数）÷ 能接新图的账号数
    /// （可用且今天没出满）。
    /// 几个账号同时出图，排队估算按它算。
    pub fn site_interval(&self) -> f64 {
        let per_account = self
            .body
            .get("cycle")
            .and_then(Value::as_f64)
            .filter(|v| *v > 0.0)
            .unwrap_or(DEFAULT_CYCLE);
        let usable = self
            .body
            .get("accounts")
            .and_then(Value::as_array)
            .map_or(0, |a| {
                a.iter()
                    .filter(|x| {
                        x.get("usable").and_then(Value::as_bool) == Some(true)
                            && x.get("full").and_then(Value::as_bool) != Some(true)
                    })
                    .count()
            })
            .max(1);
        per_account / usable as f64
    }

    fn new(body: Map<String, Value>) -> Self {
        let json = wire_text(&Value::Object(body.clone()));
        Self {
            t: body.get("t").and_then(Value::as_f64).unwrap_or(0.0),
            sig: content_sig(&body, false),
            admin_sig: content_sig(&body, true),
            json,
            body,
            made: Instant::now(),
        }
    }

    /// 不带 `me` 的 JSON 文本（紧凑、Python 浮点格式）。
    pub fn json(&self) -> &str {
        &self.json
    }

    /// `/public/live` 的完整响应文本：键顺序 = live.build → meta → me。`me` 为 None 时是 `null`。
    pub fn render(&self, me: Option<&Value>) -> String {
        self.render_raw(me.map(wire_text).as_deref().unwrap_or("null"))
    }

    /// 同上，`me` 已经是 JSON 文本。
    pub fn render_raw(&self, me_json: &str) -> String {
        // json 以 `}` 结尾：去掉它再接上 me
        let head = &self.json[..self.json.len() - 1];
        format!("{head},\"me\":{me_json}}}")
    }

    /// 同上，结构化版本（首屏注入用）。
    pub fn value(&self, me: Option<Value>) -> Value {
        let mut m = self.body.clone();
        m.insert("me".into(), me.unwrap_or(Value::Null));
        Value::Object(m)
    }
}

/// SSE 连接的种类。每种有同时连接数上限，超过就拒绝（503），页面会退回轮询。
#[derive(Clone, Copy)]
pub enum StreamKind {
    Public,
    Admin,
}

impl StreamKind {
    fn idx(self) -> usize {
        self as usize
    }
    fn cap(self) -> usize {
        match self {
            StreamKind::Public => 2000,
            StreamKind::Admin => 32,
        }
    }
    /// 单个 IP 的上限：防止一个脚本把看板名额占满，让其他访客只能轮询。
    fn per_ip_cap(self) -> usize {
        match self {
            StreamKind::Public => 20,
            StreamKind::Admin => 8,
        }
    }
}

/// 一条 SSE 连接占的名额；连接结束（流被丢弃）时归还。
pub struct StreamSlot {
    state: AppState,
    kind: StreamKind,
    ip: String,
}

impl Drop for StreamSlot {
    fn drop(&mut self) {
        self.state.live.streams[self.kind.idx()].fetch_sub(1, Ordering::SeqCst);
        if let Ok(mut map) = self.state.live.streams_ip.lock()
            && let Some(n) = map.get_mut(&self.ip)
        {
            *n = n.saturating_sub(1);
            if *n == 0 {
                map.remove(&self.ip);
            }
        }
    }
}

#[derive(Default)]
struct ClientErr {
    window: f64,
    total: u32,
    ip: HashMap<String, u32>,
}

/// 带时间戳的缓存槽（meta 30 秒、status 8 秒）。
type Cached<T> = tokio::sync::Mutex<Option<(Instant, Arc<T>)>>;

/// 实时看板与公开接口的进程内状态（挂在 `AppState` 上，没有全局变量）。
pub struct Hub {
    tx: watch::Sender<Option<Arc<Snapshot>>>,
    rebuild: tokio::sync::Mutex<()>,
    meta: Cached<Map<String, Value>>,
    status: Cached<Value>,
    ticker: AtomicBool,
    streams: [AtomicUsize; 2],
    /// 每个 IP 当前打开的 SSE 连接数（公开/后台共用同一个 IP 计数，上限按 kind 细分）。
    streams_ip: Mutex<HashMap<String, usize>>,
    keepalive_ms: AtomicU64,
    client_err: Mutex<ClientErr>,
    /// 设备指纹上报的独立预算：不能和脚本报错抢同一个桶（匿名可先打满让真错误丢失）。
    device_fp: Mutex<ClientErr>,
}

impl Default for Hub {
    fn default() -> Self {
        Self::new()
    }
}

impl Hub {
    pub fn new() -> Self {
        Self {
            tx: watch::channel(None).0,
            rebuild: tokio::sync::Mutex::new(()),
            meta: tokio::sync::Mutex::new(None),
            status: tokio::sync::Mutex::new(None),
            ticker: AtomicBool::new(false),
            streams: [AtomicUsize::new(0), AtomicUsize::new(0)],
            streams_ip: Mutex::new(HashMap::new()),
            keepalive_ms: AtomicU64::new(KEEPALIVE.as_millis() as u64),
            client_err: Mutex::new(ClientErr::default()),
            device_fp: Mutex::new(ClientErr::default()),
        }
    }

    /// 最近一份快照（可能是几秒前的，也可能还没有）。
    pub fn latest(&self) -> Option<Arc<Snapshot>> {
        self.tx.borrow().clone()
    }

    fn newer_than(&self, age: Duration) -> Option<Arc<Snapshot>> {
        self.latest().filter(|s| s.made.elapsed() < age)
    }

    /// 取一份不超过 1 秒的快照；没有就重算（多个请求同时来只算一次）。
    pub async fn current(&self, state: &AppState) -> Result<Arc<Snapshot>, InternalError> {
        self.refresh(state, SNAPSHOT_TTL).await
    }

    async fn refresh(
        &self,
        state: &AppState,
        max_age: Duration,
    ) -> Result<Arc<Snapshot>, InternalError> {
        if let Some(s) = self.newer_than(max_age) {
            return Ok(s);
        }
        let _one_at_a_time = self.rebuild.lock().await;
        if let Some(s) = self.newer_than(max_age) {
            return Ok(s); // 排队期间别人已经算好了
        }
        let now = state.now();
        let mut body = build(state, now).await?;
        // 浅拼：meta 的键与 build 的键不重叠；meta 自带 30 秒缓存
        for (k, v) in self.meta(state).await?.iter() {
            body.insert(k.clone(), v.clone());
        }
        let snap = Arc::new(Snapshot::new(body));
        self.tx.send_replace(Some(snap.clone()));
        Ok(snap)
    }

    async fn meta(&self, state: &AppState) -> Result<Arc<Map<String, Value>>, InternalError> {
        let mut slot = self.meta.lock().await;
        if let Some((at, m)) = slot.as_ref()
            && at.elapsed() < META_TTL
        {
            return Ok(m.clone());
        }
        let m = Arc::new(build_meta(state).await?);
        *slot = Some((Instant::now(), m.clone()));
        Ok(m)
    }

    /// `/public/status` 的响应体（8 秒缓存）。
    pub async fn status(&self, state: &AppState) -> Result<Arc<Value>, InternalError> {
        let mut slot = self.status.lock().await;
        if let Some((at, v)) = slot.as_ref()
            && at.elapsed() < STATUS_TTL
        {
            return Ok(v.clone());
        }
        let v = Arc::new(build_status(state).await?);
        *slot = Some((Instant::now(), v.clone()));
        Ok(v)
    }

    /// 订阅快照更新（SSE 用）。返回前保证有一份够新的快照，第一条事件不用等一秒。
    pub async fn subscribe(
        &self,
        state: &AppState,
    ) -> Result<watch::Receiver<Option<Arc<Snapshot>>>, InternalError> {
        self.start(state);
        let rx = self.tx.subscribe();
        self.current(state).await?;
        Ok(rx)
    }

    /// 起后台任务（幂等）：每秒、且有订阅者时重算一次快照。第一次订阅时自动调用；
    /// 任务挂在 `state.tasks` 上，`state.shutdown` 取消时退出。
    pub fn start(&self, state: &AppState) {
        if self.ticker.swap(true, Ordering::SeqCst) {
            return;
        }
        let st = state.clone();
        state.tasks.spawn(async move {
            let mut every = tokio::time::interval(Duration::from_secs(1));
            every.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tokio::select! {
                    _ = st.shutdown.cancelled() => break,
                    _ = every.tick() => {}
                }
                if st.live.tx.receiver_count() == 0 {
                    continue;
                }
                // 半秒内刚算过（比如有 HTTP 请求顺手算的）就不重复，保持约 1 Hz
                if let Err(e) = st.live.refresh(&st, Duration::from_millis(500)).await {
                    tracing::warn!(target: "live", "实时快照重算失败：{}", e.0);
                }
            }
        });
    }

    /// 占一个 SSE 名额；全站或单 IP 满了都返回 None。
    pub fn open_stream(&self, state: &AppState, kind: StreamKind, ip: &str) -> Option<StreamSlot> {
        let mut map = self.streams_ip.lock().expect("streams_ip");
        let per_ip = map.get(ip).copied().unwrap_or(0);
        if per_ip >= kind.per_ip_cap() {
            return None;
        }
        let n = self.streams[kind.idx()].fetch_add(1, Ordering::SeqCst);
        if n >= kind.cap() {
            self.streams[kind.idx()].fetch_sub(1, Ordering::SeqCst);
            return None;
        }
        *map.entry(ip.to_string()).or_insert(0) += 1;
        Some(StreamSlot {
            state: state.clone(),
            kind,
            ip: ip.to_string(),
        })
    }

    /// 当前打开的 SSE 连接数（测试 / 诊断用）。
    pub fn open_streams(&self, kind: StreamKind) -> usize {
        self.streams[kind.idx()].load(Ordering::SeqCst)
    }

    pub fn keepalive(&self) -> Duration {
        Duration::from_millis(self.keepalive_ms.load(Ordering::Relaxed))
    }

    /// 调整保活间隔（测试用，默认 15 秒）。
    pub fn set_keepalive(&self, d: Duration) {
        self.keepalive_ms
            .store(d.as_millis().max(1) as u64, Ordering::Relaxed);
    }

    /// 前端脚本报错上报的限流：每 IP 每 10 分钟 10 条，全站每 10 分钟 100 条。
    /// 计数发生在解析请求体之前：乱发的请求也占名额。
    pub fn client_error_slot(&self, ip: &str, now: f64) -> bool {
        let mut c = self.client_err.lock().expect("client_err");
        if now - c.window > 600.0 {
            *c = ClientErr {
                window: now,
                ..Default::default()
            };
        }
        if c.total >= 100 || c.ip.get(ip).copied().unwrap_or(0) >= 10 {
            return false;
        }
        c.total += 1;
        *c.ip.entry(ip.to_string()).or_insert(0) += 1;
        true
    }

    /// 设备指纹上报的限流：每 IP 每 10 分钟 10 条，全站每 10 分钟 200 条。
    /// 和脚本报错分开计数；匿名请求不能通过打满 device-fp 把 /public/client-error 的预算吃掉。
    pub fn device_fp_slot(&self, ip: &str, now: f64) -> bool {
        let mut c = self.device_fp.lock().expect("device_fp");
        if now - c.window > 600.0 {
            *c = ClientErr {
                window: now,
                ..Default::default()
            };
        }
        if c.total >= 200 || c.ip.get(ip).copied().unwrap_or(0) >= 10 {
            return false;
        }
        c.total += 1;
        *c.ip.entry(ip.to_string()).or_insert(0) += 1;
        true
    }

    /// 清掉所有缓存与限流计数（测试用：改了设置后马上要看到新值）。
    pub fn invalidate(&self) {
        self.tx.send_replace(None);
        if let Ok(mut m) = self.meta.try_lock() {
            *m = None;
        }
        if let Ok(mut s) = self.status.try_lock() {
            *s = None;
        }
        *self.client_err.lock().expect("client_err") = ClientErr::default();
        *self.device_fp.lock().expect("device_fp") = ClientErr::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wait_minutes_like_python_regex() {
        assert_eq!(wait_minutes("今天的额度用完了，约 12 分钟后恢复"), 12);
        assert_eq!(wait_minutes("约 x 分钟，约 3 分钟"), 3);
        assert_eq!(wait_minutes("约 7 小时"), 1_000_000);
        assert_eq!(wait_minutes("没有数字"), 1_000_000);
    }

    #[test]
    fn site_interval_divides_by_usable_accounts() {
        let snap = |v: Value| Snapshot::new(v.as_object().unwrap().clone());
        let two = snap(
            json!({"cycle": 17.5, "accounts": [{"usable": true}, {"usable": true}, {"usable": false}]}),
        );
        assert_eq!(two.site_interval(), 8.75);
        // 今天出满的账号不再接图：不算
        let one = snap(
            json!({"cycle": 17.5, "accounts": [{"usable": true, "full": true}, {"usable": true, "full": false}]}),
        );
        assert_eq!(one.site_interval(), 17.5);
        // 没有可用账号也不除以 0；没有 cycle 时按默认 10 秒
        assert_eq!(
            snap(json!({"cycle": 17.5, "accounts": []})).site_interval(),
            17.5
        );
        assert_eq!(snap(json!({})).site_interval(), DEFAULT_CYCLE);
    }

    #[test]
    fn render_appends_me_last() {
        let mut body = Map::new();
        body.insert("t".into(), json!(1.5));
        body.insert("site".into(), json!("x"));
        let s = Snapshot::new(body);
        assert_eq!(s.render(None), r#"{"t":1.5,"site":"x","me":null}"#);
        assert_eq!(
            s.render(Some(&json!({"mine": [], "waiting": 0, "running": 0}))),
            r#"{"t":1.5,"site":"x","me":{"mine":[],"waiting":0,"running":0}}"#
        );
        let v = s.value(None);
        assert_eq!(v["me"], Value::Null);
    }

    #[test]
    fn signature_ignores_t_and_countdowns() {
        let mk = |t: f64, requests: i64, next_in: f64| {
            let mut m = Map::new();
            m.insert("t".into(), json!(t));
            m.insert("auth".into(), json!({"requests": requests}));
            m.insert(
                "guard".into(),
                json!({"hour": 3, "next_in": next_in, "cooldown": 0}),
            );
            Snapshot::new(m)
        };
        let base = mk(1.0, 5, 5.0);
        // 只有 t 不同：不算变化
        let later = mk(2.0, 5, 5.0);
        assert_eq!(base.sig, later.sig);
        assert_eq!(base.admin_sig, later.admin_sig);
        // 倒计时变了：公开页要推，后台不推
        let counting = mk(2.0, 5, 4.0);
        assert_ne!(base.sig, counting.sig);
        assert_eq!(base.admin_sig, counting.admin_sig);
        // 滚动窗口里的请求数变了：同上
        let busier = mk(2.0, 6, 5.0);
        assert_ne!(base.sig, busier.sig);
        assert_eq!(base.admin_sig, busier.admin_sig);
        // 真正的状态变了（每小时出图数）：两边都要推
        let mut m = Map::new();
        m.insert("t".into(), json!(2.0));
        m.insert("auth".into(), json!({"requests": 5}));
        m.insert(
            "guard".into(),
            json!({"hour": 4, "next_in": 5.0, "cooldown": 0}),
        );
        let changed = Snapshot::new(m);
        assert_ne!(base.sig, changed.sig);
        assert_ne!(base.admin_sig, changed.admin_sig);
    }

    #[test]
    fn client_error_budget() {
        let hub = Hub::new();
        for _ in 0..10 {
            assert!(hub.client_error_slot("203.0.113.9", 1000.0));
        }
        assert!(!hub.client_error_slot("203.0.113.9", 1001.0));
        assert!(hub.client_error_slot("203.0.113.10", 1001.0));
        // 超过 10 分钟窗口就重置
        assert!(hub.client_error_slot("203.0.113.9", 1601.0));
    }

    #[test]
    fn client_error_global_cap() {
        let hub = Hub::new();
        // 10 个来源 × 10 条 = 全站 100 条；再换新来源也不收
        for ip in 0..10 {
            for _ in 0..10 {
                assert!(hub.client_error_slot(&format!("198.51.100.{ip}"), 5000.0));
            }
        }
        assert!(!hub.client_error_slot("198.51.100.99", 5001.0));
        assert!(hub.client_error_slot("198.51.100.99", 5601.1));
    }

    #[test]
    fn device_fp_budget_is_independent_from_client_errors() {
        let hub = Hub::new();
        for ip in 0..10 {
            for _ in 0..10 {
                assert!(hub.client_error_slot(&format!("198.51.100.{ip}"), 1000.0));
            }
        }
        assert!(
            !hub.client_error_slot("203.0.113.10", 1001.0),
            "client-error 全站 100 条已满"
        );
        // device-fp 有自己的桶，不会被 client-error 打满
        for _ in 0..10 {
            assert!(hub.device_fp_slot("203.0.113.9", 1000.0));
        }
        assert!(!hub.device_fp_slot("203.0.113.9", 1001.0));
        assert!(hub.device_fp_slot("203.0.113.10", 1001.0));
    }
}
