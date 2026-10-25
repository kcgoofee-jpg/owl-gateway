//! 应用状态：启动时构造一次，`Arc` 共享给所有请求与后台任务（没有全局变量）。
//! 启动顺序：建表迁移 → 运行期限制 → 上游 Token 标识迁移 → Token 设置 →
//! 账号保护 → 回填最近 3 小时出图 → Key 暂停 → 全站冷却 → 停机检测 → 后台密码。

use crate::alerts::Alerter;
use crate::bugs::Bugs;
use crate::env::EnvConfig;
use crate::guard::Guard;
use crate::hooks::GateHooks;
use crate::middleware::StatusStats;
use crate::registration::RegistrationService;
use crate::runtime::{
    AuthGuard, ImageCooldown, KeyPauses, SlidingWindow, TagQueue, UpstreamHealth,
};
use crate::web::Assets;
use gate_core::clock::Clock;
use gate_db::Db;
use gate_upstream::client::{Config as UpConfig, NaiClient};
use std::collections::HashMap;
use std::ops::Deref;
use std::sync::atomic::AtomicI64;
use std::sync::{Arc, Mutex, RwLock};
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// 运行期可在后台调的限制。每 Key 出图间隔 `key_image_min_interval`、
/// 账号出图间隔 `image_min_interval`（10-23，出图节奏只剩账号保护的「每张出完后歇 N～M 秒」）已删除，
/// 库里残留的 `runtime_*` 行不再读。
pub const RUNTIME_LIMIT_BOUNDS: [(&str, i64, i64); 2] = [
    ("queue_timeout", 15, 300),
    ("image_429_cooldown_seconds", 60, 3600),
];

/// 运行期限制的当前值（启动时是 [`crate::env`] 里的代码常量，之后可被后台覆盖）。
#[derive(Debug, Clone, Copy)]
pub struct RuntimeLimits {
    pub queue_timeout: f64,
    pub image_429_cooldown_seconds: f64,
}

/// 一次出图的额度预留（排队等待期间就占住，防止并发请求一起超额）。
#[derive(Debug, Clone)]
pub struct Reservation {
    pub key_id: i64,
    pub exclude_global_v5: bool,
    pub anlas: f64,
    pub v5: i64,
    pub legacy: i64,
}

#[derive(Default, Clone)]
pub struct Budget {
    pub next_id: u64,
    pub reservations: HashMap<u64, Reservation>,
}

#[derive(Clone)]
pub struct AppState(Arc<Inner>);

pub struct Inner {
    pub env: EnvConfig,
    pub db: Db,
    pub clock: Clock,
    pub assets: Assets,
    pub status_stats: StatusStats,
    pub alerter: Arc<Alerter>,
    pub bugs: Arc<Bugs>,
    pub guard: Arc<Guard>,
    pub nai: NaiClient<GateHooks>,
    pub limits: RwLock<RuntimeLimits>,
    pub rpm: SlidingWindow<i64>,
    pub login_attempts: SlidingWindow<String>,
    pub auth_guard: AuthGuard,
    pub cooldown: ImageCooldown,
    pub pauses: KeyPauses,
    pub tags: TagQueue,
    /// 后台运营接口的进程内状态（测试告警节流、运行期限制写锁、Anlas 核对互斥）。
    pub ops: crate::ops::Shared,
    /// V5 官方免费额度缓存（只有真实出图会刷新）。
    pub allowance: crate::allowance::ServerAllowance,
    pub health: UpstreamHealth,
    /// 每把 Key 的信号量（`KEY_CONCURRENCY`）和全站信号量都已删除：出图由排队管、标签补全由补全队列管。
    pub global_waiting: AtomicI64,
    pub global_active: AtomicI64,
    /// 额度预留：只在核对 / 修改预留时持锁，绝不跨上游 HTTP 持有。
    pub budget: tokio::sync::Mutex<Budget>,
    pub budget_idle: tokio::sync::Notify,
    /// 后台改过的密码（scrypt 哈希）；None 时用 ADMIN_PASSWORD。
    pub admin_pw_hash: RwLock<Option<String>>,
    /// 会话签名密钥（SECRET_KEY 或 data/secret_key）。
    pub secret_key: String,
    /// 记账等必须写完的后台任务（关停时等待）。
    pub tasks: TaskTracker,
    pub shutdown: CancellationToken,
    /// 近 1 分钟的请求结果（首页实时图）。
    pub live_events: Mutex<std::collections::VecDeque<(f64, String)>>,
    /// 实时看板快照（`/public/live` 与 SSE 共用）、`/public/status` 缓存、前端报错限流。
    pub live: crate::live::Hub,
    /// Discord 自助领 Key（`env.registration` 配齐才有；否则 `/self-register/*` 回 503）。
    pub registration: Option<Arc<RegistrationService>>,
    /// 定时任务：每个任务最近一轮的结果、动态额度重算的串行锁、官方状态地址的覆盖。
    pub jobs: crate::jobs::JobsState,
}

impl Deref for AppState {
    type Target = Inner;
    fn deref(&self) -> &Inner {
        &self.0
    }
}

/// 会话签名密钥：SECRET_KEY 优先；否则读 data/secret_key（≥ 32 字符），缺失或损坏就生成新的并原子写入。
pub fn resolve_secret(env: &EnvConfig) -> std::io::Result<String> {
    if !env.secret_key.is_empty() {
        return Ok(env.secret_key.clone());
    }
    let f = env.data_dir.join("secret_key");
    let existing = std::fs::read_to_string(&f)
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    if existing.chars().count() >= 32 {
        return Ok(existing);
    }
    let fresh = hex::encode(rand::random::<[u8; 32]>());
    crate::fsutil::write_private(&f, fresh.as_bytes())?;
    Ok(fresh)
}

impl AppState {
    /// 构造并完成启动步骤。
    pub async fn build(env: EnvConfig, assets: Assets) -> anyhow::Result<Self> {
        std::fs::create_dir_all(&env.data_dir)?;
        let now = crate::time::now();
        let db = Db::open(env.db_path())?;
        let clock = Clock::service();
        let alerter = Arc::new(Alerter::owner(&env));
        let tasks = TaskTracker::new();
        let bugs = Arc::new(Bugs::new(db.clone(), tasks.clone()));
        let guard = Arc::new(Guard::new(
            db.clone(),
            gate_core::clock::TZ,
            gate_core::clock::DAY_RESET_HOUR,
        ));

        // 上游 Token：后台接管过就以 upstream_tokens.json 为准，否则用 NAI_TOKENS。
        // V5 日限额、启停在后台按 Token 设（存库，见 startup 的 apply_saved）；出图并发固定 1
        let managed = gate_upstream::token_store::load(&env.upstream_tokens_path());
        let tokens: Vec<(String, i64, bool)> = match &managed {
            Some(entries) => entries
                .iter()
                .map(|e| (e.token.clone(), 0, e.allow_anlas))
                .collect(),
            None => env
                .nai_tokens
                .iter()
                .enumerate()
                .map(|(i, t)| {
                    (
                        t.clone(),
                        0,
                        env.nai_token_allow_anlas.get(i).copied().unwrap_or(true),
                    )
                })
                .collect(),
        };
        let hooks = GateHooks::new(db.clone(), clock.clone(), guard.clone(), alerter.clone());
        let nai = NaiClient::new(
            UpConfig {
                image_host: env.image_host.clone(),
                proxy: Some(env.upstream_proxy.clone()).filter(|s| !s.is_empty()),
                pace: env.pace_images,
            },
            hooks,
            tokens,
        )?;
        let limits = RuntimeLimits {
            queue_timeout: env.queue_timeout as f64,
            image_429_cooldown_seconds: env.image_429_cooldown_seconds,
        };
        let secret_key = resolve_secret(&env)?;
        let allowance = crate::allowance::ServerAllowance::new(db.clone());
        let registration = RegistrationService::from_env(&env, db.clone()).map(Arc::new);
        let state = Self(Arc::new(Inner {
            clock,
            assets,
            status_stats: StatusStats::default(),
            alerter,
            bugs,
            guard,
            nai,
            limits: RwLock::new(limits),
            rpm: SlidingWindow::new(1024),
            login_attempts: SlidingWindow::new(2048),
            auth_guard: AuthGuard::default(),
            cooldown: ImageCooldown::default(),
            pauses: KeyPauses::default(),
            tags: TagQueue::default(),
            ops: crate::ops::Shared::default(),
            allowance,
            health: UpstreamHealth::default(),
            global_waiting: AtomicI64::new(0),
            global_active: AtomicI64::new(0),
            budget: tokio::sync::Mutex::new(Budget::default()),
            budget_idle: tokio::sync::Notify::new(),
            admin_pw_hash: RwLock::new(None),
            secret_key,
            tasks,
            shutdown: CancellationToken::new(),
            live_events: Mutex::new(std::collections::VecDeque::new()),
            live: crate::live::Hub::new(),
            registration,
            jobs: crate::jobs::JobsState::new(),
            env,
            db,
        }));
        state.startup(now).await?;
        Ok(state)
    }

    async fn startup(&self, now: f64) -> anyhow::Result<()> {
        self.load_runtime_limits().await?;
        let tokens = self.nai.tokens().await;
        // 重启前被 429 / 403 冷却的账号接着冷却，不在部署后立刻再去试探
        let saved = gate_upstream::pool::decode_cooldowns(
            self.db
                .get_setting(crate::hooks::COOLDOWNS_KEY)
                .await?
                .as_deref(),
        );
        for t in &tokens {
            if let Some(c) = saved.get(&t.token_id) {
                t.restore_cooldown(*c, now);
            }
        }
        let limits = self.db.get_upstream_token_limits().await?;
        let enabled = self.db.get_upstream_token_enabled().await?;
        // 出图并发固定 1（NovelAI 一个账号同时只出 1 张，多了只会撞并发锁；10-23 删掉后台的并发设置，
        // 库里 upstream_token_image_concurrency 的旧行不再读）
        self.nai
            .apply_saved(
                &limits.into_iter().collect(),
                &enabled.into_iter().collect(),
            )
            .await;
        let invalid_tokens = crate::hooks::decode_invalid_tokens(
            self.db
                .get_setting(crate::hooks::INVALID_TOKENS_KEY)
                .await?
                .as_deref(),
        )
        .map_err(anyhow::Error::msg)?;
        self.nai.restore_unauthorized(&invalid_tokens).await;
        self.guard
            .load()
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        self.pauses.load(self.db.active_pauses(now).await?);
        self.cooldown.load(
            self.db
                .get_setting("image_cooldown_until")
                .await?
                .as_deref(),
            now,
        );
        // 停机检测：网关最后一次有动静（请求日志、定时任务跑完）距今超过 1 小时 → 从现在重新起算闲置天数
        // （不然停机 ≥ 回收天数后一重启就把所有人删光）。只看请求日志的话，安静时段凌晨没人发请求时的
        // 一次重启也会被当成停机（10-14 清晨就这样把所有人的闲置天数重置了一次）。
        let last_job = crate::jobs::JobsState::latest_run_at(
            self.db
                .get_setting(crate::jobs::registry::STATE_KEY)
                .await?
                .as_deref(),
        );
        let last_seen = self
            .db
            .last_log_ts()
            .await?
            .into_iter()
            .chain(last_job)
            .reduce(f64::max);
        if let Some(last) = last_seen
            && now - last > 3600.0
        {
            self.db
                .set_setting(
                    "key_inactivity_grace_started_at",
                    &gate_core::pyc::float_repr(now),
                )
                .await?;
            self.db
                .add_admin_action(
                    "系统",
                    "闲置计时重置",
                    "",
                    &format!(
                        "网关停机约 {:.1} 小时，闲置天数从现在重新计算",
                        (now - last) / 3600.0
                    ),
                    true,
                    now,
                )
                .await?;
        }
        *self.admin_pw_hash.write().expect("pw") =
            self.db.get_setting("admin_password_hash").await?;
        if self.env.admin_password.is_empty() {
            tracing::warn!("未设置 ADMIN_PASSWORD，/admin 管理端将无法登录！");
        }
        if tokens.is_empty() {
            tracing::warn!("未设置 NAI_TOKENS，所有生成请求将返回 503");
        }
        Ok(())
    }

    /// 后台保存过的运行期限制在每次重启后优先于代码常量（越界值忽略）。
    pub async fn load_runtime_limits(&self) -> anyhow::Result<()> {
        let mut values = Vec::new();
        for (name, lo, hi) in RUNTIME_LIMIT_BOUNDS {
            if let Some(raw) = self.db.get_setting(&format!("runtime_{name}")).await?
                && let Some(v) = gate_core::pyc::parse_int(&raw).filter(|v| (lo..=hi).contains(v))
            {
                values.push((name, v));
            }
        }
        self.apply_runtime_limits(&values).await;
        Ok(())
    }

    pub async fn apply_runtime_limits(&self, values: &[(&str, i64)]) {
        for (name, v) in values {
            let v = *v as f64;
            match *name {
                "queue_timeout" => self.limits.write().expect("limits").queue_timeout = v,
                "image_429_cooldown_seconds" => {
                    self.limits
                        .write()
                        .expect("limits")
                        .image_429_cooldown_seconds = v
                }
                _ => {}
            }
        }
    }

    pub fn limits(&self) -> RuntimeLimits {
        *self.limits.read().expect("limits")
    }

    pub fn now(&self) -> f64 {
        crate::time::now()
    }

    /// 当前服务日（7 点切换）。
    pub fn day(&self) -> String {
        self.clock.day(self.now())
    }

    pub fn month(&self) -> String {
        self.clock.month(self.now())
    }

    /// 首页「近 1 分钟」的请求结果。
    pub fn live_note(&self, status: &str) {
        let mut e = self.live_events.lock().expect("live");
        e.push_back((self.now(), status.to_string()));
        while e.len() > 5000 {
            e.pop_front();
        }
    }

    /// 上游 429：暂停全站图片请求（写库 + 写一行日志），返回剩余冷却秒数。
    pub async fn block_image_generation(&self, retry_after: f64) -> i64 {
        let now = self.now();
        let until = self.cooldown.block(retry_after, now);
        if let Err(e) = self
            .db
            .set_setting("image_cooldown_until", &gate_core::pyc::float_repr(until))
            .await
        {
            self.bugs.capture(crate::bugs::Report {
                source: "cooldown_save",
                error: Some(("DbError", &e.to_string())),
                ..Default::default()
            });
        }
        let remaining = ((until - now) as i64).max(1);
        tracing::warn!("NovelAI 对图片请求返回 429（限流），全站生图暂停 {remaining} 秒");
        remaining
    }

    /// 记一次上游调用结果（状态页的「上游」状态按最近 10 分钟算）。
    pub fn record_upstream(&self, ok: bool) {
        self.health.record(ok, self.now());
    }
}
