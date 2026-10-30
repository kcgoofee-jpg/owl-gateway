#!/usr/bin/env bash
# 一条命令走完：提交 → 推送 → 等 CI → 部署 → 验证（前端小改更该用 tools/hot-web.sh，秒级生效）。
#
#   tools/release.sh "commit message"        # 有改动才提交
#   tools/release.sh                          # 不提交，只把 main 上最新的绿构建发上去
#
# 前置：CI 必须通过（deploy.sh 拒绝失败的 run）；维护模式开着也能部署（verify 会临时切 off 再切回）。
set -euo pipefail
cd "$(dirname "$0")/.."
msg=${1:-}

if [[ -n $msg ]]; then
  if [[ -n $(git status --porcelain) ]]; then
    git add -A
    git -c core.hooksPath=tools/hooks commit -q -m "$msg"
    git push origin main
  else
    echo "==> 工作区干净，跳过提交"
  fi
fi

exec tools/deploy.sh --wait
