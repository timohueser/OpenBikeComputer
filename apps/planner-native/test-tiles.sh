#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../.."
planner_tiles_test_dir=$(mktemp -d)
trap 'rm -rf "$planner_tiles_test_dir"' EXIT
python3 -m venv "$planner_tiles_test_dir/venv"
"$planner_tiles_test_dir/venv/bin/pip" install -q -r tools/requirements-planner-maps.txt
"$planner_tiles_test_dir/venv/bin/python" apps/planner-native/tests/tiles.py "$planner_tiles_test_dir"
swiftc -swift-version 6 apps/planner-native/PMTilesArchive.swift apps/planner-native/tests/tiles.swift -o "$planner_tiles_test_dir/tiles"
"$planner_tiles_test_dir/tiles" "$planner_tiles_test_dir"
