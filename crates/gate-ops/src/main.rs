//! `nai-ops`：猫头鹰公益站运维命令行。子命令见 `nai-ops --help`；各命令的行为说明在对应模块的文档里。
//!
//! 退出码：`patrol` 为 0 正常 / 1 注意 / 2 危险 / 3 巡检自己出错；其它命令 0 成功、非 0 失败
//! （参数错误 2，运行错误 1）。

use anyhow::Result;
use clap::{Parser, Subcommand};
use gate_ops::{patrol, report, sanitize, setting, smoke, snapshot, traffic, util};
use std::path::PathBuf;
use std::time::Duration;

#[derive(Parser)]
#[command(name = "nai-ops", version, about = "猫头鹰公益站运维命令")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// 只读体检：系统 / 上游容量 / 成员请求 / 数据一致性；退出码 0 正常 · 1 注意 · 2 危险 · 3 巡检自身出错
    Patrol {
        /// 有问题时给站长发 Discord 告警（同一问题 30 分钟只发一次）
        #[arg(long)]
        alert: bool,
        /// 输出 JSON
        #[arg(long)]
        json: bool,
        /// 只输出一行摘要
        #[arg(long)]
        oneline: bool,
        /// 数据库路径（默认 $NAI_GATE_DB，其次 $DATA_DIR/nai_gate.db）
        #[arg(long)]
        db: Option<PathBuf>,
    },
    /// 改一项后台设置（走后台接口，校验与操作日志同后台页面）；先看 `set --list`，没有 --yes 不写
    Set {
        /// 设置名（见 --list）
        name: Option<String>,
        /// 新值
        value: Option<String>,
        /// 列出所有可用设置名
        #[arg(long)]
        list: bool,
        /// 确认写入（不加则只显示 改前 → 改后 并退出 2）
        #[arg(long)]
        yes: bool,
        /// 只预览，不写
        #[arg(long)]
        dry_run: bool,
    },
    /// 读一项后台设置的当前值（名字同 set）
    Get { name: String },
    /// 改后台页面上没有的临时设置（V5 临时加量 / 账号日上限 / 站付 V5 / 当天 V5 方案），或只补记一笔；
    /// 网关在同一个事务里改设置并写「手动（Claude）」记录。先看 `manual --list`，没有 --yes 不写
    Manual {
        /// 设置名（见 --list）；只补记时不填
        name: Option<String>,
        /// 新值（JSON）；null = 删除这项
        value: Option<String>,
        /// 为什么改、谁拍板、什么时候收回（必填，写进操作日志）
        #[arg(long, default_value = "")]
        why: String,
        /// 只补记一笔时的操作名
        #[arg(long)]
        action: Option<String>,
        /// 只补记一笔时的对象
        #[arg(long)]
        target: Option<String>,
        /// 列出可改的名字
        #[arg(long)]
        list: bool,
        /// 确认写入
        #[arg(long)]
        yes: bool,
        /// 只预览，不写
        #[arg(long)]
        dry_run: bool,
    },
    /// 只读周报：每把 Key 的滥用 / 健康信号（只输出 Key 编号、显示名和计数，不输出提示词）
    Report {
        /// 统计最近几天
        #[arg(long, default_value_t = report::DEFAULT_DAYS)]
        days: i64,
        /// 内容风险词表（默认 $NAI_RISK_TERMS，其次 /opt/owl/risk-terms.txt；不存在就跳过这一节）
        #[arg(long)]
        terms: Option<PathBuf>,
        #[arg(long)]
        db: Option<PathBuf>,
    },
    /// 汇总 Caddy JSON 访问日志：路径分组、状态码、p50 / p95、每分钟峰值（不读 IP，不输出查询参数）
    Traffic {
        /// 日志文件或目录，可重复（默认 $CADDY_ACCESS_LOG，其次 /var/log/caddy/access.log）
        #[arg(long = "log")]
        logs: Vec<PathBuf>,
        /// 统计窗口，如 30m / 1h / 24h / 7d
        #[arg(long, default_value = "1h")]
        since: String,
        /// 窗口结束时刻（Unix 秒，默认现在；复盘历史用）
        #[arg(long)]
        until: Option<f64>,
    },
    /// 一致快照（不含审计原图）→ 完整性校验 → gzip（权限 600）
    Snapshot {
        /// 输出文件，如 /opt/backups/daily/nai_gate-2026-10-14.db.gz
        out: PathBuf,
        #[arg(long)]
        db: Option<PathBuf>,
    },
    /// 把快照原地脱敏（Key 换短哈希、删密钥类设置 / 提示词 / 缩略图 / 头像）后 VACUUM，给协作 AI 只读分析用
    RoSanitize {
        /// 快照数据库文件（.db，不是 .db.gz）
        snapshot: PathBuf,
    },
    /// 线上冒烟：只做不出图的检查；SMOKE_KEY 环境变量有值时多测一条 /v1/me
    Smoke {
        /// 站点根地址（也可用环境变量 SITE_URL）
        #[arg(env = "SITE_URL")]
        base: String,
        /// 单个请求的超时（秒）
        #[arg(long, default_value_t = 15)]
        timeout: u64,
    },
}

fn main() {
    let cli = Cli::parse();
    let is_patrol = matches!(cli.cmd, Cmd::Patrol { .. });
    if is_patrol {
        // 巡检自己 panic 也要退出码 3（systemd 才分得出「巡检坏了」与「有警告」）
        std::panic::set_hook(Box::new(|info| {
            eprintln!("[patrol] 内部错误（panic）：{info}");
            std::process::exit(3);
        }));
    }
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("错误：创建运行时失败：{e}");
            std::process::exit(if is_patrol { 3 } else { 1 });
        }
    };
    let code = match rt.block_on(dispatch(cli)) {
        Ok(code) => code,
        Err(e) => {
            if is_patrol {
                eprintln!("[patrol] 内部错误：{e:#}");
                3
            } else {
                eprintln!("错误：{e:#}");
                1
            }
        }
    };
    std::process::exit(code);
}

async fn dispatch(cli: Cli) -> Result<i32> {
    match cli.cmd {
        Cmd::Patrol {
            alert,
            json,
            oneline,
            db,
        } => {
            let cfg = patrol::Config::from_env(db.as_deref())?;
            patrol::run(
                &cfg,
                &patrol::Args {
                    alert,
                    json,
                    oneline,
                },
            )
            .await
        }
        Cmd::Set {
            name,
            value,
            list,
            yes,
            dry_run,
        } => {
            if list {
                print!("{}", setting::list_text());
                return Ok(0);
            }
            let (Some(name), Some(value)) = (name, value) else {
                eprintln!("用法：nai-ops set <名字> <值> [--dry-run | --yes]；nai-ops set --list");
                return Ok(2);
            };
            let (code, text) = setting::run_set(&name, &value, yes, dry_run).await?;
            print!("{text}");
            Ok(code)
        }
        Cmd::Get { name } => {
            let (code, text) = setting::run_get(&name).await?;
            print!("{text}");
            Ok(code)
        }
        Cmd::Manual {
            name,
            value,
            why,
            action,
            target,
            list,
            yes,
            dry_run,
        } => {
            if list {
                print!("{}", setting::manual_list_text());
                return Ok(0);
            }
            let body = match setting::manual_body(
                name.as_deref(),
                value.as_deref(),
                &why,
                action.as_deref(),
                target.as_deref(),
            ) {
                Ok(b) => b,
                Err(why) => {
                    eprintln!("{why}");
                    return Ok(2);
                }
            };
            let (code, text) = setting::run_manual(body, yes, dry_run).await?;
            print!("{text}");
            Ok(code)
        }
        Cmd::Report { days, terms, db } => {
            print!(
                "{}",
                report::run(&util::resolve_db(db.as_deref()), days, terms)?
            );
            Ok(0)
        }
        Cmd::Traffic { logs, since, until } => {
            let Some(span) = traffic::parse_span(&since) else {
                eprintln!("--since 需要像 30m / 1h / 24h / 7d 这样的写法");
                return Ok(2);
            };
            let logs = if logs.is_empty() {
                vec![PathBuf::from(
                    util::env_nonempty("CADDY_ACCESS_LOG")
                        .unwrap_or_else(|| traffic::DEFAULT_LOG.into()),
                )]
            } else {
                logs
            };
            print!(
                "{}",
                traffic::run(&logs, span, until.unwrap_or_else(util::now_secs))?
            );
            Ok(0)
        }
        Cmd::Snapshot { out, db } => {
            let line = snapshot::run(&util::resolve_db(db.as_deref()), &out)?;
            println!("{line}");
            Ok(0)
        }
        Cmd::RoSanitize { snapshot } => {
            let n = sanitize::sanitize(&snapshot)?;
            println!("sanitized {n} keys");
            Ok(0)
        }
        Cmd::Smoke { base, timeout } => {
            let key = std::env::var("SMOKE_KEY")
                .ok()
                .map(|k| k.trim().to_string())
                .filter(|k| !k.is_empty());
            let report = smoke::run(&base, key.as_deref(), Duration::from_secs(timeout)).await?;
            print!("{}", report.summary());
            Ok(report.exit_code())
        }
    }
}
