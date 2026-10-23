#!/usr/bin/env bash
# 每夜异地备份：数据库快照 + .env + secret_key + upstream_tokens.json 打包后加密推 S3。
# 加密口令在服务器 /root/.backup_pass —— 务必在服务器之外另存一份：没有它，S3 上的备份无法解开。
# 恢复：deploy/ops/migrate_restore.sh（自动下载 latest 并解包；owl.tar.gz 里是 3.0 的 compose / 切换脚本 / Caddy 配置）。
set -euo pipefail
cd /opt/service-tools
export AWS_PROFILE=naigate-push
ACC=$(aws sts get-caller-identity --query Account --output text 2>/dev/null || true)
[ -n "$ACC" ] || { echo "$(date '+%F %T') 无法取 AWS 身份，跳过"; exit 0; }
BUCKET="nai-gate-backups-${ACC}"
STAMP=$(date +%Y%m%d-%H%M)
WORK=$(mktemp -d /tmp/nai-backup.XXXX)
trap 'rm -rf "$WORK"' EXIT

# 1) 数据库一致性快照（复用部署用的工具；含完整性校验）
/usr/local/bin/naigate-snapshot "$WORK/nai_gate.db.gz"
# 备份里不留生成记录（提示词 / 参数 / 图片指纹），见 naigate-strip-prompts
/usr/local/bin/naigate-strip-prompts "$WORK/nai_gate.db.gz"

# 2) 敏感与配置文件 + 快照 一起打包（存在的才收：upstream_tokens.json 只在接管过令牌池时存在）
FILES=(.env)
for f in data/secret_key data/upstream_tokens.json data/discord_layout.json; do
  [ -f "/opt/service-tools/$f" ] && FILES+=("$f")
done
tar -czf "$WORK/bundle.tar.gz" -C /opt/service-tools "${FILES[@]}"
# 3.0 的部署文件与线上 Caddy 配置
tar -czf "$WORK/owl.tar.gz" -C / opt/owl/compose.yml opt/owl/cutover.sh opt/caddy/Caddyfile

# 3) 单 tar 包（DB 快照 + 配置包），客户端加密（AES-256 + PBKDF2）：S3 只存密文，桶泄露也不泄露成员 Key 与令牌
tar -czf "$WORK/all.tar.gz" -C "$WORK" nai_gate.db.gz bundle.tar.gz owl.tar.gz
openssl enc -aes-256-cbc -pbkdf2 -salt \
  -pass "file:/root/.backup_pass" -in "$WORK/all.tar.gz" -out "$WORK/backup.tar.gz.enc"

# 4) 推送：按日期归档（生命周期 60 天自动过期）+ latest（推送凭证可读，迁移恢复用）
aws s3 cp "$WORK/backup.tar.gz.enc" "s3://${BUCKET}/daily/${STAMP}.tar.gz.enc" --no-progress
aws s3 cp "$WORK/backup.tar.gz.enc" "s3://${BUCKET}/daily/latest/backup.tar.gz.enc" --no-progress
echo "$(date '+%F %T') 备份完成：$(du -h "$WORK/backup.tar.gz.enc" | cut -f1) → s3://${BUCKET}/daily/${STAMP}.tar.gz.enc"
