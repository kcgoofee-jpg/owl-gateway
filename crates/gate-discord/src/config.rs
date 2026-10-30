//! 环境变量（与 `deploy/v3/compose.yml` 的 `naimei` 服务一致）。
//!
//! | 变量 | 缺省 |
//! |---|---|
//! | `DISCORD_BOT_TOKEN` | 必填 |
//! | `DISCORD_GUILD_ID` | 必填 |
//! | `REGISTRATION_BRIDGE_SECRET` | 必填：没配或为空时启动即失败（和网关一样，不带着残缺的配置上线） |
//! | `REGISTRATION_BACKEND_URL` | `http://127.0.0.1:3003`（去掉末尾 `/`） |
//! | `SITE_URL` | 空串（去掉末尾 `/`） |
//! | `NAIMEI_LLM_BASE_URL` | 空：AI 对话关闭（OpenAI 兼容接口根地址，去尾 `/`） |
//! | `NAIMEI_LLM_API_KEY` | 空：AI 对话关闭（与 BASE_URL 同时配置才开启） |
//! | `NAIMEI_LLM_MODEL` | `glm-5.3-flash` |
//!
//! AI 对话是整段可缺省的功能：三项没有同时配齐就保持关闭，其余功能不受影响。

use anyhow::{Context, bail};

use crate::chat::ChatCfg;

#[derive(Debug, Clone)]
pub struct NaimeiConfig {
    pub token: String,
    pub guild_id: u64,
    /// 网关地址，已去掉末尾 `/`。
    pub backend: String,
    /// 站点地址，已去掉末尾 `/`。
    pub site: String,
    /// 桥接密钥（非空）。
    pub secret: String,
    /// AI 对话配置；`None` = 未配置，@奶妹 不触发对话。
    pub chat: Option<ChatCfg>,
}

pub const DEFAULT_BACKEND: &str = "http://127.0.0.1:3003";

impl NaimeiConfig {
    pub fn from_env() -> anyhow::Result<Self> {
        Self::from_lookup(|k| std::env::var(k).ok())
    }

    /// 便于测试：从任意查找函数构造。
    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> anyhow::Result<Self> {
        let token = get("DISCORD_BOT_TOKEN").context("缺少环境变量 DISCORD_BOT_TOKEN")?;
        if token.is_empty() {
            bail!("DISCORD_BOT_TOKEN 为空");
        }
        let guild = get("DISCORD_GUILD_ID").context("缺少环境变量 DISCORD_GUILD_ID")?;
        let guild_id = guild
            .trim()
            .parse::<u64>()
            .with_context(|| "DISCORD_GUILD_ID 不是整数".to_owned())?;
        let secret =
            get("REGISTRATION_BRIDGE_SECRET").context("缺少环境变量 REGISTRATION_BRIDGE_SECRET")?;
        if secret.is_empty() {
            bail!("REGISTRATION_BRIDGE_SECRET 为空");
        }
        Ok(NaimeiConfig {
            token,
            guild_id,
            backend: get("REGISTRATION_BACKEND_URL")
                .unwrap_or_else(|| DEFAULT_BACKEND.to_owned())
                .trim_end_matches('/')
                .to_owned(),
            site: get("SITE_URL")
                .unwrap_or_default()
                .trim_end_matches('/')
                .to_owned(),
            secret,
            chat: chat_cfg(&get),
        })
    }
}

/// AI 对话配置：`NAIMEI_LLM_BASE_URL` 与 `NAIMEI_LLM_API_KEY` 都非空才开启。
fn chat_cfg(get: &impl Fn(&str) -> Option<String>) -> Option<ChatCfg> {
    let base_url = get("NAIMEI_LLM_BASE_URL")?;
    let api_key = get("NAIMEI_LLM_API_KEY")?;
    if base_url.trim().is_empty() || api_key.trim().is_empty() {
        return None;
    }
    let model = get("NAIMEI_LLM_MODEL")
        .unwrap_or_else(|| "glm-5.3-flash".to_owned())
        .trim()
        .to_owned();
    Some(ChatCfg {
        base_url: base_url.trim().trim_end_matches('/').to_owned(),
        api_key: api_key.trim().to_owned(),
        model: if model.is_empty() {
            "glm-5.3-flash".to_owned()
        } else {
            model
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let m: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| ((*k).into(), (*v).into()))
            .collect();
        move |k| m.get(k).cloned()
    }

    #[test]
    fn defaults_and_trailing_slashes() {
        let cfg = NaimeiConfig::from_lookup(env(&[
            ("DISCORD_BOT_TOKEN", "t"),
            ("DISCORD_GUILD_ID", "900000000000000001"),
            ("REGISTRATION_BRIDGE_SECRET", "s"),
        ]))
        .unwrap();
        assert_eq!(cfg.backend, "http://127.0.0.1:3003");
        assert_eq!(cfg.site, "");
        assert_eq!(cfg.secret, "s");
        assert_eq!(cfg.guild_id, 900000000000000001);

        let cfg = NaimeiConfig::from_lookup(env(&[
            ("DISCORD_BOT_TOKEN", "t"),
            ("DISCORD_GUILD_ID", "900000000000000001"),
            ("REGISTRATION_BACKEND_URL", "http://nai-gate:8000//"),
            ("SITE_URL", "https://site.test/"),
            ("REGISTRATION_BRIDGE_SECRET", "s"),
        ]))
        .unwrap();
        assert_eq!(cfg.backend, "http://nai-gate:8000");
        assert_eq!(cfg.site, "https://site.test");
    }

    #[test]
    fn required_vars() {
        assert!(
            NaimeiConfig::from_lookup(env(&[
                ("DISCORD_GUILD_ID", "1"),
                ("REGISTRATION_BRIDGE_SECRET", "s")
            ]))
            .is_err()
        );
        assert!(
            NaimeiConfig::from_lookup(env(&[
                ("DISCORD_BOT_TOKEN", "t"),
                ("REGISTRATION_BRIDGE_SECRET", "s")
            ]))
            .is_err()
        );
        assert!(
            NaimeiConfig::from_lookup(env(&[
                ("DISCORD_BOT_TOKEN", "t"),
                ("DISCORD_GUILD_ID", "abc"),
                ("REGISTRATION_BRIDGE_SECRET", "s")
            ]))
            .is_err()
        );
    }

    #[test]
    fn chat_cfg_fully_or_not_at_all() {
        // 两项都配 → 开启，模型取缺省
        let cfg = NaimeiConfig::from_lookup(env(&[
            ("DISCORD_BOT_TOKEN", "t"),
            ("DISCORD_GUILD_ID", "900000000000000001"),
            ("REGISTRATION_BRIDGE_SECRET", "s"),
            ("NAIMEI_LLM_BASE_URL", "http://proxy.test:8080/"),
            ("NAIMEI_LLM_API_KEY", " k "),
        ]))
        .unwrap();
        let chat = cfg.chat.unwrap();
        assert_eq!(chat.base_url, "http://proxy.test:8080");
        assert_eq!(chat.api_key, "k");
        assert_eq!(chat.model, "glm-5.3-flash");

        // 显式指定模型
        let cfg = NaimeiConfig::from_lookup(env(&[
            ("DISCORD_BOT_TOKEN", "t"),
            ("DISCORD_GUILD_ID", "900000000000000001"),
            ("REGISTRATION_BRIDGE_SECRET", "s"),
            ("NAIMEI_LLM_BASE_URL", "http://proxy.test"),
            ("NAIMEI_LLM_API_KEY", "k"),
            ("NAIMEI_LLM_MODEL", "glm-5.3"),
        ]))
        .unwrap();
        assert_eq!(cfg.chat.unwrap().model, "glm-5.3");

        // 缺一项 / 空串 → 整段关闭
        for extra in [
            vec![("NAIMEI_LLM_BASE_URL", "http://proxy.test")],
            vec![("NAIMEI_LLM_API_KEY", "k")],
            vec![("NAIMEI_LLM_BASE_URL", " "), ("NAIMEI_LLM_API_KEY", "k")],
        ] {
            let mut pairs = vec![
                ("DISCORD_BOT_TOKEN", "t"),
                ("DISCORD_GUILD_ID", "900000000000000001"),
                ("REGISTRATION_BRIDGE_SECRET", "s"),
            ];
            pairs.extend(extra);
            assert!(
                NaimeiConfig::from_lookup(env(&pairs))
                    .unwrap()
                    .chat
                    .is_none(),
                "{pairs:?}"
            );
        }
    }

    #[test]
    fn missing_or_empty_bridge_secret_fails_at_startup() {
        for secret in [None, Some("")] {
            let mut pairs = vec![
                ("DISCORD_BOT_TOKEN", "t"),
                ("DISCORD_GUILD_ID", "900000000000000001"),
            ];
            if let Some(s) = secret {
                pairs.push(("REGISTRATION_BRIDGE_SECRET", s));
            }
            let err = NaimeiConfig::from_lookup(env(&pairs)).unwrap_err();
            assert!(
                err.to_string().contains("REGISTRATION_BRIDGE_SECRET"),
                "{err}"
            );
        }
    }
}
