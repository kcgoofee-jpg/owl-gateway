#!/usr/bin/env bash
# 把宿主机上的 Caddyfile 加载进正在跑的 Caddy（在服务器上以 root 运行）：存下当前配置 → 校验 → 经管理接口加载 → 核对。
#
#   deploy/v3/caddy-reload.sh [Caddyfile]     默认 /opt/caddy/Caddyfile
#
# 为什么不用 `docker exec caddy caddy reload`：Caddy 容器是单文件只读挂载（/opt/caddy/Caddyfile:/etc/caddy/Caddyfile:ro），
# 宿主机上的文件一旦被换了 inode（编辑器保存、sed -i、写临时文件再 mv 都会），容器里看到的仍是旧文件，reload 读进去的也是旧的，
# 而且不报错。10-21 的两次改动（降级页的 https 状态页链接、临时网关地址）都因此没生效，第一次 roll 照样停站。
# 这里用管理接口 /load 直接把宿主机文件的内容交给 Caddy，不经过挂载；Caddy 以后重启时会重新挂载，读到的也是这份。
# 回滚：curl -X POST localhost:2019/load -H 'Content-Type: application/json' --data-binary @<存下的 json>
set -euo pipefail

FILE=${1:-/opt/caddy/Caddyfile}
ADMIN=${CADDY_ADMIN:-http://localhost:2019}
SAVED=$(dirname "$FILE")/running-$(date +%Y%m%d-%H%M%S).json

log() { printf '[%(%F %T)T] %s\n' -1 "$*"; }
die() { log "✗ $*"; exit 1; }
# 同一份 JSON 排好键再比，避免键顺序不同
norm() { python3 -c 'import json,sys; print(json.dumps(json.load(sys.stdin), sort_keys=True))'; }

[[ -f $FILE ]] || die "找不到 $FILE"
curl -fsS -m 5 "$ADMIN/config/" >"$SAVED" || die "连不上 Caddy 管理接口 $ADMIN"
chmod 600 "$SAVED"
log "当前运行的配置存到 $SAVED（回滚用）"

# 用一个临时容器读宿主机上的文件（它挂载的是现在这个 inode）：先校验，再转成 JSON 留着核对
run=(docker run --rm -v "$(realpath "$FILE"):/etc/caddy/Caddyfile:ro" caddy:2 caddy)
"${run[@]}" validate --config /etc/caddy/Caddyfile --adapter caddyfile >/dev/null 2>&1 ||
  { "${run[@]}" validate --config /etc/caddy/Caddyfile --adapter caddyfile 2>&1 | tail -5; die "校验没过，没有加载"; }
want=$("${run[@]}" adapt --config /etc/caddy/Caddyfile --adapter caddyfile 2>/dev/null | norm)
log "✓ 校验通过"

curl -fsS -m 30 -X POST "$ADMIN/load" -H 'Content-Type: text/caddyfile' --data-binary @"$FILE" >/dev/null ||
  die "加载失败（Caddy 保持原配置）"
got=$(curl -fsS -m 5 "$ADMIN/config/" | norm)
[[ $got == "$want" ]] || die "加载后的运行配置和 $FILE 对不上；要回到之前的配置用上面存下的 $SAVED"
log "✓ Caddy 正在跑的就是 $FILE"
