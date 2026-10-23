#!/usr/bin/env bash
# Ubuntu/Debian 服务器基础加固（需 root，可重复执行）：
# 防火墙(22/80/443)、SSH 仅密钥登录 + fail2ban、自动安全更新、1GB 交换分区、SQLite 每晚 03:30 本机备份（GFS 14/8/6：每日 14 份、每周 8 份、每月 6 份）。
# 运行前请确认你已经能用密钥登录——脚本会在 authorized_keys 为空时跳过关闭密码登录，避免把自己锁在外面。
set -euo pipefail
[ "$(id -u)" = 0 ] || { echo "请用 root 运行"; exit 1; }
export DEBIAN_FRONTEND=noninteractive
apt-get update -q && apt-get install -y -q ufw fail2ban unattended-upgrades sqlite3

# 先放行当前实际的 SSH 端口（改过端口的机器若只放行 22 会把自己锁在外面）。
ssh_port="$(sshd -T 2>/dev/null | awk '/^port /{print $2; exit}')"; ssh_port="${ssh_port:-22}"
ufw default deny incoming >/dev/null; ufw default allow outgoing >/dev/null
ufw allow "${ssh_port}/tcp" >/dev/null; ufw allow 80/tcp >/dev/null; ufw allow 443/tcp >/dev/null; ufw allow 443/udp >/dev/null
ufw --force enable >/dev/null

cat > /etc/fail2ban/jail.local <<'CONF'
[DEFAULT]
bantime = 1h
findtime = 10m
maxretry = 5
[sshd]
enabled = true
CONF
systemctl enable --now fail2ban >/dev/null 2>&1

if [ -s /root/.ssh/authorized_keys ]; then
  cat > /etc/ssh/sshd_config.d/00-hardening.conf <<'CONF'
PasswordAuthentication no
KbdInteractiveAuthentication no
PermitRootLogin prohibit-password
MaxAuthTries 3
CONF
  sshd -t && { systemctl reload ssh 2>/dev/null || systemctl reload sshd; }
else
  echo "⚠ /root/.ssh/authorized_keys 为空：已跳过关闭 SSH 密码登录。"
fi

printf 'APT::Periodic::Update-Package-Lists "1";\nAPT::Periodic::Unattended-Upgrade "1";\n' > /etc/apt/apt.conf.d/20auto-upgrades
echo 'Unattended-Upgrade::Automatic-Reboot "false";' > /etc/apt/apt.conf.d/52no-reboot

if ! swapon --show | grep -q swapfile; then
  fallocate -l 1G /swapfile && chmod 600 /swapfile && mkswap /swapfile >/dev/null && swapon /swapfile
  grep -q '^/swapfile' /etc/fstab || echo '/swapfile none swap sw 0 0' >> /etc/fstab
  echo 'vm.swappiness=10' > /etc/sysctl.d/99-swap.conf && sysctl -q -p /etc/sysctl.d/99-swap.conf
fi

# 本地每晚备份（GFS 轮换）：快照走 nai-ops（只读事务逐表复制，不锁库）——不要再装宿主机 sqlite3 .backup 的版本，
# 库大时整库复制会把磁盘 IO 打满、网关写库超时（2026-10-10 事故）。需要先有 /opt/owl/nai-ops（cutover.sh install-ops）。
mkdir -p /opt/backups
REPO_DIR=$(cd "$(dirname "$0")/.." && pwd)
install -m 755 "$REPO_DIR/deploy/v3/ops/naigate-snapshot" /usr/local/bin/naigate-snapshot
install -m 755 "$REPO_DIR/deploy/v3/ops/naigate-backup" /usr/local/bin/naigate-backup
install -m 755 "$REPO_DIR/deploy/v3/ops/naigate-strip-prompts" /usr/local/bin/naigate-strip-prompts
echo '30 3 * * * root /usr/local/bin/naigate-backup >> /var/log/naigate-backup.log 2>&1' > /etc/cron.d/naigate-backup
install -m 755 "$REPO_DIR/deploy/v3/ops/asn-update.sh" /usr/local/bin/naigate-asn-update
echo '50 4 * * 1 root /usr/local/bin/naigate-asn-update >> /var/log/naigate-asn.log 2>&1' > /etc/cron.d/naigate-asn

# Docker 日志轮转，避免日志撑满磁盘（修改后需重启 docker 并重建容器）
if command -v docker >/dev/null && [ ! -f /etc/docker/daemon.json ]; then
  echo '{"log-driver":"json-file","log-opts":{"max-size":"10m","max-file":"3"}}' > /etc/docker/daemon.json
  echo "已写入 Docker 日志轮转；请执行 systemctl restart docker 后重建容器。"
fi
echo "完成。"
