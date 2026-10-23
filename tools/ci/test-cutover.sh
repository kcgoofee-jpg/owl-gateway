#!/usr/bin/env bash
set -euo pipefail

repo=$(cd "$(dirname "$0")/../.." && pwd)
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
mkdir -p "$tmp/bin" "$tmp/legacy" "$tmp/opt/owl"
: >"$tmp/legacy/.env"
export DOCKER_STATE="$tmp/docker-state" DOCKER_CALLS="$tmp/docker-calls"
export CUTOVER_TEST_QUEUE=busy
export OWL_DIR="$tmp/opt/owl" LEGACY_DIR="$tmp/legacy" OWL_IMAGE=owl-gate
export OWL_GATE_URL=http://127.0.0.1:3003 PROBE_URL=https://example.test
export PATH="$tmp/bin:$PATH"

cat >"$tmp/bin/docker" <<'MOCK_DOCKER'
#!/usr/bin/env bash
set -euo pipefail
state=${DOCKER_STATE:?}
calls=${DOCKER_CALLS:?}
printf '%s\n' "$*" >>"$calls"
get_image() { awk -F '\t' -v key="$1" '$1==key {v=$2} END {print v}' "$state"; }
set_image() {
  local key=$1 value=$2
  awk -F '\t' -v key="$key" '$1!=key {print $0}' "$state" >"$state.tmp" 2>/dev/null || :
  printf '%s\t%s\n' "$key" "$value" >>"$state.tmp"
  mv "$state.tmp" "$state"
}
case "$1" in
  load) echo 'Loaded image: owl-gate:build-sha';;
  tag)
    src=$2 dst=$3
    value=$(get_image "$src")
    [[ -n $value ]] || value="sha256:${src##*:}"
    set_image "$dst" "$value"
    ;;
  image)
    case "$2" in
      inspect)
        ref=${@: -1}
        value=$(get_image "$ref")
        [[ -n $value ]] || exit 1
        printf '%s\n' "$value"
        ;;
      rm)
        ref=${@: -1}
        awk -F '\t' -v key="$ref" '$1!=key {print $0}' "$state" >"$state.tmp"
        mv "$state.tmp" "$state"
        ;;
    esac
    ;;
  compose) ;;
  exec) ;;
  inspect) printf '%s\n' "$(get_image owl-gate:current)" ;;
  *) echo "unexpected docker call: $*" >&2; exit 80;;
esac
MOCK_DOCKER

cat >"$tmp/bin/curl" <<'MOCK_CURL'
#!/usr/bin/env bash
set -euo pipefail
url=${@: -1}
if [[ " $* " == *" -w "* ]]; then printf '200 0.1'; exit 0; fi
case "$url" in
  */public/live)
    if [[ ${CUTOVER_TEST_QUEUE:-idle} == busy ]]; then
      printf '{"queue":{"running":1,"waiting":0}}\n'
    else
      printf '{"queue":{"running":0,"waiting":0}}\n'
    fi
    ;;
  */healthz) printf '{"ok":true}\n' ;;
  *) printf '<html>ok</html>\n' ;;
esac
MOCK_CURL

chmod +x "$tmp/bin/docker" "$tmp/bin/curl"
printf 'owl-gate:current\tsha256:old\nowl-gate:previous\tsha256:older\n' >"$DOCKER_STATE"
printf 'test image\n' | gzip >"$tmp/image.tar.gz"

# Loading an image while the gateway has traffic only stages it; rollback/current tags stay intact.
bash "$repo/deploy/v3/cutover.sh" load "$tmp/image.tar.gz" >/dev/null
[[ $(awk -F '\t' '$1=="owl-gate:current" {print $2}' "$DOCKER_STATE") == sha256:old ]]
[[ $(awk -F '\t' '$1=="owl-gate:previous" {print $2}' "$DOCKER_STATE") == sha256:older ]]
[[ $(awk -F '\t' '$1=="owl-gate:staged" {print $2}' "$DOCKER_STATE") == sha256:build-sha ]]

# If the queue does not drain before the deadline, do not promote or restart.
set +e
CUTOVER_QUEUE_WAIT_TIMEOUT=1 CUTOVER_QUEUE_POLL_SECONDS=1 \
  bash "$repo/deploy/v3/cutover.sh" restart --wait-idle >"$tmp/timeout.log" 2>&1
status=$?
set -e
[[ $status != 0 ]]
grep -Fq '等待安全切换超时' "$tmp/timeout.log"
[[ $(awk -F '\t' '$1=="owl-gate:current" {print $2}' "$DOCKER_STATE") == sha256:old ]]
[[ $(awk -F '\t' '$1=="owl-gate:previous" {print $2}' "$DOCKER_STATE") == sha256:older ]]

# When idle, promote the candidate and keep the formerly-live image as the rollback target.
CUTOVER_TEST_QUEUE=idle CUTOVER_QUEUE_WAIT_TIMEOUT=10 \
  bash "$repo/deploy/v3/cutover.sh" restart --wait-idle >"$tmp/restart.log" 2>&1
[[ $(awk -F '\t' '$1=="owl-gate:current" {print $2}' "$DOCKER_STATE") == sha256:build-sha ]]
[[ $(awk -F '\t' '$1=="owl-gate:previous" {print $2}' "$DOCKER_STATE") == sha256:old ]]
if awk -F '\t' '$1=="owl-gate:staged" {found=1} END {exit !found}' "$DOCKER_STATE"; then
  echo 'staged tag was not removed after promotion' >&2
  exit 1
fi
grep -Fq '实测：经 Caddy 探测' "$tmp/restart.log"

# Restaging the already-current image and restarting must not replace the last-good rollback tag.
bash "$repo/deploy/v3/cutover.sh" load "$tmp/image.tar.gz" >/dev/null
CUTOVER_TEST_QUEUE=idle CUTOVER_QUEUE_WAIT_TIMEOUT=10 \
  bash "$repo/deploy/v3/cutover.sh" restart --wait-idle >"$tmp/retry.log" 2>&1
[[ $(awk -F '\t' '$1=="owl-gate:current" {print $2}' "$DOCKER_STATE") == sha256:build-sha ]]
[[ $(awk -F '\t' '$1=="owl-gate:previous" {print $2}' "$DOCKER_STATE") == sha256:old ]]

echo 'cutover safety tests passed'
