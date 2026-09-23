#!/usr/bin/env bash
# Pack the companion core static library as target/OBCCompanionCore.xcframework, the binary target
# OBCKit's routing module links. Slices: the phone, the simulator, and the Mac `swift test` runs on.
# Name Rust targets to build only those; a slice already on disk for another target is kept, so a
# simulator run does not drop the phone's slice. Release always: a debug assembly is slow.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

all=(aarch64-apple-ios aarch64-apple-ios-sim aarch64-apple-darwin)
wanted=("${@:-${all[@]}}")
for target in "${wanted[@]}"; do
  [[ " ${all[*]} " == *" $target "* ]] || { echo "usage: $(basename "$0") [${all[*]}]..." >&2; exit 2; }
done

slices=()
for target in "${all[@]}"; do
  library="target/$target/release/libobc_companion_core.a"
  if [[ " ${wanted[*]} " == *" $target "* ]]; then
    cargo build -p obc-companion-core --release --locked --target "$target"
  elif [[ ! -f "$library" ]]; then
    continue
  fi
  slices+=(-library "$library" -headers apps/obc-companion-core/include)
done

# -create-xcframework refuses to write over an existing bundle.
rm -rf target/OBCCompanionCore.xcframework
xcodebuild -create-xcframework "${slices[@]}" -output target/OBCCompanionCore.xcframework
