#!/usr/bin/env bash
# SUPERSEDED by run_stage4.sh. Stage 4 made ingest emit multipolygon relations, so
# the Rust .obcm now contains relation polygons this Stage-3-only reference
# (dump_stage3_ref.py WITHOUT --with-relations) omits — expect a polygon surplus,
# not a regression. Use run_stage4.sh for the current end-to-end + render gate.
# Kept as the Stage-3 milestone record.
#
# Stage-3 END-TO-END gate (handover §6.2): build each corpus item's .obcm two
# ways — the Rust `obc-pack` (full ingest→quadtree→serialize) and a Python
# reference *restricted to the same Stage-3 set* (dump_stage3_ref.py: relations
# and closed-line-way blobs removed, bbox over that set) — then compare with
# `obcm_diff --canonical-polys`.
#
# Per Amendment 1 the gate is NOT byte-identity. Expected outcome (handover §6.2):
#   - structural identical (bbox, styles, per-LOD node/chunk counts),
#   - LINES byte-exact (the deterministic path; any line diff is a real bug),
#   - POLYGONS identical up to ring rotation/winding (osmium normalizes closed-way
#     ring start; --canonical-polys reconciles it), EXCEPT a small residual at
#     simplify>0 LODs from the GEOS 3.14 (rust) vs 3.13 (shapely) simplify skew.
# So PASS = structural_ok AND zero line diffs; the polygon residual is reported.
#
# Stage-3 omits relations (Stage 4) and land/merge (Stage 5), so --no-land.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../../.." && pwd)"
PY="${PY:-$REPO/.venv/bin/python}"
CONFIG="$REPO/packer/config.json"
DATA="$REPO/packer/tests/corpus/data"
WORK="${WORK:-/tmp/obcpack-stage3}"
mkdir -p "$WORK"

ITEMS="${ITEMS:-tiny monaco malta freiburg-forest freiburg-town}"

echo ">> building obc-pack"
cargo build --manifest-path "$REPO/firmware/Cargo.toml" -p obc-pack -q
PACK="$REPO/firmware/target/debug/obc-pack"
DIFF="$REPO/firmware/target/debug/obcm_diff"

# LOD indices with simplify==0: these run NO simplify, so they must match exactly
# (a polygon diff there is a real bug, not GEOS-version skew). Config-driven.
NOSIMP_LODS=$("$PY" -c "import json,sys; c=json.load(open('$CONFIG')); print(' '.join(str(i) for i,l in enumerate(c.get('lods') or [{}]) if not (l.get('simplify') or 0)))")

fail=0
for item in $ITEMS; do
  pbf="$DATA/$item.osm.pbf"
  if [ ! -f "$pbf" ]; then echo "SKIP $item (missing $pbf)"; continue; fi
  ref="$WORK/$item.ref.obcm"; rust="$WORK/$item.rust.obcm"
  "$PY" "$HERE/dump_stage3_ref.py" "$pbf" "$CONFIG" "$ref" >/dev/null 2>&1
  "$PACK" "$pbf" "$CONFIG" "$rust" --no-land >/dev/null 2>&1
  sz=$(wc -c < "$ref" | tr -d ' ')

  if cmp -s "$ref" "$rust"; then
    printf '  PASS  %-16s byte-identical (%s bytes)\n' "$item" "$sz"
    continue
  fi

  summary=$("$DIFF" "$ref" "$rust" --canonical-polys 2>/dev/null | grep '^SUMMARY' || true)
  so=$(echo "$summary" | sed -nE 's/.*structural_ok=([0-9]+).*/\1/p')
  ld=$(echo "$summary" | sed -nE 's/.*line_diffs=([0-9]+).*/\1/p')
  pd=$(echo "$summary" | sed -nE 's/.*poly_diffs=([0-9]+).*/\1/p')
  lpd=$(echo "$summary" | sed -nE 's/.*lodpolys=([0-9,]+).*/\1/p')

  # No-simplify LODs must have zero polygon diffs (hard guard).
  nosimp_bad=""
  IFS=',' read -ra arr <<< "$lpd"
  for i in $NOSIMP_LODS; do
    if [ "${arr[$i]:-0}" != "0" ]; then nosimp_bad="LOD$i=${arr[$i]} $nosimp_bad"; fi
  done

  if [ "$so" != "1" ] || [ "${ld:-1}" != "0" ] || [ -n "$nosimp_bad" ]; then
    printf '  FAIL  %-16s structural_ok=%s line_diffs=%s poly_diffs=%s nosimp_diffs=[%s]\n' \
      "$item" "${so:-?}" "${ld:-?}" "${pd:-?}" "${nosimp_bad:-none}"
    printf '        repro: %s "%s" "%s" --canonical-polys\n' "$DIFF" "$ref" "$rust"
    fail=1
  elif [ "${pd:-0}" = "0" ]; then
    printf '  PASS  %-16s multiset-identical (lines exact; polygons up to ring winding)\n' "$item"
  else
    printf '  PASS  %-16s lines exact, no-simplify LODs exact; %s polygons differ at simplify LODs (GEOS 3.14 vs 3.13 skew)\n' "$item" "$pd"
  fi
done

[ "$fail" = "0" ] && echo "Stage-3 end-to-end gate: PASS" || { echo "Stage-3 end-to-end gate: FAIL"; exit 1; }
