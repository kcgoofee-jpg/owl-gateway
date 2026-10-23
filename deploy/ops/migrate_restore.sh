#!/usr/bin/env bash
# 灾难恢复（3.0）：在新机器上从 S3 拉最新加密备份，恢复数据、配置与部署文件，再起 3.0。
#
# 前提（一次性）：
#   1. 新机器装好 Docker、docker compose、sqlite3、AWS CLI；git clone 本仓库到 /opt/service-tools
#   2. 有能读备份桶的 AWS 凭证（/root/.aws/credentials 的 [naigate-push]，或站长 IAM 凭证；
#      没 AWS CLI 时也可从控制台手动下载 daily/latest/backup.tar.gz.enc，用 BACKUP_FILE=路径 指定）
#   3. 加密口令（原服务器 /root/.backup_pass 的内容）——站长须在服务器之外另存
#   4. 镜像与运维命令：本机 `gh run download -n owl-gate-image -n nai-ops-linux-musl` 后传到 /opt/owl/
#      （服务器上不放 GitHub 凭据）
#
# 用法：bash deploy/ops/migrate_restore.sh      口令交互输入（不出现在进程列表里）
set -euo pipefail
cd "$(dirname "$0")/../.."                   # /opt/service-tools
WORK=$(mktemp -d /tmp/nai-restore.XXXX)
trap 'rm -rf "$WORK"' EXIT

echo "== 1/6 取最新备份 =="
if [[ -n ${BACKUP_FILE:-} ]]; then
  cp "$BACKUP_FILE" "$WORK/backup.tar.gz.enc"
else
  export AWS_PROFILE=${AWS_PROFILE:-naigate-push}
  ACC=$(aws sts get-caller-identity --query Account --output text 2>/dev/null) \
    || { echo "取不到 AWS 身份：先配好 /root/.aws/credentials，或用 BACKUP_FILE=本地文件"; exit 1; }
  aws s3 cp "s3://nai-gate-backups-${ACC}/daily/latest/backup.tar.gz.enc" "$WORK/backup.tar.gz.enc" --no-progress
fi

echo "== 2/6 解密 =="
if [[ -z ${BACKUP_PASSPHRASE:-} ]]; then
  read -rsp "加密口令（原服务器 /root/.backup_pass 的内容）：" BACKUP_PASSPHRASE; echo
fi
export BACKUP_PASSPHRASE
openssl enc -d -aes-256-cbc -pbkdf2 -pass env:BACKUP_PASSPHRASE \
  -in "$WORK/backup.tar.gz.enc" -out "$WORK/all.tar.gz"
unset BACKUP_PASSPHRASE

echo "== 3/6 拆包 =="
mkdir -p "$WORK/split"
tar -xzf "$WORK/all.tar.gz" -C "$WORK/split"
tar -xzf "$WORK/split/bundle.tar.gz" -C "$WORK/split"

echo "== 4/6 恢复数据与配置 =="
mkdir -p /opt/service-tools/data
gunzip -c "$WORK/split/nai_gate.db.gz" > /opt/service-tools/data/nai_gate.db
sqlite3 /opt/service-tools/data/nai_gate.db "PRAGMA integrity_check;" | grep -qx ok || { echo "数据库校验失败"; exit 1; }
cp "$WORK/split/.env" /opt/service-tools/.env
for f in secret_key upstream_tokens.json discord_layout.json; do
  [[ -f "$WORK/split/data/$f" ]] && cp "$WORK/split/data/$f" /opt/service-tools/data/
done
chown -R 10001:10001 /opt/service-tools/data
chmod 600 /opt/service-tools/.env

echo "== 5/6 恢复 3.0 部署文件与 Caddy 配置 =="
if [[ -f "$WORK/split/owl.tar.gz" ]]; then
  tar -xzf "$WORK/split/owl.tar.gz" -C /         # /opt/owl/compose.yml、cutover.sh、/opt/caddy/Caddyfile
else
  mkdir -p /opt/owl /opt/caddy                     # 旧备份里没有：用仓库里的
  cp deploy/v3/compose.yml deploy/v3/cutover.sh /opt/owl/
  cp deploy/Caddyfile.production /opt/caddy/Caddyfile
fi
mkdir -p /opt/owl/ops && cp deploy/v3/ops/* /opt/owl/ops/ && chmod 750 /opt/owl/ops/*

echo "== 6/6 启动 =="
[[ -f /opt/owl/owl-gate.tar.gz && -f /opt/owl/nai-ops ]] \
  || { echo "先把 CI 产出的 owl-gate.tar.gz 和 nai-ops 放到 /opt/owl/ 再重跑本步"; exit 1; }
docker network inspect service-tools_edge >/dev/null 2>&1 \
  || docker network create --subnet 172.30.0.0/24 service-tools_edge
/opt/owl/cutover.sh load /opt/owl/owl-gate.tar.gz
/opt/owl/cutover.sh install-ops /opt/owl/nai-ops
docker compose -p owl --env-file /opt/service-tools/.env -f /opt/owl/compose.yml up -d
for _ in $(seq 1 30); do curl -fsS -m 2 http://127.0.0.1:3003/healthz >/dev/null 2>&1 && break; sleep 2; done
curl -fsS http://127.0.0.1:3003/healthz && echo
echo "恢复完成。剩下：起 Caddy（配置在 /opt/caddy/Caddyfile，接 service-tools_edge 网络）、"
echo "装定时任务（deploy/harden.sh 与 docs/OPERATIONS.md）、把域名 A 记录切到本机。"
