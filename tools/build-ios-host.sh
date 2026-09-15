#!/usr/bin/env bash
# Pack the iOS host static library as target/OBCHost.xcframework — the artifact the OBCDevice app
# links. Both slices by default. `--sim-only` builds the simulator slice alone, which is what a
# simulator run needs, and keeps whatever device slice is already on disk: dropping it would leave
# the bundle with no library for a phone, and Xcode refuses to plan a device build against that.
# Release always: a debug Rust build cannot hold a 60 Hz loop.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

usage() { echo "usage: $(basename "$0") [--sim-only]" >&2; exit 2; }

sim_only=""
case "${1:-}" in
  --sim-only) sim_only=yes ;;
  "") ;;
  *) usage ;;
esac
(( $# <= 1 )) || usage

slices=()
for target in aarch64-apple-ios aarch64-apple-ios-sim; do
  library="target/$target/release/libobc_ios_host.a"
  if [[ -z "$sim_only" || "$target" == aarch64-apple-ios-sim ]]; then
    cargo build -p obc-ios-host --release --locked --target "$target"
  elif [[ ! -f "$library" ]]; then
    # Nothing to keep: this run packs the simulator alone.
    continue
  fi
  slices+=(-library "$library" -headers apps/obc-ios-host/include)
done

# -create-xcframework refuses to write over an existing bundle.
rm -rf target/OBCHost.xcframework
xcodebuild -create-xcframework "${slices[@]}" -output target/OBCHost.xcframework
