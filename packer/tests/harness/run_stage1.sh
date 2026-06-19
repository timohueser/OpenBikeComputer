#!/usr/bin/env bash
# Stage-1 (serializer) validation gate: for each corpus item, have the Python
# oracle build + serialize the quadtrees AND dump them, re-serialize the dump in
# Rust, and assert byte-identical .obcm. See packer/docs/rust-port-plan.md §8.1.
#
# Byte-identity IS the right gate here (unlike the end-to-end pipeline): given the
# same captured tree, serialization is deterministic integer work. A failure is a
# real serializer divergence — bisect it with `obcm_diff`.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../../.." && pwd)"
PY="${PY:-$REPO/.venv/bin/python}"
CONFIG="$REPO/packer/config.json"
DATA="$REPO/packer/tests/corpus/data"
WORK="${WORK:-/tmp/obcpack-stage1}"
mkdir -p "$WORK"

# Items to run (override with `ITEMS="tiny monaco" run_stage1.sh`). Land is
# skipped by default (the serializer is land-agnostic and it avoids the 1.2 GB
# shapefile); pass WITH_LAND=1 to exercise the land path too.
ITEMS="${ITEMS:-tiny monaco malta freiburg-forest freiburg-town}"
LAND_FLAG="--no-land"; [ "${WITH_LAND:-0}" = "1" ] && LAND_FLAG=""

echo ">> building obc-pack"
cargo build --manifest-path "$REPO/firmware/Cargo.toml" -p obc-pack -q
DUMP="$REPO/firmware/target/debug/serialize_from_dump"

fail=0
for item in $ITEMS; do
  pbf="$DATA/$item.osm.pbf"
  if [ ! -f "$pbf" ]; then echo "SKIP $item (missing $pbf)"; continue; fi
  d="$WORK/$item.dump.json"; ref="$WORK/$item.ref.obcm"; rust="$WORK/$item.rust.obcm"
  "$PY" "$HERE/dump_tree.py" "$pbf" "$CONFIG" "$d" "$ref" $LAND_FLAG >/dev/null 2>&1
  "$DUMP" "$d" "$rust" >/dev/null 2>&1
  sz=$(wc -c < "$ref" | tr -d ' ')
  if cmp -s "$ref" "$rust"; then
    printf '  PASS  %-16s byte-identical (%s bytes)\n' "$item" "$sz"
  else
    printf '  FAIL  %-16s %s\n' "$item" "$(cmp "$ref" "$rust" 2>&1)"
    echo "        bisect: firmware/target/debug/obcm_diff '$ref' '$rust'"
    fail=1
  fi
done

[ "$fail" = "0" ] && echo "Stage-1 gate: PASS" || { echo "Stage-1 gate: FAIL"; exit 1; }
