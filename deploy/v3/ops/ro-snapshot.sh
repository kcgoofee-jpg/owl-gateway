#!/bin/bash
# 每天 05:10（cron）做一份脱敏只读快照：/opt/backups/ro/owl-ro-YYYYMMDD.db.gz，保留 7 份。
# 一致快照（naigate-snapshot）→ nai-ops ro-sanitize 去掉 Key 原文、盐、提示词、捐赠的上游 Token。
set -euo pipefail
OUT=/opt/backups/ro; mkdir -p "$OUT"; chmod 700 "$OUT"
TMP=$(mktemp -d); trap 'rm -rf "$TMP"' EXIT
/usr/local/bin/naigate-snapshot "$TMP/s.db.gz" >/dev/null
gunzip "$TMP/s.db.gz"
/opt/owl/nai-ops ro-sanitize "$TMP/s.db" >/dev/null
F="$OUT/owl-ro-$(date +%Y%m%d).db.gz"
gzip -c "$TMP/s.db" > "$F"; chmod 600 "$F"
ls -1t "$OUT"/owl-ro-*.db.gz | tail -n +8 | xargs -r rm -f
echo "ro snapshot $F ($(du -h "$F" | cut -f1))"
