# 官网生图页镜像（/image）接入方案

目标：`gate.davidzhao.top/image` 直接跑 NovelAI 官网的构建产物，后端换成本站网关。
官网是唯一基准；网关不支持的按钮点了没反应即可，不做「本站版」改造。

## 1. 现状验证（2026-10-30）

- `novelai-image-snapshot/` 里的官网构建产物（`image.html` + `_next/*`）用静态服务器跑起来后
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

## 3. 后端需要兼容的接口（已核对官网调用）

| 接口 | 官网用途 | 本站现状 |
| --- | --- | --- |
| `GET /user/data` | 登录态 + 订阅 + 设置 + keystore | 有，但缺 `keystore` / `settings` / `information` |
| `GET /user/inventory` | 道具背包 | 缺（404） |
| `GET/PUT /user/keystore` | 加密密钥库读写 | GET 直接 403 |
| `POST /user/login` | 账号登录 / accessKey 兑换 | 缺（404） |
| `POST /ai/generate-image` | 出图 | 有 |
| `POST /ai/generate-image-stream` | 流式出图 | 有 |
| `POST /ai/upscale` | 放大 | 有 |
| `POST /ai/augment-image`、`/ai/encode-vibe` | 增强 / Vibe 编码 | 无 → 按原则点击无效 |
| `GET /ai/trial-status`（text 域） | 试用额度 | 无 → 横幅已隐藏，不影响 |

登录态的关键卡点：官网前端登录后要求 `session.keystoreReady`（`loginMethod !== "sso"` 时），
否则抛 `Keystore not ready after login` 并退回未登录。因此本站 `/user/data` 必须返回
`keystore`（可为空对象表示首次创建）并把客户端 PUT 回来的密文按 Key 存下来。

## 4. 已完成（2026-10-30）

- [x] 网关补 `/user/login`、`/user/inventory`、`/user/keystore`（GET+PUT）、`/user/clientsettings`（GET+PUT）、
      `/ai/trial-status`；`/user/data` 现在返回 `keystore` 与 `settings`。
- [x] 新表 `key_keystores`、`key_settings`（都只存客户端加密后的不透明内容）。
- [x] 镜像资源接进网关：`/_next/*`、`/js/*`、`/icons/*`、`/cf-fonts/*`、`/manifest.json`、`/bridge.js`
      → `dist/nai-app/*`；`/image` 用单独的 `IMAGE_CSP`（放开内联脚本、wasm、worker、`data:`）。
- [x] 旧工作台 UI 整块删除：`web/src/pages/workbench/**`、`web/src/lib/history-store.*`、`workbench.html`、
      Vite 的 workbench 入口与 `?preview=1` 注入、`e2e/workbench.spec.ts`。
- [x] `/workbench` → 308 `/image`；首页顶栏与入口卡改指 `/image`。
- [x] 端到端验证：登录态正常、许可弹层接受后持久化、`POST /ai/generate-image-stream` 走本站网关成功出图，
      画布/工具条/历史轨都是官网原生行为。

## 4.1 静态资源怎么上线

镜像资源 59MB（418 个 chunk + 433 张 webp + tokenizer），**不进 git、不进二进制**：

- 本地：`STATIC_DIR=$PWD/mirror cargo xtask dev`（`mirror/nai-app/` 已加进 .gitignore）
- 线上：compose 里 `STATIC_DIR=/app/static` + 只读挂载 `/opt/owl/nai-mirror:/app/static`，
  部署后用 `rsync -a mirror/nai-app/ host:/opt/owl/nai-mirror/nai-app/` 同步
- `Assets::read()` 先查磁盘再查内嵌，所以热修和整包都走同一条路

## 5. 待办

- [ ] 镜像资源瘦身：`dist/nai-app` 现在 51MB（418 个 chunk + media），可只保留 /image 用得到的部分。
- [ ] e2e 按新页面重写（现有 `release.spec.ts` 只留了「入口跳到 /image」的最小断言）。
- [ ] 首页剩余的旧出图组件（`landing/Paintbox.tsx`、`lib/nai-config.ts` 里的表单部分）随下一批清理。
- [ ] 部署：镜像要与 Rust 一起进镜像（`web/dist` 已有），`STATIC_DIR` 热修路径同时支持 `nai-app/`。
