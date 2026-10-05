#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
case "${PLATFORM_NAME:-iphonesimulator}" in
  iphoneos) planner_target=aarch64-apple-ios ;;
  iphonesimulator) planner_target=aarch64-apple-ios-sim ;;
  *) echo "Unsupported planner platform" >&2; exit 1 ;;
esac
cargo build --locked --release -p route-server --lib --no-default-features --target "$planner_target"
