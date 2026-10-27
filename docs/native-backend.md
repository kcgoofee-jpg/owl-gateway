# Native backend and prompt metadata

The gateway returns generated images to the requesting client and never archives them. Prompt text, negative and character prompts, model, seed, sampling parameters and role coordinates are available to administrators for 30 days under **Logs → Prompt records**. The member request list remains available without image retrieval.

## Interfaces

- `GET /admin/api/prompts`: authenticated prompt metadata, filtered by `key_id` (comma-separated decimal IDs, at most 50); `page` and `per_page` (24, 48 or 96); `full=1` includes full text. Records older than 30 days are excluded even before periodic cleanup runs.
- Removed: `/admin/api/audit`, image/thumbnail endpoints, image purge and export endpoints, and `/public/me/requests/image/{request_id}`. Member request objects no longer contain `image_available` or `image_url`.
- Removed: `/admin/api/ops/audit` and image/record retention controls. Prompts are always recorded and automatically expire after 30 days.
- `GET /admin/api/ops` exposes `log_retention.origin_keep_days`; `PUT /admin/api/ops/log-retention` accepts `{"origin_keep_days": N}` where N is an integer from 0 to 365. This independent IP/User-Agent policy is edited under System. `GET /admin/api/status` contains alert status only.

## Client changes

Normal generation routes, quota accounting, streaming formats and credential configuration remain available. Clients should use standard UTF-8 JSON objects and JSON integer values for integer fields. Boolean values, numeric strings, Unicode digits, numeric underscores and nonstandard Python constants are not integer substitutes. NaN, Infinity, unpaired surrogates, unrepresentable numbers and request nesting beyond 32 levels are rejected.

JSON output uses serde_json's standard serialization. Do not compare whitespace, number formatting or object order byte for byte. Parse JSON responses instead. Validation errors use native messages; the registration bridge's HTTP 422 `detail` is a string rather than an emulated Pydantic error list.

Multipart attachments use multer's MIME parameter handling and UTF-8 text. Uploads must have complete boundaries; Latin-1 fallback and Python-specific header parsing are gone. HTTP Retry-After accepts decimal seconds or a standard HTTP date; nonstandard email date spellings fall back to the normal cooldown. Base64 uses the standard padded alphabet and rejects invalid trailing bits. MessagePack events use rmpv, text map keys and a maximum nesting depth of 64; malformed frames are rejected before forwarding. Duplicate final sample events still count once, and a stream error preserves already confirmed completed images.

## Migration and rollback

Before removing any historical images, startup writes `<database-name>.before-prompts-only/database.sqlite` using SQLite VACUUM INTO, verifies its integrity, copies `audit_images/` and writes a completion marker. The backup directory is private (0700). If backup fails, the old image-bearing table is preserved and startup fails. The database migration preserves prompt IDs, metadata and durable request links, drops image columns and compacts the live database. Live image files are removed only after that verified backup. File deletion failures fail startup and are retried on the next startup; prompt rows are retained.

Existing prompt records older than 30 days and image-only rows without meaningful prompt metadata are removed. The backup is an offline recovery archive and is never used by the gateway. Keep it until the release is verified, then delete it explicitly according to the operator's backup policy.

Rollback across this schema change requires both the previous image and the database backup: stop the gateway, preserve the failed live database separately, restore `database.sqlite` as the gateway database without carrying over the failed version's WAL/SHM files, restore `audit_images/` if needed, and start the previous image. A normal image-only rollback is insufficient after the image columns have been removed.
