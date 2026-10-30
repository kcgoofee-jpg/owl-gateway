# 后端整理计划（2026-10-31 修订版）

> 旧的七步清单（数据库写队列 / 结算 / 准入预算 / 图片策略 / Discord 注册 / 上游派发 / 后台成员操作）
> 是对着一版更早的代码写的。这份按**当前**仓库（`crates/` 共 49k 行后端代码）重新排过：
> 每步都写清楚「现在长什么样、要拆成什么、怎么算做完」。

## 0. 现在的样子

| crate | 行数 | 职责 |
|---|---|---|
| `gate-server` | 22.5k | HTTP 路由、准入、记账、任务调度、注册服务、后台接口 |
| `gate-core` | 9.9k | 策略 / 校验 / 计费 / 滥用规则（纯函数，无 IO） |
| `gate-db` | 6.2k | 唯一写 SQL 的地方：写线程 + 只读连接池 + `repo/*` |
| `gate-upstream` | 3.9k | 上游客户端、令牌池、派发、冷却 |
| `gate-ops` | 2.0k | 运维命令行（`nai-ops`） |
| `gate-discord` | 1.8k | Discord REST / 机器人 |
| `gate-snapshot` | 1.2k | 快照测试框架 |

最大的几个文件（这次要动的地方）：

```
1973 crates/gate-core/src/policy.rs
1416 crates/gate-server/src/registration.rs
1176 crates/gate-upstream/src/dispatch.rs
1009 crates/gate-db/src/repo/usage.rs
 977 crates/gate-server/src/routes/admin/members.rs
 649 crates/gate-server/src/admission.rs
```

## 1. 执行原则（每一步都适用）

1. **只搬不放**：拆分过程中不改行为，不改 SQL 语义，不改对外 JSON。
2. 一步一个提交，`cargo test --workspace` + `cargo clippy --all-targets` 全绿才继续；
   快照测试（`crates/gate-db/tests/snapshots/`、`gate-snapshot`）必须**逐字节不变**。
3. 机械验收：动过的 crate 里**没有单个文件超过 700 行**（`xtask` 里加一个检查，CI 跑）。
4. 每一步都可以单独上线（`tools/deploy.sh --wait`），不需要攒着一起发。

## 2. 七步（重新排序、标注现状）

### ① 数据库写队列 —— 已完成并上线（2026-10-31，72de2c0）

**原来的问题**：`crates/gate-db/src/lib.rs` 里写线程和事务模型都是对的（单写线程、`BEGIN IMMEDIATE`、
busy 重试、panic 后重开连接），但 `Db::write()` 在 async 函数里直接 `SyncSender::send`：
队列上限 1024，一满就把 **tokio worker 线程卡死**，同一时刻的 `/healthz`、SSE 心跳、别的请求一起停。

**改法（已落地）**：加一个许可数 = 队列长度的 `tokio::sync::Semaphore`。
`write()` 先 `acquire_owned().await`（满了就在异步上排队，不占线程），拿到许可再 `try_send`，
许可随任务进闭包、事务结束才归还——队列因此永远不会满，阻塞发送路径消失。

**验收**：`crates/gate-db/tests/write_queue.rs`，单 worker 运行时 + 长度 1 的队列，
慢写任务占住时心跳任务仍按时完成（以前这条会超时）。

### ② 结算与用量日志 —— 已完成并上线（cf047b6）

**现状**：`repo/usage.rs`（1009 行）同时管三件事：写 `usage_log`、维护 `request_settlements` 状态机
（预留 → 落账 / 释放 / 恢复）、以及各种统计查询（`LogSummary`、按 Key / 时间聚合）。

**目标**：
- 新建 `repo/settlement.rs`：`request_settlements` 的全部状态转换、启动恢复、人工释放，一个入口一个函数。
- `repo/usage.rs` 只留「记一条用量 + 查询」，统计聚合搬去 `repo/usage_stats.rs`。
- 路由和任务只调这两个模块的公开函数，不再自己拼 UPDATE 语句。

**验收**：`grep -rn "request_settlements" crates/ --include=*.rs` 只出现在 `repo/settlement.rs` 和迁移文件里；
`repo/usage.rs` < 400 行。

### ③ 准入与预算 —— 已完成并上线（4a6c791）

**现状**：`admission.rs`（649 行）把排队、预算预留、派发确认、配额检查塞在一个文件里，
`state.budget.lock()` 在路由里也直接出现。

**目标**：
- `admission/{queue.rs, budget.rs, quota.rs}` 三块：排队与超时、预留与结算确认、每人/每站额度检查。
- 路由层不再持有 `state.budget` 的锁（只在 `budget.rs` 里锁），派发确认也走 `budget` 的 API。
- `Est` / `Reserved` / `Slots` 这些 RAII 类型的 Drop 语义写进注释并各配一条测试。

**验收**：`grep -rn "budget\.lock()" crates/gate-server/src/routes/` 为空；`admission.rs` 只剩 re-export。

### ④ 图片策略 —— 已完成并上线（64a2ee2）

**现状**：`gate-core/src/policy.rs` 1973 行，混着参数规范化、参考图与 Vibe 校验、提示词处理、
模型选择与费用规则。

**目标**：`policy/{normalize.rs, references.rs, prompt.rs, model.rs, pricing.rs}`，
`policy.rs` 只留 `pub use` 和模块文档。纯函数不变，测试跟着函数搬。

**验收**：每个新文件 < 500 行；`policy.rs` 的公开 API 名字不变（外部只改 `use`）。

### ⑤ Discord 注册 —— 已完成并上线（b2e4510）

**现状**：`registration.rs` 1416 行，包含 Discord REST 客户端调用、注册流程（领 Key / 邀请 / 名额）、
资料与身份组的同步、欢迎语拼装。

**目标**：
- `registration/{client.rs, service.rs, profile.rs, message.rs}`：外部 API 客户端 / 注册流程 / 资料与身份组同步 / 文案。
- 对外只留一个 `RegistrationService` 入口，路由层拿到的错误类型不变。

**验收**：`RegistrationService` 的公开方法数 ≤ 12；`client.rs` 是唯一直接打 Discord REST 的地方。

### ⑥ 上游派发 —— 已完成并上线（380c999）

**现状**：`gate-upstream/src/dispatch.rs` 1176 行，普通请求与流式请求各写一遍「准备 + 派发 + 收尾」，
429 / 403 / 402 的处理分散。

**目标**：抽出 `prepare()` / `send()` / `finish()` 骨架，普通与流式各自只实现响应处理（`Hooks` 已有类似形状）；
错误状态码处理收敛成一个 `on_status()` 分派表。

**验收**：`dispatch.rs` < 500 行；新增一条测试：同一份 `Hooks` 下，普通与流式在 429 / 403 上的
冷却与隔离行为一致。

### ⑦ 后台成员操作 —— 已完成并上线（8a2d728）

**现状**：`routes/admin/members.rs` 977 行，路由里直接做业务流程（建邀请 Key、重置额度、发券、
打标签、封禁），`admin_members.rs` 只是薄薄一层 SQL。

**目标**：`admin/member_ops.rs` 服务层承接业务流程（含操作日志与告警），路由只做鉴权 / 输入校验 / 响应。
和 ②③ 的服务层复用同一套函数。

**验收**：每个路由函数 ≤ 30 行；`routes/admin/members.rs` < 300 行。

## 3. 顺序与依赖

```
① 写队列（已完成）
② 结算 → ③ 准入与预算（依赖 ② 的结算 API）
④ 图片策略 ┐
⑤ Discord 注册 ├ 三者互不依赖，可以并行
⑥ 上游派发 ┘
⑦ 后台成员操作（最后，复用 ②③ 的服务层）
```

## 3.1 进度（2026-10-31 03:25）

| 步 | 结果 | 行数 |
|---|---|---|
| ① | 已上线：`Db::write` 改信号量背压，`tests/write_queue.rs` 守住 | — |
| ② | 已上线：`repo/usage.rs` 1009 → 406，拆出 `settlement.rs`(371) / `usage_stats.rs`(256) | — |
| ③ | 已上线：`admission.rs` 649 → 22（`queue` 209 / `quota` 168 / `budget` 284），路由不再锁 `state.budget` | — |
| ④ | 已上线：`policy.rs` 1982 → 124（`params` 376 / `references` 324 / `prompts` 529 / `pricing` 220 / `probe` 436） | — |
| ⑥ | 已上线：`dispatch.rs` 1176 → 446（`outcome.rs` 317 结算守卫+状态码表 / `stream.rs` 54 / `v5.rs` 398 V5 计费确认） | — |
| ⑦ | 已上线：`routes/admin/members.rs` 977 → **317**，`member_ops.rs` 349（重置额度 / 发券 / 删 Key+封禁 / 标签 / 解暂停 / 列表 / 邀请 Key / 换 Token / 改 Key / 黑名单），`members/view.rs` 442（成员页视图模型） | — |
| ⑤ | 已上线：`registration.rs` 1416 → 602（`transport` 55 / `roles` 213 / `profiles` 62 / `web_login` 134 / `issue` 382），公开方法签名不变 | — |

七步全部完成，所有动过的文件都在 700 行以内（最大的 `view.rs` 442 行）。

顺手修的两个线上问题：
- **部署被判失败自动回滚**（10-31 04:37）：九个定时任务原本在启动瞬间同时跑第一轮，`abuse_scan` 读十几万行、
  `maintenance` / `idle_recycle` 写库，把 `/healthz` 的 1 秒读写预算挤爆 → 503 → cutover 回滚。现在第一轮按 1.2 秒错开。
- `gate-discord/src/kb.rs` 的 `manual_div_ceil` 在 CI 里是 error（本地只是 warning），一直在让 CI 失败、卡住部署闸门，
  已改成 `div_ceil(5)`。

## 4. 每次落地的固定动作

1. 本地 `cargo test --workspace`、`cargo clippy --all-targets`；
2. `git commit`（英文 ASCII 提交信息），push，等 CI 绿；
3. `tools/deploy.sh --wait`（会跑 smoke + 维护页检查）；
4. 部署后 5 分钟看 `job_last`（9 个定时任务）与错误日志，无 `no such table/column`、无 5xx 才算完成。
