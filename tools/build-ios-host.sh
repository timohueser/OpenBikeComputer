#!/usr/bin/env bash
# Pack the iOS host static library as target/OBCHost.xcframework — the artifact the OBCDevice app
# links. Both slices by default; `--sim-only` builds and packs the simulator alone, which is what
# a simulator run needs. Release always: a debug Rust build cannot hold a 60 Hz loop.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

usage() { echo "usage: $(basename "$0") [--sim-only]" >&2; exit 2; }

targets=(aarch64-apple-ios aarch64-apple-ios-sim)
case "${1:-}" in
  --sim-only) targets=(aarch64-apple-ios-sim) ;;
  "") ;;
  *) usage ;;
esac
(( $# <= 1 )) || usage

slices=()
for target in "${targets[@]}"; do
  cargo build -p obc-ios-host --release --locked --target "$target"
  slices+=(-library "target/$target/release/libobc_ios_host.a" -headers apps/obc-ios-host/include)
done

# -create-xcframework refuses to write over an existing bundle.
rm -rf target/OBCHost.xcframework
xcodebuild -create-xcframework "${slices[@]}" -output target/OBCHost.xcframework
