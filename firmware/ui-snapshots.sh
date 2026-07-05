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
#   MAP   the .obcm map        (default: the committed Grimsel showcase fixture, OBCM v7)
#   GPX   the replay track     (default: the committed Grimsel climb fixture)
#
# The defaults are the OBCM **v7** fixtures baked into obc-sim (the Grimsel showcase
# map + its climb replay), so the sweep runs out-of-the-box; point MAP/GPX at a local
# map to sweep a different region. Routes come from the repo's protocol-vectors/
# fixtures. Exits non-zero on the first failing render (set -e), so a broken sim can't
# produce a silently short sweep.
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
SIM="${SIM:-$repo_root/firmware/target/release/obc-sim}"
MAP="${MAP:-$repo_root/firmware/obc-sim/assets/grimsel.obcm}"
GPX="${GPX:-$repo_root/firmware/obc-sim/assets/grimsel-climb.gpx}"
ROUTES="$repo_root/protocol-vectors"
OUT="${1:-ui-snapshots}"

mkdir -p "$OUT"

# Menu navigation: the compass menu is Routes / POIs / Map / Settings, so Settings is one
# ccw detent (`l`, wrapping) from the Routes start. `w` settles the needle sweep after a turn.
"$SIM" "$MAP" --boot --png "$OUT/home.png" --battery 45
"$SIM" "$MAP" --boot --script "p"            --routes-dir "$ROUTES" --png "$OUT/routemenu.png"
"$SIM" "$MAP" --boot --script "B"            --png "$OUT/menu.png"
"$SIM" "$MAP" --boot --script "B r w"        --png "$OUT/menu-pois.png"
# POIs browser (#425): the category list, then a populated nearest-16 list. The list's bearing
# arrows are live, so pin a deterministic fix (grimsel map centre) + heading so they reproduce.
"$SIM" "$MAP" --boot --script "B r w p"      --png "$OUT/poi-menu.png"
"$SIM" "$MAP" --boot --center 8305000,46601000 --heading 0 --script "B r w p p" --png "$OUT/poi-list.png"
"$SIM" "$MAP" --boot --script "B l p"        --png "$OUT/settings.png"
"$SIM" "$MAP" --boot --script "B l p p"      --png "$OUT/datetime.png"
"$SIM" "$MAP" --boot --script "B l p r p"    --png "$OUT/units.png"
"$SIM" "$MAP" --boot --script "B l p r r p"  --png "$OUT/stats-settings.png"
"$SIM" "$MAP" --boot --script "B l p r r p r p" --png "$OUT/fields.png"
"$SIM" "$MAP" --boot --script "B l p r r r p"   --png "$OUT/power.png"
"$SIM" "$MAP" --boot --script "B l p r r r r p p H" --png "$OUT/reset-hold.png"
# Riding flows go through the Route overview now: pick (p) → overview → START (p) → Map.
"$SIM" "$MAP" --boot --routes-dir "$ROUTES" --script "p p"     --png "$OUT/routeoverview.png"
"$SIM" "$MAP" --boot --routes-dir "$ROUTES" --script "p p p"   --gpx "$GPX" --at 30 --png "$OUT/map.png"
"$SIM" "$MAP" --boot --routes-dir "$ROUTES" --script "p p p b" --gpx "$GPX" --at 30 --png "$OUT/statistics.png"
"$SIM" "$MAP" --boot --routes-dir "$ROUTES" --script "p p p p" --gpx "$GPX" --at 30 --png "$OUT/ridecontrol.png"
"$SIM" "$MAP" --boot --routes-dir "$ROUTES" --script "p p p B p r p" --png "$OUT/routeswap.png"
"$SIM" "$MAP" --boot --routes-dir "$ROUTES" --script "p p p h" --png "$OUT/map-pan.png"

echo "ui-snapshots: 19 screens rendered into $OUT/"
