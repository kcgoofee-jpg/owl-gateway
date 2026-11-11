#!/usr/bin/env bash
# Hosted Ubuntu runners list azure.archive.ubuntu.com first. When that name
# does not resolve, apt-get stalls then fails, taking musl-tools and Playwright
# --with-deps with it. Point the mirror list at archive.ubuntu.com instead.
set -euo pipefail
mirrors=/etc/apt/apt-mirrors.txt
if [ ! -f "$mirrors" ]; then
  exit 0
fi
if ! grep -q 'azure.archive.ubuntu.com' "$mirrors"; then
  exit 0
fi
echo 'azure.archive.ubuntu.com is listed first; using archive.ubuntu.com'
sudo tee "$mirrors" >/dev/null <<'EOF'
http://archive.ubuntu.com/ubuntu/
http://security.ubuntu.com/ubuntu/
EOF
