#!/usr/bin/env bash
# The body of the CI `test` job. CI runs one section per step so each keeps its own
# name and result artifact; `obc check test` runs the same sections from this file.
#
# Usage: tools/ci/test.sh SECTION...
#   nextest-fast       fast hermetic binaries
#   nextest-fixtures   captured-fixture binaries, after an explicit fixture sync
#   doctests           workspace doctests
#   formats-default    obc-formats in the device's default feature shape
#   bench-golden       render + read-counter golden gate
#
# Every section runs in a subshell, so one section's exported environment cannot reach
# the next when several run in one invocation.
#
# `--all-features` is deliberate: it is the shape the host tests are written for. The
# device's feature shape is checked by the separate formats-default section.
set -euo pipefail
cd "$(dirname "$0")/../.."

nextest_fast() {
  export NEXTEST_PROFILE=ci
  rm -f target/nextest/ci/junit.xml
  cargo nextest run --workspace --all-features --locked --no-tests fail --filter-expr "$(python3 tools/test_plan.py cargo-filter --tier fast)"
}

nextest_fixtures() {
  export NEXTEST_PROFILE=fixtures
  rm -f target/nextest/fixtures/junit.xml
  python3 tools/fixtures.py sync test
  OBC_FIXTURE_ROOT="$(python3 tools/fixtures.py root)"
  export OBC_FIXTURE_ROOT
  cargo nextest run --workspace --all-features --locked --no-tests fail --filter-expr "$(python3 tools/test_plan.py cargo-filter --tier fixtures)"
}

# nextest runs no doctests, so they run here rather than falling out of the gate.
doctests() {
  mkdir -p .artifacts/rust
  cargo test --workspace --all-features --locked --doc 2>&1 | tee .artifacts/rust/doctests.log
}

formats_default() {
  mkdir -p .artifacts/rust
  cargo test -p obc-formats --locked 2>&1 | tee .artifacts/rust/formats-default.log
}

# Re-render the fixed bench scenes and route-corridor cases and fail if any frame hash
# or read counter drifts from the committed golden file, so a cache change that halves
# the hit rate cannot pass on identical pixels. Timings print but are never gated.
bench_golden() {
  cargo run -p obc-bench --release --locked -- --check host/obc-bench/golden.txt
}

[ "$#" -gt 0 ] || { echo "tools/ci/test.sh: name at least one section" >&2; exit 2; }
for section in "$@"; do
  case "$section" in
    nextest-fast) ( nextest_fast ) ;;
    nextest-fixtures) ( nextest_fixtures ) ;;
    doctests) ( doctests ) ;;
    formats-default) ( formats_default ) ;;
    bench-golden) ( bench_golden ) ;;
    *) echo "tools/ci/test.sh: unknown section $section" >&2; exit 2 ;;
  esac
done
