#!/usr/bin/env bash
# 部署后的验证：
#   1) healthcheck（永远做）
#   2) 维护档位若为 all/owner，公共接口按设计回 503 → 临时切 off，跑 nai-ops smoke 和 /image，再切回原档位
#   3) 后台登录复用缓存 Cookie，避免反复登录撞 5 次/5 分钟的限流
set -euo pipefail
DOMAIN=${1:?domain}
ENVF=${ENV_FILE:-/opt/service-tools/.env}
JAR=${ADMIN_JAR:-/var/tmp/owl-deploy-admin.jar}
PW=$(grep -E '^ADMIN_PASSWORD=' "$ENVF" | head -1 | cut -d= -f2- | tr -d '"')
ORIGIN="https://$DOMAIN"

login() {
  curl -fsS -X POST "https://$DOMAIN/admin/api/login" \
    -H 'content-type: application/json' -H "origin: $ORIGIN" \
    -d "{\"password\":\"$PW\"}" -c "$JAR" >/dev/null 2>&1
}
mode() {
  curl -fsS -b "$JAR" "https://$DOMAIN/admin/api/ops" 2>/dev/null \
    | python3 -c 'import sys,json;print(json.load(sys.stdin)["maintenance"].get("mode") or "off")' 2>/dev/null
}
set_mode() {
  curl -fsS -b "$JAR" -X PUT "https://$DOMAIN/admin/api/ops/maintenance" \
    -H 'content-type: application/json' -H "origin: $ORIGIN" \
    -d "{\"mode\":\"$1\"}" >/dev/null
}

CURRENT=$(mode || true)
if [ -z "${CURRENT:-}" ]; then
  if login; then CURRENT=$(mode || true); fi
fi
if [ -z "${CURRENT:-}" ]; then
  # 后台登录不可用（限流 / 密码变更）：不阻断部署。若公共路由本来就是通的（维护关着），照样跑 smoke。
  echo "admin session unavailable; falling back to public probe"
  curl -fsS -m 10 -o /dev/null -w 'healthz: %{http_code}\n' "https://$DOMAIN/healthz"
  code=$(curl -s -o /dev/null -w '%{http_code}' "https://$DOMAIN/public/live" || true)
  if [ "$code" = "200" ]; then
    docker exec owl-gate nai-ops smoke http://127.0.0.1:8000 | tail -3
    curl -fsS -m 10 -o /dev/null -w 'image: %{http_code}\n' "https://$DOMAIN/image"
  else
    echo "public routes are behind maintenance ($code); smoke skipped by design"
  fi
  exit 0
fi

[ "$CURRENT" = "off" ] || set_mode off
for _ in $(seq 1 10); do
  code=$(curl -s -o /dev/null -w '%{http_code}' "https://$DOMAIN/public/live" || true)
  [ "$code" = "200" ] && break
  sleep 1
done

status=0
docker exec owl-gate nai-ops smoke http://127.0.0.1:8000 | tail -3 || status=$?
curl -fsS -m 10 -o /dev/null -w 'image: %{http_code}\n' "https://$DOMAIN/image" || status=$?

[ "$CURRENT" = "off" ] || set_mode "$CURRENT"
echo "maintenance restored to $CURRENT"
exit $status
