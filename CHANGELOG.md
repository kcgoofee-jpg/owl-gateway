# 更新日志

格式参考 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)，版本号遵循 [语义化版本](https://semver.org/lang/zh-CN/)。
每个版本分「成员可感知」与「站长 / 运维」两部分；成员公告只发前者。
2.x 及更早的记录见标签 `py-final`。

## [Unreleased]

### 成员可感知

- **A simpler account and survey flow.** Members who have a key now find its copy controls in the avatar menu; members without a completed survey are prompted after login, and `/register` explains where to find the key and survey. Removed donation and direct member-reward pages, controls and API handlers. Historical database tables and rows remain intact for compatibility; no bot donor-DM behavior exists.
- **Tighter homepage proportions.** Reduced the status headline, detail and Discord login control sizes on desktop and mobile; removed the usage-guide heading and generic-client note.

- **A quieter homepage with an inspiration gallery.** Site status now sits directly on the page, with one shallow daily-usage arc: V5 images form a blue prefix, followed by amber V4.5 images, both using the total daily cap. Remaining allowances stay visible below, and historical V5 images remain in the arc when V5 pauses. Paused, unlimited and next-reset states remain explicit. The brand link scrolls to lazy-loaded public NovelAI quick-start examples below the homepage; clicking an example copies its prompt with brief image feedback. Client setup and usage rules share one guide. A weekly workflow checks the public image/prompt snapshot and opens a reviewable change only when it differs; it does not deploy the site. Closed registration shows a visible notice.

- **额度全部改成站长手动定的数，没有自动算法了（站长 10-23）。** 夜间安静时段也删除。V4.5 每人每天一个数（部署时 150），新号和老成员一样（新号额度爬坡取消）；V5 每人 / 全站每天也是手动的数，另有手动「V5 暂停」开关：暂停时 V5 请求直接拒（没有扣费），首页、成员页和客户端订阅信息都显示暂停 / 0 张。每个上游账号每天的出图上限保留（手动 2400），出图间隔固定（429 只冷却账号，不再自动翻倍），V5 Medium 只剩手动开 / 关。
- **机器人只剩 `/register`、`/resetkey`、`/反馈`。** `/quota`、`/help` 删除，额度在网页登录后查看（机器人启动时重新注册命令，旧命令随之消失）。

- **Images go to the account that can start them soonest, and waiting images move when another account frees up.** The gateway used to alternate strictly between upstream accounts and never moved a waiting image, so one slow account produced 505 queue timeouts on 10-21. Now each image picks the account with the earliest estimated start (interval due time + images ahead × average cycle; ties still alternate). While waiting, it moves to another eligible account that becomes free, or, after half the queue timeout, to one that will clearly start earlier (same eligibility rules: V5 / Anlas / caps). The site-wide queue cap shrinks to what each account can really start within the queue timeout, so requests are refused at once instead of timing out. Queue positions on the home page follow the new per-account estimate.
- **Light users go first on a busy account; each member may queue up to 2 images.** When an account frees up, the waiting image whose member has produced the fewest images today goes next (ties by arrival); an image that has waited half the queue timeout is served by arrival order regardless. Queue positions and start estimates follow this order. A third waiting image is refused at once with 「你已有 2 张在排队，等前面的出完再发」 (nothing charged, no interval used).
- **"Concurrent generation is locked" is no longer a rate limit.** NovelAI's lock on an account that has not released its previous image is retried on the same account after 2 s / 4 s / 8 s (within the queue timeout) instead of failing the image and slowing the account down.
- **No more hourly cap or paced share.** "最近 60 分钟已经出满" and "这个时段全站的出图份额用完了" (with its 5-minute wait in the gateway) are gone; the daily cap and quiet hours are unchanged.

- **「本站说明」页下线。** `/terms`、`/privacy` 两个地址连同页脚的「本站说明」入口一起删除（站长 10-22）；两个地址现在返回 404。相关的页面生成代码和测试一并移除。

- **Status page on phones: headline and button on one line, names not cut off.** At 375px the 26px "所有系统运行正常" pushed 订阅更新 onto its own row, and "出图接口（gate.davidzhao.top）" was truncated with an ellipsis. On phones the headline is 20px (15px at ≤360px) next to a smaller button, and the part of a component name in brackets moves to a second line in small grey text instead of being cut.

- **"Registration full" no longer promises idle recycling.** The /register reply said idle keys get recycled automatically even though recycling is off; that clause now only appears when `KEY_INACTIVITY_DELETE_DAYS` is not 0.

- **V5 requests that cannot fit today's quota are refused at once.** The entry quota check skipped V5 (a leftover from the removed "V5 refill"), so a second V5 image from the same key, sent while the first held the last slot, queued until the first finished before getting the 402 (47 s on 10-21), and a member already at the cap could wait in the paced queue first. V5 now goes through the same entry check as everything else, counting other requests' reservations.

- **Key and status cards side by side, status card reworked.** Members with a key see 我的 Key and the status card in one row at equal height. The status card is one line (coloured dot inside the headline, so it never wraps alone) plus two meters grouped at the bottom: name | bar | remaining number (今日剩余 / V5 剩余; 今日已出 / V5 已出 with a dashed line when unlimited; 明天可用 / V5 明天 once a pool is used up, 今天 between midnight and the reset). The cap is in the tooltip. While live data is loading the meters are a skeleton instead of zeros.
- **Status wording matches what the gateway actually does.** Queue countdowns divide the per-account interval by the number of usable accounts (two accounts draw at once; measured 9.4 s per image site-wide vs 17.5 / 2), for both 约 X 后开始 and 你的图排第 N 位. 排队已满 shows when waiting + running reach the cap, the same moment the gateway starts refusing. The hourly cap shows the current site-wide value after automatic adjustment (guard.hour_cap), and quiet-hours slowdown shows the site-wide hourly count (rules.quiet_cap) instead of nothing. A breaker reads 上游连续出错 with only the time left; an owner-written reason goes in the grey text. Upstream cooldown reads NovelAI 限流中 · 暂停约 N 秒. Details are shorter (no "滚动窗口 / 不是整点重置 / 全站出图按时段放行" explanations).
- **429 texts say "per account".** The hourly-cap and quiet-hours refusals said 「全站」 next to a per-account number; they now read 「最近 60 分钟已经出满（每个账号 N 张）」 and 「每个账号每小时只出 N 张」.

- **The status card no longer repeats the quota rules.** The "今日额度：V4.5 每人 … / V5 每人 … / Anlas 每人 …" note under the meters (shown to guests and members without a key) is gone; members see their own quotas after they get a key.
- **The join card explains itself and the button is no longer a full-width bar.** Members without a key read "Key 在猫头鹰公益站的 Discord 服务器里领：加入后在任意频道输入 /register，bot 会把 Key 发给你" next to a normal-width Discord-branded button (Blurple with the Clyde mark, like the login button); on phones the button takes its own full row.
- **Members without a key see one thing: the way into the Discord server.** The logged-in "three steps" (already verified / type /register / fill in the client) are gone; a single card with the join button replaces them, greyed out as "registration closed" when it is. The three steps remain only for guests when web login is off.
- **Members without a key can find the Discord server again.** The new home page dropped the "join Discord" button from step 2, but role checks happen in other servers, so a verified member may not be in ours yet and had no way in. The button is back under the `/register` command (greyed out when registration is closed). The "not in the server yet?" line only shows when an invite link is configured, and `cargo xtask dev` now sets a sample invite so the button can be checked locally.
- **The V5 ring counts the site-wide V5 pool.** `/public/me` now carries `today.site_v5_left` / `site_v5_cap` (null when the site has no V5 cap or the key is not counted against it). The home page V5 ring shows the smaller of what you have left and what the site has left; when the site pool is empty the ring turns grey and reads 暂停, with the reset hour in its tooltip. A member without a personal V5 cap sees the ring drawn against the site pool. The ring labels no longer finish their entrance before the splash lands: whether the splash plays is decided before the first frame.
- **Status page bars: partial outages are orange.** Level 2 (partial outage) had the same red as level 3; it now uses a separate `--orange` token (#b54700 light / #ff801f dark, ≥ 4.5:1 on every surface). Days with a level above 0 but no incident title say 「上游失败率偏高」 instead of 「没有记录的停机」.
- **Requests already queued stop at quiet hours.** When quiet hours (or a breaker, or an account's daily cap) begin while requests are waiting for an account, they were still sent (three images went out at 03:00). The gateway now re-checks the account guard right before sending and rejects with the real reason, no charge. During quiet hours, retries are told 「夜间安静时段」 before the per-minute limit is counted, instead of 「请求过于频繁」.
- **New home page and status page design, with motion.** The home page and `/status` share one top bar (the owl-eyes logo links home, 状态, a 捐赠 button that opens the donation form in a dialog, a round theme toggle, the avatar menu), the same tokens and cards. Guests see only the live status card on the first screen, with 用 Discord 登录 (Discord Blurple, official Clyde mark) on its status line, then the announcement, client settings and rules in one 760px column; the headline is 26px and the two site-wide meters sit side by side. Members with a key see today's quota as rings (what is left inside, type and daily total underneath, unlimited = full faint ring, reset hour below), then the key shown in full, reset coupons with an hh:mm:ss expiry countdown inside the 使用 button (the ring refills after use) and the status card. Members without a key get the three steps first with a `/register` copy box. Copy buttons now sit outside the text box at the same height. Motion: a once-a-day splash (two amber eyes blink, "Drawing / fantasy" in a self-hosted Newsreader subset, then the eyes fly into the logo or become the V4.5 / V5 rings; click anywhere to skip), a blink transition between pages (cross-document View Transitions), blocks developing from blur as they scroll in, a grain layer behind the first screen that moves faster when the queue is longer, and 90-day bars growing day by day on `/status`. Everything is off when the system asks for reduced motion. Quota type colours are tokens (`--c-v45`, `--c-v5`, `--c-anlas`).
- **Reset time says "today" after midnight.** Daily-cap, per-member and site-wide quota messages, and the landing status line and meters, said "明天 7 点" even between 00:00 and the 07:00 reset. The gateway now picks 今天 / 明天 from the current Beijing hour; the landing page takes the word from the 429 text, or computes it when no text is available (only the V5 meter is full).
- **V5 with `k_dpm_2` is rejected up front.** Upstream answers every V5 request using `k_dpm_2` with a 500, and clients kept retrying. The gateway now returns a 400 naming the sampler and suggesting `k_euler_ancestral` / `k_dpmpp_2m`; V4.5 keeps accepting it.
- **Older client request shapes are accepted.** Precise references sent as a raw `director_reference_images` PNG array are rewritten into the official `director_reference_images_cached` form (key = sha256 of the PNG) before validation; non-PNG images get a clear 400 before anything is sent. `parameters.uc` fills `negative_prompt` when the latter is missing. `characterReferences` and `reference_image` stay rejected.
- **两类常见报错说清原因。** ① 客户端用本站暂不支持的参考图写法（`director_reference_images`、`characterReferences`、`reference_image`）时，不再只回「请使用完整的 … 参考参数」，而是说明收到的是哪个字段、没有扣费、成员现在能怎么办，以及客户端作者该改成什么。② 客户端选了 OpenAI 格式（请求 `/v1/models`、`/v1/images/generations` 等，表现为「当前无可用模型」）时，404 不再只回 `Not Found`，而是提示把 API 格式改成 NovelAI、地址不要加 `/v1`；还在调文本 / 语音接口的会被告知已停用。状态码不变。
- **服务条款和隐私政策合成一页「本站说明」。** `/terms` 和 `/privacy` 都打开这一页（Discord 开发者门户里填的链接不失效），首页页脚只留一个「本站说明」入口。内容精简：个人公益站、按现状提供；内容上只有一条规则「不涉政」，其余以 NovelAI 返回为准、不审查不评价；Key 一人一把不转卖。记录了什么照实写，但**不写具体保留时长**（时长在后台调，改了不用动这页）。
- 领 Key 时和首页上的「本站会保留你的……」也不再写具体天数 / 小时数，改成「到期自动删除 / 清除」。

- **客户端的「连接测试」不再真的出图。** 有些客户端（比如流行的 SillyTavern 预设）点「测试连接」时会发一张真实的小图（512x512、4 步、提示词 `simple circle, white background`，甚至 64x64、1 步、提示词 `test`），在共享账号上白白占用额度。现在 Key 有效、且请求同时满足「普通文生图、单张、没有角色提示词、步数 ≤ 5、宽 × 高 ≤ 512 × 512、提示词 ≤ 40 字符」时，网关直接回 400：「连接正常，Key 有效。这是「测试」请求……请直接用正常参数生图。」——不出图、不占额度和队列、不碰上游。Key 无效 / 停用仍是原来的 401 / 403。正常参数（如 28 步 832x1216）的请求不受影响。
- **限定身份组在登录时核验。** 后台「领 Key」里设了限定身份组后，首页「登录」的 Discord 授权页一次就申请 `identify guilds.members.read`（「阅读关于您所在服务器的成员信息，例如昵称、头像、身份组」），回调里当场核验：有身份组就记下并登录，之后用 `/register` 领取；没有身份组又没有 Key 的人不会登录，首页提示「没有检测到「身份组名称」，暂时不能领取 Key」；Discord 暂时查不了时提示「Discord 暂时无法核验，请稍后再试」，不会说成「你没有身份组」。`/register` 里没核验过的人会被告知先到网站点「登录」。
- 已有 Key 的成员不受影响：核验不到身份组、甚至 Discord 暂时查不了，也照常登录进自己的额度页，不弹提示；邀请 Key 绑定同样不受影响。没设身份组门槛时，登录只申请 `identify`，行为和以前完全一样。
- **重启后不再提前碰到「每小时上限」。** 以前网关每次重启，会把最近 3 小时全站的出图数同时记到每一个上游账号头上；有两个账号时每张图被算了两遍，重启后最长一小时里更容易提前收到「账号每小时上限」的 429。现在每个账号只记自己出过的图。
- **报错说清原因和下一步。** 网关回给成员的约 150 处报错（额度 / 排队 / 限流、生图参数、参考图、上传格式、图片工具、流式出图、NovelAI 返回的错误、鉴权）统一改写：先用一句话说发生了什么、为什么，再说成员能做什么；凡是没出图的都写明「没有扣费」；字段名这类只给客户端作者看的内容放到句末「给客户端作者：…」后面；「上游」统一说成「NovelAI」或「本站的 NovelAI 账号」，并注明「不是你的额度问题」。状态码、原因码、`{"statusCode", "message"}` 格式都不变。
- 登录回调的 state 改成带签名、10 分钟有效，Discord 内置浏览器丢了 Cookie 也能完成登录（此前日志里的 `state_ok=n`）。

### 站长 / 运维

- **删除全部自动额度算法（站长 10-23）。** 删掉：V4.5 每人上限自动调整（`quota_ceiling` 等）、新号爬坡（`newbie_ramp_*`）、按电量定的 V5 当天方案与临时加量 / 自动暂停（`quota_v5_day_plan` / `quota_v5_bonus` / `quota_v5_paused`）、动态额度总开关 `quota_auto_enabled` 与每日复盘、账号日上限自动版（`quota_account_caps` / `quota_account_cap_auto`）、出图间隔 AIMD 的四个 `guard_interval_*` 参数、V5 Medium 的 `auto` 档、后台接口 `/admin/api/quota-algo`。旧设置行留在库里不再读写，没有破坏性迁移。新增手动设置 `v5_paused`；`register_daily_images`（没设过 150）/ `register_daily_v5` 改由「额度与保护」页编辑，「领 Key」页不再能改。定时任务「动态额度」改名「额度同步」，只把手动值同步给 `quota_auto=1` 的 Key 和首页文案。后台「额度与保护」页改成一张张手动表单，加一张只读「记录」（`/admin/api/quota-records`：各账号 V5 电量、官方 / 实测恢复速度、Anlas、今天 / 昨天出图量）。`nai-ops manual` 只剩 `guard_token_daily_caps` 和 `v5_paid_tokens`。
- **后台再瘦一圈（站长 10-23：日志留全，要看什么让 Claude 查）。** 删掉：总览页（`/admin` 直接是成员页；`/admin/api/overview` 只剩「上游」页用的令牌池两项，`/admin/api/status-codes` 删除）、上游页的「上游表现」（`/admin/api/perf` 和 `perf` 分析）、V5 余量里的「恢复速度怎么算的」、账号对比里的本小时计数 / 分担占比 / 1 小时等待 / 到期预测、每把 Token 的「图片并发」设置（固定 1，`upstream_token_image_concurrency` 旧行不再读）、「领 Key」页的新成员每日张数和模型范围（新成员固定含 V5，每日张数跟随全站手动额度）、「V5 余量低提示线」（固定 20%）、安静时段（`guard_quiet_*`，连同按账号的最近 3 小时出图窗口）。账号保护和出图间隔 / 冷却合成一张卡。日志和数据表都不动。
- **删除网关里 NAI 工作台的残留**：`deploy/Caddyfile.production` 的 `/workbench`、`/api/*` 等转发块，`nai-ops traffic` 的 `workbench` 分组。

- **Per-account interval AIMD replaces the hourly cap and paced share (owner 10-22).** `runtime_image_min_interval` (default 15 s, now 5–120) is the floor; an upstream 429 multiplies that account's interval by `guard_interval_increase_pct` (200) up to `guard_interval_cap` (120 s), and every `guard_interval_decay_minutes` (30) without a 429 it drops by `guard_interval_decay_pct` (20 %) back toward the floor. The four are manual knobs under 账号保护与排队 → 出图间隔自动调整; defaults equal the old constants, so nothing changes on deploy. State persists with the token cooldowns (old three-item rows still load). The admin card shows each account's current interval, last 429, cooldown, waiting count and queue capacity. Removed: `guard_account_hourly_cap`, `guard_hourly_min/max/step`, `guard_daily_burst` (old rows are ignored), the `hourly_cap` job, the 429 halving, the paced 300 s wait. An interval slot taken by a request that is then refused or moved is refunded.

- **Concurrency lock vs. rate limit; fair queue; per-member queue cap (owner 10-22).** Upstream 429/409 bodies containing "Concurrent generation is locked" no longer double the account interval, cool the account, trigger the site 429 cooldown or count as up_status 429; the request keeps its slot and retries on the same account (2 s base, doubling, max 3, capped by the queue timeout), failing as `upstream_locked` (503) only when out of retries. Every upstream response is followed by a 1 s settle gap on that account. The pacing card shows "并发锁 N 次（不算限流）" per account (today, in memory). Accounts now serve waiting images by the member's images today (ascending) then arrival, with images older than half the queue timeout first by arrival. `guard_queue_per_member` (default 2, 1–5, manual) replaces `guard_key_image_queue`: it counts a member's images not yet sent upstream; refusals use the new code `member_queue_full` (half weight in the per-key guard, like `key_busy`). Public JSON keeps the field name `key_image_queue`.

- **删除 `nai-ops notify` 子命令及其 cron 示例；2.x 回滚通道关闭（站长 10-22）。** 站长私信只剩 `patrol --alert`（nai-patrol.timer）一套；`notify` 的状态文件 `/opt/backups/patrol_notify_state.json` 可手动删。`cutover.sh` 的 `up` / `rollback`、`private_params.json` 备份、库启动时对 2.x 旧表和 `discord_id` 的清理 / 回填、`upstream_token_counters.units` 的写入一并去掉。

- **后台「疑似同人」关联接口 `GET /admin/api/device-links`。** 把网页登录采集的设备指纹按 device_id 分组：同一个设备指纹下登录过 ≥2 个 Discord 账号的，连同可读分项（显卡/屏幕/时区…）和每个账号的 Key、名字、IP 一起列出。站长据此判断小号，不用再手工拼 SQL。

- **设备指纹改成「哈希 + 可读分项」。** 原来只存一个不可读的哈希，站长没法推理「哪几项一样」。现在登录前前端把可读分项（显卡厂商/型号、屏幕、时区、硬件并发、内存、平台、语言、canvas 哈希）POST 到新接口 `/public/device-fp`（同源校验、限 2KB、按 device_id 去重），存进新表 `device_fingerprints`；`device_logins` 仍按 device_id 关联账号。站长把两张表一join，就能看到两个账号「同显卡、同屏幕、canvas 也一样」还是只是碰巧同一个哈希。保留 30 天。

- **防滥用：网页登录采集浏览器设备指纹（第一批）。** 登录时前端用 Canvas+WebGL+屏幕+时区+硬件合成一个 32 位十六进制短哈希，跟着签名 state 走一圈 Discord 回来（不依赖 Cookie），写入新表 `device_logins`（同账号同设备一行，保留 30 天，维护任务清理）。用途：同一个设备指纹下登录了多个 Discord 账号 = 疑似同一个人的小号。设备指纹可选——没有或非法时登录行为和以前完全一样，记录失败也不影响登录；老的三段式 state 仍然有效。JA4/TLS 指纹、签名设备绑定令牌、工作量证明、后台关联视图属于第二批（动基础设施或热路径，单独部署）。

- **Deploy downtime is measured correctly and shorter.** The probe loop that times a deploy ran in a subshell inheriting `set -e`, so the first curl timeout (exactly when the site is down) killed it and the report claimed no downtime (22:14 on 10-21: 17 s down, reported "2 probes, all OK"). It now keeps sampling. Caddy no longer lists the unused temporary gateway `.11` or passive health checks (`lb_policy first` / `max_fails 2` / `fail_duration 10s`), which kept a freshly restarted gateway distrusted for up to 10 s; only `dial_timeout 1s` remains. `cutover.sh roll` now refuses (no `.11` in Caddy), as intended.
- **The gateway publishes queue-full and a correct ETA.** `/public/live` has `queue.full` (the same check `admit_image` refuses with, now a shared `GuardCore::queue_full`), and `me.mine[].eta` is position × (per-account interval incl. average jitter ÷ usable accounts) instead of position × minimum interval. The home page uses both instead of re-deriving them.
- Admin: the Anlas reconcile countdown stops ticking once it reaches 0. Removed the unused `UsageStat` component and its styles.

- **Anlas reconcile button stuck on "N 秒后可再次查询".** The countdown kept the clock from when the card mounted; when the cooldown had already ended its timer never started, so the button stayed disabled showing the seconds between mount and data arrival. Both times are now taken at the same moment when data arrives.

- **Deploys are stop-then-start again.** The rolling deploy let two gateways send to NovelAI at the same time; each tracks "is this account busy / when may it send next" only in its own memory, so at 20:45 on 10-21 the draining old gateway and the temporary one both sent to the same account and upstream answered 429 (180 s site-wide pause, hourly cap halved by AIMD). `tools/deploy.sh` and `cutover.sh previous` now use the new `cutover.sh restart`: the old gateway finishes its images (up to 120 s) and exits before the new one starts, so only one gateway ever uses an account; the site shows "网关自检中" for 1-2 minutes and the measured outage is printed. `roll` stays in the script, marked do-not-use.

- **The first rolling deploy still went down for ~70 s: Caddy had never loaded the new config.** Caddy mounts the single file `/opt/caddy/Caddyfile` read-only; once the host file is replaced by a new inode (editor save, `sed -i`, write-and-rename), the container keeps seeing the old file and `caddy reload` silently reloads it, so the live config had only `.10` (the 19:24 https status-page link change was not live either). New `deploy/v3/caddy-reload.sh` saves the running config, validates the host file in a throwaway container, pushes it through the admin API `/load` and checks the running config matches. `cutover.sh roll` now refuses unless Caddy's running config has both gateway addresses, checks `.11` from the host (Caddy's view) before swapping, swaps the gateway with `--no-deps` (the bot's 10 s stop no longer leaves `.10` empty), waits until Caddy trusts `.10` again before removing the temporary gateway, and prints a measured result from probing the public URL through Caddy every 0.5 s instead of claiming "no downtime". `gate_lb` adds `dial_timeout 1s`. `cutover.sh` takes `OWL_*` overrides so the whole flow can run isolated on the standby server. Drill there (compose 2.40.3, host-network Caddy, HTTPS/h2, in-flight images and SSE): the inode trap reproduced and was refused; two rolls, 452/452 probes OK each.
- **Deploys no longer take the site down.** `tools/deploy.sh` now runs `cutover.sh roll` instead of `compose up`: the new image first starts as a temporary gateway `owl-gate-next` on 172.30.0.11 and must pass healthcheck and smoke (if not, it is removed and nothing changed); then `owl-gate` is swapped while Caddy sends traffic to `.11` (the old gateway stops accepting connections at SIGTERM and drains for up to 120 s); finally the temporary gateway drains and is removed. Caddy lists both addresses with `lb_policy first`, `fail_duration 10s`, `max_fails 2` (`deploy/Caddyfile.production`, one-time change on the server). Scheduled jobs run only in the process holding `data/jobs.lock` (flock); a second gateway serves requests and takes over the jobs when the first exits. `cutover.sh previous` uses the same path. Images carry the label `owl.jobs-lock=1`; without it `roll` falls back to stop-then-start. Local drill: two handovers, 376 request pairs, all 200, slowest 1.0 s.
- **Every 4xx rejection carries its reason code at the source.** Image parameter checks, image tools, stream format, the keystore endpoint and body-size errors are `param` (upload interrupted stays `body_abort`); img2img without Anlas and feature checks are `feature_off`; 「标签补全现在请求太多」 is `queue_full` and its timeout `queue_timeout` (site-level, not the member's fault). Disabled and expired keys get the new code `key_disabled`, missing / invalid keys `key_invalid`; both are in `KEY_GUARD_SKIP_REASONS`, so the per-key guard no longer re-pauses keys that are already off. A test scans `crates/gate-server/src` and fails on any `GateError::new` with a 4xx status.
- **Backfill patterns are precise.** `%暂停%` matched breaker, cooldown and quiet-hour texts and filed them as `key_paused`; key pauses now match only their own wording, and the breaker, cooldown, quiet-hour, daily-cap and paced texts have their own patterns. Quota patterns follow the current wording (「全站今天的 V5 额度被用完了」, 「你今天的 V5 额度用完了」, …) with the old ones kept for history. The write-time fallback `classify` now applies only to `status='rejected'`, the same scope as the startup backfill.
- **Per-key guard counts `key_busy` at half weight.** When the site is congested the queue gets longer and clients resend before the previous image finishes; those rejections now count 0.5 each (`KEY_GUARD_HALF_WEIGHT`), so a client really looping still trips the guard at twice the count.
- **V5 images per 1% of the meter: 14.2 → 15.5.** `V5_IMAGES_PER_PERCENT` was the official 17.3 per 1% scaled to 28 steps. Back-calculating from key1's meter snapshots and V5 counts gave 16.8 (10-19) and 15.5 (10-20); the lower measured value is used. The site V5 pool grows by the same ratio (5 %/day × 1 account: 71 → 77). The admin quota card's fallback follows.
- **Per-key guard threshold: 60 → 120 rejections per hour** (`KEY_GUARD_REJECTS`). With `key_busy` at half weight a looping resend now trips it at 240.
- **Self-service keys get 10 requests per minute instead of 5** (`REGISTER_KEY_RPM`). This is the default for newly issued keys; keys already issued keep the value stored on the key (`api_keys.rpm`), the code does not rewrite them.
- **Upstream snapshots record the real Anlas balance.** `upstream_snapshots.anlas` was always 0; it is now the sum of `fixedTrainingStepsLeft + purchasedTrainingSteps` from the accounts' last subscription reads, or NULL when none was read.
- **Admin log and quota card say "手动" or "自动".** Every operation-log row carries `kind` (`manual` = owner or Claude: actors starting with 站长 / 后台 / 手动 / Claude / 运维; `auto` = 系统 / 自动驾驶; member rows untagged) and shows a 手动 / 自动 badge; the 操作者 filter is now 全部 / 手动 / 自动 / 成员自己 (`actor=manual|auto|member`; old `admin` / `system` links still work). The 动态额度 card splits today's V5 into "算法 X + 手动加 Y" instead of folding the temporary raise into the formula, shows a hand-edited plan (`manual` in `quota_v5_day_plan`) with who, when and why, and lists live manual raises of per-account daily caps and site-paid V5 (`GET /admin/api/quota-algo` → `manual`, accounts by number only).
- **`nai-ops manual` replaces direct database edits.** New `POST /admin/api/ops/manual` validates and writes `quota_v5_bonus`, `guard_token_daily_caps`, `v5_paid_tokens` (or patches `global` / `each` of today's V5 plan and stamps `manual`) together with its operation-log row in one transaction (`Db::set_setting_logged`); `why` is required, every entry needs a future `until`, accounts must be in the pool. Per-account cap changes take effect without a restart; plan patches resync members at once. Without a key it only records a row. `nai-ops` sends `x-owl-actor: claude` on every request, so `set` and `manual` are logged as 手动（Claude） instead of 后台 + masked IP.
- **Every rejected request is logged with a reason code.** Some rejection paths (image parameter checks such as abnormal weights or the model whitelist, and the NovelAI queue timeout) wrote `usage_log.reason` empty; the codes only appeared after the next restart's backfill. Rows written in between were counted as member-caused by the per-key guard even when the cause was the site (queue timeout). The queue timeout now carries `queue_timeout` at the source, and any rejection still without a code is classified at write time with the same patterns as the backfill.
- **Disabled keys and a few parameter errors now get a reason code too.** Requests with a key stopped by the site-wide reset or by the owner, a missing model, Vibe on an unsupported model, and an invalid precise-reference type were still logged without a code because no pattern matched their wording. They are now `feature_off` and `param`.
- **Home page cards no longer sit on the hero's edge.** The grain behind the hero ended in a hard line that ran through the first row of cards; it now fades out, and the cards start 24px below the hero.
- **No more "Transition was skipped" errors in the console.** When the browser skips a cross-page transition, its promises reject; the head scripts of the home and status pages now catch them.
- **Gateway and bot logs survive deploys.** Both containers log to journald instead of json-file, so a recreated container no longer takes the day's logs with it (`journalctl CONTAINER_NAME=owl-gate`).
- Rejected image requests posted to the root path `/` are logged with kind `image` instead of `account`.
- **`tools/watch-prod.py` replaces `tools/watch-prod.sh`.** Besides liveness it reads the usage log: repeated identical failures per key, rejection texts not seen in a week, retry storms, the time the site total or V5 runs out, accounts at their cap, queue timeouts and waits, missing V5 plan. `--replay` re-runs the checks over a past window.
- **Workflow: no pull requests, one-command deploy.** Commit straight to `main`; `tools/deploy.sh --wait` waits for CI and ships the green build (refuses red runs, skips a build already live, syncs `cutover.sh` / `compose.yml`, verifies healthcheck, smoke and public `/healthz`). CI no longer runs on pull requests and builds artifacts in parallel with the checks. Git hooks in `tools/hooks` keep commit messages English and free of IPs, Discord IDs and key numbers. `tools/watch-prod.sh` and `tools/clean-targets.sh` move into the repo; debug builds keep line tables only. See `docs/WORKFLOW.md`.
- **数据库读请求排队不再占线程。** 以前每个读请求先占一个阻塞线程，再在里面每 2 毫秒轮询一次空闲的读连接（只有 2 条）；晚高峰线上网关同时有 37 个阻塞线程（2 核机器），白占 CPU，每个线程还各留一份分配器缓存。现在读请求先在异步队列里等许可，同一时刻最多 2 个线程在读库；查询结果不变。本地 64 个并发读：执行查询的线程从 64 个降到 ≤ 6 个。
- **保留时长都在后台「生成记录」里调。** 原有的提示词保留天数、原图保存时长之外，新增「来源 IP 和客户端保留」（设置 `usage_origin_keep_days`，1～365 天，默认 14，和以前一样）；维护任务和成员详情的「最近来源」都按它算。站长 10-18 定原图改为 24 小时：在后台把「原图保存时长」设成 24 即可。
- **站长查看生成记录会留痕。** 打开生成记录列表（每一页）、查看原图、导出原图，各记一条操作日志（「查看生成记录」「查看原图」「导出原图」）。
- **备份里不再有生成记录。** 新增 `deploy/v3/ops/naigate-strip-prompts`：把快照里的 `generation_audit`（提示词、参数）整表清掉并 VACUUM，完整性检查通过才替换。本机每日备份（`naigate-backup`，最长留 6 个月）和 S3 加密备份（`nightly_backup.sh`）都先过它。需要手动装到服务器 `/usr/local/bin/`，并对已有的本机备份跑一次。
- **脱敏只读快照里的 IP 打码。** 用量日志从 10-18 起存完整 IP，`nai-ops ro-sanitize` 现在把它打成前两段（`a.b.*.*`，IPv6 留前两组），和 2.x 时一样。
- **周报的小号 / 共用筛查改成按 Discord 账号算**（`nai-ops report`，只读）。以前「多把 Key 共用同一设备 UA」按 Key 统计：同一个人换过的新旧 Key 也算「共用」，一个常见的 Windows Chrome UA 一次报出 40 把 Key，噪音太大。现在同一账号的新旧 Key 合并；只把窗口内被 ≤ 3 个账号用过的 UA 算「罕见设备」，只报共用 ≥ 2 个罕见设备、或「同一完整 IP + 同一 UA」的不同账号对。新增「提示词相似且设备或 IP 有重合」一节：按逗号切词算 Jaccard，以全站两两相似度为基线，≥ p99.9 并且有设备或 IP 重合才报，附两边的活跃小时数和同一小时都活跃的小时数。输出仍然只有 Key 编号、显示名和数字。10-18 线上只读核对：原来 70 多组「共用设备」缩到 3 对，40 把 Key 的大组不再出现；10-18 人工筛查认定的那一对强嫌疑两节都报出（相似度全站前 0.01%，p99.9 = 0.284）。
- **删 Key 前留下「Key → Discord 账号」**：新表 `deleted_key_owners(key_id, discord_id, name, deleted_at)`（只增不减，回滚到 2.x 不受影响），闲置回收、撤销、站长删除都经过 `delete_key`，删之前写一行；跟日志里的 UA / IP 一样保留 14 天，由每天的维护任务清理。以前回收后这些 Key 的用量日志就再也对不上账号（10-18 时 14 天内 767 行、37 把 Key）。

- **巡检新增「V5 方案输入核对」**（`nai-ops patrol`，只读，最多「注意」）：用 `usage_log`（网关算额度用的是 `counters`，这里换一张表）独立重算前 3 个完整服务日用过 V5 的成员数，和当天 `quota_v5_day_plan` 的分池人数比；再用最近 3 天出过图的上游账号数给全站池一个上界。不一致、或服务日开始 30 分钟后还没有当天方案时提醒；算法关着 / 还没有方案时写 n/a。起因：3.0 之后几次额度 bug（池按账号取平均、付费 V5 当免费）都出在取数，公式有 proptest，取数没有第二个来源。10-18 线上核对：活跃 68 人两边一致，池 224 = ⌊7.9 × 2 × 14.2⌋。
- **`/healthz` 真的检查数据库了。** 以前无条件回 200：写库线程坏了、库读不了，Docker、`cutover.sh`、外部探测和巡检照样全绿。现在读、写两条路各在 1 秒内跑通一次 `SELECT 1` 才回 200；不通回 503 `{"ok": false, "reason": "db_read" | "db_write"}`。正常时的返回不变（`{"ok": true, "upstream": true}`）。（审查 2026-10-17 R1）
- **写库线程不会再因为一次 panic 永久失效。** 以前某个写任务 panic 会让写线程退出，之后所有记账、额度、审计的写入都静默失败，直到重启。现在那次事务回滚，调用方收到错误，写线程换一条新连接继续处理队列，并写一条错误日志。只读连接池同理：查询 panic 后连接照常还回池子（以前漏两次之后所有读都会卡住）。（R2）
- **上游账号的 429 / 403 冷却跨重启保留。** 以前冷却只在内存里，一次部署或重启就会立刻去试探刚被隔离的账号，403 退避档位也从 5 分钟重新算。现在冷却变化时存进设置 `upstream_token_cooldowns`，启动时恢复：1 小时内的冷却照旧生效，24 小时内的 403 档位接着算。只会更保守，不会提前放行；不改正常出图的节奏。（M-4）

- **后台「成员」页的筛选合成一条横栏。** 以前是一排页签加「更多筛选」下拉，现在只有一排标签（名称 + 实时人数，选中高亮，再点取消）：全部成员、今日活跃、还没出过图、24 小时新加入、24 小时被拒过、快被回收（满 N 天未生图，N 取自「闲置回收」设置，回收关闭时不显示）、邀请 Key、需要处理。选中「24 小时被拒过」时，被拒原因（传了底图、请求过快……）和人数紧跟在它后面同一行展开，再点一个原因进一步缩小；窄屏整条横栏横向滚动，不换行。「需要处理」保留：它有真实含义（Key 过期 / 将到期、被限流暂停、站长标了「疑似 / 待查 / 观察」、被拒次数多、今天被手动重置多次），当前为 0 只是没有这类成员。
- 去掉的筛选：「身份组：普通 / 站长 / 测试」（和 Discord 身份组门槛重名且没法据此操作）、「后台手动建的 Key」「站长 / 测试 Key」（站长和测试 Key 仍不计入成员人数，从总览搜索 / 跳转 `?focus=` 仍能看到）、站长标签筛选（标签仍可在搜索框里搜）。老链接 `?filter=role:invite` 自动当作「邀请 Key」。纯前端改动，接口不变（`GET /admin/api/members` 本来就带 `inactivity_days`）。

- 新拒绝原因码 `test_request`（400，短说明「连接测试（未出图）」）：用量日志记 `rejected`，不计每日张数 / V5 / Anlas / 上游 / 队列 / 账号上限。归类为「成员自己的拒绝」，计入单 Key 限流（1 小时内 ≥ 60 次才暂停，正常试几次远远碰不到）。管理员 Key 和测试 Key 豁免；没有新设置项。阈值在 `policy.rs` 的 `TEST_REQUEST_*` 常量，依据是线上 7 天 12,650 张成功出图里没有任何正常请求同时满足「步数 ≤ 5 且面积 ≤ 512x512」。
- 新表 `role_verifications(discord_id, guild_id, role_id, verified_at)`，只增不减，回滚到 2.x 不受影响；机器人不再需要在门槛服务器里，`/guilds/{id}/members/{uid}` 的机器人查询已删除。
- 设置不变：`register_role_guild` / `register_role_id` / `register_role_note`；上线前仍需站长在后台填写。门槛配齐（服务器和身份组都非空）后 `/login` 一律走验证模式，`/login?verify=1` 保留兼容。
- `/public/status`、`/public/live` 的 `registration` 里新增 `role_note`（只有门槛配齐才有，只含后台填的名称，不含服务器 / 身份组 ID），首页被挡时用来说明缺哪个身份组。
- **用量日志记下由哪个上游账号处理。** `usage_log` 新增可空列 `token_id`（`ALTER TABLE ... ADD COLUMN`，只增不减，回滚到 2.x 不受影响；不建索引）。选中上游账号后写进请求上下文，成功 / 出错 / 选号之后才被拒的日志都带上；准入阶段就被拒的和升级前的老行为 NULL。后台用量日志每行多一个 `token_id`，成员接口看不到。
- **重启后的每小时窗口按账号补回**（`seed_hour_by_account`）：每个账号只用自己的行；`token_id` 为 NULL 的老行——只有 1 个账号时算给它（不少算），2 个及以上账号时不算给任何账号。升级后的第一次重启，3 小时内的老行不会补回（之后精确）。旧版不分账号的 `seed_hour` 只留给对拍快照。
- **后台「上游」页新增「账号对比」卡片**（`GET /admin/api/upstream-stats`，只读、仅管理员、不向上游发请求）：每个账号最近 1 小时 / 24 小时的成功 / 出错 / 选号后被拒、平均与 p90 等待、上游 429 / 403 / 401 次数，今日张数、每小时计数、请求与余额的分担占比。
- **Anlas 余额与续费倒计时（推算）**：最近一份对账快照（固定 + 购买）减去快照之后账本里记下的 Anlas，给出估计余额、续费日期与剩余天数、近 3 天日均用量、到期预测（「到期前用完」或「到期预计浪费 N」）、上次核对距今。快照里的到期时间已过则只提示「周期已滚动，需要重新核对」，不猜；上次核对超过 3 天提示偏旧。2 个及以上账号时老行归不到账号，余额标「近似」。先扣固定额度再扣购买的是推测，未经官方核实。
- 开发：`cargo xtask dev` 的演示用量日志带 `token_id`，并写入一份一天前的对账快照；快照测试里 `usage_log` 多一列，种子 SQL 与 `list_logs` 输出相应更新（只多了 `token_id`）。

### 站长 / 运维（运维工具）

- **`nai-ops patrol` 扩展检查**（只读，输出格式与退出码不变）：近 35 分钟排队健康（等待均值 / p90、拒绝比例、`queue_full` / `queue_timeout`）、各上游账号出图占比（需要 `usage_log.token_id`，没有该列显示 `n/a`）、Anlas 估算（最近对账快照余额减去之后的记账，标注近似；对账超 3 天、到期不足 3 天、到期前会耗尽时提醒）、原图目录大小对比 `audit_image_cap_mb`、Caddy 证书剩余天数、领了 Key 但身份组未授予的人数、近 1 小时上游 401 / 403 / 429、网关刚重启的提示。危险只留给证书 < 5 天、原图 ≥ 上限 150%、上游 401 + 403 ≥ 5、排队长期堵死。
- **`nai-ops notify`**：整体级别变化（正常 ↔ 注意 / 危险）时给站长发一条 Discord 私信；同级别 60 分钟冷却，危险不解除 6 小时提醒一次，恢复只发一次，`--dry-run` 只打印。附 `deploy/v3/ops/cron.d-nai-notify.example`（只是示例，不自动安装）。不私信成员。
- **`nai-ops set` / `get`**：后台设置接口的薄客户端（白名单，`set --list`），打印 改前 → 改后，没有 `--yes` 不写；校验、操作日志和内存刷新与后台页面一致。`issue_hourly_cap` 因后台没有接口而明确不支持。
- **`nai-ops report`**：只读周报（重试轰炸、最长连续使用、额度打满天数、多设备、共用 UA、相同长提示词、新号领取、领取突发、现有暂停）；输出只有 Key 编号、显示名和计数。内容风险词表放服务器外部文件，仓库只带空示例 `deploy/risk-terms.example.txt`。
- **`nai-ops traffic`**：汇总 Caddy JSON 访问日志（路径分组、状态码、p50 / p95、每分钟峰值），不读 IP、不输出查询参数；附 `deploy/Caddyfile.access-log.example`（需运维手工应用）。
- **`deploy/v3/cutover.sh`**：`load` / `up` / `previous` / `rollback` 之前先看最近 10 分钟出图数，超过 `CUTOVER_BUSY_THRESHOLD`（默认 30）或落在 `CUTOVER_BUSY_HOURS` 内拒绝执行，`--force` 放行。

## [3.0.0] - 2026-10-15（Rust + Preact 全量重写）

网关、Discord 机器人、运维脚本全部用 Rust 重写，前端用 Vite + TypeScript + Preact 重写。
数据库只增不减，回滚窗口（到 10-22）内随时能回滚到 2.x。下面按主题列出相对 2.x 的改动。

### 成员可感知

1. **额度只剩每人每日上限。** 每天 7 点重置，用满回 402。
   - V4.5：每人上限 A 由动态额度每天 7 点后定一次；删除「保底 + 全站空闲时借用」和「每小时补 4 张」的注水，当天就能用满 A。
   - V5：全站池 = ⌊恢复速度 × 14.2⌋，每人 = 全站池 ÷ max(10, 前 3 天用过 V5 的人数)，向下取整、至少 10 张（10-17 起，原为至少 1 张）；当天第一次计算时定下，当天不变。删除电量系数、3～30 张夹取、洪峰放宽和白天收紧。
   - 账号每日上限按实际张数算，不再按「算力折算」。
2. **每把 Key 只剩两层限速：排队和每分钟次数。**
   - 同时生成 1 张、排队 1 张，多发的退回。
   - 删除每 Key 15 秒出图间隔、同时在途 ≤ 4、每个账号的 10 分钟突发窗口；账号级的出图间隔、每小时上限、安静时段不变。
3. **Key 的生命周期。**
   - 停用时说明原因：全站重置停掉的提示去 `/register` 重新领，站长停用的提示联系站长。
   - Key 默认不过期。
   - 闲置回收只认真正出图（放大、导演工具、Vibe 编码不算）；站点自己出不了图的日子不算；过期的 Key 不算闲置、不拉黑。
   - 邀请 Key 的持有人退群时不删 Key，由站长手动处理。
   - 闲置回收拉黑的人再来领 Key，提示写明原因（连续几天没出图、Key 已被自动回收），不再写「已被站长停用」。
4. **删除的成员功能。**
   - 首页「作品备份」（自助导出原图 ZIP / EPUB）。
   - V5 续杯：个人 V5 用完就是 402，不会再悄悄改花 Anlas。
   - 文本生成、语音、OpenAI 兼容聊天、模型列表整套接口，以及功能里的「文本」「语音」两项；`/v1/me` 保留。
   - `/v1/live/me`（凭 Key 查排队位置）；首页的排队信息不受影响。
5. **奶妹。**
   - 只剩 `/register`、`/quota`、`/resetkey`、`/help`、`/反馈`；管理员命令和「@奶妹」自动回复删除。
   - `/反馈` 点开就是一段文字，不再先拉 5 道选择题（以前网关慢时弹窗会超时失败）。
   - `/quota` 去掉「服务状态」一行；安静时段分「暂停出图」和「出图会放慢」。
   - 领 Key 欢迎语按现行规则重写：每天 7 点重置、全站共用先到先得、V5 只有 3 种尺寸、每把 Key 排队 1 张。
   - 不私信成员：后台停用、重置、重新生成 Key、奖励等都不发私信，新 Key 由站长手动转交。
   - 出错时不再把内部报错原样发给成员。
6. **页面。**
   - 首页：登录后第一屏就是额度卡；首屏数据随页面下发，实时看板改为推送，不再每秒轮询。
   - 网页登录只用 Discord，一直开着；登录失败跳回首页提示，不再 500。问卷用 Discord 登录识别（仍认 Key）。
   - 状态页：文字对比度 ≥ 4.5:1，事件标题可点，加「返回首页」，与首页共用亮暗主题；单个成员的失败不再把全站标成「性能下降」（要 ≥ 3 次且来自 ≥ 2 把 Key）。
   - 条款与隐私政策：写明只用 Discord 登录；去掉「记录证据」「防多人共用（行为统计）」「私信通知」；图片写「7 天后自动删除」。

### 站长 / 运维

7. **上游出站。**
   - 指纹升级：TLS / HTTP2 与请求头取自同一个 Chrome（149 / 148），每把 Token 固定一套，查 V5 余量也用这把 Token 自己的指纹；`UPSTREAM_USER_AGENT`、`UPSTREAM_HTTP2` 删除。
   - 出图后静默改成后台「账号保护」里的两项设置（默认 1～3 秒，保存即生效），`POST_REQUEST_JITTER_MIN/MAX` 删除。
   - 上游 Retry-After 读不懂时按 20 秒冷却（旧版直接不冷却）。
   - 调 Discord 统一走 `/api/v10` 并带 UA（旧版告警会被 Cloudflare 拦掉）；运维命令不再写死站点域名。
8. **告警只私信重大事故。**
   - 网关只私信 5 件事：上游 Token 失效（401）、连续 403、Token 被改动、磁盘不足 10%、测试告警；巡检只有「危险」才私信，解除后补一条「已恢复」。
   - 其余（上游 429 / 5xx、V5 余量低、新 Bug、每小时上限下调等）只写日志，后台总览照常显示。
   - 通道只剩机器人私信 `ALERT_USER_ID`；`ALERT_WEBHOOK_URL`、`ALERT_CHANNEL_ID` 不再读。
9. **内核改成定时任务；自动驾驶只剩两条规则。**
   - 内核的 6 个模块改成 7 个普通定时任务：动态额度、上游 V5 快照、每小时上限自动调整、全站熔断、单 Key 限流、维护、闲置回收。周期不变，一个出错不影响别的。
   - 后台「模块」页删除，「系统」页加「定时任务」卡片（周期、上次运行、结果、摘要、耗时）；出错或超过 3 个周期没跑，侧栏和总览会提醒。动态额度开关在「额度与保护」。
   - 接口：新增 `GET /admin/api/jobs`；`/admin/api/modules*`、`/admin/api/autopilot` 删除（404）。模块开关、交叉自检、运行色条和风险检查一并删除。
   - 熔断和单 Key 限流一律执行（删掉观察 / 关闭模式和「名额 +5」）；熔断要求失败来自 ≥ 2 把非测试 Key；单 Key 限流删掉永远不会触发的「换网段 / 多客户端」规则。
   - 额度是 PVP，每人数字只是上限：删除「开注册就绪度」、所有「每人额度 × 人数 vs 容量」检查、「站长确认」按钮和「名额 × 每人 V5 超过全站」提示。
10. **Bug 追踪降噪、自动关闭。**
    - 不再记：浏览器网络失败、不是本站脚本的报错、原因在上游的 5xx（用量日志和「上游」页照常有记录）。
    - 已记下的噪音自动关闭；注意级 24 小时、错误级 7 天没再出现的也自动关闭，再出现算「复发」。
    - 500 在用量日志里写 `500 InternalError`；定时任务出错记 Bug，来源显示为「定时任务 · 名称」。
11. **后台。**
    - 重做：8 个分区、⌘K 跳转、日志服务端分页、改动实时推送；分区地址刷新不 404。
    - 成员操作不私信成员；「今日重置」按服务日 7 点算；状态页事件进操作日志；操作日志不再一件事记两条。
    - 动态额度开着时，它管的设置（新成员每日张数、V5、模型范围、全站 V5）不能手改（400）。
    - 新建 Key、一键邀请 Key 的默认值跟「领 Key」的新成员默认走，默认不过期。
    - 「V5 官方余量告警阈值」改名「V5 余量低提示线」（它不发告警）；加「Vibe 每日 Anlas 池」入口（0 = 不开放 Vibe）；每小时上限自动调整的范围和步长可在「账号保护 → 更多」里改。
    - 新增「V5 Medium 降档」（设置 `v5_medium_mode`，「额度与保护」页三选一，默认关闭，行为与以前完全一致）：开启 = V5 Full（含局部重绘）请求在估价和额度准入前改走 Medium，价格和免费额度按 Medium 算，仍占成员 V5 额度，出图记录写「站长已临时切到 V5 Medium」；自动 = 只在账号 V5 余量低于「V5 余量低提示线」时才改，回升到提示线 + 5% 以上切回，读不到余量时不改，不新增上游请求。接口：`GET/PUT /admin/api/settings` 多一个 `v5_medium_mode`（写错值回 422），GET 另带 `v5_medium_state`。
    - 删除：客户端识别与「常用设备」、导出客户端日志、Discord 页的跑图分享设置 / AI 评论 / 最近动作、「疑似同一人」逻辑、Anlas 自动分配与 V5 固定的字段、影子模式、首页「加入」按钮开关。
12. **安全和健壮性。**
    - 首页、后台、状态页的 CSP 都去掉 `unsafe-inline`；状态页加防点击劫持的头。
    - 桥接与成员接口的请求体上限 1 MiB；发 Key 放进一个事务，不留无主 Key。
    - CORS、HEAD、405 改用框架的标准行为。
    - 刻意构造的输入（非 UTF-8 公告、解析不了的 Origin、Discord 返回非 JSON、功能名写成列表）不再导致 500。
13. **运维。**
    - 部署：CI 构建镜像，服务器只加载；`deploy/v3/cutover.sh` 一条命令切换，健康检查或冒烟失败自动回滚；CI 不再推 GHCR。网关常驻内存约 50 MB（2.x 约 90 MB）。
    - 配置收成两层：环境变量只放密钥和基础设施，运行时参数在后台设置或代码常量。私密参数文件 `data/private_params.json` 不再读。
    - 删掉的环境变量（`.env` 里留着不报错、不读）：新 Key 与自助领取的默认值（`DEFAULT_*`、`REGISTER_*`）、`DISCORD_ROLE_ID`、`AUDIT_*`、时区 / 服务日 / 排队 / 防爆破类数字、免费档钳制（`SAFE_CLAMP`、`MAX_PIXELS`、`MAX_STEPS`）与 `ALLOW_IMG2IMG`、令牌池的 `NAI_TOKEN`、`NAI_TOKEN_V5_DAILY_LIMITS`、`SINGLE_IMAGE_SLOT_ENFORCED`、文本相关变量、`ADMIN_DISCORD_IDS`、`WARDEN_BOT_TOKEN`。`NAI_TOKEN_ALLOW_ANLAS` 保留。删之前先确认后台已经有对应的值。
    - `nai-ops` 只留 `patrol` / `snapshot` / `ro-sanitize` / `smoke`。巡检只查宿主机层面的事（容器、健康检查、内存、磁盘、上游 429、桥接密钥），自己出错退出码 3；冒烟的站点地址必填。
    - 关停时等当前一轮做完，状态码统计先落库；动态额度同一时刻只算一次。
    - 身份组同步每轮最多 5 个；删除每日清空自助注册；燕双鹰（执法 bot）删除，compose 去掉 `warden`；机器人缺桥接密钥时启动即失败。
14. **数据库只增不减。**
    - 新增：`error_events.auto_resolved`、`api_keys.disabled_reason` 两列，设置键 `job_last`（定时任务的上次结果）。
    - 不再使用但保留：`usage_log` 的 `client` / `src` / `ver`，`api_keys` 的 `anlas_auto` / `daily_text_tokens` 等列；`upstream_token_counters.units` 照旧按张数写。
    - 残留的设置行（保底、V5 夹取、自动驾驶模式、`kernel_last`、`module_*` 等）不读也不删。回滚窗口到 10-22。
15. **删除死代码。**
    - 没有生产调用方的函数、查询和接口全部删除（调度影子、请求计时、只有测试在调的接口等），删前逐个确认过。
    - 实验室（回测 / 日报）、画廊点赞机器人、Anlas 自动分配与 V5 固定的残留代码删除。
    - 出图路由、上游客户端按职责拆开（纯搬移，行为不变）。
16. **测试。**
    - 上线期间靠与 2.x 逐位对拍（约 15 万条用例 + 119 个冻结时钟的内核场景）守住额度、处罚和上游节奏。
    - 之后对拍退役：改成对当前输出做快照（`UPDATE_SNAPSHOT=1` 更新，CI 不许更新），加手写的规则测试和不变量（熔断、单 Key 限流、动态额度、记账对数）。表结构对拍保留到 10-22。

逐条明细见标签 `v3-deviations-final` 的 `docs/v3-deviations.md`。
