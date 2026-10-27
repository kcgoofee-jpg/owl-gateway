# 猫头鹰公益站 · 系统事实清单（唯一来源）

> **更新日期** 2026-10-15 · **版本** 3.0 · **规则**：额度等事实以本文件为准，备份以 `docs/OPERATIONS.md` 为准；数字以代码为准，改了代码先改这里，再同步首页、后台说明和其他文档。
>
> 来源标记：**码** = 代码里的常量或默认值（文件都在 `crates/` 下）；**线** = 线上 2026-10-15 的只读快照（站点设置 `site_settings` 与环境变量，不含密钥和成员数据）；**算** = 由前两者算出来的数。
> 表里「线上」一列会随后台调整变化，「代码默认」是重装后得到的值。线上一栏写「未核对」= 快照里没有这个变量或键，按代码默认理解，没有第二来源。
> 配置只有两层：**环境变量**（`.env`，只放密钥和基础设施）和**后台设置**（`site_settings`，后台可改；没设过时用代码里的常量）。运行时可调的参数没有环境变量；私密参数文件 `data/private_params.json` 已取消、程序不再读（回滚到 2.x 时旧版仍会读它，所以备份脚本还带着它）。

## 0. 一句话

站长的 NovelAI Opus 账号被所有成员共用：网关给每人发一把虚拟 Key，所有出图都经过上游账号。
额度是 **PVP**：全站共用的固定池子，先到先得；每人上限只是上限，不是承诺。服务日每天 07:00（Asia/Shanghai）切换。

- 线上现有活跃成员 Key 170 把、Discord 领取记录 164 条（线，两个数口径不同，未做第二来源核对）；名额不限（线；后台设置 `register_max_users=0`，10-15 核对过库里有这一行。环境变量 `REGISTER_MAX_USERS` 已删除）。
- 上游账号数：代码按令牌池大小计（`每日上限 × 账号数`）；线上快照没有直接给出账号数（V5 采样只见 1 个 Token），未核对。
- 定时任务（§4）：额度同步、上游 V5 快照、全站熔断、单 Key 限流每 10 分钟，维护每 5 分钟，都带 ±20% 抖动（下限 30 秒）；闲置回收每小时。（宿主机巡检定时器 10-27 删除。）

## 1. 账号保护（`guard_*` 与运行期限制）

| 参数（设置键 / 环境变量） | 代码默认 | 线上 | 说明 |
|---|---|---|---|
| 出图并发（每个上游账号） | 固定 1 | — | NovelAI 一个账号同时只出 1 张，多了只会撞并发锁；后台的并发设置 10-23 删除（`upstream_token_image_concurrency` 的旧行不再读） |
| 每张出完后歇（最少）`guard_post_jitter_min` | 1（1～30，手动） | 1 | **出图节奏唯一的旋钮**（10-23）：出图结束（成功或失败，只要发到了上游）后，这个账号的下一张最早 = 结束时刻 + 歇的秒数，秒数在最少～最多之间均匀随机。下限 1 秒同时是「上游并发锁比响应晚一点释放」要的那一下（原来写死的 `pace::SETTLE_GAP` 删除）。开始到开始的账号出图间隔 `runtime_image_min_interval` 和间隔抖动 `guard_interval_jitter` 10-23 删除（并发 1 时出图本身约 7.6 秒，间隔从来不起作用），库里的旧行不再读 |
| 每张出完后歇（最多）`guard_post_jitter_max` | 3（1～30，手动） | 3 | 小于最少时按最少。一个账号的平均周期 = 实测出图耗时 + (最少 + 最多) ÷ 2 |
| 每日上限 `guard_account_daily_cap` | 2400（0～20000，手动） | 未核对 | 每个上游账号每个服务日的出图总量（V4.5 + V5 合计）；用完等次日 07:00。10-23 起只有这个手动值（加上 `nai-ops manual` 的临时值 `guard_token_daily_caps`）；按最近 7 天自动定的 `quota_account_caps` / `quota_account_cap_auto` 删除 |
| 每人排队 `guard_queue_per_member` | 2（1～5，手动） | 未部署 | 10-22 取代 `guard_key_image_queue`（旧键留在库里也不再读）。每个成员最多同时 2 张**还没发往上游**的图（正在生成的不算），再发当场回 429（`member_queue_full`，「你已有 2 张在排队，等前面的出完再发」），不占账号、不扣 Anlas。同一把 Key 仍然一次只生成 1 张：排着的那张等前一张出完（或被拒 / 取消）才往上游发；等待超过排队超时回 429（`queue_timeout`）。首页 / `/public/status` 里的字段名仍是 `key_image_queue`（兼容），值是这一项 |
| 全站排队 `guard_queue_per_account` | 5（0～100） | 5 | 每个上游账号最多同时 5 张（含正在生成的），再收紧到排队超时内这个账号真能开始的张数（见下面「选号与排队中换号」）；满了回 429（`queue_full`） |
| 每把 Key 每分钟请求数 | 成员 Key 5；后台手建 Key 10（代码常量，`defaults.rs`；手建时可在弹窗里改） | 未核对 | 生图 / 放大 / 导演工具 / Vibe 请求都计数，被拒的也计；标签补全不计。超出回 429（`key_rate`） |
| 排队超时 `runtime_queue_timeout` | 90 秒（代码常量；后台可改，范围 15～300） | 90 | 超时回 429（`queue_timeout`） |
| 上游 429 后的全站冷却 `runtime_image_429_cooldown_seconds` | 60 秒（代码常量；后台可改，范围 60～3600） | 180 | 实际冷却 = max(此值, 上游 Retry-After)，Retry-After 先夹到 5～3600 秒。冷却期内所有图片请求回 429（`cooldown`）；冷却状态跨重启保留（`image_cooldown_until`，超过 1 小时的值视为无效） |
| 上游连续 403（图片通道） | — | — | 连续 3 次 → 该 Token 冷却 5 → 15 → 45 → 60 分钟（`min(60, 5×3ⁿ)`），24 小时内没有 403 档位归零；私信站长。每个 Token 的 429 / 403 冷却和档位跨重启保留（设置 `upstream_token_cooldowns`，超过 1 小时的冷却、24 小时前的 403 视为无效） |
| 无效 Key 防爆破 | 同一 IP（IPv6 按 /64）600 秒内 ≥ 20 次无效 Key → 拦截 900 秒（代码常量） | — | 只拦无效 Key；有效 Key 不受影响。后台登录另有一套：每 IP 300 秒内最多 5 次（代码常量） |
| V5 官方额度查询 | 只在真实 V5 出图时刷新缓存 | 「低」线固定 20% | 缓存：剩 ≤ 2% 为 15 秒，低于 20%（后台的「V5 余量低提示线」`v5_alert_threshold` 10-23 删除，旧值不再读）为 60 秒，其余 300 秒。定时任务「上游 V5 快照」每小时最多再主动读一次订阅信息 |
| V5 Medium 降档 `v5_medium_mode` | `off`（后台「额度与保护」页，off / on；认不出的值和已删除的 `auto` 都按 `off`） | off | `on`：模型是 V5 Full（`nai-diffusion-5-full`）或其局部重绘版的请求，在费用估算和额度准入之前改成对应的 Medium 模型并按 Medium 整理参数（14 步 / Euler-a / Heavy 负面预设），出图记录的说明写「站长已临时切到 V5 Medium」；仍占成员 V5 额度。已经是 Medium、Curated、别名（`nai-v5` 等）不动。按余量自动切换的 `auto` 档 10-23 删除 |

每把 Key 的限速只有上面两层：排队（同时生成 1 张，每人最多排 2 张，见 `guard_queue_per_member`）和每分钟请求数。2.x 的「每把 Key 两张图至少隔 15 秒」（`KEY_IMAGE_MIN_INTERVAL` / `runtime_key_image_min_interval`）、每把 Key 一个信号量（`KEY_CONCURRENCY`）、「同一把 Key 同时在途 ≤ 4」和每个账号的「10 分钟突发窗口」（`burst_cap`）都已删除（与排队重复，或永远触发不了）。

**用量日志记哪个上游账号**（`usage_log.token_id`，可空文本，只增不减）：请求选中上游账号（令牌池挑号）时写进请求上下文，成功 / 出错 / 选号之后才被拒（如排队超时）的日志都带它；鉴权、准入阶段就被拒的、以及 3.0 之前的老行为 NULL。不建索引（按账号统计总是先按 `ts` 缩小范围）。后台用量日志每行多一个 `token_id`，成员接口看不到。

**按账号的最近 3 小时出图窗口**（安静时段每小时上限用）和启动时的补回随安静时段在 10-23 删除。

**账号对比与 Anlas 推算**（后台「上游」页，`GET /admin/api/upstream-stats`，只读、仅管理员、不向上游发请求）：最近 1 小时 / 24 小时按账号统计成功 / 出错 / 选号后被拒、成功请求的平均与 p90 等待（`wait_ms`）、上游 429 / 403 / 401 次数，加今日张数、每小时计数（账号保护内存窗口）、请求与余额的分担占比。余额推算（`anlas_estimate.rs`）：该账号最近一份含它的对账快照里的固定 + 购买 Anlas，减去快照之后（`usage_log.id > last_log_id`）成功请求记下的 Anlas；先扣固定额度、不够再扣购买的（顺序是推测，未经官方核实）；日均用量 = 近 3 天账本 Anlas ÷ 3；到期预测 = 固定额度剩余 − 日均 × 剩余天数，小于 0 提示「到期前用完」，否则提示「到期预计浪费 N」。快照里 `expires_at` 已过 = 周期已滚动，不推算、提示重新核对；上次真实核对超过 3 天标「核对已过期」。2 个及以上账号时，没记账号的老行归不到任何账号，余额标「近似」并写出没算进去的 Anlas；1 个账号时老行都算给它。`unconfirmed_anlas`（未确认扣费）不计入。残留键 `anlas_pool_last` 仍无人更新，不要用。

**选号与排队中换号**（10-22 起，`gate-upstream` 的 `pool.rs` / `scheduler.rs` / `dispatch.rs`）：

- 选号不再严格轮询，而是选「这张图最早能开始」的账号：估算开始时间 = max(现在, 该账号歇完的时刻) + 前面排着的张数 × 平均周期（正在生成的占满并发时还要等它大约出完再歇完）；估算差不到 0.25 秒算一样快，按轮询。资格与以前完全一样（启停、429 / 403 冷却、Anlas、V5 见底与日限、账号保护）。
- 平均周期 = (实测出图耗时滑动平均 + 出完后平均歇的秒数) ÷ 并发。
- 排队中每 0.5 秒看一次：别的有资格的账号「空出来」（估算 ≤ 0.5 秒）就把这张挪过去；等了一半排队超时以后，别的账号比现在这个早 2 秒以上开始也挪。更早来的请求也能去时先让它（最多让 1 秒）。一张图最多挪 3 次；挪过去要重新确认 V5 计费。成员看到的排队位次按上游给的预计开始时刻排。
- 占了槽却没发出去（排完队被拒 / 换号 / 客户端断开）时不算出了一张，账号不歇。
- 全站排队上限 = 每个可用账号 min(`guard_queue_per_account`, ⌈排队超时 ÷ 该账号平均周期⌉) 之和（间隔 15 + 抖动 2.5、超时 90 秒时每个账号 6 张）。
- **公平排队**（10-22，参照 AI Horde）：同一个账号上谁先轮到，按这个成员今天已经出的张数（`counters.images`，出图准入、轮到这把 Key 时读）从少到多，一样多按先来后到；重度用户自然沉底。防饿死：等了一半排队超时还没开始的，不再看用量，按先来后到排在最前。账号间隔到点、并发槽有空的那一刻才决定谁上（不是先占槽再等间隔），所以后来的轻度用户能排到前面。选号、换号的估算和成员看到的位次 / 预计开始时间都按这个顺序；换号时保留原来的排队时刻和用量。

**上游并发锁**（10-22，`gate-upstream/src/dispatch.rs` 的 `retry_after_lock`）：

- NovelAI 对同一个账号上一张还没释放时回 429（偶见 409），正文含 `Concurrent generation is locked`（不分大小写，`gate_core::upstream_policy::is_concurrency_lock`）。这**不是限流**：不冷却账号、不触发全站 429 冷却、不报 429 状态码（用量日志的 `up_status` 不记它）。
- 处理：占着这个账号的并发槽，等 2 秒在同一个账号上再发，之后 4 秒、8 秒，最多 3 次，不超过这张图的排队超时（`pace::LOCK_RETRY_BASE` / `LOCK_RETRIES`）；重试不再记一次「开始」。用完才以 503 `upstream_locked` 失败（没出图、没扣费）。
- 每次上游响应后，这个账号至少歇 1 秒（就是「每张出完后歇」的下限；测试环境不歇）：锁比响应晚一点释放。
- 后台「账号保护与排队」每个账号一列「并发锁 N 次（不算限流）」：当天（服务日）次数，内存计数，重启清零。日志里是 `code=upstream_locked`。
- 上游 401 / 403 不变：401 停用这把 Token 并告警；403 连续 3 次冷却并告警；都不重试。

**出图节奏只有「每张出完后歇 N～M 秒」、没有安静时段**（10-23）：安静时段（`guard_quiet_start` / `guard_quiet_end` / `guard_quiet_hourly_cap`，原因码 `quiet_cap`）删除，旧行不再读。10-22 加的「上游 429 后账号间隔自动翻倍、平稳后逐档降回」（AIMD）和它的四个参数 `guard_interval_cap` / `guard_interval_increase_pct` / `guard_interval_decay_pct` / `guard_interval_decay_minutes` 删除（库里的旧行不再读）。上游 429 只让这个账号按 Retry-After 冷却（加全站 429 冷却），节奏不变。`upstream_token_cooldowns` 只存前三项；10-22 版多存的第 4、5 项读到忽略。更早删除的：每小时上限（`guard_account_hourly_cap`、`guard_hourly_*`、`guard_daily_burst`、`guard_429_cap_at`）和按时段放行（`paced`）。

## 2. 额度分配

10-23 起**没有任何自动额度算法**（站长：「坚守初心让更多人进来用，调低个人限额，重度用户自己花钱」）。额度全是后台「额度与保护」页上手动设的数（`PUT /admin/api/settings`）：

| 设置 | 没设过 | 说明 |
|---|---|---|
| `register_daily_images` | 150 | V4.5 每人每天；新号老号一样（新号爬坡 `newbie_ramp_*` 删除）；0 = 不限 |
| `register_daily_v5` | 0 | V5 每人每天；0 = 不限（只受全站管） |
| `global_daily_v5` | 150 | V5 全站每天，先到先得；0 = 不限 |
| `v5_paused` | 0 | V5 暂停（开关）：V5 请求本地直接回 402（`v5_exhausted`，「V5 暂停中，没有扣费，请先用 V4.5」），不扣 Anlas、不发上游；首页、成员页显示暂停，客户端订阅信息里 V5 是 0 张（`v5Paused: true`），对所有 Key 生效 |
| `member_daily_anlas` 等 | 见 §2.4 | Anlas |

`quota_auto=1`（新建默认，后台显示「统一」）的成员 Key 由定时任务「额度同步」（每 10 分钟，后台改了额度时立即同步）把 `daily_images` / `daily_v5` 设成上面的手动值、`image_model_scope=all`，并更新首页文案 `algo_notice`；后台单独设过额度的 Key（`quota_auto=-1`）不动。新领的 Key 直接用这两个值。「领 Key」页不再能改每人张数（回 400）。

删除（库里的旧行不删，只是不再读写）：V4.5 每人上限自动调整（`quota_ceiling`、`quota_target_avg`、`quota_step`、`quota_ceiling_max`、`quota_base_min_fair`、water-fill）、总开关 `quota_auto_enabled`、每日复盘（`quota_algo_day` / `quota_algo_history` / `quota_algo_last`）、按电量定的 V5 当天方案（`quota_v5_day_plan`）、临时加量 `quota_v5_bonus`、自动暂停 `quota_v5_paused`、后台接口 `/admin/api/quota-algo`。V5 电量读数、官方恢复速度、安静时段实测恢复速度只在后台「记录」里显示（`GET /admin/api/quota-records`），不参与任何限额；上游确认免费 V5 见底时拒绝 V5（`v5_exhausted`）照旧。

### 2.4 Anlas

成员 Key 默认没有 Anlas 权限（`allow_anlas=false`），实际只有 Vibe（参考图编码）会花 Anlas，走全站 PVP 池。

| 项 | 代码默认 | 线上 | 说明 |
|---|---|---|---|
| 每人每日上限 `member_daily_anlas` | 50（0 = 不限） | 未设置，首页文案「Anlas 每人 50/天」与默认一致 | Key 自己设置了 `daily_anlas` > 0 则以 Key 为准（成员 Key 为 0）；Vibe 编码一张参考图约 2 点，50 点 ≈ 25 次 |
| 每人每月 | Key 自己的 `monthly_anlas`（成员 Key 为 0 = 不限） | — | |
| 全站每日 Anlas 池 `global_daily_anlas` | 200（0 = 不开放；后台「额度与保护 → Anlas 与 V5 预算」可改，范围 0～100000） | 1000 | Vibe PVP：全站合计先到先得，抢完回 402（`site_quota`）。开放截止时间戳 `global_daily_anlas_until` 已删除（后台从来没有写入口） |
| 全站每月预算 `global_monthly_anlas` | 10000（0 = 不限） | 9000 | 超出回 402 |

价格（码 `policy.rs`）：Vibe 编码 2 Anlas/张；精确参考 5 Anlas/张/图；V4 / V4.5 的 Vibe 超过 4 张，每多 1 张 +2/图。Anlas 自动分配（旧的分配池）已删除，快照里残留的 `anlas_pool_last` 等键不再被读取。

### 2.5 免费规格（对没有 Anlas 权限的 Key 生效；数值写死在 `policy.rs`，没有环境变量）

- 每次 1 张（`n_samples` 强制为 1）；步数 ≤ 28（`FREE_MAX_STEPS`）；SMEA 关闭。
- V4.5 及更早的模型：总像素 ≤ 1024×1024 = 1048576（`FREE_MAX_PIXELS`），宽高取 64 的倍数，超出等比缩小。
- V5：不超过三个预设之一——832×1216、1216×832、1024×1024，超出按长宽比吸附（> 1.1 → 1216×832，< 0.9 → 832×1216，其余 1024×1024）。
- 图生图 / 局部重绘只对后台勾了「允许图生图」（`allow_img2img`）且有 Anlas 权限的 Key 开放，其余回 400；ControlNet 回 400；角色参考（`characterReferences`）被移除。
- 新领 Key 开通的功能 `register_features`（线 `image,vibe,tags`；`*` = 不限）；功能可选 `image`、`upscale`、`augment`、`vibe`、`tags`（文本生成 / 语音 / OpenAI 兼容聊天 2026-10-15 已整套删除，库里 Key 的 `features` 或 `features_global` 里残留的 `text` / `voice` 读取时被忽略）。站长还有全局开关 `features_global`（JSON，缺省全开）。
- 上游对「模型不存在」回的是 403，网关会窥探一小段响应体把「模型级 403」和「账号级 403」分开（只有账号级才计入账号健康度和 403 冷却）。

### 2.6 重置券

站长在后台给某把 Key 发券；成员在首页使用后，重置**本人**今天已用的 V5 与 Anlas 计数（`daily_quota_offsets`），不含 V4.5，也不改变全站池。一张券只能用一次，有过期时间。

## 3. 自动驾驶（两个定时任务，各每 10 分钟）

两条规则一律执行。2.x 每条规则还有 `off` / `observe` / `enforce` 模式（设置 `autopilot_<规则>`），以及第三条规则「名额 +5」（`slots`），3.0 都已删除（名额规则从不生效，只用过 enforce），库里残留的 `autopilot_slots` / `autopilot_breaker` / `autopilot_key_guard` / `autopilot_slots_at` 不再读。`autopilot_last` / `autopilot_history`（每小时快照）3.0 起不再写；判断结果看后台「系统 → 定时任务」里两个任务的摘要。

| 规则 | 做什么 |
|---|---|
| `breaker` 熔断 | 最近 15 分钟图片请求里「上游失败」≥ 5 次且 ≥ 30%，且这些失败来自 ≥ 2 把不同的 Key → 全站暂停出图 300 秒（`breaker`）。只算上游 5xx 和流中途断开；本地拦截、上游 429、4xx 不算；只看上次熔断结束之后的数据；失败只来自 1 把 Key（多半是这把 Key 的请求有问题）不熔断 |
| `key_guard` 单 Key 限流 | 1 小时内同一把 Key 被拒 ≥ 60 次 → 暂停该 Key 1 小时（回 403，`key_paused`）。统计不含站点层面的拒绝（排队满、各类账号上限、熔断、冷却、`v5_exhausted`、`queue_timeout`、`site_quota`、`key_paused` 本身）。提示「{原因}，{到期时间} 自动恢复。」（2.x 留下的旧行原因为空，写「这把 Key 已被临时暂停，{到期时间} 自动恢复。」）。2.x 还有「换网段」「多客户端全天在用」两条规则，输入来自早已删除的防分享证据、恒为空，3.0 已删除；数据库表 `share_state` 原样保留，现在只存每把 Key 的临时暂停 |

## 4. 定时任务（7 个，码 `jobs.rs`、`jobs/`）

每件事一个循环，启动后立即跑第一轮，之后按周期再跑；一轮做完才响应关停。每轮的结果（时间、正常 / 出错、一句摘要、耗时）记进设置 `job_last`，后台「系统」页的「定时任务」卡片显示：超过 3 个周期没跑或上次出错时，「系统」的侧栏角标和总览「N 个定时任务上次出错或超时没运行」会提醒。任务出错或 panic 只算这一轮失败（记 Bug，来源 `job:<名字>`），循环照常继续。卡片里没有开关。

| 任务 | 周期 | 做什么 | 开关 |
|---|---|---|---|
| `quota` 额度同步 | 10 分钟 ±20% | §2：把手动的每人额度下发给 `quota_auto=1` 的 Key、更新首页文案；后台改了额度、把 Key 交回统一额度时立即同步一次（同样记进 `job_last`） | 没有开关 |
| `upstream_snapshot` 上游 V5 快照 | 10 分钟 ±20%，距上次满 1 小时才记 | 主动读一次订阅，写一行 `upstream_snapshots`（`fresh=1` 才拿来做安静时段测速）；读不到也记一行无读数的 | — |
| `hourly_cap` 每小时上限自动调整 | 10 分钟 ±20% | §1 的 AIMD 加窗：平稳一天且顶到过上限 +10，24 小时最多一次，只升不降 | `guard_hourly_step=0` 不上调 |
| `breaker` 全站熔断 | 10 分钟 ±20% | §3 | — |
| `key_guard` 单 Key 限流 | 10 分钟 ±20% | §3 | — |
| `maintenance` 维护 | 5 分钟 ±20% | 清过期记录、状态码统计落库、原图保存期与容量上限、自动关闭不用处理的 Bug、注册服务的身份组同步、磁盘自检、官方状态；每一步各自出错、各自记 Bug（来源 `maintenance:<步骤>`） | — |
| `idle_recycle` 闲置回收 | 1 小时（不抖动） | §8.2 | `KEY_INACTIVITY_DELETE_DAYS=0` 关闭 |

2.x / 3.0 早期的「内核模块」（观测、容量、分配、自动驾驶、风险检查、领 Key）连同交叉自检、开关（`module_<名字>`）、运行条和后台「模块」页一起删除。原来线上核对的恒等式改由测试守住：生成记录条数 = 成功出图张数 = 今日计数、内存每小时计数 = 本小时日志张数、算法管理的 Key 都已同步。出站指纹的只读体检（风险检查）没有替代。

## 5. 告警：谁私信站长，谁只记日志

**原则**（站长 2026-10-15）：只有重大事故才私信——成员现在用不了，或账号 / 数据有风险，且需要站长动手。其余只写日志，后台总览「有 N 件事要处理」能看到同样的情况。只写日志的事件在发生的地方直接打一行日志，不经过告警通道、也不去重。私信只有一个通道：机器人（`DISCORD_BOT_TOKEN`）私信 `ALERT_USER_ID`（`ALERT_CHANNEL_ID`、`ALERT_WEBHOOK_URL` 不再读取）。

### 5.1 网关会私信的（共 5 个，见 `alerts.rs` 模块说明；同一个 kind 在冷却期内只发一次）

| kind | 触发 | 冷却 |
|---|---|---|
| `upstream_401` | 上游返回 401：Token 失效，生图全部不可用 | 30 分钟 |
| `upstream_403_cooldown` | 图片通道连续 3 次 403，Token 进入冷却 | 30 分钟 |
| `upstream_token_changed` | 后台改动了上游 Token（安全事件） | 无 |
| `disk_low` | 数据盘剩余不足 10% | 6 小时 |
| `test` | 后台「发送测试告警」 | 无 |

### 5.2 只写日志的

上游 5xx（出图请求的 `[upstream-5xx]` 行带请求形态）、上游 429 与每小时上限自动下调、V5 余量低（`V5 low allowance`）、无效 Key 洪水（某 IP 被临时拦截）、Bug（每次记录都有一行 `bug` 日志）、单次 402 / 403。上游降级 / 恢复、上游表现变化（变慢 / 429 增多 / 失败率上升）不再单独记：状态页按最近 10 分钟算上游状态，后台「上游」页打开时现算表现。服务启动只有进程自己的启动日志。

## 6. Bug 追踪与自动处理

- 来源：接口未捕获的错误（500）、后台任务失败、记账写入失败、成员在图片返回前断开、网页脚本报错。同一个特征码只占一行（次数累加），复发会重新打开。不记 Key、提示词或 IP。
- **不入库**：浏览器网络失败（`Failed to fetch`、`NetworkError`、`Load failed`、`AbortError`、`cancelled`、`已取消` 等 10 种说法，不分大小写）；出错脚本不在 `/_owl/` 下的 `Uncaught …`；原因在上游的 5xx（它们在用量日志和「上游」页里照常有记录）。
- **自动关闭**（维护循环，最多每 10 分钟一次，启动后第一轮就跑）：上述噪音的存量立刻关；warn 级 24 小时、error 级 7 天没再出现的关。关闭原因存在 `error_events.auto_resolved`（`network` / `upstream` / `foreign_script` / `stale_warn` / `stale_error`），后台显示「自动处理」徽标。
- Bug 记录长期保留。新 Bug / 复发只写日志，不私信。

## 7. 数据保留与备份

| 项 | 值 | 来源 |
|---|---|---|
| 提示词 | 开关 `audit_prompts`（线 1 = 记录，没设过 = 不记）；保留天数 `audit_retention_days`（线 0 = 长期保留；没设过 = 7；范围 0～3650）。环境变量 `AUDIT_*` 已删除（只看后台设置） | 线 / 码 |
| 原图 | 开关 `audit_thumbs`（线 1 = 保存成功结果的原图，设置名沿用旧名；没设过 = 不存）；`audit_image_keep_hours`（码 24，线 0 = 只按容量上限删）；`audit_image_cap_mb`（码 8192，线 8192）：超过后从最旧的开始删到约 90%；没有记录指向的原图文件（崩溃残留）超过 1 小时清掉 | 线 / 码 |
| 用量日志 | `USAGE_LOG_RETENTION_DAYS` 默认 0 = 长期保留；大于 0 时按 max(7, N) 天清理。线上未核对。每日计数器和账本不受影响 | 码 |
| 后台操作日志 | 180 天 | 码 |
| 状态码统计 / 已处理的 Bug | 状态码统计保留 30 天；已处理的 Bug 超过 30 天没再出现才清理 | 码 |
| 披露 | 首页、`/register` 回复都用同一个函数（`audit_disclosure`）生成「本站记录了什么」，以它为准；10-18 起不写具体时长，只说「到期自动删除 / 清除」（时长在后台「生成记录」卡里调：提示词天数、原图小时数、来源 IP 和客户端天数 `usage_origin_keep_days`）。`/terms` 和 `/privacy` 是同一页「本站说明」，同样不写时长 | 码 / 线 |

备份（本机 GFS、异地加密、脱敏只读快照、状态页镜像、Mac 拉取）的时间和内容只在 `docs/OPERATIONS.md` 「备份与快照」写一份。

快照由宿主机 `nai-ops snapshot` 逐表复制（只读事务，不经过网关容器，不锁库）。部署、切换、回滚、灾难恢复见 `docs/OPERATIONS.md` 与 `deploy/ops/migrate_restore.sh`。停机时网关最多等 110 秒让生成中的图完成并记账，compose 的 `stop_grace_period` 为 120 秒。

## 8. Key 的一生

### 8.1 领取（Discord `/register` 是唯一发 Key 的路径）

| 检查（按代码里的先后顺序） | 值 |
|---|---|
| 必须在指定服务器里发命令 | `DISCORD_GUILD_ID` |
| Discord 账号年龄 | ≥ 7 天（代码常量 `defaults.rs`，没有环境变量） |
| 没被拉黑 | 只有站长手动拉黑（闲置回收 10-23 起不拉黑；之前闲置回收留下的黑名单行仍然生效，要放人得站长在后台解除） |
| 一个 Discord 账号一把 Key | 重复领取会提示在网页看额度、用 `/resetkey` 换新 |
| （填了邀请 Key 的在这里分流，直接绑定，不看下面三项） | |
| 每小时最多发放 `issue_hourly_cap` | 码 12，线 0（不限） |
| `register_open` 与名额 | `register_open` 线 1，关 = 「领 Key 当前已关闭」；名额 `register_max_users`（没设过 = 10；线 0 = 不限，须确认是库里的值），满了先到先得，没有候补 |
| 可选：身份组门槛 | `register_role_guild` / `register_role_id`，线均为空（不设门槛）；机器人不在该服务器里，门槛配齐后首页「登录」（`/login`，`?verify=1` 兼容）一律用 scope `identify guilds.members.read` 授权，回调里用成员自己的授权查身份组，核验结果记在 `role_verifications` 表；没有身份组且没有 Key 的人不发会话、回首页 `/?verify=no`（Discord 查不了为 `/?verify=error`），已有 Key 的成员照常登录；`/register` 只查这张表；没配门槛时登录只要 `identify`。`/public/status` / `/public/live` 的 `registration.role_note` 仅在门槛配齐时给出名称 |

新 Key 的默认值：每日 V4.5 = `register_daily_images`、每日 V5 = `register_daily_v5`（都是「额度与保护」页的手动值，见 §2）、模型范围固定含 V5（`register_image_scope` 10-23 起不再读）。

- **邀请 Key**：站长预先发的、带「邀请」标签；成员 `/register` 时填入即绑定到自己的账号，Key 不变，不受开关 / 名额 / 每小时上限限制。邀请 Key 豁免闲置回收和全站重置。
- **到期**：过期的 Key 回 403「该 Key 已过期」；成员再 `/register` 时旧 Key 和记录自动释放，可重新领取。被站长停用的 Key 不会因过期被释放。
- **换 Key**：`/resetkey`，旧 Key 立即失效。

### 8.2 闲置回收

`KEY_INACTIVITY_DELETE_DAYS`：码默认 3，线 7（10-23 起）；0 = 关闭（什么都不删）。每小时扫一次。

- 判定：**连续 N 天没有成功生成过图片**（只看 `last_image_at`，从没生成过则从创建时间起算）；只在 Key 启用中且未过期的才算。
- 站点出不了图的日子不算：已完结的服务日全站成功出图不到 50 张（上游 Token 失效、站长关生图、熔断等）就跳过这一天，往回多数一天，凑够 N 天（最多回看 30 天）；当前服务日不在安静时段、且最近 60 分钟全站一张都没出成功时，本轮不回收。平时截止时间仍是「现在 − N 天」。
- 豁免：「邀请」标签的 Key；已被停用的 Key 也不回收；已过期的 Key 不算闲置（照旧在成员下次 `/register` 时释放，不拉黑）。
- 动作：删 Key、释放名额、摘掉身份组；**不拉黑**（站长 10-23），成员之后可以再 `/register` 领新 Key。不提前私信提醒（站长 2026-10-13）。操作日志「闲置回收 Key」，详情「连续 N 天未活动自动回收」。
- 保险：网关停机超过 1 小时再启动时，闲置计时从启动那一刻重新起算（`key_inactivity_grace_started_at`），不会因停机一次性删光；该设置读不懂时不删任何 Key。「停机」看最后一次有动静的时间：请求日志和定时任务的运行记录（`job_last`，维护任务每 5 分钟一次）取较晚者。只看请求日志会把安静时段凌晨没人发请求时的重启误判成停机（10-14 清晨误判过一次）。
- 10-23 之前被闲置回收拉黑的人（黑名单理由「连续 N 天未活动自动回收」）再来 `/register`：「你之前的 Key 连续 N 天没有出图，已被自动回收，这个 Discord 账号不能再领取 Key。」站长手动拉黑的照旧是「已被站长停用」。两种提示都不提解封。

### 8.3 退群回收

`member_sweep`（默认开）：每轮维护（约 5 分钟）用机器人 Token 核对 1 人，每人每天一次。只有 Discord 明确回复 404 + code 10007（不是成员）才删 Key、释放名额、写操作日志「退群回收 Key」；任何其他结果（限流、网络、机器人不在服务器）都不动，1 小时后重查。不拉黑，不私信。邀请 Key 只由站长手动删：持有人退群时不删 Key，只写操作日志「退群（邀请 Key 保留）」（对象「Key #编号 名字」）提醒站长，之后照常每天核对一次。

### 8.4 全站重置与停用原因

- 后台「全站重置 Key」（需要输入确认词）：失效所有非管理员、非「邀请」的 Key，写停用原因 `site_reset`，清空它们的 Discord 绑定和标签。成员拿旧 Key 请求回 403「这把 Key 在全站重置时已经失效：请在 Discord 服务器里用 /register 重新领取……」。
- 站长在后台手动停用：原因 `admin`（没写原因时同样处理），回 403「该 Key 已被站长停用，有疑问请联系站长。」；重新启用时清空原因。
- 被占名额：停用的成员仍占名额（避免停用后空出名额、启用后超限）。
- 封禁（拉黑）：写入封禁表，撤销现有 Key 与身份组，之后不能再领。

## 9. 共用站点设置（登记表 `site_flags.rs`）

读写只走 `site_flags::get / put`；缺失或格式不对一律回到默认值（默认值按 fail-closed 选）。下表是登记表的全部 10 项：前 9 项是 `ALL`，第 10 项 `member_daily_anlas` 已登记在表里但没列进 `ALL`。web_login_paused、home_join_open、global_daily_anlas_until、audit_image_retention_days 四项已删除，库里残留的值读到也忽略。各模块自己的参数组不在此表，见表后。

| 键 | 类型 | 默认 | 范围 | 说明 | 线上当前值（2026-10-15） |
|---|---|---|---|---|---|
| `member_sweep` | flag | True | | 退群回收：每人每天核对一次是否还在服务器，不在就删 Key | 未设置，按默认（开） |
| `issue_hourly_cap` | int | 12 | 0–500 | 每小时最多发放的新 Key（0 = 不限） | 0（不限） |
| `register_open` | flag | False | | 开放 /register 领取 | 1 |
| `global_daily_v5` | int | 150（码；环境变量 `GLOBAL_DAILY_V5` 已删除） | 0–100000 | 全站每日 V5 张数（0 = 不限）；手动（10-23 起算法不再写） | 171 |
| `global_daily_anlas` | int | 200 | 0–100000 | 全站每日 Anlas 额度（0 = 不开放 Vibe；Vibe PVP 抢额度） | 1000 |
| `global_monthly_anlas` | float | 10000.0 | 0–1e+06 | 全站每月 Anlas 预算（0 = 不限） | 9000 |
| `algo_notice` | str | '' | | 首页显示的今日额度说明（算法每个 tick 写入） | 每天变，不抄 |
| `audit_image_cap_mb` | int | 8192 | 0–500000 | 原图占用上限（MB）：超过后自动删掉最旧的；0 = 不限 | 8192 |
| `audit_image_keep_hours` | int | 24 | 0–8760 | 原图在服务器上保存几小时；0 = 只按容量上限删 | 0 |
| `member_daily_anlas`（第 14 项） | int | 50 | 0–100000 | 每人每日 Anlas 上限（仅对未手动设置 Anlas 日额度的 Key 生效；0 = 不限） | 未设置，按默认（首页文案核对为 50） |

其他参数组（都存在 `site_settings`，键名带前缀）：

| 组 | 键 | 默认 / 线上 |
|---|---|---|
| `guard_*` | 5 个账号保护字段：`account_daily_cap`、`post_jitter_min`、`post_jitter_max`、`queue_per_member`、`queue_per_account`（另有运行态 `guard_token_daily_caps`、熔断状态），见 §1 | 见 §1 |
| `runtime_*` | `queue_timeout`、`image_429_cooldown_seconds`，后台可调，重启后仍优先于代码常量（`runtime_image_min_interval` 10-23 删除，旧行不再读） | 见 §1 |
| `quota_*` | 10-23 起全部不再读写（见 §2「删除」） | — |
| `autopilot_*` | `autopilot_breaker_until`（规则模式键、`autopilot_last`、`autopilot_history` 已不再读写，见 §3） | 运行态 |
| `register_*` | `register_open`、`register_max_users`、`register_daily_images`、`register_daily_v5`、`register_image_scope`、`register_expires_days`、`register_features`、`register_role_guild` / `register_role_id` / `register_role_note` | 线 1 / 未设 / 95 / 3 / all / 0 / `image,vibe,tags` / 空 |
| `audit_*` | `audit_prompts`、`audit_thumbs`、`audit_retention_days`（见 §7） | 线 1 / 1 / 0 |
| `bot_*` | `bot_status`（机器人心跳，奶妹每分钟经桥接接口写一次：账号 / 服务器 / 网关延迟 / 启动时间 / `seen_at`；3 分钟没更新算离线，后台 Discord 页的「机器人状态」读它） | 随机器人上报 |
| 其他 | `features_global`（JSON，缺省全开）、`v5_alert_threshold`（后台叫「V5 余量低提示线」，默认 20，范围 1～100；决定官方余量缓存刷多勤、后台是否标「低」，以及 `v5_medium_mode=auto` 的切换线）、`v5_medium_mode`（`off` / `on` / `auto`，默认 `off`，见 §1）、`job_last`（定时任务每轮的结果，见 §4） | 线 `v5_alert_threshold` = 20 |

线上库里还有一些代码已不再读取的残留键，看到不要当真：`anlas_pool_last`、`anlas_member_daily_cap`、`anlas_min_key_age_days`、`economy_mode`、`autopilot_economy`、`guard_daily_cap_fallback`、`guard_account_3h_cap`、`guard_account_10m_cap`、`ack_*`、`quota_shadow_*`、`guard_base_daily_images`、`quota_base` / `quota_base_min` / `quota_base_step` / `quota_base_now`、`quota_v5_min` / `quota_v5_max`、`runtime_key_image_min_interval`、`autopilot_slots` / `autopilot_breaker` / `autopilot_key_guard` / `autopilot_slots_at`、`autopilot_last` / `autopilot_history`、`kernel_last`、`module_*`（内核模块改成定时任务后不再读写）、`module_allocation`（3.0 额度精简删掉的），以及 `bot_gallery_*`（跑图分享设置）、`bot_events`（机器人最近动作）——后两者随功能一起删除。

## 10. 拒绝原因码（`usage_log.reason`，`reasons.rs`）

统计、自动驾驶和每小时上限的自动调整都按码数，不数中文说明。

| 码 | 状态 | 含义 |
|---|---|---|
| `queue_full` | 429 | 全站排队满 |
| `key_busy` | — | 10-22 起改记 `member_queue_full`，只剩历史记录 |
| `member_queue_full` | 429 | 这个成员排队中的图已到每人上限（`guard_queue_per_member`）；单 Key 限流按半次算（与 `key_busy` 相同） |
| `upstream_locked` | 503 | 上游「并发锁」重试用完仍被锁（见下面「上游并发锁」）；站点层面，不计入单 Key 限流，也不算 429 |
| `hourly_cap` / `quiet_cap` / `daily_cap` / `paced` | 429 | 账号每小时 / 安静时段 / 每日上限 / 全站时段份额 |
| `cap_3h` / `burst_cap` | — | 3 小时上限、10 分钟突发窗口已删除，只剩历史记录 |
| `over_base` | — | 保底一层已删除，历史记录按说明第一句显示 |
| `breaker` / `cooldown` | 429 | 全站熔断 / 上游 429 后全站冷却 |
| `queue_timeout` | 429 | 排队或预算核对等待超时 |
| `key_rate` | 429 | 这把 Key 每分钟请求太多 |
| `key_paused` | 403 | Key 被自动驾驶暂停 |
| `feature_off` | 403 | 功能没给这把 Key 开通 / 全局关闭 / 模型范围不够 |
| `member_quota` | 402 | 这把 Key 自己的每日 / 每月额度用完 |
| `site_quota` / `v5_exhausted` | 402 | 全站共享额度用完 / 上游账号 V5 免费额度耗尽 |
| `test_request` | 400 | 客户端的「连接测试」（普通文生图、步数 ≤ 5、面积 ≤ 512x512、提示词 ≤ 40 字符）：Key 有效，但不出图；计入单 Key 限流（60 次 / 小时） |
| `param` | 400 | 参数不被支持（模型、采样器、角色数、权重写法等） |
| `body_abort` | — | 请求体传输中断（客户端或网络） |
