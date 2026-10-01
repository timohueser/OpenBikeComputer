#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../.."
planner_http_test_dir=$(mktemp -d)
trap 'rm -rf "$planner_http_test_dir"' EXIT
swiftc -swift-version 6 apps/planner-native/PlannerHTTPServer.swift apps/planner-native/tests/server.swift -o "$planner_http_test_dir/tests"
"$planner_http_test_dir/tests"
uv run --with-requirements tools/requirements-planner-maps.txt python apps/planner-native/tests/tiles.py "$planner_http_test_dir"
swiftc -swift-version 6 apps/planner-native/PMTilesArchive.swift apps/planner-native/tests/tiles.swift -o "$planner_http_test_dir/tiles"
"$planner_http_test_dir/tiles" "$planner_http_test_dir"
