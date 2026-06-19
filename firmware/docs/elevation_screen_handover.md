# Next session — the Elevation screen (route loading M2)

The deliverable: the **Elevation screen** (`bikepacking-computer-ui-spec.md` §5/§8) — the
second riding view, a sibling of the Map. It also pulls in the **map-matching** that
both screens want ("where am I on the route"). Build on the M1 route-loading work
(`route_loading_handover.md`); branch **`ui-framework`**.

## Status — Phase A DONE (2026-06-18)

The static Elevation screen ships and is reachable via the Map↔Elevation `back` toggle.
What landed (a bit ahead of the literal Phase-A list, by design — see decisions below):

- **`obc-route`:** `Profile` (`src/profile.rs`, `PROFILE_COLS = 256` per-column
  min/max + peak) built by `RouteReader::elevation_profile()`; `RouteReader::ascent_to(dist_m)`
  (interpolates `cum_ascent_m`). Distance/elevation math factored into a shared `src/geo.rs`
  (`seg_dist_m` etc.) reused by `convert.rs`, so the profile buckets by the *same* metric
  the format stored. Tests in `tests/profile.rs`.
- **`obc-app`:** `Screen::Elevation` (`src/screen/elevation.rs`) + the toggle wiring
  (`map.rs` `back` → `Replace(Elevation)`, and back). `App` owns a resident `profile`
  rebuilt in `render_frame` only when `active_route` changes (decision (b)); threaded via
  `Render.profile`. `list_frame` was generalized into `title_frame(…, right)` so the
  Elevation header matches the menus.
- **`obc-render`:** added `Canvas::vline` + `Canvas::disc` (cursor line + dot).
- **Look:** wood-framed chrome like the menus; the band is **tan fill + amber top line**
  with the **traveled portion in dark olive** (no green — device-64 quantizes `#4F6B43`
  to gray; the app's amber/tan/olive survives). Snapshot: `--boot --script "ppb"` then add
  `r`/`l` to scrub.
- **The `turn` cursor drives everything in Phase A** (chosen with the user): the
  traveled shading, the amber progress bar, the "you are here" marker, and the
  route-relative stats (done/to-go via totals×frac, climbed/to-climb via `ascent_to`) all
  follow it. Speed comes from `fix.speed_mps`; **Avg. Speed is the only `--` placeholder.**

**Phase B is the remaining work below** (live map-matching). When it lands, the matched
`progress_m` simply *replaces the scrub fraction* as the cursor source — the draw code and
the stats already consume a fraction, so most of step 7 is wiring, not new drawing.
**→ Detailed Phase B plan (RouteMatch + ride stats + off-route UX on both screens):
`map_matching_handover.md`.** The phasing below is the original sketch it expands.

## Target (from the spec)

- **Content:** the route's **elevation profile** with traveled-portion shading + a
  **"you are here" marker** + a **peak label** + a thin **amber progress bar**, and a
  **2×3 stat grid**: Speed · Avg. Speed · done (km) · to go (km) · climbed (m) · to
  climb (m). Parchment body + the dark HUD title strip (like the menus).
- **Bindings** (spec table): `turn` = scrub a profile cursor · `press` = pause → Ride
  control · `back` = → Map · `back-hold` = → Menu · `hold` = unbound.
- **Map ↔ Elevation toggle:** Map's `back` opens Elevation, Elevation's `back` returns —
  via `Replace` (siblings, the stack stays `[Home, <one of them>]`). Map's `back` is
  currently the stub `Transition::None` in `screen/map.rs` ("Elevation — later slice").

## Suggested phasing

**Phase A — the screen, static (ships something visible without map-matching).**
1. New `screen/elevation.rs` + `Screen::Elevation` variant + three match arms in
   `screen/mod.rs` (the "adding a screen is a local edit" pattern). Wire the toggle:
   `MapScreen` `back` → `Replace(Elevation)`, `ElevationScreen` `back` → `Replace(Map)`,
   both `back-hold` → Menu, `press` → Ride control (crib `map.rs`).
2. **Elevation profile from the route.** Add a helper that fills a fixed
   `[(i16,i16); PROFILE_W]` (per-column min/max elevation) by streaming the route in
   order: each chunk starts at its `ChunkMeta.cum_distance_m`; accumulate per-segment
   haversine within the chunk to get each point's distance, bucket into a column
   (`col = dist * PROFILE_W / total_distance`). Y-range from the header
   `min_ele_m..max_ele_m`. Put it on `RouteReader` (`obc-route`, testable, MCU-shared).
   It reads every chunk once, so **build it once and cache** — see decision (b).
3. Draw the profile (filled band + amber top line) + peak label, the `turn` scrub
   cursor (manual inspection, needs no GPS), and the stat grid with what's available now
   (total km / total climb as "to go"/"to climb"; Speed from `fix.speed_mps`; the rest
   as placeholders until Phase B). Crib `Canvas` + `list_frame`/HUD chrome.

**Phase B — live position (map-matching + ride stats).**
4. **`RouteMatch`** (new, in `obc-route`): a forward-biased cursor (chunk + segment +
   interpolation) with `update(fix, &RouteReader) -> { progress_m, off_route }` that
   searches a *local* window around the last match (O(window), not O(route)) and flags
   off-route past a distance threshold. `App` owns one, reset on route load.
5. Run it each frame: the matcher needs the fix **and** the route geometry, but
   `App::tick` only gets the `LocationSource`. **Decision (a):** thread the active
   `Option<&RouteReader>` into `tick` (or a sibling `App::update_match(route)`) — the
   host already has it (it passes it to `render_frame`).
6. **Ride accumulators on `Activity`** (the `activity.rs` "later slice"): `distance_m`,
   `moving_time_s`, `climb_m`, fed from consecutive fixes in `tick`. → Speed / Avg.
   Speed / climbed.
7. Fill the rest of the grid + the overlays: done = `progress_m`, to-go =
   `total − progress`; to-climb = `total_ascent − cum_ascent@cursor` (interpolate the
   `ChunkMeta.cum_ascent_m` the format already stores); traveled-portion shading + the
   "you are here" marker + amber progress bar at `progress / total`. On the **Map**, use
   the same `progress_m` to style traveled-vs-ahead (and show off-route).

## Decisions to make first

- **(a) Where map-matching runs.** Recommend extending `App::tick(loc, route)` (the host
  already owns the reader) and storing the result on `Activity`. Keeps the matcher in the
  shared layer, not the host.
- **(b) Where the profile is cached.** Building it re-reads every chunk, so don't do it
  per frame. Recommend `App` owns a resident `profile` buffer rebuilt on route load
  (host calls it once with the reader, like `set_routes`), exposed via `Render`. The
  alternative — caching in `ElevationScreen` state — rebuilds on every Map↔Elevation
  toggle.
- **(c) Stat semantics (spec open Q, brief §"Open").** Are "done"/"climbed"
  **route-relative** (from `progress_m` + the route profile) or **actually-ridden**
  (from the `Activity` accumulators)? They differ when off-route. Pick per stat;
  "to go"/"to climb" are necessarily route-relative.

## Seams already in place

- `Render.route: Option<&RouteReader>` + `ChunkMeta.cum_distance_m`/`cum_ascent_m`
  (written in M1 precisely for remaining-stats) + the header totals/ele range.
- `Activity` (Copy/small, `mode` + `active_route`) — grow it with `progress_m`,
  `off_route`, and the ride accumulators.
- `AppState::enter_riding_view` (the riding camera preset) — the matcher's progress can
  later drive map cropping; the camera already follows + heading-up.
- The screen-stack pattern, `Canvas`, `palette`, `list_frame`/HUD, and the
  `--script`/`--png` headless loop (e.g. `--boot --script pb` = load route → Map →
  `back` → Elevation, once the toggle is wired) for fast visual iteration.

## Verify

`cargo test --workspace` (clippy clean). Headless: import a GPX with real elevation
(`../test.gpx`, 193–297 m), then snapshot the Elevation screen via the toggle script and
read the PNG — profile shape, peak label, stat grid. Add `obc-route` unit tests for the
profile helper and `RouteMatch` (a synthetic route + a few fixes on/off it).
