# Native backend and request records

The gateway returns generated images to the requesting client without saving images, prompt text, character prompts or detailed generation-parameter archives. Request logs retain the model, result, timings, specification adjustments and quota usage for troubleshooting and accounting. The member request list remains available without image retrieval.

## Interfaces

- Removed: `/admin/api/prompts`, `/admin/api/audit`, image/thumbnail endpoints, purge/export endpoints, and `/public/me/requests/image/{request_id}`. Member request objects do not contain `image_available` or `image_url`.
- Removed: `/admin/api/ops/audit` and image/prompt retention controls. The admin Logs page contains request and operation logs only.
- `GET /admin/api/ops` exposes `log_retention.origin_keep_days`; `PUT /admin/api/ops/log-retention` accepts `{"origin_keep_days": N}` where N is an integer from 0 to 365. This independent IP/User-Agent policy is edited under System. `GET /admin/api/status` contains alert status only.
- Anti-abuse scans no longer compare negative prompts. Device, browser, network, quota-handoff and key-sharing signals remain; obsolete prompt-derived behavior signals are cleared during migration and surviving behavior evidence is recomputed on the next scan.

## Client changes

Normal generation routes, quota accounting, streaming formats and credential configuration remain available. Clients should use standard UTF-8 JSON objects and JSON integer values for integer fields. Boolean values, numeric strings, Unicode digits, numeric underscores and nonstandard Python constants are not integer substitutes. NaN, Infinity, unpaired surrogates, unrepresentable numbers and request nesting beyond 32 levels are rejected.

JSON output uses serde_json's standard serialization. Do not compare whitespace, number formatting or object order byte for byte. Parse JSON responses instead. Validation, feature configuration, bot status and reconciliation errors use native messages and error categories rather than Python exception names; the registration bridge's HTTP 422 `detail` is a string rather than an emulated Pydantic error list.

Multipart attachments use multer's MIME parameter handling and UTF-8 text. Uploads must have complete boundaries; Latin-1 fallback and Python-specific header parsing are gone. HTTP Retry-After accepts decimal seconds or a standard HTTP date; nonstandard email date spellings fall back to the normal cooldown. Base64 uses the standard padded alphabet and rejects invalid trailing bits. MessagePack events use rmpv, text map keys and a maximum nesting depth of 64; malformed frames are rejected before forwarding. Duplicate final sample events still count once, and a stream error preserves already confirmed completed images.

## Migration and rollback

Before removing historical creative-content records, startup writes `<database-name>.before-records-removal/database.sqlite` using SQLite VACUUM INTO and verifies its integrity. Any remaining `audit_images/` files are copied into the same private backup directory (0700, database 0600). Backup or file cleanup failure stops startup before prompt rows are deleted. Retrying verifies the recovery database again. The transaction drops `generation_audit` and `audit_request_links`, removes their settings and obsolete negative-prompt signals, then VACUUM removes freed pages. Interrupted compaction is retried at the next startup. Request logs, quota counters and durable settlements remain intact.

Recovery backups, including older `before-prompts-only` archives, are offline operator archives and are never served or read by the gateway. They can still contain historical prompts and images. Delete them explicitly according to the operator's backup policy after verifying the release.

Rollback across this schema change requires both the previous image and a database backup: stop the gateway, preserve the failed live database separately, restore `database.sqlite` as the gateway database without carrying over the failed version's WAL/SHM files, and start the previous image. A normal image-only rollback is insufficient after the record tables have been removed.


## 成员规则与运行保护

成员规则只有一份默认值和按 Discord 成员 ID 保存的逐项例外。未设置或清空的例外随默认值变化；例外等于默认值时自动合并。额度使用数字，0 表示停用。Key 只保存凭据和生命周期元数据，邀请来源不会改变权限或回收条件。

| 接口 | 客户端调整 |
| --- | --- |
| `GET/PUT /admin/api/member-rules` | 读取或完整提交 13 项默认规则；响应为 `{defaults}`。 |
| `GET/PUT /admin/api/members/{discord_id}/rules` | 读取 `{defaults, overrides, effective}`；提交逐项例外，`null` 清空。 |
| `GET/PUT /admin/api/settings` | 完整提交 `{budgets:{daily_v5,daily_anlas,monthly_anlas},v5_paused,v5_medium_mode}`；缺少字段或旧字段返回 422。 |
| `PUT /admin/api/ops/registration` | 完整提交 `{open,max_users,expires_days,roles:[{guild,role,note}]}`。身份组仅决定领取资格，多条之间为“或”。 |
| `PATCH /admin/api/keys/{id}` | 仅接受 `name`、`enabled`、`expires_at`；权限改用成员规则接口。 |
| `POST /admin/api/keys`、`.../{id}/role`、`.../{id}/bind-discord` | 已移除；使用邀请凭据或 Discord 自助领取。 |

成员和 Key 后台视图使用 `policy` 返回默认规则、例外和有效规则；`invited` 仅表示邀请来源，替代 `role` 与 `role_label`。成员公开视图的 `policy` 为有效规则。删除 `quota_auto`、`quota_limits`、`image_model_scope`、身份管理员名单和全站预算豁免；客户端不应再提供“手动／自动／继承／不限”选项。订阅视图保留实际数字额度和暂停状态，删除 `*Unlimited`、`quotaLimits` 和 `imageModelScope`。所有成员都受全站预算与暂停约束，排队请求在发送上游前再次核验当前规则。

升级先检查旧规则并建立私有数据库备份，再在同一事务中写入新规则、删除旧字段和设置。历史请求与结算记录保持不变，执行结果不明的预算占用继续保留。失败时回滚，重复启动不会重复转换。新安装直接使用原生结构；存在旧结构的数据库必须经过显式网关迁移入口。


## 上游完整信息

`POST /admin/api/upstream/{token_id}/details` 只允许后台会话、同源请求和账号信息作用域的 CSRF；请求体必须为 `{}`，账号必须在当前令牌池中。它只向 NovelAI 发出 `GET /user/information`、`GET /user/subscription` 和 `GET /user/priority`，不读取 keystore、客户端设置或用户作品，不执行上游写操作。[官方接口文档](https://image.novelai.net/docs/index.html)

每个账号 60 秒内只接受一次查询，重复请求返回 429 和 `Retry-After`。服务端只保留查询时间，不缓存完整响应；正文不写入数据库或日志。各接口分别返回状态、完整元数据与错误，保留未知字段，隐藏凭据和密钥材料，不回显上游错误正文。返回使用 `Cache-Control: no-store`。

“上游”页每个账号的“完整信息”打开独立弹窗，展示账号、订阅和优先级分类。关闭、切换账号或退出后台后清除弹窗状态，不写入 localStorage、sessionStorage 或应用查询缓存。页面现有运营读数仍使用允许字段的投影缓存。
