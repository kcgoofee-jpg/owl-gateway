//! 站长告警：机器人私信站长（`ALERT_USER_ID`）。成员公告（机器人发到公告频道）10-23 删除。
//! - 只发事件类型和一句话，不含 Key、提示词或成员内容；
//! - 同一 kind 在冷却期内只发一次；
//! - **只有重大事故才私信**（站长 10-15：有后台，非重大事故不私信）——成员现在用不了、
//!   或账号 / 数据有风险，且需要站长动手：上游 Token 失效（`upstream_401`）、连续 403 冷却
//!   （`upstream_403_cooldown`）、上游 Token 被改动（`upstream_token_changed`）、后台密码被修改（`admin_password_changed`）、
//!   磁盘剩余不足 10%（`disk_low`）、
//!   后台「发送测试告警」（`test`）。其余事件在发生的地方直接写一行日志，后台总览能看到同样的情况。

use serde_json::json;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Instant;

/// 发到哪里。
enum Target {
    /// 私信这个 Discord 用户（第一次发时建私信频道并缓存）。
    Dm(String),
}

pub struct Alerter {
    bot_token: String,
    target: Target,
    api_base: String,
    last: Mutex<HashMap<String, Instant>>,
    dm_channel: tokio::sync::Mutex<Option<String>>,
    http: reqwest::Client,
    pub sent: std::sync::atomic::AtomicU64,
}

impl Alerter {
    fn new(api_base: &str, bot_token: &str, target: Target) -> Self {
        Self {
            bot_token: bot_token.to_string(),
            target,
            api_base: api_base.trim_end_matches('/').to_string(),
            last: Mutex::new(HashMap::new()),
            dm_channel: tokio::sync::Mutex::new(None),
            // 没有 UA 会被 Discord 前面的 Cloudflare 判成机器人直接 403（1010），告警就悄悄发不出去
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(10))
                .user_agent(gate_core::discord_ua::DISCORD_USER_AGENT)
                .build()
                .unwrap_or_default(),
            sent: Default::default(),
        }
    }

    /// 站长告警通道：私信 `ALERT_USER_ID`。
    pub fn owner(env: &crate::env::EnvConfig) -> Self {
        Self::new(
            &env.discord_api_base,
            &env.discord_bot_token,
            Target::Dm(env.alert_user_id.trim().to_string()),
        )
    }

    pub fn configured(&self) -> bool {
        let Target::Dm(id) = &self.target;
        !self.bot_token.is_empty() && !id.is_empty()
    }

    /// 私信站长（不阻塞调用方）；同一 kind 在 `cooldown` 秒内只发一次。
    pub fn notify(self: &std::sync::Arc<Self>, kind: &str, message: &str, cooldown: f64) {
        {
            let mut last = self.last.lock().expect("alerts");
            let now = Instant::now();
            if last
                .get(kind)
                .is_some_and(|t| now.duration_since(*t).as_secs_f64() < cooldown)
            {
                return;
            }
            last.insert(kind.to_string(), now);
        }
        self.post(&format!("⚠ **猫头鹰公益站告警**（{kind}）\n{message}"));
    }

    /// 不带告警前缀，无冷却。
    fn post(self: &std::sync::Arc<Self>, text: &str) {
        if !self.configured() {
            return;
        }
        let me = self.clone();
        let text = text.to_string();
        if let Ok(rt) = tokio::runtime::Handle::try_current() {
            rt.spawn(async move { me.send(&text).await });
        }
    }

    async fn send(&self, text: &str) {
        let content: String = gate_core::pyc::take_chars(text, 1900).to_string();
        let body = json!({"content": content, "allowed_mentions": {"parse": []}});
        let result: Result<(), String> = async {
            let auth = format!("Bot {}", self.bot_token);
            let channel = match &self.target {
                Target::Dm(user_id) => {
                    let mut dm = self.dm_channel.lock().await;
                    if dm.is_none() {
                        let created: serde_json::Value = self
                            .http
                            .post(format!("{}/users/@me/channels", self.api_base))
                            .header("Authorization", &auth)
                            .json(&json!({"recipient_id": user_id}))
                            .send()
                            .await
                            .map_err(|e| e.to_string())?
                            .error_for_status()
                            .map_err(|e| e.to_string())?
                            .json()
                            .await
                            .map_err(|e| e.to_string())?;
                        *dm = created
                            .get("id")
                            .and_then(|v| v.as_str())
                            .map(str::to_string);
                    }
                    dm.clone().ok_or("no dm channel")?
                }
            };
            self.http
                .post(format!("{}/channels/{channel}/messages", self.api_base))
                .header("Authorization", &auth)
                .json(&body)
                .send()
                .await
                .map_err(|e| e.to_string())?
                .error_for_status()
                .map_err(|e| e.to_string())?;
            Ok(())
        }
        .await;
        match result {
            Ok(()) => {
                self.sent.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
            // 告警失败不能影响服务；不打印错误详情（可能含 URL / Token）
            Err(_) => tracing::warn!("alert delivery failed"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gate_core::discord_ua::DISCORD_USER_AGENT;
    use gate_testkit::{MockUpstream, Reply};

    /// 告警请求必须带 Discord 认可的 UA（默认 UA 会被 Cloudflare 1010 拦），与注册服务、运维命令共用一个。
    #[tokio::test]
    async fn alert_requests_carry_the_shared_discord_user_agent() {
        let mock = MockUpstream::start().await;
        mock.push(
            "/users/@me/channels",
            Reply::json(200, &json!({"id": "900000000000000030"})),
        );
        // 私信站长：先建私信频道，再发到这个频道
        let owner = Alerter::new(
            &mock.url(),
            "synthetic-bot",
            Target::Dm("900000000000000031".into()),
        );
        owner.send("测试").await;
        let reqs = mock.requests();
        let paths: Vec<&str> = reqs.iter().map(|r| r.path.as_str()).collect();
        assert_eq!(
            paths,
            [
                "/users/@me/channels",
                "/channels/900000000000000030/messages"
            ]
        );
        assert_eq!(
            reqs[0].json(),
            json!({"recipient_id": "900000000000000031"})
        );
        for r in &reqs {
            assert_eq!(
                r.header("user-agent"),
                Some(DISCORD_USER_AGENT),
                "{}",
                r.path
            );
        }
    }
}
