#!/usr/bin/env bash
# Check 7: every fixture cell gives the same plan digest natively and in WASM.
# Needs the package of builder/build-wasm-bridges.sh.
set -euo pipefail

repo_dir="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$repo_dir"
# The requests of every fixture cell: about 320 cells at the largest request.
limit_mib=512

dir="$(mktemp -d)"
trap 'rm -rf "$dir"' EXIT
cargo run --locked --release -p obc-ground-plan --example debug -- requests --all --runs 1 --out "$dir"
size_mib=$(du -sm "$dir" | cut -f1)
echo "requests: ${size_mib} MiB"
if [ "$size_mib" -gt "$limit_mib" ]; then
  echo "::error::the fixture's requests take ${size_mib} MiB, more than ${limit_mib} MiB"
  exit 1
fi
node builder/ground-plan-wasm/check.mjs "$dir"
