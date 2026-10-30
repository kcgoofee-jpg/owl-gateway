//! 网页登录：OAuth 地址、换 token 拿到身份、读服务器列表与外部账号绑定。

use super::*;

impl RegistrationService {
    /// 网页登录 OAuth 的 scope：普通登录见 [`WEB_LOGIN_SCOPES`]；「验证身份组」再多要 guilds.members.read。
    pub fn oauth_scope(verify: bool) -> &'static str {
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
            .ok_or_else(|| RegErr::Internal("Discord OAuth 响应缺少 access_token".into()))?;
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
    pub async fn list_user_guilds(&self, bearer: &str) -> Option<Vec<(String, String)>> {
        let r = self
            .send(Method::GET, "/users/@me/guilds", bearer, 8, None)
            .await
            .ok()
            .filter(|r| r.status == 200)?;
        Some(parse_guilds(&r.json()?))
    }

    /// 用成员自己的授权查他绑定的外部账号（scope `connections`）。任何失败都返回 None。
    pub async fn list_user_connections(&self, bearer: &str) -> Option<Vec<DiscordConnection>> {
        let r = self
            .send(Method::GET, "/users/@me/connections", bearer, 8, None)
            .await
            .ok()
            .filter(|r| r.status == 200)?;
        Some(parse_connections(&r.json()?))
    }
}
