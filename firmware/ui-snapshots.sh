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

# A deterministic /tracks fixture for the Rides screen (#454): two stored ride objects. The pinned
# `ride-v1.bin` protocol vector *is* a valid `RD{id}.ORD` (the stored file == the wire object), so we
# copy it under two ids. No `SYNCED.SET` → both read as unsynced, which is what the warning-red delete
# footer snapshot needs. Staged in a temp dir cleaned on exit.
TRACKS="$(mktemp -d)"
# A scratch routes dir for the create-route sweep below — the router writes its reserved
# `_nav.obcr` there instead of littering a `routes/` in the working directory.
NAVDIR="$(mktemp -d)"
trap 'rm -rf "$TRACKS" "$NAVDIR"' EXIT
cp "$ROUTES/ride-v1.bin" "$TRACKS/RD0.ORD"
cp "$ROUTES/ride-v1.bin" "$TRACKS/RD1.ORD"

# Menu navigation: Home's press (and back-hold) opens the compass Menu — the single door into the
# app — so the Route menu is now `p p` from boot (open Menu, then press the Routes station, which the
# menu starts on). The compass menu is Routes / Rides / POIs / Map / Settings, so Settings is one ccw
# detent (`l`, wrapping) from the Routes start, Rides is one cw (`r`), POIs two cw (`r r`). `w`
# settles the needle sweep after a turn.
"$SIM" "$MAP" --boot --png "$OUT/home.png" --battery 45
"$SIM" "$MAP" --boot --script "p p"          --routes-dir "$ROUTES" --png "$OUT/routemenu.png"
# Route menu hold-to-delete footer (#453): `p p H` opens the Menu, presses into the Route menu, and
# partial-holds the encoder over the highlighted route, so the trash + warning-red delete bar draws.
"$SIM" "$MAP" --boot --script "p p H"        --routes-dir "$ROUTES" --png "$OUT/routemenu-delete.png"
# The footer greyed while the highlighted route is the active-ride route (#453): ride route 0
# (`p p p p`), climb back to the Route menu (`B p`: Map back-hold → Menu, then press Routes) with it
# still highlighted, then partial-hold — the footer shows the "In use" greyed state and no bar fills.
"$SIM" "$MAP" --boot --routes-dir "$ROUTES" --script "p p p p B p H" --png "$OUT/routemenu-delete-active.png"
"$SIM" "$MAP" --boot --script "B"            --png "$OUT/menu.png"
# Rides screen (#454): the two-line list, then the two delete-footer states. The tracks fixture holds
# two unsynced rides. `p` presses into the Rides screen from the Menu (one `r` detent + `w` settle).
"$SIM" "$MAP" --boot --tracks-dir "$TRACKS" --script "B r w p"     --png "$OUT/rides.png"
# The warning-red "not synced" delete footer: `p H` opens the Rides screen and partial-holds the
# encoder over the highlighted (unsynced) ride, so the trash + red bar + "not synced" cue draw.
"$SIM" "$MAP" --boot --tracks-dir "$TRACKS" --script "B r w p H"   --png "$OUT/rides-delete-unsynced.png"
# The footer greyed while a ride is being recorded (#454): ride route 0 (`p p p p` → Map, riding)
# **with the GPX replay driving fixes** — the tracking session only starts once positions flow, and
# `is_tracking` (the greying predicate) is `session.is_some()`, so without `--gpx` this frame would
# wrongly show the live red footer. Then BackHold to the Menu (`B`), turn to the Rides station
# (`r w`), press in, and partial-hold — the footer shows the "Recording" greyed state, no bar fills.
"$SIM" "$MAP" --boot --routes-dir "$ROUTES" --tracks-dir "$TRACKS" --gpx "$GPX" --at 30 --script "p p p p B r w p H" --png "$OUT/rides-delete-recording.png"
"$SIM" "$MAP" --boot --script "B r r w"      --png "$OUT/menu-pois.png"
# POIs browser (#425): the category list, then a populated nearest-16 list. The list's bearing
# arrows are live, so pin a deterministic fix (grimsel map centre) + heading so they reproduce.
"$SIM" "$MAP" --boot --script "B r r w p"    --png "$OUT/poi-menu.png"
"$SIM" "$MAP" --boot --center 8305000,46601000 --heading 0 --script "B r r w p p" --png "$OUT/poi-list.png"
# POI detail (#444): the hours + open/closed badge need the hours-rich monaco fixture (grimsel has
# no shop hours). Pin the Resupply "Carrefour" supermarket (--center on it → row 0), a fix +heading
# for the live arrow, and a deterministic --clock (Mon 2025-01-06 12:00 → OPEN). `p d p` presses into
# the list, draws once to fill the lazy snapshot, then presses the POI into its detail.
MONACO="$repo_root/firmware/obc-sim/assets/monaco.obcm"
"$SIM" "$MONACO" --boot --center 7416969,43730798 --heading 0 --clock "2025-01-06T12:00" \
    --script "B r r w p r r r p d p" --png "$OUT/poi-detail.png"
# POI create-route flow (epic #116, R4). The `d` token also drains a pending create-route request
# (running the real A* router over the map's v8 nav graph), so one script walks the whole flow.
# The confirm: detail of a resupply POI ~600 m away → press.
"$SIM" "$MONACO" --boot --routes-dir "$NAVDIR" --center 7420000,43735000 --heading 0 --clock "2025-01-06T12:00" \
    --script "B r r w p r r r p d p p" --png "$OUT/nav-confirm.png"
# The computed-route overview (length only — no elevation band, no climb/descent rows): confirm →
# Create route → `d` runs the router; the answer swaps in the overview.
"$SIM" "$MONACO" --boot --routes-dir "$NAVDIR" --center 7420000,43735000 --heading 0 --clock "2025-01-06T12:00" \
    --script "B r r w p r r r p d p p p d" --png "$OUT/nav-overview.png"
# The planning screen (#499): accepting the confirm swaps to the spinning-needle wait while the
# host steps the resumable planner. `--nav-hold` leaves the recorded request un-drained so the
# screen stays up for the snapshot (needle at its deterministic initial angle).
"$SIM" "$MONACO" --boot --routes-dir "$NAVDIR" --center 7420000,43735000 --heading 0 --clock "2025-01-06T12:00" \
    --script "B r r w p r r r p d p p p" --nav-hold --png "$OUT/nav-planning.png"
# The two locked failure tiers. The range tier ("Too far to route here") = the router's fixed
# table exhausting — with no distance cap that IS the device's range limit — which the small
# fixture graphs can't reach (grimsel plans even ~25 km routes inside the 1536-node table), so
# the card is injected through the real notify_nav_result seam with the planning screen on top,
# pinning the exhausted→range-tier mapping. The generic tier ("Couldn't find a route.") stays a
# real plan: a mountain fix with no routable node within the 250 m snap radius.
"$SIM" "$MONACO" --boot --routes-dir "$NAVDIR" --center 7420000,43735000 --heading 0 --clock "2025-01-06T12:00" \
    --script "B r r w p r r r p d p p p" --inject-nav-fail exhausted --png "$OUT/nav-toofar.png"
"$SIM" "$MAP" --boot --routes-dir "$NAVDIR" --center 8140000,46480000 --heading 0 \
    --script "B r r w p p d p p p d" --png "$OUT/nav-nopath.png"
# The Settings list (Date & Time, Units, Bike type, Stats, Display, Power, Bluetooth, Reset — Bike
# type inserted at index 2 by routing-v2 N5 #538, so every row past Units shifts one turn further in).
"$SIM" "$MAP" --boot --script "B l p"        --png "$OUT/settings.png"
"$SIM" "$MAP" --boot --script "B l p p"      --png "$OUT/datetime.png"
"$SIM" "$MAP" --boot --script "B l p r p"    --png "$OUT/units.png"
# Bike type (routing-v2 N5): the map's §8.6 profile names — grimsel ships Road/Gravel/MTB/Touring.
# The default selection (Road), then one detent cycling it to Gravel — pinning the name list + cycle.
"$SIM" "$MAP" --boot --script "B l p r r p"   --png "$OUT/biketype.png"
"$SIM" "$MAP" --boot --script "B l p r r p r" --png "$OUT/biketype-cycled.png"
"$SIM" "$MAP" --boot --script "B l p r r r p"  --png "$OUT/stats-settings.png"
"$SIM" "$MAP" --boot --script "B l p r r r p r p" --png "$OUT/fields.png"
# The Display page (row 4): the two Map-overlay toggles + the idle-return picker moved from Power.
"$SIM" "$MAP" --boot --script "B l p r r r r p"   --png "$OUT/display.png"
"$SIM" "$MAP" --boot --script "B l p r r r r r p" --png "$OUT/power.png"
# Bluetooth screen (#455): the main state (radio on, advertising, a stored bond -> Paired: yes) and
# the Forget-phone guarded hold mid-charge (select the Forget row, then a partial hold fills it).
"$SIM" "$MAP" --boot --ble-paired --script "B l p r r r r r r p"     --png "$OUT/bluetooth.png"
"$SIM" "$MAP" --boot --ble-paired --script "B l p r r r r r r p r H" --png "$OUT/bluetooth-forget-hold.png"
"$SIM" "$MAP" --boot --script "B l p r r r r r r r p p H" --png "$OUT/reset-hold.png"
# Riding flows: Home press → Menu → Routes (p) → Route menu → pick (p) → overview → START (p) → Map.
"$SIM" "$MAP" --boot --routes-dir "$ROUTES" --script "p p p"     --png "$OUT/routeoverview.png"
# The Map's chrome overlays land here: the floating top-centre clock digits (pinned time via
# --clock), the bottom-left scale bar (corner normally, stepped above the chip band while a chip is
# up), and — priority order unchanged — the bottom-centre one-slot warning chip.
"$SIM" "$MAP" --boot --routes-dir "$ROUTES" --clock "2025-06-29T14:40" --script "p p p p"   --gpx "$GPX" --at 30 --png "$OUT/map.png"
"$SIM" "$MAP" --boot --routes-dir "$ROUTES" --script "p p p p b" --gpx "$GPX" --at 30 --png "$OUT/statistics.png"
# The low-battery cue (issue: < 10 %): a warning-red battery glyph in the map's top-left corner.
"$SIM" "$MAP" --boot --routes-dir "$ROUTES" --clock "2025-06-29T14:40" --battery 5 --script "p p p p" --gpx "$GPX" --at 30 --png "$OUT/map-lowbatt.png"
# Climb screen (epic #506, C4). The default protocol-vectors routes don't match the Grimsel replay
# (they're tiny test routes), so ride the committed grimsel-climb.obcr — the route the GPX follows,
# giving the detector its three back-to-back climbs. `--at 1500` replays ~25 min in (progress ~5 km,
# ~40 % up climb 0), so the cursor sits mid-profile with grade stripes on both sides. `--open-climb`
# then swaps the base riding view for the Climb screen; it isn't reachable by gesture until C5 wires
# the Back-cycle, so this debug seam opens it. Staged in a temp routes dir (the fixture lives in the
# sim crate's assets, not protocol-vectors).
CLIMBROUTES="$(mktemp -d)"; trap 'rm -rf "$TRACKS" "$NAVDIR" "$CLIMBROUTES"' EXIT
cp "$repo_root/firmware/obc-sim/assets/grimsel-climb.obcr" "$CLIMBROUTES/"
"$SIM" "$MAP" --boot --routes-dir "$CLIMBROUTES" --script "p p p p" --gpx "$GPX" --at 1500 --open-climb --png "$OUT/climb.png"
"$SIM" "$MAP" --boot --routes-dir "$ROUTES" --script "p p p p p" --gpx "$GPX" --at 30 --png "$OUT/ridecontrol.png"
# Route-less ride tracking (Menu's Map station). The Menu compass is Routes/Rides/POIs/Map/Settings,
# so the Map station is three cw detents from the Routes start (`r r r w`). A live `--gpx` fix pins
# the follow camera + marker so the frames reproduce (no route → no magenta line, no off-route chip).
# (a) The route-less BROWSE map: Menu → Map (not tracking) → the follow map with clock + scale bar.
"$SIM" "$MAP" --boot --clock "2025-06-29T14:40" --gpx "$GPX" --at 30 --script "B r r r w p"     --png "$OUT/map-browse.png"
# (b) The start card (browse map → press): "RIDE" — Start ride / Back.
"$SIM" "$MAP" --boot --clock "2025-06-29T14:40" --gpx "$GPX" --at 30 --script "B r r r w p p"   --png "$OUT/ride-start.png"
# (c) A route-less RIDING map (start card → Start ride): the follow map with the recorded breadcrumb,
# no route line and no off-route chip (there's no route to be off).
"$SIM" "$MAP" --boot --clock "2025-06-29T14:40" --gpx "$GPX" --at 30 --script "B r r r w p p p" --png "$OUT/map-routeless.png"
# (d) The route-less Statistics page: the "No route loaded" band note over the stat grid, where the
# route-relative tiles (KM TO GO, TO CLIMB) read "--" and the rest are live.
"$SIM" "$MAP" --boot --clock "2025-06-29T14:40" --gpx "$GPX" --at 30 --script "B r r r w p p p b" --png "$OUT/statistics-routeless.png"
"$SIM" "$MAP" --boot --routes-dir "$ROUTES" --script "p p p p B p r p" --png "$OUT/routeswap.png"
# Pan mode: the pan HUD (chevrons + compass) plus the bottom-left scale bar (visible in pan too);
# the clock digits are suppressed while panning (the top chevron owns the slot).
"$SIM" "$MAP" --boot --routes-dir "$ROUTES" --clock "2025-06-29T14:40" --script "p p p p h" --png "$OUT/map-pan.png"
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
"$SIM" "$MAP" --boot --routes-dir "$ROUTES" --script "p p p p" --inject-upload 1 --png "$OUT/routeswap-received.png"
# Active route replaced (riding id 0, id 0 re-uploaded): the info-only "ROUTE UPDATED" card.
"$SIM" "$MAP" --boot --routes-dir "$ROUTES" --script "p p p p" --inject-upload-replace 0 --png "$OUT/route-updated.png"
# Storage/sensor warnings (issue #504). The three undismissable boot faults are drawn standalone
# (no app), exactly as `main` does at the fatal SD/map sites — `--boot-fault` bypasses render_frame.
"$SIM" "$MAP" --boot-fault nocard --png "$OUT/fault-nocard.png"
"$SIM" "$MAP" --boot-fault nomap  --png "$OUT/fault-nomap.png"
"$SIM" "$MAP" --boot-fault badmap --png "$OUT/fault-badmap.png"
# The dismissable warning card, raised through the real notify_warning seam: one missing sensor, and
# the coalesced worst case (all three sensors absent + a slow/fragmented map).
"$SIM" "$MAP" --boot --inject-warning gps --png "$OUT/warning-gps.png"
"$SIM" "$MAP" --boot --inject-warning gps,altimeter,compass,map --png "$OUT/warning-all.png"

# The idle-return picker in its open (editing) state, on the Display page's third row
# (Home → Menu → Settings → Display, two turns down to Idle, press to open the picker). The idle
# timeout still works end-to-end: sit in Settings, elapse (`I`), land back on Home.
"$SIM" "$MAP" --boot --script "B l p r r r r p r r p" --png "$OUT/display-idle-return.png"
"$SIM" "$MAP" --boot --script "B l p I"               --png "$OUT/idle-return-home.png"

echo "ui-snapshots: 54 screens rendered into $OUT/"
