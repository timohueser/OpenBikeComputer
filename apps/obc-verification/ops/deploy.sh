#!/bin/bash
# Forced SSH command: accepts one built application archive on stdin.
set -euo pipefail
umask 022
root=/opt/obc-verification
mkdir -p "$root/releases"
exec 9>"$root/deploy.lock"
flock -n 9 || { echo 'Another deployment is active.' >&2; exit 1; }
staging=$(mktemp -d "$root/releases/deploy-XXXXXXXX")
trap 'rm -rf "$staging"' EXIT
python3 -c '
import sys, tarfile
from pathlib import Path
root = Path(sys.argv[1])
with tarfile.open(fileobj=sys.stdin.buffer, mode="r|gz") as archive:
    archive.extractall(root, filter="data")
assert (root / "build/index.js").is_file(), "Missing application entry point"
assert (root / "package.json").is_file(), "Missing package metadata"
assert (root / "deployment.json").is_file(), "Missing deployment identity"
' "$staging"
revision=$(python3 -c 'import json,re,sys; s=json.load(open(sys.argv[1]))["sourceSha"]; assert re.fullmatch(r"[0-9a-f]{40}",s); print(s)' "$staging/deployment.json")
release="$root/releases/$revision-$(date -u +%Y%m%dT%H%M%S)"
[ ! -e "$release" ] || { echo "Deployment directory already exists." >&2; exit 1; }
chown -R root:root "$staging"
chmod -R go-w "$staging"
mv "$staging" "$release"
previous=$(readlink "$root/current" || true)
ln -s "$release" "$root/next"
mv -Tf "$root/next" "$root/current"
systemctl restart obc-verification
healthy=false
for attempt in $(seq 1 20); do
  if curl --fail --silent http://127.0.0.1:3100/health >/dev/null; then healthy=true; break; fi
  sleep 1
done
if [ "$healthy" != true ]; then
  if [ -n "$previous" ]; then
    ln -s "$previous" "$root/next"
    mv -Tf "$root/next" "$root/current"
    systemctl restart obc-verification
  else
    systemctl stop obc-verification
  fi
  echo 'Deployment failed health check; previous application restored when available.' >&2
  exit 1
fi
if [ -n "$previous" ]; then ln -sfn "$previous" "$root/previous"; fi
systemctl enable obc-verification >/dev/null
echo "Deployed $revision"
