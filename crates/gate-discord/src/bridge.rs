//! 网关桥接客户端：`/self-register/*`（命令）与 `/self-register/bot/*`（心跳）。
//!
//! 鉴权：`Authorization: Bearer <REGISTRATION_BRIDGE_SECRET>`（密钥缺失时机器人启动即失败，见 `config`）；超时：
//! - 命令类（[`Bridge::command`]）：POST JSON，8 秒；
//! - `/register`（[`Bridge::issue`]）：POST JSON，12 秒；
//! - 心跳（[`Bridge::bot`]）：8 秒，任何失败返回 None，不影响机器人其他功能。
//!
//! 超时是整个请求的上限；网关在同机 docker 网络里，连接几乎是瞬时的。

use std::time::Duration;

use serde_json::{Map, Value};

use crate::events::Interaction;
use crate::handlers::{Backend, HandlerError, interpret_backend};

pub const COMMAND_TIMEOUT: Duration = Duration::from_secs(8);
pub const REGISTER_TIMEOUT: Duration = Duration::from_secs(12);

#[derive(Clone)]
pub struct Bridge {
    http: reqwest::Client,
    backend: String,
    secret: String,
    command_timeout: Duration,
    register_timeout: Duration,
}

impl Bridge {
    /// `backend` 已去掉末尾的 `/`。
    pub fn new(backend: impl Into<String>, secret: impl Into<String>) -> Self {
        let http = reqwest::Client::builder()
            .user_agent(concat!("nai-bots/", env!("CARGO_PKG_VERSION")))
            .build()
            .expect("reqwest 客户端配置固定，不会失败");
        Bridge {
            http,
            backend: backend.into(),
            secret: secret.into(),
            command_timeout: COMMAND_TIMEOUT,
            register_timeout: REGISTER_TIMEOUT,
        }
    }

    /// 换掉超时（只给测试用：生产环境一律用 8 秒 / 12 秒）。
    #[must_use]
    pub fn with_timeouts(mut self, command: Duration, register: Duration) -> Self {
        self.command_timeout = command;
        self.register_timeout = register;
        self
    }

    fn bearer(&self) -> String {
        format!("Bearer {}", self.secret)
    }

    /// POST 到网关，返回 `(status, data)`。
    /// 连不上 / 超时 / 返回体不是 JSON → `(0, "服务暂时不可用，请稍后再试。")`。
    pub async fn command(&self, path: &str, payload: &Value) -> Result<Backend, HandlerError> {
        let sent = self
            .http
            .post(format!("{}{path}", self.backend))
            .timeout(self.command_timeout)
            .header("Authorization", self.bearer())
            .json(payload)
            .send()
            .await;
        let Ok(response) = sent else {
            return Ok(Backend::unavailable());
        };
        let status = response.status().as_u16();
        let Ok(body) = response.bytes().await else {
            return Ok(Backend::unavailable());
        };
        interpret_backend(status, &body)
    }

    /// `/register` 直发：返回网关的 `(status, body)`；传输层失败返回 `None`（调用方回「领 Key 暂不可用」）。
    pub async fn issue(&self, payload: &Value) -> Option<(u16, Vec<u8>)> {
        let response = self
            .http
            .post(format!("{}/self-register/issue", self.backend))
            .timeout(self.register_timeout)
            .header("Authorization", self.bearer())
            .json(payload)
            .send()
            .await
            .ok()?;
        let status = response.status().as_u16();
        let body = response.bytes().await.ok()?;
        Some((status, body.to_vec()))
    }

    /// 机器人专用接口 `/self-register/bot{path}`（心跳）；非 200 / 连不上 / 不是 JSON 一律 `None`。
    pub async fn bot(&self, path: &str, payload: &Value) -> Option<Value> {
        let response = self
            .http
            .post(format!("{}/self-register/bot{path}", self.backend))
            .timeout(self.command_timeout)
            .header("Authorization", self.bearer())
            .json(payload)
            .send()
            .await
            .ok()?;
        if response.status().as_u16() != 200 {
            return None;
        }
        response.json::<Value>().await.ok()
    }
}

/// 发给网关的请求体：`{discord_id, guild_id, ...extra}`，`discord_id` 是发起命令的人。
pub fn who_payload(it: &Interaction, extra: &[(&str, Value)]) -> Result<Value, HandlerError> {
    let user = it
        .actor()
        .ok_or_else(|| HandlerError::Malformed("interaction has no user".into()))?;
    let mut map = Map::new();
    map.insert("discord_id".into(), Value::String(user.id.clone()));
    map.insert("guild_id".into(), Value::String(it.guild_id_str()));
    for (k, v) in extra {
        map.insert((*k).to_owned(), v.clone());
    }
    Ok(Value::Object(map))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn interaction() -> Interaction {
        serde_json::from_value(json!({
            "id": "900000000000000010", "type": 2, "token": "t", "guild_id": "900000000000000001",
            "member": {"user": {"id": "900000000000000005", "username": "alice"}},
            "data": {"name": "quota"},
        }))
        .unwrap()
    }

    #[test]
    fn who_payload_default_and_extra() {
        let p = who_payload(&interaction(), &[("text", json!("你好"))]).unwrap();
        assert_eq!(
            p.to_string(),
            r#"{"discord_id":"900000000000000005","guild_id":"900000000000000001","text":"你好"}"#
        );
    }

    #[test]
    fn who_payload_without_a_server_says_none() {
        let it: Interaction = serde_json::from_value(json!({
            "id": "900000000000000010", "type": 2, "token": "t",
            "user": {"id": "900000000000000005", "username": "alice"},
            "data": {"name": "help"},
        }))
        .unwrap();
        let p = who_payload(&it, &[]).unwrap();
        assert_eq!(p["guild_id"], "None");
    }

    #[test]
    fn who_payload_extra_overrides_in_place() {
        let p = who_payload(&interaction(), &[("guild_id", json!("x"))]).unwrap();
        assert_eq!(
            p.to_string(),
            r#"{"discord_id":"900000000000000005","guild_id":"x"}"#
        );
    }
}
