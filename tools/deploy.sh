#!/usr/bin/env bash
# Deploy a green CI build of main to production, end to end.
#
#   tools/deploy.sh              latest successful push run of CI on main
#   tools/deploy.sh --wait       wait for the CI run of origin main HEAD, then deploy it
#   tools/deploy.sh <run-id>     a specific run (must have succeeded)
#   tools/deploy.sh ... --force  pass --force to cutover.sh (skip its active-queue guard)
#
# Steps: pick run -> refuse unless every job passed -> skip if already live -> download
# artifacts -> sync cutover.sh / compose.yml from this checkout -> load image -> install
# nai-ops -> stop-then-start (cutover.sh restart: the old gateway finishes its images and exits before
# the new one starts, so only one gateway ever sends to an upstream account; the site is down 1-2 minutes)
# -> healthcheck + smoke inside the container -> public /healthz.
# Rollback on failure: ssh in and run `/opt/owl/cutover.sh previous`.
#
# Needs: gh (logged in), ~/.config/owl/owl.env exporting OWL_IP (never printed),
# ~/.ssh/naigate_ed25519. Output masks IPv4 addresses.
set -euo pipefail
# shellcheck disable=SC2016,SC2029  # remote snippets are meant to expand on the server
cd "$(dirname "$0")/.."

REPO=${OWL_REPO:-kcgoofee-jpg/owl-gateway}
DOMAIN=${OWL_DOMAIN:-gate.davidzhao.top}
# shellcheck source=/dev/null
source "$HOME/.config/owl/owl.env"
: "${OWL_IP:?OWL_IP missing in ~/.config/owl/owl.env}"
KEY=(-i "$HOME/.ssh/naigate_ed25519" -o IdentitiesOnly=yes -o BatchMode=yes -o LogLevel=QUIET)

mask() { sed -E 's/[0-9]{1,3}\.[0-9]{1,3}\.[0-9]{1,3}\.[0-9]{1,3}/x.x.x.x/g'; }
remote() { ssh "${KEY[@]}" "root@$OWL_IP" "$@" 2>&1 | mask; return "${PIPESTATUS[0]}"; }
say() { printf '\033[1m==> %s\033[0m\n' "$*"; }
die() { printf '\033[31mxx %s\033[0m\n' "$*" >&2; exit 1; }

run_id="" wait=0 force=""
for a in "$@"; do
  case $a in
    --wait) wait=1 ;;
    --force) force=--force ;;
    [0-9]*) run_id=$a ;;
    *) die "unknown argument: $a" ;;
  esac
done

if [[ -z $run_id && $wait == 1 ]]; then
  sha=$(gh api "repos/$REPO/commits/main" -q .sha)
  say "waiting for CI on main ${sha:0:7}"
  for _ in $(seq 1 30); do
    run_id=$(gh run list -R "$REPO" --workflow CI --commit "$sha" --event push --limit 1 --json databaseId -q '.[0].databaseId')
    [[ -n $run_id ]] && break
    sleep 10
  done
  [[ -n $run_id ]] || die "no CI run found for ${sha:0:7}"
  # gh run watch can return before the run ends (no TTY); poll the status instead
  until [[ $(gh run view "$run_id" -R "$REPO" --json status -q .status) == completed ]]; do sleep 30; done
elif [[ -z $run_id ]]; then
  run_id=$(gh run list -R "$REPO" --workflow CI --branch main --event push --status success --limit 1 --json databaseId -q '.[0].databaseId')
  [[ -n $run_id ]] || die "no successful CI run on main"
fi

IFS='|' read -r conclusion sha < <(gh run view "$run_id" -R "$REPO" --json conclusion,headSha -q '"\(.conclusion)|\(.headSha)"')
[[ $conclusion == success ]] || die "run $run_id concluded '$conclusion'; refusing to deploy"
say "run $run_id = ${sha:0:7}: $(git log -1 --format=%s "$sha" 2>/dev/null || echo '(commit not fetched locally)')"

live=$(remote "a=\$(docker image inspect -f '{{.Id}}' owl-gate:$sha 2>/dev/null); b=\$(docker inspect -f '{{.Image}}' owl-gate 2>/dev/null); [ -n \"\$a\" ] && [ \"\$a\" = \"\$b\" ] && echo running" || true)
if [[ $live == running ]]; then
  say "owl-gate:${sha:0:7} is already running; nothing to do"
  exit 0
fi

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
say "downloading artifacts"
gh run download "$run_id" -R "$REPO" -n owl-gate-image -n nai-ops-linux-musl -D "$tmp"

say "uploading image, nai-ops, cutover.sh, caddy-reload.sh, compose.yml"
scp -q "${KEY[@]}" "$tmp/owl-gate-image/owl-gate.tar.gz" "$tmp/nai-ops-linux-musl/nai-ops" \
  deploy/v3/cutover.sh deploy/v3/caddy-reload.sh deploy/v3/compose.yml "root@$OWL_IP:/opt/owl/" 2>&1 | mask

say "cutover"
remote "set -e; cd /opt/owl; chmod +x cutover.sh caddy-reload.sh
  ./cutover.sh load owl-gate.tar.gz $force
  ./cutover.sh install-ops /opt/owl/nai-ops
  ./cutover.sh restart
  rm -f owl-gate.tar.gz" || die "cutover failed; consider: ssh in and run /opt/owl/cutover.sh previous"

say "verifying"
remote 'for i in $(seq 1 30); do docker exec owl-gate nai-gate healthcheck >/dev/null 2>&1 && break; sleep 2; done
  docker exec owl-gate nai-gate healthcheck && docker exec owl-gate nai-ops smoke http://127.0.0.1:8000 | tail -3' \
  || die "post-deploy checks failed; consider: /opt/owl/cutover.sh previous"
curl -fsS -m 10 "https://$DOMAIN/healthz" | grep -q '"ok":true' || die "public /healthz not ok"
say "live: owl-gate:${sha:0:7}"
