# Native backend and request records

The gateway returns generated images to the requesting client without saving images, prompt text, character prompts or detailed generation-parameter archives. Request logs retain the model, result, timings, specification adjustments and quota usage for troubleshooting and accounting. The member request list remains available without image retrieval.

## Interfaces

- Removed: `/admin/api/prompts`, `/admin/api/audit`, image/thumbnail endpoints, purge/export endpoints, and `/public/me/requests/image/{request_id}`. Member request objects do not contain `image_available` or `image_url`.
- Removed: `/admin/api/ops/audit` and image/prompt retention controls. The admin Logs page contains request and operation logs only.
- `GET /admin/api/ops` exposes `log_retention.origin_keep_days`; `PUT /admin/api/ops/log-retention` accepts `{"origin_keep_days": N}` where N is an integer from 0 to 365. This independent IP/User-Agent policy is edited under System. `GET /admin/api/status` contains alert status only.
- Anti-abuse scans no longer compare negative prompts. Device, browser, network, quota-handoff and key-sharing signals remain; obsolete prompt-derived behavior signals are cleared during migration and surviving behavior evidence is recomputed on the next scan.

## Client changes

Normal generation routes, quota accounting, streaming formats and credential configuration remain available. Clients should use standard UTF-8 JSON objects and JSON integer values for integer fields. Boolean values, numeric strings, Unicode digits, numeric underscores and nonstandard Python constants are not integer substitutes. NaN, Infinity, unpaired surrogates, unrepresentable numbers and request nesting beyond 32 levels are rejected.

JSON output uses serde_json's standard serialization. Do not compare whitespace, number formatting or object order byte for byte. Parse JSON responses instead. Validation errors use native messages; the registration bridge's HTTP 422 `detail` is a string rather than an emulated Pydantic error list.

Multipart attachments use multer's MIME parameter handling and UTF-8 text. Uploads must have complete boundaries; Latin-1 fallback and Python-specific header parsing are gone. HTTP Retry-After accepts decimal seconds or a standard HTTP date; nonstandard email date spellings fall back to the normal cooldown. Base64 uses the standard padded alphabet and rejects invalid trailing bits. MessagePack events use rmpv, text map keys and a maximum nesting depth of 64; malformed frames are rejected before forwarding. Duplicate final sample events still count once, and a stream error preserves already confirmed completed images.

## Migration and rollback

Before removing historical creative-content records, startup writes `<database-name>.before-records-removal/database.sqlite` using SQLite VACUUM INTO and verifies its integrity. Any remaining `audit_images/` files are copied into the same private backup directory (0700, database 0600). Backup or file cleanup failure stops startup before prompt rows are deleted. Retrying verifies the recovery database again. The transaction drops `generation_audit` and `audit_request_links`, removes their settings and obsolete negative-prompt signals, then VACUUM removes freed pages. Interrupted compaction is retried at the next startup. Request logs, quota counters and durable settlements remain intact.

Recovery backups, including older `before-prompts-only` archives, are offline operator archives and are never served or read by the gateway. They can still contain historical prompts and images. Delete them explicitly according to the operator's backup policy after verifying the release.

Rollback across this schema change requires both the previous image and a database backup: stop the gateway, preserve the failed live database separately, restore `database.sqlite` as the gateway database without carrying over the failed version's WAL/SHM files, and start the previous image. A normal image-only rollback is insufficient after the record tables have been removed.
