#!/usr/bin/env bash
# Deploy the code of the follower `obc data serve prod` and of the planner services to the VPS.
# Data never goes this way: the follower reads the bucket. tools/vps/README.md prepares the VPS.
#
#   tools/vps/deploy-services.sh [--dry-run] HOST [COMMIT]
#
# HOST is an SSH host that logs in as root. The user obc-data builds `git archive COMMIT` (default
# HEAD) in /opt/obc-data/<commit> on the VPS, for the target of data/planner-runtime.toml. Then
# /opt/obc-data/current points at it, the follower restarts, and /opt/obc-data/previous keeps the
# build before. Other builds go.
# `--dry-run` prints the commands and runs none.
set -euo pipefail
cd "$(dirname "$0")/../.."

dry=0
if [[ "${1:-}" == --dry-run ]]; then dry=1; shift; fi
host="${1:?Usage: tools/vps/deploy-services.sh [--dry-run] HOST [COMMIT]}"
commit="$(git rev-parse --verify "${2:-HEAD}^{commit}")"
read -r triple python node < <(python3 -c 'import tomllib; t = tomllib.load(open("data/planner-runtime.toml", "rb"))["target"]; print(t["triple"], t["python"], t["node"])')
base="/opt/obc-data/$commit"

extract="runuser -u obc-data -- sh -c 'mkdir -p $base && tar -x -C $base'"
build="set -euo pipefail
export PATH=\$HOME/.cargo/bin:\$HOME/.local/bin:\$PATH
cd $base
test \"\$(rustc -vV | sed -n 's/^host: //p')\" = $triple || { echo 'The VPS is not $triple.' >&2; exit 1; }
test \"\$(python3 -c 'import sys; print(*sys.version_info[:2], sep=\".\")')\" = $python || { echo 'The VPS needs Python $python.' >&2; exit 1; }
test \"\$(node -p 'process.versions.node.split(\".\").slice(0, 2).join(\".\")')\" = $node || { echo 'The VPS needs Node $node.' >&2; exit 1; }
cargo build --locked --release --bin obc-data-plumbing --bin planner-service
npm ci --prefix planner/search
UV_PROJECT_ENVIRONMENT=planner/search/.venv uv sync --locked --python $python --group search-runtime"
switch="set -euo pipefail
old=\$(readlink /opt/obc-data/current || true)
if [ -n \"\$old\" ] && [ \"\$old\" != $base ]; then ln -sfn \"\$old\" /opt/obc-data/previous; fi
ln -sfn $base /opt/obc-data/current.next
mv -T /opt/obc-data/current.next /opt/obc-data/current
systemctl restart obc-data-serve
keep=\$(readlink /opt/obc-data/previous || echo $base)
find /opt/obc-data -mindepth 1 -maxdepth 1 -type d ! -path $base ! -path \"\$keep\" -exec rm -rf {} +"

if (( dry )); then
  printf 'git archive --format=tar %s | ssh %s "%s"\n' "$commit" "$host" "$extract"
  printf 'ssh %s runuser -u obc-data -- bash -s <<'"'"'BUILD'"'"'\n%s\nBUILD\n' "$host" "$build"
  printf 'ssh %s bash -s <<'"'"'SWITCH'"'"'\n%s\nSWITCH\n' "$host" "$switch"
  exit 0
fi
git archive --format=tar "$commit" | ssh "$host" "$extract"
ssh "$host" runuser -u obc-data -- bash -s <<< "$build"
ssh "$host" bash -s <<< "$switch"
