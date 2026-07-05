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
# Route menu hold-to-delete footer (#453): `p H` opens the menu and partial-holds the encoder over
# the highlighted route, so the trash + warning-red delete bar draws mid-charge.
"$SIM" "$MAP" --boot --script "p H"          --routes-dir "$ROUTES" --png "$OUT/routemenu-delete.png"
# The footer greyed while the highlighted route is the active-ride route (#453): ride route 0
# (`p p p`), climb back to the Route menu (`B p`) with it still highlighted, then partial-hold — the
# footer shows the "In use" greyed state and no delete bar fills.
"$SIM" "$MAP" --boot --routes-dir "$ROUTES" --script "p p p B p H" --png "$OUT/routemenu-delete-active.png"
"$SIM" "$MAP" --boot --script "B"            --png "$OUT/menu.png"
"$SIM" "$MAP" --boot --script "B r w"        --png "$OUT/menu-pois.png"
# POIs browser (#425): the category list, then a populated nearest-16 list. The list's bearing
# arrows are live, so pin a deterministic fix (grimsel map centre) + heading so they reproduce.
"$SIM" "$MAP" --boot --script "B r w p"      --png "$OUT/poi-menu.png"
"$SIM" "$MAP" --boot --center 8305000,46601000 --heading 0 --script "B r w p p" --png "$OUT/poi-list.png"
# POI detail (#444): the hours + open/closed badge need the hours-rich monaco fixture (grimsel has
# no shop hours). Pin the Resupply "Carrefour" supermarket (--center on it → row 0), a fix +heading
# for the live arrow, and a deterministic --clock (Mon 2025-01-06 12:00 → OPEN). `p d p` presses into
# the list, draws once to fill the lazy snapshot, then presses the POI into its detail.
MONACO="$repo_root/firmware/obc-sim/assets/monaco.obcm"
"$SIM" "$MONACO" --boot --center 7416969,43730798 --heading 0 --clock "2025-01-06T12:00" \
    --script "B r w p r r r p d p" --png "$OUT/poi-detail.png"
"$SIM" "$MAP" --boot --script "B l p"        --png "$OUT/settings.png"
"$SIM" "$MAP" --boot --script "B l p p"      --png "$OUT/datetime.png"
"$SIM" "$MAP" --boot --script "B l p r p"    --png "$OUT/units.png"
"$SIM" "$MAP" --boot --script "B l p r r p"  --png "$OUT/stats-settings.png"
"$SIM" "$MAP" --boot --script "B l p r r p r p" --png "$OUT/fields.png"
"$SIM" "$MAP" --boot --script "B l p r r r p"   --png "$OUT/power.png"
# Bluetooth screen (#455): the main state (radio on, advertising, a stored bond -> Paired: yes) and
# the Forget-phone guarded hold mid-charge (select the Forget row, then a partial hold fills it).
"$SIM" "$MAP" --boot --ble-paired --script "B l p r r r r p"     --png "$OUT/bluetooth.png"
"$SIM" "$MAP" --boot --ble-paired --script "B l p r r r r p r H" --png "$OUT/bluetooth-forget-hold.png"
"$SIM" "$MAP" --boot --script "B l p r r r r r p p H" --png "$OUT/reset-hold.png"
# Riding flows go through the Route overview now: pick (p) → overview → START (p) → Map.
"$SIM" "$MAP" --boot --routes-dir "$ROUTES" --script "p p"     --png "$OUT/routeoverview.png"
"$SIM" "$MAP" --boot --routes-dir "$ROUTES" --script "p p p"   --gpx "$GPX" --at 30 --png "$OUT/map.png"
"$SIM" "$MAP" --boot --routes-dir "$ROUTES" --script "p p p b" --gpx "$GPX" --at 30 --png "$OUT/statistics.png"
"$SIM" "$MAP" --boot --routes-dir "$ROUTES" --script "p p p p" --gpx "$GPX" --at 30 --png "$OUT/ridecontrol.png"
"$SIM" "$MAP" --boot --routes-dir "$ROUTES" --script "p p p B p r p" --png "$OUT/routeswap.png"
"$SIM" "$MAP" --boot --routes-dir "$ROUTES" --script "p p p h" --png "$OUT/map-pan.png"
# BLE connected indicator (#448): the static Bluetooth rune on the Home battery row and the menu
# title bar. `--ble-connected` injects a linked phone, exactly as the sim control-panel toggle does.
"$SIM" "$MAP" --boot --ble-connected --png "$OUT/home-ble.png" --battery 45
"$SIM" "$MAP" --boot --ble-connected --script "B" --png "$OUT/menu-ble.png"
# BLE passkey card (#449): the host-pushed 6-digit LESC pairing code, rendered huge. `--ble-passkey N`
# injects the passkey exactly as the sim control-panel "Pairing" toggle does; the card auto-opens.
"$SIM" "$MAP" --boot --ble-passkey 42 --png "$OUT/passkey-card.png"
# Route-upload popups (#451), all three variants. `--inject-upload[-replace] ID` raises the upload
# event after the script, exactly as the control panel's inject buttons do. protocol-vectors holds
# two routes: id 0 = route-plain, id 1 = route-waypoints (filename order).
# Idle: "ROUTE RECEIVED" — Start navigation / Dismiss.
"$SIM" "$MAP" --boot --routes-dir "$ROUTES" --inject-upload 0 --png "$OUT/route-received.png"
# Tracking (riding id 0, id 1 arrives): the retitled Route-swap popup.
"$SIM" "$MAP" --boot --routes-dir "$ROUTES" --script "p p p" --inject-upload 1 --png "$OUT/routeswap-received.png"
# Active route replaced (riding id 0, id 0 re-uploaded): the info-only "ROUTE UPDATED" card.
"$SIM" "$MAP" --boot --routes-dir "$ROUTES" --script "p p p" --inject-upload-replace 0 --png "$OUT/route-updated.png"

echo "ui-snapshots: 30 screens rendered into $OUT/"
