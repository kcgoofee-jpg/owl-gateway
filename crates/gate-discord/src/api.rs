//! Discord REST：用 twilight-http 发请求（含它自带的限流器），请求体自己拼成 JSON。
//!
//! 请求体为什么不用 twilight 的 typed builder：
//! - 发出去的键要固定（`tts:false`、`flags:64` 的写法），typed builder 会多发或少发默认字段；
//! - 纯函数拼体，不连 Discord 就能断言发出去的内容（见 `tests/flows.rs`）。
//!
//! 交互回调 / 跟进消息是「令牌即鉴权」的接口，不带机器人 Authorization。

use serde_json::{Value, json};
use twilight_http::{Client, request::Request, routing::Route};

/// 消息标志位：EPHEMERAL（只有触发者可见）。
pub const EPHEMERAL: u64 = 64;

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("无效的 ID：{0}")]
    BadId(String),
    #[error("Discord 请求失败：{0}")]
    Http(#[from] twilight_http::Error),
    #[error("响应体读取失败：{0}")]
    Body(#[from] twilight_http::response::DeserializeBodyError),
}

fn id(s: &str) -> Result<u64, ApiError> {
    s.parse().map_err(|_| ApiError::BadId(s.to_owned()))
}

// ---------- 请求体（纯函数） ----------

/// 先确认收到（3 秒内必须回）：类型 5（DEFERRED_CHANNEL_MESSAGE），
/// 带 `flags: 64`（只有触发者可见）。
pub fn defer_body() -> Value {
    json!({"type": 5, "data": {"flags": EPHEMERAL}})
}

/// 直接回复一条只有本人可见的消息：类型 4。
pub fn message_body(content: &str) -> Value {
    json!({"type": 4, "data": {"tts": false, "content": content, "flags": EPHEMERAL}})
}

/// 跟进消息（只有本人可见）。
pub fn followup_body(content: &str) -> Value {
    json!({"content": content, "tts": false, "flags": EPHEMERAL})
}

// ---------- 发送 ----------

#[derive(Clone)]
pub struct DiscordApi {
    client: std::sync::Arc<Client>,
}

impl DiscordApi {
    /// `proxy` 仅测试用：把所有 REST 请求指到本机假 Discord（`host:port`，明文 HTTP）。
    pub fn new(token: &str, proxy: Option<String>) -> Self {
        let mut builder = Client::builder().token(token.to_owned());
        if let Some(host) = proxy {
            // 假服务器没有限流头，关掉限流器；生产环境不传 proxy，保留 twilight 默认限流器
            builder = builder.proxy(host, true).ratelimiter(None);
        }
        DiscordApi {
            client: std::sync::Arc::new(builder.build()),
        }
    }

    async fn send(&self, request: Result<Request, twilight_http::Error>) -> Result<(), ApiError> {
        self.client.request::<()>(request?).await?;
        Ok(())
    }

    /// `POST /interactions/{id}/{token}/callback?with_response=true`
    pub async fn callback(
        &self,
        interaction_id: &str,
        token: &str,
        body: &Value,
    ) -> Result<(), ApiError> {
        let route = Route::InteractionCallback {
            interaction_id: id(interaction_id)?,
            interaction_token: token,
            with_response: true,
        };
        self.send(
            Request::builder(&route)
                .json(body)
                .use_authorization_token(false)
                .build(),
        )
        .await
    }

    /// `POST /webhooks/{application_id}/{token}?wait=true`（交互跟进消息）。
    pub async fn followup(
        &self,
        application_id: &str,
        token: &str,
        body: &Value,
    ) -> Result<(), ApiError> {
        let route = Route::ExecuteWebhook {
            thread_id: None,
            token,
            wait: Some(true),
            with_components: Some(false),
            webhook_id: id(application_id)?,
        };
        self.send(
            Request::builder(&route)
                .json(body)
                .use_authorization_token(false)
                .build(),
        )
        .await
    }

    /// `PUT /applications/{app}/guilds/{guild}/commands`（整体覆盖：不在列表里的旧命令会被 Discord 删掉）。
    pub async fn sync_guild_commands(
        &self,
        application_id: &str,
        guild_id: u64,
        definitions: &[Value],
    ) -> Result<(), ApiError> {
        let route = Route::SetGuildCommands {
            application_id: id(application_id)?,
            guild_id,
        };
        self.send(Request::builder(&route).json(&definitions).build())
            .await
    }

    /// `POST /channels/{id}/messages`（频道里发消息；body 由调用方拼，须带 `allowed_mentions`）。
    pub async fn create_message(&self, channel_id: u64, body: &Value) -> Result<(), ApiError> {
        let route = Route::CreateMessage { channel_id };
        self.send(Request::builder(&route).json(body).build()).await
    }

    /// 同上，但取回新消息的 ID（流式回复要先发占位、再反复编辑）。
    pub async fn create_message_get_id(
        &self,
        channel_id: u64,
        body: &Value,
    ) -> Result<String, ApiError> {
        let route = Route::CreateMessage { channel_id };
        let response = self
            .client
            .request::<Value>(Request::builder(&route).json(body).build()?)
            .await?;
        let v = response.model().await.map_err(ApiError::Body)?;
        v["id"]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| ApiError::BadId("新建消息没有 id".into()))
    }

    /// `PATCH /channels/{id}/messages/{msg}`（流式回复的渐进编辑）。
    pub async fn edit_message(
        &self,
        channel_id: u64,
        message_id: &str,
        body: &Value,
    ) -> Result<(), ApiError> {
        let route = Route::UpdateMessage {
            channel_id,
            message_id: id(message_id)?,
        };
        self.send(Request::builder(&route).json(body).build()).await
    }

    /// `POST /channels/{id}/typing`（「正在输入…」，有效期约 10 秒；失败无所谓）。
    pub async fn create_typing_trigger(&self, channel_id: u64) -> Result<(), ApiError> {
        let route = Route::CreateTypingTrigger { channel_id };
        self.send(Request::builder(&route).build()).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defer_bodies() {
        assert_eq!(
            defer_body().to_string(),
            r#"{"type":5,"data":{"flags":64}}"#
        );
        // 弹窗提交：类型 6 不带 data
    }

    #[test]
    fn message_and_followup_bodies() {
        assert_eq!(
            message_body("你好").to_string(),
            r#"{"type":4,"data":{"tts":false,"content":"你好","flags":64}}"#
        );
        assert_eq!(
            followup_body("你好").to_string(),
            r#"{"content":"你好","tts":false,"flags":64}"#
        );
    }
}
