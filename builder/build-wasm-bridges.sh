#!/usr/bin/env bash
set -euo pipefail

repo_dir="$(cd "$(dirname "$0")/.." && pwd)"
cd "$repo_dir"

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
