#!/usr/bin/env bash
# 部署前把维护模式临时切成 off（cutover 内部的 smoke 要求公共接口 200），
# 并把原档位写到 /var/tmp/owl-maintenance-mode，供 verify-maintenance.sh 恢复。
set -uo pipefail
DOMAIN=${1:?domain}
ENVF=${ENV_FILE:-/opt/service-tools/.env}
JAR=${ADMIN_JAR:-/var/tmp/owl-deploy-admin.jar}
STATE=${MAINTENANCE_STATE:-/var/tmp/owl-maintenance-mode}
PW=$(grep -E '^ADMIN_PASSWORD=' "$ENVF" | head -1 | cut -d= -f2- | tr -d '"')
A="https://$DOMAIN"
api() { curl -s -b "$JAR" -H "content-type: application/json" -H "origin: $A" "$@"; }
mode() { api "$A/admin/api/ops" | python3 -c 'import sys,json;print(json.load(sys.stdin)["maintenance"].get("mode") or "off")' 2>/dev/null; }

CURRENT=$(mode || true)
if [[ -z ${CURRENT:-} ]]; then
  curl -s -o /dev/null -X POST "$A/admin/api/login" -H 'content-type: application/json' -H "origin: $A" \
    -d "{\"password\":\"$PW\"}" -c "$JAR"
  CURRENT=$(mode || true)
fi
if [[ -z ${CURRENT:-} ]]; then
  echo "admin unavailable; cannot suspend maintenance (cutover smoke may fail by design)"
  exit 0
fi
echo "$CURRENT" > "$STATE"
echo "maintenance was: $CURRENT"
[[ $CURRENT == off ]] || api -X PUT "$A/admin/api/ops/maintenance" -d '{"mode":"off"}' >/dev/null
sleep 2
echo "maintenance now: $(mode)"
