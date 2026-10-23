//! Discord REST（用 bot token，不走网关连接）：只服务站长告警私信（巡检告警）。
//!
//! 基地址从环境变量 `DISCORD_API_BASE` 读（默认官方地址）——测试里指向本机假服务，
//! 绝不会真的发消息。Discord 前面有 Cloudflare：默认 UA 会被判成机器人直接 403（error code 1010），
//! 所以统一用 `DiscordBot (站点地址, 版本)`：站点地址读已有配置 `SITE_URL`，读不到就用与网关相同的中性写法
//! （`gate_core::discord_ua`，网关的注册服务和站长告警也用它）。

use anyhow::{Context, Result};
use serde_json::Value;
use std::path::Path;
use std::time::Duration;

pub const DEFAULT_API_BASE: &str = "https://discord.com/api/v10";

/// 请求 Discord 用的 UA：读得到站点地址就带上，否则用与网关相同的中性默认（见 [`gate_core::discord_ua`]）。
pub fn user_agent_from_env() -> String {
    gate_core::discord_ua::discord_user_agent(&crate::util::env_or_file(
        &crate::util::env_file(),
        "SITE_URL",
    ))
}

/// 站长告警私信的凭据（机器人 Token, 收件人 ID）：进程环境变量优先，其次 `.env` 文件。缺哪个就是空串。
pub fn credentials(env_file: &Path) -> (String, String) {
    (
        crate::util::env_or_file(env_file, "DISCORD_BOT_TOKEN"),
        crate::util::env_or_file(env_file, "ALERT_USER_ID"),
    )
}

/// `DISCORD_API_BASE`（去掉末尾斜杠）。
pub fn api_base_from_env() -> String {
    crate::util::env_nonempty("DISCORD_API_BASE")
        .unwrap_or_else(|| DEFAULT_API_BASE.to_string())
        .trim_end_matches('/')
        .to_string()
}

/// 一次 REST 调用的结果；传输层错误用 `Err` 返回。
#[derive(Debug, Clone)]
pub struct Resp {
    pub status: u16,
    pub body: String,
}

impl Resp {
    pub fn json(&self) -> Option<Value> {
        serde_json::from_str(&self.body).ok()
    }
    pub fn ok(&self) -> bool {
        matches!(self.status, 200 | 201)
    }
}

pub struct Discord {
    base: String,
    token: String,
    http: reqwest::Client,
}

impl Discord {
    pub fn new(base: &str, token: &str, timeout: Duration) -> Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(timeout)
            .user_agent(user_agent_from_env())
            .build()
            .context("创建 HTTP 客户端失败")?;
        Ok(Self {
            base: base.trim_end_matches('/').to_string(),
            token: token.to_string(),
            http,
        })
    }

    async fn send(&self, req: reqwest::RequestBuilder) -> Result<Resp> {
        let resp = req
            .header("Authorization", format!("Bot {}", self.token))
            .header("Accept", "application/json")
            .send()
            .await?;
        let status = resp.status().as_u16();
        let body = resp.text().await.unwrap_or_default();
        Ok(Resp { status, body })
    }

    /// 往频道发消息。
    pub async fn post_message(&self, channel: &str, body: &Value) -> Result<Resp> {
        let url = format!("{}/channels/{channel}/messages", self.base);
        self.send(self.http.post(url).json(body)).await
    }

    /// 打开（或取回）与某人的私信频道。
    pub async fn create_dm(&self, user_id: &str) -> Result<Resp> {
        let url = format!("{}/users/@me/channels", self.base);
        let body = serde_json::json!({"recipient_id": user_id});
        self.send(self.http.post(url).json(&body)).await
    }
}

/// 给一个人发私信（先开私信频道再发消息，关掉所有 @ 提及）。失败返回不含密钥的简短原因。
/// 巡检告警用它给站长发通知；运维命令里没有任何面向成员的私信入口。
pub async fn send_dm(base: &str, token: &str, user: &str, text: &str) -> Result<(), String> {
    let transport = |e: &anyhow::Error| {
        let name = e.downcast_ref::<reqwest::Error>().map_or("Error", |r| {
            if r.is_timeout() {
                "TimeoutError"
            } else if r.is_connect() {
                "URLError"
            } else {
                "Error"
            }
        });
        format!("网络错误 {name}")
    };
    let status = |what: &str, r: &Resp| {
        let hint = if r.status == 403 && gate_core::pyc::take_chars(&r.body, 200).contains("1010") {
            "（Cloudflare 1010：User-Agent 被拦）"
        } else {
            ""
        };
        format!("{what} HTTP {}{hint}", r.status)
    };
    let d = Discord::new(base, token, Duration::from_secs(10)).map_err(|e| transport(&e))?;
    let resp = d.create_dm(user).await.map_err(|e| transport(&e))?;
    if !resp.ok() {
        return Err(status("创建私信频道", &resp));
    }
    let channel = resp
        .json()
        .and_then(|j| j.get("id").and_then(Value::as_str).map(String::from))
        .ok_or_else(|| "创建私信频道：返回里没有 id".to_string())?;
    let resp = d
        .post_message(&channel, &message_body(text))
        .await
        .map_err(|e| transport(&e))?;
    if resp.ok() {
        Ok(())
    } else {
        Err(status("发送消息", &resp))
    }
}

/// 不允许任何 @ 提及的消息体（站长告警文案里的 @everyone 也不会生效）。
pub fn message_body(text: &str) -> Value {
    serde_json::json!({
        "content": gate_core::pyc::take_chars(text, 1900),
        "allowed_mentions": {"parse": []},
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use gate_core::discord_ua::{DISCORD_USER_AGENT, discord_user_agent};

    #[test]
    fn site_url_comes_from_existing_config() {
        let dir = tempfile::tempdir().unwrap();
        let env_file = dir.path().join(".env");
        let site = |f: &Path| crate::util::env_or_file(f, "OWL_TEST_SITE_URL_UNSET");
        // 读不到配置：中性默认，不写死任何域名
        assert_eq!(site(&env_file), "");
        assert_eq!(discord_user_agent(&site(&env_file)), DISCORD_USER_AGENT);
        // .env 里有 SITE_URL
        std::fs::write(
            &env_file,
            "A=1\nOWL_TEST_SITE_URL_UNSET=\"https://owl.example/\"\n",
        )
        .unwrap();
        assert_eq!(site(&env_file), "https://owl.example/");
        assert_eq!(
            discord_user_agent(&site(&env_file)),
            format!(
                "DiscordBot (https://owl.example, {})",
                env!("CARGO_PKG_VERSION")
            )
        );
    }
}
