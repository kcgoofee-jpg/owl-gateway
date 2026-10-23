#!/usr/bin/env bash
# Reclaim disk from Rust build caches across every worktree of this repository.
# Each worktree keeps its own target/ (10-180 GB); a full Mac stops builds, tests and agents.
#
#   tools/clean-targets.sh            drop target/ of worktrees idle for 2+ days, and
#                                     target/debug/incremental everywhere
#   tools/clean-targets.sh --days 0   drop every worktree's target/ (next build starts cold)
#   tools/clean-targets.sh --dry-run  only report
#
# Never touches sources, branches or worktrees themselves; skips a worktree with a live cargo/rustc.
set -euo pipefail
days=2 dry=0
while (($#)); do
  case $1 in
    --days) days=$2; shift ;;
    --dry-run) dry=1 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
  shift
done

busy_dirs=$(for p in $(pgrep -x cargo; pgrep -x rustc); do lsof -a -p "$p" -d cwd -Fn 2>/dev/null | sed -n 's/^n//p'; done | sort -u)
drop() {
  local size; size=$(du -sh "$1" 2>/dev/null | cut -f1)
  if ((dry)); then echo "would remove $1 ($size)"; else rm -rf "$1" && echo "removed $1 ($size)"; fi
}

git worktree list --porcelain | sed -n 's/^worktree //p' | while read -r wt; do
  t=$wt/target
  [[ -d $t ]] || continue
  if grep -qxF "$wt" <<<"$busy_dirs"; then echo "skip $wt (build running)"; continue; fi
  if [[ -z $(find "$t" -maxdepth 2 -mtime "-$days" -print -quit 2>/dev/null) ]]; then
    drop "$t"
  elif [[ -d $t/debug/incremental ]]; then
    drop "$t/debug/incremental"
  fi
done
df -h "$HOME" | tail -1
