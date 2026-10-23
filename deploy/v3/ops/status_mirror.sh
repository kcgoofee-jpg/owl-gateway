#!/usr/bin/env bash
# 状态页独立镜像：把 /status 渲染成静态页推到 S3（每 5 分钟一次，cron，见 deploy/v3/ops/crontab）。
# 主站挂了，镜像仍在：http://nai-gate-status-<账号>.s3-website.us-east-2.amazonaws.com/
# 页面底部的快照时间停止更新超过 10 分钟，本身就是「主站故障」的信号。
#
# 3.0 的状态页样式和脚本不再内联，而是外置在 /_owl/status-page.css、/_owl/status-page.js（地址带内容哈希）：
# 只推 HTML 的话镜像会没有样式。所以这里连 CSS / JS 一起取下来、传到桶根目录，页面里的链接改成相对路径
# （status-page.css?v=<哈希>），最后才传 index.html，页面不会引用还没传上去的文件。
set -euo pipefail
cd /opt/service-tools
export AWS_PROFILE=naigate-push
BASE=http://127.0.0.1:3003
ACC=$(aws sts get-caller-identity --query Account --output text 2>/dev/null || true)
[ -n "$ACC" ] || { echo "$(date '+%F %T') 无法取 AWS 身份，跳过"; exit 0; }
BUCKET="nai-gate-status-${ACC}"

TMP=$(mktemp -d /tmp/status-mirror.XXXXXX)
trap 'rm -rf "$TMP"' EXIT
curl -fsS -m 20 "$BASE/status" -o "$TMP/index.html"

# 页面里的样式 / 脚本地址（带 ?v=<内容哈希>）
CSS_URL=$(grep -o '/_owl/status-page\.css?v=[0-9A-Za-z]*' "$TMP/index.html" | head -n 1 || true)
JS_URL=$(grep -o '/_owl/status-page\.js?v=[0-9A-Za-z]*' "$TMP/index.html" | head -n 1 || true)
[ -n "$CSS_URL" ] && [ -n "$JS_URL" ] || { echo "$(date '+%F %T') 页面里找不到样式 / 脚本地址，跳过"; exit 1; }
curl -fsS -m 20 "$BASE$CSS_URL" -o "$TMP/status-page.css"
curl -fsS -m 20 "$BASE$JS_URL" -o "$TMP/status-page.js"

# 链接改成相对路径（保留 ?v=，哈希变了浏览器会重新取）
sed -i \
  -e "s|$CSS_URL|status-page.css?v=${CSS_URL##*=}|g" \
  -e "s|$JS_URL|status-page.js?v=${JS_URL##*=}|g" \
  "$TMP/index.html"

# 注入镜像横幅：说明这是快照、正常 5 分钟一更
TS=$(date '+%m-%d %H:%M')
BANNER="<div style=\"position:fixed;bottom:0;left:0;right:0;padding:5px 12px;background:#151a23;color:#8b93a3;font:12px/1.5 -apple-system,sans-serif;text-align:center;z-index:9999\">🔒 独立镜像 · 快照生成于 ${TS}（北京时间），正常每 5 分钟自动更新；本页超过 10 分钟未更新 = 主站可能故障</div>"
sed -i "s|</body>|${BANNER}</body>|" "$TMP/index.html"

aws s3 cp "$TMP/status-page.css" "s3://${BUCKET}/status-page.css" --cache-control "max-age=300" --content-type "text/css; charset=utf-8" --no-progress
aws s3 cp "$TMP/status-page.js" "s3://${BUCKET}/status-page.js" --cache-control "max-age=300" --content-type "text/javascript; charset=utf-8" --no-progress
aws s3 cp "$TMP/index.html" "s3://${BUCKET}/index.html" --cache-control "max-age=60" --content-type "text/html; charset=utf-8" --no-progress
echo "$(date '+%F %T') mirror pushed"
