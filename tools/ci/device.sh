#!/usr/bin/env bash
# The body of the CI `device` job: the shared no_std crates still compile for the nRF54L target.
#
# Usage: tools/ci/device.sh [SECTION...]; with no section, every section runs.
#   obc-app    the app and the crates it pulls in (reader, route, render)
#   obc-link   the protocol engine and codec; the fixture producer is host-only
set -euo pipefail
cd "$(dirname "$0")/../.."

obc_app() { cargo build -p obc-app --target thumbv8m.main-none-eabihf --locked; }
obc_link() { cargo build -p obc-link --target thumbv8m.main-none-eabihf --locked; }

SECTIONS=(obc-app obc-link)
. tools/ci/sections.sh
