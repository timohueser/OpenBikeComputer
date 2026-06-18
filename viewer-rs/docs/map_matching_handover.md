# Live map-matching + off-route (route loading M2, Phase B)

> **STATUS: DONE (2026-06-18).** Shipped `obcm-route::matcher` (`RouteMatch`/`Match`),
> `App::tick(now_ms, loc, ele, route)`, actually-ridden `Activity` accumulators, and the
> off-route presentation on both screens. User decisions: stats = **actually-ridden**;
> off-route on **both** screens; **breadcrumb deferred** to its own slice; scrub =
> **transient** (snap-back) + a current-elevation readout. Plus a design change beyond this
> plan: **elevation is a separate barometric `ElevationSource`** (not GPS altitude / not on
> `Fix`) — see the handover note. **Next = Elevation profile zoom** (`elevation_zoom_handover.md`).
> The original plan follows for reference.

---


The deliverable: make the riding views **live**. Snap the GPS fix to the route
(`RouteMatch`), feed the matched position + ride accumulators into the Map and the
Elevation screen, and handle the **off-route** case on both. Branch **`ui-framework`**.
Builds directly on M2 Phase A (the static Elevation screen) — see the "Status" block at
the top of `elevation_screen_handover.md` for exactly what shipped.

## The one idea that makes this small

Phase A already routes everything through **a cursor fraction `0.0..1.0`**: the
"you are here" marker, the traveled-portion shading, the amber progress bar, and the
route-relative stats (`done`/`to go` from `total × frac`, `climbed`/`to go` from
`RouteReader::ascent_to`) all consume that one number. In Phase A the encoder produces
it. **Phase B's core job is to make it come from GPS** (`progress_m / total`). Most of
the drawing already exists; this is matching + wiring + the off-route presentation.

## What exists to build on (Phase A seams)

- `obcm-route`: `Profile` + `RouteReader::elevation_profile()` (`src/profile.rs`);
  `RouteReader::ascent_to(dist_m)` (`src/reader.rs`); shared distance/projection math in
  `src/geo.rs` (`cos_lat` / `delta_m` / `seg_dist_m`, all `pub(crate)`).
- `obcm-app`: `Screen::Elevation` (`src/screen/elevation.rs`); `App` owns a resident
  `profile` (rebuilt in `render_frame` on `active_route` change) exposed via
  `Render.profile`; `Activity { mode, active_route }` (grow it here).
- `obcm-app`: `AppState::enter_riding_view` (the follow + heading-up camera preset);
  `Render.route: Option<&RouteReader>` already threaded to draw.
- `obcm-render`: `Canvas::vline` / `Canvas::disc`.

## Step 1 — `RouteMatch` (obcm-route, new `src/match.rs`)

A **forward-biased** cursor that snaps a fix to the route in a *local window*, so it is
O(window) per fix, not O(route), and never snaps backward to an earlier near-pass on a
loop.

```text
struct RouteMatch { chunk: usize, seg: usize, progress_m: u32, off_route: bool }
struct Match { progress_m: u32, off_route: bool, dist_m: u32 }   // dist_m = cross-track
fn update(&mut self, fix: Fix, route: &RouteReader) -> Match
```

- **Search**: from the current `(chunk, seg)`, scan forward (and a couple of segments
  back) over a bounded window — decode the chunk(s) in range into the reused buffer, and
  for each segment compute the clamped projection of the fix onto it (cross-track
  distance + along-segment `t`). Take the nearest; advance `(chunk, seg)` to it.
- **`progress_m`** = `cum_distance_m` at the segment's start point + `t × seg_len`. (The
  per-chunk `ChunkMeta.cum_distance_m` re-anchors so this stays exact over a long route,
  same trick the profile builder uses.)
- **`dist_m`** = nearest cross-track distance.
- **`off_route`** = hysteresis on `dist_m` (enter at ~`OFF_M`, clear at ~`ON_M`, with
  `ON_M < OFF_M`) so it doesn't flap on GPS jitter at the boundary.
- **Off-route freeze**: while `off_route`, do **not** advance `progress_m` (a far fix
  must not drag progress); keep reporting the live `dist_m`. Resume advancing on rejoin.
- **Rejoin**: when off-route, widen the window (bounded) so a return to the line is
  found; optionally a periodic coarse pass that filters by `ChunkMeta.bbox` first (cheap)
  before decoding. Never an unbounded per-frame full scan.

**Reuse, don't duplicate the geometry**: promote a `project_to_segment(a, b, p) ->
(t_clamped, dist_m)` into `geo.rs` (it's the clamped sibling of `convert.rs`'s private
`perp_dist_m`). `RouteMatch` and the converter then share one projection. Mirrors the
Phase-A `geo.rs` extraction.

`App` owns one `RouteMatch`; reset it on route load / change (next to the profile-cache
reset in `render_frame`, or fold both into a small "route changed" hook).

## Step 2 — Decision (a): run the matcher in `App::tick`

The matcher needs the fix **and** the route geometry, but `tick` only gets the
`LocationSource`. Extend it: `App::tick(&mut self, loc, route: Option<&RouteReader>)`,
store the `Match` on `Activity`. Keeps matching in the shared layer.

- **Host edits** (both hosts open the reader already): reorder the frame so the route is
  opened **before** `tick`, and pass it to both `tick` and `render_frame`.
  - `obcm-sim/src/gui.rs` ~L257–288: move the `sync_active` + `RouteReader::open` block
    (currently L275–279) above the `tick` calls (L260/L272).
  - `obcm-sim/src/main.rs` headless: it renders a single frame without `tick`; for
    off-route **snapshots**, add one `app.tick(&mut player, route.as_ref())` after the
    `--gpx` seek so the matcher runs once before render.

## Step 3 — ride accumulators on `Activity` (Speed / Avg / climbed)

Grow `Activity` with: `progress_m`, `off_route`, `dist_to_route_m`, plus actually-ridden
`ridden_m`, `moving_s`, `climb_m`, and a `last_fix` cache. Feed them in `tick` from
consecutive fixes:

- `ridden_m += haversine(prev, cur)`; `moving_s += dt` only when moving (speed above a
  small threshold) so red lights don't tank the average; `climb_m += max(0, Δele)` with a
  dead-band like the converter's `ELE_THRESHOLD_M` so it reads like a planner, not noise.
- Paused (`Mode::Paused`) records nothing — drop the `last_fix` so resume doesn't book a
  giant jump across the gap (spec §6: pausing leaves a real gap).

## Step 4 — fill the grid + overlays (and decision (c): which stat is which)

Replace Phase A's scrub fraction with `progress_m / total`; the draw code is unchanged.
Per-tile source (the open part is route-relative vs actually-ridden — also spec §10.5):

| tile | source | note |
|---|---|---|
| Speed | `fix.speed_mps` | instantaneous (already wired) |
| Avg. Speed | `ridden_m / moving_s` | the last Phase-A `--` placeholder |
| done | **route-relative** `progress_m` | pairs with `to go` |
| to go | `total − progress_m` | necessarily route-relative |
| climbed | **decide**: `ascent_to(progress_m)` *or* ridden `climb_m` | differ off-route |
| to climb | `total_ascent − ascent_to(progress_m)` | necessarily route-relative |

Recommend route-relative `done`/`climbed` so each pair stays coherent (done+to-go=total),
and let the **"off route" readout** carry the rider's live-vs-route divergence rather than
splitting it across tiles. Confirm with the user.

## Step 5 — OFF-ROUTE handling (the part to decide with the user)

**Policy is fixed by the spec (§6):** *no dynamic rerouting in v1 — keep recording the
breadcrumb and show a small "off route · NNN m" readout; do not nag.* The palette even
reserves **Breadcrumb (cool, dotted) `#6E8FA0`** for the recorded track. What's left is
the **presentation on each screen** — bring these as concrete options:

### Map (off-route)
- Active route (amber) stays drawn — it's the line home. Camera stays Follow on the live
  fix, so the rider sees how far they've strayed.
- **Marker**: (A, rec.) turn it **warning red `#C0492E`** while off-route — glanceable,
  reuses `WARNING`; vs (B) leave it amber and signal only via the readout.
- **"off route · NNN m" readout**: the Map is intentionally chrome-free ("map only, no
  status bar"). (A, rec.) show a small parchment+warning **pill only while off-route**
  (disappears on rejoin, so steady state stays pure); vs (B) no Map chip — show the
  readout on Elevation only.

### Elevation (off-route)
- **"you are here" marker + progress bar**: progress is frozen, so the marker sits at the
  last on-route point. (A, rec.) tint marker/bar **warning** and swap the header's
  right-hand readout from `grade N%` → **`off route · NNN m`** (reuses the slot,
  glanceable); vs (B) keep grade, add a badge elsewhere.
- **Stats**: progress-derived tiles freeze (progress frozen); Speed/Avg keep moving. This
  is coherent **iff** `done`/`climbed` are route-relative (step 4). The off-route readout
  is what tells the rider their live state.

### Manual scrub vs the live cursor (Elevation)
Phase A's `turn` scrubs the cursor; the spec keeps `turn` = scrub on Elevation even with a
live position. **Decide**: does scrubbing temporarily override the live cursor and
**snap back after a few seconds idle** (recommend — a transient inspection overlay), or do
we keep a separate inspection cursor distinct from the "you are here" marker? Either is a
small state addition to `ElevationScreen`.

### Breadcrumb (recommend a separate slice)
Drawing the dotted cool-blue recorded track is orthogonal to matching: matching needs only
the fix; the breadcrumb needs the **stored ridden polyline** (a ring buffer of fixes, plus
the Finish/save path). Recommend Phase B only flags `off_route` + shows the readout, and
the breadcrumb gets its own slice with track recording/saving.

## Verify

- **`obcm-route` unit tests** (`tests/match.rs`): synthetic route + crafted fixes —
  on-line fix (progress increases monotonically, `off_route=false`), a fix beside the
  line (`off_route=true` past the threshold, progress **frozen**), a rejoin (clears,
  progress resumes), and a loop (forward-bias: doesn't snap to the earlier near-pass).
  Cross-track distance correctness on a known offset.
- **Headless snapshots**: a `--gpx` replay that leaves and rejoins the route (craft one,
  or detour `../test.gpx`), with the added headless `tick`; snapshot Map + Elevation in
  on-route and off-route states — check the readout, marker color, and the frozen
  cursor/bar. `cargo test --workspace`, clippy clean.

## Decisions to bring to the user (tomorrow)

1. Off-route thresholds — enter/clear meters (tune to real GPS noise).
2. Map: warning-red marker + an on-demand "off route" pill, **or** keep the Map pure and
   show off-route only on Elevation?
3. Elevation: swap header `grade` → `off route · NNN m` and tint the marker/bar?
4. `climbed`/`done` = route-relative (freeze off-route) **or** actually-ridden (keep
   moving)? (Also spec §10.5.)
5. Manual scrub once a live cursor exists — transient inspection (snap-back) or a separate
   cursor?
6. Breadcrumb drawing now or its own slice? (recommend its own slice.)
7. Carryover from Phase A: the Elevation header is the menu **wood** bar, not the spec
   §7 **dark HUD strip** — we chose menu-consistency; keep, or switch in a styling pass?
```
