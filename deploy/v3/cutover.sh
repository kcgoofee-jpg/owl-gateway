#!/usr/bin/env bash
# 3.0 部署 / 重启 / 回上一版（在服务器上以 root 运行）。
#
#   deploy/v3/cutover.sh load <owl-gate.tar.gz>   载入并暂存 CI 镜像，不改 current / previous
#   deploy/v3/cutover.sh install-ops <nai-ops>     安装宿主机用的运维命令（静态二进制，快照脚本要用宿主机的 docker / 磁盘 / .env）
#   deploy/v3/cutover.sh restart                   先停后起换上 owl-gate:current（tools/deploy.sh 用它）：旧网关在途的图收尾、
#                                                  完全退出后再起新的。网站断 1–2 分钟（Caddy 显示「网关自检中」），但同一时刻只有一个网关在用账号
#   deploy/v3/cutover.sh previous                  3.0 回到上一版 3.0 镜像（owl-gate:previous），同样走 restart
# 改 Caddy 配置用 deploy/v3/caddy-reload.sh（不要 docker exec caddy caddy reload，原因见那个脚本）。
#   deploy/v3/cutover.sh status
#
# restart / previous 之前会读取网关实时队列（/public/live 的排队 + 生成中）：超过阈值
# （环境变量 CUTOVER_BUSY_THRESHOLD，默认 0）或现在正落在忙时（CUTOVER_BUSY_HOURS，如 "20-24"，默认不设）
# 就拒绝执行，除非加 --force（放在子命令后面或前面都行）。
set -euo pipefail

# 下面几项只给演练改（换个目录 / 项目名 / 容器名在备用机上整套跑一遍），线上全用默认值
OWL_DIR=${OWL_DIR:-/opt/owl}
LEGACY_DIR=${LEGACY_DIR:-/opt/service-tools}
PROJECT=${OWL_PROJECT:-owl}
GATE=${OWL_GATE:-owl-gate}
IMAGE=${OWL_IMAGE:-owl-gate}
GATE_URL=${OWL_GATE_URL:-http://127.0.0.1:3003}
# 换网关时实测用的公网地址（经本机 Caddy，不出机器）
PROBE_URL=${OWL_PROBE_URL:-https://gate.davidzhao.top}
ENV_FILE=$LEGACY_DIR/.env
COMPOSE=(docker compose -p "$PROJECT" --env-file "$ENV_FILE" -f "$OWL_DIR/compose.yml")

BUSY_THRESHOLD=${CUTOVER_BUSY_THRESHOLD:-0}
BUSY_HOURS=${CUTOVER_BUSY_HOURS:-}
FORCE=0
WAIT_IDLE=0
QUEUE_WAIT_TIMEOUT=${CUTOVER_QUEUE_WAIT_TIMEOUT:-1800}
QUEUE_POLL_SECONDS=${CUTOVER_QUEUE_POLL_SECONDS:-5}

log() { printf '[%(%F %T)T] %s\n' -1 "$*"; }
die() { log "✗ $*"; exit 1; }

# 网关当前排队与生成中的图片数；历史完成量不代表重启会中断任务。
current_images() {
  curl -fsS -m 3 "$GATE_URL/public/live" |
    jq -er '.queue | if (.running | type) == "number" and (.waiting | type) == "number" then .running + .waiting else error("queue counts unavailable") end'
}

# 现在是不是落在 CUTOVER_BUSY_HOURS（如 "20-24"，含起点、不含终点，可跨午夜 "22-6"）里
in_busy_hours() {
  [[ $BUSY_HOURS =~ ^([0-9]{1,2})-([0-9]{1,2})$ ]] || return 1
  local from=$((10#${BASH_REMATCH[1]})) to=$((10#${BASH_REMATCH[2]})) now
  now=$((10#$(date +%H)))
  if ((from <= to)); then ((now >= from && now < to)); else ((now >= from || now < to)); fi
}

# 部署 / 重启前的忙碌检查：忙就拒绝，--force 才放行
busy_guard() {
  local n reason=""
  n=$(current_images) || die "无法读取网关当前排队 / 生成任务数；为避免中断图片，取消切换"
  [[ $n =~ ^[0-9]+$ ]] || die "网关返回的当前队列数无效；为避免中断图片，取消切换"
  log "当前排队 / 生成中的图片：$n（阈值 $BUSY_THRESHOLD）"
  if ((n > BUSY_THRESHOLD)); then reason="当前有 $n 张图片正在排队或生成，超过安全阈值 $BUSY_THRESHOLD"; fi
  if [[ -z $reason ]] && in_busy_hours; then reason="现在（$(date +%H:%M)）在忙时 $BUSY_HOURS 内"; fi
  if [[ -n $reason ]]; then
    if ((FORCE)); then
      log "! $reason；已加 --force，继续"
    else
      die "$reason；等队列清空或忙时结束后再试，--force 会跳过保护"
    fi
  fi
  log "切换前会再次检查实时队列；网关停机时会先排空已接收的图片请求（最多 120 秒）"
}

# 自动发布可等待任务自然完成；手动 restart 仍立即失败，除非显式传 --wait-idle。
wait_until_safe() {
  if ((FORCE)); then busy_guard; return; fi
  [[ $QUEUE_WAIT_TIMEOUT =~ ^(0|[1-9][0-9]*)$ && $QUEUE_POLL_SECONDS =~ ^[1-9][0-9]*$ ]] ||
    die "CUTOVER_QUEUE_WAIT_TIMEOUT / CUTOVER_QUEUE_POLL_SECONDS 必须是正整数"
  local deadline=$((SECONDS + QUEUE_WAIT_TIMEOUT)) last_report=-60 n
  while :; do
    n=$(current_images) || die "无法读取网关当前排队 / 生成任务数；为避免中断图片，取消切换"
    [[ $n =~ ^[0-9]+$ ]] || die "网关返回的当前队列数无效；为避免中断图片，取消切换"
    if ((n <= BUSY_THRESHOLD)) && ! in_busy_hours; then
      busy_guard
      return
    fi
    local elapsed=$((SECONDS - (deadline - QUEUE_WAIT_TIMEOUT)))
    ((elapsed < QUEUE_WAIT_TIMEOUT)) || die "等待安全切换超时（队列 $n，阈值 $BUSY_THRESHOLD）；当前版本保持运行，可稍后重试"
    if ((elapsed - last_report >= 60)); then
      log "等待安全窗口：当前在途图片 $n，阈值 $BUSY_THRESHOLD；已等 ${elapsed}s / 最多 ${QUEUE_WAIT_TIMEOUT}s"
      last_report=$elapsed
    fi
    sleep "$QUEUE_POLL_SECONDS"
  done
}

wait_healthy() {
  local deadline=$((SECONDS + ${1:-90}))
  while ((SECONDS < deadline)); do
    if curl -fsS -m 3 "$GATE_URL/healthz" >/dev/null 2>&1; then
      return 0
    fi
    sleep 2
  done
  return 1
}

smoke() {
  docker exec "$GATE" /usr/local/bin/nai-ops smoke http://127.0.0.1:8000
}

# 换网关期间每 0.5 秒经本机 Caddy 打一次公网地址的 /healthz 和 /（curl 3 秒超时，和外部探测一样），
# 结束时按实测结果说有没有停站，而不是按流程推断
probe_start() {
  PROBE_OUT=$(mktemp)
  (
    # 子 shell 会继承脚本的 set -e：curl 一超时（退出码 28，正好是停站的时候）整个循环就退出，
    # 结果「实测」只剩停站前的几条、报成全部正常（10-21 22:14 停了 17 秒却报「探测 2 次，全部正常」）
    set +e
    while :; do
      for path in /healthz /; do
        r=$(curl -sk -m 3 --connect-to ::127.0.0.1: -o /dev/null -w '%{http_code} %{time_total}' "$PROBE_URL$path" 2>/dev/null || true)
        echo "$(date +%s.%N | cut -c1-14) $path ${r:-000 3}" >>"$PROBE_OUT"
      done
      sleep 0.5
    done
  ) &
  PROBE_PID=$!
}

# 打印实测结果；有失败返回 1
probe_report() {
  kill "$PROBE_PID" 2>/dev/null || true
  wait "$PROBE_PID" 2>/dev/null || true
  local summary
  summary=$(awk '
    { n++; ok = ($3 ~ /^[23]/); if ($4 > slow) slow = $4
      if (!ok) { bad++; if (!start) start = $1; last = $1; if ($1 - start > gap) gap = $1 - start + $4 } else start = 0 }
    END { printf "%d %d %.1f %.1f", n, bad, slow, gap }' "$PROBE_OUT")
  rm -f "$PROBE_OUT"
  local n bad slow gap
  read -r n bad slow gap <<<"$summary"
  if ((bad == 0)); then
    log "✓ 实测：经 Caddy 探测 $n 次（/healthz 和 /，每 0.5 秒），全部正常，最慢 ${slow} 秒"
    return 0
  fi
  log "! 实测：经 Caddy 探测 $n 次，失败 $bad 次，最长连续失败约 ${gap} 秒，最慢 ${slow} 秒"
  return 1
}

cmd_load() {
  local tar=${1:?用法：cutover.sh load <owl-gate.tar.gz>}
  [[ -f $tar ]] || die "找不到 $tar"
  local loaded
  loaded=$(gunzip -c "$tar" | docker load | sed -n 's/^Loaded image: //p' | tail -1)
  [[ -n $loaded ]] || die "docker load 没有输出镜像名"
  docker tag "$loaded" "$IMAGE:staged"
  log "✓ owl-gate:staged = $loaded；正在运行的 current / 回滚用 previous 均未改动"
}

cmd_install_ops() {
  local bin=${1:?用法：cutover.sh install-ops <nai-ops>}
  "$bin" --version >/dev/null || die "$bin 在这台机器上跑不起来"
  if [[ $(realpath "$bin") != $(realpath -m "$OWL_DIR/nai-ops") ]]; then
    install -m 755 "$bin" "$OWL_DIR/nai-ops"
  else
    chmod 755 "$OWL_DIR/nai-ops"
  fi
  log "✓ $OWL_DIR/nai-ops = $("$OWL_DIR/nai-ops" --version)"
}

# 先停后起换版本：任何时刻只有一个网关进程在给 NovelAI 发图。
# 每个账号「有没有图在出、下一张什么时候能发」只记在网关进程的内存里，两个网关同时在线时互相看不到，
# 会让同一账号同时出两张而被上游回 429，所以部署不再让新旧网关重叠。
# 旧网关收到 SIGTERM 立即关掉监听，在途和已排队的图出完（最多 120 秒）才退出；这期间 Caddy 连不上网关，显示「网关自检中」。
restart_container() {
  docker image inspect "$IMAGE:current" >/dev/null 2>&1 || die "先 load 镜像"
  probe_start
  log "停 $GATE：不再接新请求，手上的图出完（最多 120 秒）再退出；这期间网站显示「网关自检中」"
  "${COMPOSE[@]}" stop -t 120 gate
  log "起新的 $GATE"
  if ! "${COMPOSE[@]}" up -d --remove-orphans || ! wait_healthy 90 || ! smoke; then
    "${COMPOSE[@]}" logs --tail 80 gate || true
    probe_report || true
    die "新的 $GATE 没通过检查：cutover.sh previous 回上一版"
  fi
  # 停站是预期的：这里只是把实测的中断时长报出来
  probe_report || true
  # docker load 每次都会留一层旧的悬空镜像；换版成功后顺手清掉，别让磁盘慢慢被吃满
  if pruned=$(docker image prune -f 2>/dev/null | tail -1) && [[ -n $pruned && $pruned != "Total reclaimed space: 0B" ]]; then
    log "✓ 清理悬空镜像：$pruned"
  fi
  log "$GATE = $(docker inspect -f '{{.Image}}' "$GATE" | cut -c8-19)"
}

cmd_restart() {
  if ((WAIT_IDLE)); then wait_until_safe; else busy_guard; fi
  if docker image inspect "$IMAGE:staged" >/dev/null 2>&1; then
    local staged_id current_id running_id
    staged_id=$(docker image inspect -f '{{.Id}}' "$IMAGE:staged")
    current_id=$(docker image inspect -f '{{.Id}}' "$IMAGE:current" 2>/dev/null || true)
    if [[ $current_id != "$staged_id" ]]; then
      # Only replace the rollback image when the current tag is the healthy image actually serving traffic.
      # This keeps `previous` pointing to the last working release after a failed/partial cutover retry.
      running_id=$(docker inspect -f '{{.Image}}' "$GATE" 2>/dev/null || true)
      if [[ -n $current_id && $current_id == "$running_id" ]] &&
        docker exec "$GATE" /usr/local/bin/nai-gate healthcheck >/dev/null 2>&1; then
        docker tag "$IMAGE:current" "$IMAGE:previous"
      fi
      docker tag "$IMAGE:staged" "$IMAGE:current"
    fi
    docker image rm "$IMAGE:staged" >/dev/null 2>&1 || true
    log "✓ staged 镜像已提升为 current；原 current 保留为 previous"
  fi
  restart_container
}

cmd_previous() {
  if ((WAIT_IDLE)); then wait_until_safe; else busy_guard; fi
  docker image inspect "$IMAGE:previous" >/dev/null 2>&1 || die "没有 owl-gate:previous"
  docker tag "$IMAGE:current" "$IMAGE:failed"
  docker tag "$IMAGE:previous" "$IMAGE:current"
  restart_container || die "上一版也起不来：看上面的日志（docker logs $GATE）"
  log "✓ 已回到上一版 3.0（坏的那版标成 owl-gate:failed）"
}

cmd_status() {
  docker ps -a --filter name=owl- --format '{{.Names}}\t{{.Status}}\t{{.Image}}'
  curl -fsS -m 3 "$GATE_URL/healthz" && echo
}

args=()
for a in "$@"; do
  case $a in
    --force) FORCE=1 ;;
    --wait-idle) WAIT_IDLE=1 ;;
    *) args+=("$a") ;;
  esac
done
set -- "${args[@]+"${args[@]}"}"

case ${1:-status} in
  load) shift; cmd_load "$@" ;;
  install-ops) shift; cmd_install_ops "$@" ;;
  restart) cmd_restart ;;
  previous) cmd_previous ;;
  status) cmd_status ;;
  *) die "未知子命令：$1" ;;
esac
