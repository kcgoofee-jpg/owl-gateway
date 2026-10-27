//! Discord 自助领 Key。
//!
//! 发 Key 只有一条路径：机器人 /register → `POST /self-register/issue` → [`RegistrationService::issue_direct`]
//! → [`RegistrationService::provision`]。
//!
//! - 服务器 ID / 身份组 / 站点地址全部来自环境变量（`EnvConfig::registration`），这里不写死任何 ID。
//! - Discord REST 的基地址来自 `DISCORD_API_BASE`（默认官方 v10），测试里指向本机假服务。
//! - 机器人不私信成员：发 Key 的结果只在 /register 的临时回复里给本人看，后台「重新生成 Key」
//!   把新 Key 展示给站长手动转交（Discord 应用曾被标记，错误码 680009）。
//! - 后台循环不在这里起：见文件末尾的 [`registrar_jobs`]，由任务调度方安排。
//!   每日清空自助注册（旧的 `REGISTER_RESET_AT`）已删除（线上从来没配过）。

use crate::action_log::log_action;
use crate::defaults;
use crate::env::{EnvConfig, RegistrationEnv};
use crate::key::Key;
use crate::ops::{self, Registration};
use crate::state::AppState;
use gate_core::primitives::{strip, take_chars};
use gate_core::snowflake::snowflake_created_at;
use gate_core::{features, site_flags, validation, values};
use gate_db::repo::discord::BanInfo;
use gate_db::repo::footprint::DiscordConnection;
use gate_db::repo::keys::NewKey;
use gate_db::repo::registration::{NewRegistration, RegistrationProfile};
use gate_db::{Db, DbError};
use reqwest::Method;
use serde_json::{Value, json};
use std::time::Duration;

/// 身份组同步每轮最多摘几个（不制造突发请求）。
pub const SYNC_ROLES_PER_RUN: usize = 5;
/// 邀请 Key 持有人退群后，多久再核对一次。
const INVITE_RECHECK_SECS: f64 = 30.0 * 86400.0;

// ------------------------------------------------------------------ 错误

/// 领 Key 流程里的失败。`Denied` = 业务拒绝（中文原因，直接给成员看）；
/// `Internal` = 数据库 / 响应解析之类的内部错误，路由层按 500 处理。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegErr {
    Denied(String),
    Internal(String),
}

impl std::fmt::Display for RegErr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RegErr::Denied(s) | RegErr::Internal(s) => f.write_str(s),
        }
    }
}

impl std::error::Error for RegErr {}

impl From<DbError> for RegErr {
    fn from(e: DbError) -> Self {
        RegErr::Internal(e.to_string())
    }
}

fn denied<T>(msg: impl Into<String>) -> Result<T, RegErr> {
    Err(RegErr::Denied(msg.into()))
}

// ------------------------------------------------------------------ 进服时间 / 成员标记位

/// `GuildMember` 标记位：退群后又重新加入（DID_REJOIN）。
pub const MEMBER_FLAG_DID_REJOIN: i64 = 1;

/// 从 Discord 的成员对象里读到的进服时间与标记位（防小号用）。
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct MemberJoin {
    /// `joined_at`（ISO 8601）换成的 Unix 秒；没有 / 读不出为 None。
    pub joined_at: Option<f64>,
    /// `flags` 位域；没有按 0。
    pub flags: i64,
}

impl MemberJoin {
    pub fn from_member(member: &Value) -> Self {
        let joined_at = member
            .get("joined_at")
            .and_then(Value::as_str)
            .and_then(|s| s.parse::<jiff::Timestamp>().ok())
            .map(|t| t.as_millisecond() as f64 / 1000.0);
        let flags = member
            .get("flags")
            .and_then(|v| v.as_i64().or_else(|| v.as_f64().map(|f| f as i64)))
            .unwrap_or(0);
        MemberJoin { joined_at, flags }
    }

    /// 多个服务器合在一起：进服时间取最早的，标记位按位或。
    pub fn merge(self, other: MemberJoin) -> Self {
        let joined_at = match (self.joined_at, other.joined_at) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        MemberJoin {
            joined_at,
            flags: self.flags | other.flags,
        }
    }
}

// ------------------------------------------------------------------ 小工具

/// `_clip(value)`：空值为 None，否则最多 80 个字符。
pub fn clip(value: &str) -> Option<String> {
    if value.is_empty() {
        None
    } else {
        Some(take_chars(value, 80).to_string())
    }
}

/// Discord 用户资料（机器人从 interaction 带来的，或 REST 查到的）。空串 = 没有。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DiscordUser {
    pub id: String,
    pub username: String,
    pub global_name: String,
    pub avatar: String,
}

impl DiscordUser {
    /// 从 Discord 返回的用户 JSON 里取（缺的、`null` 的都当空）。
    pub fn from_json(v: &Value) -> Self {
        let s = |k: &str| match v.get(k) {
            Some(x) if values::enabled(x) => validation::text(x),
            _ => String::new(),
        };
        Self {
            id: s("id"),
            username: s("username"),
            global_name: s("global_name"),
            avatar: s("avatar"),
        }
    }
}

/// 后台里展示的成员名：显示名（@用户名），取不到时退回用户名或 ID。
pub fn member_label(user: &DiscordUser) -> String {
    let (username, display) = (clip(&user.username), clip(&user.global_name));
    if let (Some(d), Some(u)) = (&display, &username)
        && d != u
    {
        return take_chars(&format!("{d} (@{u})"), 60).to_string();
    }
    let label = display
        .or(username)
        .unwrap_or_else(|| format!("Discord:{}", user.id));
    take_chars(&label, 60).to_string()
}

/// 领取成功的欢迎语（/register 的临时回复，只有本人可见）所需的内容。
pub struct Welcome<'a> {
    pub key: &'a str,
    /// 站点地址；结尾的 `/` 会去掉。
    pub site: &'a str,
    /// 每人每天最多出几张 V4.5 及以下的图，0 = 不限。
    pub daily_images: i64,
    /// 当前有效 V4.5 额度不是显式的零限额。
    pub images_available: bool,
    /// 当前成员是否实际可以用 V5（权限、全局暂停和有效额度都通过）。
    pub v5: bool,
    /// 每人每天最多出几张 V5；仅在 `v5` 可用时用于显示，Unlimited 以 0 表示。
    pub daily_v5: i64,
    /// 每天几点重置（服务日起点）。
    pub reset_hour: i64,
    /// 连续几天没有成功出图会被回收，0 = 不回收。
    pub idle_days: i64,
    /// 已开通功能的名字，空串 = 不写这一行。
    pub opened: &'a str,
}

fn welcome_v5_available(
    image_scope: &str,
    policy_allowed: bool,
    paused: bool,
    member_limit: Option<f64>,
    global_limit: Option<f64>,
) -> bool {
    image_scope == "all"
        && policy_allowed
        && !paused
        && member_limit != Some(0.0)
        && global_limit != Some(0.0)
}

/// 领取成功的欢迎语：Key + 一步步配置教程 + 规则。控制在 Discord 2000 字以内。
///
/// 额度由全站共享额度池提供；每人的数字是个人上限。
pub fn welcome_message(w: &Welcome) -> String {
    let site = w.site.trim_end_matches('/');
    // 10-27 额度规范化：0 = 停用，上限 9999（= 放开）；「不限张数」的旧说法不再出现
    let mut cap = if !w.images_available {
        "V4.5 及以下当前不可用".to_string()
    } else if w.daily_images == 0 {
        "V4.5 及以下已停用".to_string()
    } else {
        format!("V4.5 及以下 {} 张", w.daily_images)
    };
    if w.v5 && w.daily_v5 != 0 {
        cap.push_str(&format!("；V5 {} 张", w.daily_v5));
    } else if w.v5 {
        cap.push_str("；V5 已停用");
    }
    let mut rules = vec![format!(
        "• 每日额度：每人每天最多 {cap}，全站共用额度池；每天 {} 点重置",
        w.reset_hour
    )];
    if !w.opened.is_empty() {
        rules.push(format!("• 已开通功能：{}", w.opened));
    }
    if w.idle_days != 0 {
        rules.push(format!(
            "• 连续 {} 天没有成功出图会被自动回收（不另行提醒），之后可以再 /register 领新的",
            w.idle_days
        ));
    }
    rules.push("• 每把 Key 同时生成 1 张、最多再排 2 张，多发的会被退回，等前面的出完再发".into());
    let v5_sizes = if w.v5 {
        "；V5 只有 832×1216、1216×832、1024×1024 三种尺寸，其他尺寸会自动换成最接近的一种"
    } else {
        ""
    };
    rules.push(format!(
        "• 只提供免费出图：V4.5 及以下总像素 ≤1024×1024（宽高取 64 的倍数，超出自动等比缩小）{v5_sizes}；\
         ≤28 步、每次 1 张；图生图 / 局部重绘不开放；Vibe 在「已开通功能」里时可用，消耗每日 Anlas 额度"
    ));
    rules.push("• Key 只给本服务器成员：退出服务器后 Key 自动失效".into());
    rules.push("• 一人一把，请勿分享：共用或转售会被回收".into());
    rules.push("• 本站不保存提示词或生成图片；请求结果与额度记录用于排障和防滥用".into());
    let text = format!(
        "🦉 **欢迎来到猫头鹰公益站！** 这是你的 API Key（只发这一次，请先保存）：\n`{key}`\n\n\
         **三步开始出图（以柏宝绘为例）**\n\
         1. 打开柏宝绘 → 渠道 → 配置 → 新建接入点\n\
         2. 接口地址填 `{site}`（**不要**加 /v1）\n\
         3. API Key 填上面 `nai-` 开头的整串，保存后随便生成一张图试试\n\
         其他支持自定义 NovelAI 地址的客户端同样这样填。\n\n\
         **不确定填对没有？** 在 {site} 用 Discord 登录，能看到自己的额度就说明 Key 正常。首次登录未提交使用体验问卷时，网站会自动打开问卷。\n\n\
         **规则**\n{rules}\n\n\
         **常用命令**：`/resetkey` Key 丢了或泄露时换新；额度在 {site} 登录后查看\n\
         遇到问题到 🛠️｜问题反馈 发截图（记得打码 Key）。",
        key = w.key,
        rules = rules.join("\n"),
    );
    take_chars(&text, 1990).to_string()
}

pub fn urlencode(pairs: &[(&str, &str)]) -> String {
    fn quote(s: &str, out: &mut String) {
        for b in s.bytes() {
            match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'_' | b'.' | b'-' | b'~' => {
                    out.push(b as char)
                }
                b' ' => out.push('+'),
                _ => out.push_str(&format!("%{b:02X}")),
            }
        }
    }
    let mut out = String::new();
    for (i, (k, v)) in pairs.iter().enumerate() {
        if i > 0 {
            out.push('&');
        }
        quote(k, &mut out);
        out.push('=');
        quote(v, &mut out);
    }
    out
}

// ------------------------------------------------------------------ 服务

/// `issue_direct` 的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Issued {
    /// 新发的 Key（`key` 只在临时回复里给本人看一次）。
    Provisioned(Provisioned),
    /// 填了邀请 Key：绑定到发命令的账号，Key 不变。
    Bound { key_id: i64, message: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Provisioned {
    pub key: String,
    pub message: String,
}

/// 网页登录换到的身份。
/// 网页登录申请的 OAuth scope（站长 10-23 加了 guilds / connections：读服务器列表和绑定的外部账号，防一人多号 / 共用 Key；
/// 登录页有一行说明）。Discord 的授权页是「全部同意或取消」，不能只勾一部分：成员点取消就回到首页（`?login=failed`），
/// 和以前点取消一样。已经登录的会话不用重新授权，下次登录时才开始记。
pub const WEB_LOGIN_SCOPES: &str = "identify guilds connections";
/// 验证身份组时的 scope：在 [`WEB_LOGIN_SCOPES`] 之外多要 guilds.members.read。
pub const WEB_LOGIN_SCOPES_VERIFY: &str = "identify guilds connections guilds.members.read";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebIdentity {
    pub id: String,
    pub username: String,
    pub global_name: String,
    pub avatar: String,
    /// 只有验证身份组的授权（verify=true）才会是 `Some`；普通登录（门槛没配）不查成员信息，也绝不因此拒绝登录。
    pub role: Option<RoleCheck>,
    /// 所在服务器 `(id, 名字)`；查不到（老授权没有 guilds scope、Discord 出错）为 None。尽力而为，不影响登录。
    pub guilds: Option<Vec<(String, String)>>,
    /// 绑定的外部账号；查不到为 None。
    pub connections: Option<Vec<DiscordConnection>>,
}

/// 验证身份组授权的结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoleCheck {
    /// 后台没配身份组门槛，无需核验。
    NotRequired,
    /// 已核验有这个身份组（已写入 `role_verifications`）。
    Verified,
    /// Discord 明确回答：不在服务器或没有这个身份组。
    Missing,
    /// Discord 故障 / 限流 / 网络错误：不能说成「你没有身份组」。
    Unavailable,
}

/// Discord REST 调用失败的种类。
#[derive(Debug)]
enum DiscordErr {
    Rejected,
    Net,
    Json,
}

struct Reply {
    status: u16,
    body: Vec<u8>,
}

impl Reply {
    fn json(&self) -> Option<Value> {
        serde_json::from_slice(&self.body).ok()
    }
}

pub struct RegistrationService {
    db: Db,
    /// 进程配置：Discord API 地址、闲置回收天数。
    env: EnvConfig,
    cfg: RegistrationEnv,
    api_base: String,
    http: reqwest::Client,
    /// 与 `issue_direct` / `ban` 共用：否则在发 Key 进行中（身份组核验等网络等待）封禁，仍会发出有效 Key。
    lock: tokio::sync::Mutex<()>,
}

impl RegistrationService {
    /// `configured_service`：全部配置来自环境变量；缺任意一项（或桥接密钥太短）则自助注册保持关闭。
    pub fn from_env(env: &EnvConfig, db: Db) -> Option<Self> {
        let cfg = env.registration.clone()?;
        let http = reqwest::Client::builder()
            // Discord 要求机器人带可识别的 User-Agent（空的 / 默认的会被 Cloudflare 1010 拦），与告警、运维命令共用
            .user_agent(gate_core::discord_ua::DISCORD_USER_AGENT)
            // httpx 默认不跟随重定向；也避免把机器人 Token 带到别处
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .ok()?;
        Some(Self {
            db,
            env: env.clone(),
            cfg,
            // 容忍写成 `.../v10/` 的环境变量：拼路径时不出现 `//`
            api_base: env.discord_api_base.trim_end_matches('/').to_string(),
            http,
            lock: tokio::sync::Mutex::new(()),
        })
    }

    // ---- 配置读取 ----

    pub fn bridge_secret(&self) -> &str {
        &self.cfg.bridge_secret
    }
    /// 发出 /register 的服务器（也是退群回收 / 身份组所在的服务器）。
    pub fn command_guild(&self) -> &str {
        &self.cfg.guild_id
    }
    pub fn idle_days(&self) -> i64 {
        self.env.key_inactivity_delete_days
    }

    // ---- 名额与查询 ----

    /// 占用名额的人数：持有未过期 Key 的已注册用户。Key 过期或被删则名额释放。
    /// 被站长停用（暂停）的成员仍占名额：否则停用即空出名额，重新启用后会超出上限。
    pub async fn count_active(&self) -> Result<i64, DbError> {
        self.db.registered_member_count(crate::time::now()).await
    }

    /// 生效的注册设置（和后台共用 `ops::registration_settings_for`，不另写一份）。
    pub async fn settings(&self) -> Result<Registration, DbError> {
        ops::registration_settings_for(&self.db, true).await
    }

    /// 在黑名单里就返回拉黑理由，不在返回 None。
    pub async fn ban_reason(&self, discord_id: &str) -> Result<Option<String>, DbError> {
        self.db.discord_ban_reason(discord_id).await
    }

    pub async fn key_row_for(&self, discord_id: &str) -> Result<Option<Key>, DbError> {
        Ok(self.db.key_row_for_discord(discord_id).await?.map(Key))
    }

    /// 领取记录里的资料：没有记录只有 `{"id"}`。
    pub async fn registration_profile(&self, discord_id: &str) -> Result<Value, DbError> {
        Ok(match self.db.discord_profile_row(discord_id).await? {
            None => json!({"id": discord_id}),
            Some((username, display_name, avatar)) => {
                json!({"id": discord_id, "username": username, "display_name": display_name, "avatar": avatar})
            }
        })
    }

    /// 名额检查：有空位先到先得，满了直接拒绝（候补排队已移除，2026-10-13）。
    async fn check_capacity(&self) -> Result<Registration, RegErr> {
        let cfg = self.settings().await?;
        if !cfg.open {
            return denied("领 Key 当前已关闭，开放时间看站长公告。");
        }
        if cfg.max_users == 0 || cfg.max_users - self.count_active().await? > 0 {
            return Ok(cfg);
        }
        // 只有开着闲置回收时才提它（10-17 起已关闭，写了就是误导）
        let recycle = if self.idle_days() != 0 {
            "闲置的 Key 会被自动回收，"
        } else {
            ""
        };
        denied(format!(
            "名额已满（上限 {} 人），请过段时间再试。{recycle}空出的名额先到先得。",
            cfg.max_users
        ))
    }

    // ---- Discord REST ----

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.api_base)
    }

    async fn send(
        &self,
        method: Method,
        path: &str,
        auth: &str,
        secs: u64,
        body: Option<&Value>,
    ) -> Result<Reply, reqwest::Error> {
        let mut req = self
            .http
            .request(method, self.url(path))
            .header("Authorization", auth)
            .timeout(Duration::from_secs(secs));
        if let Some(b) = body {
            req = req.json(b);
        }
        let resp = req.send().await?;
        let status = resp.status().as_u16();
        Ok(Reply {
            status,
            body: resp.bytes().await?.to_vec(),
        })
    }

    fn bot_auth(&self) -> String {
        format!("Bot {}", self.cfg.bot_token)
    }

    /// `_discord(method, path, bearer, json)`：状态码 ≥ 400 一律是 `Rejected`；成功时返回响应 JSON。
    async fn discord(
        &self,
        method: Method,
        path: &str,
        auth: &str,
        body: Option<&Value>,
    ) -> Result<Value, DiscordErr> {
        let r = self
            .send(method, path, auth, 12, body)
            .await
            .map_err(|_| DiscordErr::Net)?;
        if r.status >= 400 {
            return Err(DiscordErr::Rejected);
        }
        r.json().ok_or(DiscordErr::Json)
    }

    // ---- 身份组 ----

    /// 给 / 摘「已领 Key」身份组。失败只返回 false，不影响注册或撤销。
    async fn set_role(&self, discord_id: &str, grant: bool) -> bool {
        if self.cfg.member_role_id.is_empty() {
            return false;
        }
        let path = format!(
            "/guilds/{}/members/{discord_id}/roles/{}",
            self.cfg.guild_id, self.cfg.member_role_id
        );
        let reason = format!("NAI Gate key {}", if grant { "issued" } else { "ended" });
        let req = self
            .http
            .request(
                if grant { Method::PUT } else { Method::DELETE },
                self.url(&path),
            )
            .header("Authorization", self.bot_auth())
            .header("X-Audit-Log-Reason", reason)
            .timeout(Duration::from_secs(10));
        match req.send().await {
            // 404: 成员已离开服务器，也算完成
            Ok(r) => matches!(r.status().as_u16(), 200 | 204 | 404),
            Err(_) => false,
        }
    }

    /// 摘身份组：先登记待办，成功后再划掉；失败（限流、网络）会由后台同步重试，不会永久残留。
    pub async fn release_role(&self, discord_id: &str) -> Result<(), DbError> {
        if self.cfg.member_role_id.is_empty() {
            return Ok(());
        }
        self.db
            .queue_role_removal(discord_id, crate::time::now())
            .await?;
        if self.set_role(discord_id, false).await {
            self.db.finish_role_removal(discord_id).await?;
        }
        Ok(())
    }

    /// 后台同步：Key 已过期 / 被停用 / 被删除的成员摘掉身份组，并重试之前失败的摘除。返回本次摘掉的人数。
    ///
    /// 每轮最多处理 [`SYNC_ROLES_PER_RUN`] 个（维护循环每 5 分钟一轮），和 `backfill_profiles(5)` 一样不制造突发请求：
    /// 全站重置一次堆起几十个待办时，分几轮慢慢摘，剩下的留到下一轮（待办表不丢）。
    pub async fn sync_roles(&self) -> Result<usize, DbError> {
        if self.cfg.member_role_id.is_empty() {
            return Ok(0);
        }
        self.db
            .queue_expired_role_removals(crate::time::now())
            .await?;
        let mut done = 0;
        for discord_id in self
            .db
            .pending_role_removals()
            .await?
            .into_iter()
            .take(SYNC_ROLES_PER_RUN)
        {
            if self.set_role(&discord_id, false).await {
                // 每摘一个就提交：不能在持有未提交写事务时去等下一个 Discord 请求
                self.db.finish_role_removal(&discord_id).await?;
                done += 1;
            }
        }
        Ok(done)
    }

    // ---- 退群回收 / 资料补全 ----

    /// 退群回收：每个领了 Key 的人每天核对一次是否还在本服务器，不在就删 Key、放名额（站长 2026-10-10 决定直接回收）。
    ///
    /// 维护循环每 5 分钟调一次、每次只查 1 人 → 约 49 次读取 / 天，均匀分散，不发私信（申诉期间不制造突发请求）。
    /// 只有 Discord 明确回复「不是成员」（404 + code 10007）才回收；网络错误、限流、机器人不在服务器（10004）
    /// 或任何其他情况一律跳过、下轮再查，宁可漏收也不误删。返回本次回收人数。
    ///
    /// 邀请 Key 只由站长手动删（与每日清空 / 全站重置同一口径）：持有人退群时不删 Key，只写一条操作日志
    /// 「退群（邀请 Key 保留）」提醒站长，并把下次核对推到 30 天后（每月最多提醒一条）。
    pub async fn sweep_departed(&self, now: f64, batch: i64) -> Result<usize, DbError> {
        if !self.flag(&site_flags::MEMBER_SWEEP).await? {
            return Ok(0);
        }
        let mut removed = 0;
        for cand in self.db.departed_candidates(now, batch).await? {
            let discord_id = cand.discord_id;
            let path = format!("/guilds/{}/members/{discord_id}", self.cfg.guild_id);
            // 查不清（网络错误 / 404 的响应体读不出 code）都当作「没有响应」
            let answer: Option<(u16, Option<Value>)> = match self
                .send(Method::GET, &path, &self.bot_auth(), 10, None)
                .await
            {
                Ok(r) if r.status == 404 => r
                    .json()
                    .filter(Value::is_object)
                    .map(|v| (404, v.get("code").cloned())),
                // 这里查的是机器人所在的服务器（本站自己的服），不是领 Key 门槛要求的服：它的进服时间
                // 不能当「进服多久」用（大家都是最近才进的本站服，会被全当成新人），所以不记。
                // 进服时间只在网页登录核验身份组时，从门槛服务器的成员信息里记。
                Ok(r) if r.status == 200 => Some((200, None)),
                Ok(r) => Some((r.status, None)),
                Err(_) => None,
            };
            if let Some((404, Some(code))) = &answer
                && values::number(code) == Some(10007.0)
            {
                let label = self.registration_profile(&discord_id).await?;
                let who = ["display_name", "username"]
                    .iter()
                    .find_map(|k| label.get(k).filter(|v| values::enabled(v)))
                    .map(validation::text)
                    .unwrap_or_else(|| discord_id.clone());
                if let Some((key_id, key_name)) = &cand.invite_key {
                    log_action(
                        &self.db,
                        "系统",
                        "退群（邀请 Key 保留）",
                        &format!("Key #{key_id} {key_name}"),
                        &format!(
                            "Discord:{discord_id} {who} 已不在服务器；邀请 Key 只由站长手动删除，未回收"
                        ),
                        true,
                        now,
                    )
                    .await;
                    // 30 天后再核对一次：持有人一直不回来时，操作日志每月提醒一条而不是每天一条
                    self.db
                        .set_member_checked(&discord_id, now + INVITE_RECHECK_SECS)
                        .await?;
                    continue;
                }
                if self.revoke(&discord_id, false).await? {
                    log_action(
                        &self.db,
                        "系统",
                        "退群回收 Key",
                        &format!("Discord:{discord_id} {who}"),
                        "已不在服务器，Key 已删除、名额释放",
                        true,
                        now,
                    )
                    .await;
                    removed += 1;
                }
                continue;
            }
            // 在服务器 → 明天再查；没查清（限流 / 网络 / 其他错误）→ 1 小时后重查，不能一直卡在队首挡住别人
            let checked = if matches!(answer, Some((200, _))) {
                now
            } else {
                now - 86400.0 + 3600.0
            };
            self.db.set_member_checked(&discord_id, checked).await?;
        }
        Ok(removed)
    }

    /// 记下成员的进服时间 / 标记位（只更新已有的领取记录）。尽力而为：写库失败只记日志，不影响登录和退群核对。
    async fn record_member_join(&self, discord_id: &str, join: MemberJoin) {
        if let Err(e) = self
            .db
            .record_member_join(discord_id, join.joined_at, join.flags)
            .await
        {
            tracing::warn!("[member-join] record failed: {e}");
        }
    }

    /// 用机器人读一个 Discord 账号的公开资料（`GET /users/{id}`）。网络错误、超时、非 200、
    /// 响应不是 JSON 对象都返回 None；`secs` 是整个请求的超时。
    pub async fn fetch_user(&self, discord_id: &str, secs: u64) -> Option<DiscordUser> {
        let path = format!("/users/{discord_id}");
        match self
            .send(Method::GET, &path, &self.bot_auth(), secs, None)
            .await
        {
            Ok(r) if r.status == 200 => r
                .json()
                .filter(Value::is_object)
                .map(|v| DiscordUser::from_json(&v)),
            _ => None,
        }
    }

    /// 给还没有 Discord 用户名 / 头像的登记补全（用机器人读取公开资料）；每次最多处理几条，避免触发限流。
    pub async fn backfill_profiles(&self, limit: i64) -> Result<usize, DbError> {
        let mut done = 0;
        for (discord_id, key_id) in self.db.profile_backfill_candidates(limit).await? {
            let Some(u) = self.fetch_user(&discord_id, 10).await else {
                continue;
            };
            let profile = RegistrationProfile {
                username: Some(clip(&u.username).unwrap_or_default()),
                display_name: clip(&u.global_name),
                avatar: clip(&u.avatar),
            };
            self.db
                .apply_profile_backfill(&discord_id, key_id, &profile, &member_label(&u))
                .await?;
            done += 1;
        }
        Ok(done)
    }

    // ---- 封禁 / 撤销 / 清空 ----

    /// 永久禁止该 Discord 账号领取：写入封禁表，并撤销其现有 Key 与身份组。
    pub async fn ban(
        &self,
        discord_id: &str,
        reason: &str,
        banned_by: &str,
    ) -> Result<(), DbError> {
        // 记录用户的 Discord 资料，供后台黑名单展示
        let (u, d, a) = self
            .db
            .discord_profile_row(discord_id)
            .await?
            .unwrap_or_default();
        let had_key = {
            let _g = self.lock.lock().await;
            self.db
                .ban_discord(
                    discord_id,
                    &BanInfo {
                        reason: if reason.is_empty() {
                            "站长手动封禁".into()
                        } else {
                            reason.into()
                        },
                        username: u.unwrap_or_default(),
                        display_name: d.unwrap_or_default(),
                        avatar: a.unwrap_or_default(),
                        banned_by: if banned_by.is_empty() {
                            "站长".into()
                        } else {
                            banned_by.into()
                        },
                    },
                    crate::time::now(),
                )
                .await?;
            self.revoke(discord_id, false).await?
        };
        if had_key {
            self.release_role(discord_id).await?;
        }
        Ok(())
    }

    /// 成员的 Key 已过期：自动清掉旧 Key 和记录，让他可以在有名额时重新领取（每个周期自然轮换）。
    /// 被站长停用（enabled=0）的 Key 不会被自动释放：停用等于暂停该成员，不能靠过期绕过。
    async fn release_if_expired(&self, discord_id: &str, defer_role: bool) -> Result<(), DbError> {
        if let Some(key) = self.key_row_for(discord_id).await?
            && key.enabled()
            && key.expired(crate::time::now())
        {
            self.revoke(discord_id, !defer_role).await?;
            log_action(
                &self.db,
                "系统",
                "Key 到期释放名额",
                &format!("Key #{} {}", key.id(), key.name()),
                &format!("Discord:{discord_id} 重新领取时自动释放"),
                true,
                crate::time::now(),
            )
            .await;
        }
        Ok(())
    }

    /// 删除该用户的 Key 和领取记录，名额释放，用户可重新领取。`remove_role=false` 时只登记待办，
    /// 由后台同步稍后执行，避免在锁内等 Discord。
    pub async fn revoke(&self, discord_id: &str, remove_role: bool) -> Result<bool, DbError> {
        if self
            .db
            .revoke_registration(discord_id, crate::time::now())
            .await?
            .is_none()
        {
            return Ok(false);
        }
        if remove_role {
            self.release_role(discord_id).await?;
        } else if !self.cfg.member_role_id.is_empty() {
            self.db
                .queue_role_removal(discord_id, crate::time::now())
                .await?;
        }
        Ok(true)
    }

    // ---- 网页登录 ----

    /// 网页登录 OAuth 的 scope：普通登录见 [`WEB_LOGIN_SCOPES`]；「验证身份组」再多要 guilds.members.read。
    fn oauth_scope(verify: bool) -> &'static str {
        if verify {
            WEB_LOGIN_SCOPES_VERIFY
        } else {
            WEB_LOGIN_SCOPES
        }
    }

    /// 身份组门槛是否配齐（后台「领 Key」的服务器和身份组都非空）：配齐后网页登录一律走验证模式。
    pub async fn role_gate_configured(&self) -> Result<bool, DbError> {
        let cfg = self.settings().await?;
        Ok(!cfg.role_rules().is_empty())
    }

    /// 网页「用 Discord 登录」的授权链接（回调到 /login/callback）。只用于登录看状态，不发 Key。
    pub fn web_login_url(&self, state: &str, verify: bool) -> String {
        let redirect = format!("{}login/callback", self.cfg.site_url);
        format!(
            "https://discord.com/oauth2/authorize?{}",
            urlencode(&[
                ("client_id", &self.cfg.client_id),
                ("redirect_uri", &redirect),
                ("response_type", "code"),
                ("scope", Self::oauth_scope(verify)),
                ("state", state),
            ])
        )
    }

    /// 网页登录：用授权码换身份，返回 Discord 资料。`verify=true`（授权时申请了 guilds.members.read）
    /// 才会再查身份组并记入 `role_verifications`。不发 Key。
    pub async fn web_identify(&self, code: &str, verify: bool) -> Result<WebIdentity, RegErr> {
        let redirect = format!("{}login/callback", self.cfg.site_url);
        let resp = self
            .http
            .post(self.url("/oauth2/token"))
            .form(&[
                ("client_id", self.cfg.client_id.as_str()),
                ("client_secret", self.cfg.client_secret.as_str()),
                ("grant_type", "authorization_code"),
                ("code", code),
                ("redirect_uri", redirect.as_str()),
            ])
            .timeout(Duration::from_secs(12))
            .send()
            .await
            .map_err(|e| RegErr::Internal(format!("HTTPError: {e}")))?;
        let status = resp.status().as_u16();
        let body = resp
            .bytes()
            .await
            .map_err(|e| RegErr::Internal(format!("HTTPError: {e}")))?;
        let token_json: Option<Value> = serde_json::from_slice(&body).ok();
        if status != 200 {
            // 只记 Discord 返回的错误码（如 invalid_grant / invalid_client），不记授权码和密钥：
            // 10-14 网页登录「点了没反应」时日志里什么都没有，只能猜
            let why = token_json
                .as_ref()
                .and_then(|v| v.get("error"))
                .filter(|v| values::enabled(v))
                .map(validation::text)
                .unwrap_or_default();
            tracing::info!(
                "[web-login] token exchange {status} {}",
                take_chars(&why, 40)
            );
            return denied("Discord 授权失败，请重试。");
        }
        let token = token_json
            .as_ref()
            .and_then(|v| v.get("access_token"))
            .map(validation::text)
            .ok_or_else(|| RegErr::Internal("KeyError: 'access_token'".into()))?;
        let bearer = format!("Bearer {token}");
        let user = match self.discord(Method::GET, "/users/@me", &bearer, None).await {
            Ok(u) => u,
            Err(DiscordErr::Rejected) => {
                return denied("Discord 身份核验或私信失败，请检查授权和私信设置后重试。");
            }
            Err(DiscordErr::Net) => return Err(RegErr::Internal("HTTPError".into())),
            Err(DiscordErr::Json) => return Err(RegErr::Internal("invalid_value".into())),
        };
        let id = if user.get("id").is_some_and(|v| !v.is_null()) {
            validation::text(&user["id"])
        } else {
            "None".into()
        };
        let role = if verify {
            Some(self.check_role(&id, &bearer).await?)
        } else {
            None
        };
        let u = DiscordUser::from_json(&user);
        let (guilds, connections) = tokio::join!(
            self.list_user_guilds(&bearer),
            self.list_user_connections(&bearer)
        );
        Ok(WebIdentity {
            id,
            username: u.username,
            global_name: u.global_name,
            avatar: u.avatar,
            role,
            guilds,
            connections,
        })
    }

    /// 用成员自己的授权查他所在的服务器（scope `guilds`）。任何失败都返回 None。
    async fn list_user_guilds(&self, bearer: &str) -> Option<Vec<(String, String)>> {
        let r = self
            .send(Method::GET, "/users/@me/guilds", bearer, 8, None)
            .await
            .ok()
            .filter(|r| r.status == 200)?;
        Some(parse_guilds(&r.json()?))
    }

    /// 用成员自己的授权查他绑定的外部账号（scope `connections`）。任何失败都返回 None。
    async fn list_user_connections(&self, bearer: &str) -> Option<Vec<DiscordConnection>> {
        let r = self
            .send(Method::GET, "/users/@me/connections", bearer, 8, None)
            .await
            .ok()
            .filter(|r| r.status == 200)?;
        Some(parse_connections(&r.json()?))
    }

    /// 用成员自己的授权查他在门槛服务器里的身份组；有就记入 `role_verifications`。
    /// 门槛可以有多条（后台「领 Key」的主门槛 + 额外身份组，满足任意一条就算通过），可以分属不同的服务器：
    /// 同一个服务器只查一次，里面哪几条身份组命中就记哪几条。没配 = 不限定。
    ///
    /// 结果：有命中 = `Verified`；所有服务器都明确回答「不在 / 没有」= `Missing`；
    /// 没命中、但有服务器故障 / 限流 / 网络错误 = `Unavailable`（不能说成「你没有身份组」）。
    async fn check_role(&self, user_id: &str, bearer: &str) -> Result<RoleCheck, RegErr> {
        let cfg = self.settings().await?;
        let rules = cfg.role_rules();
        if rules.is_empty() {
            return Ok(RoleCheck::NotRequired);
        }
        let mut guilds: Vec<&str> = Vec::new();
        for r in &rules {
            if !guilds.contains(&r.guild.as_str()) {
                guilds.push(&r.guild);
            }
        }
        let (mut verified, mut unavailable) = (false, false);
        let mut join: Option<MemberJoin> = None;
        for guild in guilds {
            let path = format!("/users/@me/guilds/{guild}/member");
            let Ok(r) = self.send(Method::GET, &path, bearer, 12, None).await else {
                unavailable = true;
                continue;
            };
            match r.status {
                200 => {}
                // 10007 Unknown Member：账号不在这个服务器，这个服务器里的门槛都不满足
                404 => continue,
                _ => {
                    unavailable = true;
                    continue;
                }
            }
            let Some(member) = r.json() else {
                unavailable = true;
                continue;
            };
            let this = MemberJoin::from_member(&member);
            join = Some(join.map_or(this, |j| j.merge(this)));
            let roles: Vec<&str> = member
                .get("roles")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            for rule in rules.iter().filter(|r| r.guild == guild) {
                if roles.contains(&rule.role.as_str()) {
                    self.db
                        .upsert_role_verification(
                            user_id,
                            &rule.guild,
                            &rule.role,
                            crate::time::now(),
                        )
                        .await?;
                    verified = true;
                }
            }
        }
        if let Some(j) = join {
            self.record_member_join(user_id, j).await;
        }
        Ok(if verified {
            RoleCheck::Verified
        } else if unavailable {
            RoleCheck::Unavailable
        } else {
            RoleCheck::Missing
        })
    }

    // ---- 发 Key ----

    async fn flag(&self, spec: &site_flags::Spec) -> Result<bool, DbError> {
        crate::settings::site_bool(&self.db, spec).await
    }

    /// 每小时领取 Key 总数的硬上限：把短时间的授权/领取突增抹平（2026-10-10 Discord 应用因「增长异常」被标记，
    /// 就是网页登录上线后一小时内 ~43 人集中授权触发的）。0 = 关闭。默认 12/小时，远高于自然速率、远低于会触发风控的突增。
    async fn issue_rate_blocked(&self, now: f64) -> Result<bool, DbError> {
        let cap = crate::settings::site_f64(&self.db, &site_flags::ISSUE_HOURLY_CAP).await?;
        if cap <= 0.0 {
            return Ok(false);
        }
        Ok(self.db.registrations_since(now - 3600.0).await? as f64 >= cap)
    }

    /// 站长发的邀请 Key（带「邀请」标签）还没绑定 Discord 账号。
    pub async fn is_unbound_invite(&self, key_id: i64) -> Result<bool, DbError> {
        self.db.is_unbound_invite(key_id).await
    }

    /// /register 时填了邀请 Key：把这把 Key 绑定到发命令的 Discord 账号（10-11 站长：不用后台手动认人）。
    /// 邀请是站长预先放行的，所以不受「领 Key 开关 / 名额 / 每小时上限」限制；Key 本身不变。调用方须持有 `lock`。
    async fn bind_invite(
        &self,
        user_id: &str,
        token: &str,
        user: &DiscordUser,
    ) -> Result<Issued, RegErr> {
        let token = strip(token);
        let row = if token.starts_with("nai-") {
            self.db.key_id_enabled_by_token(token).await?
        } else {
            None
        };
        let Some((key_id, enabled)) = row else {
            return denied(
                "这不是一把待绑定的邀请 Key。请检查是否复制完整；普通领取请不填这一项。",
            );
        };
        if !self.db.is_unbound_invite(key_id).await? {
            return denied(
                "这不是一把待绑定的邀请 Key。请检查是否复制完整；普通领取请不填这一项。",
            );
        }
        if !enabled {
            return denied("这把邀请 Key 已被停用。");
        }
        let (username, display, avatar) = (
            clip(&user.username),
            clip(&user.global_name),
            clip(&user.avatar),
        );
        self.db
            .save_registration(&NewRegistration {
                discord_id: user_id,
                key_id,
                now: crate::time::now(),
                username: username.as_deref(),
                display_name: display.as_deref(),
                avatar: avatar.as_deref(),
                key_name: &member_label(user),
                member_role: !self.cfg.member_role_id.is_empty(),
            })
            .await?;
        if !self.cfg.member_role_id.is_empty() {
            self.set_role(user_id, true).await;
        }
        Ok(Issued::Bound {
            key_id,
            message: "✅ 邀请 Key 已绑定到你的 Discord 账号，Key 不变，客户端里不用改。\
                      登录网站后可在右上角账户菜单复制 Key、查看额度；首次登录会弹出使用体验问卷。Key 泄露可用 /resetkey 重置。"
                .into(),
        })
    }

    /// /register 直接发 Key：斜杠命令的 interaction 已被 Discord 签名验明发起人身份，
    /// 不必再走 OAuth 授权（应用被标记审查期间 OAuth 被封；这条通路同时去掉了批量私信）。
    /// 这是唯一的发 Key 入口（旧的 OAuth 领取 begin/finish 已删除，不能再恢复）。
    ///
    /// 校验：限服务器 / 账号年龄 / 封禁 / 去重 / 封禁号关联（[`crate::ban_link`]）/ 每小时上限 / 名额 / 可选身份组（机器人 Token 核验）；
    /// 通过后交给 [`Self::provision`] 落库。不通过返回 [`RegErr::Denied`]。
    pub async fn issue_direct(
        &self,
        user_id: &str,
        guild_id: &str,
        user: &DiscordUser,
        invite_key: &str,
    ) -> Result<Issued, RegErr> {
        if guild_id != self.cfg.guild_id || !validation::is_decimal(user_id) {
            return denied("请在指定服务器使用 /register。");
        }
        if defaults::REGISTER_MIN_ACCOUNT_DAYS != 0 {
            // Discord 雪花 ID 里带创建时间；读不出创建时间一律拒绝
            let age_days =
                snowflake_created_at(user_id).map(|t| (crate::time::now() - t) / 86_400.0);
            if age_days.is_none_or(|d| d < defaults::REGISTER_MIN_ACCOUNT_DAYS as f64) {
                return denied(format!(
                    "Discord 账号创建满 {} 天后才能领 Key，请稍后再来。",
                    defaults::REGISTER_MIN_ACCOUNT_DAYS
                ));
            }
        }
        let _g = self.lock.lock().await;
        if let Some(reason) = self.ban_reason(user_id).await? {
            return denied(gate_core::idle::banned_notice(&reason));
        }
        self.release_if_expired(user_id, true).await?;
        if self.db.registration_key_id(user_id).await?.is_some() {
            return denied(
                "这个 Discord 账号已经领取过 Key 了。额度在网页登录后查看；Key 找不到的话，用 /resetkey 换一把新的。",
            );
        }
        // 和封禁号同浏览器 / 同（非苹果）设备登录过：不发 Key、不绑邀请（站长在待审里忽略那一对 = 放行）
        if crate::ban_link::blocked(&self.db, user_id).await? {
            return denied(crate::ban_link::BLOCK_NOTICE);
        }
        if !invite_key.is_empty() {
            return self.bind_invite(user_id, invite_key, user).await;
        }
        if self.issue_rate_blocked(crate::time::now()).await? {
            return denied(
                "本小时领取人数较多，为保护服务稳定已暂时限流，请过几分钟再用 /register 领取。",
            );
        }
        let cfg = self.check_capacity().await?;
        // 身份组门槛（后台「领 Key」可配，可跨服，可有多条、满足任意一条即可）：
        // 机器人不在那些服务器里，改查成员网页授权后记下的核验结果
        let rules = cfg.role_rules();
        if !rules.is_empty() {
            let mut passed = false;
            for rule in &rules {
                if self
                    .db
                    .role_verified(user_id, &rule.guild, &rule.role)
                    .await?
                {
                    passed = true;
                    break;
                }
            }
            if !passed {
                return denied(format!(
                    "目前只开放给「{}」：请先到 {}login 用 Discord 登录（授权时会核验身份组），再用 /register 领取。",
                    cfg.role_note_text(),
                    self.cfg.site_url
                ));
            }
        }
        let user = DiscordUser {
            id: user_id.to_string(),
            ..user.clone()
        };
        Ok(Issued::Provisioned(
            self.provision(user_id, &cfg, &user).await?,
        ))
    }

    /// 唯一发 Key 的地方：建 api_keys 行、写 discord_registrations、按成员名改 Key 名、
    /// 身份组标志 / 清掉旧的待摘除、挂身份组、生成欢迎消息。
    /// 调用方必须已持有 `lock` 并完成全部校验。
    async fn provision(
        &self,
        user_id: &str,
        cfg: &Registration,
        user: &DiscordUser,
    ) -> Result<Provisioned, RegErr> {
        let key = gate_core::policy::gen_key("nai", &rand::random::<[u8; 24]>());
        let created = crate::time::now();
        let row = self
            .db
            .create_key(
                &NewKey {
                    name: format!("Discord:{user_id}"),
                    token: key.clone(),
                    enabled: true,
                    daily_images: cfg.daily_images,
                    daily_anlas: 0.0,
                    daily_v5: cfg.daily_v5,
                    monthly_anlas: 0.0,
                    daily_text_tokens: 0,
                    rpm: defaults::REGISTER_KEY_RPM,
                    allow_anlas: false,
                    allow_img2img: false,
                    exclude_global_v5: false,
                    image_model_scope: cfg.image_scope.clone(),
                    expires_at: (cfg.expires_days > 0)
                        .then(|| created + (cfg.expires_days * 86400) as f64),
                    features: cfg
                        .features
                        .as_ref()
                        .and_then(|f| features::dump(Some(f.iter()))),
                    discord_id: Some(user_id.to_string()),
                    quota_limits: Some(
                        serde_json::to_string(&gate_core::quota_limits::MemberLimits::legacy(
                            cfg.daily_images,
                            cfg.daily_v5,
                            0.0,
                            0.0,
                            1,
                        ))
                        .expect("valid default quota modes"),
                    ),
                },
                created,
            )
            .await?;
        let key_id = row.get("id").and_then(Value::as_i64).unwrap_or(0);
        let (username, display, avatar) = (
            clip(&user.username),
            clip(&user.global_name),
            clip(&user.avatar),
        );
        let stored = self
            .db
            .save_registration(&NewRegistration {
                discord_id: user_id,
                key_id,
                now: crate::time::now(),
                username: username.as_deref(),
                display_name: display.as_deref(),
                avatar: avatar.as_deref(),
                key_name: &member_label(user),
                member_role: !self.cfg.member_role_id.is_empty(),
            })
            .await;
        if let Err(e) = stored {
            // Key 已建好但登记失败：不留没人认领的孤儿 Key
            let _ = self.db.delete_key(key_id, crate::time::now()).await;
            return Err(e.into());
        }
        if !self.cfg.member_role_id.is_empty() {
            self.set_role(user_id, true).await;
        }
        let opened = match &cfg.features {
            Some(list) => list
                .iter()
                .filter_map(|f| {
                    features::FEATURES
                        .iter()
                        .find(|(n, _)| n == f)
                        .map(|(_, l)| *l)
                })
                .collect::<Vec<_>>()
                .join("、"),
            None => "全部已开放功能".to_string(),
        };
        let access = crate::v5_access::configuration_for(&self.db).await?;
        let access = access.evaluate(Some(user_id));
        let quota_modes = crate::ops::global_quota_modes(&self.db).await?;
        let member_v5 = quota_modes.register_daily_v5.resolve(None);
        let global_v5 = quota_modes.global_daily_v5.resolve(None);
        let member_images = quota_modes.register_daily_images.resolve(None);
        let v5 = welcome_v5_available(
            &cfg.image_scope,
            access.decision.allowed,
            access.paused,
            member_v5,
            global_v5,
        );
        let images_available = member_images != Some(0.0);
        let daily_images = member_images.unwrap_or(0.0).trunc() as i64;
        let daily_v5 = member_v5.unwrap_or(0.0).trunc() as i64;
        let message = welcome_message(&Welcome {
            key: &key,
            site: &self.cfg.site_url,
            daily_images,
            images_available,
            v5,
            daily_v5,
            reset_hour: gate_core::clock::DAY_RESET_HOUR,
            idle_days: self.env.key_inactivity_delete_days,
            opened: &opened,
        });
        Ok(Provisioned { key, message })
    }
}

// ------------------------------------------------------------------ 成员网页的进程内状态

// ------------------------------------------------------------------ 后台任务（由任务调度方安排）

/// 每 5 分钟的维护：同步身份组、补全资料、退群回收。每一步单独记错，不互相影响
pub async fn registrar_jobs(state: &AppState) {
    let Some(svc) = state.registration.as_ref() else {
        return;
    };
    let report = |name: &str, e: &DbError| {
        state.bugs.capture(crate::bugs::Report {
            source: &format!("maintenance:{name}"),
            error: Some(("DbError", &e.to_string())),
            ..Default::default()
        });
    };
    if let Err(e) = svc.sync_roles().await {
        report("sync_roles", &e);
    }
    if let Err(e) = svc.backfill_profiles(5).await {
        report("backfill_profiles", &e);
    }
    if let Err(e) = svc.sweep_departed(state.now(), 1).await {
        report("sweep_departed", &e);
    }
}

/// `GET /users/@me/guilds` 的结果 → `(id, 名字)`。
pub fn parse_guilds(v: &Value) -> Vec<(String, String)> {
    v.as_array()
        .map(|a| {
            a.iter()
                .filter_map(|g| {
                    let id = g.get("id")?.as_str()?.to_string();
                    let name = g.get("name").and_then(Value::as_str).unwrap_or("");
                    Some((id, name.to_string()))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// `GET /users/@me/connections` 的结果 → 外部账号列表（type / id / name / verified）。
pub fn parse_connections(v: &Value) -> Vec<DiscordConnection> {
    v.as_array()
        .map(|a| {
            a.iter()
                .filter_map(|c| {
                    Some(DiscordConnection {
                        kind: c.get("type")?.as_str()?.to_string(),
                        id: c.get("id")?.as_str()?.to_string(),
                        name: c
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string(),
                        verified: c.get("verified").and_then(Value::as_bool).unwrap_or(false),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn welcome_v5_requires_scope_policy_pause_and_nonzero_effective_quotas() {
        assert!(welcome_v5_available("all", true, false, None, None));
        assert!(!welcome_v5_available("all", false, false, None, None));
        assert!(!welcome_v5_available("all", true, true, None, None));
        assert!(!welcome_v5_available("legacy", true, false, None, None));
        assert!(!welcome_v5_available("all", true, false, Some(0.0), None));
        assert!(!welcome_v5_available("all", true, false, None, Some(0.0)));
    }

    #[test]
    fn label_and_clip() {
        let u = DiscordUser {
            id: "900000000000000001".into(),
            username: "neo".into(),
            global_name: "Neo".into(),
            avatar: String::new(),
        };
        assert_eq!(member_label(&u), "Neo (@neo)");
        let same = DiscordUser {
            global_name: "neo".into(),
            ..u.clone()
        };
        assert_eq!(member_label(&same), "neo");
        let none = DiscordUser {
            username: String::new(),
            global_name: String::new(),
            ..u
        };
        assert_eq!(member_label(&none), "Discord:900000000000000001");
        assert_eq!(clip(""), None);
        assert_eq!(clip(&"长".repeat(100)).unwrap().chars().count(), 80);
    }

    #[test]
    fn urlencode_like_python() {
        assert_eq!(
            urlencode(&[
                ("redirect_uri", "https://x.test/login/callback"),
                ("s", "a b~")
            ]),
            "redirect_uri=https%3A%2F%2Fx.test%2Flogin%2Fcallback&s=a+b~"
        );
    }

    fn welcome(daily_images: i64, daily_v5: i64) -> Welcome<'static> {
        Welcome {
            key: "nai-x",
            site: "https://site.test/",
            daily_images,
            images_available: true,
            v5: daily_v5 != 0,
            daily_v5,
            reset_hour: 7,
            idle_days: 3,
            opened: "文生图",
        }
    }

    #[test]
    fn welcome_text_states_the_current_rules() {
        let t = welcome_message(&welcome(30, 0));
        assert!(t.contains("`nai-x`"));
        // 接口地址不带结尾斜杠
        assert!(
            t.contains("接口地址填 `https://site.test`（**不要**加 /v1）"),
            "{t}"
        );
        assert!(!t.contains("https://site.test/"), "{t}");
        // 每日 7 点重置，没有「保底」「次日自动恢复」「有效期」
        assert!(
            t.contains("• 每日额度：每人每天最多 V4.5 及以下 30 张，全站共用额度池；每天 7 点重置"),
            "{t}"
        );
        for gone in ["保底", "次日自动恢复", "有效期", "全站空闲"] {
            assert!(!t.contains(gone), "{gone}：{t}");
        }
        assert!(
            t.contains("• 每把 Key 同时生成 1 张、最多再排 2 张，多发的会被退回"),
            "{t}"
        );
        // 不能用 V5 时不提 V5 的额度和尺寸
        assert!(!t.contains("832×1216") && !t.contains("V5"), "{t}");
        assert!(t.contains("• 一人一把，请勿分享：共用或转售会被回收"));
        assert!(t.chars().count() <= 1990);
    }

    #[test]
    fn welcome_text_covers_v5_sizes_and_unlimited() {
        let t = welcome_message(&welcome(0, 3));
        assert!(
            t.contains("每人每天最多 V4.5 及以下已停用；V5 3 张，全站共用额度池"),
            "{t}"
        );
        assert!(
            t.contains(
                "V5 只有 832×1216、1216×832、1024×1024 三种尺寸，其他尺寸会自动换成最接近的一种"
            ),
            "{t}"
        );
        // 能用 V5 但个人上限是 0（10-27 语义 = 停用）：明说停用，不写张数
        let t = welcome_message(&Welcome {
            v5: true,
            ..welcome(30, 0)
        });
        assert!(
            t.contains("每人每天最多 V4.5 及以下 30 张；V5 已停用，全站共用额度池"),
            "{t}"
        );
        assert!(t.contains("V5 只有 832×1216"), "{t}");
        // 不回收、没有已开通功能
        let t = welcome_message(&Welcome {
            idle_days: 0,
            opened: "",
            ..welcome(30, 0)
        });
        assert!(
            !t.contains("自动回收") && !t.contains("• 已开通功能："),
            "{t}"
        );
    }

    #[test]
    fn welcome_does_not_show_explicit_zero_image_quota_as_unlimited() {
        let t = welcome_message(&Welcome {
            images_available: false,
            ..welcome(0, 0)
        });
        assert!(t.contains("V4.5 及以下当前不可用"), "{t}");
        assert!(!t.contains("V4.5 及以下不限张数"), "{t}");
    }

    #[test]
    fn welcome_text_is_cut_at_discords_limit() {
        let t = welcome_message(&Welcome {
            opened: &"长".repeat(3000),
            ..welcome(30, 3)
        });
        assert_eq!(t.chars().count(), 1990);
    }
}
