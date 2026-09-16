#!/usr/bin/env bash
# The body of the CI `test` job. CI runs one section per step so each keeps its own
# name and result artifact; `obc check test` runs the same sections from this file.
#
# Usage: tools/ci/test.sh SECTION...
#   nextest-fast       fast hermetic binaries over the selected packages
#   nextest-fixtures   captured-fixture binaries, after an explicit fixture sync
#   doctests           doctests for the same package set
#   formats-default    obc-formats in the device's default feature shape
#   bench-golden       render + read-counter golden gate
#
# `--all-features` is deliberate: it is the shape the host tests are written for. The
# device's feature shape is checked by the separate formats-default section.
# Set OBC_COVERAGE=1 to run the sections under llvm-cov instrumentation.
set -euo pipefail
cd "$(dirname "$0")/../.."

if [ "${OBC_COVERAGE:-}" = "1" ]; then
  eval "$(cargo llvm-cov show-env --sh)"
fi

# An empty package set means the tier selected nothing. That is not a reason to fall
# back to the whole workspace, and the tier then writes no report for CI to upload.
report() {
  if [ -n "${GITHUB_OUTPUT:-}" ]; then echo "report=true" >> "$GITHUB_OUTPUT"; fi
}

nextest_fast() {
  local packages
  packages="$(python3 tools/ci/rust_packages.py --tier fast)"
  if [ -z "$packages" ]; then echo "no Rust packages selected for the fast tier"; return 0; fi
  export NEXTEST_PROFILE="${NEXTEST_PROFILE:-ci}"
  mkdir -p target
  find target -maxdepth 1 -name '*.profraw' -delete
  rm -f target/nextest/ci/junit.xml
  report
  # shellcheck disable=SC2086
  cargo nextest run $packages --all-features --locked --no-tests fail --filter-expr "$(python3 tools/suite_registry.py cargo-filter --tier fast)"
}

nextest_fixtures() {
  local packages
  packages="$(python3 tools/ci/rust_packages.py --tier fixtures)"
  if [ -z "$packages" ]; then echo "no Rust packages selected for the fixtures tier"; return 0; fi
  export NEXTEST_PROFILE=fixtures
  rm -f target/nextest/fixtures/junit.xml
  report
  python3 tools/fixtures.py sync test
  OBC_FIXTURE_ROOT="$(python3 tools/fixtures.py root)"
  export OBC_FIXTURE_ROOT
  # shellcheck disable=SC2086
  cargo nextest run $packages --all-features --locked --no-tests fail --filter-expr "$(python3 tools/suite_registry.py cargo-filter --tier fixtures)"
}

# nextest runs no doctests, so they run here rather than falling out of the gate.
doctests() {
  local packages
  packages="$(python3 tools/ci/rust_packages.py --tier fast)"
  if [ -z "$packages" ]; then echo "no Rust packages selected for the fast tier"; return 0; fi
  mkdir -p .artifacts/rust
  # shellcheck disable=SC2086
  cargo test $packages --all-features --locked --doc 2>&1 | tee .artifacts/rust/doctests.log
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
    nextest-fast) nextest_fast ;;
    nextest-fixtures) nextest_fixtures ;;
    doctests) doctests ;;
    formats-default) formats_default ;;
    bench-golden) bench_golden ;;
    *) echo "tools/ci/test.sh: unknown section $section" >&2; exit 2 ;;
  esac
done
