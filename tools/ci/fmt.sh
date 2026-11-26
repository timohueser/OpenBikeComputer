#!/usr/bin/env bash
# The body of the CI `fmt` job: `cargo fmt --check` for every Cargo root. rustfmt walks up from
# each file to the shared .rustfmt.toml, so the standalone roots inherit it. Every root is checked
# before the script fails, so one run names every unformatted root.
set -uo pipefail
cd "$(dirname "$0")/../.."
rc=0
cargo fmt --all --check || rc=1
for root in firmware/obc-fw-nrf54l firmware/obc-boot firmware/obc-sensor-sim; do
  cargo fmt --check --manifest-path "$root/Cargo.toml" || rc=1
done
exit "$rc"
