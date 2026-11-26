#!/usr/bin/env bash
# The checks of the board crate. It runs from its own directory, so its .cargo/config.toml
# selects the thumbv8m target. It has no test harness, so it gets clippy and build only. Every
# section cross-compiles the FLPR blob with a RISC-V gcc.
#
# Usage: tools/ci/obc-fw-nrf54l.sh [SECTION...]; with no section, every section runs.
#   clippy   the default image and the debug-uart image
#   build    the release image; with OBC_BUILD_LOG set, the -vv log goes to that file
set -euo pipefail
cd "$(dirname "$0")/../.."
ci="$PWD/tools/ci"
cd firmware/obc-fw-nrf54l

clippy() {
  cargo clippy --locked -- -D warnings
  cargo clippy --locked --features debug-uart -- -D warnings
}
build() { cargo build --release --locked ${OBC_BUILD_LOG:+-vv} 2>&1 | tee "${OBC_BUILD_LOG:-/dev/null}"; }

SECTIONS=(clippy build)
. "$ci/sections.sh"
