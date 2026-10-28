#!/usr/bin/env bash
set -euo pipefail

repo=$(cd "$(dirname "$0")/../.." && pwd)
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
mkdir -p "$tmp/bin" "$tmp/legacy" "$tmp/opt/owl"
: >"$tmp/legacy/.env"
export DOCKER_STATE="$tmp/docker-state" DOCKER_CALLS="$tmp/docker-calls"
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
        if [[ " $* " == *org.owl-gateway.member-policy-schema* ]]; then
          printf '%s\n' "${MOCK_POLICY_SCHEMA:-}"
          exit 0
        fi
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

# 10-27：不再等队列——restart 立即提升候选镜像并重启，原 current 保留为回滚目标。
bash "$repo/deploy/v3/cutover.sh" restart >"$tmp/restart.log" 2>&1
[[ $(awk -F '\t' '$1=="owl-gate:current" {print $2}' "$DOCKER_STATE") == sha256:build-sha ]]
[[ $(awk -F '\t' '$1=="owl-gate:previous" {print $2}' "$DOCKER_STATE") == sha256:old ]]
if awk -F '\t' '$1=="owl-gate:staged" {found=1} END {exit !found}' "$DOCKER_STATE"; then
  echo 'staged tag was not removed after promotion' >&2
  exit 1
fi
grep -Fq '实测：经 Caddy 探测' "$tmp/restart.log"

# Restaging the already-current image and restarting must not replace the last-good rollback tag.
bash "$repo/deploy/v3/cutover.sh" load "$tmp/image.tar.gz" >/dev/null
bash "$repo/deploy/v3/cutover.sh" restart >"$tmp/retry.log" 2>&1
[[ $(awk -F '\t' '$1=="owl-gate:current" {print $2}' "$DOCKER_STATE") == sha256:build-sha ]]
[[ $(awk -F '\t' '$1=="owl-gate:previous" {print $2}' "$DOCKER_STATE") == sha256:old ]]

# Native schema blocks an older incompatible image before any tags or containers change.
mkdir -p "$LEGACY_DIR/data"
python3 - "$LEGACY_DIR/data/nai_gate.db" <<'PYTHON'
import sqlite3,sys
conn=sqlite3.connect(sys.argv[1])
conn.execute('CREATE TABLE site_settings(key TEXT PRIMARY KEY,value TEXT)')
conn.execute("INSERT INTO site_settings VALUES('member_policy_v1_migrated','1')")
conn.commit()
PYTHON
: >"$DOCKER_CALLS"
if bash "$repo/deploy/v3/cutover.sh" previous >"$tmp/incompatible.log" 2>&1; then
  echo 'incompatible rollback was allowed' >&2; exit 1
fi
! grep -Eq '^(tag|compose) ' "$DOCKER_CALLS"
grep -Fq '禁止只回滚镜像' "$tmp/incompatible.log"

# Later releases with the same native schema can use the normal previous-image path.
export MOCK_POLICY_SCHEMA=1
bash "$repo/deploy/v3/cutover.sh" previous >"$tmp/compatible.log" 2>&1
grep -Fq 'tag owl-gate:previous owl-gate:current' "$DOCKER_CALLS"
grep -Fq 'compose ' "$DOCKER_CALLS"

echo 'cutover safety tests passed'
