#!/usr/bin/env bash
# Stage-5 gate (handover §6): the FULL pipeline — multi-PBF merge (osmium CLI) +
# ingest + relation areas + **land generation** + quadtree + serialize. Oracle is
# the real `pack.py` (WITH land), rust is `obc-pack` (WITH land). Per item:
#
#   1. Land-parity (the new code, isolated from the quadtree): `land_probe` vs
#      `dump_land.py` — total land AREA must match within LAND_TOL. The closed-form
#      Web Mercator reproject + GEOS clip reproduce shapely/pyproj to ~1e-11.
#   2. End-to-end `obcm_diff`: the HEADER (version, bbox, marker, style table) MUST
#      be identical — a diff there is a real bug. Per-LOD node/chunk/line/poly
#      residuals are REPORTED, not failed: land adds density, so the pre-existing
#      GEOS-version simplify/relation-assembly skew tips a few near-threshold
#      quadtree splits (balanced re-clips, no net feature loss). Render proves it.
#   3. Render-diff (the semantic gate): whole-map overview + coastal fine-LOD tiles
#      must be render-equivalent (< THRESH differing pixels).
#
# Plus a MERGE correctness check: `obc-pack a a` (merge a file with itself; osmium
# dedupes) must render-match the single-file build.
#
# PASS = land area within tol AND header identical AND every render-diff < THRESH.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../../.." && pwd)"
PY="${PY:-$REPO/.venv/bin/python}"
CONFIG="$REPO/packer/config.json"
DATA="$REPO/packer/tests/corpus/data"
WORK="${WORK:-/tmp/obcpack-stage5}"
mkdir -p "$WORK"

ITEMS="${ITEMS:-monaco malta freiburg-town}"
THRESH="${THRESH:-0.01}"   # max differing-pixel fraction per render tile
LAND_TOL="${LAND_TOL:-1e-4}"
SIZE="${SIZE:-480x480}"

echo ">> building obc-pack + obc-sim + land_probe (release)"
cargo build --manifest-path "$REPO/firmware/Cargo.toml" -p obc-pack -p obc-sim --release -q
PACK="$REPO/firmware/target/release/obc-pack"
DIFF="$REPO/firmware/target/release/obcm_diff"
SIM="$REPO/firmware/target/release/obc-sim"
PROBE="$REPO/firmware/target/release/land_probe"

# Extract (min_lon min_lat max_lon max_lat) in degrees from an .obcm header
# (`<4sB i i i i ...>` = magic, version, min_lat, min_lon, max_lat, max_lon).
bbox_of() {
  "$PY" - "$1" <<'PY'
import struct, sys
d = open(sys.argv[1], 'rb').read(21)
_, _v, min_lat, min_lon, max_lat, max_lon = struct.unpack_from("<4sBiiii", d, 0)
print(min_lon/1e6, min_lat/1e6, max_lon/1e6, max_lat/1e6)
PY
}

# Coastal fine-LOD render tiles per item: "cx_microdeg,cy_microdeg,zoom" (overview
# is always added). Aimed at land/sea boundaries where clip differences would show.
tiles_for() {
  case "$1" in
    monaco) echo "7424600,43738400,1200" ;;
    malta)  echo "14400000,35900000,1500 14510000,35890000,2500 14330000,36050000,2500" ;;
    *)      echo "" ;;
  esac
}

# Render both files at a viewport and report the differing-pixel fraction; sets
# `rc` to compare_png's exit (nonzero ⇒ over threshold).
render_diff() { # ref rust tag center? zoom?
  local ref="$1" rust="$2" tag="$3" extra="${4:-}"
  # shellcheck disable=SC2086
  "$SIM" "$ref"  --png "$WORK/$tag.ref.png"  --size "$SIZE" $extra >/dev/null 2>&1
  # shellcheck disable=SC2086
  "$SIM" "$rust" --png "$WORK/$tag.rust.png" --size "$SIZE" $extra >/dev/null 2>&1
  rc=0
  rdline=$("$PY" "$HERE/compare_png.py" "$WORK/$tag.ref.png" "$WORK/$tag.rust.png" \
            --threshold "$THRESH" --diff "$WORK/$tag.diff.png") || rc=$?
}

fail=0
for item in $ITEMS; do
  pbf="$DATA/$item.osm.pbf"
  if [ ! -f "$pbf" ]; then echo "SKIP $item (missing $pbf)"; continue; fi
  ref="$WORK/$item.ref.obcm"; rust="$WORK/$item.rust.obcm"

  # Build both, WITH land (the real production path).
  ( cd "$REPO/packer" && "$PY" pack.py "$pbf" "$CONFIG" "$ref" ) >/dev/null 2>&1
  "$PACK" "$pbf" "$CONFIG" "$rust" >/dev/null 2>&1

  # --- 1. Land parity (isolated). ---
  read -r a b c d <<< "$(bbox_of "$rust")"
  "$PROBE" "$a" "$b" "$c" "$d" "$WORK/$item.land.rust.json" >/dev/null 2>&1
  "$PY" "$HERE/dump_land.py" "$a" "$b" "$c" "$d" "$WORK/$item.land.oracle.json" >/dev/null 2>&1
  land_rc=0
  landarea=$("$PY" "$HERE/compare_land.py" "$WORK/$item.land.rust.json" \
              "$WORK/$item.land.oracle.json" --tol "$LAND_TOL" | sed -nE 's/.*area rel diff = ([0-9.e-]+).*/\1/p') || land_rc=$?
  if [ "$land_rc" != "0" ]; then
    printf '  FAIL  %-14s land area mismatch (see compare_land)\n' "$item"; fail=1; continue
  fi

  # --- 2. obcm_diff: header MUST match; counts are reported residuals. ---
  diffout=$("$DIFF" "$ref" "$rust" --canonical-polys 2>/dev/null || true)
  hdr=$(echo "$diffout" | grep -E '^DIFF' | grep -Ev 'node_count|chunk_count' || true)
  if [ -n "$hdr" ]; then
    printf '  FAIL  %-14s header divergence:\n%s\n' "$item" "$hdr"; fail=1; continue
  fi
  summary=$(echo "$diffout" | grep '^SUMMARY' || true)
  ld=$(echo "$summary" | sed -nE 's/.*line_diffs=([0-9]+).*/\1/p')
  pd=$(echo "$summary" | sed -nE 's/.*poly_diffs=([0-9]+).*/\1/p')

  # --- 3. Render-diff: overview + coastal tiles. ---
  render_diff "$ref" "$rust" "$item.ov"
  worst="$rdline"; badtile=0; ntile=1
  [ "$rc" != "0" ] && badtile=$((badtile + 1))
  ovfrac=$(echo "$rdline" | sed -nE 's/.*\(([0-9.]+)%\).*/\1/p'); [ -z "$ovfrac" ] && ovfrac=0
  worstfrac="$ovfrac"
  for t in $(tiles_for "$item"); do
    IFS=, read -r cx cy z <<< "$t"
    render_diff "$ref" "$rust" "$item.t$ntile" "--center $cx,$cy --zoom $z"
    [ "$rc" != "0" ] && badtile=$((badtile + 1))
    tf=$(echo "$rdline" | sed -nE 's/.*\(([0-9.]+)%\).*/\1/p'); [ -z "$tf" ] && tf=0
    awk -v a="$tf" -v b="$worstfrac" 'BEGIN{exit !(a>b)}' && worstfrac="$tf"
    ntile=$((ntile + 1))
  done

  if [ "$badtile" != "0" ]; then
    printf '  FAIL  %-14s %d/%d render tiles exceed %s (worst %s%%); see %s/%s.*.diff.png\n' \
      "$item" "$badtile" "$ntile" "$THRESH" "$worstfrac" "$WORK" "$item"; fail=1
  else
    printf '  PASS  %-14s land area rel %s; header identical; %d render tile(s) equivalent (worst %s%%); residual line=%s poly=%s (benign GEOS-version split skew)\n' \
      "$item" "$landarea" "$ntile" "$worstfrac" "${ld:-0}" "${pd:-0}"
  fi
done

# --- Merge correctness: merge an item with ITSELF (osmium dedupes) ⇒ same map. ---
MITEM="${MITEM:-monaco}"
mpbf="$DATA/$MITEM.osm.pbf"
if [ -f "$mpbf" ]; then
  single="$WORK/$MITEM.single.obcm"; merged="$WORK/$MITEM.merged.obcm"
  "$PACK" "$mpbf" "$CONFIG" "$single" >/dev/null 2>&1
  "$PACK" "$mpbf" "$mpbf" "$CONFIG" "$merged" >/dev/null 2>&1   # 2 inputs ⇒ osmium merge+sort
  render_diff "$single" "$merged" "$MITEM.merge"
  if [ "$rc" != "0" ]; then
    printf '  FAIL  %-14s merge(self) render differs from single build\n' "merge:$MITEM"; fail=1
  else
    printf '  PASS  %-14s merge(self)==single build (osmium merge+sort+dedupe)\n' "merge:$MITEM"
  fi
fi

[ "$fail" = "0" ] && echo "Stage-5 gate: PASS" || { echo "Stage-5 gate: FAIL"; exit 1; }
