#!/bin/bash
# 每周一 04:50（/etc/cron.d/naigate-asn）：把 iptoasn.com 的 IP→运营商数据下载到网关数据目录，
# 「小号扫描」用它认出机房 / VPN IP 和「一把 Key 在两家运营商网络上来回跳」（共享 Key）。下载失败就保留旧文件。
set -euo pipefail
DEST=/opt/service-tools/data/ip2asn-combined.tsv.gz
TMP="$DEST.tmp"
curl -fsSL --max-time 300 -A owl-gate/asn -o "$TMP" https://iptoasn.com/data/ip2asn-combined.tsv.gz
gzip -t "$TMP"
chmod 644 "$TMP"
mv -f "$TMP" "$DEST"
echo "$(date '+%F %T') ASN 库已更新：$(du -h "$DEST" | cut -f1)"
