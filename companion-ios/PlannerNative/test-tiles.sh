#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../.."
planner_tiles_test_dir=$(mktemp -d)
trap 'rm -rf "$planner_tiles_test_dir"' EXIT
python3 -m venv "$planner_tiles_test_dir/venv"
"$planner_tiles_test_dir/venv/bin/pip" install -q --upgrade "pip>=25.1"
"$planner_tiles_test_dir/venv/bin/pip" install -q --group planner-maps
"$planner_tiles_test_dir/venv/bin/python" companion-ios/PlannerNative/tests/tiles.py "$planner_tiles_test_dir"
swiftc -swift-version 6 companion-ios/PlannerNative/PMTilesArchive.swift companion-ios/PlannerNative/tests/tiles.swift -o "$planner_tiles_test_dir/tiles"
"$planner_tiles_test_dir/tiles" "$planner_tiles_test_dir"
