#!/usr/bin/env bash
# Hosted Ubuntu runners list azure.archive.ubuntu.com first. Switching this
# file is not enough: archive.ubuntu.com often fails DNS too. CI no longer
# installs musl-tools or Playwright OS packages via host apt (see ci.yml).
# Keep this helper for any leftover apt-get on the runner.
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
