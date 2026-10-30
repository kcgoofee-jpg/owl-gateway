#!/usr/bin/env bash
# 部署后的验证：维护档位如果是 all/owner，公共接口会按设计回 503，
# 这里临时切 off 跑 smoke 与 /image，再切回原档位（smoke 失败也切回）。
set -euo pipefail
[ -n "${VERIFY_DEBUG:-}" ] && set -x
DOMAIN=${1:?domain}
ENVF=${ENV_FILE:-/opt/service-tools/.env}
JAR=$(mktemp)
PW=$(grep -E '^ADMIN_PASSWORD=' "$ENVF" | head -1 | cut -d= -f2- | tr -d '"')
ORIGIN="https://$DOMAIN"

login() {
  curl -fsS -X POST "https://$DOMAIN/admin/api/login" \
    -H 'content-type: application/json' -H "origin: $ORIGIN" \
    -d "{\"password\":\"$PW\"}" -c "$JAR" >/dev/null
}
mode() {
  curl -fsS -b "$JAR" "https://$DOMAIN/admin/api/ops" \
    | python3 -c 'import sys,json;print(json.load(sys.stdin)["maintenance"].get("mode") or "off")'
}
set_mode() {
  curl -fsS -b "$JAR" -X PUT "https://$DOMAIN/admin/api/ops/maintenance" \
    -H 'content-type: application/json' -H "origin: $ORIGIN" \
    -d "{\"mode\":\"$1\"}" >/dev/null
}

login
CURRENT=$(mode)
[ "$CURRENT" = "off" ] || set_mode off
# 等公共路由真的放行（设置落库后生效需要一瞬间），最多 10 秒
for _ in $(seq 1 10); do
  code=$(curl -s -o /dev/null -w '%{http_code}' "https://$DOMAIN/public/live" || true)
  [ "$code" = "200" ] && break
  sleep 1
done
echo "maintenance now $(mode), public/live $code"
status=0
docker exec owl-gate nai-ops smoke http://127.0.0.1:8000 | tail -3 || status=$?
curl -fsS -m 10 -o /dev/null -w 'image: %{http_code}\n' "https://$DOMAIN/image" || status=$?
[ "$CURRENT" = "off" ] || set_mode "$CURRENT"
echo "maintenance restored to $CURRENT"
exit $status
