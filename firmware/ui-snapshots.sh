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
#      station and `B u p d d d d p` quietly snapshots a different screen under the old
#      filename. Stating it turns that into a failed sweep. Add the flag to every new
#      command; if you don't know the name, guess, and the error message names the
#      screen the script actually reached.
#   2. Its output is digested in `firmware/ui-snapshots.sha256` — one row per PNG.
#      After a sweep: `python3 firmware/tools/ui_snapshot_manifest.py check
#      firmware/ui-snapshots.sha256 "$OUT"`. A change of pixels is intentional or it is a
#      regression; look at the changed frames first, then record them with `update`.
#
# The sweep covers the screens reachable through simulator fixtures. The persisted Assistant
# journey also opens ride recovery after restart; Continue reveals the Journey resume card.
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
SIM="${SIM:-$repo_root/target/release/obc-sim}"
python3 "$repo_root/tools/fixtures.py" sync sim sim-assistant-west-cork
fixture_root="$(python3 "$repo_root/tools/fixtures.py" root)"
GRIMSEL_FIXTURES="$fixture_root/sim-grimsel"
MONACO_FIXTURES="$fixture_root/sim-monaco"
CORK="$fixture_root/sim-assistant-west-cork/west-cork.obcm"
MAP="${MAP:-$GRIMSEL_FIXTURES/grimsel.obcm}"
GPX="${GPX:-$GRIMSEL_FIXTURES/tracks/grimsel-climb.gpx}"
# A second, tiny replay that lies *on* specs/vectors' `route-waypoints.obcr` ("Vector Loop") — the
# Grimsel climb GPX above is far off it, so it can't drive the waypoint chip/ticks. Synthetic + its
# provenance are pinned in the assets README; it stops ~300 m short of the "Pass Summit" waypoint.
WPTGPX="$repo_root/fixtures/sources/vector/vector-loop-replay.gpx"
# Stage the two UI routes; the vector directory also contains deliberately invalid inputs.
ROUTES="$(mktemp -d)"
cp "$repo_root/specs/vectors/route-plain.obcr" "$repo_root/specs/vectors/route-waypoints.obcr" "$ROUTES/"
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
# An empty import directory for create-route sessions. Generated routes stay on each session's card.
NAVDIR="$(mktemp -d)"
JOURNEYDIR="$(mktemp -d)"
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
trap 'rm -rf "$ROUTES" "$TRACKS" "$NAVDIR" "$JOURNEYDIR" "$TRIPDIR" "$PLAINROUTE" "$ETAROUTE" "$ETAFLAT"' EXIT
cp "$GRIMSEL_FIXTURES/routes/grimsel-climb.obcr" "$ETAROUTE/"
sed 's#<ele>[^<]*</ele>#<ele>0</ele>#g' "$GPX" > "$ETAFLAT/grimsel-flat.gpx"
"$SIM" --import "$ETAFLAT/grimsel-flat.gpx" --routes-dir "$ETAFLAT" > /dev/null
rm "$ETAFLAT/grimsel-flat.gpx"
cp "$repo_root/specs/vectors/ride-v3.bin" "$TRACKS/ride-0.obcr"
cp "$repo_root/specs/vectors/ride-v3.bin" "$TRACKS/ride-1.obcr"
printf '\x88\x45\x00\x00' | dd of="$TRACKS/ride-1.obcr" bs=1 seek=72 conv=notrunc status=none
cp "$ROUTES/route-plain.obcr"     "$TRIPDIR/1-plain.obcr"
cp "$ROUTES/route-waypoints.obcr" "$TRIPDIR/2-waypoints.obcr"
cp "$ROUTES/route-plain.obcr" "$PLAINROUTE/"
cp "$GRIMSEL_FIXTURES/routes/grimsel-climb.obcr" "$TRIPDIR/3-grimsel.obcr"
cp "$GRIMSEL_FIXTURES/routes/TP1.OBT" "$TRIPDIR/TP1.OBT"

# Menu navigation: Home's press (and back-hold) opens the compass Menu — the single door into the
# app — so the Route menu is now `p p` from boot (open Menu, then press the Routes station, which the
# menu starts on). The compass menu is Routes / Rides / Map / Peaks / Settings, so Settings is one Up
# step (`u`, wrapping) from the Routes start, Rides is one down (`d`), Map two down (`d d`). `w`
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
# Peak View's geographic fixture: Live follows the preset's stopped-compass heading;
# Select freezes that panorama and selects its most prominent visible summit.
# Geographic fixture files are independent from MAP. `f` finishes the full panorama before capture or Browse input.
"$SIM" "$MAP" --boot --peak-view gornergrat --script "B d d d p f" --expect-screen PeakView --png "$OUT/peak-view.png"
"$SIM" "$MAP" --boot --peak-view scheidegg --script "B d d d p f" --expect-screen PeakView --png "$OUT/peak-view-scheidegg.png"
"$SIM" "$MAP" --boot --peak-view glockner --script "B d d d p f" --expect-screen PeakView --png "$OUT/peak-view-glockner.png"
"$SIM" "$MAP" --boot --peak-view gornergrat --script "B d d d p f p" --expect-screen PeakView --png "$OUT/peak-view-browse.png"
# A heading outside the initial west-facing crop proves that Live mode discovers a different set
# of named ridge summits from the fixture's full-circle peak catalog.
"$SIM" "$MAP" --boot --peak-view gornergrat --heading 90 --script "B d d d p f" --expect-screen PeakView --png "$OUT/peak-view-heading-east.png"
# Up enters Browse from the right edge and steps left through the ridge candidates.
"$SIM" "$MAP" --boot --peak-view gornergrat --heading 90 --script "B d d d p f u u u" --expect-screen PeakView --png "$OUT/peak-view-inner-ridge.png"
# Down enters Browse from the left edge and steps right. Generic labels stay in place.
"$SIM" "$MAP" --boot --peak-view gornergrat --script "B d d d p f d d" --expect-screen PeakView --png "$OUT/peak-view-matterhorn.png"
# Installed summit identity, shared reading/photo/Sources, and restored Browse panorama.
PEAK_MAP="$repo_root/apps/obc-sim/assets/grimsel-demo.obcm"
"$SIM" "$PEAK_MAP" --center 7961000,46585000 --heading 141.25 --script "B d d d p f p f" --expect-screen PeakView --png "$OUT/peak-article-indicator.png"
"$SIM" "$PEAK_MAP" --center 7961000,46585000 --heading 141.25 --script "B d d d p f p f p f" --expect-screen PeakArticle --png "$OUT/peak-article.png"
"$SIM" "$PEAK_MAP" --center 7961000,46585000 --heading 141.25 --script "B d d d p f p f p f u f" --expect-screen LandmarkPhoto --png "$OUT/peak-photo.png"
"$SIM" "$PEAK_MAP" --center 7961000,46585000 --heading 141.25 --script "B d d d p f p f p f u f C p f" --expect-screen LandmarkSources --png "$OUT/peak-sources.png"
"$SIM" "$PEAK_MAP" --center 7961000,46585000 --heading 141.25 --script "B d d d p f p f p f u f b b f" --expect-screen PeakView --png "$OUT/peak-article-back.png"

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
# out to the main Menu — a bare `B` reaches it from anywhere since #1515 D3 — step to the Rides
# station (`d w`), press into the detail: the page ends at the stat ledger with NO Delete-ride row
# at all, and `H` fills nothing.
"$SIM" "$MAP" --boot --routes-dir "$ROUTES" --tracks-dir "$TRACKS" --gpx "$GPX" --at 30 --script "p p p p B d w p p H" --expect-screen RideDetail --png "$OUT/ride-detail-recording.png"
"$SIM" "$MAP" --boot --script "B d d w"      --expect-screen Menu --png "$OUT/menu-pois.png"
# POIs browser (#425): the category list, then a populated nearest-16 list. The list's bearing
# arrows are live, so pin a deterministic fix (grimsel map centre) + heading so they reproduce.
"$SIM" "$MAP" --boot --script "A"    --expect-screen Assistant --png "$OUT/assistant.png"
"$SIM" "$MAP" --boot --center 8305000,46601000 --heading 0 --script "A p p u p f" --expect-screen PoiList --png "$OUT/poi-list.png"
# POI detail (#444, reworked in #685): category glyph on the name row, the promoted distance +
# bearing row, the hours block with the OPEN/CLOSED pill riding the "Today" caption line
# (right-aligned — owner review round 2's overlay fix), and the full-width "Route here" footer
# bar. The hours/badge need the hours-rich monaco fixture (grimsel has no shop hours). Pin the
# Resupply "Carrefour" supermarket (--center on it → row 0), a fix + heading for the live arrow,
# and a deterministic --clock (Mon 2025-01-06 12:00 → OPEN). `p d p` presses into the list, draws
# once to fill the lazy snapshot, then presses the POI into its detail.
MONACO="$MONACO_FIXTURES/monaco.obcm"
"$SIM" "$MONACO" --boot --center 7416969,43730798 --heading 0 --clock "2025-01-06T12:00" \
    --script "A p d d d p u p f p f" --expect-screen PoiDetail --png "$OUT/poi-detail.png"
# Select Carrefour while open, then advance the trusted clock past its 21:00 closing time.
# Closed places are excluded from a new nearby query; an already-open detail must update in place.
"$SIM" "$MONACO" --boot --center 7416969,43730798 --heading 0 --clock "2025-01-06T12:00" \
    --clock-after-script "2025-01-06T23:00" --script "A p d d d p u p f p f" \
    --expect-screen PoiDetail --png "$OUT/poi-detail-closed.png"
# The layout worst case (owner review round 2's overlay bug): a two-line wrapping name
# ("Pharmacie du Jardin Exot..") + the format's two-intervals-per-day maximum (split lunch hours,
# Mon 08:30-12:30 / 15:00-19:00) — the stack that used to push the badge under the Route-here
# bar. With the badge on the Today line the whole block clears the footer. Pharmacy is one more
# step into the category list than Resupply.
"$SIM" "$MONACO" --boot --center 7413793,43734832 --heading 0 --clock "2025-01-06T12:00" \
    --script "A p d d d d p u p f p f" --expect-screen PoiDetail --png "$OUT/poi-detail-split-hours.png"
# Selected-place profile controls and real offline Visit preview share the production owner.
PLACEDETAIL="A p d d d p u p f p f"
"$SIM" "$MONACO" --boot --heading 0 --center 7416969,43730798 --clock "2025-01-06T12:00" \
    --script "$PLACEDETAIL C" --expect-screen ContextDrawer --png "$OUT/route-plan-context.png"
"$SIM" "$MONACO" --boot --heading 0 --center 7416969,43730798 --clock "2025-01-06T12:00" \
    --script "$PLACEDETAIL C p w d" --expect-screen ContextDrawer --png "$OUT/route-plan-biketype-editor.png"
LANDMARKS="A d d d d d p f"
"$SIM" "$CORK" --boot --heading 0 --center -9829419,51482665 --script "$LANDMARKS" \
    --expect-screen Landmarks --png "$OUT/landmarks.png"
"$SIM" "$CORK" --boot --heading 0 --center -9829419,51482665 --script "$LANDMARKS p f u f" \
    --expect-screen LandmarkPhoto --png "$OUT/landmark-photo.png"
"$SIM" "$CORK" --boot --heading 0 --center -9829419,51482665 --script "$LANDMARKS p f C p f" \
    --expect-screen LandmarkSources --png "$OUT/landmark-sources.png"
"$SIM" "$CORK" --boot --heading 0 --center -9825560,51485575 --script "$LANDMARKS p p f p f" \
    --expect-screen VisitReview --png "$OUT/visit-preview.png"
"$SIM" "$CORK" --boot --heading 0 --center -9825560,51485575 --script "$LANDMARKS p p f p f p f" \
    --expect-screen Map --png "$OUT/visit-accepted.png"
# Persist a real Cork destination and recording, then continue the ride to reach Resume.
"$SIM" "$CORK" --routes-dir "$NAVDIR" --create-card "$JOURNEYDIR/card.obc"
"$SIM" --card "$JOURNEYDIR/card.obc" --boot --heading 0 --center -9825560,51485575 \
    --script "$LANDMARKS p p f p f p f" --expect-screen Map --png "$JOURNEYDIR/accepted.png"
"$SIM" --card "$JOURNEYDIR/card.obc" --boot --heading 0 --center -9825560,51485575 \
    --script "f p f" --expect-screen Journey --png "$OUT/journey-resume.png"
"$SIM" "$CORK" --boot --heading 0 --center -9829419,51482665 --script "A p p f" \
    --expect-screen FindPlace --png "$OUT/find-place.png"

# Find preferences share the existing drawer from overview, recommendations, and the browser.
FIND_READY="f f f f f f f f f f f f f f f f f f f f f f f f f f f f f f f f f f f f f f f f f f f f f f f f f f f f f f f f f f f f f f f f"
"$SIM" "$CORK" --boot --script "A p C w" \
    --expect-screen ContextDrawer --png "$OUT/find-context.png"
"$SIM" "$CORK" --boot --script "A p C w p" \
    --expect-screen ContextDrawer --png "$OUT/find-context-all.png"
"$SIM" "$CORK" --boot --script "A p C w d p w d" \
    --expect-screen ContextDrawer --png "$OUT/find-results-six.png"
"$SIM" "$MONACO" --boot --center 7416969,43730798 --heading 0 --clock "2025-01-06T23:00" \
    --script "A p C w p d p w d p w b d d d p $FIND_READY" \
    --expect-screen FindPlace --png "$OUT/find-six-closed.png"

# The existing Detour planner uses the real imported Monaco loop and an actual replay fix.
"$SIM" --import "$MONACO_FIXTURES/tracks/monaco-upahead.gpx" --routes-dir "$NAVDIR" >/dev/null
DETOUR_PRE="p p p p T"
DETOUR_FIX=(--gpx "$MONACO_FIXTURES/tracks/monaco-upahead.gpx" --at 60)
"$SIM" "$MONACO" --boot --routes-dir "$NAVDIR" "${DETOUR_FIX[@]}" \
    --script "$DETOUR_PRE C" --expect-screen ContextDrawer --png "$OUT/map-context-live.png"
"$SIM" "$MONACO" --boot --routes-dir "$NAVDIR" "${DETOUR_FIX[@]}" \
    --script "$DETOUR_PRE C d p w" --expect-screen Detour --png "$OUT/detour-chooser.png"
"$SIM" "$MONACO" --boot --routes-dir "$NAVDIR" "${DETOUR_FIX[@]}" \
    --script "$DETOUR_PRE C d p w p" --hold detour --expect-screen NavPlanning --png "$OUT/detour-planning.png"
"$SIM" "$MONACO" --boot --routes-dir "$NAVDIR" "${DETOUR_FIX[@]}" \
    --script "$DETOUR_PRE C d p d d p f" --expect-screen DetourPreview --png "$OUT/detour-preview.png"
"$SIM" "$MONACO" --boot --routes-dir "$NAVDIR" "${DETOUR_FIX[@]}" \
    --script "$DETOUR_PRE C d p w p" --inject detour-fail=exhausted --expect-screen NavFail --png "$OUT/detour-fail.png"
"$SIM" "$MONACO" --boot --routes-dir "$NAVDIR" "${DETOUR_FIX[@]}" \
    --script "$DETOUR_PRE C d p d d p f p f T" --expect-screen Climb --png "$OUT/detour-committed.png"
# --- Settings ------------------------------------------------------------------------------------
# System — so every settings screen sits two levels down. The shape of every script below is:
#   B u p        open the Menu, one Up step to the Settings station, press -> the Settings list
#                5 System)
#   p            press into the group
#   d × R  [p]   step to row R inside it, and press if that row opens a page / cycles a value
"$SIM" "$MAP" --boot --script "B u p w"      --expect-screen Settings --png "$OUT/settings.png"

# Ride settings: Data fields, Pages, Climb, and Waypoints.
"$SIM" "$MAP" --boot --script "B u p p w"    --expect-screen Ride --png "$OUT/ride-settings.png"
# The Bike type row is **gone** from this group (#1515 D4d): it is the one row of the create-route
# confirm card's own context sheet now — see `route-plan-context.png` below, which is its only home.
# Its four hero-bike frames left with it, and the group's remaining rows shift up one slot.
# Data fields (row 0) — the WYSIWYG grid editor.
"$SIM" "$MAP" --boot --script "B u p p p" --expect-screen StatFields --png "$OUT/fields.png"
# The 2×3 waypoint list panel placed in the WYSIWYG field editor (epic #523): from the Fields grid,
# six steps reach the ADD ghost (the six default tiles fill page 1), press to open the picker, then
# five steps to `Waypoint list` (the last hidden non-sensor entry) and press. The page-sized panel lands on
# its own page — the `2 / 3` counter, full-width and three rows tall (`--` with no route loaded).
"$SIM" "$MAP" --boot --script "B u p p p d d d d d d p d d d d d p" --expect-screen StatFields --png "$OUT/fields-wpt-panel.png"
# The six `Next: <category>` fields (epic #946, U5). `B u p p p` is Home -> Settings -> Ride ->
# Data fields (the Fields grid). (a) the Add-field picker scrolled onto the new group: six rows
# wearing the category's own row icon in place of a span badge, directly under `Next waypoint`.
"$SIM" "$MAP" --boot --script "B u p p p d d d d d d p d d d d d" --expect-screen AddField --png "$OUT/addfield-next-category.png"
# (b) three of them placed, drawn by the WYSIWYG editor's ghost: icon + the localized category word
# + a per-category sample distance (the editor has no route, so the live cell would read `--`).
"$SIM" "$MAP" --boot --stat-fields "next-water,next-campsite,next-lodging" \
    --script "B u p p p" --expect-screen StatFields --png "$OUT/fields-next-category.png"
# The Waypoints mode row (epic #523): the group's 4th row, under Climb. Three steps park the amber
# cursor on it, showing the default `Approach` mode.
"$SIM" "$MAP" --boot --script "B u p p d d d" --expect-screen Ride --png "$OUT/settings-ride-waypoints.png"

# Display (group 1): the idle-return picker, alone. The three Map-overlay switches left this page in
# #1515 D4c — they are rows of the map's own sheet now, see `map-display-sheet.png`, their only home
# — so the picker is row 0 and one press opens it. (The old recipe walked two rows down first and
# pressed *Contours*, which is not the state this filename has always claimed.)
"$SIM" "$MAP" --boot --script "B u p d p"   --expect-screen Display --png "$OUT/display.png"
"$SIM" "$MAP" --boot --script "B u p d p p" --expect-screen Display --png "$OUT/display-idle-return.png"

# lost one `d` because the list is five rows.
#
# Connections (group 2): the two radios in one drawer — Phone (Bluetooth) then Sensors.
"$SIM" "$MAP" --boot --script "B u p d d p"  --expect-screen Connections --png "$OUT/connections.png"
# Bluetooth screen (#455, Forget restyled to the Pause-menu row family in owner review round 3):
# the main state (radio on, advertising, a stored bond -> Paired: yes, the Forget row a plain label
# at the bottom anchor), the row selected (a step puts the shaded guarded base on it), the guarded
# hold mid-charge (a partial hold fills it warning-red), and the unpaired state — no bond, so the
# Forget row isn't drawn at all (the round-1 only-when-possible grammar).
"$SIM" "$MAP" --boot --ble paired --script "B u p d d p p"     --expect-screen Bluetooth --png "$OUT/bluetooth.png"
"$SIM" "$MAP" --boot --ble paired --script "B u p d d p p d"   --expect-screen Bluetooth --png "$OUT/bluetooth-forget-selected.png"
"$SIM" "$MAP" --boot --ble paired --script "B u p d d p p d H" --expect-screen Bluetooth --png "$OUT/bluetooth-forget-hold.png"
"$SIM" "$MAP" --boot              --script "B u p d d p p"     --expect-screen Bluetooth --png "$OUT/bluetooth-unpaired.png"
# Sensors screen (BLE sensors epic #707, SE7) — the group's second row, under Phone. `--sensors screen`
# drives the sim's fake central manager: the three-row list (Heart rate Connected · 78 %, Power
# Searching, Cadence Not set — the HR row selected, so its hold-to-forget footer shows), and the scan
# list one press deeper (the HR-filtered discovered sensors, name/address + RSSI). A third run with no
# fake manager pins the empty `Searching...` state while the scan finds nothing.
"$SIM" "$MAP" --boot --sensors screen --script "B u p d d p d p"   --expect-screen Sensors --png "$OUT/sensors.png"
"$SIM" "$MAP" --boot --sensors screen --script "B u p d d p d p p" --expect-screen SensorScan --png "$OUT/sensors-scan.png"
"$SIM" "$MAP" --boot                  --script "B u p d d p d p p" --expect-screen SensorScan --png "$OUT/sensors-scanning.png"

# Power (group 3): the GPS fix-interval stepper + the power-saver toggle.
"$SIM" "$MAP" --boot --script "B u p d d d p" --expect-screen Power --png "$OUT/power.png"

# System (group 4) — the device drawer: Units, Date & Time, Language, Firmware, About, Reset. The
# menu itself first, then each row's page.
"$SIM" "$MAP" --boot --script "B u p d d d d p"     --expect-screen System --png "$OUT/system.png"
"$SIM" "$MAP" --boot --script "B u p d d d d p p"   --expect-screen Units --png "$OUT/units.png"
"$SIM" "$MAP" --boot --script "B u p d d d d p d p" --expect-screen DateTime --png "$OUT/datetime.png"
# The Language screen (epic #602): the endonym value picker. The default (English), then two
# steps cycling to Français — pinning the ç glyph the Latin font (#601) adds.
"$SIM" "$MAP" --boot --script "B u p d d d d p d d p"     --expect-screen Language --png "$OUT/language.png"
"$SIM" "$MAP" --boot --script "B u p d d d d p d d p d d" --expect-screen Language --png "$OUT/language-french.png"
# The About page (#1149) — System row 4, above Reset: the read-only credits page.
"$SIM" "$MAP" --boot --script "B u p d d d d p d d d d p" --expect-screen About --png "$OUT/about.png"
# Factory Reset is the group's last row: five steps in, press to open, arm (press), then a
# partial-hold to fill the bar. (Four steps stopped on About until this recipe was corrected.)
"$SIM" "$MAP" --boot --script "B u p d d d d p d d d d d p p H" --expect-screen Reset --png "$OUT/reset-hold.png"
# The Firmware page (epic #615 S5, #620) — System row 3, the SD-sideload door ("Install update from
# card") over the read-only device-info ledger.
"$SIM" "$MAP" --boot --script "B u p d d d d p d d d p" --expect-screen Firmware --png "$OUT/firmware.png"
# The row greyed (disabled) while a ride records: ride route 0 (`p p p p`, GPX-driven so the session
# is live), out to the Menu with the global escape (`B`), then Settings -> System -> Firmware. The
# row loses its amber box and shows the "Recording" cue.
"$SIM" "$MAP" --boot --routes-dir "$ROUTES" --tracks-dir "$TRACKS" --gpx "$GPX" --at 30 \
    --script "p p p p B u p d d d d p d d d p" --expect-screen Firmware --png "$OUT/firmware-recording.png"
# The SD-sideload update flow (epic #615 S5, #620). The scan/arm runs board-side; the script leaves
# the "Checking card..." wait on top (Firmware -> Install), and --dfu scan/error answer it
# through the real notify_dfu_scan_result seam (the sim stages a synthetic UPDATE.BIN and runs the
# real obc-dfu scan). --dfu progress then presses Install so the "Preparing update..." spinner shows.
DFU_PRE="B u p d d d d p d d d p p"
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
# Page B after the 5 s dwell (each `w` elapses ~800 ms; seven cross the flip): the elevation band
# over CLIMB + DESCENT — the same band slot, so nothing jumps.
"$SIM" "$MAP" --boot --routes-dir "$ROUTES" --script "p p p w w w w w w w" --expect-screen RouteOverview --png "$OUT/routeoverview-elevation.png"
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
trap 'rm -rf "$ROUTES" "$TRACKS" "$NAVDIR" "$JOURNEYDIR" "$TRIPDIR" "$PLAINROUTE" "$ELEVDIR" "$ETAROUTE" "$ETAFLAT"' EXIT

# --- Terrain-filled device-planned route -------------------------------------------------------
# The pinned Grimsel pack has a separate OBCT input. Stage it inside a temporary OBCM so the
# planner reads terrain through the retained map object, as it does for an assembled device map.
# Only these three frames use the staged map; registered fixture bytes remain unchanged.
ELEVMAP="$ELEVDIR/terrain.obcm"
python3 - "$MAP" "$ELEVMAP" <<'PYTERRAIN'
from pathlib import Path
import struct
import sys

source, output = map(Path, sys.argv[1:])
map_bytes = bytearray(source.read_bytes())
# OBCM §1.1/§1.3: v17 header, 16-byte units, terrain offset/length at bytes 41/45.
assert len(map_bytes) >= 65 and map_bytes[:5] == b"OBCM\x11" and map_bytes[40] == 4
terrain_offset, terrain_length = struct.unpack_from("<II", map_bytes, 41)
assert bool(terrain_offset) == bool(terrain_length), "incomplete terrain region"
if not terrain_offset:
    terrain = source.with_suffix(".obcd").read_bytes()
    assert len(terrain) >= 64 and terrain[:5] == b"OBCT\x01", "expected the pinned OBCT v1 input"
    map_bytes.extend(b"\xff" * (-len(map_bytes) % 16))
    terrain_offset = len(map_bytes) // 16
    map_bytes.extend(terrain)
    map_bytes.extend(b"\xff" * (-len(map_bytes) % 16))
    terrain_length = len(map_bytes) // 16 - terrain_offset
    struct.pack_into("<II", map_bytes, 41, terrain_offset, terrain_length)
assert (terrain_offset + terrain_length) * 16 <= len(map_bytes)
output.write_bytes(map_bytes)
PYTERRAIN

# Easier compares the actual matched route; no alternative is fabricated when none improves it.
"$SIM" "$ELEVMAP" --boot --routes-dir "$ETAROUTE" --gpx "$GPX" --at 30 \
    --script "p p p p b T A d d p f" --expect-screen Easier --png "$OUT/easier-route.png"
"$SIM" "$MAP" --boot --routes-dir "$ROUTES" --script "p p p p p" --gpx "$GPX" --at 30 --expect-screen RideControl --png "$OUT/ridecontrol.png"
# The **map context** (#1515 D3, extended by D4c): a Down+Back squeeze (`C`) on the riding Map
# raises the bottom sheet carrying the ride's secondary actions — Up ahead / Detour / POIs / Routes
# — plus the fifth row only the Map declares, Map display. It replaced the compass RIDE menu, whose
# own fifth station (Main menu) is now the global Back-hold escape. Same base as the quick-drawer
# frames — a real map under the device-64 dim LUT — so the two sheets can be judged against each
# other.
#
# Grimsel carries **no routing graph**, so this frame is also the inert-row case: Detour draws
# recessed with no chevron and a press does nothing. The drawer's dim means inert, unlike the
# compass dial's, which dimmed a station a press still opened. The all-live arrangement is
# `map-context-live.png`, shot on the Monaco graph down in the detour block.
"$SIM" "$MAP" --boot --routes-dir "$ROUTES" --script "p p p p C" --gpx "$GPX" --at 30 --expect-screen ContextDrawer --png "$OUT/map-context.png"
# The **map display sheet** (#1515 D4c): the fifth row swaps the five-row sheet for the three
# switches that are the only home the map's clock pill, scale bar and terrain layer have. `u` wraps
# the cursor to the last row; there is **no settle token after the press**, because the swap is
# instantaneous by design — a frame that needed one would be evidence the swap re-ran the open.
"$SIM" "$MAP" --boot --routes-dir "$ROUTES" --script "p p p p C u p" --gpx "$GPX" --at 30 --expect-screen ContextDrawer --png "$OUT/map-display-sheet.png"
# …and the Clock row flipped off, which is the pair that shows the switch working. The `HH:MM` pill
# is gone from the map too, and that is the **headless** path being honest rather than the frozen
# base failing: `--png` composes the whole frame from nothing, so it never claims a resident frame
# and draws the base as always. On the device the map keeps the pill until the sheet closes, which
# is the one repaint the rider pays for however many switches they flip.
"$SIM" "$MAP" --boot --routes-dir "$ROUTES" --script "p p p p C u p p" --gpx "$GPX" --at 30 --expect-screen ContextDrawer --png "$OUT/map-display-clock-off.png"
# The **ride context** the other three riding views share — the unchanged four-row table, shot over
# Statistics (`p p p p b C`), which is what keeps it covered at all now that the Map declares its own.
"$SIM" "$MAP" --boot --routes-dir "$ETAROUTE" --script "p p p p b C" --gpx "$GPX" --at 30 --expect-screen ContextDrawer --png "$OUT/ride-context.png"
# The Climb view (epic #506, C4/C5): the current climb's grade-striped profile + cursor + the four
# climb-scoped tiles. Reached with **no gesture at all** — `climb_mode` defaults to Auto, so riding
# into a climb replaces the riding view with this screen on the entry edge. `$ETAROUTE` holds the
# Grimsel climb alone, so `p p p p` rides it and the replay (well inside the pass road at --at 1500)
# crosses the entry the auto-switch fires on. That is also what makes this frame the C5 regression
# surface: if the auto-switch stops firing, the sweep fails here rather than quietly saving a Map.
"$SIM" "$MAP" --boot --routes-dir "$ETAROUTE" --script "p p p p" --gpx "$GPX" --at 1500 --expect-screen Climb --png "$OUT/climb.png"
# The "Up ahead" timeline (epic #946, U3) — the ride context's first row. Needs a POI-DENSE map,
# so these frames use `monaco.obcm` (not $MAP) with the committed `monaco-upahead.gpx`: a ~2.7 km line
# across central Monaco whose 300 m corridor catches real Resupply / Pharmacy / Lodging POIs, and whose
# waypoints cover five categories, two Generic ones, and offsets on both sides of the line. The route is
# imported at run time (`--import`), so no second `.obcr` is committed to re-cut on a format bump.
# `f` draws one throwaway frame so the corridor snapshot lands before the next token.
UPMAP="$MONACO_FIXTURES/monaco.obcm"
UPGPX="$MONACO_FIXTURES/tracks/monaco-upahead.gpx"
UPROUTES="$(mktemp -d)"; trap 'rm -rf "$ROUTES" "$TRACKS" "$NAVDIR" "$JOURNEYDIR" "$TRIPDIR" "$PLAINROUTE" "$ELEVDIR" "$UPROUTES" "$ETAROUTE" "$ETAFLAT"' EXIT
"$SIM" --import "$UPGPX" --routes-dir "$UPROUTES" >/dev/null
UPBASE="p p p p T A d p f p f f f f f f f f"
# (a) The merged list: map-POI rows (muted icons) and custom-waypoint rows (AMBER icon + diamond pip)
# on one along-route axis, each with distance-to-go, climb-to-go and — past 50 m — the side arrow.
"$SIM" "$UPMAP" --boot --routes-dir "$UPROUTES" --gpx "$UPGPX" --at 60 --script "$UPBASE" --expect-screen WhatsNext --png "$OUT/up-ahead.png"
# The timeline's own **context sheet** (#1515 D4a) and the two controls it is the only home for.
# `UPFILTER n` opens the sheet, presses its Filter row into the nested editor, stages `n` steps and
# commits, then closes the sheet — so the list comes back filtered. Every `p` that starts a page
# slide is followed by `w`, because the sheet owns its input while a slide runs.
UPFILTER() { local n=$1 s="C p w"; for _ in $(seq 1 "$n"); do s="$s d"; done; echo "$s p w b f f f f f f f f"; }
# (b) Water from authored and mapped sources. Finish the page load after advancing the list.
"$SIM" "$UPMAP" --boot --routes-dir "$UPROUTES" --gpx "$UPGPX" --at 60 \
    --script "$UPBASE $(UPFILTER 1) d d d d d d d d d f f f f f f f f" --expect-screen WhatsNext --png "$OUT/up-ahead-water.png"
# (c1) The context sheet itself: two value rows, Filter and Sources, each a door into its editor.
# This frame replaces the Hold picker's — the filter is a sheet row now, not a mode on the list.
"$SIM" "$UPMAP" --boot --routes-dir "$UPROUTES" --gpx "$UPGPX" --at 60 --script "$UPBASE C" \
    --expect-screen ContextDrawer --png "$OUT/up-ahead-context.png"
# (c2) The nested value editor, on the row the rider is browsing (Campsite) with the committed
# choice (Everything) still ticked under the first notch — the mark the grammar promises.
"$SIM" "$UPMAP" --boot --routes-dir "$UPROUTES" --gpx "$UPGPX" --at 60 --script "$UPBASE C p w d d" \
    --expect-screen ContextDrawer --png "$OUT/up-ahead-filter-editor.png"
# (c3) The Sources editor, where the choices carry no icon of their own — the other shape the one
# generic editor draws.
"$SIM" "$UPMAP" --boot --routes-dir "$UPROUTES" --gpx "$UPGPX" --at 60 --script "$UPBASE C d p w d" \
    --expect-screen ContextDrawer --png "$OUT/up-ahead-sources-editor.png"
# (d) A POI row's detail, now carrying the signed off-route offset with the side spelled out.
"$SIM" "$UPMAP" --boot --routes-dir "$UPROUTES" --gpx "$UPGPX" --at 60 \
    --script "$UPBASE C d p w d d p w b f f f f f f f f p f" --expect-screen PoiDetail --png "$OUT/up-ahead-poi-detail.png"
# (e) No-route and outside-map states. The plain vector route is outside Monaco;
# the coverage guard takes precedence over map-place filters.
"$SIM" "$UPMAP" --boot --script "A d p f p f" --expect-screen WhatsNext --png "$OUT/up-ahead-noroute.png"
"$SIM" "$UPMAP" --boot --routes-dir "$PLAINROUTE" --script "$UPBASE" --expect-screen WhatsNext --png "$OUT/up-ahead-outside-map.png"
# (f) The **source scope** (U4). Since #1515 D4a it is edited from the timeline's own sheet, not from
# Ride settings: `UPSCOPE n` opens the sheet on an already-running list, steps to the Sources row,
# presses into its editor, stages `n` steps round the Both → Waypoints → Map POIs ring, commits and
# closes. Waypoints-only must show no map-POI row (every row keeps its amber icon + diamond pip) and
# Map-POIs-only no waypoint row; each also pins the scope-named empty sub-line on the plain route,
# where "No stops on route" would be a lie.
UPSCOPE() { local n=$1 s="C d p w"; for _ in $(seq 1 "$n"); do s="$s d"; done; echo "$s p w b f f f f f f f f"; }
"$SIM" "$UPMAP" --boot --routes-dir "$UPROUTES" --gpx "$UPGPX" --at 60 \
    --script "$UPBASE $(UPSCOPE 1)" --expect-screen WhatsNext --png "$OUT/up-ahead-waypoints-only.png"
"$SIM" "$UPMAP" --boot --routes-dir "$UPROUTES" --gpx "$UPGPX" --at 60 \
    --script "$UPBASE $(UPSCOPE 2)" --expect-screen WhatsNext --png "$OUT/up-ahead-pois-only.png"
# The two controls composing: waypoints-only + the Water filter = just the rider's own water stops.
"$SIM" "$UPMAP" --boot --routes-dir "$UPROUTES" --gpx "$UPGPX" --at 60 \
    --script "$UPBASE $(UPSCOPE 1) $(UPFILTER 1)" --expect-screen WhatsNext --png "$OUT/up-ahead-waypoints-only-water.png"
"$SIM" "$UPMAP" --boot --routes-dir "$PLAINROUTE" --script "$UPBASE $(UPSCOPE 1)" --expect-screen WhatsNext --png "$OUT/up-ahead-nothing-waypoints.png"
# The `Next: <category>` stat tiles live (epic #946, U5), on the same POI-dense Monaco ride. The
# Auto climb panel would take the base screen on this line, so the script turns it Off first
# (`B u p p d d d p`), climbs back to Home, starts the ride and steps Back once to the Statistics
# view; the trailing frames let the per-category cache arm and harvest one snapshot per placed
# category (never per frame — that is the whole refresh policy). Water resolves to a *custom
# waypoint*, resupply and pharmacy to corridor POIs, so one frame pins both sources; the long names
# pin the ellipsis. NOTE the U4 source setting deliberately does not scope these tiles.
U5FIELDS="next-water,next-resupply,next-pharmacy,speed,dist-to-go"
U5CLIMBOFF="B u p p d d p b b b"
"$SIM" "$UPMAP" --boot --routes-dir "$UPROUTES" --gpx "$UPGPX" --at 60 --stat-fields "$U5FIELDS" \
    --script "$U5CLIMBOFF p p p p b f f f f f f" --expect-screen Statistics --png "$OUT/stats-next-category.png"
# The empty state: a route-less ride, where nothing can be "ahead" — icon + the category's own word
# + `--`, at the taller tile height the chart-less grid gives.
"$SIM" "$MAP" --boot --gpx "$GPX" --at 30 --stat-fields "$U5FIELDS" \
    --script "$U5CLIMBOFF B d d w p p p b f f" --expect-screen Statistics --png "$OUT/stats-next-category-empty.png"
# Route-less ride tracking (Menu's Map station). The Menu compass is Routes/Rides/Map/Peaks/Settings,
# so the Map station is two steps down from the Routes start (`d d w`). A live `--gpx` fix pins
# the follow camera + marker so the frames reproduce (no route → no magenta line, no off-route chip).
# (a) The route-less BROWSE map: Menu → Map (not tracking) → the follow map with clock + scale bar,
# and — new in T6 (#684) — the one-shot `Press to start a ride` hint chip (a two-line pill, since the
# sentence can't fit one line at 240 px) at the bottom on entry, the scale bar stepped above it. The
# `-settled` frame runs the browse map ~4.8 s past entry (enough `w` tokens > the 4 s window) to prove
# the hint auto-hides and the scale bar drops back to the corner. (The GPX replay runs after the
# script and drives the hint's clock not at all, so the extra `w`s are what expire it.)
"$SIM" "$MAP" --boot --clock "2025-06-29T14:40" --gpx "$GPX" --at 30 --script "B d d w p"     --expect-screen Map --png "$OUT/map-browse.png"
"$SIM" "$MAP" --boot --clock "2025-06-29T14:40" --gpx "$GPX" --at 30 --script "B d d w p w w w w w w" --expect-screen Map --png "$OUT/map-browse-settled.png"
# (b) The start card (browse map → press, T6 #684): the hero bike (the selected profile's sprite +
# colour) over its profile name, the two-row GPS / Battery checklist (the static Card row dropped
# in owner review round 1), then Start ride / Back. `--battery 45` pins the % and the `--gpx --at
# 30` fix makes GPS read `fix`; the second frame drops the `--gpx` (no fix) so GPS reads
# `searching..` (and a low --battery to vary the row).
"$SIM" "$MAP" --boot --clock "2025-06-29T14:40" --gpx "$GPX" --at 30 --battery 45 --script "B d d w p p"   --expect-screen RideStart --png "$OUT/ride-start.png"
"$SIM" "$MAP" --boot --clock "2025-06-29T14:40" --battery 8 --script "B d d w p p"   --expect-screen RideStart --png "$OUT/ride-start-nofix.png"
# (c) A route-less RIDING map (start card → Start ride): the follow map with the recorded breadcrumb,
# no route line and no off-route chip (there's no route to be off).
"$SIM" "$MAP" --boot --clock "2025-06-29T14:40" --gpx "$GPX" --at 30 --script "B d d w p p p" --expect-screen Map --png "$OUT/map-routeless.png"
# (d) The route-less Statistics page: the "No route loaded" band note over the stat grid, where the
# route-relative tiles (KM TO GO, TO CLIMB) read "--" and the rest are live.
"$SIM" "$MAP" --boot --clock "2025-06-29T14:40" --gpx "$GPX" --at 30 --script "B d d w p p p b" --expect-screen Statistics --png "$OUT/statistics-routeless.png"
# The mid-ride "ROUTE ACTIVE" swap card: riding route 0, out to the ride context's Routes row
# (`C d d` — the third row down) and press, then pick the *other* vector route (`d p`). Choosing
# a route while a ride is live raises the Swap / Finish & new / Cancel card instead of opening the
# overview.
"$SIM" "$MAP" --boot --routes-dir "$ROUTES" --script "p p p p C d d p d p" --expect-screen RouteSwap --png "$OUT/routeswap.png"
# Inspect mode: a thin rounded amber/ink frame follows the panel corners across Route, Free, and
# Zoom; only the active action's edge cues and the bottom-left scale bar join it. The clock and
# redundant labels stay out. A final `w` lets the entry hold's edge bulge retract before capture.
"$SIM" "$MAP" --boot --routes-dir "$ROUTES" --clock "2025-06-29T14:40" --script "p p p p h w" --expect-screen Map --png "$OUT/map-pan.png"
# Select tap walks the pan **mode ring** — Route Move -> Free Move -> Zoom (#1515 D3, where the
# family moved off Back-hold); Select-hold changes the axis only once Free is active.
"$SIM" "$MAP" --boot --routes-dir "$ROUTES" --clock "2025-06-29T14:40" --script "p p p p h p p w" --expect-screen Map --png "$OUT/map-pan-zoom.png"
"$SIM" "$MAP" --boot --routes-dir "$ROUTES" --clock "2025-06-29T14:40" --script "p p p p h p w" --expect-screen Map --png "$OUT/map-pan-free-v.png"
"$SIM" "$MAP" --boot --routes-dir "$ROUTES" --clock "2025-06-29T14:40" --script "p p p p h p h w" --expect-screen Map --png "$OUT/map-pan-free-h.png"
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
# LUT recessing a real map. States include the icon row, Bluetooth off, the held Assistant shortcut,
# the nested brightness editor, the guarded power confirmation, and that
# confirmation with the hold part-way through (`H`).
QUICK=(--routes-dir "$ROUTES" --clock "2025-06-29T14:40" --gpx "$GPX" --at 30)
"$SIM" "$MAP" --boot "${QUICK[@]}" --script "p p p p Q"           --expect-screen QuickDrawer --png "$OUT/quick-root.png"
"$SIM" "$MAP" --boot "${QUICK[@]}" --script "p p p p A w"     --expect-screen Assistant --png "$OUT/quick-assistant.png"
"$SIM" "$MAP" --boot "${QUICK[@]}" --script "p p p p Q d p w" --expect-screen QuickDrawer --png "$OUT/quick-bluetooth-off.png"
"$SIM" "$MAP" --boot "${QUICK[@]}" --script "p p p p Q p w"       --expect-screen QuickDrawer --png "$OUT/quick-brightness.png"
"$SIM" "$MAP" --boot "${QUICK[@]}" --script "p p p p Q d d d p w" --expect-screen QuickDrawer --png "$OUT/quick-power-confirm.png"
"$SIM" "$MAP" --boot "${QUICK[@]}" --script "p p p p Q d d d p w H" --expect-screen QuickDrawer --png "$OUT/quick-power-hold.png"
# The **other** root row: a platform whose panel has no controllable light offers three controls,
# not four, and opens on Bluetooth instead of brightness. That is the shipping board today (no
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
    # The Ride group per-language — the longest settings screen there is: five two-line rows, each
    # with a right-aligned value on the sub-caption line. Eyeball every label/sub pair against its
    # ◄value group (the clearance `cycle_row_value_clears_the_sub_caption` pins numerically).
    "$SIM" "$MAP" --boot --lang "$lang" --script "B u p p w"     --expect-screen Ride --png "$OUT/ride-settings-$lang.png"
    "$SIM" "$MAP" --boot --lang "$lang" --script "B u p d d d d p p"   --expect-screen Units --png "$OUT/units-$lang.png"
    # The `Next: <category>` tiles + their picker rows per language (epic #946, U5): the longest
    # category words (de `Campingplatz` / `Fahrradladen`, fr `Hébergement`) are what the tile caption
    # and the icon-gutter picker row have to fit whole.
    "$SIM" "$MAP" --boot --lang "$lang" --stat-fields "next-campsite,next-lodging,next-bike-shop" \
        --script "B u p p p" --expect-screen StatFields --png "$OUT/fields-next-category-$lang.png"
    "$SIM" "$MAP" --boot --lang "$lang" --script "B u p p p d d d d d d p d d d d d d d d d d" \
        --expect-screen AddField --png "$OUT/addfield-next-category-$lang.png"
    # Date & Time is the tightest screen per-language: the localized month name fills the fixed
    # month stepper cell (#614 widened it to 70 px for the four-char French months). Eyeball the
    # month glyphs against the active cell's amber border.
    "$SIM" "$MAP" --boot --lang "$lang" --script "B u p d d d d p d p" --expect-screen DateTime --png "$OUT/datetime-$lang.png"
    "$SIM" "$MAP" --boot --lang "$lang" --routes-dir "$ROUTES" --clock "2025-06-29T14:40" --gpx "$GPX" --at 30 \
        --script "p p p p b"    --expect-screen Statistics --png "$OUT/statistics-$lang.png"
    "$SIM" "$MAP" --boot --lang "$lang" --routes-dir "$ROUTES" --clock "2025-06-29T14:40" --gpx "$GPX" --at 30 \
        --script "p p p p"      --expect-screen Map --png "$OUT/map-$lang.png"
    "$SIM" "$MAP" --boot --lang "$lang" --routes-dir "$ROUTES" --script "p p p" --expect-screen RouteOverview --png "$OUT/routeoverview-$lang.png"
    # The route-plan sheet's one row label per language (#1515 D4d). "Type de vélo" / "Tipo de
    # bici" are 168 px, the widest labels the centred row's 172 px budget holds, so these are the
    # frames that show that fit on-glass. No per-language *editor* frame: its choices are the map's
    # own §8.6 names, byte-identical in every column.
    "$SIM" "$MONACO" --boot --lang "$lang" --routes-dir "$NAVDIR" --center 7416969,43730798 --heading 0 \
        --clock "2025-01-06T12:00" --script "$PLACEDETAIL C" \
        --expect-screen ContextDrawer --png "$OUT/route-plan-context-$lang.png"
    # The trip cascade-delete confirm (epic #526, TR3), per-language — the wrapped warning line + the
    # shortened "Delete all" button are the copy to eyeball for clipping in the longer translations.
    "$SIM" "$MAP" --boot --lang "$lang" --routes-dir "$TRIPDIR" --script "p p h" --expect-screen TripDelete --png "$OUT/trip-delete-confirm-$lang.png"
    # The received-route card family (#682): the idle card's View route / Dismiss rows, and the
    # mid-ride swap + ROUTE ACTIVE cards' Swap / Finish & new / Cancel rows — eyeball each for a
    # clipped option row now that the copy is per-language.
    "$SIM" "$MAP" --boot --lang "$lang" --routes-dir "$ROUTES" --inject upload=0 --expect-screen RouteReceived --png "$OUT/route-received-$lang.png"
    "$SIM" "$MAP" --boot --lang "$lang" --routes-dir "$ROUTES" --script "p p p p" --inject upload=1 \
        --expect-screen RouteSwap --png "$OUT/routeswap-received-$lang.png"
    "$SIM" "$MAP" --boot --lang "$lang" --routes-dir "$ROUTES" --script "p p p p C d d p d p" --expect-screen RouteSwap --png "$OUT/routeswap-$lang.png"
    # The Sensors screen (epic #707, SE7): the three kind rows + status lines, per-language — eyeball
    # for a clipped kind label ("Herzfrequenz" / "Fréq. cardiaque" / "Frec. cardíaca") or status line.
    "$SIM" "$MAP" --boot --lang "$lang" --sensors screen --script "B u p d d p d p" --expect-screen Sensors --png "$OUT/sensors-$lang.png"
    # The ride-start card (T6 #684): the checklist labels/values (GPS/Battery) are the copy to
    # eyeball for clipped rows in the longer translations. --battery 100 pins the widest % value.
    "$SIM" "$MAP" --boot --lang "$lang" --battery 100 --script "B d d w p p" --expect-screen RideStart --png "$OUT/ride-start-$lang.png"
    # The browse-map start hint chip (T6 #684): the two-line pill in each language, to eyeball for a
    # clipped line now that the copy is longer.
    "$SIM" "$MAP" --boot --lang "$lang" --clock "2025-06-29T14:40" --gpx "$GPX" --at 30 \
        --script "B d d w p" --expect-screen Map --png "$OUT/map-browse-$lang.png"
    # The SD-sideload update flow (epic #615 S5): the System menu, the Firmware page (whose "Install
    # update from card" label wraps to three Label lines in the longer translations), the
    # first-install confirm (the worst case for vertical fit — the two-row version table + the
    # no-undo note, which wraps to two Label lines), the progress spinner, an error card, and the
    # post-update toast — the text-heaviest DFU screens, to eyeball for clipped/overflowing copy.
    "$SIM" "$MAP" --boot --lang "$lang" --script "B u p d d d d p"         --expect-screen System --png "$OUT/system-$lang.png"
    "$SIM" "$MAP" --boot --lang "$lang" --script "B u p d d d d p d d d p" --expect-screen Firmware --png "$OUT/firmware-$lang.png"
    "$SIM" "$MAP" --boot --lang "$lang" --script "$DFU_PRE" --dfu scan=first --expect-screen DfuConfirm --png "$OUT/dfu-confirm-$lang.png"
    "$SIM" "$MAP" --boot --lang "$lang" --script "$DFU_PRE" --dfu progress=normal --expect-screen DfuProgress --png "$OUT/dfu-progress-$lang.png"
    # The terminal installing card per-language — the wrapped Body headline (two lines in French)
    # + the Label body + the warning line, to eyeball for clipped copy.
    "$SIM" "$MAP" --boot --lang "$lang" --script "$DFU_PRE" --dfu installing=normal --expect-screen DfuInstalling --png "$OUT/dfu-installing-$lang.png"
    "$SIM" "$MAP" --boot --lang "$lang" --script "$DFU_PRE" --dfu error=fragmented --expect-screen DfuError --png "$OUT/dfu-error-$lang.png"
    "$SIM" "$MAP" --boot --lang "$lang" --dfu confirmed=v1.0.0-14-g0a1b2c3-dirty --expect-screen DfuUpdated --png "$OUT/dfu-updated-$lang.png"
  WXNAV="p d d d d w p"
  # title + the choice it stages. `Jetzt laden` is the width constraint on the row.
  # The quick drawer's five states per language (#1515 D2) — the copy to eyeball is the caption
  # under the icon row, the brightness editor's title, and the two lines of the power confirmation,
  # each of which has to fit the sheet's width in the longer translations.
  # The map context's five row labels at 240 px — the sheet is all copy, so this is its overflow
  # check — and then its display sub-sheet, whose three labels have 36 px less room because the
  # slider takes it (#1515 D4c).
  "$SIM" "$MAP" --boot --lang "$lang" "${QUICK[@]}" --script "p p p p C"           --expect-screen ContextDrawer --png "$OUT/map-context-$lang.png"
  "$SIM" "$MAP" --boot --lang "$lang" "${QUICK[@]}" --script "p p p p C u p"       --expect-screen ContextDrawer --png "$OUT/map-display-sheet-$lang.png"
  # The Up-ahead sheet (#1515 D4a): its two row labels, then the nested editor's title + the choice
  # it stages. `Campingplatz` / `Alojamiento` are the width constraint on the editor line.
  "$SIM" "$MAP" --boot --lang "$lang" "${QUICK[@]}" --script "p p p p A d p f p f C"       --expect-screen ContextDrawer --png "$OUT/up-ahead-context-$lang.png"
  "$SIM" "$MAP" --boot --lang "$lang" "${QUICK[@]}" --script "p p p p A d p f p f C p w d d" --expect-screen ContextDrawer --png "$OUT/up-ahead-filter-editor-$lang.png"
  "$SIM" "$MAP" --boot --lang "$lang" "${QUICK[@]}" --script "p p p p Q"           --expect-screen QuickDrawer --png "$OUT/quick-root-$lang.png"
  "$SIM" "$MAP" --boot --lang "$lang" "${QUICK[@]}" --script "p p p p A w"     --expect-screen Assistant --png "$OUT/quick-assistant-$lang.png"
  "$SIM" "$MAP" --boot --lang "$lang" "${QUICK[@]}" --script "p p p p Q d p w" --expect-screen QuickDrawer --png "$OUT/quick-bluetooth-off-$lang.png"
  "$SIM" "$MAP" --boot --lang "$lang" "${QUICK[@]}" --script "p p p p Q p w"       --expect-screen QuickDrawer --png "$OUT/quick-brightness-$lang.png"
  "$SIM" "$MAP" --boot --lang "$lang" "${QUICK[@]}" --script "p p p p Q d d d p w" --expect-screen QuickDrawer --png "$OUT/quick-power-confirm-$lang.png"
  "$SIM" "$MAP" --boot --lang "$lang" "${QUICK[@]}" --script "p p p p Q d d d p w H" --expect-screen QuickDrawer --png "$OUT/quick-power-hold-$lang.png"

done

"$SIM" "$MAP" --boot --route-cleanup --clock "2025-07-10T09:41" --expect-screen RouteCleanup --png "$OUT/route-cleanup.png"
for lang in de fr es; do
    "$SIM" "$MAP" --boot --lang "$lang" --route-cleanup --clock "2025-07-10T09:41" --expect-screen RouteCleanup --png "$OUT/route-cleanup-$lang.png"
done

# Counted from the directory rather than hand-maintained — the literal that used to live here had
# drifted 37 frames behind the script.
echo "ui-snapshots: $(ls "$OUT"/*.png | wc -l | tr -d ' ') screens rendered into $OUT/"
