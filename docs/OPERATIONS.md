# 运维手册（3.0）

服务器上有两套目录：

| 目录 | 内容 |
|---|---|
| `/opt/service-tools` | 旧检出（**不再更新**）：`.env`、`data/`（库、`secret_key`、上游 Token 存档）、旧版 cron 脚本。3.0 使用这里的 `.env` 和 `data/`。 |
| `/opt/owl` | 3.0：`compose.yml`（= 仓库 `deploy/v3/compose.yml`）、`cutover.sh`（= `deploy/v3/cutover.sh`）、宿主机用的运维命令 `nai-ops`、定时脚本 `ops/`（= 仓库 `deploy/v3/ops/`）。 |
| `/opt/caddy` | `Caddyfile`（= 仓库 `deploy/Caddyfile.production`）。 |

3.0 只有一个镜像 `owl-gate`，两个进程：`owl-gate`（网关，`nai-gate serve`）、`owl-naimei`（奶妹，`nai-bots naimei`）。
燕双鹰已删除（它什么都不做）。2.x 已下线，没有回滚到 2.x 的通道（站长 10-22 关闭了回滚窗口）。

## Release

See [WORKFLOW.md](WORKFLOW.md). In short: push to `main`, then `tools/deploy.sh --wait`.
The server never compiles (2 cores) and holds no GitHub credentials: the script downloads the CI artifacts
locally, copies them over with `scp`, syncs `cutover.sh` / `compose.yml`, and runs `cutover.sh load`
(new image becomes `owl-gate:staged`; `current` and rollback tag `previous` stay untouched), waits for
an idle queue, promotes the candidate while retaining the old `current` as `previous`, restarts, then runs
healthcheck and smoke. If waiting times out, the current container and rollback tag are unchanged; retry the
same run after traffic subsides.

`restart` 是先停后起：旧网关收到 SIGTERM 立即关掉监听，在途和已排队的图出完（最多 120 秒）才退出，然后起新的，同一时刻只有一个网关在用账号。
这期间网站断 1–2 分钟，Caddy 显示降级页（「网关自检中」）；脚本每 0.5 秒经本机 Caddy 打一次公网地址的 `/healthz` 和 `/`，最后按**实测**打印停站时长。
不做不停站发布：交接期间两个网关各自不知道对方在用哪个账号，会让同一账号同时出两张而被上游回 429。
`restart` 的 `up -d --remove-orphans` 会顺手删掉 compose 里已经没有的容器（比如已删除的 `owl-warden`）；手动 `docker rm -f owl-warden` 也行。
网关收到停止信号后：停止接新请求 → 进行中的出图最多等 110 秒收尾记账 → 落盘状态码统计 → WAL checkpoint。
compose 的 `stop_grace_period` 是 120 秒，所以发布时进行中的图不会丢账。

**改 Caddy 配置一律用 `deploy/v3/caddy-reload.sh`**：Caddy 容器是单文件只读挂载，宿主机上的文件被换了 inode（编辑器保存、`sed -i`、写临时文件再 `mv` 都会）后，容器里看到的还是旧文件，`caddy reload` 读进去的也是旧的，而且不报错。
脚本先把当前运行配置存成 `/opt/caddy/running-<时间>.json`（回滚用），用临时容器校验宿主机上的文件，再经管理接口 `/load` 直接把文件内容交给 Caddy（不经过挂载），最后核对运行配置和文件一致。Caddy 以后重启时会重新挂载，读到的也是这份。

```bash
/opt/owl/caddy-reload.sh            # 默认 /opt/caddy/Caddyfile
```

仓库里的配置文件**不会自动同步到服务器**：改了 `deploy/Caddyfile.production`（或 `compose.yml`、`ops/` 里的脚本）之后，要手动拷到服务器对应位置
（Caddyfile → `/opt/caddy/Caddyfile`，然后 `/opt/owl/caddy-reload.sh`，**不要** `docker exec caddy caddy reload`：见上面「改 Caddy 配置」）。
`deploy/Caddyfile.production` 是模板，线上文件里还有 Cloudflare 信任段等本地改动，改线上时只做同样的增量；`deploy/Caddyfile.production` 里的 `MIRROR_BUCKET` / `REGION` 要换成线上状态页镜像的桶地址。

**页面降级**（10-18）：首页 / 服务条款 / 隐私政策在网关不可用时（Caddy 连不上网关，或网关回 502/503/504），2 秒内显示静态降级页（503 + `Retry-After`，附独立状态页镜像链接）；`/status` 同样降级但保持 200。
出图和用户接口**不降级**，仍最长等 150 秒让重启中的网关接上、拿到真实错误码。镜像只显示状态，不能出图，也不会连 NovelAI（不会换出口 IP）。

10-27 起不再等队列：`restart` / `previous` 立即停机换版，停机 1–2 分钟由 Caddy 的维护页兜住（在途和已排队的图在旧进程退出前最多收尾 120 秒）。`load` 只将候选镜像打上 `staged` 标签，不改正在运行的版本或回滚标签。

## 回上一版

| 情况 | 命令 | 说明 |
|---|---|---|
| 3.0 新版本有问题 | `cutover.sh previous` | 回到上一版 3.0 镜像；坏的那版标成 `owl-gate:failed` |

## 流量（`nai-ops traffic`）

汇总 Caddy 的 JSON 访问日志：按路径分组（`image-gen` / `gate-api` / `admin-api` / `admin-ui` / `public` / `bridge` / `static` / `member` / `other`）的请求数、状态码分类、p50 / p95 耗时、每分钟峰值。只解析时间、状态码、耗时、请求路径，**不读 IP、请求头**，路径在 `?` 处截断，输出里没有查询参数。

```text
$ nai-ops traffic --log /var/log/caddy --since 1h        # 目录 = 里面所有 .log 和 .log.gz
=== 流量：近 1 小时（09-21 21:13 ~ 09-21 22:13）· 共 6 个请求 · 峰值 4 次/分钟（09-21 21:57） ===
分组              请求    2xx   3xx    4xx   5xx     其它    p50ms    p95ms     峰值/分钟
image-gen        2      2     0      0     0      0     2000     4000         2
...
```

日志文件默认 `/var/log/caddy/access.log`（`CADDY_ACCESS_LOG`）。Caddy 一侧要开访问日志：`deploy/Caddyfile.access-log.example` 是一段 `log { ... }`（JSON、写文件、`roll_size` / `roll_keep` 滚动，并用 `filter` 编码器删掉 IP、请求头、响应头、TLS 细节）。**由站长 / 运维手工应用，仓库没有改线上 Caddyfile**：把这段放进站点块，给 caddy 容器挂 `/var/log/caddy`，先 `caddy validate` 再 `caddy reload`。其中去掉 uri 查询参数的 `regexp` 行没有在线上验证过，所以默认注释着——不开也不影响，`traffic` 自己会截掉。

## 备份与快照

快照由宿主机的 `nai-ops` 做（`/usr/local/bin/naigate-snapshot` 是共用入口），不经过网关容器，不锁库。恢复见 `deploy/ops/migrate_restore.sh`。

| 时间 | 脚本 | 内容 |
|---|---|---|
| 每晚 03:30 | `naigate-backup`（`/etc/cron.d/naigate-backup`，`deploy/harden.sh` 装的） | 本机 GFS 轮换，快照先过 `naigate-strip-prompts`（清掉整张 `generation_audit`，不含提示词）：每日 14 份、每周日 8 份、每月 1 号 6 份，放在 `/opt/backups/{daily,weekly,monthly}/nai_gate-*.db.gz` |
| 每晚 04:30 | `nightly_backup.sh` | 一致快照（不含原图，同样先过 `naigate-strip-prompts`）+ `.env` / `secret_key` / 上游 Token 存档 / 3.0 部署文件 / Caddy 配置，客户端 AES-256 加密后推 S3（脚本注释写 60 天过期，桶的生命周期配置未核对）；口令在服务器 `/root/.backup_pass`，务必在服务器之外另存一份 |
| 每天 05:10 | `ro-snapshot.sh` | 脱敏只读快照 `/opt/backups/ro/owl-ro-YYYYMMDD.db.gz`（保留 7 份；去掉 Key 原文、盐、提示词、捐赠的上游 Token，来源 IP 打成前两段），本机用 `deploy/ops/pull-ro-snapshot.sh` 拉取 |
| 每 5 分钟 | `status_mirror.sh` | 状态页（HTML + 外置的样式 / 脚本）静态镜像推 S3，主站挂了镜像仍在；页面快照时间停了超过 10 分钟 = 主站可能故障 |

后三条是 root 的 crontab，原文在 `deploy/v3/ops/crontab`。本机还可以用 `deploy/ops/pull-backup-mac.sh`（每天 04:30，校验后保留 30 份）把加密备份多拉一份（异地第三份）。

## 改密钥

3.0 和 2.x 共用 `/opt/service-tools/.env`（`NAI_TOKENS`、`DISCORD_BOT_TOKEN`、`REGISTRATION_BRIDGE_SECRET`、`ADMIN_PASSWORD` 等都在这里）。
改完要让 3.0 的三个容器重新读取：

```bash
nano /opt/service-tools/.env      # 或其它编辑器；保持 600 权限，值里不要有 $、引号、空格
cd /opt/owl && docker compose -p owl --env-file /opt/service-tools/.env -f compose.yml up -d
/opt/owl/cutover.sh status
```

`up -d` 只重建配置变了的容器；网关重启时进行中的出图照常收尾记账（见「发布」）。
**不要**在 `/opt/service-tools` 下执行 `docker compose up`：那里是 2.x 的检出，会把 2.x 的容器拉起来。
`ADMIN_PASSWORD` 只作初始 / 恢复用：后台改过密码的话以库里保存的为准。

## 配置：只有两层

- **环境变量**（`/opt/service-tools/.env`）：只放密钥和基础设施（上游 Token、Discord Token / ID、站点地址、数据目录、网络……），清单见仓库 `.env.example`；改完怎么生效见上面「改密钥」。
- **后台设置**（`site_settings`，在后台页面里改，不用重启）：额度、名额、新成员默认值、有效期、记录开关、账号保护、排队与冷却、Anlas 预算……没设过的项用代码里的默认值，逐项说明见 `deploy/ops/SYSTEM-FACTS.md`。「额度与保护」页管全部手动额度（V4.5 / V5 每人每天、V5 全站与暂停、Anlas）、账号保护与排队、出图节奏（每张出完后歇 N～M 秒）与冷却，外加只读的「记录」；10-23 起没有自动额度算法。「领 Key」页管开放、名额、新成员有效期 / 功能 / 模型范围、限定身份组、生成记录开关。后台手动新建 Key 和一键邀请 Key 读的是「领 Key」页同一份「新成员默认」。
- **没有第三层**：2.x 的私密参数文件 `data/private_params.json` 3.0 不再读（文件如果还留在原处就不用管，备份脚本已不再带它）。
- **代码常量**：时区 Asia/Shanghai、服务日 07:00 重置、标签补全并发 1、无效 Key 防爆破和后台登录限流的几个数、新 Key 的每分钟请求数等，没有后台入口也没有环境变量，要改只能改代码（`gate-core/src/clock.rs`、`gate-server/src/env.rs`、`gate-server/src/defaults.rs`）。

线上 `.env` 里残留的 `DEFAULT_*`、`REGISTER_*`、`AUDIT_*`、`DISCORD_ROLE_ID`、`SEED_DEMO_KEY`、`TZ`、`DAY_RESET_HOUR`、`GLOBAL_CONCURRENCY`、`GLOBAL_MONTHLY_ANLAS`、`GLOBAL_DAILY_ANLAS`、`QUEUE_TIMEOUT`、`IMAGE_MIN_INTERVAL`、`IMAGE_429_COOLDOWN_SECONDS`、`AUTH_FAIL_*`、`LOGIN_*`、`ALLOW_IMG2IMG`、`SAFE_CLAMP`、`MAX_PIXELS`、`MAX_STEPS`、`ALERT_CHANNEL_ID`、`ALERT_WEBHOOK_URL` 3.0 不再读，可以删掉。删之前先确认后台设置里已经有你要的值——比如名额 `register_max_users`、新领 Key 的有效天数 `register_expires_days`：库里没有这一行时，3.0 用代码默认（10 / 不过期），不再回落到 `.env`。

## 常用命令

```bash
docker logs --since 1h owl-gate                     # 网关日志（不含 Key、提示词、IP）
docker logs --since 1h owl-naimei                   # 奶妹日志（出错会带 [bug] 前缀）
/opt/owl/nai-ops snapshot <目标.db.gz>               # 一致快照（备份脚本用）
/opt/owl/nai-ops get queue_timeout                 # 读后台设置；改用 set <名字> <值> --yes
/opt/owl/nai-ops traffic --since 1h                # Caddy 访问日志汇总
docker exec owl-gate nai-ops smoke http://127.0.0.1:8000   # 线上冒烟（不出图）；地址必填，也可设环境变量 SITE_URL
curl -s http://127.0.0.1:3003/healthz
```

状态页事件没有后台入口，需要时直接写库（`incidents` / `incident_updates` 表，公开 `/status` 和 `/status.json` 照旧读）；`nai-ops` 里没有 `incident`、`announce`、`dm-broadcast`、`prompt-stats`（已删除）。

## 安全红线

- 仓库是公开的：不提交 `.env`、数据库、快照、成员数据；夹具和测试只用合成数据（CI 有 gitleaks 和 `check-fixtures.sh`）；文档里也不写真实 Discord ID、App ID、IP、域名。
- **不私信成员**：机器人没有任何私信出口，不群发、不批量发消息；给站长的告警只在重大事故时发（网关 `alerts.rs`：上游 Token 失效 / 连续 403 / Token 被改动 / 磁盘不足；部署完成通知走 `tools/deploy.sh` 的可选 webhook）。宿主机巡检 10-27 删除，站点可达性由 GitHub 的「站点探测」workflow 每 5 分钟外测。
- 上游请求的指纹 / 节奏改动要站长点头。

### 平台 / 第三方依赖中断预案（通用）

2026-10-10 Discord 把奶妹应用判为 Application Growth Disabled（错误码 680009），冻结了一切新的 OAuth 授权，
新用户领 Key 的两条路（网页「用 Discord 登录」和 `/register`）都走 OAuth，所以约 7.5 小时没有新增，老用户的 Key 和出图不受影响。
三个触发信号：网页登录刚上线就在一小时内有 ~43 人授权一个新应用并申请了敏感权限 `guilds.members.read`；机器人给每个新用户私信发 Key；
画廊频道里机器人自动点赞 + AI 评论。整改是：OAuth 只要 `identify`；`/register` 不走 OAuth，从斜杠命令的 interaction 直接发 Key，只在本人可见的临时消息里给；
不私信；关掉所有自动互动。处置记录之外，留下下面这些通用原则：

1. **核心功能（领 Key、出图）不要单点依赖任何可被第三方一键掐断的授权链路。** 斜杠命令的 interaction 本身就证明了身份，能不发起新授权就不发起。
2. **任何自动化群发 / 群互动默认关、能一键关。** 私信、自动点赞、自动评论、@全体……出事要能在 1 分钟内停。（现在的做法更彻底：这些出口都删掉了。）
3. **最小权限**：OAuth scope、Bot 权限、Intent 只要真正用到的（奶妹现在只有 GUILDS）；能用 Bot Token 核验的就别用用户授权。
4. **双通路**：面向用户的关键动作尽量有「不依赖该平台某项特性」的备用路。
5. **出事响应顺序**：先停掉可疑自动化（止血）→ 查根因（后台操作日志里的失败记录、容器日志）→ 上线绕开故障的修复，优先恢复核心 →
   向该平台正式申诉 → 发公告安抚用户。
6. **申诉要点**：小型非商业自用、成员自愿、已关闭所有自动化、已移除群发私信、权限收到最小、带 App ID 和错误码；一次性提交，别重复。
7. 申诉通过之前保持自动化全关；通过之后即使要恢复某项互动，也要限频 + 限总量 + 一键可关。
