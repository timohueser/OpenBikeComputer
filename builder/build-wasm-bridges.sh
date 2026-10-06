#!/usr/bin/env bash
set -euo pipefail

repo_dir="$(cd "$(dirname "$0")/.." && pwd)"
cd "$repo_dir"

wasm-pack build builder/wasm --release --target web \
  --out-dir ../app/src/lib/core/pkg \
  --out-name obc_builder_bridge

# The test device: the real flat engine over a real card, for Vitest and the dev harness.
# It lands outside src/ because nothing shipped may import it, and it carries no size budget
# because it is downloaded by tests only.
wasm-pack build builder/wasm --release --target web \
  --out-dir ../app/test-support/flat-device/pkg \
  --out-name obc_builder_bridge --features test-device
