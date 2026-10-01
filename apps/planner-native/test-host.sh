#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../.."
node --check apps/planner-native/build.mjs
node --check apps/planner-search/native-build.mjs
node --check host/route-engine/examples/phone/planner-host-benchmark.js
python3 -m py_compile host/route-engine/examples/phone/prepare-parser.py host/route-engine/examples/phone/parser_notices.py
build=$(mktemp -d)
trap 'rm -rf "$build"' EXIT
xcrun swiftc -swift-version 6 -warnings-as-errors -O -whole-module-optimization -emit-object \
  -target arm64-apple-ios17.0 -sdk "$(xcrun --sdk iphoneos --show-sdk-path)" \
  apps/planner-search/native/*.swift apps/planner-native/*.swift \
  host/route-engine/examples/phone/PlannerHostBenchmark.swift -o "$build/host.o"
