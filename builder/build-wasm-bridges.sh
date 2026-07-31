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
