#!/usr/bin/env bash
# The cargo-deny runs of the CI `deny` job: advisories, licences, bans and sources, for each
# Cargo root and its lock file.
#
# Usage: tools/ci/deny.sh [SECTION...]; with no section, every section runs.
#   workspace        the root workspace
#   obc-fw-nrf54l    the board crate, and the feature bans of tools/licenses/board-features.toml
#   obc-boot         the bootloader
#   obc-sensor-sim   the sensor simulator
set -euo pipefail
cd "$(dirname "$0")/../.."

deny() { cargo deny --manifest-path "$1/Cargo.toml" --all-features --config tools/licenses/deny.toml check; }
workspace() { deny .; }
obc_fw_nrf54l() {
  deny firmware/obc-fw-nrf54l
  cargo deny --manifest-path firmware/obc-fw-nrf54l/Cargo.toml --all-features --config tools/licenses/board-features.toml check bans
}
obc_boot() { deny firmware/obc-boot; }
obc_sensor_sim() { deny firmware/obc-sensor-sim; }

SECTIONS=(workspace obc-fw-nrf54l obc-boot obc-sensor-sim)
. tools/ci/sections.sh
