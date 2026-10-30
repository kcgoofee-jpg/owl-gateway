#!/usr/bin/env bash
# 把本地 mirror/nai-app（官网生图页的静态资源，59MB，不进仓库也不进镜像）同步到服务器只读挂载点。
# 单独跑：tools/sync-mirror.sh；部署脚本在 cutover 之前也会调用它。
set -euo pipefail
cd "$(dirname "$0")/.."
source "$HOME/.config/owl/owl.env"
: "${OWL_IP:?OWL_IP missing in ~/.config/owl/owl.env}"
KEY=(-i "$HOME/.ssh/naigate_ed25519" -o IdentitiesOnly=yes -o BatchMode=yes -o LogLevel=QUIET)

say() { printf '\033[1m==> %s\033[0m\n' "$*"; }
mask() { sed -E 's/[0-9]{1,3}(\.[0-9]{1,3}){3}/x.x.x.x/g'; }
remote() { ssh "${KEY[@]}" "root@$OWL_IP" "$@" 2>&1 | mask; return "${PIPESTATUS[0]}"; }

[[ -f mirror/nai-app/image.html ]] || { echo "mirror/nai-app 缺失：先准备官网镜像资源" >&2; exit 1; }

# 桥接层是我们自己写的（官网产物里没有），仓库里留一份，同步前覆盖到镜像目录：
# mirror/ 整个目录不进仓库，改 bridge.js 只有这里能跟进版本。
cp deploy/nai-mirror-overrides/bridge.js mirror/nai-app/bridge.js

remote "mkdir -p /opt/owl/nai-mirror"
say "syncing mirror ($(du -sh mirror/nai-app | cut -f1 | tr -d ' '))"
rsync -a --delete -e "ssh ${KEY[*]}" mirror/nai-app/ "root@$OWL_IP:/opt/owl/nai-mirror/nai-app/" 2>&1 | mask
say "synced"
