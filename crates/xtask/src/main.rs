//! 开发辅助命令（`cargo xtask <子命令>`，别名见 `.cargo/config.toml`）。只用于本地开发与端到端测试，不进发布产物。
//!
//! - `cargo xtask dev`：本地网关 + 假上游 + 合成演示数据，见 [`dev`]。

mod dev;
mod seed;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "xtask", about = "猫头鹰公益站开发辅助命令")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// 起本地开发环境：假上游 + 网关 + 合成演示数据（前端开发与 Playwright 端到端的后端）
    Dev(dev::Args),
}

fn main() -> anyhow::Result<()> {
    match Cli::parse().cmd {
        Cmd::Dev(args) => dev::run(args),
    }
}
