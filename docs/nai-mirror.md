# 官网生图页镜像（/image）接入方案

目标：`gate.davidzhao.top/image` 直接跑 NovelAI 官网的构建产物，后端换成本站网关。
官网是唯一基准；网关不支持的按钮点了没反应即可，不做「本站版」改造。

## 1. 现状验证（2026-10-30）

- 当时抓取的官网构建产物（`image.html` + `_next/*`）用静态服务器跑起来后
  React 正常 hydrate，三栏 UI、快速入门画廊、模型菜单、Quality/Sampler 全部可用。
  只加载 /image 需要的资源共 **154 个文件、12.5MB**（不含全站其余路由的 421 个 chunk）。
- 官网前端所有接口都从 `image.novelai.net` / `api.novelai.net` / `text.novelai.net` / `explore.novelai.net` 派生，
  所以只需在网络层改 base，不必改 bundle。

## 2. 桥接层 `bridge.js`（随 HTML 第一个加载）

1. `fetch` / `XMLHttpRequest` 的 URL 重写：四个官网域名 → 本站同源（同源才能免 CORS）。
2. 自动补 `Authorization: Bearer <member key>`。
3. 会话：写 `localStorage.session` **和** `sessionStorage.session`（官网优先读 sessionStorage），
   形如 `{auth_token, encryption_key}`；`encryption_key` 是本地历史加密用的随机 32 字节 base64。
4. 账号横幅（注册/续费/试用）按老板要求先隐藏，后续单独做。

## 3. 接口

镜像调用的域名、路径及抓包记录见 [官网生图页 API 清单](nai-api/README.md)。

## 4. 已完成（2026-10-30）

- [x] 网关补 `/user/login`、`/user/inventory`、`/user/keystore`（GET+PUT）、`/user/clientsettings`（GET+PUT）、
      `/ai/trial-status`；`/user/data` 现在返回 `keystore` 与 `settings`。
- [x] 新表 `key_keystores`、`key_settings`（都只存客户端加密后的不透明内容）。
- [x] 镜像资源接进网关：`/_next/*`、`/js/*`、`/icons/*`、`/cf-fonts/*`、`/manifest.json`、`/bridge.js`
      → `STATIC_DIR/nai-app/*`；`/image` 用单独的 `IMAGE_CSP`（放开内联脚本、wasm、worker、`data:`）。
- [x] 旧工作台 UI 整块删除：`web/src/pages/workbench/**`、`web/src/lib/history-store.*`、`workbench.html`、
      Vite 的 workbench 入口与 `?preview=1` 注入、`e2e/workbench.spec.ts`。
- [x] `/workbench` → 308 `/image`；首页顶栏与入口卡改指 `/image`。
- [x] 端到端验证：登录态正常、许可弹层接受后持久化、`POST /ai/generate-image-stream` 走本站网关成功出图，
      画布/工具条/历史轨都是官网原生行为。

## 4.1 静态资源怎么上线

官网镜像作为外置静态资源部署。

- 本地：`STATIC_DIR=$PWD/mirror cargo xtask dev`（`mirror/nai-app/` 已加进 .gitignore）
- 线上：compose 里 `STATIC_DIR=/app/static` + 只读挂载 `/opt/owl/nai-mirror:/app/static`，
  由 `tools/sync-mirror.sh` 将版本库中的桥接脚本合入本地镜像并同步到服务器
- `Assets::read()` 先查磁盘再查内嵌，所以热修和整包都走同一条路

## 4.2 维护模式（三档）

`maintenance_mode` 现在有三种取值，后台「全站维护」卡片是三选一：

| 值 | 含义 |
| --- | --- |
| `0` / `off` | 正常开放 |
| `1` / `all` | 全部关闭（访客看维护页，接口 503，后台仍可用） |
| `2` / `owner` | 只有站长可用：只有网页登录且 Discord ID 在 `MAINTENANCE_OWNER_IDS` 里的请求放行 |

`MAINTENANCE_OWNER_IDS`（逗号分隔）放在服务器 `.env`，compose 透传；留空时 `owner` 档等同全关，防止误配置。

另：`/user/data` 对还没存过客户端设置的 Key 会返回
`{"defaultModel":"nai-diffusion-4-5-curated"}`，让官网页面默认落在 V4.5（V5 没开时不会先撞墙）。

## 5. 待办

- [ ] 镜像资源瘦身前，核查 `mirror/nai-app/` 中按需加载的 chunk、Wasm 和媒体文件。
- [ ] e2e 按新页面重写（现有 `release.spec.ts` 覆盖入口跳转、模型行和生成按钮）。

### 分享时的元数据选择

工作台分享确认弹窗默认勾选「附带生成参数」。取消勾选时，在浏览器内解码图片、清除像素最低位中的生成参数（包括 NovelAI alpha 隐写），重新编码为 PNG，不发送提示词、种子或模型等表单字段。尺寸保持不变，像素通道值可能有一个色阶的微小变化；清理失败时停止提交，不回退原图。勾选后保持原图字节和现有参数转发。取消或 Escape 不读取或上传图片。

回归验证包含 PNG 文本、WebP EXIF、JPEG 注释、透明通道参数、取消、保留原图和清理失败，测试上传使用隔离接口，不投递真实 Discord。

### 个人 Discord 频道

分享弹窗里的「分享设置」是同一个 dialog 内的二级视图（不再往工具栏另加按钮），只放勾选框：目的地、附带生成参数、记住分享选择、自动备份。记住分享选择后，点分享按记住的直接发送、不再弹确认；发送成功的提示条保留 5 秒并带「分享设置」入口，直发之后也改得动。可在设置里连接一个个人频道，使用 `identify webhook.incoming` 官方授权，复用已注册的 `/login/callback` 地址。成员在 Discord 授权页选择频道；成功后授权页尝试自动关闭，原工作台重新聚焦时刷新连接状态。连接后默认选择个人频道，可明确切回 OWL 公共频道。可查看频道、更换或断开连接；实际频道权限由 Discord 授权页校验。

`GET /public/me/share-destination` 只返回当前成员的连接状态、绑定标识及频道链接；`POST` 开始绑定，`DELETE` 断开。写操作校验来源，回调同时校验签名 state、发起浏览器 Cookie、当前成员会话及 Discord identify 身份。Webhook id/token 等信息通过 AES-256-GCM 加密保存在独立成员设置中，认证数据绑定成员身份，密钥从网关持久化会话密钥按专用用途派生；备份恢复必须同时保留原网关密钥。运行设置公开视图不包含这些凭据。

上传表单指定 `destination=personal` 并携带确认时的 `binding_id`。服务端只使用该成员已保存的目的地，从可信 Discord API 基地址拼接发送地址，不接受客户端 URL。缺少连接、绑定已变更、权限失效或 Discord 拒绝时均不回退公共频道。个人发送使用 `wait=true` 等待消息确认；超时提示先核对频道，不自动重发；429 按 Retry-After 冷却。

断开时移除本站绑定并尝试撤销远端 Webhook；若远端撤销失败，界面提示用户可在 Discord 删除对应 Webhook。更换连接后尽力撤销旧 Webhook。回滚后端时应同步恢复匹配版本的 bridge；目的地接口不可用时分享按钮保持禁用，防止错误公开发送。
