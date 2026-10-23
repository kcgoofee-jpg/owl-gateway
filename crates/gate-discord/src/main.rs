//! `nai-bots naimei`：奶妹（docker-compose 里 `owl-naimei` 容器）的入口。

use std::process::ExitCode;
use std::time::Duration;

use clap::{Parser, Subcommand};
use gate_discord::{config, naimei};
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(name = "nai-bots", version, about = "猫头鹰公益站的 Discord 机器人")]
struct Cli {
    #[command(subcommand)]
    bot: Bot,
}

#[derive(Subcommand)]
enum Bot {
    /// 奶妹：领 Key / 额度 / 反馈（需要 DISCORD_BOT_TOKEN、DISCORD_GUILD_ID、REGISTRATION_BRIDGE_SECRET）
    Naimei,
}

/// 致命错误后先等一会儿再退出：容器是 `restart: unless-stopped`，别让它飞快地重复登录（会被 Discord 限流）。
const FATAL_BACKOFF: Duration = Duration::from_secs(30);

#[tokio::main]
async fn main() -> ExitCode {
    // rustls 的默认加密后端显式指定为 ring（reqwest / twilight 共用），避免依赖图变化后运行时 panic
    let _ = rustls::crypto::ring::default_provider().install_default();
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_target(false)
        .with_ansi(false)
        .init();

    let result = match Cli::parse().bot {
        Bot::Naimei => match config::NaimeiConfig::from_env() {
            Ok(cfg) => naimei::run(cfg).await,
            Err(e) => Err(e),
        },
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            tracing::error!("[bug] 机器人退出：{e:#}");
            tokio::time::sleep(FATAL_BACKOFF).await;
            ExitCode::FAILURE
        }
    }
}
