#!/usr/bin/env bash
# The checks of the sensor simulator.
#
# Usage: tools/ci/obc-sensor-sim.sh [SECTION...]; with no section, every section runs.
#   check    its tests, doctests and device build (its own check.sh)
#   clippy   the host library and tests, and the device binary
set -euo pipefail
cd "$(dirname "$0")/../.."
ci="$PWD/tools/ci"
cd firmware/obc-sensor-sim

check() { bash check.sh; }
clippy() {
  cargo clippy --locked --lib --tests -- -D warnings
  cargo clippy --locked --features device --bin obc-sensor-sim --target thumbv8m.main-none-eabihf -- -D warnings
}

SECTIONS=(check clippy)
. "$ci/sections.sh"
