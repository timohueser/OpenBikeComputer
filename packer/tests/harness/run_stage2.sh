#!/usr/bin/env bash
# Stage-2 (quadtree) validation gate: for each corpus item, dump the Python
# oracle's per-LOD simplified feature list + its reference .obcm, build the
# quadtree in Rust from the same features, and compare. See
# packer/docs/rust-port-plan.md §8.2.
#
# Simplify is shared (dumped from Python), so the only *expected* divergence is
# last-digit GEOS-version drift in the boundary clip. On the current corpus the
# system GEOS (3.14) and shapely's (3.13) agree, so this is byte-identical; if a
# future GEOS bump diverges, the gate falls back to the feature-multiset report.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../../.." && pwd)"
PY="${PY:-$REPO/.venv/bin/python}"
CONFIG="$REPO/packer/config.json"
DATA="$REPO/packer/tests/corpus/data"
WORK="${WORK:-/tmp/obcpack-stage2}"
mkdir -p "$WORK"

ITEMS="${ITEMS:-tiny monaco malta freiburg-forest freiburg-town}"
LAND_FLAG="--no-land"; [ "${WITH_LAND:-0}" = "1" ] && LAND_FLAG=""

echo ">> building obc-pack"
cargo build --manifest-path "$REPO/firmware/Cargo.toml" -p obc-pack -q
BUILD="$REPO/firmware/target/debug/build_from_features"
DIFF="$REPO/firmware/target/debug/obcm_diff"

fail=0
for item in $ITEMS; do
  pbf="$DATA/$item.osm.pbf"
  if [ ! -f "$pbf" ]; then echo "SKIP $item (missing $pbf)"; continue; fi
  feat="$WORK/$item.feat.json"; ref="$WORK/$item.ref.obcm"; rust="$WORK/$item.rust.obcm"
  "$PY" "$HERE/dump_features.py" "$pbf" "$CONFIG" "$feat" "$ref" $LAND_FLAG >/dev/null 2>&1
  "$BUILD" "$feat" "$rust" >/dev/null 2>&1
  sz=$(wc -c < "$ref" | tr -d ' ')
  if cmp -s "$ref" "$rust"; then
    printf '  PASS  %-16s byte-identical (%s bytes)\n' "$item" "$sz"
  elif "$DIFF" "$ref" "$rust" >/dev/null 2>&1; then
    printf '  PASS  %-16s feature-multiset identical (bytes differ — clip-order only)\n' "$item"
  else
    printf '  FAIL  %-16s see: %s "%s" "%s"\n' "$item" "$DIFF" "$ref" "$rust"
    fail=1
  fi
done

[ "$fail" = "0" ] && echo "Stage-2 gate: PASS" || { echo "Stage-2 gate: FAIL"; exit 1; }
