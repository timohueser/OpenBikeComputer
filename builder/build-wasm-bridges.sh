#!/usr/bin/env bash
set -euo pipefail

repo_dir="$(cd "$(dirname "$0")/.." && pwd)"
cd "$repo_dir"

wasm-pack build apps/obc-web-convert --release --target web \
  --out-dir ../../builder/app/src/lib/convert/pkg \
  --out-name obc_web_convert

wasm-pack build apps/obc-web-assemble --release --target web \
  --out-dir ../../builder/app/src/lib/assemble/pkg \
  --out-name obc_web_assemble

wasm-pack build apps/obc-skin-preview --release --target web \
  --out-dir ../../builder/app/src/lib/skin/pkg \
  --out-name obc_skin_preview

# The test device: the real flat engine over a real card, for Vitest and the dev harness.
# It lands outside src/ because nothing shipped may import it, and it carries no size budget
# because it is downloaded by tests only.
wasm-pack build host/obc-flat-device --release --target web \
  --out-dir ../../builder/app/test-support/flat-device/pkg \
  --out-name obc_flat_device
