#!/usr/bin/env bash
# The wasm builds of the CI `wasm` job.
#
# Usage: tools/ci/wasm.sh [SECTION...]; with no section, every section runs.
#   web-demo        the web demo, through Trunk
#   obcm-assemble   the assembly engine for wasm32 with the `cli` feature off. The CLI owns every
#                   std::fs call of the crate, so a filesystem or native dependency in the engine,
#                   which the browser runs, fails here.
set -euo pipefail
cd "$(dirname "$0")/../.."

web_demo() { trunk build --config docs/Trunk.toml; }
obcm_assemble() { cargo build -p obcm-assemble --lib --no-default-features --locked --target wasm32-unknown-unknown; }

SECTIONS=(web-demo obcm-assemble)
. tools/ci/sections.sh
