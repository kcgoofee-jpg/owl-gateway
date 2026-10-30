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
