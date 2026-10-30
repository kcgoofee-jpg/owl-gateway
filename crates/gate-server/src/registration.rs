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
pub use gate_core::abuse::MEMBER_FLAG_DID_REJOIN;

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
    /// 每人每天最多出几张 V5；0 表示停用，仅在 `v5` 可用时用于显示。
    pub daily_v5: i64,
    /// 每天几点重置（服务日起点）。
    pub reset_hour: i64,
    /// 连续几天没有成功出图会被回收，0 = 不回收。
    pub idle_days: i64,
    /// 已开通功能的名字，空串 = 不写这一行。
    pub opened: &'a str,
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
    rules.push(
        "• 禁止商业化使用、公开 Key、绕过限制或异常高频调用、提交违规内容及影响其他用户正常使用"
            .into(),
    );
    rules.push("• 本站不保存提示词或生成图片；请求结果与额度记录用于排障和防滥用".into());
    let text = format!(
        "🦉 **欢迎来到猫头鹰公益站！** 这是你的 API Key（只发这一次，请先保存）：\n`{key}`\n\n\
         **三步开始出图（以柏宝绘为例）**\n\
         1. 打开柏宝绘 → 渠道 → 配置 → 新建接入点\n\
         2. 接口地址填 `{site}`（**不要**加 /v1）\n\
         3. API Key 填上面 `nai-` 开头的整串，保存后随便生成一张图试试\n\
         其他支持自定义 NovelAI 地址的客户端同样这样填。\n\n\
         **不确定填对没有？** 在 {site} 用 Discord 登录，能看到自己的额度就说明 Key 正常。\n\n\
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
pub(crate) enum DiscordErr {
    Rejected,
    Net,
    Json,
}

pub(crate) struct Reply {
    status: u16,
    body: Vec<u8>,
}

impl Reply {
    fn json(&self) -> Option<Value> {
        serde_json::from_slice(&self.body).ok()
    }
}

mod issue;
mod profiles;
mod roles;
mod transport;
mod web_login;

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

    // ---- Discord REST ----

    // ---- 身份组 ----

    // ---- 退群回收 / 资料补全 ----

    // ---- 封禁 / 撤销 / 清空 ----

    // ---- 网页登录 ----

    // ---- 发 Key ----
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
