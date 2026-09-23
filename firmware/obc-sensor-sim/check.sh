#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")"
cargo nextest run --locked --no-tests fail
cargo test --doc --locked
cargo build --release --locked --features device --target thumbv8m.main-none-eabihf
