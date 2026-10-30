//! 奶妹：网关事件循环、斜杠命令分发、向网关报心跳，以及（配置了 `NAIMEI_LLM_*` 时的）
//! 「@奶妹 / 回复奶妹」AI 对话。
//!
//! 与 Discord 的交互次序：
//! - `/register /resetkey`：先 `defer()`（3 秒内确认），再去问网关，最后用跟进消息（ephemeral）回话；
//! - 命令出错 → 打 `[bug]` 日志，并回一句「出了点问题……」（已确认过就用跟进消息，否则直接回复）；
//! - AI 对话：成员 @奶妹 或回复奶妹的消息 → 检索知识库 → 调 LLM → 出口过滤 → 分片公开发送。
//!   只在配置的服务器里说话；不私信、不点赞、不碰论坛；一个频道同时只处理一条、冷却 3 秒；
//!   管理员命令都在网页后台，机器人里没有。

use std::collections::{HashMap, HashSet, VecDeque};
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
use crate::chat::{self, Chat, ChatError, Turn};
use crate::chatlog::ChatLog;
use crate::commands::{self, Command, NAIMEI_INTENTS};
use crate::config::NaimeiConfig;
use crate::events::{
    self, Gateway, GuildInfo, INTERACTION_APPLICATION_COMMAND, Interaction, MessageInfo, ReadyInfo,
    User,
};
use crate::handlers::{self, Backend, HandlerError};
use crate::kb;

/// 心跳间隔。
pub const SYNC_INTERVAL: Duration = Duration::from_secs(60);

/// 同一频道两次对话至少隔 3 秒（防连点刷屏）。
const CHAT_COOLDOWN: f64 = 3.0;
/// 流式渐进编辑的最小间隔（Discord 编辑限流 5 次/5 秒每频道，2 秒很安全）。
const EDIT_INTERVAL: tokio::time::Duration = tokio::time::Duration::from_secs(2);
/// 每个频道保留的最近对话轮数（作为上下文）。
const CHAT_HISTORY_TURNS: usize = 6;
/// 最多记住多少个频道的对话；超出就整表清空（防内存无限涨）。
const CHAT_MAX_CHANNELS: usize = 128;
/// Discord 单条消息长度上限（2000 字符，留了余量）。
const DISCORD_MESSAGE_LIMIT: usize = 1900;
/// 上游额度/并发不足时的兜底话术（不透露任何内部实现）。
const CHAT_QUOTA: &str = "呜——我的脑子今天被大家用光光啦 (´•̥ω•̥`) 过一阵再来戳我吧～";
/// 其他对话失败时的兜底话术。
const CHAT_FAILED: &str = "呜，刚才走神了……再戳我一次试试 (´･ω･`)";

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
    // ---------- AI 对话状态 ----------
    /// 奶妹自己的用户 ID（READY 里来），对话触发判断用。
    bot_id: Option<String>,
    /// 每个频道的最近对话：频道 ID → (成员说的, 奶妹答的) 队列。
    histories: HashMap<u64, VecDeque<Turn>>,
    /// 正在生成回复的频道：一个频道同时只处理一条，多出来的直接忽略。
    busy: HashSet<u64>,
    /// 每个频道上次对话的 Unix 秒（冷却用）。
    last_chat: HashMap<u64, f64>,
}

/// 墙钟（Unix 秒）。测试里换成固定值，「Key 剩余有效期」才不会随时间漂移。
pub type Clock = Box<dyn Fn() -> f64 + Send + Sync>;

pub struct Naimei {
    pub cfg: NaimeiConfig,
    pub api: DiscordApi,
    pub bridge: Bridge,
    /// AI 对话客户端；未配置 `NAIMEI_LLM_*` 时为 `None`，对话功能整体关闭。
    chat: Option<Chat>,
    /// 对话留档；未配置 `NAIMEI_CHAT_LOG` 时为 `None`。
    chat_log: Option<ChatLog>,
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
        let chat = cfg.chat.as_ref().map(|c| Chat::new(c.clone()));
        let chat_log = cfg.chat_log.as_ref().map(|p| ChatLog::new(p));
        Arc::new(Naimei {
            cfg,
            api,
            bridge,
            chat,
            chat_log,
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
        let mut inner = self.lock();
        inner.bot_id = Some(user.id.clone());
        inner.bot = Some(user);
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

    // ---------- AI 对话 ----------

    /// 处理一条 MESSAGE_CREATE：@奶妹 或回复奶妹 → AI 对话；其余一律忽略。
    pub async fn handle_message(self: &Arc<Self>, m: MessageInfo) {
        let Some(bot_id) = self.lock().bot_id.clone() else {
            return; // READY 之前收到的消息不带触发信息
        };
        let Some(chat) = self.chat.as_ref() else {
            return; // 未配置 NAIMEI_LLM_*，对话功能关闭
        };
        let Some(author) = m.author.as_ref() else {
            return;
        };
        if author.bot || author.id == bot_id {
            return; // 自己 / 其他机器人一律不回，防自环
        }
        let same_guild = m
            .guild_id
            .as_deref()
            .and_then(|g| g.parse::<u64>().ok())
            .is_some_and(|g| g == self.cfg.guild_id);
        if !same_guild {
            return; // 只在配置的服务器里说话
        }
        if !is_chat_trigger(&m, &bot_id) {
            return;
        }
        // 图片：自己的附件 + 被引用消息的附件（只收 image/*，最多 4 张）
        let image_urls = |atts: &[events::Attachment]| -> Vec<String> {
            atts.iter()
                .filter(|a| {
                    a.content_type
                        .as_deref()
                        .is_some_and(|t| t.starts_with("image/"))
                })
                .map(|a| a.url.clone())
                .collect()
        };
        let mut images = image_urls(&m.attachments);
        // 引用上下文：被回复消息的内容（和它的图）对模型可见，不然「fix it」没法修
        let mut text = strip_mention(&m.content, &bot_id).trim().to_owned();
        if let Some(r) = m.referenced_message.as_ref() {
            let who = r
                .author
                .as_ref()
                .map(User::display_tag)
                .unwrap_or_else(|| "某人".into());
            let quoted: String = r.content.trim().chars().take(600).collect();
            if !quoted.is_empty() {
                text = format!("[引用 {who}：{quoted}]\n{text}");
            }
            for url in image_urls(&r.attachments) {
                if images.len() >= 4 {
                    break;
                }
                images.push(url);
            }
        }
        if text.is_empty() && images.is_empty() {
            return; // 只 @ 不说话（纯表情）不触发
        }
        if text.is_empty() {
            text = "(看这张图)".to_owned(); // 纯图消息：给模型一个明确指令
        }
        images.truncate(4);
        let Ok(channel_id) = m.channel_id.parse::<u64>() else {
            return;
        };
        let now = self.now();
        {
            let mut inner = self.lock();
            if inner.busy.contains(&channel_id)
                || inner
                    .last_chat
                    .get(&channel_id)
                    .is_some_and(|t| now - *t < CHAT_COOLDOWN)
            {
                return; // 正在回复 / 冷却中：直接忽略，不排队（宁可漏答不可刷屏）
            }
            inner.busy.insert(channel_id);
            inner.last_chat.insert(channel_id, now);
            if inner.histories.len() > CHAT_MAX_CHANNELS {
                inner.histories.clear();
            }
        }
        let is_owner = author
            .id
            .parse::<u64>()
            .is_ok_and(|id| self.cfg.owner_ids.contains(&id));
        let req = ChatRequest {
            channel_id,
            message_id: &m.id,
            text: &text,
            images,
            is_owner,
            author_id: &author.id,
            author_name: &author.display_tag(),
        };
        let result = self.chat_once(chat, req).await;
        self.lock().busy.remove(&channel_id);
        match result {
            Ok(()) => {}
            Err(ChatError::Quota) => {
                self.send_chat_message(channel_id, &m.id, CHAT_QUOTA).await;
            }
            Err(e) => {
                // 额度类失败是常态，不打 [bug]；其他失败记日志但不惊动成员
                info!("[chat] 对话失败：{e}");
                self.send_chat_message(channel_id, &m.id, CHAT_FAILED).await;
            }
        }
    }

    /// 完整一轮对话：发占位 → 流式生成（渐进编辑占位）→ 终稿（超长则首片编辑、其余分片发送）→ 记历史。
    /// 失败时把占位改成兜底文案（或直接发兜底），返回 Err 只用于日志。
    async fn chat_once(
        self: &Arc<Self>,
        chat: &Chat,
        req: ChatRequest<'_>,
    ) -> Result<(), ChatError> {
        let history: Vec<Turn> = self
            .lock()
            .histories
            .get(&req.channel_id)
            .map(|h| {
                h.iter()
                    .rev()
                    .take(CHAT_HISTORY_TURNS)
                    .rev()
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        let stickers = chat::wants_stickers(req.text).then(kb::nailong);
        let system = chat::system_prompt(&kb::retrieve(req.text, 2), stickers, req.is_owner);
        // 占位消息：流式生成的画布；发不出去就退回一次性整段发送
        let placeholder = match self
            .api
            .create_message_get_id(
                req.channel_id,
                &json!({
                    "content": "…",
                    "allowed_mentions": {"parse": ["users"]},
                    "message_reference": {"req.message_id": req.message_id, "type": 0},
                }),
            )
            .await
        {
            Ok(id) => Some(id),
            Err(e) => {
                debug!("[chat] 占位消息发送失败，退回整段发送：{e}");
                None
            }
        };
        // 渐进编辑器：上游增量 → 限频（EDIT_INTERVAL）编辑占位；scrub 之后的预览才允许上屏
        let (tx, mut rx) = tokio::sync::mpsc::channel::<String>(16);
        let editor = placeholder.as_ref().map(|pid| {
            let api = self.api.clone();
            let pid = pid.clone();
            tokio::spawn(async move {
                let mut last = tokio::time::Instant::now() - EDIT_INTERVAL;
                let mut shown_last = String::new();
                while let Some(acc) = rx.recv().await {
                    if last.elapsed() < EDIT_INTERVAL {
                        continue;
                    }
                    last = tokio::time::Instant::now();
                    let shown = chat::preview_cut(&chat::scrub(&acc), DISCORD_MESSAGE_LIMIT);
                    if shown == shown_last {
                        continue;
                    }
                    shown_last = shown.clone();
                    let _ = api
                        .edit_message(req.channel_id, &pid, &json!({"content": shown}))
                        .await;
                }
            })
        });
        let started = std::time::Instant::now();
        let user_content = chat::user_content(req.text, &req.images);
        let reply = chat
            .reply_stream(&system, &history, user_content, move |acc| {
                let _ = tx.try_send(acc);
            })
            .await;
        drop(editor);
        match reply {
            Ok(answer) => {
                let answer = chat::scrub(&answer);
                {
                    let mut inner = self.lock();
                    let h = inner.histories.entry(req.channel_id).or_default();
                    h.push_back((req.text.to_owned(), answer.clone()));
                    while h.len() > CHAT_HISTORY_TURNS {
                        h.pop_front();
                    }
                }
                if let Some(log) = self.chat_log.as_ref() {
                    log.record(
                        self.cfg.guild_id,
                        req.channel_id,
                        req.author_id,
                        req.author_name,
                        req.is_owner,
                        &if req.images.is_empty() {
                            req.text.to_owned()
                        } else {
                            format!("{}（附图{}张）", req.text, req.images.len())
                        },
                        &answer,
                        chat.model(),
                        started.elapsed().as_millis(),
                        "ok",
                    );
                }
                let parts = chat::split_for_discord(&answer, DISCORD_MESSAGE_LIMIT);
                match &placeholder {
                    Some(pid) => {
                        // 首片编辑进占位（保留对原消息的回复关系），其余分片发新消息
                        if let Err(e) = self
                            .api
                            .edit_message(
                                req.channel_id,
                                pid,
                                &json!({
                                    "content": &parts[0],
                                    "allowed_mentions": {"parse": ["users"]},
                                }),
                            )
                            .await
                        {
                            warn!("[chat] 终稿编辑失败：{e}");
                        }
                        for part in &parts[1..] {
                            if let Err(e) = self
                                .api
                                .create_message(
                                    req.channel_id,
                                    &json!({
                                        "content": part,
                                        "allowed_mentions": {"parse": ["users"]},
                                    }),
                                )
                                .await
                            {
                                warn!("[chat] 续片发送失败：{e}");
                            }
                        }
                    }
                    None => {
                        for (i, part) in parts.iter().enumerate() {
                            let mut body = json!({
                                "content": part,
                                "allowed_mentions": {"parse": ["users"]},
                            });
                            if i == 0 {
                                body["message_reference"] =
                                    json!({"req.message_id": req.message_id, "type": 0});
                            }
                            if let Err(e) = self.api.create_message(req.channel_id, &body).await {
                                warn!("[chat] 回复第 {} 片发送失败：{e}", i + 1);
                            }
                        }
                    }
                }
                Ok(())
            }
            Err(e) => {
                // 占位还在生成态：把它改成兜底文案，成员看到的是同一条消息的最终状态
                let fallback = match e {
                    ChatError::Quota => CHAT_QUOTA,
                    _ => CHAT_FAILED,
                };
                match &placeholder {
                    Some(pid) => {
                        if let Err(e) = self
                            .api
                            .edit_message(
                                req.channel_id,
                                pid,
                                &json!({
                                    "content": fallback,
                                    "allowed_mentions": {"parse": ["users"]},
                                }),
                            )
                            .await
                        {
                            warn!("[chat] 兜底编辑失败：{e}");
                        }
                    }
                    None => {
                        self.send_chat_message(req.channel_id, req.message_id, fallback)
                            .await
                    }
                }
                Err(e)
            }
        }
    }

    /// 对话的兜底回复（额度用完 / 其他失败），不带知识库，直接以回复消息发出。
    async fn send_chat_message(&self, channel_id: u64, reply_to: &str, content: &str) {
        let body = json!({
            "content": content,
            "allowed_mentions": {"parse": ["users"]},
            "message_reference": {"message_id": reply_to, "type": 0},
        });
        if let Err(e) = self.api.create_message(channel_id, &body).await {
            warn!("[chat] 兜底消息发送失败：{e}");
        }
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

/// 一轮对话的上下文：handle_message 收集，chat_once 消费。
struct ChatRequest<'a> {
    channel_id: u64,
    message_id: &'a str,
    text: &'a str,
    images: Vec<String>,
    is_owner: bool,
    author_id: &'a str,
    author_name: &'a str,
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

/// 是否触发 AI 对话：被 @ 或回复的对象是奶妹本人。其余消息（哪怕内容里提到奶妹）不触发。
fn is_chat_trigger(m: &MessageInfo, bot_id: &str) -> bool {
    let mentioned = m.mentions.iter().any(|u| u.id == bot_id);
    let reply_to_bot = m
        .referenced_message
        .as_ref()
        .and_then(|r| r.author.as_ref())
        .is_some_and(|a| a.id == bot_id);
    mentioned || reply_to_bot
}

/// 把消息文本里的 `<@id>` / `<@!id>` 提及标记去掉（@奶妹 本身不算提问内容）。
fn strip_mention(content: &str, bot_id: &str) -> String {
    content
        .replace(&format!("<@{bot_id}>"), "")
        .replace(&format!("<@!{bot_id}>"), "")
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
        Ok(Gateway::MessageCreate(m)) => {
            let app = app.clone();
            tokio::spawn(async move { app.handle_message(*m).await });
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
            chat: None,
            owner_ids: Vec::new(),
            chat_log: None,
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

    // ---------- AI 对话触发 ----------

    const BOT_ID: &str = "900000000000000099";

    fn message(content: &str, mentions: Value, referenced: Value) -> MessageInfo {
        serde_json::from_value(json!({
            "id": "900000000000000020",
            "channel_id": "900000000000000021",
            "guild_id": "900000000000000001",
            "author": {"id": "900000000000000005", "username": "alice", "bot": false},
            "content": content,
            "mentions": mentions,
            "referenced_message": referenced,
        }))
        .unwrap()
    }

    #[test]
    fn mention_and_reply_trigger_others_do_not() {
        let m = message(
            "<@900000000000000099> 你好",
            json!([{"id": BOT_ID}]),
            Value::Null,
        );
        assert!(is_chat_trigger(&m, BOT_ID));

        let m = message(
            "再讲讲",
            json!([]),
            json!({"id": "1", "author": {"id": BOT_ID, "username": "naimei", "bot": true}}),
        );
        assert!(is_chat_trigger(&m, BOT_ID));

        // 只有内容里提到奶妹、没有 @ 也不是回复奶妹：不触发
        let m = message("奶妹真可爱", json!([]), Value::Null);
        assert!(!is_chat_trigger(&m, BOT_ID));
        // 回复的是别人的消息：不触发
        let m = message(
            "看看这条",
            json!([]),
            json!({"id": "1", "author": {"id": "900000000000000005", "username": "bob"}}),
        );
        assert!(!is_chat_trigger(&m, BOT_ID));
    }

    #[test]
    fn mention_token_is_stripped() {
        assert_eq!(
            strip_mention("<@900000000000000099> 怎么领 key", BOT_ID),
            " 怎么领 key"
        );
        assert_eq!(
            strip_mention("<@!900000000000000099>旧式写法", BOT_ID),
            "旧式写法"
        );
        // 别人的 @ 不动
        assert_eq!(
            strip_mention("<@900000000000000005> 看看", BOT_ID),
            "<@900000000000000005> 看看"
        );
    }

    #[tokio::test]
    async fn chat_disabled_ignores_messages() {
        // 未配置 NAIMEI_LLM_*：消息进来直接忽略（不发任何请求）
        let app = app();
        let m = message(
            "<@900000000000000099> 你好",
            json!([{"id": BOT_ID}]),
            Value::Null,
        );
        app.handle_message(m).await; // 不 panic、不产生回复即为通过
    }
}
