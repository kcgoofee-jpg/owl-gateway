# 路线图

按「收益 ÷ 风险」排序，未排期的不做承诺；每项都写清楚为什么值得做、代价是什么。

## 1. 后台瘦身：把管理后台从全功能 SPA 砍成「总览 + 三个高频操作」+ CLI

**现状体量**（2026-10-30 实测）：

| 部分 | 规模 |
|---|---|
| 后台 Rust 路由 `crates/gate-server/src/routes/admin/**` | 4,159 行 |
| 后台前端 `web/src/pages/admin/**` | 8,960 行 + 独立构建 / CSP / e2e |
| `nai-ops` CLI（已存在，走同一套 admin API） | 已在用 |

**结论**：删页面 ≠ 删接口。4,159 行 Rust 是 Discorder 机器人、部署脚本、巡检、`nai-ops` 共用的接口层，删不掉；能省的只有前端的 8,960 行和它的构建链。

**做法**：

1. 保留全部 `/admin/api/*`（一行不改）；
2. 前端只留：趋势总览、维护开关、改额度、封/解 Key —— 目标 ≤ 1,500 行；
3. 其余能力（风险线索、日志多维筛选、成员详情、上游账号详情）全部下沉成 `nai-ops` 子命令，输出表格；
4. 删掉后台的 e2e 套件，改用 `nai-ops` 的 CLI 冒烟。

**收益**：前端代码 -80%，构建/测试链路简化，安全面变小（少一个可交互的登录面）。
**代价**：手机上看后台会变难（现在能在手机上处理事务）；需要补 6–8 个 `nai-ops` 子命令。
**风险**：低（接口不动，随时可以恢复页面）。

## 2. 工作台深水区逐项实测

已过：生成、Quality/UC 预设、Variations、Upscale、Enhance、历史三键、Upload to Explore、预设持久化。
还差（每项都要真实跑一次请求）：

- Director Tools 逐个（Line Art / Colorize / Declutter / Emotion）
- Character Prompts 加到 3+ 角色
- Reference Images 多图 + Strength / Information Extracted
- Inpaint 画掩码 → 执行
- 图生图 img2img 完整链路

## 3. 镜像体积与设置页

- `mirror/nai-app` 目前 59 MB，能否只保留 V4.5 / V5 用得到的 chunk 与字体（风险：按需加载的 chunk 被删会在某个入口 404）。
- 官网的账号设置页没有镜像（`/settings`、`/account` 在官网和镜像都 404，入口找不到），所以界面语言只能留在英文；若以后定位到真实路由，可一并镜像，顺带解锁 `zh-CN`。

## 4. 上游与容量（产品决定，不是工程项）

单账号 Opus，吞吐约 5.7 张/分钟；开放后 30 分钟出现过 `queue_full` 101 次。站长决定不补账号、PVP 排队，因此不做工程改动，只在需要时调 `runtime_queue_timeout`。

## 5. 观测

- 巡检脚本（子代理用的那套）沉淀成 `docs/monitoring-playbook.md`，可复用；
- 关注：`upstream 5xx`、`queue_full` 比例、`client_disconnected` 比例、日志轮转周期。

## 6. 排队反馈与客户端断连（2026-11-01 开项）

**问题**：11-01 全站断连率从 3.7%（10-31）跳到 16.2%，单个成员最高 37%。根因不是网络，是**排队 90–120 秒、而客户端在 60–120 秒超时**：队列满时请求进得去队但要等，官网镜像页在这段时间什么都不显示，成员以为卡死就刷新 —— 刷新即断开，前面白等。数字见 `docs/report-20261101-1620.md`。

### 已落地（不需再动，改动已持久）

| 项 | 值 | 持久性 |
|---|---|---|
| `runtime_queue_timeout` | 240 → **90 秒** | 存在 `site_settings` 表，重启/重部署不丢 |
| `guard_queue_per_member` | 2 → **3** | 同上 |
| 生图页排队反馈（计时条 + 429 人话） | `mirror/nai-app/bridge.js` | 宿主机只读挂载 + `Cache-Control: no-store`，刷新页面仍在 |

**为什么 90 秒**：客户端超时是 60 秒（浏览器）/ 120 秒（代理链路）。超时定在 240 秒时，站点让请求在队里挂到 4 分钟，客户端早走了，成员拿到的是「网络错误」不是「排队超时」；改成 90 秒后站点先于客户端放弃，给出明确 429 + `Retry-After`。

**为什么改 `bridge.js` 而不是重发镜像**：它是运行时可热更的静态资源，而且这一层只加显示、不动请求；权威源在 `deploy/nai-mirror-overrides/bridge.js`，`tools/sync-mirror.sh:18` 每次同步前覆盖过去。

### 待办

1. **发版** —— 一次带上三样：① `guard.rs` 的排队满文案去掉了张数（那是站点内部闸门数字，还随上游账号数变）；② `fix/queue-heartbeat` 的排队期 SSE 心跳；③ §6 的 V5 短作业车道（新参数 `guard_v5_priority` 默认 1）。都已在 main / 分支，**要重新构建镜像才进生产**。
2. **排队期 SSE 心跳（治本）** —— 已在 `fix/queue-heartbeat` 实现（2026-11-04，待发版）：`/ai/generate-image-stream` 排队超过 20 秒仍未派发时先发 `200 text/event-stream`，之后每 15 秒发一帧 SSE 注释保活；短错误仍在 20 秒内以 JSON 返回。发版后「长排队 + 短客户端超时」才真正和解：链路上一直有字节，层间的空闲超时不再动手。风险：中（流式路径是所有出图的必经之路），已补回归测试。
3. **告警投递通道** —— 磁盘 48%（10-30 是 33%）、V5 余量 11%（`v5_alert_threshold=20`）、`upstream 5xx`。现在只有日志，没有投递口；加 cron 只写 syslog 等于没告警。需要先定通道（Discord webhook / 邮件），再挂检查。
4. **429 后客户端退避** —— 成员会 1 秒内连点 5 次，撞上 `key_rate`（每 Key 每分钟 10 次）。站点侧已给 `Retry-After`（`MEMBER_QUEUE_FULL` 10s / `KEY_RATE` 30s / 其余 15s），生图页也已按它显示；但**按钮本身还不会在冷却期内禁用**，可以补。

> 吞吐本身（单上游 ~145 张/小时 vs 需求 ~275 张/小时）不在这里 —— 见第 4 节，站长决定不补账号。

### 11-07：V5 短作业车道（代码已在 main，待发版）

**数据**（`docs/report-20261107-v5-queue.md`，33 小时窗口 4,454 张 ok）：`dur_ms` 是纯上游往返，v5 中位 **3.9 s**、上界 9.7 s，v4.5 中位 8.9 s、p90 42 s、上界 52 s；控尺寸/步数/时段后 v5 仍快 **64.6%**（95% CI 60.3–68.5%，Wilcoxon p=5.7e-91）。同期两者的排队等待**没有差异**（p=0.526），v5 把自己 4 秒的活排进 30 秒的队里。

**机制**：`pace::Arrival` 带 [`Lane`](../crates/gate-core/src/lane.rs) → `fair_cmp` 比的是 **车道 → 老化 → 先来后到**。老化两档：等满半个超时（45 秒）只在车道内抢先，等满整个超时（90 秒）才压过 **V5** 车道（不能压过插队，见 §7）。**非抢占**：正在生成的那张不打断。同一把 Key 内部（`key_turn`）按同一套车道顺序。

**11-07 同日删除**：「这个成员今天出得最少的人先」（10-22 从 AI Horde 借来的公平层）。它让重度用户沉底、轻度用户又被后来的轻度用户插队，所有人一起多等；防饿死靠老化就够了。删除波及 `Observer::share`、`ReqCtx::set_share`、准入时对 `counters.images` 的那次读、`fair_cmp` 的排序键，以及两处模块注释和运维手册。**`counters.images` 本身保留**（统计用）。

**代价（模拟，实测服务时间，ρ=0.90，公共随机数）**：v5 平均等待 −89.6%（149.9→15.6 s），v4.5 +1.9%（149.0→151.8 s）；理论放大上界 `1/(1-ρ_v5)=1.019`，因为 v5 只占忙时 1.8%。

| 落点 | 内容 |
|---|---|
| `crates/gate-core/src/lane.rs` | `Lane`：插队 / V5 / 普通 |
| `crates/gate-upstream/src/pace.rs` | `Arrival.lane` + `fair_cmp` 排序键 |
| `crates/gate-upstream/src/dispatch.rs`、`dispatch/v5.rs` | 流式与非流式都带车道 |
| `crates/gate-core/src/guard.rs` | 参数 `guard_v5_priority`（0/1，默认 **1**）；`Entry.lane` |
| `crates/gate-server/src/routes/image/{mod,settle,stream,tools}.rs` | `lane()` → `Lane::of(queue_jump, v5)` |
| `crates/gate-server/src/live.rs` | `/public/live` 的 `mode` 报 `v5-priority` 或 `fifo` |

回滚：`nai-ops set v5_priority 0`（热更新，无需发版）。验收：发版后 24 小时 v5 的 `wait_ms` 中位降 ≥50%、v4.5 升 ≤5%；`queue_timeout` 与断连率不上升。

## 7. 赞助席：指定成员插队、捐赠 Key、独立入口（已落地）

站长在成员详情指定赞助身份。赞助者不进 `/admin`，另走 `/patron`（页面中文名「赞助席」）。两种赞助可以叠在同一个人身上，也可以分开给：

- **金钱赞助**：仍走全站共用上游账号；插队；在自己的页面打开自己成员 Key 的全部功能开关。看不到网关其它部分。
- **账号赞助**：绑定其捐赠的 NovelAI Token；出图只走这把 Token（不和站账号混用）；网关不再按免费档裁掉这把账号本来就能做的事。

现有 `owner_image_full`（环境变量绑定的站长号）保持不动，插队和功能放行与赞助席共用同一套判断，避免第三套特例。

**插队**（11-12）：`Lane::Jump` = 站长号或赞助席 `queue_jump`；V5 是 `Lane::Fast`；普通是 `Lane::Normal`。`skip_site_cap` 仍是 `queue_jump` 的另一面（全站排满仍能入队）。同一个账号空出来时插队排在所有等待中的 V5 / V4.5 前面；每人排队上限和 RPM 不提高；不抢占已经发往上游的那张；多名赞助者在插队车道里按先来后到。等满 90 秒的普通 / V5 可以压过 V5 车道，不能压过插队。准入、派发、`key_turn`、首页位次共用同一个 `Lane`。

**专属 Token**：不进入普通选号池，也不计入全站 `queue_cap`。该成员出图只派到这把 Token；冷却 / 401 / 见底时用明确 429/503，不悄悄改走站账号。走专属 Token 或已打开 `paid_parameters` 时不再套免费档裁剪。专属 Token 的 Anlas 花在捐赠账号上，不扣全站池、不受全站 V5 暂停。金钱赞助开挂费开关仍花全站 Anlas / V5 池。

**入口**：`/patron` 独立密码、独立 Cookie（`nai_gate_patron`，`Path=/patron`）。登录限速与后台同一套（每 IP 5 次 / 300 秒）。接口只暴露登录、退出、欢迎与开关、自己改密。赞助 Cookie 打到 `/admin/api` 为 401。

> 与 §6 的 V5 车道区分：V5 车道是**模型维度**；这一节是**成员维度**。插队是单独一条更高的车道（站长 / 赞助），V5 仍只压过普通请求。

## 8. 单上游账号更高并发评估（未排期）

当前每账号并发保持 1。评估更高在途请求上限时，同时验证派发间隔、并发锁、冷却与逐级恢复，区分调度器配置上限和上游实际可用容量；单独验证吞吐及错误率后再决定是否启用。

## 9. ST-iPhonie 首页教程（未排期）

参考 [ST-iPhonie](https://github.com/hey8v0/ST-iPhonie)，后续验证接入流程后编写首页使用指南。当前仅记录需求，不增加教程正文或首页入口。

## 10. 牌局视觉参考 yusheng9511（未排期）

`/poker` 现在跑的是 [lhz960904/texas-holdem](https://github.com/lhz960904/texas-holdem)，功能齐全（房间码、游客 SSO、结算），但界面只算中上。

[挂起] 另一个开源项目 [yusheng9511/texas_holdem](https://github.com/yusheng9511/texas_holdem)（Vue 3 + Go，1★）的牌桌观感明显更好：深绿椭圆桌 + 棕木边、金色圆座位、座位下挂一排筹码圆片、中央 Pot / Side Pot 分栏、右侧动作历史。

先记录，不动手。真要做的路径是把它的视觉搬进现有 `PokerTable.tsx`（只改 class 与 `index.css`，逻辑一行不动），预期 1~2 天。整套换过去不划算：它只有单机 AI 对战，没有房间码和成员身份，后端等于重写。
