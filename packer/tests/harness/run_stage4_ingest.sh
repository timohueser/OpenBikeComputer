#!/usr/bin/env bash
# Stage-4 INGEST gate (handover §6.1): like run_stage3_ingest.sh but the oracle
# dump KEEPS relation-assembled polygons (--with-relations), so this validates the
# new multipolygon/boundary relation assembly. For each corpus item, dump the
# Python oracle's expected set (dump_ingest.py --with-relations) and the Rust
# ingest (ingest_dump, which now assembles relations via GEOS build_area), then
# compare as a multiset of (style_id, kind, microdeg vertices). Lines compare
# exactly; polygons (closed-way AND relation) compare up to ring rotation +
# winding (compare_ingest.py) — osmium and GEOS won't agree on ring start/winding.
#
# Corpus order (handover §6.4): tiny (R1 hole + R2 two-outer, must be exact),
# then the MP stressors freiburg-forest + malta, then the rest.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../../.." && pwd)"
PY="${PY:-$REPO/.venv/bin/python}"
CONFIG="$REPO/packer/config.json"
DATA="$REPO/packer/tests/corpus/data"
WORK="${WORK:-/tmp/obcpack-stage4-ingest}"
mkdir -p "$WORK"

ITEMS="${ITEMS:-tiny freiburg-forest malta monaco freiburg-town}"

echo ">> building obc-pack (ingest_dump)"
cargo build --manifest-path "$REPO/firmware/Cargo.toml" -p obc-pack --bin ingest_dump -q
DUMP="$REPO/firmware/target/debug/ingest_dump"

# A few broken relations (self-touching / crossing members) get a different vertex
# set from osmium vs GEOS build_area/node — a balanced polygon residual (each
# divergent oracle polygon has a rust counterpart), render-verified equivalent by
# run_stage4.sh. Accept that here as long as lines stay exact; flag anything else.
fail=0
for item in $ITEMS; do
  pbf="$DATA/$item.osm.pbf"
  if [ ! -f "$pbf" ]; then echo "SKIP $item (missing $pbf)"; continue; fi
  oracle="$WORK/$item.oracle.json"; rust="$WORK/$item.rust.json"
  "$PY" "$HERE/dump_ingest.py" "$pbf" "$CONFIG" "$oracle" --with-relations >/dev/null 2>&1
  "$DUMP" "$pbf" "$CONFIG" "$rust" >/dev/null 2>&1

  out=$("$PY" "$HERE/compare_ingest.py" "$oracle" "$rust" 2>/dev/null || true)
  summary=$(echo "$out" | grep '^SUMMARY' || true)
  lo=$(echo "$summary" | sed -nE 's/.*line_only_oracle=([0-9]+).*/\1/p')
  lr=$(echo "$summary" | sed -nE 's/.*line_only_rust=([0-9]+).*/\1/p')
  po=$(echo "$summary" | sed -nE 's/.*poly_only_oracle=([0-9]+).*/\1/p')
  pr=$(echo "$summary" | sed -nE 's/.*poly_only_rust=([0-9]+).*/\1/p')
  cd=$(echo "$summary" | sed -nE 's/.*coast_diff=([0-9]+).*/\1/p')

  if [ "${lo:-1}" = "0" ] && [ "${lr:-1}" = "0" ] && [ "${po:-1}" = "0" ] && [ "${pr:-1}" = "0" ] && [ "${cd:-1}" = "0" ]; then
    printf '  PASS  %-16s ingest multiset identical (incl. relation polygons)\n' "$item"
  elif [ "${lo:-1}" = "0" ] && [ "${lr:-1}" = "0" ] && [ "${cd:-1}" = "0" ] && [ "${po:-X}" = "${pr:-Y}" ] && [ "${po:-999}" -le "${MAX_RETESS:-20}" ]; then
    printf '  PASS  %-16s lines+coastlines exact; %s relation polygon(s) re-tessellated (balanced; render-verified by run_stage4.sh)\n' "$item" "$po"
  else
    printf '  FAIL  %-16s line_only=[o=%s r=%s] poly_only=[o=%s r=%s] coast=%s; repro: %s "%s" "%s"\n' \
      "$item" "${lo:-?}" "${lr:-?}" "${po:-?}" "${pr:-?}" "${cd:-?}" "$PY $HERE/compare_ingest.py" "$oracle" "$rust"
    fail=1
  fi
done

[ "$fail" = "0" ] && echo "Stage-4 ingest gate: PASS" || { echo "Stage-4 ingest gate: FAIL"; exit 1; }
