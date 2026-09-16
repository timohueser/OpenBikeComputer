#!/bin/bash
# Run as root on Debian. Installs runtime/service, but never supplies credentials.
set -euo pipefail
[ "$(id -u)" -eq 0 ] || { echo 'Run as root.' >&2; exit 1; }
script_dir=$(cd -- "$(dirname -- "$0")" && pwd)
node_version=v24.21.0
case "$(uname -m)" in x86_64) architecture=x64;; aarch64) architecture=arm64;; *) exit 1;; esac
apt-get update
apt-get install -y ca-certificates curl xz-utils python3 caddy sudo
if [ "$(/usr/local/bin/node --version 2>/dev/null || true)" != "$node_version" ]; then
  staging=$(mktemp -d)
  trap 'rm -rf "$staging"' EXIT
  archive="node-$node_version-linux-$architecture.tar.xz"
  curl --fail --silent --show-error "https://nodejs.org/dist/$node_version/$archive" -o "$staging/$archive"
  curl --fail --silent --show-error "https://nodejs.org/dist/$node_version/SHASUMS256.txt" -o "$staging/SHASUMS256.txt"
  (cd "$staging" && grep "  $archive\$" SHASUMS256.txt | sha256sum --check -)
  tar -xJf "$staging/$archive" -C /opt
  ln -sfn "/opt/node-$node_version-linux-$architecture/bin/node" /usr/local/bin/node
fi
id obc-verification >/dev/null 2>&1 || useradd --system --home-dir /var/lib/obc-verification --shell /usr/sbin/nologin obc-verification
id obc-verification-deploy >/dev/null 2>&1 || useradd --system --create-home --home-dir /var/lib/obc-verification-deploy --shell /bin/bash obc-verification-deploy
install -d -o obc-verification -g obc-verification -m 0700 /var/lib/obc-verification
install -d -m 0755 /opt/obc-verification/releases
install -d -m 0700 /etc/obc-verification /var/backups/obc-verification
install -m 0755 "$script_dir/deploy.sh" /usr/local/sbin/obc-verification-deploy
install -m 0755 "$script_dir/backup.py" /usr/local/sbin/obc-verification-backup
install -m 0644 "$script_dir/obc-verification.service" /etc/systemd/system/obc-verification.service
printf '%s\n' 'obc-verification-deploy ALL=(root) NOPASSWD: /usr/local/sbin/obc-verification-deploy' > /etc/sudoers.d/obc-verification-deploy
chmod 0440 /etc/sudoers.d/obc-verification-deploy
visudo -cf /etc/sudoers.d/obc-verification-deploy
install -m 0644 "$script_dir/obc-verification-backup.service" /etc/systemd/system/obc-verification-backup.service
install -m 0644 "$script_dir/obc-verification-backup.timer" /etc/systemd/system/obc-verification-backup.timer
systemctl daemon-reload
systemctl enable --now obc-verification-backup.timer
printf '%s\n' 'Runtime installed. Configure service.env, authorized_keys, and Caddy before deployment.'
