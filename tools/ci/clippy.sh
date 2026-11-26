#!/usr/bin/env bash
# The body of the CI `clippy` job.
#
# Usage: tools/ci/clippy.sh [SECTION...]; with no section, every section runs.
#   workspace   clippy over the root workspace, all targets and features
#   ios-host    the iOS portability gate: the shared stack type-checks for the phone
set -euo pipefail
cd "$(dirname "$0")/../.."

workspace() { cargo clippy --workspace --all-targets --all-features --locked -- -D warnings; }
ios_host() { cargo check -p obc-ios-host --target aarch64-apple-ios --locked; }

SECTIONS=(workspace ios-host)
. tools/ci/sections.sh
