#!/usr/bin/env bash
set -euo pipefail

repo_dir="$(cd "$(dirname "$0")/.." && pwd)"
cd "$repo_dir"

# Cargo takes a crate as fresh when this checkout's sources are older than the crate's last build, and a
# target that several checkouts share holds one build of each crate: wasm-pack would pack the bridge of
# another checkout. Each checkout builds its bridges in a target of its own.
if [[ -n "${CARGO_TARGET_DIR:-}" ]]; then
  CARGO_TARGET_DIR="$CARGO_TARGET_DIR/checkouts/$(printf '%s' "$repo_dir" | sha256sum | cut -c1-16)"
  export CARGO_TARGET_DIR
fi

wasm-pack build builder/wasm --release --target web \
  --out-dir ../web/src/lib/core/pkg \
  --out-name obc_builder_bridge

# The test device: the real flat engine over a real card, for Vitest and the dev harness.
# It lands outside src/ because nothing shipped may import it, and it carries no size budget
# because it is downloaded by tests only.
wasm-pack build builder/wasm --release --target web \
  --out-dir ../web/test-support/flat-device/pkg \
  --out-name obc_builder_bridge --features test-device

# The TrailView ground plan: the cell build in the viewer's plan worker, the timing page and check 7.
wasm-pack build builder/ground-plan-wasm --release --target web \
  --out-dir ../web/src/lib/trailview/plan/pkg \
  --out-name obc_ground_plan_bridge
