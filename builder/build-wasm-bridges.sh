#!/usr/bin/env bash
set -euo pipefail

repo_dir="$(cd "$(dirname "$0")/.." && pwd -P)"
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

# The TrailView tunnel parts: the stamp code inlines it, and its stamp version takes the hash of the tracked sources
# that built it but their Markdown (`partsSources` of builder/web/vite/stamp-bundle.ts), which sources.sha256 records.
parts_pkg=builder/web/src/lib/trailview/structures/pkg
wasm-pack build builder/trailview-parts-wasm --release --target web \
  --out-dir ../web/src/lib/trailview/structures/pkg \
  --out-name obc_trailview_parts_bridge
git ls-files -z -- host/obc-trailview-parts builder/trailview-parts-wasm data/trailview/parts.json ':(exclude)*.md' \
  | LC_ALL=C sort -z | xargs -0 sha256sum | sha256sum | cut -d' ' -f1 > "$parts_pkg/sources.sha256"

# The exact WebP decode of TrailView's workers where the browser's decode is not exact.
wasm-pack build builder/webp-wasm --release --target web \
  --out-dir ../web/src/lib/trailview/stream/webp \
  --out-name obc_webp_bridge
