//! `cargo xtask dev`：本地开发环境，一个进程里起三样东西——
//!
//! 1. 假上游（`gate_testkit::MockUpstream`，默认 18001）：出图 / 工具 / 标签补全 / 订阅查询都有默认回复，不连真实 NovelAI；
//! 2. 合成演示数据（`seed`）：每次启动都重建数据目录（默认 `target/owl-dev`），并写出 `cookies.json`
//!    （后台与成员会话，Playwright 端到端读它）；
//! 3. 真正的网关（`AppState::build` + `app`，默认 18000，只监听本机），环境变量与 2.x 的本地演示脚本一致：
//!    假 Discord 配置让「领 Key / 网页登录」路径处于已配置状态，`DISCORD_API_BASE` 指向一个没人听的本机端口，
//!    所以任何 Discord 请求都会立刻失败，不会出本机；官方状态页地址同样指向死端口。
//!
//! 前端另开：`cd web && pnpm vite`（把接口代理到 `VITE_GATE`，默认 `http://127.0.0.1:18000`）。Ctrl-C 退出。

use crate::seed::{self, Seeded};
use anyhow::{Context, Result, bail};
use gate_core::clock::Clock;
use gate_db::Db;
use gate_server::env::EnvConfig;
use gate_server::key::Key;
use gate_server::session;
use gate_server::web::Assets;
use gate_server::{AppState, jobs};
use gate_testkit::MockUpstream;
use std::collections::HashMap;
use std::future::IntoFuture;
use std::net::{Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// 后台写接口放行的前端来源：Vite 开发服务（5173）与端到端用的（5174）。
const DEFAULT_ORIGINS: &str = "http://localhost:5173,http://127.0.0.1:5173,\
    http://localhost:5174,http://127.0.0.1:5174";
/// 数据目录里的记号：只有带它的目录才会被清掉重建，防止 `--data` 手滑指到别的目录。
const MARKER: &str = ".owl-dev";

#[derive(clap::Args)]
pub struct Args {
    /// 数据目录（合成数据，每次启动都清掉重建）
    #[arg(long, value_name = "DIR")]
    data: Option<PathBuf>,
    /// 网关端口（只监听 127.0.0.1）
    #[arg(long, default_value_t = 18000)]
    gate_port: u16,
    /// 假上游端口（只监听 127.0.0.1）
    #[arg(long, default_value_t = 18001)]
    mock_port: u16,
    /// 前端开发服务的地址（SITE_URL：登录回跳、领取回复里的站点地址）
    #[arg(long, default_value = "http://127.0.0.1:5173")]
    site_url: String,
    /// 后台写接口放行的前端来源，逗号分隔
    #[arg(long, env = "ADMIN_ALLOWED_ORIGINS", default_value = DEFAULT_ORIGINS)]
    origins: String,
}

pub fn run(args: Args) -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            // 「注意」级的 Bug（target = bug，客户端断开之类）会存成后台的 Bug 记录，不用在终端里再刷一遍；真正的错误照常显示
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "warn,bug=error".into()),
        )
        .init();
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(serve(args))
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf)
}

/// 建好（清空重建）数据目录，返回绝对路径。目录已存在且不是本命令建的就拒绝。
fn prepare_data_dir(arg: Option<PathBuf>) -> Result<PathBuf> {
    let dir = arg.unwrap_or_else(|| workspace_root().join("target/owl-dev"));
    let dir = if dir.is_absolute() {
        dir
    } else {
        std::env::current_dir()?.join(dir)
    };
    if dir.exists() {
        let empty = std::fs::read_dir(&dir)?.next().is_none();
        if !empty && !dir.join(MARKER).exists() {
            bail!(
                "{} 已存在且不是 cargo xtask dev 建的目录（没有 {MARKER} 记号），不会清空它；换一个 --data 目录",
                dir.display()
            );
        }
        std::fs::remove_dir_all(&dir)?;
    }
    std::fs::create_dir_all(&dir)?;
    std::fs::write(
        dir.join(MARKER),
        "cargo xtask dev 建的合成数据目录：每次启动都会清掉重建，不要放别的东西\n",
    )?;
    Ok(dir)
}

/// 找一个没人听的本机端口（拿来当 Discord / 官方状态页的地址，请求会立刻被拒绝）。
fn dead_local_port() -> Result<u16> {
    let l = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    Ok(l.local_addr()?.port())
}

/// 2.x 本地演示脚本的环境变量，加上 Rust 网关里需要显式关掉的几项（上游抖动、HTTP/2）。
fn gateway_env(
    args: &Args,
    data: &Path,
    mock: &MockUpstream,
    dead: u16,
) -> HashMap<String, String> {
    let mock_url = mock.url();
    [
        ("DATA_DIR", data.to_string_lossy().into_owned()),
        ("ADMIN_PASSWORD", seed::ADMIN_PASSWORD.into()),
        ("NAI_TOKENS", seed::DEV_UPSTREAM_TOKEN.into()),
        ("NAI_IMAGE_HOST", mock_url),
        // 假 Discord 配置：让自助领 Key / 网页登录处于「已配置」状态；地址是死端口，什么都发不出去
        ("DISCORD_CLIENT_ID", "1".into()),
        ("DISCORD_CLIENT_SECRET", "dev".into()),
        ("DISCORD_BOT_TOKEN", "dev".into()),
        ("DISCORD_GUILD_ID", "1".into()),
        ("DISCORD_API_BASE", format!("http://127.0.0.1:{dead}")),
        // 示例邀请链接：首页「加入 Discord」按钮在本地也能看到（点了打不开，只用来看排版）
        (
            "DISCORD_INVITE_URL",
            "https://discord.gg/owl-dev-example".into(),
        ),
        ("SITE_URL", args.site_url.clone()),
        // 本地验证「只有站长可用」维护档：直接用合成成员 ID
        (
            "MAINTENANCE_OWNER_IDS",
            std::env::var("MAINTENANCE_OWNER_IDS").unwrap_or_default(),
        ),
        (
            "REGISTRATION_BRIDGE_SECRET",
            "dev-bridge-secret-0123456789abcdefghij".into(),
        ),
        ("ADMIN_ALLOWED_ORIGINS", args.origins.clone()),
        ("ADMIN_COOKIE_SECURE", "0".into()),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect()
}

async fn serve(args: Args) -> Result<()> {
    // 先占端口：端口被占多半是另一个 cargo xtask dev 还在跑，这时不能去清它的数据目录
    let mock = MockUpstream::start_on((Ipv4Addr::LOCALHOST, args.mock_port))
        .await
        .with_context(|| {
            format!(
                "假上游端口 {} 被占用（已经有一个 cargo xtask dev 在跑？可用 --mock-port 换端口）",
                args.mock_port
            )
        })?;
    let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, args.gate_port))
        .await
        .with_context(|| {
            format!(
                "网关端口 {} 被占用（已经有一个 cargo xtask dev 在跑？可用 --gate-port 换端口）",
                args.gate_port
            )
        })?;
    let data = prepare_data_dir(args.data.clone())?;
    let dead = dead_local_port()?;
    std::fs::write(data.join("secret_key"), seed::SECRET)?;

    let mut env = EnvConfig::load(&gateway_env(&args, &data, &mock, dead));
    // 这两项是代码常量，不读环境变量：假上游出完不用歇；反复跑端到端不要被登录限流挡住
    env.pace_images = false;
    env.login_max_attempts = 100;
    let now = gate_server::time::now();
    let clock = Clock::service();

    // 先把数据写进库、关掉这个连接，再让网关像对着一份现成的库那样启动（启动时的回填、停机检测都按真实路径走）
    let seeded = {
        let db = Db::open_for_gateway(env.db_path(), now, env.key_inactivity_delete_days)?;
        seed::seed(&db, &clock, now)
            .await
            .context("写入合成演示数据失败")?
    };

    let assets = Assets::new(std::env::var_os("STATIC_DIR").map(Into::into));
    let state = AppState::build(env, assets).await?;
    jobs::startup(&state)
        .await
        .map_err(|e| anyhow::anyhow!("{e:?}"))?;
    write_cookies(&state, &seeded, &data).await?;
    print_banner(&data, &listener, &mock);

    let app = gate_server::app(state.clone());
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
    let mut server = Box::pin(
        axum::serve(
            listener,
            axum::ServiceExt::<axum::extract::Request>::into_make_service_with_connect_info::<
                SocketAddr,
            >(app),
        )
        .with_graceful_shutdown(async {
            let _ = stop_rx.await;
        })
        .into_future(),
    );
    tokio::select! {
        r = &mut server => r?,
        () = wait_for_signal() => {
            eprintln!("\n收到停止信号，正在退出……");
            state.shutdown.cancel();
            let _ = stop_tx.send(());
            // 浏览器 / Vite 代理的长连接（SSE）可能让优雅关停拖很久：最多等 5 秒
            if tokio::time::timeout(Duration::from_secs(5), &mut server).await.is_err() {
                eprintln!("仍有连接没有关闭，强制结束。");
            }
        }
    }
    jobs::shutdown(&state).await;
    drop(mock);
    Ok(())
}

async fn wait_for_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let term = async {
        if let Ok(mut s) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            s.recv().await;
        }
    };
    #[cfg(not(unix))]
    let term = std::future::pending::<()>();
    tokio::select! { () = ctrl_c => {}, () = term => {} }
}

/// `cookies.json`：和旧 `seed_demo.py` 一样的五个字段——后台会话、成员会话、成员的 Discord ID、后台密码、成员 Key。
/// 会话用网关自己的签名函数签发（密钥来自刚写下的 `secret_key`），所以对这个网关一定有效。
async fn write_cookies(state: &AppState, seeded: &Seeded, data: &Path) -> Result<()> {
    let member = seeded.keys.first().context("没有成员")?;
    let row = state
        .db
        .get_key(member.id)
        .await?
        .context("刚建的 Key 读不到")?;
    let tag = session::key_tag(Some(&Key::load(&state.db, row).await?));
    let exp = state.now() as i64 + session::MEMBER_SESSION_DAYS * 86400;
    let json = serde_json::json!({
        "admin": session::make_admin_cookie(state),
        "member": session::sign_member(state, &member.discord_id, exp, &tag),
        "member_discord_id": member.discord_id,
        "admin_password": seed::ADMIN_PASSWORD,
        "member_key": member.token,
    });
    std::fs::write(
        data.join("cookies.json"),
        serde_json::to_string_pretty(&json)?,
    )?;
    Ok(())
}

fn print_banner(data: &Path, listener: &tokio::net::TcpListener, mock: &MockUpstream) {
    let gate = listener
        .local_addr()
        .map_or_else(|_| "?".into(), |a| format!("http://{a}"));
    let default_data = workspace_root().join("target/owl-dev");
    let e2e_cookies = if data == default_data {
        String::new()
    } else {
        format!("E2E_COOKIES={} ", data.join("cookies.json").display())
    };
    println!(
        "\n猫头鹰公益站 · 本地开发环境已就绪（合成数据，只监听本机）\n\
         \n  网关      {gate}   后台 {gate}/admin  密码 {pw}\
         \n  假上游    {mock}\
         \n  数据目录  {data}   （cookies.json 里有后台与成员会话）\
         \n\
         \n前端开发    cd web && VITE_GATE={gate} pnpm vite            # http://localhost:5173\
         \n端到端      cd web && VITE_GATE={gate} pnpm exec vite --port 5174 --strictPort &\
         \n            cd web && {e2e_cookies}E2E_BASE=http://localhost:5174 pnpm exec playwright test\
         \n\
         \nCtrl-C 退出（数据目录留着，下次启动会清掉重建）。\n",
        pw = seed::ADMIN_PASSWORD,
        mock = mock.url(),
        data = data.display(),
    );
}
