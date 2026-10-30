# 反滥用重做：设备簇 + 行为簇

> 站长 10-31 原话：「现在的反滥用系统出现 2 个或几个 ip 毫无意义，不要老是提示，重新思考反滥用系统。」
> 这份文档是重做的方案与验收口径。代码现状、改法、上线顺序、验证项四段。

## 1. 现在的做法，以及它错在哪

`abuse_scan` 每 6 小时跑一次（`jobs/abuse_scan.rs`），规则是纯函数 `gate_core::abuse::evaluate`，
结果写进 `abuse_signals` 待审队列（不自动封号）。线上 00:00 那轮的摘要：

```
可疑信号：banned_link 4，behavior 2，device_exact 23，device_fuzzy 70，foreign_origin 37，
fp_missing 38，ip_switch 7，key_concurrent 9，key_sharing 8，new_account 1，same_battery 3，
same_browser 7，same_ref_image 15
```

三个问题：

1. **网络证据当主证据**。`foreign_origin`、`ip_switch` 这类「同一个出口 IP 下冒出好几个账号」的
   规则，在 NAT、手机流量、机场 / VPN 面前等于把一批真人当成一个人。37 + 7 条里没有一条值得看。
2. **强证据被弱证据淹没**。同一个 `owl_bid` 浏览器标记绑了多个账号（23 条 `device_exact`）是
   真证据，但它和「IP 段相近」在一个平铺列表里，站长没有排序，只能逐条翻。
3. **`fp_missing` 38 条是我们自己的锅**。换成官网镜像以后前端不再上报设备指纹（那套代码只在
   旧工作台里），所以这批是「我们没收到」而不是「用户藏了」。bridge 已在
   `2026-10-31-a` 恢复上报，见 §3 第 1 步。

## 2. 新模型

### 2.1 信号按「可伪造难度」分层

| 层 | 信号 | 可伪造性 | 用途 |
|---|---|---|---|
| **A 强** | `owl_bid`（httpOnly Cookie + localStorage 双份） | 清 Cookie 才算换，且 localStorage 还留着 | 一号多开 / 共享账号的**主证据** |
| **A 强** | 设备指纹：Canvas + WebGL + 字体可达集 + 屏幕 + 时区 + GPU 厂商型号 + 硬件并发数 | 换机器才算换 | 与 `bid` 交叉验证 |
| **A 强** | JA4 TLS 指纹（`X-JA4`，`deploy/caddy-ja4/`） | 换整套客户端栈才算换，代理只换出口 | 识别「同一个脚本批量跑」 |
| **B 中** | `hfp` 请求头指纹 + UA + 语言 + 屏幕 | 换浏览器 / 改配置会变 | 聚类辅助 |
| **B 中** | 跨账号相同参考图哈希、相同种子 + 提示词 | 需要真的换素材 | 搬运同一批素材养号 |
| **C 弱** | 同 IP / 同 ASN / IP 段相近 | 一个出口几十个真人 | **只记 detail，不进提示** |

### 2.2 规则：从「两两比对」改成「簇」

设备簇（同簇 = 同一台机器 / 同一个浏览器实例；14 天窗口）：

- **D1** 同一 `owl_bid` 下 ≥2 个账号有成功请求 → 强（50 分）。
  簇里只要有一个封禁名单里的账号 → 直接置顶（不管分数）。
- **D2** 同一设备指纹（精确相等）≥3 个账号 → 强（50 分）。
- **D3** 同一 `JA4 + hfp` 组合 ≥5 个账号 → 中强（30 分）。
- **D4** 设备指纹相近（≤2 项不同）≥8 个账号 → 弱（10 分，仅记录）。

行为簇：

- **B1** 同一 Key 的并发请求来自 ≥2 个设备簇 → 共享 Key（40 分，保留现有 `key_sharing`）。
- **B2** 同一设备簇内请求覆盖 ≥20 个小时、且来源 ≥3 个城市 → 共享号（20 分）。
- **B3** 跨账号出现相同参考图哈希 ≥2 → 同一批素材养号（20 分）。
- **B4** 请求间隔的变异系数 CV < 0.05（机器节拍）→ 脚本（20 分）。

出队规则：`score ≥ 60` 才进提示（也就是**必须有设备或行为证据**）；纯网络证据永远
`alert = None`，只在 `abuse_signals.detail` 里留一行 `network: {ip, asn, accounts}` 供人工翻。
每个簇一条汇总（不是每个账号一条），带上成员账号、共同点、时间跨度、分数构成。

### 2.3 为什么这样就「不会老是提示」

- 一个出口 IP 下 20 个真人：`bid` 各异、设备指纹各异 → 0 条提示（现在会有 1 条 `foreign_origin`）。
- 一个人养 5 个小号：`bid` 或设备指纹相同 → 1 条 50 分的强提示。
- 一个脚本 50 个号：`JA4 + hfp` 全同 → 1 条 30 分，加上 B4 机器节拍 → 50 分；再撞上同一批
  参考图 → 70 分进提示。
- 清 Cookie 换号：`owl_bid` 变了，但 JavaScript 侧 `localStorage` 的备份还在，服务端两边都认。

## 3. 上线顺序

1. **设备指纹上报（已完成）**。`bridge.js` `2026-10-31-a`：页面空闲时按旧工作台同一套分项
   （ua / 屏幕 / 时区 / 硬件 / GPU / Canvas / WebGL / 字体）`POST /public/device-fp`，服务端算
   指纹并种 `owl_bid`。线上 `bridge.js` md5 `623f7198…`，`/public/device-fp` 路由为 `allow: POST`。
2. **簇模型 + 影子模式**。`gate_core::abuse` 外面加一层 `DeviceCluster` 聚合（新文件
   `crates/gate-core/src/abuse/cluster.rs`），保留 `evaluate()` 出口签名不变；新旧两套并行跑一周，
   只记 `detail`、`alert` 全关，用 `abuse_signals` 两套计数对比误报率。
3. **JA4**。`deploy/caddy-ja4/Dockerfile` 已有带 `ja3_ja4` 模块的 Caddy 构建配方，
   `Caddyfile.prod-snippet` 给出 `header_up X-JA4 {tls.ja4}` 的接法（客户端自带的 `X-JA4` 会被
   Caddy 覆盖，不能伪造）。要重编 Caddy 镜像并把 `X-JA4` 落到 `usage_log`（新列）或
   `abuse_signals.detail`。JA4H 模块是 FoxIO 非商用许可，先只上 JA4。
4. **降级旧规则**。`ip_switch`、`foreign_origin` 改成 detail-only；`fp_missing` 从提示里去掉，
   改成「链路健康度」指标（见 §4）。影子模式对比通过后，切正式、阈值按实际分布再调一轮。

## 4. 验证项（每一条都能在库里查）

| 项 | 口径 | 目标 |
|---|---|---|
| 设备指纹链路 | `usage_log` 最近 1 小时 `bid` 非空率 | > 80% |
| 指纹增量 | `device_fingerprints` 每小时新增 | > 0 |
| 断链残留 | `abuse_scan` 摘要里的 `fp_missing` | 一周内降 > 50% |
| 网络噪音 | 同 IP 多账号产生的提示条数 | 0（detail 仍记录） |
| 强信号覆盖 | `device_exact` 簇里账号数 ≥2 的比例 | 人工抽查 100% 成立 |
| 影子对比 | 新旧两套同窗口报警条数 | 新 ≤ 旧的 1/3，且新提示里至少 1 条人工确认成立 |

## 5. 不做的事

- 不自动封号：新模型只排序 + 出证据，封禁仍然人工点。
- 不用 JA4H（许可限制）先不做。
- 不引入第三方设备指纹 SDK（会把页面访问数据送到别人那里）。
