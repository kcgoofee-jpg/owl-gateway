#!/usr/bin/env bash
# shellcheck disable=SC2016,SC2029  # remote snippets intentionally interpolate on the server
# Deploy a green CI build of main to production, end to end.
#
#   tools/deploy.sh              latest successful push run of CI on main
#   tools/deploy.sh --wait       wait for the CI run of origin main HEAD, then deploy it
#   tools/deploy.sh <run-id>     a specific run (must have succeeded)
#
# Steps: pick run -> refuse unless every job passed -> skip if already live -> download
# artifacts (the image tarball is skipped when the server already loaded that exact image,
# e.g. a cutover retry) -> sync deployment scripts -> stage image without touching the rollback
# tag -> promote the staged image and stop-then-start immediately (a maintenance page covers the
# 1-2 minute window; the old gateway finishes accepted work while shutting down, and only one
# gateway ever sends to an upstream account) -> healthcheck + smoke inside the container -> public
# /healthz -> optional Discord notice (export DISCORD_ALERT_WEBHOOK in owl.env).
# Rollback on failure: ssh in and run `/opt/owl/cutover.sh previous`.
#
# Needs: gh (logged in), ~/.config/owl/owl.env exporting OWL_IP (never printed),
# ~/.ssh/naigate_ed25519. Output masks IPv4 addresses.
set -euo pipefail
cd "$(dirname "$0")/.."
start=$SECONDS

REPO=${OWL_REPO:-kcgoofee-jpg/owl-gateway}
DOMAIN=${OWL_DOMAIN:-gate.davidzhao.top}
# shellcheck source=/dev/null
source "$HOME/.config/owl/owl.env"
: "${OWL_IP:?OWL_IP missing in ~/.config/owl/owl.env}"
KEY=(-i "$HOME/.ssh/naigate_ed25519" -o IdentitiesOnly=yes -o BatchMode=yes -o LogLevel=QUIET -o ServerAliveInterval=15 -o ServerAliveCountMax=6)
CI_WAIT_TIMEOUT=${OWL_CI_WAIT_TIMEOUT_SECONDS:-3600}

mask() { sed -E 's/[0-9]{1,3}\.[0-9]{1,3}\.[0-9]{1,3}\.[0-9]{1,3}/x.x.x.x/g'; }
remote() { ssh "${KEY[@]}" "root@$OWL_IP" "$@" 2>&1 | mask; return "${PIPESTATUS[0]}"; }
say() { printf '\033[1m==> %s\033[0m\n' "$*"; }
die() { printf '\033[31mxx %s\033[0m\n' "$*" >&2; exit 1; }
run_id="" wait=0 notify=1 mirror_only=0
for a in "$@"; do
  case $a in
    --wait) wait=1 ;;
    --no-notify) notify=0 ;;
    --mirror-only) mirror_only=1 ;;
    [0-9]*) run_id=$a ;;
    *) die "unknown argument: $a" ;;
  esac
done

if [[ $mirror_only == 1 ]]; then
  # 只同步镜像资源（改 bridge.js、换静态资源时用），不动镜像、不重启容器
  tools/sync-mirror.sh
  ssh "${KEY[@]}" "root@$OWL_IP" "bash -s -- $(printf %q "$DOMAIN")" < tools/deploy/verify-maintenance.sh 2>&1 | mask \
    || die "mirror verification failed"
  say "mirror synced and verified"
  exit 0
fi

if [[ -z $run_id && $wait == 1 ]]; then
  sha=$(gh api "repos/$REPO/commits/main" -q .sha)
  say "waiting for CI on main ${sha:0:7}"
  for _ in $(seq 1 30); do
    run_id=$(gh run list -R "$REPO" --workflow CI --commit "$sha" --event push --limit 1 --json databaseId -q '.[0].databaseId')
    [[ -n $run_id ]] && break
    sleep 10
  done
  [[ -n $run_id ]] || die "no CI run found for ${sha:0:7}"
  # Poll status instead of gh run watch, which can return before completion without a TTY.
  [[ $CI_WAIT_TIMEOUT =~ ^[1-9][0-9]*$ ]] || die "OWL_CI_WAIT_TIMEOUT_SECONDS must be a positive integer"
  start=$SECONDS last_report=-60
  while :; do
    status=$(gh run view "$run_id" -R "$REPO" --json status -q .status)
    [[ $status == completed ]] && break
    elapsed=$((SECONDS - start))
    ((elapsed < CI_WAIT_TIMEOUT)) || die "CI still $status after ${elapsed}s: https://github.com/$REPO/actions/runs/$run_id"
    if ((elapsed - last_report >= 60)); then
      say "CI $status · ${elapsed}s elapsed · https://github.com/$REPO/actions/runs/$run_id"
      last_report=$elapsed
    fi
    sleep 10
  done
elif [[ -z $run_id ]]; then
  run_id=$(gh run list -R "$REPO" --workflow CI --branch main --event push --status success --limit 1 --json databaseId -q '.[0].databaseId')
  [[ -n $run_id ]] || die "no successful CI run on main"
fi

IFS='|' read -r conclusion sha < <(gh run view "$run_id" -R "$REPO" --json conclusion,headSha -q '"\(.conclusion)|\(.headSha)"')
[[ $conclusion == success ]] || die "run $run_id concluded '$conclusion'; refusing to deploy"
say "run $run_id = ${sha:0:7}: $(git log -1 --format=%s "$sha" 2>/dev/null || echo '(commit not fetched locally)')"

live=$(remote "a=\$(docker image inspect -f '{{.Id}}' owl-gate:$sha 2>/dev/null); b=\$(docker inspect -f '{{.Image}}' owl-gate 2>/dev/null); [ -n \"\$a\" ] && [ \"\$a\" = \"\$b\" ] && echo running" || true)
if [[ $live == running ]]; then
  say "owl-gate:${sha:0:7} is already running; syncing mirror resources only"
  tools/sync-mirror.sh
  exit 0
fi

# 服务器已经 load 过这个 sha（上次 cutover 中断后的重试）：几百 MB 的镜像 tar 不用再下载上传
have_image=$(remote "docker image inspect owl-gate:$sha >/dev/null 2>&1 && echo yes || echo no")
if [[ $have_image == yes ]]; then
  say "owl-gate:${sha:0:7} already staged on the server; skipping image download/upload"
fi

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
say "downloading artifacts"
dl=(-n nai-ops-linux-musl)
[[ $have_image == yes ]] || dl+=(-n owl-gate-image)
gh run download "$run_id" -R "$REPO" "${dl[@]}" -D "$tmp"

say "uploading nai-ops, cutover.sh, caddy-reload.sh, compose.yml$([[ $have_image == yes ]] || echo ', candidate image')"
files=("$tmp/nai-ops-linux-musl/nai-ops" deploy/v3/cutover.sh deploy/v3/caddy-reload.sh deploy/v3/compose.yml)
[[ $have_image == yes ]] || files=("$tmp/owl-gate-image/owl-gate.tar.gz" "${files[@]}")
scp -q "${KEY[@]}" "${files[@]}" "root@$OWL_IP:/opt/owl/" 2>&1 | mask

# 官网生图页镜像：59MB 静态资源不进仓库也不进镜像，cutover 之前同步到宿主机只读挂载点。
tools/sync-mirror.sh

# 前端产物（landing / admin）：网关 STATIC_DIR 优先读磁盘，镜像里那份是构建时的旧快照。
# 不在这里同步的话，改了前端样式/文案部署完看不到变化（10-30 踩过两次），所以每次部署都顺手刷一遍。
# 一定要现场构建：web/dist 是 gitignore 的，谁的机器上只要有一份旧产物，`--no-build` 就会把旧 CSS
# 推上去（10-31 05:18 就是这么把后台的框线「改回去了」——线上 CSS 的时间戳还停在一天前）。
tools/hot-web.sh

# cutover 自己的 smoke 也要求公共接口 200；维护模式开着时先临时切 off，部署完由 verify 脚本切回。
# 这里把当前档位写到服务器上的一个小文件，verify 脚本读它恢复。
say "suspending maintenance for the cutover"
ssh "${KEY[@]}" "root@$OWL_IP" "bash -s -- $(printf %q "$DOMAIN")" < tools/deploy/suspend-maintenance.sh 2>&1 | mask || true

say "cutover"
remote "set -e; cd /opt/owl; chmod +x cutover.sh caddy-reload.sh
  ./cutover.sh load owl-gate.tar.gz
  ./cutover.sh restart
  ./cutover.sh install-ops /opt/owl/nai-ops
  rm -f owl-gate.tar.gz" || die "cutover failed; consider: ssh in and run /opt/owl/cutover.sh previous"

say "verifying"
remote 'for i in $(seq 1 30); do docker exec owl-gate nai-gate healthcheck >/dev/null 2>&1 && break; sleep 2; done
  docker exec owl-gate nai-gate healthcheck' \
  || die "post-deploy checks failed; consider: /opt/owl/cutover.sh previous"

# 维护模式开着时公共接口按设计回 503，smoke 会误判成部署失败。
# 这里先记下当前档位、临时切到 off 跑 smoke 和 /image，再原样切回去（失败也切回）。
ssh "${KEY[@]}" "root@$OWL_IP" "bash -s -- $(printf %q "$DOMAIN")" < tools/deploy/verify-maintenance.sh 2>&1 | mask \
  || die "post-deploy checks failed; consider: /opt/owl/cutover.sh previous"

curl -fsS -m 10 "https://$DOMAIN/healthz" | grep -q '"ok":true' || die "public /healthz not ok"
say "live: owl-gate:${sha:0:7} ($((SECONDS - start))s total)"

# 可选：部署完成通知。owl.env 里 export DISCORD_ALERT_WEBHOOK 即开启；发不出去不影响部署结果。
if [[ $notify == 1 && -n ${DISCORD_ALERT_WEBHOOK:-} ]]; then
  now=$(TZ=Asia/Shanghai date '+%m-%d %H:%M')
  curl -fsS -m 15 -H 'Content-Type: application/json' \
    -d "{\"content\":\"🚀 猫头鹰公益站已上线 ${sha:0:7}（${now} 北京时间，部署用时 $((SECONDS - start))s）。出问题时：ssh 上服务器执行 /opt/owl/cutover.sh previous 回上一版。\"}" \
    "$DISCORD_ALERT_WEBHOOK" >/dev/null 2>&1 ||
    say "Discord 通知发送失败（不影响部署）"
fi
