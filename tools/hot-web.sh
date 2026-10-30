#!/usr/bin/env bash
# 前端热更新：本地构建 web/dist，直接同步到服务器的 STATIC_DIR（/opt/owl/nai-mirror），
# 网关优先读磁盘，所以不用重建镜像、不用重启、不用等 CI。只覆盖前端产物，不动 nai-app/。
#
#   tools/hot-web.sh            # 构建 + 同步 + 验证
#   tools/hot-web.sh --no-build # 只同步现有 dist
set -euo pipefail
cd "$(dirname "$0")/.."
source "$HOME/.config/owl/owl.env"
: "${OWL_IP:?OWL_IP missing in ~/.config/owl/owl.env}"
KEY=(-i "$HOME/.ssh/naigate_ed25519" -o IdentitiesOnly=yes -o BatchMode=yes -o LogLevel=QUIET)
DOMAIN=${OWL_DOMAIN:-gate.davidzhao.top}
build=1
[[ ${1:-} == "--no-build" ]] && build=0

say() { printf '\033[1m==> %s\033[0m\n' "$*"; }
mask() { sed -E 's/[0-9]{1,3}(\.[0-9]{1,3}){3}/x.x.x.x/g'; }

if [[ $build == 1 ]]; then
  say "building web"
  (cd web && pnpm build >/dev/null)
fi
[[ -f web/dist/landing.html ]] || { echo "web/dist 缺失，先构建" >&2; exit 1; }

say "syncing dist to $OWL_IP:/opt/owl/nai-mirror"
rsync -a --delete --exclude 'nai-app' -e "ssh ${KEY[*]}" web/dist/ "root@$OWL_IP:/opt/owl/nai-mirror/" 2>&1 | mask

say "verifying"
for p in /admin /favicon.svg; do
  code=$(curl -s -o /dev/null -w '%{http_code}' "https://$DOMAIN$p" || true)
  echo "  $p -> $code"
  [[ $code == 200 ]] || { echo "hot update verification failed" >&2; exit 1; }
done
asset=$(cd web/dist && ls landing-*.js | head -1)
code=$(curl -s -o /dev/null -w '%{http_code}' "https://$DOMAIN/_owl/$asset" || true)
echo "  /_owl/$asset -> $code"
[[ $code == 200 ]] || { echo "asset not served from disk" >&2; exit 1; }
say "hot web update done"
