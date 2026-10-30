//! Discord REST 传输：URL 拼装、带机器人 Token 的请求、把状态码折成 `DiscordErr`。

use super::*;

impl RegistrationService {
    pub fn url(&self, path: &str) -> String {
        format!("{}{path}", self.api_base)
    }

    pub(crate) async fn send(
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

    pub fn bot_auth(&self) -> String {
        format!("Bot {}", self.cfg.bot_token)
    }

    /// `_discord(method, path, bearer, json)`：状态码 ≥ 400 一律是 `Rejected`；成功时返回响应 JSON。
    pub(crate) async fn discord(
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
}
