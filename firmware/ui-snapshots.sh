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
# A second, tiny replay that lies *on* protocol-vectors' `route-waypoints.obcr` ("Vector Loop") — the
# Grimsel climb GPX above is far off it, so it can't drive the waypoint chip/ticks. Synthetic + its
# provenance are pinned in the assets README; it stops ~300 m short of the "Pass Summit" waypoint.
WPTGPX="$repo_root/firmware/obc-sim/assets/vector-loop-replay.gpx"
ROUTES="$repo_root/protocol-vectors"
OUT="${1:-ui-snapshots}"

mkdir -p "$OUT"

# A deterministic /tracks fixture for the Rides screen (#454): two stored ride objects. The pinned
# `ride-v1.bin` protocol vector *is* a valid `RD{id}.ORD` (the stored file == the wire object), so we
# copy it under two ids. No `SYNCED.SET` → both read as unsynced, so the rows draw the hollow
# not-synced ring. RD1 gets its header `distance` patched (u32 LE at byte 16 = 3 + name_len 9 + 4;
# 42500 → 17800 m) so the two same-day rides are visually distinct on the redesigned rows'
# `D MON · distance` line (#680's C1 re-cut) — the exact ambiguity the re-cut exists to prevent.
# Distance isn't part of the object's length validation, so the patched copy still reads as a valid
# ride. Staged in a temp dir cleaned on exit.
TRACKS="$(mktemp -d)"
# A scratch routes dir for the create-route sweep below — the router writes its reserved
# `_nav.obcr` there instead of littering a `routes/` in the working directory.
NAVDIR="$(mktemp -d)"
trap 'rm -rf "$TRACKS" "$NAVDIR"' EXIT
cp "$ROUTES/ride-v1.bin" "$TRACKS/RD0.ORD"
cp "$ROUTES/ride-v1.bin" "$TRACKS/RD1.ORD"
printf '\x88\x45\x00\x00' | dd of="$TRACKS/RD1.ORD" bs=1 seek=16 conv=notrunc status=none

# Menu navigation: Home's press (and back-hold) opens the compass Menu — the single door into the
# app — so the Route menu is now `p p` from boot (open Menu, then press the Routes station, which the
# menu starts on). The compass menu is Routes / Rides / POIs / Map / Settings, so Settings is one ccw
# detent (`l`, wrapping) from the Routes start, Rides is one cw (`r`), POIs two cw (`r r`). `w`
# settles the needle sweep after a turn — and the back-hold charge indicator (a half-disc at the
# right screen edge) decays over a few frames, so scripts that snapshot within ~3 tokens of a `B`
# end in `w` too, or the residue bakes into the PNG.
"$SIM" "$MAP" --boot --clock "2025-07-10T09:41" --png "$OUT/home.png" --battery 45
# The Route list: arrow-less, column-aligned two-line rows (distance under the name, the climb group
# at a fixed second column) with no footer — hold-to-delete moved to the Route overview (T3, #681).
"$SIM" "$MAP" --boot --script "p p"          --routes-dir "$ROUTES" --png "$OUT/routemenu.png"
"$SIM" "$MAP" --boot --battery 45 --script "B w"          --png "$OUT/menu.png"
# Rides screen (#454, rows redesigned by #680): the name + sync-glyph rows over the olive
# `D MON · distance` line (the tracks fixture's two unsynced same-day rides draw the hollow ring
# and distinct distances; the drop-rightmost guard normally never fires at this shape). `p` presses
# into the Rides screen from the Menu (one `r` detent + `w` settle).
"$SIM" "$MAP" --boot --tracks-dir "$TRACKS" --script "B r w p"     --png "$OUT/rides.png"
# The Ride detail (#680): press the highlighted ride — RIDE bar with the "not synced" slot, name,
# date · time, the recorded track's elevation band (the staged RD0.ORD fixture, host-filled), the
# four-row ledger, and the guarded Delete-ride row.
"$SIM" "$MAP" --boot --tracks-dir "$TRACKS" --script "B r w p p"   --png "$OUT/ride-detail.png"
# The detail's delete charging: `H` partial-holds the encoder over the Delete-ride row, so its
# warning-red fill draws mid-charge (the guarded-hold idiom, ride_control's pattern).
"$SIM" "$MAP" --boot --tracks-dir "$TRACKS" --script "B r w p p H" --png "$OUT/ride-detail-delete.png"
# The delete row HIDDEN while a ride is being recorded (owner review round 1 — no greyed face):
# ride route 0 (`p p p p` → Map, riding) **with the GPX replay driving fixes** — the tracking
# session only starts once positions flow, and `is_tracking` (the hiding predicate) is
# `session.is_some()`, so without `--gpx` this frame would wrongly show the live delete row. Then
# BackHold to the Menu (`B`), turn to the Rides station (`r w`), press into the detail — the page
# ends at the stat ledger with NO Delete-ride row at all, and `H` fills nothing.
"$SIM" "$MAP" --boot --routes-dir "$ROUTES" --tracks-dir "$TRACKS" --gpx "$GPX" --at 30 --script "p p p p B r w p p H" --png "$OUT/ride-detail-recording.png"
"$SIM" "$MAP" --boot --script "B r r w"      --png "$OUT/menu-pois.png"
# POIs browser (#425): the category list, then a populated nearest-16 list. The list's bearing
# arrows are live, so pin a deterministic fix (grimsel map centre) + heading so they reproduce.
"$SIM" "$MAP" --boot --script "B r r w p"    --png "$OUT/poi-menu.png"
"$SIM" "$MAP" --boot --center 8305000,46601000 --heading 0 --script "B r r w p p" --png "$OUT/poi-list.png"
# POI detail (#444, reworked in #685): category glyph on the name row, the promoted distance +
# bearing row, the hours + the OPEN/CLOSED pill, and the full-width "Route here" footer bar. The
# hours/badge need the hours-rich monaco fixture (grimsel has no shop hours). Pin the Resupply
# "Carrefour" supermarket (--center on it → row 0), a fix + heading for the live arrow, and a
# deterministic --clock (Mon 2025-01-06 12:00 → OPEN). `p d p` presses into the list, draws once
# to fill the lazy snapshot, then presses the POI into its detail.
MONACO="$repo_root/firmware/obc-sim/assets/monaco.obcm"
"$SIM" "$MONACO" --boot --center 7416969,43730798 --heading 0 --clock "2025-01-06T12:00" \
    --script "B r r w p r r r p d p" --png "$OUT/poi-detail.png"
# The closed state (#685): the same detail at Mon 23:00 — after Carrefour's 08:00-21:00 — so the
# pill wears its warning-red CLOSED face.
"$SIM" "$MONACO" --boot --center 7416969,43730798 --heading 0 --clock "2025-01-06T23:00" \
    --script "B r r w p r r r p d p" --png "$OUT/poi-detail-closed.png"
# POI create-route flow (epic #116, R4). The `d` token also drains a pending create-route request
# (running the real A* router over the map's v8 nav graph), so one script walks the whole flow.
# The confirm (#685: the category glyph in the T1 slot + the straight-line 'NNN m away' under
# the name): detail of a resupply POI ~600 m away → press.
"$SIM" "$MONACO" --boot --routes-dir "$NAVDIR" --center 7420000,43735000 --heading 0 --clock "2025-01-06T12:00" \
    --script "B r r w p r r r p d p p" --png "$OUT/nav-confirm.png"
# The computed-route overview (length only — no elevation band, no climb/descent rows; #685:
# static NEW ROUTE title, the destination name as the first body line, metres below 1 km, and the
# decimated route-shape preview polyline in the middle): confirm → Create route → `d` runs the
# router; the answer swaps in the overview and hands the app the ≤64-point preview.
"$SIM" "$MONACO" --boot --routes-dir "$NAVDIR" --center 7420000,43735000 --heading 0 --clock "2025-01-06T12:00" \
    --script "B r r w p r r r p d p p p d" --png "$OUT/nav-overview.png"
# The planning screen (#499): accepting the confirm swaps to the spinning-needle wait while the
# host steps the resumable planner. `--nav-hold` leaves the recorded request un-drained so the
# screen stays up for the snapshot (needle at its deterministic initial angle).
"$SIM" "$MONACO" --boot --routes-dir "$NAVDIR" --center 7420000,43735000 --heading 0 --clock "2025-01-06T12:00" \
    --script "B r r w p r r r p d p p p" --nav-hold --png "$OUT/nav-planning.png"
# The two locked failure tiers. The range tier ("Too far to route here.") = the router's fixed
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
"$SIM" "$MAP" --boot --script "B l p w"      --png "$OUT/settings.png"
"$SIM" "$MAP" --boot --script "B l p p"      --png "$OUT/datetime.png"
"$SIM" "$MAP" --boot --script "B l p r p"    --png "$OUT/units.png"
# Bike type (routing-v2 N5): the map's §8.6 profile names — grimsel ships Road/Gravel/MTB/Touring,
# each with its own name-matched pixel-art bike (Road/Gravel/MTB/Touring silhouettes). The default
# selection (Road), then detents cycling through the other three — pinning the name list, the cycle,
# and each hero sprite.
"$SIM" "$MAP" --boot --script "B l p r r p"     --png "$OUT/biketype.png"
"$SIM" "$MAP" --boot --script "B l p r r p r"   --png "$OUT/biketype-gravel.png"
"$SIM" "$MAP" --boot --script "B l p r r p r r" --png "$OUT/biketype-mtb.png"
"$SIM" "$MAP" --boot --script "B l p r r p l"   --png "$OUT/biketype-touring.png"
"$SIM" "$MAP" --boot --script "B l p r r r p"  --png "$OUT/stats-settings.png"
# The Waypoints mode row (epic #523): the Stats settings screen's 4th press-to-cycle row, under
# Climb. Three extra detents park the amber cursor on it, showing the default `Approach` mode.
"$SIM" "$MAP" --boot --script "B l p r r r p r r r" --png "$OUT/settings-stats-waypoints.png"
"$SIM" "$MAP" --boot --script "B l p r r r p r p" --png "$OUT/fields.png"
# The 2×3 waypoint list panel placed in the WYSIWYG field editor (epic #523): from the Fields grid,
# six detents reach the ADD ghost (the six default tiles fill page 1), press to open the picker, then
# five detents to `Waypoint list` (last in StatField::ALL) and press. The page-sized panel lands on
# its own page — the `2 / 3` counter, full-width and three rows tall (`--` with no route loaded).
"$SIM" "$MAP" --boot --script "B l p r r r p r p r r r r r r p r r r r r p" --png "$OUT/fields-wpt-panel.png"
# The Display page (row 4): the two Map-overlay toggles + the idle-return picker moved from Power.
"$SIM" "$MAP" --boot --script "B l p r r r r p"   --png "$OUT/display.png"
"$SIM" "$MAP" --boot --script "B l p r r r r r p" --png "$OUT/power.png"
# Bluetooth screen (#455): the main state (radio on, advertising, a stored bond -> Paired: yes) and
# the Forget-phone guarded hold mid-charge (select the Forget row, then a partial hold fills it).
"$SIM" "$MAP" --boot --ble-paired --script "B l p r r r r r r p"     --png "$OUT/bluetooth.png"
"$SIM" "$MAP" --boot --ble-paired --script "B l p r r r r r r p r H" --png "$OUT/bluetooth-forget-hold.png"
# The Language screen (epic #602): the endonym value picker (row 7). The default (English), then two
# detents cycling to Français — pinning the ç glyph the Latin font (#601) adds.
"$SIM" "$MAP" --boot --script "B l p r r r r r r r p"     --png "$OUT/language.png"
"$SIM" "$MAP" --boot --script "B l p r r r r r r r p r r" --png "$OUT/language-french.png"
# Factory Reset moved one row down (System inserted at index 8 by epic #615 S5), so Reset is now 9
# detents from the Date&Time top: `r`x9, press in, arm (press), then partial-hold to fill the bar.
"$SIM" "$MAP" --boot --script "B l p r r r r r r r r r p p H" --png "$OUT/reset-hold.png"
# System settings screen (epic #615 S5, #620): "Install update from card" (row 8, above Reset).
"$SIM" "$MAP" --boot --script "B l p r r r r r r r r p" --png "$OUT/system.png"
# The row greyed (disabled) while a ride records: ride route 0 (`p p p p`, GPX-driven so the session
# is live), BackHold to the Menu, into Settings -> System — the row dims + shows the "Recording" cue.
"$SIM" "$MAP" --boot --routes-dir "$ROUTES" --tracks-dir "$TRACKS" --gpx "$GPX" --at 30 \
    --script "p p p p B l p r r r r r r r r p" --png "$OUT/system-recording.png"
# The SD-sideload update flow (epic #615 S5, #620). The scan/arm runs board-side; the script leaves
# the "Checking card..." wait on top (System -> Install), and --dfu-scan / --dfu-error answer it
# through the real notify_dfu_scan_result seam (the sim stages a synthetic UPDATE.BIN and runs the
# real obc-dfu scan). --dfu-progress then presses Install so the "Preparing update..." spinner shows.
"$SIM" "$MAP" --boot --script "B l p r r r r r r r r p p" --png "$OUT/dfu-check.png"
"$SIM" "$MAP" --boot --script "B l p r r r r r r r r p p" --dfu-scan normal --png "$OUT/dfu-confirm.png"
"$SIM" "$MAP" --boot --script "B l p r r r r r r r r p p" --dfu-scan same   --png "$OUT/dfu-confirm-same.png"
"$SIM" "$MAP" --boot --script "B l p r r r r r r r r p p" --dfu-scan first  --png "$OUT/dfu-confirm-first.png"
"$SIM" "$MAP" --boot --script "B l p r r r r r r r r p p" --dfu-scan normal --dfu-progress --png "$OUT/dfu-progress.png"
"$SIM" "$MAP" --boot --script "B l p r r r r r r r r p p" --dfu-error notfound   --png "$OUT/dfu-error-notfound.png"
"$SIM" "$MAP" --boot --script "B l p r r r r r r r r p p" --dfu-error unreadable --png "$OUT/dfu-error-unreadable.png"
"$SIM" "$MAP" --boot --script "B l p r r r r r r r r p p" --dfu-error damaged    --png "$OUT/dfu-error-damaged.png"
"$SIM" "$MAP" --boot --script "B l p r r r r r r r r p p" --dfu-error toolarge   --png "$OUT/dfu-error-toolarge.png"
"$SIM" "$MAP" --boot --script "B l p r r r r r r r r p p" --dfu-error fragmented --png "$OUT/dfu-error-fragmented.png"
# The one-time post-update toast, raised through the real notify_update_confirmed seam. A
# deliberately long git-describe tag exercises the version wrap to a second centred line.
"$SIM" "$MAP" --boot --dfu-confirmed "v1.0.0-14-g0a1b2c3-dirty" --png "$OUT/dfu-updated.png"
# Riding flows: Home press → Menu → Routes (p) → Route menu → pick (p) → overview → START (p) → Map.
# The overview also carries the guarded Delete-route row (T3 #681, reordered by owner review round
# 1): the bottommost element, BELOW the raised START RIDE bar — idle here, charging below. While the
# route is the active ride's the row is hidden entirely (no greyed face); that state is unreachable
# by gesture (the active route's overview never opens from the menu), so it has no frame — the
# route_overview delete_enabled tests pin the guard.
"$SIM" "$MAP" --boot --routes-dir "$ROUTES" --script "p p p"     --png "$OUT/routeoverview.png"
# The Delete row charging: `p p p H` opens the overview and partial-holds the encoder over it, so the
# warning-red row fill draws under the "Delete route" label.
"$SIM" "$MAP" --boot --routes-dir "$ROUTES" --script "p p p H"   --png "$OUT/routeoverview-delete.png"
# The Map's chrome overlays land here: the floating top-centre clock digits (pinned time via
# --clock; bumped one font step up in #688 so the time reads at a glance), the bottom-left scale bar
# (corner normally, stepped above the chip band while a chip is up), and — priority order unchanged —
# the bottom-centre one-slot warning chip.
"$SIM" "$MAP" --boot --routes-dir "$ROUTES" --clock "2025-06-29T14:40" --script "p p p p"   --gpx "$GPX" --at 30 --png "$OUT/map.png"
"$SIM" "$MAP" --boot --routes-dir "$ROUTES" --script "p p p p b" --gpx "$GPX" --at 30 --png "$OUT/statistics.png"
# The low-battery cue (issue: < 10 %): a warning-red battery glyph in the map's top-left corner.
"$SIM" "$MAP" --boot --routes-dir "$ROUTES" --clock "2025-06-29T14:40" --battery 5 --script "p p p p" --gpx "$GPX" --at 30 --png "$OUT/map-lowbatt.png"
# Waypoint UI (epic #523). protocol-vectors holds two routes in filename order: id 0 = route-plain,
# id 1 = route-waypoints ("Vector Loop": named waypoints Brunnen @ ~0 m and Pass Summit @ ~1.70 km on
# a 2.20 km track). The default `p p p p` rides id 0, so the extra `r` after the Route-menu press
# (`p p r p p`) picks id 1 — the *only* route these shots use. `--gpx $WPTGPX` is the committed replay
# that lies on that track, so the matcher locks on and progress drives the chip/tick countdowns; the
# Grimsel basemap doesn't reach 48°N, which is fine — these frames pin the waypoint chrome, not the map.
# (a) Map diamonds: at the start (--at 5 ⇒ ~30 m in) the black Brunnen diamond sits on the route by
# the marker — waypoints render as always-on ink furniture on the route line.
"$SIM" "$MAP" --boot --routes-dir "$ROUTES" --clock "2025-06-29T14:00" --script "p p r p p" --gpx "$WPTGPX" --at 5   --png "$OUT/map-waypoints.png"
# (b) The Approach chip: replayed to ~300 m short of Pass Summit (inside the 500 m approach radius),
# default `Approach` mode → the calm `◆ Pass Summit  299m` pill counts down at bottom-centre with the
# full name visible (#688 widened the name allocation), the scale bar stepped up above the chip band.
"$SIM" "$MAP" --boot --routes-dir "$ROUTES" --clock "2025-06-29T14:03" --script "p p r p p" --gpx "$WPTGPX" --at 233 --png "$OUT/map-wpt-chip.png"
# (c) Stats mid-route: the amber live-fraction progress bar carries a black tick per named waypoint
# (Brunnen at the left edge, Pass Summit at its ~0.77 fraction) with the fill sweeping between them.
"$SIM" "$MAP" --boot --routes-dir "$ROUTES" --script "p p r p p b" --gpx "$WPTGPX" --at 233 --png "$OUT/stats-wpt.png"
# Climb screen (epic #506, C4). The default protocol-vectors routes don't match the Grimsel replay
# (they're tiny test routes), so ride the committed grimsel-climb.obcr — the route the GPX follows,
# giving the detector its three back-to-back climbs. `--at 1500` replays ~25 min in (progress ~5 km,
# ~40 % up climb 0), so the cursor sits mid-profile with grade stripes on both sides. The title bar
# carries the summit-flag glyph left of the summit elevation (#688). `--open-climb` then swaps the
# base riding view for the Climb screen; it isn't reachable by gesture until C5 wires the Back-cycle,
# so this debug seam opens it. Staged in a temp routes dir (the fixture lives in the sim crate's
# assets, not protocol-vectors).
CLIMBROUTES="$(mktemp -d)"; trap 'rm -rf "$TRACKS" "$NAVDIR" "$CLIMBROUTES"' EXIT
cp "$repo_root/firmware/obc-sim/assets/grimsel-climb.obcr" "$CLIMBROUTES/"
"$SIM" "$MAP" --boot --routes-dir "$CLIMBROUTES" --script "p p p p" --gpx "$GPX" --at 1500 --open-climb --png "$OUT/climb.png"
"$SIM" "$MAP" --boot --routes-dir "$ROUTES" --script "p p p p p" --gpx "$GPX" --at 30 --png "$OUT/ridecontrol.png"
# Route-less ride tracking (Menu's Map station). The Menu compass is Routes/Rides/POIs/Map/Settings,
# so the Map station is three cw detents from the Routes start (`r r r w`). A live `--gpx` fix pins
# the follow camera + marker so the frames reproduce (no route → no magenta line, no off-route chip).
# (a) The route-less BROWSE map: Menu → Map (not tracking) → the follow map with clock + scale bar,
# and — new in T6 (#684) — the one-shot `Press to start a ride` hint chip (a two-line pill, since the
# sentence can't fit one line at 240 px) at the bottom on entry, the scale bar stepped above it. The
# `-settled` frame runs the browse map ~4.8 s past entry (enough `w` tokens > the 4 s window) to prove
# the hint auto-hides and the scale bar drops back to the corner. (The GPX replay runs after the
# script and drives the hint's clock not at all, so the extra `w`s are what expire it.)
"$SIM" "$MAP" --boot --clock "2025-06-29T14:40" --gpx "$GPX" --at 30 --script "B r r r w p"     --png "$OUT/map-browse.png"
"$SIM" "$MAP" --boot --clock "2025-06-29T14:40" --gpx "$GPX" --at 30 --script "B r r r w p w w w w w w" --png "$OUT/map-browse-settled.png"
# (b) The start card (browse map → press, T6 #684): the hero bike (the selected profile's sprite +
# colour) over its profile name, the two-row GPS / Battery checklist (the static Card row dropped
# in owner review round 1), then Start ride / Back. `--battery 45` pins the % and the `--gpx --at
# 30` fix makes GPS read `fix`; the second frame drops the `--gpx` (no fix) so GPS reads
# `searching..` (and a low --battery to vary the row).
"$SIM" "$MAP" --boot --clock "2025-06-29T14:40" --gpx "$GPX" --at 30 --battery 45 --script "B r r r w p p"   --png "$OUT/ride-start.png"
"$SIM" "$MAP" --boot --clock "2025-06-29T14:40" --battery 8 --script "B r r r w p p"   --png "$OUT/ride-start-nofix.png"
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
"$SIM" "$MAP" --boot --ble-connected --clock "2025-07-10T09:41" --png "$OUT/home-ble.png" --battery 45
"$SIM" "$MAP" --boot --ble-connected --battery 100 --script "B w" --png "$OUT/menu-ble.png"
# BLE passkey card (#449): the host-pushed 6-digit LESC pairing code, rendered huge — plain
# `000042` (ungrouped, owner review round 1) under the device<->phone pair glyph (#679).
# `--ble-passkey N` injects the passkey exactly as the sim control-panel "Pairing" toggle does;
# the card auto-opens.
"$SIM" "$MAP" --boot --ble-passkey 42 --png "$OUT/passkey-card.png"
# Route-upload popups (#451), all three variants. `--inject-upload[-replace] ID` raises the upload
# event after the script, exactly as the control panel's inject buttons do. protocol-vectors holds
# two routes: id 0 = route-plain, id 1 = route-waypoints (filename order).
# Idle: "ROUTE RECEIVED" — a stats line, a mini elevation sparkline (route 0 has elevation), and
# View route / Dismiss (#682).
"$SIM" "$MAP" --boot --routes-dir "$ROUTES" --inject-upload 0 --png "$OUT/route-received.png"
# Tracking (riding id 0, id 1 arrives): the retitled Route-swap popup.
"$SIM" "$MAP" --boot --routes-dir "$ROUTES" --script "p p p p" --inject-upload 1 --png "$OUT/routeswap-received.png"
# Active route replaced (riding id 0, id 0 re-uploaded): the info-only "ROUTE UPDATED" card, with
# the shared check in the glyph slot (#679).
"$SIM" "$MAP" --boot --routes-dir "$ROUTES" --script "p p p p" --inject-upload-replace 0 --png "$OUT/route-updated.png"
# Storage/sensor warnings (issue #504). The three undismissable boot faults are drawn standalone
# (no app), exactly as `main` does at the fatal SD/map sites — `--boot-fault` bypasses render_frame.
# Each carries the shared SD-card pictogram + the parallel what/fix copy family (#679).
"$SIM" "$MAP" --boot-fault nocard --png "$OUT/fault-nocard.png"
"$SIM" "$MAP" --boot-fault nomap  --png "$OUT/fault-nomap.png"
"$SIM" "$MAP" --boot-fault badmap --png "$OUT/fault-badmap.png"
# The dismissable warning card, raised through the real notify_warning seam: one missing sensor, and
# the coalesced worst case (all three sensors absent + a slow/fragmented map) — the widest layout
# for the #679 glyph-slot triangle + per-sensor leading glyphs, pinning that nothing collides.
"$SIM" "$MAP" --boot --inject-warning gps --png "$OUT/warning-gps.png"
"$SIM" "$MAP" --boot --inject-warning gps,altimeter,compass,map --png "$OUT/warning-all.png"

# The idle-return picker in its open (editing) state, on the Display page's third row
# (Home → Menu → Settings → Display, two turns down to Idle, press to open the picker). The idle
# timeout still works end-to-end: sit in Settings, elapse (`I`), land back on Home.
"$SIM" "$MAP" --boot --script "B l p r r r r p r r p" --png "$OUT/display-idle-return.png"
"$SIM" "$MAP" --boot --script "B l p I"               --png "$OUT/idle-return-home.png"

# Per-language sweep (epic #602, L5). The i18n catalog (obc-app/i18n/*.toml -> Msg/TABLE) renders
# every screen in the runtime Language setting; `--lang de|fr|es` seeds it into the headless
# Settings (English is the default the sweep above already captures, so it isn't re-shot). Re-render
# the text-heaviest representative slice — Menu, the Settings list + a couple of value screens
# (Units, Stats), Statistics, Climb, the off-route Map (warning chip + scale bar), and the Route
# overview — in each of de/fr/es. These are the shots to eyeball for a stray `?` (a char outside the
# Latin font's #601 repertoire, caught deterministically by `obc-app`'s i18n repertoire test) and for
# clipped / overflowing rows now that the copy is longer. Scripts mirror the English lines above.
for lang in de fr es; do
    "$SIM" "$MAP" --boot --lang "$lang" --script "B w"           --png "$OUT/menu-$lang.png"
    "$SIM" "$MAP" --boot --lang "$lang" --script "B l p w"       --png "$OUT/settings-$lang.png"
    "$SIM" "$MAP" --boot --lang "$lang" --script "B l p r p"     --png "$OUT/units-$lang.png"
    "$SIM" "$MAP" --boot --lang "$lang" --script "B l p r r r p" --png "$OUT/stats-settings-$lang.png"
    "$SIM" "$MAP" --boot --lang "$lang" --routes-dir "$ROUTES" --clock "2025-06-29T14:40" --gpx "$GPX" --at 30 \
        --script "p p p p b"    --png "$OUT/statistics-$lang.png"
    "$SIM" "$MAP" --boot --lang "$lang" --routes-dir "$CLIMBROUTES" --gpx "$GPX" --at 1500 --open-climb \
        --script "p p p p"      --png "$OUT/climb-$lang.png"
    "$SIM" "$MAP" --boot --lang "$lang" --routes-dir "$ROUTES" --clock "2025-06-29T14:40" --gpx "$GPX" --at 30 \
        --script "p p p p"      --png "$OUT/map-$lang.png"
    "$SIM" "$MAP" --boot --lang "$lang" --routes-dir "$ROUTES" --script "p p p" --png "$OUT/routeoverview-$lang.png"
    # The received-route card family (#682): the idle card's View route / Dismiss rows, and the
    # mid-ride swap + ROUTE ACTIVE cards' Swap / Finish & new / Cancel rows — eyeball each for a
    # clipped option row now that the copy is per-language.
    "$SIM" "$MAP" --boot --lang "$lang" --routes-dir "$ROUTES" --inject-upload 0 --png "$OUT/route-received-$lang.png"
    "$SIM" "$MAP" --boot --lang "$lang" --routes-dir "$ROUTES" --script "p p p p" --inject-upload 1 \
        --png "$OUT/routeswap-received-$lang.png"
    "$SIM" "$MAP" --boot --lang "$lang" --routes-dir "$ROUTES" --script "p p p p B p r p" --png "$OUT/routeswap-$lang.png"
    # The ride-start card (T6 #684): the checklist labels/values (GPS/Battery) are the copy to
    # eyeball for clipped rows in the longer translations. --battery 100 pins the widest % value.
    "$SIM" "$MAP" --boot --lang "$lang" --battery 100 --script "B r r r w p p" --png "$OUT/ride-start-$lang.png"
    # The browse-map start hint chip (T6 #684): the two-line pill in each language, to eyeball for a
    # clipped line now that the copy is longer.
    "$SIM" "$MAP" --boot --lang "$lang" --clock "2025-06-29T14:40" --gpx "$GPX" --at 30 \
        --script "B r r r w p" --png "$OUT/map-browse-$lang.png"
    # The SD-sideload update flow (epic #615 S5): the System row, the first-install confirm (the
    # worst case for vertical fit — the two-row version table + the no-undo note, which wraps to two
    # Label lines in the longer translations), the progress spinner, an error card, and the
    # post-update toast — the text-heaviest DFU screens, to eyeball for clipped/overflowing copy.
    "$SIM" "$MAP" --boot --lang "$lang" --script "B l p r r r r r r r r p" --png "$OUT/system-$lang.png"
    "$SIM" "$MAP" --boot --lang "$lang" --script "B l p r r r r r r r r p p" --dfu-scan first --png "$OUT/dfu-confirm-$lang.png"
    "$SIM" "$MAP" --boot --lang "$lang" --script "B l p r r r r r r r r p p" --dfu-scan normal --dfu-progress --png "$OUT/dfu-progress-$lang.png"
    "$SIM" "$MAP" --boot --lang "$lang" --script "B l p r r r r r r r r p p" --dfu-error fragmented --png "$OUT/dfu-error-$lang.png"
    "$SIM" "$MAP" --boot --lang "$lang" --dfu-confirmed "v1.0.0-14-g0a1b2c3-dirty" --png "$OUT/dfu-updated-$lang.png"
done

echo "ui-snapshots: 133 screens rendered into $OUT/"
