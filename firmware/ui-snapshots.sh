#!/usr/bin/env bash
# PNG snapshot sweep of every UI screen (epic #335's shared regression net):
# headless obc-sim renders, diffed before/after each cleanup phase — byte-identical
# unless a phase explicitly changes pixels.
#
# Usage: ui-snapshots.sh [OUT_DIR]
#   OUT_DIR   where the PNGs land (default: ui-snapshots/)
#
# Env overrides:
#   SIM   the obc-sim binary   (default: <repo>/firmware/target/release/obc-sim)
#   MAP   the .obcm map        (default: /Users/timo/Documents/OSM/freiburg.obcm)
#   GPX   the replay track     (default: /Users/timo/Documents/OSM/kandel.gpx)
#
# Routes come from the repo's protocol-vectors/ fixtures. Exits non-zero on the
# first failing render (set -e), so a broken sim can't produce a silently short sweep.
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
SIM="${SIM:-$repo_root/firmware/target/release/obc-sim}"
MAP="${MAP:-/Users/timo/Documents/OSM/freiburg.obcm}"
GPX="${GPX:-/Users/timo/Documents/OSM/kandel.gpx}"
ROUTES="$repo_root/protocol-vectors"
OUT="${1:-ui-snapshots}"

mkdir -p "$OUT"

"$SIM" "$MAP" --boot --png "$OUT/home.png" --battery 45
"$SIM" "$MAP" --boot --script "p"            --routes-dir "$ROUTES" --png "$OUT/routemenu.png"
"$SIM" "$MAP" --boot --script "B"            --png "$OUT/menu.png"
"$SIM" "$MAP" --boot --script "B r p"        --png "$OUT/settings.png"
"$SIM" "$MAP" --boot --script "B r p p"      --png "$OUT/datetime.png"
"$SIM" "$MAP" --boot --script "B r p r p"    --png "$OUT/units.png"
"$SIM" "$MAP" --boot --script "B r p r r p"  --png "$OUT/stats-settings.png"
"$SIM" "$MAP" --boot --script "B r p r r p r p" --png "$OUT/fields.png"
"$SIM" "$MAP" --boot --script "B r p r r r p"   --png "$OUT/power.png"
"$SIM" "$MAP" --boot --script "B r p r r r r p p H" --png "$OUT/reset-hold.png"
"$SIM" "$MAP" --boot --routes-dir "$ROUTES" --script "p p"   --gpx "$GPX" --at 30 --png "$OUT/map.png"
"$SIM" "$MAP" --boot --routes-dir "$ROUTES" --script "p p b" --gpx "$GPX" --at 30 --png "$OUT/statistics.png"
"$SIM" "$MAP" --boot --routes-dir "$ROUTES" --script "p p p" --gpx "$GPX" --at 30 --png "$OUT/ridecontrol.png"
"$SIM" "$MAP" --boot --routes-dir "$ROUTES" --script "p p B p r p" --png "$OUT/routeswap.png"
"$SIM" "$MAP" --boot --routes-dir "$ROUTES" --script "p p h" --png "$OUT/map-pan.png"

echo "ui-snapshots: 15 screens rendered into $OUT/"
