//! 奶妹：网关事件循环、斜杠命令分发、向网关报心跳。
//!
//! 与 Discord 的交互次序：
//! - `/register /quota /resetkey /help`：先 `defer()`（3 秒内确认），再去问网关，最后用跟进消息（ephemeral）回话；
//! - 命令出错 → 打 `[bug]` 日志，并回一句「出了点问题……」（已确认过就用跟进消息，否则直接回复）；
//! - 不发私信、不加表情、不碰论坛，也不读频道里的消息（没有 GUILD_MESSAGES Intent，@奶妹 不会回应）；
//!   管理员命令都在网页后台，机器人里没有。

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use futures::StreamExt as _;
use serde_json::{Value, json};
use tracing::{debug, error, info, warn};
use twilight_gateway::{Config as GatewayConfig, Message, Shard, ShardId};
use twilight_model::gateway::CloseFrame;

use crate::api::{self, ApiError, DiscordApi};
use crate::bridge::{Bridge, who_payload};
use crate::commands::{self, Command, NAIMEI_INTENTS};
use crate::config::NaimeiConfig;
use crate::events::{
    self, Gateway, GuildInfo, INTERACTION_APPLICATION_COMMAND, Interaction, ReadyInfo, User,
};
use crate::handlers::{self, Backend, HandlerError};

/// 心跳间隔。
pub const SYNC_INTERVAL: Duration = Duration::from_secs(60);

/// 命令执行中可能出现的错误；全部会被统一接住：打日志，并回成员「出了点问题」。
#[derive(Debug, thiserror::Error)]
pub enum CmdError {
    /// 网关回的内容读不懂。
    #[error("{0}")]
    Handler(#[from] HandlerError),
    /// 回 Discord 失败。
    #[error("{0}")]
    Api(#[from] ApiError),
    /// 命令名不认识。
    #[error("{0}")]
    Invalid(String),
}

impl CmdError {
    /// 日志里的错误类别。
    fn kind(&self) -> &'static str {
        match self {
            CmdError::Handler(_) => "handler",
            CmdError::Api(_) => "discord_api",
            CmdError::Invalid(_) => "invalid_command",
        }
    }
}

#[derive(Default)]
struct Inner {
    bot: Option<User>,
    /// 配置的那个服务器的名字（只用来在心跳里显示）。
    guild_name: Option<String>,
    sync_started: bool,
}

/// 墙钟（Unix 秒）。测试里换成固定值，「Key 剩余有效期」才不会随时间漂移。
pub type Clock = Box<dyn Fn() -> f64 + Send + Sync>;

pub struct Naimei {
    pub cfg: NaimeiConfig,
    pub api: DiscordApi,
    pub bridge: Bridge,
    clock: Clock,
    /// 最近一次心跳延迟（毫秒）；`u64::MAX` = 还没有数据。
    latency_ms: AtomicU64,
    inner: Mutex<Inner>,
}

fn unix_now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

impl Naimei {
    /// `discord_proxy` 仅测试用，生产环境传 `None`。
    pub fn new(cfg: NaimeiConfig, discord_proxy: Option<String>) -> Arc<Self> {
        Self::with_clock(cfg, discord_proxy, Box::new(unix_now))
    }

    pub fn with_clock(cfg: NaimeiConfig, discord_proxy: Option<String>, clock: Clock) -> Arc<Self> {
        let api = DiscordApi::new(&cfg.token, discord_proxy);
        let bridge = Bridge::new(cfg.backend.clone(), cfg.secret.clone());
        Arc::new(Naimei {
            cfg,
            api,
            bridge,
            clock,
            latency_ms: AtomicU64::new(u64::MAX),
            inner: Mutex::new(Inner::default()),
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// 记录网关心跳延迟，上报时四舍五入成毫秒。
    pub fn set_latency(&self, latency: Option<Duration>) {
        let ms = latency.map_or(u64::MAX, |d| (d.as_secs_f64() * 1000.0).round() as u64);
        self.latency_ms.store(ms, Ordering::Relaxed);
    }

    fn now(&self) -> f64 {
        (self.clock)()
    }

    pub fn set_bot_user(&self, user: User) {
        self.lock().bot = Some(user);
    }

    /// GUILD_CREATE / GUILD_UPDATE：只关心配置的那个服务器的名字。
    pub fn upsert_guild(&self, g: GuildInfo) {
        if g.unavailable || g.id != self.cfg.guild_id.to_string() {
            return;
        }
        if let Some(name) = g.name {
            self.lock().guild_name = Some(name);
        }
    }

    /// GUILD_DELETE：被踢出配置的服务器后不再报它的名字。
    pub fn remove_guild(&self, id: &str) {
        if id == self.cfg.guild_id.to_string() {
            self.lock().guild_name = None;
        }
    }

    // ---------- 向网关要数据 ----------

    /// POST 到网关，请求体是 `{discord_id, guild_id, ...extra}`。
    async fn backend(
        &self,
        path: &str,
        it: &Interaction,
        extra: &[(&str, Value)],
    ) -> Result<Backend, CmdError> {
        let payload = who_payload(it, extra)?;
        Ok(self.bridge.command(path, &payload).await?)
    }

    // ---------- 交互入口 ----------

    /// 处理一条 INTERACTION_CREATE（斜杠命令）；其他类型忽略。
    pub async fn handle_interaction(self: &Arc<Self>, it: Interaction) {
        if it.kind == INTERACTION_APPLICATION_COMMAND {
            self.handle_command(&it).await;
        }
    }

    async fn handle_command(self: &Arc<Self>, it: &Interaction) {
        let data = it.data.as_ref();
        let name = data.and_then(|d| d.name.as_deref());
        // 只认斜杠命令（type 1）；右键菜单命令我们没有注册，等同找不到命令
        let is_chat_input = data.and_then(|d| d.kind).is_none_or(|k| k == 1);
        let command = name.filter(|_| is_chat_input).and_then(Command::from_name);
        let responder = Responder::new(self, it);
        let result = match command {
            Some(c) => self.run_command(c, it, &responder).await,
            None => Err(CmdError::Invalid(format!(
                "Application command '{}' not found",
                name.unwrap_or("?")
            ))),
        };
        if let Err(err) = result {
            // 打日志（监控会抓 [bug]），并告诉成员不是他的问题
            let shown: &str = match command {
                Some(c) => c.name(),
                None => name.unwrap_or("?"),
            };
            error!("[bug] bot command /{shown} failed: {}: {err}", err.kind());
            let text = handlers::GENERIC_ERROR;
            let sent = if responder.is_done() {
                responder.followup(text).await
            } else {
                responder.send_message(text).await
            };
            if let Err(e) = sent {
                debug!("错误提示也没发出去：{e}");
            }
        }
    }

    async fn run_command(
        self: &Arc<Self>,
        command: Command,
        it: &Interaction,
        r: &Responder<'_>,
    ) -> Result<(), CmdError> {
        match command {
            Command::Register => self.cmd_register(it, r).await,
            Command::ResetKey => self.cmd_resetkey(it, r).await,
        }
    }

    // ---------- 各命令 ----------

    /// `/register`。
    async fn cmd_register(&self, it: &Interaction, r: &Responder<'_>) -> Result<(), CmdError> {
        let invite = string_option_opt(it, "key")
            .unwrap_or_default()
            .trim()
            .to_owned();
        let same_guild = it
            .guild_id
            .as_deref()
            .and_then(|g| g.parse::<u64>().ok())
            .is_some_and(|g| g == self.cfg.guild_id);
        if !same_guild || it.actor().is_none() {
            return Ok(r.send_message(handlers::REGISTER_WRONG_GUILD).await?);
        }
        r.defer().await?;
        let payload = handlers::register_payload(it, &invite)?;
        // 直发：斜杠命令已由 Discord 验明身份，不走 OAuth，Key 只在本人可见的临时消息里给
        let message = match self.bridge.issue(&payload).await {
            Some((status, body)) => handlers::register_message(status, &body)?,
            None => handlers::REGISTER_UNAVAILABLE.to_owned(),
        };
        Ok(r.followup(&message).await?)
    }

    async fn cmd_resetkey(&self, it: &Interaction, r: &Responder<'_>) -> Result<(), CmdError> {
        r.defer().await?;
        let b = self.backend("/self-register/resetkey", it, &[]).await?;
        let text = if b.is_ok() {
            handlers::resetkey_text(&b.data)?
        } else {
            b.text()
        };
        Ok(r.followup(&text).await?)
    }

    // ---------- 启动 / 同步 ----------

    /// READY：把斜杠命令同步到指定服务器，然后（只启动一次）开始每分钟的心跳。
    pub async fn on_ready(self: Arc<Self>, ready: ReadyInfo) {
        let defs = commands::definitions();
        if let Err(e) = self
            .api
            .sync_guild_commands(&ready.application_id, self.cfg.guild_id, &defs)
            .await
        {
            // 命令没同步成功就不开始心跳（后台显示离线，站长能发现）
            error!("[bug] on_ready failed: {e}");
            return;
        }
        info!(
            "[bot] ready as {}, commands synced",
            ready.user.display_tag()
        );
        let start = {
            let mut inner = self.lock();
            !std::mem::replace(&mut inner.sync_started, true) // 断线重连也会再触发 READY，只启动一次
        };
        if start {
            tokio::spawn(self.clone().sync_loop());
        }
    }

    /// 每分钟报一次心跳（后台据此显示在线状态）。
    pub async fn sync_loop(self: Arc<Self>) {
        let ready_at = self.now();
        loop {
            if !self.sync_once(ready_at).await {
                warn!("[sync_loop] heart-beat not delivered");
            }
            tokio::time::sleep(SYNC_INTERVAL).await;
        }
    }

    /// 报一次心跳；网关收到（200）返回 true。
    pub async fn sync_once(&self, ready_at: f64) -> bool {
        let status = self.status_report(ready_at);
        self.bridge
            .bot("/report", &json!({"status": status}))
            .await
            .is_some()
    }

    /// 心跳里的 `status`。
    pub fn status_report(&self, ready_at: f64) -> Value {
        let inner = self.lock();
        let user = inner
            .bot
            .as_ref()
            .map(User::display_tag)
            .unwrap_or_else(|| "None".into());
        let latency = match self.latency_ms.load(Ordering::Relaxed) {
            u64::MAX => 0,
            ms => ms,
        };
        json!({
            "user": user,
            "guild": inner.guild_name.clone().unwrap_or_default(),
            "latency_ms": latency,
            "ready_at": ready_at,
        })
    }
}

/// 一次交互的回复器：记录是否已经响应过。
pub struct Responder<'a> {
    app: &'a Naimei,
    it: &'a Interaction,
    done: AtomicBool,
}

impl<'a> Responder<'a> {
    pub fn new(app: &'a Naimei, it: &'a Interaction) -> Self {
        Responder {
            app,
            it,
            done: AtomicBool::new(false),
        }
    }

    pub fn is_done(&self) -> bool {
        self.done.load(Ordering::Relaxed)
    }

    async fn callback(&self, body: Value) -> Result<(), ApiError> {
        self.app
            .api
            .callback(&self.it.id, &self.it.token, &body)
            .await?;
        self.done.store(true, Ordering::Relaxed);
        Ok(())
    }

    /// 先确认收到：「正在思考」（类型 5）。
    pub async fn defer(&self) -> Result<(), ApiError> {
        self.callback(api::defer_body()).await
    }

    /// 直接回复一条只有本人可见的消息。
    pub async fn send_message(&self, content: &str) -> Result<(), ApiError> {
        self.callback(api::message_body(content)).await
    }

    /// 跟进消息（只有本人可见）。
    pub async fn followup(&self, content: &str) -> Result<(), ApiError> {
        self.app
            .api
            .followup(
                &self.it.application_id,
                &self.it.token,
                &api::followup_body(content),
            )
            .await
    }
}

// ---------- 选项解析 ----------

fn string_option_opt(it: &Interaction, name: &str) -> Option<String> {
    it.option(name)
        .and_then(|o| o.value.as_ref())
        .and_then(Value::as_str)
        .map(str::to_owned)
}

// ---------- 事件循环 ----------

/// 等 SIGINT / SIGTERM（docker stop）。
pub async fn wait_for_shutdown() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let (Ok(mut term), Ok(mut int)) = (
            signal(SignalKind::terminate()),
            signal(SignalKind::interrupt()),
        ) else {
            std::future::pending::<()>().await;
            return;
        };
        tokio::select! {
            _ = term.recv() => {}
            _ = int.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

/// 分发一条网关文本消息。
pub fn dispatch(app: &Arc<Naimei>, raw: &str) {
    match events::parse(raw) {
        Ok(Gateway::Ready(ready)) => {
            app.set_bot_user(ready.user.clone());
            tokio::spawn(app.clone().on_ready(ready));
        }
        Ok(Gateway::GuildCreate(g) | Gateway::GuildUpdate(g)) => app.upsert_guild(g),
        Ok(Gateway::GuildDelete(id)) => app.remove_guild(&id),
        Ok(Gateway::Interaction(it)) => {
            let app = app.clone();
            tokio::spawn(async move { app.handle_interaction(*it).await });
        }
        Ok(Gateway::Other) => {}
        Err(e) => warn!("无法解析的网关消息：{e}"),
    }
}

/// 运行奶妹，直到收到停止信号（正常返回）或网关致命关闭（Token 无效 / Intent 未开通等，返回 Err）。
pub async fn run(cfg: NaimeiConfig) -> anyhow::Result<()> {
    let app = Naimei::new(cfg.clone(), None);
    let config = GatewayConfig::new(cfg.token.clone(), NAIMEI_INTENTS);
    let mut shard = Shard::with_config(ShardId::ONE, config);
    let shutdown = wait_for_shutdown();
    tokio::pin!(shutdown);
    let mut closing = false;
    loop {
        let item = tokio::select! {
            item = shard.next() => item,
            () = &mut shutdown, if !closing => {
                info!("[bot] 收到停止信号，正在断开网关");
                shard.close(CloseFrame::NORMAL);
                closing = true;
                continue;
            }
        };
        match item {
            Some(Ok(Message::Text(raw))) => {
                app.set_latency(shard.latency().recent().first().copied());
                dispatch(&app, &raw);
            }
            Some(Ok(Message::Close(frame))) => {
                info!("[bot] 网关连接关闭：{frame:?}");
            }
            Some(Err(e)) => warn!("[bot] 网关消息出错：{e}"),
            None if closing => return Ok(()),
            None => anyhow::bail!(
                "网关连接被致命关闭（常见原因：DISCORD_BOT_TOKEN 无效或 Intent 未开通），详见上面的关闭码"
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> Arc<Naimei> {
        let cfg = NaimeiConfig {
            token: "t".into(),
            guild_id: 900000000000000001,
            backend: "http://127.0.0.1:1".into(),
            site: "https://site.test".into(),
            secret: "s".into(),
        };
        Naimei::new(cfg, None)
    }

    fn guild(id: &str, name: Option<&str>, unavailable: bool) -> GuildInfo {
        GuildInfo {
            id: id.into(),
            name: name.map(str::to_owned),
            unavailable,
        }
    }

    #[tokio::test]
    async fn only_the_configured_guild_name_is_kept() {
        let app = app();
        app.upsert_guild(guild("900000000000000009", Some("别的服"), false));
        assert_eq!(app.status_report(1.0)["guild"], "");
        app.upsert_guild(guild("900000000000000001", None, true));
        assert_eq!(app.status_report(1.0)["guild"], "", "掉线的服务器没有名字");
        app.upsert_guild(guild("900000000000000001", Some("测试服"), false));
        assert_eq!(app.status_report(1.0)["guild"], "测试服");
        app.remove_guild("900000000000000009");
        assert_eq!(app.status_report(1.0)["guild"], "测试服");
        app.remove_guild("900000000000000001");
        assert_eq!(app.status_report(1.0)["guild"], "");
    }

    #[tokio::test]
    async fn latency_is_reported_in_whole_milliseconds() {
        let app = app();
        assert_eq!(app.status_report(1.0)["latency_ms"], 0, "还没有数据按 0 报");
        app.set_latency(Some(Duration::from_secs_f64(0.0425)));
        assert_eq!(app.status_report(1.0)["latency_ms"], 43);
    }
}
