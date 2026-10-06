#!/usr/bin/env bash
# Pack the existing OBCHost static library for the selected Apple surface. Keep other slices
# already on disk so a simulator or package-test build does not remove a phone's library.
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"
usage() { echo "usage: $(basename "$0") [--sim-only|--mac-only]" >&2; exit 2; }
mode="${1:-all}"
case "$mode" in all|--sim-only|--mac-only) ;; *) usage ;; esac
(( $# <= 1 )) || usage
mac_target="$(rustc -vV | sed -n 's/^host: //p')"
case "$mac_target" in *-apple-darwin) ;; *) echo "OBCHost packaging requires macOS" >&2; exit 1 ;; esac
build_dir="${CARGO_TARGET_DIR:-$root/target}"
[[ "$build_dir" == /* ]] || build_dir="$root/$build_dir"
slices=()
for target in aarch64-apple-ios aarch64-apple-ios-sim "$mac_target"; do
  library="$build_dir/$target/release/libobc_ios_host.a"
  [[ "$target" != "$mac_target" ]] || library="$build_dir/release/libobc_ios_host.a"
  if [[ "$mode" == all && "$target" != "$mac_target" ]] ||
     [[ "$mode" == --sim-only && "$target" == aarch64-apple-ios-sim ]] ||
     [[ "$mode" == --mac-only && "$target" == "$mac_target" ]]; then
    if [[ "$target" == "$mac_target" ]]; then
      env -u CARGO_BUILD_TARGET cargo build -p obc-ios-host --release --locked
    else
      cargo build -p obc-ios-host --release --locked --target "$target"
    fi
  elif [[ ! -f "$library" ]]; then
    continue
  fi
  slices+=(-library "$library" -headers apps/obc-ios-host/include)
done
framework="$build_dir/OBCHost.xcframework"
rm -rf "$framework"
xcodebuild -create-xcframework "${slices[@]}" -output "$framework"
mkdir -p companion-ios/Packages/OBCKit/.swiftpm
ln -sfn "$framework" companion-ios/Packages/OBCKit/.swiftpm/OBCHost.xcframework
