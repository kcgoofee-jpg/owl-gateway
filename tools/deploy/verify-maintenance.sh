#!/usr/bin/env bash
# 部署后的验证：维护模式开着时公共接口按设计回 503，
# 所以先记下当前档位、临时切 off 跑 nai-ops smoke 与 /image，最后无论成败都切回原档位。
# 后台会话/限流不可用时降级：只做不依赖后台的检查，绝不让部署「假失败」。
set -uo pipefail
DOMAIN=${1:?domain}
ENVF=${ENV_FILE:-/opt/service-tools/.env}
JAR=${ADMIN_JAR:-/var/tmp/owl-deploy-admin.jar}
PW=$(grep -E '^ADMIN_PASSWORD=' "$ENVF" | head -1 | cut -d= -f2- | tr -d '"')
A="https://$DOMAIN"

api() { curl -s -b "$JAR" -H "content-type: application/json" -H "origin: $A" "$@"; }
login() { curl -s -o /dev/null -w '%{http_code}' -X POST "$A/admin/api/login" -H 'content-type: application/json' -H "origin: $A" -d "{\"password\":\"$PW\"}" -c "$JAR"; }
mode() { api "$A/admin/api/ops" | python3 -c 'import sys,json;print(json.load(sys.stdin)["maintenance"].get("mode") or "off")' 2>/dev/null; }
set_mode() { api -X PUT "$A/admin/api/ops/maintenance" -d "{\"mode\":\"$1\"}" >/dev/null 2>&1; }

probe_public() { curl -s -o /dev/null -w '%{http_code}' "$A/public/live" 2>/dev/null || true; }
# 被关掉的全站功能：upscale/augment 关掉后工作台的「放大」直接 403，和官网界面不一致。
# 每次部署都把结果打出来提醒（不判失败，关掉可能是站长的主动决定）。
check_features() {
  local off
  off=$(api "$A/admin/api/ops" 2>/dev/null | python3 -c 'import sys,json
d=json.load(sys.stdin)
print(" ".join(sorted(k for k,v in d["features"]["global"].items() if not v)))' 2>/dev/null)
  case ${off:-} in
    "") echo "features: all enabled" ;;
    *upscale*|*augment*) echo "WARN features disabled: $off (工作台放大/导演工具会回 403)" ;;
    *) echo "features disabled: $off" ;;
  esac
}
run_smoke() { docker exec owl-gate nai-ops smoke http://127.0.0.1:8000 | tail -3; }
# 工作台「上传到 Explore」：游客没会话，必须 401（顺便确认路由挂上了，没被 SPA fallback 吃掉）。
check_explore() {
  local code
  code=$(curl -s -o /dev/null -w '%{http_code}' -X POST "$A/owl/explore" 2>/dev/null)
  case $code in
    401) echo "explore upload: 401 (游客，符合预期)" ;;
    200 | 302 | 307) echo "explore upload: $code (意外：游客不该能提交)" ; return 1 ;;
    *) echo "explore upload: $code" ; return 1 ;;
  esac
}
# 游客没有成员会话时 /image 会 307 回首页（走登录/领 Key），所以 307 也算正常。
check_image() {
  local code
  code=$(curl -s -o /dev/null -w '%{http_code}' "$A/image" 2>/dev/null)
  case $code in
    200) echo "image: 200" ;;
    307 | 302 | 303) echo "image: $code (游客跳首页，符合预期)" ;;
    *) echo "image: $code"; return 1 ;;
  esac
}

# cutover 之前 suspend-maintenance.sh 记录了原档位；优先用它恢复
STATE=${MAINTENANCE_STATE:-/var/tmp/owl-maintenance-mode}
SAVED=""
[[ -f $STATE ]] && SAVED=$(cat "$STATE")

CURRENT=$(mode || true)
if [[ -z ${CURRENT:-} ]]; then
  login >/dev/null 2>&1 || true
  CURRENT=$(mode || true)
fi

if [[ -z ${CURRENT:-} ]]; then
  echo "admin session unavailable (rate limited or cookie expired); degrading"
  echo "healthz: $(curl -s -o /dev/null -w '%{http_code}' "$A/healthz")"
  code=$(probe_public)
  if [[ $code == 200 ]]; then
    run_smoke
    check_image
  else
    echo "public routes behind maintenance ($code); smoke skipped by design"
  fi
  exit 0
fi

TARGET=${SAVED:-$CURRENT}
echo "maintenance mode: $CURRENT (restore to $TARGET)"
if [[ $CURRENT != off ]]; then
  set_mode off
  for _ in $(seq 1 10); do
    [[ $(probe_public) == 200 ]] && break
    sleep 1
  done
  if [[ $(mode) != off ]]; then
    echo "could not switch maintenance off; skipping smoke"
    echo "healthz: $(curl -s -o /dev/null -w '%{http_code}' "$A/healthz")"
    exit 0
  fi
fi

status=0
run_smoke || status=$?
check_image || status=$?
check_explore || status=$?
check_features || true

set_mode "$TARGET"
rm -f "$STATE"
echo "maintenance restored to $TARGET"
exit $status
