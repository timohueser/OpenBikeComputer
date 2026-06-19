#!/usr/bin/env bash
# SUPERSEDED by run_stage4_ingest.sh. Stage 4 made ingest emit multipolygon
# relations, so the Rust output now carries relation polygons this Stage-3-only
# oracle (dump_ingest.py WITHOUT --with-relations) drops — the gate will report a
# `poly_only_rust` surplus equal to the relation polygons. That is expected, not a
# regression; use run_stage4_ingest.sh for the current ingest gate. Kept as the
# Stage-3 milestone record (the lines + closed-way subset still compares exactly).
#
# Stage-3 INGEST gate (handover §6.1): isolates the new ingest port. For each
# corpus item, dump the Python oracle's Stage-3 *expected* feature set
# (dump_ingest.py — relations and closed-line-way blobs removed) and the Rust
# ingest (ingest_dump), then compare as a multiset of (style_id, kind, microdeg
# vertices). Lines compare exactly; closed-way polygons compare up to ring
# rotation + winding (compare_ingest.py).
#
# This is the pre-quadtree gate; run_stage3.sh covers the end-to-end .obcm.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../../.." && pwd)"
PY="${PY:-$REPO/.venv/bin/python}"
CONFIG="$REPO/packer/config.json"
DATA="$REPO/packer/tests/corpus/data"
WORK="${WORK:-/tmp/obcpack-stage3-ingest}"
mkdir -p "$WORK"

ITEMS="${ITEMS:-tiny monaco malta freiburg-forest freiburg-town}"

echo ">> building obc-pack (ingest_dump)"
cargo build --manifest-path "$REPO/firmware/Cargo.toml" -p obc-pack --bin ingest_dump -q
DUMP="$REPO/firmware/target/debug/ingest_dump"

fail=0
for item in $ITEMS; do
  pbf="$DATA/$item.osm.pbf"
  if [ ! -f "$pbf" ]; then echo "SKIP $item (missing $pbf)"; continue; fi
  oracle="$WORK/$item.oracle.json"; rust="$WORK/$item.rust.json"
  "$PY" "$HERE/dump_ingest.py" "$pbf" "$CONFIG" "$oracle" >/dev/null 2>&1
  "$DUMP" "$pbf" "$CONFIG" "$rust" >/dev/null 2>&1
  if "$PY" "$HERE/compare_ingest.py" "$oracle" "$rust" >/dev/null 2>&1; then
    printf '  PASS  %-16s ingest multiset identical\n' "$item"
  else
    printf '  FAIL  %-16s see: %s "%s" "%s"\n' "$item" "$PY $HERE/compare_ingest.py" "$oracle" "$rust"
    fail=1
  fi
done

[ "$fail" = "0" ] && echo "Stage-3 ingest gate: PASS" || { echo "Stage-3 ingest gate: FAIL"; exit 1; }
