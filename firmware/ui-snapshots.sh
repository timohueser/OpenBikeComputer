#!/usr/bin/env bash
# PNG snapshot sweep of every UI screen (epic #335's shared regression net):
# headless obc-sim renders, diffed before/after each cleanup phase — byte-identical
# unless a phase explicitly changes pixels.
#
# Usage: ui-snapshots.sh [OUT_DIR]
#   OUT_DIR   where the PNGs land (default: ui-snapshots/)
#
# Env overrides:
#   SIM   the obc-sim binary   (default: <repo>/target/release/obc-sim)
#   MAP   the .obcm map        (default: registry scenario `grimsel`)
#   GPX   the replay track     (default: registry scenario `grimsel`)
#
# The registry sync makes the complete Grimsel and Monaco scenarios available;
# point MAP/GPX at local files to sweep a different region. Exits non-zero on the
# first failing render (set -e), so a broken sim cannot produce a short sweep.
#
# Two rules hold the net together, and every render command below obeys both:
#
#   1. It states its destination with `--expect-screen NAME` (the `screens!` table's own
#      variant string). A scripted recipe is a hostage to the menus it walks: insert one
#      station and `B u p d d d d d p` quietly snapshots a different screen under the old
#      filename. Stating it turns that into a failed sweep. Add the flag to every new
#      command; if you don't know the name, guess, and the error message names the
#      screen the script actually reached.
#   2. Its output is digested in `firmware/ui-snapshots.sha256` — one row per PNG.
#      After a sweep: `python3 firmware/tools/ui_snapshot_manifest.py check
#      firmware/ui-snapshots.sha256 "$OUT"`. A change of pixels is intentional or it is a
#      regression; look at the changed frames first, then record them with `update`.
#
# Coverage against the `screens!` table: 60 of the 61 screens have at least one frame here.
# The exception is **RideRecovery**, the boot card that offers a ride recovered from a durable
# recording after a reset. Its only entry is `App::offer_recovered_ride(RideContinuation)` — a
# host call carrying thirteen reconstructed accumulator fields, which the simulator has no
# fixture for and no gesture can stand in for. It is named here rather than left to be
# discovered: an uncovered screen is exactly where a refactor breaks silently, so adding that
# seed is the way to close the gap, not quietly widening the net's claim.
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
SIM="${SIM:-$repo_root/target/release/obc-sim}"
python3 "$repo_root/tools/fixtures.py" sync sim
fixture_root="$(python3 "$repo_root/tools/fixtures.py" root)"
GRIMSEL_FIXTURES="$fixture_root/sim-grimsel"
MONACO_FIXTURES="$fixture_root/sim-monaco"
MAP="${MAP:-$GRIMSEL_FIXTURES/grimsel.obcm}"
GPX="${GPX:-$GRIMSEL_FIXTURES/tracks/grimsel-climb.gpx}"
# A second, tiny replay that lies *on* specs/vectors' `route-waypoints.obcr` ("Vector Loop") — the
# Grimsel climb GPX above is far off it, so it can't drive the waypoint chip/ticks. Synthetic + its
# provenance are pinned in the assets README; it stops ~300 m short of the "Pass Summit" waypoint.
WPTGPX="$repo_root/fixtures/sources/vector/vector-loop-replay.gpx"
ROUTES="$repo_root/specs/vectors"
OUT="${1:-ui-snapshots}"

mkdir -p "$OUT"

# A deterministic /tracks fixture for the Rides screen (#454): two stored ride objects. The pinned
# `ride-v3.bin` protocol vector is a valid ride object, so we copy it under two simulator-only
# `ride-{id}.obcr` fixture ids. `ride-1` gets its footer `distance` patched (u32 LE at byte 72 = 60-byte sample
# stream + footer offset 12; 12345 → 17800 m) so the two same-day rides are visually distinct on the redesigned rows'
# `D MON · distance` line (#680's C1 re-cut) — the exact ambiguity the re-cut exists to prevent.
# Distance isn't part of the object's length validation, so the patched copy still reads as a valid
# ride. Both fixture rows are conservatively unsynced; flat synced/retention metadata belongs to
# the later ride-domain boundary (#1398). Staged in a temp dir cleaned on exit.
TRACKS="$(mktemp -d)"
# A scratch routes dir for the create-route sweep below — the router writes its reserved
# `_nav.obcr` there instead of littering a `routes/` in the working directory.
NAVDIR="$(mktemp -d)"
# A routes dir with a trip folder (epic #526, TR3): the two specs/vectors routes + the sim crate's
# grimsel-climb, named so their sorted-scan ids are 0/1/2, plus the committed `TP1.OBT` ("Alpen
# Traverse", stages [0, 1, 99]) — so the top level shows one folder grouping ids 0+1 (its two vector
# routes, the 99 dangling) above the loose grimsel route (id 2), and drilling in lists the two stages.
TRIPDIR="$(mktemp -d)"
# A routes dir holding only the waypoint-less `route-plain` vector route — the Up-ahead
# "nothing ahead" empty states below need a route whose corridor is genuinely empty.
PLAINROUTE="$(mktemp -d)"
# The EL9 ETA A/B (#1077): the Grimsel climb route, and a **zero-elevation twin** of it — the same
# 19 km of geometry with every <ele> zeroed, imported through the sim's own GPX path. One replay
# then drives both, so the only difference between the two ETA frames is the elevation, which is
# exactly what the gradient-aware model is supposed to react to. (The twin also stands in for a
# device-planned route, whose points are all zero-elevation until EL7 fills them from terrain.)
ETAROUTE="$(mktemp -d)"
ETAFLAT="$(mktemp -d)"
trap 'rm -rf "$TRACKS" "$NAVDIR" "$TRIPDIR" "$PLAINROUTE" "$ETAROUTE" "$ETAFLAT"' EXIT
cp "$GRIMSEL_FIXTURES/routes/grimsel-climb.obcr" "$ETAROUTE/"
sed 's#<ele>[^<]*</ele>#<ele>0</ele>#g' "$GPX" > "$ETAFLAT/grimsel-flat.gpx"
"$SIM" --import "$ETAFLAT/grimsel-flat.gpx" --routes-dir "$ETAFLAT" > /dev/null
rm "$ETAFLAT/grimsel-flat.gpx"
cp "$ROUTES/ride-v3.bin" "$TRACKS/ride-0.obcr"
cp "$ROUTES/ride-v3.bin" "$TRACKS/ride-1.obcr"
printf '\x88\x45\x00\x00' | dd of="$TRACKS/ride-1.obcr" bs=1 seek=72 conv=notrunc status=none
cp "$ROUTES/route-plain.obcr"     "$TRIPDIR/1-plain.obcr"
cp "$ROUTES/route-waypoints.obcr" "$TRIPDIR/2-waypoints.obcr"
cp "$ROUTES/route-plain.obcr" "$PLAINROUTE/"
cp "$GRIMSEL_FIXTURES/routes/grimsel-climb.obcr" "$TRIPDIR/3-grimsel.obcr"
# The trip object comes from the repository's own tracked source, not the packaged copy: the
# published `sim-grimsel` package still carries a v1 trip object (`python3 tools/fixtures.py verify
# sim-grimsel` says so), which every reader rejects — so the packaged file silently produced a
# folder-less Route menu under the three trip filenames below.
cp "$repo_root/fixtures/sources/sim-grimsel/routes/TP1.OBT" "$TRIPDIR/TP1.OBT"

# Menu navigation: Home's press (and back-hold) opens the compass Menu — the single door into the
# app — so the Route menu is now `p p` from boot (open Menu, then press the Routes station, which the
# menu starts on). The compass menu is Routes / Rides / POIs / Map / Settings, so Settings is one Up
# step (`u`, wrapping) from the Routes start, Rides is one down (`d`), POIs two down (`d d`). `w`
# settles the needle sweep after a step — and the back-hold charge indicator (a half-disc at the
# right screen edge) decays over a few frames, so scripts that snapshot within ~3 tokens of a `B`
# end in `w` too, or the residue bakes into the PNG.
"$SIM" "$MAP" --boot --clock "2025-07-10T09:41" --expect-screen Home --png "$OUT/home.png" --battery 45
# The Route list: arrow-less, column-aligned two-line rows (distance under the name, the climb group
# at a fixed second column) with no footer — hold-to-delete moved to the Route overview (T3, #681).
"$SIM" "$MAP" --boot --script "p p"          --routes-dir "$ROUTES" --expect-screen RouteMenu --png "$OUT/routemenu.png"
# The Route menu with a trip folder (epic #526, TR3): the `--routes-dir` staged with `TP1.OBT` +
# routes, so the top level shows the "Alpen Traverse" folder row (folder glyph + name + `N routes` +
# summed km/climb) above the loose grimsel route. `p p p` then drills into the folder — the stage
# list, the trip's member routes as standard route rows under the trip's own name as the title.
# Weak spot, stated so nobody mistakes it for coverage: both member routes are specs/vectors routes
# named "Vector Loop", so the stage list's two rows are identical and a refactor that swapped or
# collapsed them would not move a pixel. Distinguishing them needs differently-named member routes,
# which means new committed vectors — worth doing when the stage list is next touched.
"$SIM" "$MAP" --boot --script "p p"   --routes-dir "$TRIPDIR" --expect-screen RouteMenu --png "$OUT/routemenu-trips.png"
"$SIM" "$MAP" --boot --script "p p p" --routes-dir "$TRIPDIR" --expect-screen RouteMenu --png "$OUT/trip-stage-list.png"
# The trip cascade-delete confirm (TR3): long-press the folder (`h` fires the completed hold) → the
# warning-red hold-guarded "Delete all" + "Cancel" card, naming the trip. Entry selects Cancel.
"$SIM" "$MAP" --boot --script "p p h" --routes-dir "$TRIPDIR" --expect-screen TripDelete --png "$OUT/trip-delete-confirm.png"
"$SIM" "$MAP" --boot --battery 45 --script "B w"          --expect-screen Menu --png "$OUT/menu.png"
# Rides screen (#454, rows redesigned by #680, polished in owner review round 2): inset name rows
# over the olive `D MON · distance` line. Both fixtures are unsynced until the later flat
# synced/retention metadata boundary lands.
# `p` presses into the Rides screen from the Menu (one `d` step + `w` settle).
"$SIM" "$MAP" --boot --tracks-dir "$TRACKS" --script "B d w p"     --expect-screen Rides --png "$OUT/rides.png"
# The Ride detail (#680, repaged in owner review round 2, content-paired in round 3): press the
# highlighted ride (the unsynced `ride-1` fixture) — RIDE bar with the "not synced" slot, name, date · time, the
# content-paired pager on its entry page (page A: the recorded track's shape preview, host-filled —
# start disc + end diamond — over DISTANCE + RIDE TIME), and the guarded Delete-ride row.
"$SIM" "$MAP" --boot --tracks-dir "$TRACKS" --script "B d w p p"   --expect-screen RideDetail --png "$OUT/ride-detail.png"
# Page B after the 5 s dwell (seven `w` ticks): the recorded elevation band (the staged fixture,
# host-filled) over AVG + CLIMBED — the same band slot, so nothing jumps.
"$SIM" "$MAP" --boot --tracks-dir "$TRACKS" --script "B d w p p w w w w w w w" --expect-screen RideDetail --png "$OUT/ride-detail-elevation.png"
# The detail's delete charging: `H` partial-holds Select over the Delete-ride row, so its
# warning-red fill draws mid-charge (the guarded-hold idiom, ride_control's pattern).
"$SIM" "$MAP" --boot --tracks-dir "$TRACKS" --script "B d w p p H" --expect-screen RideDetail --png "$OUT/ride-detail-delete.png"
# The delete row HIDDEN while a ride is being recorded (owner review round 1 — no greyed face):
# ride route 0 (`p p p p` → Map, riding) **with the GPX replay driving fixes** — the tracking
# session only starts once positions flow, and `is_tracking` (the hiding predicate) is
# `session.is_some()`, so without `--gpx` this frame would wrongly show the live delete row. Then
# out to the main Menu — mid-ride a BackHold opens the RIDE menu (epic #789), whose fifth station
# is Main menu, so that exit is `B u p` and not a bare `B` — step to the Rides station (`d w`),
# press into the detail: the page ends at the stat ledger with NO Delete-ride row at all, and `H`
# fills nothing.
"$SIM" "$MAP" --boot --routes-dir "$ROUTES" --tracks-dir "$TRACKS" --gpx "$GPX" --at 30 --script "p p p p B u p d w p p H" --expect-screen RideDetail --png "$OUT/ride-detail-recording.png"
"$SIM" "$MAP" --boot --script "B d d w"      --expect-screen Menu --png "$OUT/menu-pois.png"
# POIs browser (#425): the category list, then a populated nearest-16 list. The list's bearing
# arrows are live, so pin a deterministic fix (grimsel map centre) + heading so they reproduce.
"$SIM" "$MAP" --boot --script "B d d w p"    --expect-screen PoiMenu --png "$OUT/poi-menu.png"
"$SIM" "$MAP" --boot --center 8305000,46601000 --heading 0 --script "B d d w p p" --expect-screen PoiList --png "$OUT/poi-list.png"
# POI detail (#444, reworked in #685): category glyph on the name row, the promoted distance +
# bearing row, the hours block with the OPEN/CLOSED pill riding the "Today" caption line
# (right-aligned — owner review round 2's overlay fix), and the full-width "Route here" footer
# bar. The hours/badge need the hours-rich monaco fixture (grimsel has no shop hours). Pin the
# Resupply "Carrefour" supermarket (--center on it → row 0), a fix + heading for the live arrow,
# and a deterministic --clock (Mon 2025-01-06 12:00 → OPEN). `p d p` presses into the list, draws
# once to fill the lazy snapshot, then presses the POI into its detail.
MONACO="$MONACO_FIXTURES/monaco.obcm"
"$SIM" "$MONACO" --boot --center 7416969,43730798 --heading 0 --clock "2025-01-06T12:00" \
    --script "B d d w p d d d p f p" --expect-screen PoiDetail --png "$OUT/poi-detail.png"
# The closed state (#685): the same detail at Mon 23:00 — after Carrefour's 08:00-21:00 — so the
# pill wears its warning-red CLOSED face on the Today line.
"$SIM" "$MONACO" --boot --center 7416969,43730798 --heading 0 --clock "2025-01-06T23:00" \
    --script "B d d w p d d d p f p" --expect-screen PoiDetail --png "$OUT/poi-detail-closed.png"
# The layout worst case (owner review round 2's overlay bug): a two-line wrapping name
# ("Pharmacie du Jardin Exot..") + the format's two-intervals-per-day maximum (split lunch hours,
# Mon 08:30-12:30 / 15:00-19:00) — the stack that used to push the badge under the Route-here
# bar. With the badge on the Today line the whole block clears the footer. Pharmacy is one more
# step into the category list than Resupply.
"$SIM" "$MONACO" --boot --center 7413793,43734832 --heading 0 --clock "2025-01-06T12:00" \
    --script "B d d w p d d d d p f p" --expect-screen PoiDetail --png "$OUT/poi-detail-split-hours.png"
# POI create-route flow (epic #116, R4). The `f` token also drains a pending create-route request
# (running the real A* router over the map's v8 nav graph), so one script walks the whole flow.
# The confirm (#685: the category glyph in the T1 slot + the straight-line 'NNN m away' under
# the name): detail of a resupply POI ~600 m away → press.
"$SIM" "$MONACO" --boot --routes-dir "$NAVDIR" --center 7420000,43735000 --heading 0 --clock "2025-01-06T12:00" \
    --script "B d d w p d d d p f p p" --expect-screen NavConfirm --png "$OUT/nav-confirm.png"
# The computed-route overview (length only — no elevation band, no climb/descent rows; #685:
# static NEW ROUTE title, the destination name as the first body line, metres below 1 km, and the
# decimated route-shape preview polyline in the middle): confirm → Create route → `f` runs the
# router; the answer swaps in the overview and hands the app the ≤64-point preview.
"$SIM" "$MONACO" --boot --routes-dir "$NAVDIR" --center 7420000,43735000 --heading 0 --clock "2025-01-06T12:00" \
    --script "B d d w p d d d p f p p p f" --expect-screen RouteOverview --png "$OUT/nav-overview.png"
# The planning screen (#499): accepting the confirm swaps to the spinning-needle wait while the
# host steps the resumable planner. `--hold nav` consumes the recorded request without starting it,
# so the screen stays up for the snapshot (needle at its deterministic initial angle).
"$SIM" "$MONACO" --boot --routes-dir "$NAVDIR" --center 7420000,43735000 --heading 0 --clock "2025-01-06T12:00" \
    --script "B d d w p d d d p f p p p" --hold nav --expect-screen NavPlanning --png "$OUT/nav-planning.png"
# The two locked failure tiers. The range tier ("Too far to route here.") = the router's fixed
# table exhausting — with no distance cap that IS the device's range limit — which the small
# fixture graphs can't reach (grimsel plans even ~25 km routes inside the 1536-node table), so
# the card is injected through the real notify_nav_result seam with the planning screen on top,
# pinning the exhausted→range-tier mapping. The generic tier ("Couldn't find a route.") stays a
# real plan: a mountain fix with no routable road within the 100 m acceptance envelope.
"$SIM" "$MONACO" --boot --routes-dir "$NAVDIR" --center 7420000,43735000 --heading 0 --clock "2025-01-06T12:00" \
    --script "B d d w p d d d p f p p p" --inject nav-fail=exhausted --expect-screen NavFail --png "$OUT/nav-toofar.png"
"$SIM" "$MAP" --boot --routes-dir "$NAVDIR" --center 8140000,46480000 --heading 0 \
    --script "B d d w p p f p p p f" --expect-screen NavFail --png "$OUT/nav-nopath.png"

# The routed-detour flow (#882) on the dense monaco graph, where a corridor detour genuinely has
# side-street alternatives. The shared prefix plans a ~1.6 km POI route (7th Resupply hit), accepts
# it from the overview (which starts the ride), then `T` runs one route-aware tick — the GUI ticks
# every frame, but the headless script path doesn't, and the Detour chooser reads the tick-built
# `route_total_m`. The chooser opens off the ride menu (`B r p`); the flow then walks
# plan → preview (+cost line) → commit — the commit splices INTO the reserved `_nav.obcr` the
# prefix planned (the self-splice case) and lands back on the riding map.
DETOUR_PRE="B d d w p d d d p f d d d d d d p p p f p T"
# (a) The chooser: skipped-span ink + rejoin ring over the fitted camera, the 600 m minimum span.
"$SIM" "$MONACO" --boot --routes-dir "$NAVDIR" --center 7420000,43735000 --heading 0 --clock "2025-01-06T12:00" \
    --script "$DETOUR_PRE B d p w" --expect-screen Detour --png "$OUT/detour-chooser.png"
# (b) The planning spinner (detour copy; Back would cancel). `--hold detour` consumes the request
# without starting it so the screen stays up, exactly like `--hold nav`.
"$SIM" "$MONACO" --boot --routes-dir "$NAVDIR" --center 7420000,43735000 --heading 0 --clock "2025-01-06T12:00" \
    --script "$DETOUR_PRE B d p w p" --hold detour --expect-screen NavPlanning --png "$OUT/detour-planning.png"
# (c) The preview: a real corridor-blacklisted A* plan over the monaco graph — the detour polyline
# in blue over the warning-colored skipped span, the signed distance-cost line on the HUD.
"$SIM" "$MONACO" --boot --routes-dir "$NAVDIR" --center 7420000,43735000 --heading 0 --clock "2025-01-06T12:00" \
    --script "$DETOUR_PRE B d p d d p f" --expect-screen DetourPreview --png "$OUT/detour-preview.png"
# (d) The failure card: detour title + the one honest remedy hint ("Try a farther rejoin."),
# injected through the real `DetourPlanned` seam with the planning screen on top (the range tier
# is unreachable on the small fixture graphs, same as nav-toofar).
"$SIM" "$MONACO" --boot --routes-dir "$NAVDIR" --center 7420000,43735000 --heading 0 --clock "2025-01-06T12:00" \
    --script "$DETOUR_PRE B d p w p" --inject detour-fail=exhausted --expect-screen NavFail --png "$OUT/detour-fail.png"
# (e) Committed: preview Press splices `original[0..rider] + detour + original[rejoin..]` into the
# reserved route, re-adopts it (session kept), and truncates the flow back to the riding map; the
# trailing `T` re-syncs the route-derived state so the map draws the spliced line.
"$SIM" "$MONACO" --boot --routes-dir "$NAVDIR" --center 7420000,43735000 --heading 0 --clock "2025-01-06T12:00" \
    --script "$DETOUR_PRE B d p d d p f p f T" --expect-screen Map --png "$OUT/detour-committed.png"
# --- Settings ------------------------------------------------------------------------------------
# The Settings list is six themed GROUP rows — Ride / Display / Weather / Connections / Power /
# System — so every settings screen sits two levels down. The shape of every script below is:
#   B u p        open the Menu, one Up step to the Settings station, press -> the Settings list
#   d × G        step to group row G (0 Ride, 1 Display, 2 Weather, 3 Connections, 4 Power,
#                5 System)
#   p            press into the group
#   d × R  [p]   step to row R inside it, and press if that row opens a page / cycles a value
# The old flat list (Date & Time, Auto-delete, Units, Bike type, Stats, Display, Power, Bluetooth,
# Sensors, Language, System, Reset, all straight off the top level) is gone — a leading `d` count is
# now a *group* index, so the pre-group scripts landed several screens off their frame's name.
"$SIM" "$MAP" --boot --script "B u p w"      --expect-screen Settings --png "$OUT/settings.png"

# Ride (group 0) — everything you tune for a ride, in one scrolling seven-row group: Bike type,
# Data fields, Pages, Climb, Waypoints, Up ahead, Auto-delete. It absorbed the old standalone Stats
# and Auto-delete screens, so those two names are gone from the sweep. Only four rows fit the panel;
# the cursor drives the window, so the frames below past row 3 show it scrolled.
"$SIM" "$MAP" --boot --script "B u p p w"    --expect-screen Ride --png "$OUT/ride-settings.png"
# Bike type (routing-v2 N5): the map's §8.6 profile names — grimsel ships Road/Gravel/MTB/Touring,
# each with its own name-matched pixel-art bike (Road/Gravel/MTB/Touring silhouettes). The default
# selection (Road), then steps cycling through the other three — pinning the name list, the cycle,
# and each hero sprite.
"$SIM" "$MAP" --boot --script "B u p p p"     --expect-screen BikeType --png "$OUT/biketype.png"
"$SIM" "$MAP" --boot --script "B u p p p d"   --expect-screen BikeType --png "$OUT/biketype-gravel.png"
"$SIM" "$MAP" --boot --script "B u p p p d d" --expect-screen BikeType --png "$OUT/biketype-mtb.png"
"$SIM" "$MAP" --boot --script "B u p p p u"   --expect-screen BikeType --png "$OUT/biketype-touring.png"
# Data fields (row 1) — the WYSIWYG grid editor.
"$SIM" "$MAP" --boot --script "B u p p d p" --expect-screen StatFields --png "$OUT/fields.png"
# The 2×3 waypoint list panel placed in the WYSIWYG field editor (epic #523): from the Fields grid,
# six steps reach the ADD ghost (the six default tiles fill page 1), press to open the picker, then
# five steps to `Waypoint list` (the last hidden non-sensor entry) and press. The page-sized panel lands on
# its own page — the `2 / 3` counter, full-width and three rows tall (`--` with no route loaded).
"$SIM" "$MAP" --boot --script "B u p p d p d d d d d d p d d d d d p" --expect-screen StatFields --png "$OUT/fields-wpt-panel.png"
# The six `Next: <category>` fields (epic #946, U5). `B u p p d p` is Home -> Settings -> Ride ->
# Data fields (the Fields grid). (a) the Add-field picker scrolled onto the new group: six rows
# wearing the category's own row icon in place of a span badge, directly under `Next waypoint`.
"$SIM" "$MAP" --boot --script "B u p p d p d d d d d d p d d d d d" --expect-screen AddField --png "$OUT/addfield-next-category.png"
# (b) three of them placed, drawn by the WYSIWYG editor's ghost: icon + the localized category word
# + a per-category sample distance (the editor has no route, so the live cell would read `--`).
"$SIM" "$MAP" --boot --stat-fields "next-water,next-campsite,next-lodging" \
    --script "B u p p d p" --expect-screen StatFields --png "$OUT/fields-next-category.png"
# The Waypoints mode row (epic #523): the group's 5th row, under Climb. Four steps park the amber
# cursor on it, showing the default `Approach` mode.
"$SIM" "$MAP" --boot --script "B u p p d d d d" --expect-screen Ride --png "$OUT/settings-ride-waypoints.png"
# The "Up ahead shows" source row (epic #946, U4): the 6th row, right under the Waypoints chip. Five
# steps park the amber cursor on it (default `Both`), and each extra press cycles it one further
# round the ring. The unselected face comes free in the frames above/below, which scroll it into
# view without the cursor.
"$SIM" "$MAP" --boot --script "B u p p d d d d d"     --expect-screen Ride --png "$OUT/settings-ride-up-ahead.png"
"$SIM" "$MAP" --boot --script "B u p p d d d d d p"   --expect-screen Ride --png "$OUT/settings-ride-up-ahead-waypoints.png"
"$SIM" "$MAP" --boot --script "B u p p d d d d d p p" --expect-screen Ride --png "$OUT/settings-ride-up-ahead-pois.png"
# The Auto-delete row (epic #638 S5, folded into this group from its old standalone page): the
# synced-ride retention ring on the last row, defaulting to 1 week. Six steps park the cursor on it;
# one press cycles to the next value (1 month) for the stepped shot.
"$SIM" "$MAP" --boot --script "B u p p d d d d d d"   --expect-screen Ride --png "$OUT/settings-ride-autodelete.png"
"$SIM" "$MAP" --boot --script "B u p p d d d d d d p" --expect-screen Ride --png "$OUT/settings-ride-autodelete-month.png"

# Display (group 1): the two Map-overlay toggles + the idle-return picker moved from Power. The
# picker in its open (editing) state is two rows down, press to open.
"$SIM" "$MAP" --boot --script "B u p d p"       --expect-screen Display --png "$OUT/display.png"
"$SIM" "$MAP" --boot --script "B u p d p d d p" --expect-screen Display --png "$OUT/display-idle-return.png"

# Weather (group 2) is the refresh-interval picker; it is shot with the rest of the weather
# surfaces further down (`weather-settings.png`), which reach it from the Menu's Weather station.
#
# Connections (group 3): the two radios in one drawer — Phone (Bluetooth) then Sensors.
"$SIM" "$MAP" --boot --script "B u p d d d p"  --expect-screen Connections --png "$OUT/connections.png"
# Bluetooth screen (#455, Forget restyled to the Pause-menu row family in owner review round 3):
# the main state (radio on, advertising, a stored bond -> Paired: yes, the Forget row a plain label
# at the bottom anchor), the row selected (a step puts the shaded guarded base on it), the guarded
# hold mid-charge (a partial hold fills it warning-red), and the unpaired state — no bond, so the
# Forget row isn't drawn at all (the round-1 only-when-possible grammar).
"$SIM" "$MAP" --boot --ble paired --script "B u p d d d p p"     --expect-screen Bluetooth --png "$OUT/bluetooth.png"
"$SIM" "$MAP" --boot --ble paired --script "B u p d d d p p d"   --expect-screen Bluetooth --png "$OUT/bluetooth-forget-selected.png"
"$SIM" "$MAP" --boot --ble paired --script "B u p d d d p p d H" --expect-screen Bluetooth --png "$OUT/bluetooth-forget-hold.png"
"$SIM" "$MAP" --boot              --script "B u p d d d p p"     --expect-screen Bluetooth --png "$OUT/bluetooth-unpaired.png"
# Sensors screen (BLE sensors epic #707, SE7) — the group's second row, under Phone. `--sensors screen`
# drives the sim's fake central manager: the three-row list (Heart rate Connected · 78 %, Power
# Searching, Cadence Not set — the HR row selected, so its hold-to-forget footer shows), and the scan
# list one press deeper (the HR-filtered discovered sensors, name/address + RSSI). A third run with no
# fake manager pins the empty `Searching...` state while the scan finds nothing.
"$SIM" "$MAP" --boot --sensors screen --script "B u p d d d p d p"   --expect-screen Sensors --png "$OUT/sensors.png"
"$SIM" "$MAP" --boot --sensors screen --script "B u p d d d p d p p" --expect-screen SensorScan --png "$OUT/sensors-scan.png"
"$SIM" "$MAP" --boot                  --script "B u p d d d p d p p" --expect-screen SensorScan --png "$OUT/sensors-scanning.png"

# Power (group 4): the GPS fix-interval stepper + the power-saver toggle.
"$SIM" "$MAP" --boot --script "B u p d d d d p" --expect-screen Power --png "$OUT/power.png"

# System (group 5) — the device drawer: Units, Date & Time, Language, Firmware, About, Reset. The
# menu itself first, then each row's page.
"$SIM" "$MAP" --boot --script "B u p d d d d d p"     --expect-screen System --png "$OUT/system.png"
"$SIM" "$MAP" --boot --script "B u p d d d d d p p"   --expect-screen Units --png "$OUT/units.png"
"$SIM" "$MAP" --boot --script "B u p d d d d d p d p" --expect-screen DateTime --png "$OUT/datetime.png"
# The Language screen (epic #602): the endonym value picker. The default (English), then two
# steps cycling to Français — pinning the ç glyph the Latin font (#601) adds.
"$SIM" "$MAP" --boot --script "B u p d d d d d p d d p"     --expect-screen Language --png "$OUT/language.png"
"$SIM" "$MAP" --boot --script "B u p d d d d d p d d p d d" --expect-screen Language --png "$OUT/language-french.png"
# The About page (#1149) — System row 4, above Reset: the read-only credits page.
"$SIM" "$MAP" --boot --script "B u p d d d d d p d d d d p" --expect-screen About --png "$OUT/about.png"
# Factory Reset is the group's last row: five steps in, press to open, arm (press), then a
# partial-hold to fill the bar. (Four steps stopped on About until this recipe was corrected.)
"$SIM" "$MAP" --boot --script "B u p d d d d d p d d d d d p p H" --expect-screen Reset --png "$OUT/reset-hold.png"
# The Firmware page (epic #615 S5, #620) — System row 3, the SD-sideload door ("Install update from
# card") over the read-only device-info ledger.
"$SIM" "$MAP" --boot --script "B u p d d d d d p d d d p" --expect-screen Firmware --png "$OUT/firmware.png"
# The row greyed (disabled) while a ride records: ride route 0 (`p p p p`, GPX-driven so the session
# is live), out through the ride menu's Main-menu station (`B u p` — see ride-detail-recording
# above), then Settings -> System -> Firmware. The row loses its amber box and shows the
# "Recording" cue.
"$SIM" "$MAP" --boot --routes-dir "$ROUTES" --tracks-dir "$TRACKS" --gpx "$GPX" --at 30 \
    --script "p p p p B u p u p d d d d d p d d d p" --expect-screen Firmware --png "$OUT/firmware-recording.png"
# The SD-sideload update flow (epic #615 S5, #620). The scan/arm runs board-side; the script leaves
# the "Checking card..." wait on top (Firmware -> Install), and --dfu scan/error answer it
# through the real notify_dfu_scan_result seam (the sim stages a synthetic UPDATE.BIN and runs the
# real obc-dfu scan). --dfu progress then presses Install so the "Preparing update..." spinner shows.
DFU_PRE="B u p d d d d d p d d d p p"
"$SIM" "$MAP" --boot --script "$DFU_PRE" --expect-screen DfuCheck --png "$OUT/dfu-check.png"
"$SIM" "$MAP" --boot --script "$DFU_PRE" --dfu scan=normal --expect-screen DfuConfirm --png "$OUT/dfu-confirm.png"
"$SIM" "$MAP" --boot --script "$DFU_PRE" --dfu scan=same   --expect-screen DfuConfirm --png "$OUT/dfu-confirm-same.png"
"$SIM" "$MAP" --boot --script "$DFU_PRE" --dfu scan=first  --expect-screen DfuConfirm --png "$OUT/dfu-confirm-first.png"
"$SIM" "$MAP" --boot --script "$DFU_PRE" --dfu progress=normal --expect-screen DfuProgress --png "$OUT/dfu-progress.png"
# The terminal "Installing update" card — the static pre-reset frame the MIP panel holds through
# the whole bootloader install (no spinner by design: the frame freezes at the reset, and the LED
# is named as the liveness signal). --dfu installing runs the board drain's show_dfu_installing swap.
"$SIM" "$MAP" --boot --script "$DFU_PRE" --dfu installing=normal --expect-screen DfuInstalling --png "$OUT/dfu-installing.png"
"$SIM" "$MAP" --boot --script "$DFU_PRE" --dfu error=notfound   --expect-screen DfuError --png "$OUT/dfu-error-notfound.png"
"$SIM" "$MAP" --boot --script "$DFU_PRE" --dfu error=unreadable --expect-screen DfuError --png "$OUT/dfu-error-unreadable.png"
"$SIM" "$MAP" --boot --script "$DFU_PRE" --dfu error=damaged    --expect-screen DfuError --png "$OUT/dfu-error-damaged.png"
"$SIM" "$MAP" --boot --script "$DFU_PRE" --dfu error=toolarge   --expect-screen DfuError --png "$OUT/dfu-error-toolarge.png"
"$SIM" "$MAP" --boot --script "$DFU_PRE" --dfu error=fragmented --expect-screen DfuError --png "$OUT/dfu-error-fragmented.png"
# OBCU v2 (#997): the file is intact but not signed by us — its own card, not "damaged".
"$SIM" "$MAP" --boot --script "$DFU_PRE" --dfu error=untrusted  --expect-screen DfuError --png "$OUT/dfu-error-untrusted.png"
# The one-time post-update toast, raised through the real notify_update_confirmed seam. A
# deliberately long git-describe tag exercises the version wrap to a second centred line.
"$SIM" "$MAP" --boot --dfu confirmed=v1.0.0-14-g0a1b2c3-dirty --expect-screen DfuUpdated --png "$OUT/dfu-updated.png"
# Its failure twin, raised through the real notify_update_failed seam: the boot-outcome reconcile
# found the armed update is not what is running. Both verdicts — the bootloader consumed the arm and
# rolled back (with the staged version named), and an arm the bootloader never consumed (no version
# to name, so the card is the sentence alone). The reverted frame reuses the long git-describe tag
# from the toast above: this card centres the version on ONE line, so the tag runs off both edges —
# the toast's wrap has no counterpart here. Recorded, not fixed, by the verification baseline.
"$SIM" "$MAP" --boot --dfu failed=reverted:v1.0.0-14-g0a1b2c3-dirty --expect-screen DfuFailed --png "$OUT/dfu-failed-reverted.png"
"$SIM" "$MAP" --boot --dfu failed=notstarted --expect-screen DfuFailed --png "$OUT/dfu-failed-notstarted.png"
# Riding flows: Home press → Menu → Routes (p) → Route menu → pick (p) → overview → START (p) → Map.
# The overview also carries the guarded Delete-route row (T3 #681, reordered by owner review round
# 1): the bottommost element, below the START RIDE row. Since owner review round 3 the two action
# rows are the Pause-menu (ride_control) family — entry selects START (the standard amber-selected
# row), a step moves onto the Delete row (its shaded base draws only while selected), and only then
# does a hold charge the delete. While the route is the active ride's the row is hidden entirely (no
# greyed face); that state is unreachable by gesture (the active route's overview never opens from
# the menu), so it has no frame — the route_overview guard tests pin it.
# Entry shows the content-paired pager's page A (owner review round 3): the route's track-shape
# preview (host-decimated, start disc + destination diamond) over its DISTANCE row.
"$SIM" "$MAP" --boot --routes-dir "$ROUTES" --script "p p p"     --expect-screen RouteOverview --png "$OUT/routeoverview.png"
# The Auto-delete expiry row (epic #638 S5). It is a "this route is about to be deleted" heads-up,
# shown ONLY when a *started* deadline is ≤ 5 days out; `routeoverview.png` above is the absent
# state (every route defaults to retention Never — byte-unchanged). `--route-retention LEVEL:AGE`
# stamps every route's meta off the (--clock-pinned) wall clock. The three ≤5-day states — the row
# tucks under the title in the smallest (Label) font, muted label + ink value, and the media band
# starts lower to make room: level 2 = 1 week used 2 days ago → "in 5 d"; level 1 = 1 day used 19 h
# ago → "in 5 h"; level 1 used 25 h ago (past due, before the sweep) → "soon".
"$SIM" "$MAP" --boot --routes-dir "$ROUTES" --clock "2025-07-10T09:41" --route-retention 2:2d \
    --script "p p p" --expect-screen RouteOverview --png "$OUT/routeoverview-expiry.png"
"$SIM" "$MAP" --boot --routes-dir "$ROUTES" --clock "2025-07-10T09:41" --route-retention 1:19h \
    --script "p p p" --expect-screen RouteOverview --png "$OUT/routeoverview-expiry-hours.png"
"$SIM" "$MAP" --boot --routes-dir "$ROUTES" --clock "2025-07-10T09:41" --route-retention 1:25h \
    --script "p p p" --expect-screen RouteOverview --png "$OUT/routeoverview-expiry-soon.png"
# The gate's two "absent" cases must render exactly like `routeoverview.png` (no row, full band):
# a started deadline MORE than 5 days out (level 4 = 1 month used 20 days ago → 10 days left), and a
# route whose clock never started (`unknown` → no deadline).
"$SIM" "$MAP" --boot --routes-dir "$ROUTES" --clock "2025-07-10T09:41" --route-retention 4:20d \
    --script "p p p" --expect-screen RouteOverview --png "$OUT/routeoverview-expiry-far-absent.png"
"$SIM" "$MAP" --boot --routes-dir "$ROUTES" --clock "2025-07-10T09:41" --route-retention 2:unknown \
    --script "p p p" --expect-screen RouteOverview --png "$OUT/routeoverview-expiry-unstarted-absent.png"
# Page B after the 5 s dwell (each `w` elapses ~800 ms; seven cross the flip): the elevation band
# over CLIMB + DESCENT — the same band slot, so nothing jumps.
"$SIM" "$MAP" --boot --routes-dir "$ROUTES" --script "p p p w w w w w w w" --expect-screen RouteOverview --png "$OUT/routeoverview-elevation.png"
# The expiry row on the elevation page: the band's lowered top applies on both pager pages.
"$SIM" "$MAP" --boot --routes-dir "$ROUTES" --clock "2025-07-10T09:41" --route-retention 2:2d \
    --script "p p p w w w w w w w" --expect-screen RouteOverview --png "$OUT/routeoverview-expiry-elevation.png"
# The cursor on the Delete row (idle): `d` moves the selection onto it, nothing charging.
"$SIM" "$MAP" --boot --routes-dir "$ROUTES" --script "p p p d"   --expect-screen RouteOverview --png "$OUT/routeoverview-delete-selected.png"
# The Delete row charging: `p p p d H` selects it, then partial-holds Select, so the
# warning-red row fill draws under the "Delete route" label.
"$SIM" "$MAP" --boot --routes-dir "$ROUTES" --script "p p p d H" --expect-screen RouteOverview --png "$OUT/routeoverview-delete.png"
# The Map's chrome overlays land here: the floating top-centre clock digits (pinned time via
# --clock; bumped one font step up in #688 so the time reads at a glance), the bottom-left scale bar
# (corner normally, stepped above the chip band while a chip is up), and — priority order unchanged —
# the bottom-centre one-slot warning chip.
"$SIM" "$MAP" --boot --routes-dir "$ROUTES" --clock "2025-06-29T14:40" --script "p p p p"   --gpx "$GPX" --at 30 --expect-screen Map --png "$OUT/map.png"
"$SIM" "$MAP" --boot --routes-dir "$ROUTES" --script "p p p p b" --gpx "$GPX" --at 30 --expect-screen Statistics --png "$OUT/statistics.png"
# Elevation-profile Inspect mirrors the Map: hold enters Pan, Select tap toggles Zoom, and another
# Select tap returns to Pan without discarding the magnification. `w` clears the entry hold bulge.
"$SIM" "$MAP" --boot --routes-dir "$ROUTES" --script "p p p p b h w" --gpx "$GPX" --at 30 --expect-screen Statistics --png "$OUT/statistics-pan.png"
"$SIM" "$MAP" --boot --routes-dir "$ROUTES" --script "p p p p b h p d d d w" --gpx "$GPX" --at 30 --expect-screen Statistics --png "$OUT/statistics-zoom.png"
"$SIM" "$MAP" --boot --routes-dir "$ROUTES" --script "p p p p b h p d d d p d d w" --gpx "$GPX" --at 30 --expect-screen Statistics --png "$OUT/statistics-pan-zoomed.png"
# The live BLE-sensor stat tiles (epic #707, SE5): the Statistics grid pinned to HR / PWR / RPM (the
# three new single-column raw-int tiles) alongside a couple of live neighbours. `--sensors demo` seeds
# that grid and feeds a fixed synthetic HR/power/cadence through SE2's HAL traits for one tick, so the
# tiles read live values (152 bpm / 210 W / 88 rpm) rather than `--`. A minimal stub until SE8 wires
# the sim control-panel sliders; this frame pins the new tiles' captions + value formatting.
"$SIM" "$MAP" --boot --routes-dir "$ROUTES" --script "p p p p b" --gpx "$GPX" --at 30 --sensors demo --expect-screen Statistics --png "$OUT/statistics-sensors.png"
# The EL9 time tiles (#1077): TIME TO GO (`h TO GO`) and ETA on the Statistics grid, beside the
# DIST TO GO / TO CLIMB pair they are derived from, at a pinned 14:40 wall clock. The A/B is the
# point — the two frames ride the *same* replay over the *same* 19 km of geometry, and differ only
# in whether the loaded route carries elevation:
#   * `-grimsel`: the real climb route — 18.6 km and 1083 m still to go → 1:19, arriving 16:00;
#   * `-flat`:    its zero-elevation twin — the same 18.6 km, 0 m → 0:50, arriving 15:31.
# The 29-minute gap is the model's climb term (1083 m × 1.6 s/m on the Road profile); the flat frame
# is also the "no elevation" degradation, which must read as a plain distance ÷ speed answer rather
# than a `--` or a special case.
ETAFIELDS="time-to-go,eta,dist-to-go,to-climb,speed,ride-time"
"$SIM" "$MAP" --boot --routes-dir "$ETAROUTE" --clock "2025-06-29T14:40" --stat-fields "$ETAFIELDS" \
    --script "p p p p b" --gpx "$GPX" --at 30 --expect-screen Statistics --png "$OUT/statistics-eta.png"
"$SIM" "$MAP" --boot --routes-dir "$ETAFLAT"  --clock "2025-06-29T14:40" --stat-fields "$ETAFIELDS" \
    --script "p p p p b" --gpx "$GPX" --at 30 --expect-screen Statistics --png "$OUT/statistics-eta-flat.png"
# The same pair as the Route overview's EST TIME row sees them (page A of the content-paired pager,
# alongside DISTANCE): the whole-route estimate before the ride starts.
"$SIM" "$MAP" --boot --routes-dir "$ETAROUTE" --script "p p p" --expect-screen RouteOverview --png "$OUT/routeoverview-est-time.png"
"$SIM" "$MAP" --boot --routes-dir "$ETAFLAT"  --script "p p p" --expect-screen RouteOverview --png "$OUT/routeoverview-est-time-flat.png"
# The low-battery cue (issue: < 10 %): a warning-red battery glyph in the map's top-left corner.
"$SIM" "$MAP" --boot --routes-dir "$ROUTES" --clock "2025-06-29T14:40" --battery 5 --script "p p p p" --gpx "$GPX" --at 30 --expect-screen Map --png "$OUT/map-lowbatt.png"
# Rain overlay (WX10, epic #1185): the deterministic `--weather demo` scenarios over the map's own
# bbox, through the production adapter -> renderer path, on the one screen the raster belongs to --
# the WX11 rain map ("$WXRAIN" walks Home -> Menu -> Weather -> RAIN MAP). Two scenes the WX11 block
# below doesn't already cover: a frontal edge (rendered heading-up so the rotated fixed-point walk
# is pinned too) and a violet storm core (the high-coverage end; roads/route stay above the rain).
# The scattered-shower scene lives there as `weather-rainmap.png` -- same screen, same scenario --
# so it isn't shot twice here. Byte-stable: the demo bundle, the Bayer matrix and the sampler are
# all deterministic -- but the frames move whenever `RAIN_SAMPLING` does (bilinear since #1250).
WXRAIN="p d d d d w p d p"
"$SIM" "$MAP" --boot --weather demo:frontal --heading 35 --zoom 4 --weather-now 1800000000 --clock "2025-06-29T14:40" --script "$WXRAIN" --expect-screen WeatherRainMap --png "$OUT/map-rain-frontal-heading.png"
"$SIM" "$MAP" --boot --weather demo:storm --weather-now 1800000000 --clock "2025-06-29T14:40" --script "$WXRAIN" --expect-screen WeatherRainMap --png "$OUT/map-rain-storm.png"
# ...and the same bundle mounted while the rider is on the *ordinary* Map: rain-free, because the
# overlay is the rain map's declared content (`Caps::rain_overlay`), not a property of the frame.
# This is the state-leak regression surface -- it must stay a plain map however heavy the weather.
"$SIM" "$MAP" --boot --weather demo:storm --weather-now 1800000000 --clock "2025-06-29T14:40" --script "p d d d w p" --expect-screen Map --png "$OUT/map-rain-free.png"

# Weather screens (WX11, epic #1185): the production dashboard / hourly / rain-map / alert /
# settings surfaces over the deterministic demo bundles. The script prefix "p d d d d w p" walks
# Home -> Menu -> (4 steps to the Weather station, needle settled) -> dashboard; the demo clock
# anchors on the bundle's first frame (no --clock), so every derivation is byte-stable.
WXNAV="p d d d d w p"
"$SIM" "$MAP" --boot --weather demo:dry --script "$WXNAV" --expect-screen Weather --png "$OUT/weather-dash-dry.png"
"$SIM" "$MAP" --boot --weather demo:incoming --weather-now 1800001500 --script "$WXNAV" --expect-screen Weather --png "$OUT/weather-dash-rain.png"
# A *current* storm is a storm the alert engine fires on, on every host, from stage 10 (#1549):
# two classes trip, the rider dismisses both, and what is left underneath is the dashboard this
# frame has always photographed — byte-identical, with the cards the device really shows named.
"$SIM" "$MAP" --boot --weather demo:storm --script "d p f d p f $WXNAV" --expect-screen Weather --png "$OUT/weather-dash-storm.png"
# Honest states: frames outrun (stale -> WEATHER UPDATE NEEDED), a frameless hourly-only bundle,
# no store at all, and the non-blocking refresh cue over cached content.
"$SIM" "$MAP" --boot --weather demo:storm --weather-now 1800012000 --script "d p f $WXNAV" --expect-screen Weather --png "$OUT/weather-dash-stale.png"
"$SIM" "$MAP" --boot --weather demo:hourly --script "$WXNAV" --expect-screen Weather --png "$OUT/weather-dash-hourly-only.png"
"$SIM" "$MAP" --boot --script "$WXNAV" --expect-screen Weather --png "$OUT/weather-dash-nodata.png"
"$SIM" "$MAP" --boot --weather demo:incoming --weather-refreshing --script "$WXNAV" --expect-screen Weather --png "$OUT/weather-dash-refreshing.png"
# Hourly rows (no separators; icons/temp/precip/wind columns), fresh + scrolled.
"$SIM" "$MAP" --boot --weather demo:incoming --script "$WXNAV p" --expect-screen WeatherHourly --png "$OUT/weather-hourly.png"
"$SIM" "$MAP" --boot --weather demo:incoming --script "$WXNAV p d d d d d d" --expect-screen WeatherHourly --png "$OUT/weather-hourly-scrolled.png"
# Rain map: NOW frame, two time-steps ahead, the honest banners (stale / hourly-only), and the
# zoom clamp — entering from a far-out camera snaps to the product's regime floor (round 2:
# riders never see the out-of-regime state; the banner remains a defensive fallback only).
"$SIM" "$MAP" --boot --weather demo:scattered --script "$WXNAV d p" --expect-screen WeatherRainMap --png "$OUT/weather-rainmap.png"
"$SIM" "$MAP" --boot --weather demo:scattered --script "$WXNAV d p d d" --expect-screen WeatherRainMap --png "$OUT/weather-rainmap-step2.png"
"$SIM" "$MAP" --boot --weather demo:storm --weather-now 1800012000 --script "d p f $WXNAV d p" --expect-screen WeatherRainMap --png "$OUT/weather-rainmap-stale.png"
"$SIM" "$MAP" --boot --weather demo:hourly --script "$WXNAV d p" --expect-screen WeatherRainMap --png "$OUT/weather-rainmap-hourly-only.png"
"$SIM" "$MAP" --boot --weather demo:scattered --zoom 0.02 --script "$WXNAV d p" --expect-screen WeatherRainMap --png "$OUT/weather-rainmap-zoom-clamped.png"
# The alert card (locked VIEW RAIN MAP + DISMISS) and the settings refresh picker (open field).
# The pushed card is the *presentation* seam, so the bundle under it must not alert on its own:
# with the engine live on every host (#1549) a `demo:storm` bundle fires its own card and updates
# this one's minutes in place, which is the seam working, not the seam being tested. A dry bundle
# leaves `--weather-alert` the only writer — byte-identical, because the card is full-screen.
"$SIM" "$MAP" --boot --weather demo:dry --weather-alert storm:28 --expect-screen WeatherAlert --png "$OUT/weather-alert-storm.png"
"$SIM" "$MAP" --boot --weather demo:incoming --weather-now 1800001500 --weather-alert rain:34 --expect-screen WeatherAlert --png "$OUT/weather-alert-rain.png"
"$SIM" "$MAP" --boot --script "p d d d d d w p d d p" --expect-screen WeatherSettings --png "$OUT/weather-settings.png"
# WX12 (#1197): the two-hour *ride* decision + engine-fired alerts. `stormahead`/`rainahead` are
# stationary rings around the grid centre: parked at the centre the dashboard honestly reads DRY,
# while `--weather-decide` samples the bundle **route-projected** (the app's own matched progress +
# recent pace) and runs the production alert engine on the final frame — the ride crosses the ring.
# ETAROUTE holds just the Grimsel route, so `p p p p` rides it and the replay locks the matcher;
# WXRIDE then walks ride menu -> Main menu -> Weather station -> dashboard.
WXRIDE="p p p p B u p d d d d w p"
"$SIM" "$MAP" --boot --weather demo:stormahead --script "$WXNAV" --expect-screen Weather --png "$OUT/weather-dash-parked-dry.png"
"$SIM" "$MAP" --boot --routes-dir "$ETAROUTE" --gpx "$GPX" --at 1500 --weather demo:rainahead --weather-decide \
    --script "$WXRIDE" --expect-screen Weather --png "$OUT/weather-dash-ride-rain-ahead.png"
"$SIM" "$MAP" --boot --routes-dir "$ETAROUTE" --gpx "$GPX" --at 1500 --weather demo:stormahead --weather-decide \
    --script "$WXRIDE" --expect-screen WeatherAlert --png "$OUT/weather-alert-storm-engine.png"
# The gust card needs no walk to the dashboard any more: the engine runs at stage 10 on the very
# first settle (#1549), so the card is up before a gesture is recognised. Byte-identical — the
# card is a full-screen surface and never depended on what it covered.
"$SIM" "$MAP" --boot --weather demo:gusty --weather-decide --expect-screen WeatherAlert --png "$OUT/weather-alert-gust.png"
# Route-relative wind: the hourly rows' arrows pick up tail/cross/head ink from the ride's travel
# direction (the same replay-locked tangent), where the routeless sweep above stays neutral.
"$SIM" "$MAP" --boot --routes-dir "$ETAROUTE" --gpx "$GPX" --at 1500 --weather demo:rainahead --weather-decide \
    --script "$WXRIDE p" --expect-screen WeatherHourly --png "$OUT/weather-hourly-wind-route.png"
# Waypoint UI (epic #523). specs/vectors holds two routes in filename order: id 0 = route-plain,
# id 1 = route-waypoints ("Vector Loop": named waypoints Brunnen @ ~0 m and Pass Summit @ ~1.70 km on
# a 2.20 km track). The default `p p p p` rides id 0, so the extra `d` after the Route-menu press
# (`p p r p p`) picks id 1 — the *only* route these shots use. `--gpx $WPTGPX` is the committed replay
# that lies on that track, so the matcher locks on and progress drives the chip/tick countdowns; the
# Grimsel basemap doesn't reach 48°N, which is fine — these frames pin the waypoint chrome, not the map.
# (a) Map diamonds: at the start (--at 5 ⇒ ~30 m in) the black Brunnen diamond sits on the route by
# the marker — waypoints render as always-on ink furniture on the route line.
"$SIM" "$MAP" --boot --routes-dir "$ROUTES" --clock "2025-06-29T14:00" --script "p p d p p" --gpx "$WPTGPX" --at 5   --expect-screen Map --png "$OUT/map-waypoints.png"
# (b) The Approach chip: replayed to ~300 m short of Pass Summit (inside the 500 m approach radius),
# default `Approach` mode → the calm `◆ Pass Summit  299m` pill counts down at bottom-centre with the
# full name visible (#688 widened the name allocation), the scale bar stepped up above the chip band.
"$SIM" "$MAP" --boot --routes-dir "$ROUTES" --clock "2025-06-29T14:03" --script "p p d p p" --gpx "$WPTGPX" --at 233 --expect-screen Map --png "$OUT/map-wpt-chip.png"
# (c) Stats mid-route: the amber live-fraction progress bar carries a black tick per named waypoint
# (Brunnen at the left edge, Pass Summit at its ~0.77 fraction) with the fill sweeping between them.
"$SIM" "$MAP" --boot --routes-dir "$ROUTES" --script "p p d p p b" --gpx "$WPTGPX" --at 233 --expect-screen Statistics --png "$OUT/stats-wpt.png"
# The EL7 sweep below plans a route on the device and rides it; its own dir.
ELEVDIR="$(mktemp -d)"
trap 'rm -rf "$TRACKS" "$NAVDIR" "$TRIPDIR" "$PLAINROUTE" "$ELEVDIR" "$ETAROUTE" "$ETAFLAT"' EXIT

# --- Terrain-filled device-planned route (elevation epic #1068, EL7) -----------------------------
# The unlock, end to end: the sim mounts `grimsel.obcd` (the terrain sidecar beside the map, EL2)
# and the on-device router samples it as it emits, so a route the *device* planned now carries real
# per-point elevation — and every already-shipped elevation consumer lights up with no change of
# its own. Before EL7 all four frames below were flat: `▲0 m` in the list, an empty band, and a
# Climb screen that could not open because no climb could be detected in a zero-elevation profile.
#
# The plan: a fix at the Grimsel replay's first track point, routed to the "Handegg" Lodging POI up
# the pass road (`B d d w p` opens the POI categories, `d d p` picks Lodging, `d d d` steps to
# Handegg, `p p p` opens it → Route here → confirm, and the trailing `f` drains the request and runs
# the real A*). Starting the plan on the replay's own road is what lets the same GPX ride it below.
"$SIM" "$MAP" --boot --routes-dir "$ELEVDIR" --center 8290977,46653917 --heading 0 \
    --script "B d d w p d d p f d d d p p p f" --expect-screen RouteOverview --png "$OUT/elev-nav-overview.png"
# (a) The route list row for that saved plan: `6 km  ▲394 m` — the climb group is read straight off
# the emitted header, which the router filled from the raster.
"$SIM" "$MAP" --boot --routes-dir "$ELEVDIR" --script "p p" --expect-screen RouteMenu --png "$OUT/elev-routemenu.png"
# (b) Its overview, held long enough for the content pager to flip to page B: the elevation profile
# band with the summit label, over the CLIMB / DESCENT rows. Seven `w` settles ≈ 5.6 s, just past
# the 5 s flip.
"$SIM" "$MAP" --boot --routes-dir "$ELEVDIR" --script "p p p f w w w w w w w f" \
    --expect-screen RouteOverview --png "$OUT/elev-route-profile.png"
"$SIM" "$MAP" --boot --routes-dir "$ROUTES" --script "p p p p p" --gpx "$GPX" --at 30 --expect-screen RideControl --png "$OUT/ridecontrol.png"
# The mid-ride compass (epic #789): a BackHold on the riding Map opens the RIDE menu — Up ahead /
# Detour / POIs / Routes / Main menu — instead of the main Menu. Many scripts above *pass through* it
# (`B u p` is how they climb out to the main menu mid-ride); this is the frame of the menu itself,
# with the needle settled on its Up-ahead entry station.
"$SIM" "$MAP" --boot --routes-dir "$ROUTES" --script "p p p p B w" --gpx "$GPX" --at 30 --expect-screen RideMenu --png "$OUT/ride-menu.png"
# The Climb view (epic #506, C4/C5): the current climb's grade-striped profile + cursor + the four
# climb-scoped tiles. Reached with **no gesture at all** — `climb_mode` defaults to Auto, so riding
# into a climb replaces the riding view with this screen on the entry edge. `$ETAROUTE` holds the
# Grimsel climb alone, so `p p p p` rides it and the replay (well inside the pass road at --at 1500)
# crosses the entry the auto-switch fires on. That is also what makes this frame the C5 regression
# surface: if the auto-switch stops firing, the sweep fails here rather than quietly saving a Map.
"$SIM" "$MAP" --boot --routes-dir "$ETAROUTE" --script "p p p p" --gpx "$GPX" --at 1500 --expect-screen Climb --png "$OUT/climb.png"
# The "Up ahead" timeline (epic #946, U3) — the ride compass's north station. Needs a POI-DENSE map,
# so these frames use `monaco.obcm` (not $MAP) with the committed `monaco-upahead.gpx`: a ~2.7 km line
# across central Monaco whose 300 m corridor catches real Resupply / Pharmacy / Lodging POIs, and whose
# waypoints cover five categories, two Generic ones, and offsets on both sides of the line. The route is
# imported at run time (`--import`), so no second `.obcr` is committed to re-cut on a format bump.
# `f` draws one throwaway frame so the corridor snapshot lands before the next token.
UPMAP="$MONACO_FIXTURES/monaco.obcm"
UPGPX="$MONACO_FIXTURES/tracks/monaco-upahead.gpx"
UPROUTES="$(mktemp -d)"; trap 'rm -rf "$TRACKS" "$NAVDIR" "$TRIPDIR" "$PLAINROUTE" "$ELEVDIR" "$UPROUTES"' EXIT
"$SIM" --import "$UPGPX" --routes-dir "$UPROUTES" >/dev/null
UPBASE="p p p p T B w p f"
# (a) The merged list: map-POI rows (muted icons) and custom-waypoint rows (AMBER icon + diamond pip)
# on one along-route axis, each with distance-to-go, climb-to-go and — past 50 m — the side arrow.
"$SIM" "$UPMAP" --boot --routes-dir "$UPROUTES" --gpx "$UPGPX" --at 60 --script "$UPBASE" --expect-screen UpAhead --png "$OUT/up-ahead.png"
# (b) The same list filtered to Water, scrolled onto the custom "Fontaine du port" waypoint sitting
# between two map fountains — the source-colour + pip check the epic wanted eyeballed.
"$SIM" "$UPMAP" --boot --routes-dir "$UPROUTES" --gpx "$UPGPX" --at 60 \
    --script "$UPBASE h d p w f d d d d d d d d d" --expect-screen UpAhead --png "$OUT/up-ahead-water.png"
# (c) The Hold category picker: Everything over the six categories, all seven rows on one page.
"$SIM" "$UPMAP" --boot --routes-dir "$UPROUTES" --gpx "$UPGPX" --at 60 --script "$UPBASE h w" --expect-screen UpAhead --png "$OUT/up-ahead-picker.png"
# (d) A POI row's detail, now carrying the signed off-route offset with the side spelled out.
"$SIM" "$UPMAP" --boot --routes-dir "$UPROUTES" --gpx "$UPGPX" --at 60 \
    --script "$UPBASE d d d d d d d d d d p" --expect-screen PoiDetail --png "$OUT/up-ahead-poi-detail.png"
# (e) The empty-state trio: no route (a route-less ride), nothing ahead, and nothing of this category
# ahead (the specs/vectors plain route is far from the Monaco map, so its corridor is genuinely empty).
"$SIM" "$UPMAP" --boot --script "B d d d w p p p B w p" --expect-screen UpAhead --png "$OUT/up-ahead-noroute.png"
"$SIM" "$UPMAP" --boot --routes-dir "$PLAINROUTE" --script "$UPBASE" --expect-screen UpAhead --png "$OUT/up-ahead-nothing.png"
"$SIM" "$UPMAP" --boot --routes-dir "$PLAINROUTE" --script "$UPBASE h d p w f" --expect-screen UpAhead --png "$OUT/up-ahead-nocategory.png"
# (f) The Ride-settings **source scope** (U4). `UPSCOPE n` walks Home → Settings → Ride → the Up
# ahead row, presses it `n` times round the Both → Waypoints → Map POIs ring, and climbs back to
# Home before the ride flow starts — so the whole thing is one scripted device session, no seam.
# Waypoints-only must show no map-POI row (every row keeps its amber icon + diamond pip) and
# Map-POIs-only no waypoint row; each also pins the scope-named empty sub-line on the plain route,
# where "No stops on route" would be a lie.
UPSCOPE() { local n=$1 s="p u p p d d d d d"; for _ in $(seq 1 "$n"); do s="$s p"; done; echo "$s b b b"; }
"$SIM" "$UPMAP" --boot --routes-dir "$UPROUTES" --gpx "$UPGPX" --at 60 \
    --script "$(UPSCOPE 1) $UPBASE" --expect-screen UpAhead --png "$OUT/up-ahead-waypoints-only.png"
"$SIM" "$UPMAP" --boot --routes-dir "$UPROUTES" --gpx "$UPGPX" --at 60 \
    --script "$(UPSCOPE 2) $UPBASE" --expect-screen UpAhead --png "$OUT/up-ahead-pois-only.png"
# The two controls composing: waypoints-only + the Water filter = just the rider's own water stops.
"$SIM" "$UPMAP" --boot --routes-dir "$UPROUTES" --gpx "$UPGPX" --at 60 \
    --script "$(UPSCOPE 1) $UPBASE h d p w f" --expect-screen UpAhead --png "$OUT/up-ahead-waypoints-only-water.png"
"$SIM" "$UPMAP" --boot --routes-dir "$PLAINROUTE" --script "$(UPSCOPE 1) $UPBASE" --expect-screen UpAhead --png "$OUT/up-ahead-nothing-waypoints.png"
"$SIM" "$UPMAP" --boot --routes-dir "$PLAINROUTE" --script "$(UPSCOPE 2) $UPBASE" --expect-screen UpAhead --png "$OUT/up-ahead-nothing-pois.png"
# The `Next: <category>` stat tiles live (epic #946, U5), on the same POI-dense Monaco ride. The
# Auto climb panel would take the base screen on this line, so the script turns it Off first
# (`B u p p d d d p`), climbs back to Home, starts the ride and steps Back once to the Statistics
# view; the trailing frames let the per-category cache arm and harvest one snapshot per placed
# category (never per frame — that is the whole refresh policy). Water resolves to a *custom
# waypoint*, resupply and pharmacy to corridor POIs, so one frame pins both sources; the long names
# pin the ellipsis. NOTE the U4 source setting deliberately does not scope these tiles.
U5FIELDS="next-water,next-resupply,next-pharmacy,speed,dist-to-go"
U5CLIMBOFF="B u p p d d d p b b b"
"$SIM" "$UPMAP" --boot --routes-dir "$UPROUTES" --gpx "$UPGPX" --at 60 --stat-fields "$U5FIELDS" \
    --script "$U5CLIMBOFF p p p p b f f f f f f" --expect-screen Statistics --png "$OUT/stats-next-category.png"
# The empty state: a route-less ride, where nothing can be "ahead" — icon + the category's own word
# + `--`, at the taller tile height the chart-less grid gives.
"$SIM" "$MAP" --boot --gpx "$GPX" --at 30 --stat-fields "$U5FIELDS" \
    --script "$U5CLIMBOFF B d d d w p p p b f f" --expect-screen Statistics --png "$OUT/stats-next-category-empty.png"
# Route-less ride tracking (Menu's Map station). The Menu compass is Routes/Rides/POIs/Map/Settings,
# so the Map station is three steps down from the Routes start (`d d d w`). A live `--gpx` fix pins
# the follow camera + marker so the frames reproduce (no route → no magenta line, no off-route chip).
# (a) The route-less BROWSE map: Menu → Map (not tracking) → the follow map with clock + scale bar,
# and — new in T6 (#684) — the one-shot `Press to start a ride` hint chip (a two-line pill, since the
# sentence can't fit one line at 240 px) at the bottom on entry, the scale bar stepped above it. The
# `-settled` frame runs the browse map ~4.8 s past entry (enough `w` tokens > the 4 s window) to prove
# the hint auto-hides and the scale bar drops back to the corner. (The GPX replay runs after the
# script and drives the hint's clock not at all, so the extra `w`s are what expire it.)
"$SIM" "$MAP" --boot --clock "2025-06-29T14:40" --gpx "$GPX" --at 30 --script "B d d d w p"     --expect-screen Map --png "$OUT/map-browse.png"
"$SIM" "$MAP" --boot --clock "2025-06-29T14:40" --gpx "$GPX" --at 30 --script "B d d d w p w w w w w w" --expect-screen Map --png "$OUT/map-browse-settled.png"
# (b) The start card (browse map → press, T6 #684): the hero bike (the selected profile's sprite +
# colour) over its profile name, the two-row GPS / Battery checklist (the static Card row dropped
# in owner review round 1), then Start ride / Back. `--battery 45` pins the % and the `--gpx --at
# 30` fix makes GPS read `fix`; the second frame drops the `--gpx` (no fix) so GPS reads
# `searching..` (and a low --battery to vary the row).
"$SIM" "$MAP" --boot --clock "2025-06-29T14:40" --gpx "$GPX" --at 30 --battery 45 --script "B d d d w p p"   --expect-screen RideStart --png "$OUT/ride-start.png"
"$SIM" "$MAP" --boot --clock "2025-06-29T14:40" --battery 8 --script "B d d d w p p"   --expect-screen RideStart --png "$OUT/ride-start-nofix.png"
# (c) A route-less RIDING map (start card → Start ride): the follow map with the recorded breadcrumb,
# no route line and no off-route chip (there's no route to be off).
"$SIM" "$MAP" --boot --clock "2025-06-29T14:40" --gpx "$GPX" --at 30 --script "B d d d w p p p" --expect-screen Map --png "$OUT/map-routeless.png"
# (d) The route-less Statistics page: the "No route loaded" band note over the stat grid, where the
# route-relative tiles (KM TO GO, TO CLIMB) read "--" and the rest are live.
"$SIM" "$MAP" --boot --clock "2025-06-29T14:40" --gpx "$GPX" --at 30 --script "B d d d w p p p b" --expect-screen Statistics --png "$OUT/statistics-routeless.png"
# The mid-ride "ROUTE ACTIVE" swap card: riding route 0, out to the ride menu's Routes station
# (`B d d d` — the fourth station along, epic #789) and press, then pick the *other* vector route
# (`d p`). Choosing a route while a ride is live raises the Swap / Finish & new / Cancel card
# instead of opening the overview.
"$SIM" "$MAP" --boot --routes-dir "$ROUTES" --script "p p p p B d d d p d p" --expect-screen RouteSwap --png "$OUT/routeswap.png"
# Inspect mode: a thin rounded amber/ink frame follows the panel corners across Route, Free, and
# Zoom; only the active action's edge cues and the bottom-left scale bar join it. The clock and
# redundant labels stay out. A final `w` lets the entry hold's edge bulge retract before capture.
"$SIM" "$MAP" --boot --routes-dir "$ROUTES" --clock "2025-06-29T14:40" --script "p p p p h w" --expect-screen Map --png "$OUT/map-pan.png"
"$SIM" "$MAP" --boot --routes-dir "$ROUTES" --clock "2025-06-29T14:40" --script "p p p p h p w" --expect-screen Map --png "$OUT/map-pan-zoom.png"
# Back-hold changes Route/Free; Select-hold changes the axis only after Free is active.
"$SIM" "$MAP" --boot --routes-dir "$ROUTES" --clock "2025-06-29T14:40" --script "p p p p h B w" --expect-screen Map --png "$OUT/map-pan-free-v.png"
"$SIM" "$MAP" --boot --routes-dir "$ROUTES" --clock "2025-06-29T14:40" --script "p p p p h B h w" --expect-screen Map --png "$OUT/map-pan-free-h.png"
# BLE connected indicator (#448): the static Bluetooth rune on the Home battery row and the menu
# title bar. `--ble connected` injects a linked phone, exactly as the sim control-panel toggle does.
"$SIM" "$MAP" --boot --ble connected --clock "2025-07-10T09:41" --expect-screen Home --png "$OUT/home-ble.png" --battery 45
"$SIM" "$MAP" --boot --ble connected --battery 100 --script "B w" --expect-screen Menu --png "$OUT/menu-ble.png"
# BLE passkey card (#449): the host-pushed 6-digit LESC pairing code, rendered huge — plain
# `000042` (ungrouped, owner review round 1) under the device<->phone pair glyph (#679).
# `--ble passkey=N` injects the passkey exactly as the sim control-panel "Pairing" toggle does;
# the card auto-opens.
"$SIM" "$MAP" --boot --ble passkey=42 --expect-screen Passkey --png "$OUT/passkey-card.png"
# Route-upload popups (#451), all three variants. `--inject upload[-replace]=ID` raises the upload
# event after the script, exactly as the control panel's inject buttons do. specs/vectors holds
# two routes: id 0 = route-plain, id 1 = route-waypoints (filename order).
# Idle: "ROUTE RECEIVED" — a stats line, a mini elevation sparkline (route 0 has elevation), and
# View route / Dismiss (#682).
"$SIM" "$MAP" --boot --routes-dir "$ROUTES" --inject upload=0 --expect-screen RouteReceived --png "$OUT/route-received.png"
# Tracking (riding id 0, id 1 arrives): the retitled Route-swap popup.
"$SIM" "$MAP" --boot --routes-dir "$ROUTES" --script "p p p p" --inject upload=1 --expect-screen RouteSwap --png "$OUT/routeswap-received.png"
# Active route replaced (riding id 0, id 0 re-uploaded): the info-only "ROUTE UPDATED" card, with
# the shared check in the glyph slot (#679).
"$SIM" "$MAP" --boot --routes-dir "$ROUTES" --script "p p p p" --inject upload-replace=0 --expect-screen RouteUpdated --png "$OUT/route-updated.png"
# The trip-upload popup (epic #526): a committed trip always lands *after* its member routes, so one
# "TRIP RECEIVED" card replaces the burst's last per-route popup. `--routes-dir $TRIPDIR` is the
# already-rescanned store the event's id resolves against (`TP1.OBT` → id 1, "Alpen Traverse").
"$SIM" "$MAP" --boot --routes-dir "$TRIPDIR" --inject trip-upload=1 --expect-screen TripReceived --png "$OUT/trip-received.png"
# The map-transfer card (issue #927) — the only thing on glass through a multi-minute SD write, fed
# through the same level-style seam the board's ride loop polls. Two grades: receiving (modal, the
# progress bar mid-write) and the terminal installed card (dismissable, "restart to use it").
# Figures are kibibytes, so 120000/400000 KiB is the ~30 % point of a 390 MB map.
"$SIM" "$MAP" --boot --inject map-transfer=receiving:120000/400000 --expect-screen MapTransfer --png "$OUT/map-transfer-receiving.png"
"$SIM" "$MAP" --boot --inject map-transfer=installed --expect-screen MapTransfer --png "$OUT/map-transfer-installed.png"
# …and each failure face, the dfu-error family's grammar applied to the map: one card per sentence
# the rider can act on. `refused` is the volume-set case (#1044) — the one announce-time refusal
# that reaches the glass, because it lands on top of a stale "Map installed".
"$SIM" "$MAP" --boot --inject map-transfer=failed:storage --expect-screen MapTransfer --png "$OUT/map-transfer-failed-storage.png"
"$SIM" "$MAP" --boot --inject map-transfer=failed:damaged --expect-screen MapTransfer --png "$OUT/map-transfer-failed-damaged.png"
"$SIM" "$MAP" --boot --inject map-transfer=failed:notamap --expect-screen MapTransfer --png "$OUT/map-transfer-failed-notamap.png"
"$SIM" "$MAP" --boot --inject map-transfer=failed:refused --expect-screen MapTransfer --png "$OUT/map-transfer-failed-refused.png"
# Storage/sensor warnings (issue #504). The dismissable warning card is raised through the real
# notify_warning seam: one missing sensor, and
# the coalesced worst case (all three sensors absent + a slow/fragmented map) — the widest layout
# for the #679 glyph-slot triangle + per-sensor leading glyphs, pinning that nothing collides.
"$SIM" "$MAP" --boot --inject warning=gps --expect-screen Warning --png "$OUT/warning-gps.png"
"$SIM" "$MAP" --boot --inject warning=gps,altimeter,compass,map --expect-screen Warning --png "$OUT/warning-all.png"

# The idle timeout works end-to-end: sit in the Settings list, elapse (`I`), land back on Home. (The
# picker that configures it is shot with the rest of the Display page, up in the Settings block.)
"$SIM" "$MAP" --boot --script "B u p I"               --expect-screen Home --png "$OUT/idle-return-home.png"

# The universal quick drawer (#1515 D2): the Up+Select squeeze (`Q`) over the **riding Map**, which
# is the base worth judging — the sheet's contrast, the four unlabelled icons, and the device-64 dim
# LUT recessing a real map rather than a flat menu. Five states: the icon row, the row with the BLE
# radio switched off, the nested brightness editor, the guarded power confirmation, and that
# confirmation with the hold part-way through (`H`).
QUICK=(--routes-dir "$ROUTES" --clock "2025-06-29T14:40" --gpx "$GPX" --at 30)
"$SIM" "$MAP" --boot "${QUICK[@]}" --script "p p p p Q"           --expect-screen QuickDrawer --png "$OUT/quick-root.png"
"$SIM" "$MAP" --boot "${QUICK[@]}" --script "p p p p Q d p w"     --expect-screen QuickDrawer --png "$OUT/quick-ble-off.png"
"$SIM" "$MAP" --boot "${QUICK[@]}" --script "p p p p Q p w"       --expect-screen QuickDrawer --png "$OUT/quick-brightness.png"
"$SIM" "$MAP" --boot "${QUICK[@]}" --script "p p p p Q d d d p w" --expect-screen QuickDrawer --png "$OUT/quick-power-confirm.png"
"$SIM" "$MAP" --boot "${QUICK[@]}" --script "p p p p Q d d d p w H" --expect-screen QuickDrawer --png "$OUT/quick-power-hold.png"
# The **other** root row: a platform whose panel has no controllable light offers three controls,
# not four, and opens on the radio instead of on brightness. That is the shipping board today (no
# light line exists on it — see `PanelBacklight`), so this frame is the arrangement a rider actually
# gets on hardware. English only: it is an arrangement, and the copy is already swept in four
# languages above.
"$SIM" "$MAP" --boot "${QUICK[@]}" --no-backlight --script "p p p p Q" --expect-screen QuickDrawer --png "$OUT/quick-root-no-backlight.png"

# Per-language sweep (epic #602, L5). The i18n catalog (obc-app/i18n/*.toml -> Msg/TABLE) renders
# every screen in the runtime Language setting; `--lang de|fr|es` seeds it into the headless
# Settings (English is the default the sweep above already captures, so it isn't re-shot). Re-render
# the text-heaviest representative slice — Menu, the Settings list + a few value screens
# (Units, Ride settings, Date & Time), Statistics, the off-route Map (warning chip + scale bar), and
# the Route overview — in each of de/fr/es. (The Climb screen is *not* in this slice: it is drawn
# almost entirely from numbers and a grade-striped band, so it has nearly no copy to eyeball. Its
# English frame is `climb.png`.) These are the shots to eyeball for a stray `?` (a char outside the
# Latin font's #601 repertoire, caught deterministically by `obc-app`'s i18n repertoire test) and for
# clipped / overflowing rows now that the copy is longer. Scripts mirror the English lines above.
for lang in de fr es; do
    "$SIM" "$MAP" --boot --lang "$lang" --script "B w"           --expect-screen Menu --png "$OUT/menu-$lang.png"
    "$SIM" "$MAP" --boot --lang "$lang" --script "B u p w"       --expect-screen Settings --png "$OUT/settings-$lang.png"
    # The Ride group per-language — the longest settings screen there is: seven two-line rows, each
    # with a right-aligned value on the sub-caption line. Eyeball every label/sub pair against its
    # ◄value group (the clearance `cycle_row_value_clears_the_sub_caption` pins numerically).
    "$SIM" "$MAP" --boot --lang "$lang" --script "B u p p w"     --expect-screen Ride --png "$OUT/ride-settings-$lang.png"
    # The Auto-delete row (epic #638 S5) per-language — eyeball the retention value words
    # (Never / 1 day / 1 week / 1 month) for clipping in the longer translations.
    "$SIM" "$MAP" --boot --lang "$lang" --script "B u p p d d d d d d" --expect-screen Ride --png "$OUT/settings-ride-autodelete-$lang.png"
    "$SIM" "$MAP" --boot --lang "$lang" --script "B u p d d d d d p p"   --expect-screen Units --png "$OUT/units-$lang.png"
    # The `Next: <category>` tiles + their picker rows per language (epic #946, U5): the longest
    # category words (de `Campingplatz` / `Fahrradladen`, fr `Hébergement`) are what the tile caption
    # and the icon-gutter picker row have to fit whole.
    "$SIM" "$MAP" --boot --lang "$lang" --stat-fields "next-campsite,next-lodging,next-bike-shop" \
        --script "B u p p d p" --expect-screen StatFields --png "$OUT/fields-next-category-$lang.png"
    "$SIM" "$MAP" --boot --lang "$lang" --script "B u p p d p d d d d d d p d d d d d d d d d d" \
        --expect-screen AddField --png "$OUT/addfield-next-category-$lang.png"
    # Date & Time is the tightest screen per-language: the localized month name fills the fixed
    # month stepper cell (#614 widened it to 70 px for the four-char French months). Eyeball the
    # month glyphs against the active cell's amber border.
    "$SIM" "$MAP" --boot --lang "$lang" --script "B u p d d d d d p d p" --expect-screen DateTime --png "$OUT/datetime-$lang.png"
    "$SIM" "$MAP" --boot --lang "$lang" --routes-dir "$ROUTES" --clock "2025-06-29T14:40" --gpx "$GPX" --at 30 \
        --script "p p p p b"    --expect-screen Statistics --png "$OUT/statistics-$lang.png"
    "$SIM" "$MAP" --boot --lang "$lang" --routes-dir "$ROUTES" --clock "2025-06-29T14:40" --gpx "$GPX" --at 30 \
        --script "p p p p"      --expect-screen Map --png "$OUT/map-$lang.png"
    "$SIM" "$MAP" --boot --lang "$lang" --routes-dir "$ROUTES" --script "p p p" --expect-screen RouteOverview --png "$OUT/routeoverview-$lang.png"
    # The Route overview's Auto-delete expiry row per-language (epic #638 S5) — a ≤5-day heads-up;
    # eyeball the label ("Auto-Lösch" / "Suppr. auto" / "Autoborrado") beside the ink "in 5 d".
    "$SIM" "$MAP" --boot --lang "$lang" --routes-dir "$ROUTES" --clock "2025-07-10T09:41" --route-retention 2:2d \
        --script "p p p" --expect-screen RouteOverview --png "$OUT/routeoverview-expiry-$lang.png"
    # The trip cascade-delete confirm (epic #526, TR3), per-language — the wrapped warning line + the
    # shortened "Delete all" button are the copy to eyeball for clipping in the longer translations.
    "$SIM" "$MAP" --boot --lang "$lang" --routes-dir "$TRIPDIR" --script "p p h" --expect-screen TripDelete --png "$OUT/trip-delete-confirm-$lang.png"
    # The received-route card family (#682): the idle card's View route / Dismiss rows, and the
    # mid-ride swap + ROUTE ACTIVE cards' Swap / Finish & new / Cancel rows — eyeball each for a
    # clipped option row now that the copy is per-language.
    "$SIM" "$MAP" --boot --lang "$lang" --routes-dir "$ROUTES" --inject upload=0 --expect-screen RouteReceived --png "$OUT/route-received-$lang.png"
    "$SIM" "$MAP" --boot --lang "$lang" --routes-dir "$ROUTES" --script "p p p p" --inject upload=1 \
        --expect-screen RouteSwap --png "$OUT/routeswap-received-$lang.png"
    "$SIM" "$MAP" --boot --lang "$lang" --routes-dir "$ROUTES" --script "p p p p B d d d p d p" --expect-screen RouteSwap --png "$OUT/routeswap-$lang.png"
    # The Sensors screen (epic #707, SE7): the three kind rows + status lines, per-language — eyeball
    # for a clipped kind label ("Herzfrequenz" / "Fréq. cardiaque" / "Frec. cardíaca") or status line.
    "$SIM" "$MAP" --boot --lang "$lang" --sensors screen --script "B u p d d d p d p" --expect-screen Sensors --png "$OUT/sensors-$lang.png"
    # The ride-start card (T6 #684): the checklist labels/values (GPS/Battery) are the copy to
    # eyeball for clipped rows in the longer translations. --battery 100 pins the widest % value.
    "$SIM" "$MAP" --boot --lang "$lang" --battery 100 --script "B d d d w p p" --expect-screen RideStart --png "$OUT/ride-start-$lang.png"
    # The browse-map start hint chip (T6 #684): the two-line pill in each language, to eyeball for a
    # clipped line now that the copy is longer.
    "$SIM" "$MAP" --boot --lang "$lang" --clock "2025-06-29T14:40" --gpx "$GPX" --at 30 \
        --script "B d d d w p" --expect-screen Map --png "$OUT/map-browse-$lang.png"
    # The SD-sideload update flow (epic #615 S5): the System menu, the Firmware page (whose "Install
    # update from card" label wraps to three Label lines in the longer translations), the
    # first-install confirm (the worst case for vertical fit — the two-row version table + the
    # no-undo note, which wraps to two Label lines), the progress spinner, an error card, and the
    # post-update toast — the text-heaviest DFU screens, to eyeball for clipped/overflowing copy.
    "$SIM" "$MAP" --boot --lang "$lang" --script "B u p d d d d d p"         --expect-screen System --png "$OUT/system-$lang.png"
    "$SIM" "$MAP" --boot --lang "$lang" --script "B u p d d d d d p d d d p" --expect-screen Firmware --png "$OUT/firmware-$lang.png"
    "$SIM" "$MAP" --boot --lang "$lang" --script "$DFU_PRE" --dfu scan=first --expect-screen DfuConfirm --png "$OUT/dfu-confirm-$lang.png"
    "$SIM" "$MAP" --boot --lang "$lang" --script "$DFU_PRE" --dfu progress=normal --expect-screen DfuProgress --png "$OUT/dfu-progress-$lang.png"
    # The terminal installing card per-language — the wrapped Body headline (two lines in French)
    # + the Label body + the warning line, to eyeball for clipped copy.
    "$SIM" "$MAP" --boot --lang "$lang" --script "$DFU_PRE" --dfu installing=normal --expect-screen DfuInstalling --png "$OUT/dfu-installing-$lang.png"
    "$SIM" "$MAP" --boot --lang "$lang" --script "$DFU_PRE" --dfu error=fragmented --expect-screen DfuError --png "$OUT/dfu-error-$lang.png"
    "$SIM" "$MAP" --boot --lang "$lang" --dfu confirmed=v1.0.0-14-g0a1b2c3-dirty --expect-screen DfuUpdated --png "$OUT/dfu-updated-$lang.png"
  # Weather screens (WX11): the text-heavy surfaces re-shot per language.
  WXNAV="p d d d d w p"
  "$SIM" "$MAP" --boot --weather demo:incoming --weather-now 1800001500 --lang "$lang" --script "$WXNAV" --expect-screen Weather --png "$OUT/weather-dash-rain-$lang.png"
  "$SIM" "$MAP" --boot --weather demo:storm --weather-now 1800012000 --lang "$lang" --script "d p f $WXNAV" --expect-screen Weather --png "$OUT/weather-dash-stale-$lang.png"
  "$SIM" "$MAP" --boot --weather demo:incoming --lang "$lang" --script "$WXNAV p" --expect-screen WeatherHourly --png "$OUT/weather-hourly-$lang.png"
  "$SIM" "$MAP" --boot --weather demo:dry --lang "$lang" --weather-alert storm:28 --expect-screen WeatherAlert --png "$OUT/weather-alert-storm-$lang.png"
  # WX12: the new STRONG WIND card copy, per language.
  "$SIM" "$MAP" --boot --weather demo:gusty --lang "$lang" --weather-alert gust:0 --expect-screen WeatherAlert --png "$OUT/weather-alert-gust-$lang.png"
  "$SIM" "$MAP" --boot --lang "$lang" --script "p d d d d d w p d d p" --expect-screen WeatherSettings --png "$OUT/weather-settings-$lang.png"
  # The quick drawer's five states per language (#1515 D2) — the copy to eyeball is the caption
  # under the icon row, the brightness editor's title, and the two lines of the power confirmation,
  # each of which has to fit the sheet's width in the longer translations.
  "$SIM" "$MAP" --boot --lang "$lang" "${QUICK[@]}" --script "p p p p Q"           --expect-screen QuickDrawer --png "$OUT/quick-root-$lang.png"
  "$SIM" "$MAP" --boot --lang "$lang" "${QUICK[@]}" --script "p p p p Q d p w"     --expect-screen QuickDrawer --png "$OUT/quick-ble-off-$lang.png"
  "$SIM" "$MAP" --boot --lang "$lang" "${QUICK[@]}" --script "p p p p Q p w"       --expect-screen QuickDrawer --png "$OUT/quick-brightness-$lang.png"
  "$SIM" "$MAP" --boot --lang "$lang" "${QUICK[@]}" --script "p p p p Q d d d p w" --expect-screen QuickDrawer --png "$OUT/quick-power-confirm-$lang.png"
  "$SIM" "$MAP" --boot --lang "$lang" "${QUICK[@]}" --script "p p p p Q d d d p w H" --expect-screen QuickDrawer --png "$OUT/quick-power-hold-$lang.png"

done

# Counted from the directory rather than hand-maintained — the literal that used to live here had
# drifted 37 frames behind the script.
echo "ui-snapshots: $(ls "$OUT"/*.png | wc -l | tr -d ' ') screens rendered into $OUT/"
