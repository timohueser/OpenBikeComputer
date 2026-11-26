#!/usr/bin/env bash
# The checks of the bootloader. It runs from its own directory, so its .cargo/config.toml
# selects the thumbv8m target. It has no test harness and no FLPR blob.
#
# Usage: tools/ci/obc-boot.sh [SECTION...]; with no section, every section runs.
#   clippy   the default image and the rtt image
#   build    the release image; with OBC_BUILD_LOG set, the -vv log goes to that file
set -euo pipefail
cd "$(dirname "$0")/../.."
ci="$PWD/tools/ci"
cd firmware/obc-boot

clippy() {
  cargo clippy --locked -- -D warnings
  cargo clippy --locked --features rtt -- -D warnings
}
build() { cargo build --release --locked ${OBC_BUILD_LOG:+-vv} 2>&1 | tee "${OBC_BUILD_LOG:-/dev/null}"; }

SECTIONS=(clippy build)
. "$ci/sections.sh"
