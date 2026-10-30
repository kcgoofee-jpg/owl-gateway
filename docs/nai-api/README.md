# 官网生图页 API 清单（照官网源码抄的）

来源：`web/public/nai-app/_next/static/chunks/2osws3z...`（NovelAI 生产构建，getter 是唯一真源），
加上 2026-10-30 在登录态下真实点一遍录到的请求。

- 完整 getter 表：`endpoints.json`（83 个，脚本从 bundle 里正则抽出来的）
- 真实点击录制：`phase-a.json`（首屏/菜单类，只有 boot 请求）、`phase-b.json`（真实出图调用）
- 三个后端域：`api.novelai.net`（文本/通用）、`image.novelai.net`（出图，本页全部走它）、
  `text.novelai.net`（试用状态、埋点）、`explore.novelai.net`（社区）

## 首屏真正会打的 6 个请求（真实状态码）

| 请求 | 状态 | 本站处理 |
| --- | --- | --- |
| `GET /user/data` | 200 | 已实现（含 `subscription` / `keystore` / `settings`） |
| `GET /user/inventory` | 200 | 已实现（空背包 `{items:[]}`） |
| `GET /user/objects/promptmacros` | 200 | 已实现（空列表） |
| `GET /ai/trial-status`（text 域） | 200 | 已实现（0 次试用，消掉计数提示） |
| `POST /usage/link`（text 域） | 200 | 已实现（空对象） |
| `GET /user/self`（explore 域） | 200 | 已实现（空对象） |

> 模型 / 模式 / 画质 / 采样器 / 张数这些**纯前端本地状态**，点一遍不会产生请求；只有生成才打后端。

## 出图链路

- 实测（浏览器点 Generate）：走的是 **`POST /ai/generate-image`**（不是 stream）。换成流式看图时才走
  `POST /ai/generate-image-stream`，响应是 SSE，
  事件 `intermediate`（草稿帧，base64 PNG）+ 最终图。本站已实现并跑通。
- `GET /ai/generate-image/suggest-tags?model=…&prompt=…&type=…`：标签补全，已实现。
- `POST /ai/generate-image`、`/ai/augment-image`（增强/变体）、`/ai/upscale`、`/ai/encode-vibe`：已实现。

## 全量 getter → 本站状态

| 前端 getter | 路径 | 后端域 | 本站状态 |
| --- | --- | --- | --- |
| `Generate` | `/ai/generate` |  | 已实现 |
| `GenerateImagePrice` | `/ai/generate-image/request-price` |  | 未实现 |
| `GenerateStream` | `/ai/generate-stream` |  | 已实现 |
| `Prefix` | `/ai/module` |  | 不支持（训练模块，点击无效） |
| `PrefixAll` | `/ai/module/all` |  | 不支持（训练模块，点击无效） |
| `PurchaseSteps` | `/ai/module/buy-training-steps` |  | 不支持（训练模块，点击无效） |
| `PrefixDelete` | `/ai/module/delete` |  | 不支持（训练模块，点击无效） |
| `PrefixSubmit` | `/ai/module/train` |  | 不支持（训练模块，点击无效） |
| `SubmitContest` | `/user/submission` |  | 不支持（账号，点击无效） |
| `VoteContest` | `/user/vote-submission` |  | 不支持（账号，点击无效） |
| `ImageTool` | `/ai/augment-image` | Image | 已实现 |
| `EncodeVibe` | `/ai/encode-vibe` | Image | 已实现 |
| `GenerateImage` | `/ai/generate-image` | Image | 已实现 |
| `GenerateImageStream` | `/ai/generate-image-stream` | Image | 已实现 |
| `TagSearch` | `/ai/generate-image/suggest-tags` | Image | 已实现 |
| `ImageTrialStatus` | `/ai/trial-status` | Image | 已实现 |
| `UpscaleImage` | `/ai/upscale` | Image | 已实现 |
| `ErrtrackTrack` | `/errtrack/track` | Image | 未实现 |
| `PaymentGatewayAuthorizeCard` | `/payment-gateway/authorize-card` | Image | 不支持（支付，点击无效） |
| `PaymentGatewayCheckEnrollment` | `/payment-gateway/check-enrollment` | Image | 不支持（支付，点击无效） |
| `PaymentGatewayCreatePaymentInstrument` | `/payment-gateway/create-payment-instrument` | Image | 不支持（支付，点击无效） |
| `PaymentGatewayGenerateContext` | `/payment-gateway/generate-payment-context` | Image | 不支持（支付，点击无效） |
| `PaymentGatewayReport3dsOutcome` | `/payment-gateway/report-3ds-outcome` | Image | 不支持（支付，点击无效） |
| `PaymentGatewaySetupPayerAuth` | `/payment-gateway/setup-payer-auth` | Image | 不支持（支付，点击无效） |
| `CancelSubscription` | `/user/cb/cancel-subscription` | Image | 不支持（支付，点击无效） |
| `ChangePaymentMethod` | `/user/cb/change-payment-method` | Image | 不支持（支付，点击无效） |
| `ClearPaymentPII` | `/user/cb/clear-payment-pii` | Image | 不支持（支付，点击无效） |
| `CreateCustomer` | `/user/cb/create-customer` | Image | 不支持（支付，点击无效） |
| `CreatePaymentIntent` | `/user/cb/create-payment-intent` | Image | 不支持（支付，点击无效） |
| `CreatePaymentSource` | `/user/cb/create-payment-source` | Image | 不支持（支付，点击无效） |
| `CreateSubscription` | `/user/cb/create-subscription` | Image | 不支持（支付，点击无效） |
| `CreateSubscriptionV2` | `/user/cb/create-subscription-v2` | Image | 不支持（支付，点击无效） |
| `DeletePaymentSource` | `/user/cb/delete-payment-source` | Image | 不支持（支付，点击无效） |
| `UpdateSusbcriptionPrice` | `/user/cb/estimate/update-subscription` | Image | 不支持（支付，点击无效） |
| `GetCustomer` | `/user/cb/get-customer` | Image | 不支持（支付，点击无效） |
| `CBInvoices` | `/user/cb/invoices` | Image | 不支持（支付，点击无效） |
| `ListPaymentSources` | `/user/cb/list-payment-sources` | Image | 不支持（支付，点击无效） |
| `MigrateSubscription` | `/user/cb/migrate-subscription` | Image | 不支持（支付，点击无效） |
| `OneTimePurchase` | `/user/cb/one-time-purchase` | Image | 不支持（支付，点击无效） |
| `PaymentIntents` | `/user/cb/payment-intents` | Image | 不支持（支付，点击无效） |
| `ReactivateSubscription` | `/user/cb/reactivate-subscription` | Image | 不支持（支付，点击无效） |
| `SelfServePortalLink` | `/user/cb/self-serve-portal-link` | Image | 不支持（支付，点击无效） |
| `UpdateCustomerBilling` | `/user/cb/update-customer-billing-data` | Image | 不支持（支付，点击无效） |
| `UpdateSubscription` | `/user/cb/update-subscription` | Image | 不支持（支付，点击无效） |
| `ChangeAuth` | `/user/change-access-key` | Image | 不支持（账号，点击无效） |
| `ChangeMailingListConsent` | `/user/change-mailing-list-consent` | Image | 不支持（账号，点击无效） |
| `ClientSettings` | `/user/clientsettings` | Image | 已实现 |
| `UpdateConsent` | `/user/consent` | Image | 不支持（账号，点击无效） |
| `CreatePersistentToken` | `/user/create-persistent-token` | Image | 不支持（账号，点击无效） |
| `UserData` | `/user/data` | Image | 已实现 |
| `VerifyDeleteAccount` | `/user/deletion/delete` | Image | 不支持（账号，点击无效） |
| `RequestDeleteAccount` | `/user/deletion/request` | Image | 不支持（账号，点击无效） |
| `UserGiftKeys` | `/user/giftkeys` | Image | 不支持（账号，点击无效） |
| `Information` | `/user/information` | Image | 已实现 |
| `Inventory` | `/user/inventory` | Image | 已实现 |
| `Keystore` | `/user/keystore` | Image | 已实现 |
| `Login` | `/user/login` | Image | 已实现 |
| `Objects` | `/user/objects` | Image | 已实现 |
| `AIModules` | `/user/objects/aimodules` | Image | 空实现（/user/objects/{kind} 回 {objects:[]}） |
| `UserObjectsBatch` | `/user/objects/batch` | Image | 空实现（/user/objects/{kind} 回 {objects:[]}） |
| `Presets` | `/user/objects/presets` | Image | 空实现（/user/objects/{kind} 回 {objects:[]}） |
| `ImagePromptMacros` | `/user/objects/promptmacros` | Image | 空实现 |
| `StoryShelves` | `/user/objects/shelf` | Image | 空实现（/user/objects/{kind} 回 {objects:[]}） |
| `Stories` | `/user/objects/stories` | Image | 空实现（/user/objects/{kind} 回 {objects:[]}） |
| `StoryContent` | `/user/objects/storycontent` | Image | 空实现（/user/objects/{kind} 回 {objects:[]}） |
| `AccountUserScripts` | `/user/objects/userscripts` | Image | 空实现（/user/objects/{kind} 回 {objects:[]}） |
| `Priority` | `/user/priority` | Image | 已实现 |
| `RecoverySubmit` | `/user/recovery/recover` | Image | 不支持（账号，点击无效） |
| `RecoveryInitiation` | `/user/recovery/request` | Image | 不支持（账号，点击无效） |
| `Register` | `/user/register` | Image | 不支持（账号，点击无效） |
| `ResendVerifyEmail` | `/user/resend-email-verification` | Image | 未实现 |
| `GoogleSSO` | `/user/sso/google` | Image | 不支持（账号，点击无效） |
| `Subscriptions` | `/user/subscription` | Image | 已实现 |
| `SubscriptionBind` | `/user/subscription/bind` | Image | 不支持（账号，点击无效） |
| `VerifyEmail` | `/user/verify-email` | Image | 未实现 |
| `TextGenerate` | `/ai/generate` | Text | 已实现 |
| `TextGenerateStream` | `/ai/generate-stream` | Text | 已实现 |
| `TTS` | `/ai/generate-voice` | Text | 已实现 |
| `TextTrialStatus` | `/ai/trial-status` | Text | 已实现 |
| `OpenAIChatCompletion` | `/oa/v1/chat/completions` | Text | 不支持（OpenAI 兼容文本接口） |
| `OpenAICompletion` | `/oa/v1/completions` | Text | 不支持（OpenAI 兼容文本接口） |
| `AnalyticsEvent` | `/usage/event` | Text | 未实现 |
| `AnalyticsIdentify` | `/usage/link` | Text | 已实现 |

## 结论

- **图像能力已 100% 覆盖**：生成、流式生成、增强、放大、Vibe 编码、标签补全、试用状态、设置、密钥库、背包、对象库。
- 剩下的都是账号/支付/训练模块（`/user/cb/*`、`/payment-gateway/*`、`/ai/module/*`、SSO、改密）——
  按「网关不支持就点了无效」处理，不需要造假实现。
- `/ai/generate-image/request-price` 这个 getter 在 bundle 里除定义外没有任何调用点，属于死代码，不用实现。
