//! `nai-ops set <name> <value>` / `nai-ops get <name>`：后台设置接口的薄客户端。
//!
//! 不直接碰数据库：全部走网关已有的 `/admin/api/*` 接口，所以校验、操作日志、内存里的即时生效都和后台页面完全一致。
//! 只认白名单里的名字（`set --list` 可看）。改之前先读当前值，打印「改前 → 改后」，没有 `--yes` 不写
//! （`--dry-run` 只演示）。密码读 `ADMIN_PASSWORD`（环境变量，其次 `.env`），登录后的 Cookie 只放在内存里，
//! 任何输出和错误信息都不带密码 / Cookie。
//! 注意：后台改过登录密码的话，以库里保存的为准，`.env` 里的 `ADMIN_PASSWORD` 只是初始值。
//! 每个请求都带 `x-owl-actor: claude`：操作日志里的操作者记「手动（Claude）」（站长 10-20：Claude 的改动算手动）。
//!
//! `nai-ops manual`：后台页面上没有的临时设置（V5 临时加量、账号日上限临时调整、站付 V5、当天 V5 方案）走
//! `POST /ops/manual`，网关在同一个事务里改设置并写「手动（Claude）」记录。以后不再直接改数据库。

use crate::util;
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::time::Duration;

pub const DEFAULT_API_BASE: &str = "http://127.0.0.1:3003/admin/api";
/// 网关据此把操作者记成「手动（Claude）」（`routes/admin/mod.rs` 的 `ACTOR_HEADER`）。
pub const ACTOR_HEADER: (&str, &str) = ("x-owl-actor", "claude");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ep {
    /// `/settings`：Anlas / V5 预算
    Settings,
    MemberDefaults,
    /// `/guard`：账号保护与排队
    Guard,
    /// `/runtime-limits`：排队与冷却
    RuntimeLimits,
    /// `/ops/registration`：领 Key
    Registration,
    /// 没有后台接口：明确不支持（不去写数据库）
    Unsupported(&'static str),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Int,
    Float,
    Bool,
    Text,
    Choice(&'static [&'static str]),
    /// 完整的 OR 领取条件 JSON 数组。
    Roles,
}

#[derive(Debug, Clone, Copy)]
pub struct Spec {
    pub name: &'static str,
    pub ep: Ep,
    pub kind: Kind,
    /// 接口里的字段名
    pub field: &'static str,
    pub desc: &'static str,
}

const fn s(name: &'static str, ep: Ep, kind: Kind, desc: &'static str) -> Spec {
    Spec {
        name,
        ep,
        kind,
        field: name,
        desc,
    }
}

/// 白名单。范围以服务端为准（这里只做类型检查，服务端不接受会原样转述原因）。
pub const SPECS: &[Spec] = &[
    Spec {
        name: "global_daily_anlas",
        ep: Ep::Settings,
        kind: Kind::Int,
        field: "budgets.daily_anlas",
        desc: "全站每日 Anlas（0 停用，上限9999）",
    },
    Spec {
        name: "global_monthly_anlas",
        ep: Ep::Settings,
        kind: Kind::Float,
        field: "budgets.monthly_anlas",
        desc: "全站每月 Anlas（0 停用，上限99999）",
    },
    Spec {
        name: "global_daily_v5",
        ep: Ep::Settings,
        kind: Kind::Int,
        field: "budgets.daily_v5",
        desc: "全站每日 V5（0 停用，上限9999）",
    },
    Spec {
        name: "member_daily_images",
        ep: Ep::MemberDefaults,
        kind: Kind::Int,
        field: "daily_images",
        desc: "默认每日 V4.5 免费图（0 停用，上限9999）",
    },
    Spec {
        name: "member_daily_v5",
        ep: Ep::MemberDefaults,
        kind: Kind::Int,
        field: "daily_v5",
        desc: "默认每日 V5（0 停用，上限9999）",
    },
    Spec {
        name: "member_daily_anlas",
        ep: Ep::MemberDefaults,
        kind: Kind::Int,
        field: "daily_anlas",
        desc: "默认每日 Anlas（0 停用，上限9999）",
    },
    Spec {
        name: "member_monthly_anlas",
        ep: Ep::MemberDefaults,
        kind: Kind::Float,
        field: "monthly_anlas",
        desc: "默认每月 Anlas（0 停用，上限99999）",
    },
    Spec {
        name: "member_rpm",
        ep: Ep::MemberDefaults,
        kind: Kind::Int,
        field: "rpm",
        desc: "默认每分钟请求数（1~600）",
    },
    Spec {
        name: "v5_paused",
        ep: Ep::Settings,
        kind: Kind::Bool,
        field: "v5_paused",
        desc: "暂停 V5（on/off）",
    },
    Spec {
        name: "register_open",
        ep: Ep::Registration,
        kind: Kind::Bool,
        field: "open",
        desc: "开放领取（on/off）",
    },
    Spec {
        name: "register_max_users",
        ep: Ep::Registration,
        kind: Kind::Int,
        field: "max_users",
        desc: "领取人数上限（0 不限，上限100000）",
    },
    Spec {
        name: "register_expires_days",
        ep: Ep::Registration,
        kind: Kind::Int,
        field: "expires_days",
        desc: "Key 有效天数（0 永久，上限3650）",
    },
    Spec {
        name: "claim_roles",
        ep: Ep::Registration,
        kind: Kind::Roles,
        field: "roles",
        desc: "符合任一身份组的 JSON 数组；[] 不限制身份组",
    },
    Spec {
        name: "member_image",
        ep: Ep::MemberDefaults,
        kind: Kind::Bool,
        field: "image",
        desc: "默认开放图片生成（on/off）",
    },
    Spec {
        name: "member_v5",
        ep: Ep::MemberDefaults,
        kind: Kind::Bool,
        field: "v5",
        desc: "默认开放V5（on/off）",
    },
    Spec {
        name: "member_vibe",
        ep: Ep::MemberDefaults,
        kind: Kind::Bool,
        field: "vibe",
        desc: "默认开放Vibe 与参考图（on/off）",
    },
    Spec {
        name: "member_tags",
        ep: Ep::MemberDefaults,
        kind: Kind::Bool,
        field: "tags",
        desc: "默认开放标签补全（on/off）",
    },
    Spec {
        name: "member_upscale",
        ep: Ep::MemberDefaults,
        kind: Kind::Bool,
        field: "upscale",
        desc: "默认开放放大（on/off）",
    },
    Spec {
        name: "member_augment",
        ep: Ep::MemberDefaults,
        kind: Kind::Bool,
        field: "augment",
        desc: "默认开放导演工具（on/off）",
    },
    Spec {
        name: "member_img2img",
        ep: Ep::MemberDefaults,
        kind: Kind::Bool,
        field: "img2img",
        desc: "默认开放图生图与局部重绘（on/off）",
    },
    Spec {
        name: "member_paid_parameters",
        ep: Ep::MemberDefaults,
        kind: Kind::Bool,
        field: "paid_parameters",
        desc: "默认开放高规格生图（on/off）",
    },
    s(
        "v5_medium_mode",
        Ep::Settings,
        Kind::Choice(&["off", "on"]),
        "V5 Full 转 Medium：off/on",
    ),
    Spec {
        name: "issue_hourly_cap",
        ep: Ep::Unsupported(
            "后台目前没有 issue_hourly_cap（每小时最多发放的新 Key）的修改接口。nai-ops 不会绕过后台直接写数据库（那样没有校验、没有操作日志、网关内存也不会刷新）；需要改请先在网关加后台接口",
        ),
        kind: Kind::Int,
        field: "issue_hourly_cap",
        desc: "每小时最多发放的新 Key（不支持：后台没有接口）",
    },
    s(
        "account_daily_cap",
        Ep::Guard,
        Kind::Int,
        "每个上游账号每天最多出图张数",
    ),
    s(
        "queue_per_account",
        Ep::Guard,
        Kind::Int,
        "每个上游账号全站最多同时排几张图",
    ),
    s(
        "queue_per_member",
        Ep::Guard,
        Kind::Int,
        "每个成员最多同时排几张图（正在生成的不算，1~5）",
    ),
    s(
        "post_jitter_min",
        Ep::Guard,
        Kind::Int,
        "每张出完后这个账号至少歇几秒（1~30；实际在最少~最多之间随机）",
    ),
    s(
        "post_jitter_max",
        Ep::Guard,
        Kind::Int,
        "每张出完后这个账号最多歇几秒（1~30）",
    ),
    s(
        "queue_timeout",
        Ep::RuntimeLimits,
        Kind::Int,
        "排队超时（秒，15~300）",
    ),
];

pub fn find(name: &str) -> Option<&'static Spec> {
    SPECS.iter().find(|s| s.name == name)
}

fn kind_label(k: Kind) -> String {
    match k {
        Kind::Int => "整数".into(),
        Kind::Float => "数字".into(),
        Kind::Bool => "on/off".into(),
        Kind::Text => "文本".into(),
        Kind::Choice(c) => c.join("/"),
        Kind::Roles => "JSON 数组".into(),
    }
}

fn ep_label(ep: Ep) -> &'static str {
    match ep {
        Ep::Settings => "PUT /settings",
        Ep::MemberDefaults => "PUT /member-rules",
        Ep::Guard => "PUT /guard",
        Ep::RuntimeLimits => "PUT /runtime-limits",
        Ep::Registration => "PUT /ops/registration",
        Ep::Unsupported(_) => "（不支持）",
    }
}

/// `set --list` 的输出。
pub fn list_text() -> String {
    let mut out = String::from("可用设置名（全部走后台接口，校验与操作日志同后台页面）：\n");
    for sp in SPECS {
        out.push_str(&format!(
            "  {:<22} {:<22} {:<18} {}\n",
            sp.name,
            ep_label(sp.ep),
            kind_label(sp.kind),
            sp.desc
        ));
    }
    out.push_str("\n用法：nai-ops set <名字> <值> [--dry-run | --yes]    nai-ops get <名字>\n");
    out
}

/// 把命令行文本变成接口要的 JSON；类型不对返回原因。
pub fn parse_value(kind: Kind, raw: &str) -> Result<Value, String> {
    let t = raw.trim();
    match kind {
        Kind::Int => t
            .parse::<i64>()
            .map(Value::from)
            .map_err(|_| format!("{raw:?} 不是整数")),
        Kind::Float => match t.parse::<f64>() {
            Ok(f) if f.is_finite() => Ok(json!(f)),
            _ => Err(format!("{raw:?} 不是数字")),
        },
        Kind::Bool => match t.to_ascii_lowercase().as_str() {
            "on" | "true" | "1" | "yes" => Ok(Value::Bool(true)),
            "off" | "false" | "0" | "no" => Ok(Value::Bool(false)),
            _ => Err(format!("{raw:?} 不是 on / off")),
        },
        Kind::Text => Ok(Value::String(raw.to_string())),
        Kind::Choice(c) => {
            if c.contains(&t) {
                Ok(Value::String(t.to_string()))
            } else {
                Err(format!("{raw:?} 不在 {} 之中", c.join(" / ")))
            }
        }
        Kind::Roles => {
            let roles: Vec<gate_core::member_policy::EligibilityRule> =
                serde_json::from_str(t).map_err(|_| "需要领取条件 JSON 数组".to_owned())?;
            gate_core::member_policy::ClaimRules {
                roles: roles.clone(),
                ..gate_core::member_policy::ClaimRules::initial()
            }
            .validate()
            .map_err(str::to_owned)?;
            serde_json::to_value(roles).map_err(|error| error.to_string())
        }
    }
}

/// 展示用文本。
pub fn show(v: &Value) -> String {
    match v {
        Value::Null => "(未设置)".into(),
        Value::String(s) if s.is_empty() => "(空)".into(),
        Value::String(s) => s.clone(),
        Value::Number(n) => n
            .as_f64()
            .filter(|f| f.fract() == 0.0 && f.abs() < 1e15)
            .map_or_else(|| n.to_string(), |f| format!("{}", f as i64)),
        other => other.to_string(),
    }
}

fn same(a: &Value, b: &Value) -> bool {
    match (a.as_f64(), b.as_f64()) {
        (Some(x), Some(y)) => (x - y).abs() < 1e-9,
        _ => a == b,
    }
}

/// PUT 的请求体。
fn put_body(sp: &Spec, value: &Value, current: &Value) -> Option<Value> {
    let mut body = match sp.ep {
        Ep::Settings => current.clone(),
        Ep::MemberDefaults => current.get("defaults")?.clone(),
        Ep::Registration => {
            let saved = current.get("registration")?.as_object()?;
            Value::Object(
                ["open", "max_users", "expires_days", "roles"]
                    .iter()
                    .map(|key| Some(((*key).to_owned(), saved.get(*key)?.clone())))
                    .collect::<Option<_>>()?,
            )
        }
        Ep::Guard | Ep::RuntimeLimits => return Some(json!({sp.field:value})),
        Ep::Unsupported(_) => return None,
    };
    *body.pointer_mut(&format!("/{}", sp.field.replace('.', "/")))? = value.clone();
    Some(body)
}

fn put_path(ep: Ep) -> &'static str {
    match ep {
        Ep::Settings => "/settings",
        Ep::MemberDefaults => "/member-rules",
        Ep::Guard => "/guard",
        Ep::RuntimeLimits => "/runtime-limits",
        Ep::Registration => "/ops/registration",
        Ep::Unsupported(_) => "",
    }
}

/// 从 GET 响应里取当前值。
fn read_current(sp: &Spec, ep_json: &Value) -> Option<Value> {
    let base = match sp.ep {
        Ep::Settings | Ep::RuntimeLimits => ep_json,
        Ep::MemberDefaults => ep_json.get("defaults")?,
        Ep::Guard => ep_json.get("values")?,
        Ep::Registration => ep_json.get("registration")?,
        Ep::Unsupported(_) => return None,
    };
    base.pointer(&format!("/{}", sp.field.replace('.', "/")))
        .cloned()
}

fn get_path(ep: Ep) -> &'static str {
    match ep {
        Ep::Settings => "/settings",
        Ep::MemberDefaults => "/member-rules",
        Ep::Guard => "/guard",
        Ep::RuntimeLimits => "/runtime-limits",
        Ep::Registration => "/ops",
        Ep::Unsupported(_) => "",
    }
}

// ---------------------------------------------------------------- 后台接口客户端

pub struct Admin {
    base: String,
    http: reqwest::Client,
    cookie: String,
}

fn error_text(status: u16, body: &str) -> String {
    let detail = serde_json::from_str::<Value>(body).ok().and_then(|j| {
        j.get("detail")
            .or_else(|| j.get("message"))
            .map(|d| d.as_str().map_or_else(|| d.to_string(), String::from))
    });
    let detail = detail.unwrap_or_else(|| body.chars().take(200).collect());
    format!("HTTP {status}：{detail}")
}

impl Admin {
    pub async fn login(base: &str, password: &str) -> Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .build()?;
        let base = base.trim_end_matches('/').to_string();
        let resp = http
            .post(format!("{base}/login"))
            .header(ACTOR_HEADER.0, ACTOR_HEADER.1)
            .json(&json!({"password": password}))
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("连不上后台接口 {base}（{}）", e.without_url()))?;
        let status = resp.status().as_u16();
        let cookie = resp
            .headers()
            .get_all("set-cookie")
            .iter()
            .filter_map(|v| v.to_str().ok())
            .filter_map(|v| v.split(';').next())
            .filter(|kv| kv.contains('='))
            .collect::<Vec<_>>()
            .join("; ");
        let body = resp.text().await.unwrap_or_default();
        if status == 401 {
            bail!(
                "后台登录失败：密码不对。后台改过密码的话以库里保存的为准，.env 里的 ADMIN_PASSWORD 只是初始值"
            );
        }
        if !(200..300).contains(&status) {
            bail!("后台登录失败：{}", error_text(status, &body));
        }
        if cookie.is_empty() {
            bail!("后台登录成功但没有收到会话 Cookie");
        }
        Ok(Self { base, http, cookie })
    }

    async fn call(&self, req: reqwest::RequestBuilder) -> Result<Value> {
        let resp = req
            .header("Cookie", &self.cookie)
            .header(ACTOR_HEADER.0, ACTOR_HEADER.1)
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("请求后台失败（{}）", e.without_url()))?;
        let status = resp.status().as_u16();
        let body = resp.text().await.unwrap_or_default();
        if !(200..300).contains(&status) {
            bail!("{}", error_text(status, &body));
        }
        serde_json::from_str(&body).context("后台返回的不是 JSON")
    }

    pub async fn get(&self, path: &str) -> Result<Value> {
        self.call(self.http.get(format!("{}{path}", self.base)))
            .await
    }

    pub async fn put(&self, path: &str, body: &Value) -> Result<Value> {
        self.call(self.http.put(format!("{}{path}", self.base)).json(body))
            .await
    }

    pub async fn post(&self, path: &str, body: &Value) -> Result<Value> {
        self.call(self.http.post(format!("{}{path}", self.base)).json(body))
            .await
    }
}

/// 管理密码：环境变量优先，其次 `.env`（`NAI_GATE_ENV`，默认 `/opt/service-tools/.env`）。
pub fn admin_password() -> String {
    util::env_or_file(&util::env_file(), "ADMIN_PASSWORD")
}

pub fn api_base() -> String {
    util::env_nonempty("ADMIN_API_BASE").unwrap_or_else(|| DEFAULT_API_BASE.to_string())
}

async fn connect() -> Result<Admin> {
    let pw = admin_password();
    if pw.is_empty() {
        bail!("没有找到 ADMIN_PASSWORD（环境变量或 .env）");
    }
    Admin::login(&api_base(), &pw).await
}

// ---------------------------------------------------------------- 命令

/// 返回 (退出码, 输出文本)。0 成功；2 = 用法错误 / 被拒绝 / 不支持。
pub async fn run_get(name: &str) -> Result<(i32, String)> {
    let Some(sp) = find(name) else {
        return Ok((
            2,
            format!("未知设置名 {name:?}；nai-ops set --list 可看全部\n"),
        ));
    };
    if let Ep::Unsupported(why) = sp.ep {
        return Ok((2, format!("{}：{why}\n", sp.name)));
    }
    let admin = connect().await?;
    run_get_with(&admin, sp).await
}

pub async fn run_get_with(admin: &Admin, sp: &Spec) -> Result<(i32, String)> {
    let j = admin.get(get_path(sp.ep)).await?;
    let cur = read_current(sp, &j).with_context(|| format!("后台返回里没有 {}", sp.name))?;
    Ok((0, format!("{} = {}\n", sp.name, show(&cur))))
}

pub async fn run_set(name: &str, raw: &str, yes: bool, dry_run: bool) -> Result<(i32, String)> {
    let Some(sp) = find(name) else {
        return Ok((
            2,
            format!("未知设置名 {name:?}；nai-ops set --list 可看全部\n"),
        ));
    };
    if let Ep::Unsupported(why) = sp.ep {
        return Ok((2, format!("{}：不支持。{why}\n", sp.name)));
    }
    let new = match parse_value(sp.kind, raw) {
        Ok(v) => v,
        Err(why) => return Ok((2, format!("{}：{why}\n", sp.name))),
    };
    let admin = connect().await?;
    run_set_with(&admin, sp, new, yes, dry_run).await
}

pub async fn run_set_with(
    admin: &Admin,
    sp: &Spec,
    new: Value,
    yes: bool,
    dry_run: bool,
) -> Result<(i32, String)> {
    let before_json = admin.get(get_path(sp.ep)).await?;
    let before =
        read_current(sp, &before_json).with_context(|| format!("后台返回里没有 {}", sp.name))?;
    let plan = format!("{}: {} → {}\n", sp.name, show(&before), show(&new));
    if same(&before, &new) {
        return Ok((0, format!("{plan}（值没有变化，不写）\n")));
    }
    if dry_run {
        return Ok((0, format!("{plan}（--dry-run：没有写入）\n")));
    }
    if !yes {
        return Ok((
            2,
            format!("{plan}没有写入：要真的修改请加 --yes（先预览用 --dry-run）\n"),
        ));
    }
    let body = put_body(sp, &new, &before_json).context("后台设置结构无效，未写入")?;
    admin.put(put_path(sp.ep), &body).await?;
    let after_json = admin.get(get_path(sp.ep)).await?;
    let after =
        read_current(sp, &after_json).with_context(|| format!("后台返回里没有 {}", sp.name))?;
    let mut out = format!("{}: {} → {}\n", sp.name, show(&before), show(&after));
    if same(&after, &new) {
        out.push_str("已写入（后台操作日志里可见，网关内存已即时生效）\n");
        Ok((0, out))
    } else {
        out.push_str("警告：写入后读回的值与期望不一致，请到后台核对\n");
        Ok((1, out))
    }
}

// ---------------------------------------------------------------- 手动临时设置

/// `nai-ops manual` 认的名字（与网关 `routes/admin/ops/manual.rs` 的 `KEYS` 一致）和说明。
pub const MANUAL_KEYS: [(&str, &str); 2] = [
    (
        "guard_token_daily_caps",
        r#"账号日上限临时调整：{"<token_id>": {"cap": 2200, "until": <Unix 秒>}}；即时生效"#,
    ),
    (
        "v5_paid_tokens",
        r#"站付 V5：{"<token_id>": {"max_anlas": 9700, "until": <Unix 秒>}}"#,
    ),
];

pub fn manual_list_text() -> String {
    let mut out = String::from(
        "nai-ops manual 可改的临时设置（改动和「手动（Claude）」记录在同一个事务里）：\n",
    );
    for (k, d) in MANUAL_KEYS {
        out.push_str(&format!("  {k:<24} {d}\n"));
    }
    out.push_str(concat!(
        "\n用法：nai-ops manual <名字> '<JSON>' --why '原因' [--dry-run | --yes]\n",
        "      nai-ops manual <名字> null --why '原因' --yes      删除这项（不再加量）\n",
        "      nai-ops manual --action '操作名' [--target '对象'] --why '经过' --yes   只补记一笔\n",
    ));
    out
}

/// `POST /ops/manual` 的请求体；参数不对返回原因。
pub fn manual_body(
    key: Option<&str>,
    value: Option<&str>,
    why: &str,
    action: Option<&str>,
    target: Option<&str>,
) -> Result<Value, String> {
    if why.trim().is_empty() {
        return Err("--why 不能为空：写清楚为什么改、谁拍板、什么时候收回".into());
    }
    let Some(key) = key else {
        let action = action.map(str::trim).filter(|a| !a.is_empty());
        let Some(action) = action else {
            return Err("只补记一笔时需要 --action".into());
        };
        return Ok(json!({"action": action, "target": target.unwrap_or(""), "why": why}));
    };
    if !MANUAL_KEYS.iter().any(|(k, _)| *k == key) {
        return Err(format!("不支持 {key:?}；nai-ops manual --list 可看全部"));
    }
    let raw = value.ok_or("缺少新值（JSON；传 null 表示删除）")?;
    let v: Value = serde_json::from_str(raw).map_err(|e| format!("新值不是合法 JSON：{e}"))?;
    if key == "v5_plan" && v.is_null() {
        return Err("v5_plan 不能删除，只能改 global / each".into());
    }
    Ok(json!({"key": key, "value": v, "why": why}))
}

pub async fn run_manual(body: Value, yes: bool, dry_run: bool) -> Result<(i32, String)> {
    let plan = format!("POST /ops/manual {body}\n");
    if dry_run {
        return Ok((0, format!("{plan}（--dry-run：没有写入）\n")));
    }
    if !yes {
        return Ok((
            2,
            format!("{plan}没有写入：要真的修改请加 --yes（先预览用 --dry-run）\n"),
        ));
    }
    let admin = connect().await?;
    run_manual_with(&admin, &body).await
}

pub async fn run_manual_with(admin: &Admin, body: &Value) -> Result<(i32, String)> {
    let r = admin.post("/ops/manual", body).await?;
    if r.get("recorded").is_some() {
        return Ok((0, "已补记（操作者：手动（Claude））\n".into()));
    }
    let mut out = format!(
        "{}: {} → {}\n已写入，并记进操作日志（操作者：手动（Claude））\n",
        r["key"].as_str().unwrap_or(""),
        show(&r["before"]),
        show(&r["after"])
    );
    if let Some(n) = r["note"].as_str().filter(|n| !n.is_empty()) {
        out.push_str(&format!("{n}\n"));
    }
    Ok((0, out))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_values() {
        assert_eq!(parse_value(Kind::Int, " 12 "), Ok(json!(12)));
        assert!(parse_value(Kind::Int, "1.5").is_err());
        assert_eq!(parse_value(Kind::Float, "2.5"), Ok(json!(2.5)));
        assert!(parse_value(Kind::Float, "nan").is_err());
        assert_eq!(parse_value(Kind::Bool, "ON"), Ok(json!(true)));
        assert_eq!(parse_value(Kind::Bool, "0"), Ok(json!(false)));
        assert!(parse_value(Kind::Bool, "maybe").is_err());
        assert!(parse_value(Kind::Choice(&["off", "on"]), "auto").is_err());
        assert_eq!(parse_value(Kind::Roles, "[]"), Ok(json!([])));
        assert!(parse_value(Kind::Roles, "1:2").is_err());
    }

    #[test]
    fn show_numbers() {
        assert_eq!(show(&json!(5000.0)), "5000");
        assert_eq!(show(&json!(2.5)), "2.5");
        assert_eq!(show(&json!("")), "(空)");
    }

    #[test]
    fn names_are_unique_and_guard_fields_exist() {
        let mut names: Vec<_> = SPECS.iter().map(|s| s.name).collect();
        names.sort_unstable();
        let n = names.len();
        names.dedup();
        assert_eq!(n, names.len());
        for sp in SPECS.iter().filter(|s| s.ep == Ep::Guard) {
            assert!(
                gate_core::guard::FIELDS.iter().any(|f| f.name == sp.field),
                "{} 不是 guard 字段",
                sp.name
            );
        }
        assert!(list_text().contains("issue_hourly_cap"));
    }

    #[test]
    fn manual_bodies() {
        let b = manual_body(
            Some("guard_token_daily_caps"),
            Some(r#"{"daily": 400, "until": 2000}"#),
            "站长批准",
            None,
            None,
        )
        .unwrap();
        assert_eq!(
            b,
            json!({"key": "guard_token_daily_caps", "value": {"daily": 400, "until": 2000}, "why": "站长批准"})
        );
        assert_eq!(
            manual_body(Some("v5_paid_tokens"), Some("null"), "到期", None, None).unwrap()["value"],
            Value::Null
        );
        assert!(manual_body(Some("guard_token_daily_caps"), Some("{}"), " ", None, None).is_err());
        assert!(manual_body(Some("guard_account_daily_cap"), Some("1"), "x", None, None).is_err());
        assert!(
            manual_body(
                Some("guard_token_daily_caps"),
                Some("{oops"),
                "x",
                None,
                None
            )
            .is_err()
        );
        assert!(manual_body(Some("v5_plan"), Some("null"), "x", None, None).is_err());
        assert!(manual_body(None, None, "x", None, None).is_err());
        assert_eq!(
            manual_body(None, None, "补记", Some("重启网关"), None).unwrap(),
            json!({"action": "重启网关", "target": "", "why": "补记"})
        );
        assert!(manual_list_text().contains("guard_token_daily_caps"));
    }

    #[test]
    fn put_body_preserves_other_canonical_fields() {
        let current = json!({"budgets":{"daily_v5":150,"daily_anlas":200,"monthly_anlas":10000},"v5_paused":false,"v5_medium_mode":"off"});
        let body = put_body(find("global_daily_v5").unwrap(), &json!(0), &current).unwrap();
        assert_eq!(body["budgets"]["daily_v5"], 0);
        assert_eq!(body["budgets"]["daily_anlas"], 200);
        assert_eq!(body["v5_paused"], false);
        let current = json!({"registration":{"configured":true,"active":5,"open":false,"max_users":20,"expires_days":0,"roles":[]}});
        assert_eq!(
            put_body(find("register_open").unwrap(), &json!(true), &current).unwrap(),
            json!({"open":true,"max_users":20,"expires_days":0,"roles":[]})
        );
    }
}
