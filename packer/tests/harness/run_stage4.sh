#!/usr/bin/env bash
# Stage-4 END-TO-END + RENDER gate (handover §6.2/§6.3). For each corpus item,
# build the .obcm two ways — Rust `obc-pack` (full ingest→quadtree→serialize, now
# WITH relation areas) and a Python reference restricted to the same set
# (dump_stage3_ref.py --with-relations) — then:
#
#   1. obcm_diff --canonical-polys: hard-guard structural identity + zero line
#      diffs; report the per-LOD polygon residual.
#   2. Render-diff (obc-sim --png) each relation-assembly divergence at the finest
#      (no-simplify) LOD and assert it is render-equivalent (< THRESH differing
#      pixels). These are the few broken relations osmium's AreaManager and GEOS
#      build_area/node repair into different vertex sets — the multiset can't
#      reconcile them, but the filled areas are identical, so the renders are
#      (sub-pixel boundary jitter only). Plus a whole-map overview, reported.
#
# So PASS = structural_ok AND zero line diffs AND every divergence render-equivalent.
# The remaining polygon residual lives at the simplify LODs and is the SAME GEOS
# 3.14-vs-3.13 TopologyPreservingSimplifier skew Stage 3 documented (reported, not
# failed). Stage-4 omits land/merge (Stage 5), so --no-land.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../../.." && pwd)"
PY="${PY:-$REPO/.venv/bin/python}"
CONFIG="$REPO/packer/config.json"
DATA="$REPO/packer/tests/corpus/data"
WORK="${WORK:-/tmp/obcpack-stage4}"
mkdir -p "$WORK"

ITEMS="${ITEMS:-tiny freiburg-forest malta monaco freiburg-town}"
THRESH="${THRESH:-0.01}"   # max differing-pixel fraction per divergence tile
SIZE="${SIZE:-480x480}"

echo ">> building obc-pack + obc-sim (release)"
cargo build --manifest-path "$REPO/firmware/Cargo.toml" -p obc-pack -p obc-sim --release -q
PACK="$REPO/firmware/target/release/obc-pack"
DIFF="$REPO/firmware/target/release/obcm_diff"
DUMP="$REPO/firmware/target/release/ingest_dump"
SIM="$REPO/firmware/target/release/obc-sim"

# Finest LOD = smallest finite max_mpp in config; the divergence render-diff forces
# the camera there so it measures assembly, not the coarse-LOD simplify skew.
FINEST_MPP=$("$PY" -c "import json; c=json.load(open('$CONFIG')); v=[l.get('max_mpp') for l in (c.get('lods') or []) if l.get('max_mpp')]; print(min(v) if v else 18)")

fail=0
for item in $ITEMS; do
  pbf="$DATA/$item.osm.pbf"
  if [ ! -f "$pbf" ]; then echo "SKIP $item (missing $pbf)"; continue; fi
  ref="$WORK/$item.ref.obcm"; rust="$WORK/$item.rust.obcm"
  ojson="$WORK/$item.oracle.json"; rjson="$WORK/$item.rust.json"

  # Build both .obcm + both ingest dumps (the dumps drive divergence finding).
  "$PY" "$HERE/dump_stage3_ref.py" "$pbf" "$CONFIG" "$ref" --with-relations >/dev/null 2>&1
  "$PACK" "$pbf" "$CONFIG" "$rust" --no-land >/dev/null 2>&1
  "$PY" "$HERE/dump_ingest.py" "$pbf" "$CONFIG" "$ojson" --with-relations >/dev/null 2>&1
  "$DUMP" "$pbf" "$CONFIG" "$rjson" >/dev/null 2>&1

  # --- 1. Structural + multiset. ---
  summary=$("$DIFF" "$ref" "$rust" --canonical-polys 2>/dev/null | grep '^SUMMARY' || true)
  so=$(echo "$summary" | sed -nE 's/.*structural_ok=([0-9]+).*/\1/p')
  ld=$(echo "$summary" | sed -nE 's/.*line_diffs=([0-9]+).*/\1/p')
  pd=$(echo "$summary" | sed -nE 's/.*poly_diffs=([0-9]+).*/\1/p')
  lpd=$(echo "$summary" | sed -nE 's/.*lodpolys=([0-9,]+).*/\1/p')

  if [ "${so:-0}" != "1" ] || [ "${ld:-1}" != "0" ]; then
    printf '  FAIL  %-16s structural_ok=%s line_diffs=%s (repro: %s "%s" "%s" --canonical-polys)\n' \
      "$item" "${so:-?}" "${ld:-?}" "$DIFF" "$ref" "$rust"
    fail=1; continue
  fi

  # --- 2. Render-diff each assembly divergence at the finest (no-simplify) LOD. ---
  divs=$("$PY" "$HERE/find_divergences.py" "$ojson" "$rjson" --finest-mpp "$FINEST_MPP")
  ndiv=0; badtile=0; worst="0"
  while IFS=, read -r cx cy zoom; do
    [ -z "${cx:-}" ] && continue
    ndiv=$((ndiv + 1))
    "$SIM" "$ref"  --png "$WORK/$item.div$ndiv.ref.png"  --size "$SIZE" --center "$cx,$cy" --zoom "$zoom" >/dev/null 2>&1
    "$SIM" "$rust" --png "$WORK/$item.div$ndiv.rust.png" --size "$SIZE" --center "$cx,$cy" --zoom "$zoom" >/dev/null 2>&1
    rc=0
    line=$("$PY" "$HERE/compare_png.py" "$WORK/$item.div$ndiv.ref.png" "$WORK/$item.div$ndiv.rust.png" \
            --threshold "$THRESH" --diff "$WORK/$item.div$ndiv.diff.png" 2>/dev/null) || rc=$?
    frac=$(echo "$line" | sed -nE 's/.*\(([0-9.]+)%\).*/\1/p')
    [ -n "$frac" ] && awk -v a="$frac" -v b="$worst" 'BEGIN{exit !(a>b)}' && worst="$frac"
    [ "$rc" != "0" ] && badtile=$((badtile + 1))
  done <<< "$divs"

  # Whole-map overview (gross-breakage canary; informational).
  "$SIM" "$ref"  --png "$WORK/$item.ov.ref.png"  --size "$SIZE" >/dev/null 2>&1
  "$SIM" "$rust" --png "$WORK/$item.ov.rust.png" --size "$SIZE" >/dev/null 2>&1
  ovline=$("$PY" "$HERE/compare_png.py" "$WORK/$item.ov.ref.png" "$WORK/$item.ov.rust.png" --threshold 1 || true)
  ovfrac=$(echo "$ovline" | sed -nE 's/.*\(([0-9.]+)%\).*/\1/p')
  [ -z "$ovfrac" ] && ovfrac="0"

  if [ "$badtile" != "0" ]; then
    printf '  FAIL  %-16s %d/%d divergence tiles exceed %s (worst %s%%); see %s/%s.div*.diff.png\n' \
      "$item" "$badtile" "$ndiv" "$THRESH" "$worst" "$WORK" "$item"
    fail=1
  else
    printf '  PASS  %-16s structural ok, lines exact; %d assembly divergence(s) render-equivalent (worst %s%%); overview %s%%; %s simplify-LOD poly residual [%s]\n' \
      "$item" "$ndiv" "$worst" "$ovfrac" "${pd:-0}" "${lpd:-}"
  fi
done

[ "$fail" = "0" ] && echo "Stage-4 end-to-end gate: PASS" || { echo "Stage-4 end-to-end gate: FAIL"; exit 1; }
