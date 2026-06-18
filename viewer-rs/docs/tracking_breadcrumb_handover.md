# Tracking, breadcrumb & the load→ride→save loop — HANDOVER (STATUS: DONE)

Done 2026-06-18 on branch `ui-framework` (unpushed). Closes the ride-recording loop: loading
a route starts a **tracking session**, the path travelled is drawn as a **breadcrumb** and
streamed to an SD log, **Finish** writes a `.gpx`, and the out-and-back map-matcher bug is
fixed. Plan: `~/.claude/plans/lucky-nibbling-toast.md`.

## Model: session vs route (the core idea)

A **tracking session** (an `Activity.session: Option<u32>` id) is distinct from the
**navigated route** (`active_route`). A session spans load→Finish/Discard and *survives a
swap*; the route can change within it. The shared `obcm-app` core owns the session state
machine + the breadcrumb (RAM); the **host owns the file I/O and reconciles to the app's
intent each frame**, exactly like `RouteStore::sync_active` does for routes. `Activity` stays
`Copy`, so the session is a bare `u32` — the host derives the save *name* from
`catalog[active_route].name` when it opens a log.

Reset split (was conflated in `App::tick`): the **matcher** keys on `active_route`
(`App::matched_route`); the **accumulators + breadcrumb** key on `session`
(`App::ride_session`). So a *swap* re-locks the matcher but keeps stats/trail; a *new session*
resets everything.

## What landed, by crate

- **`obcm-route/src/track.rs`** (new, no_std): `TrackPoint` + a headerless 16-byte record
  (`encode_record`/`decode_record`, `TRACK_RECORD_LEN`) — truncating to a 16-byte boundary is
  always valid (crash-robust). `track_to_gpx(ByteSource→ByteSink)` streams GPX 1.1, opening a
  fresh `<trkseg>` on each `segment_start`. **No `<time>`** yet (no clock). Tests:
  `tests/track.rs`.
- **`obcm-route/src/matcher.rs`**: first-lock earliest-progress tie-break (`TIE_EPS_M = 8.0`)
  so an out-and-back's coincident finish can't latch the cursor. Regression:
  `tests/matcher.rs::out_and_back_first_lock_biases_to_the_start`.
- **`obcm-app`**: `Activity` gains `session`/`session_seq`/`track_action`/`last_alt` +
  `start_session`/`end_session`/`request_track`/`take_track_action`; `record_motion` returns
  `Motion{log,segment_start}`. New `breadcrumb.rs` = two-tier bounded trail (full-res `recent`
  `Deque<512>` + adaptive `spine` `Vec<768>` that halves+doubles-spacing when full; ~10 KB).
  `hal.rs` adds `trait TrackSink` + `Sensors.track`. `App::tick` feeds breadcrumb + sink on a
  logged Riding fix. New screen `route_swap.rs` (**ROUTE ACTIVE** prompt: Swap route / Save &
  new [hold-guarded] / Cancel); `route_menu.rs` branches on `is_tracking()`; `ride_control.rs`
  Finish→`Save`/Discard→`Discard`; `Transition::Root(Screen)` lands cleanly on `[Home, Map]`.
  Palette `ROUTE` deep blue `(0,0,170)` + `BREADCRUMB` red `(170,0,0)` (route ahead = blue,
  trail behind = red).
- **`obcm-render`**: `MapRenderer::stroke_path(iter of (i32,i32))` — `draw_route`'s
  project+overflow-guard loop for an in-RAM polyline; the Map strokes the breadcrumb **over**
  the route. (`draw_route` left untouched.)
- **`obcm-sim/src/track.rs`** (new): `TrackRecorder` mirroring `RouteStore` —
  `reconcile(action, session, name)` (drain action → finalise/abandon, then open on id change),
  `sink()`, finalise→`<name>.gpx` (numeric suffix if exists) via `track_to_gpx`. Wired into
  `gui.rs` (`reconcile_tracks` + `Sensors.track`), `replay_step`, and the headless `--png`
  path. New flags `--tracks-dir` (default `tracks/`) and `--save-track`.

## Verify

- `cargo test` (24 binaries green), `cargo clippy --all-targets` (clean), and the no_std
  crates build for `thumbv8m.main-none-eabihf`.
- **Full loop (headless):** from the repo root —
  `cargo run -p obcm-sim -- freiburg.obcm --boot --script pp --routes-dir viewer-rs/routes
  --gpx kandel.gpx --at 600 --tracks-dir /tmp/t --save-track --png /tmp/bc.png --scale 3`
  → writes `/tmp/t/kandel.gpx` (400 pts, `xmllint`-clean), renders the red breadcrumb behind
  the marker in the riding view, marker **not** warning-red (kandel no longer trips off-route).
  (`--routes-dir viewer-rs/routes` matters — `routes/` is cwd-relative.)
- **Swap prompt:** `--boot --script ppBprp --routes-dir viewer-rs/routes --png /tmp/swap.png`.

## Post-review tweaks (2026-06-18, after first render)

- **Breadcrumb is one line, not two.** The first cut drew `spine` *and* `recent` as
  overlapping polylines → two offset trails at riding zoom. Now the tiers are **disjoint**:
  a point lives in `recent` until it ages out of the ring, and only *then* is handed to
  `spine` (`Breadcrumb::push` → `spine_push`). The Map draws the whole trail as **one**
  chained stroke via `Breadcrumb::points()` (spine→recent). Short rides are all-`recent`
  (spine empty). Tests updated.
- **Bolder lines:** `ROUTE_WEIGHT` 3→5, `BREADCRUMB_WEIGHT` 2→4 (`screen/map.rs`).
- **Colours + z-order (final):** route recoloured amber→**deep blue** `ROUTE (0,0,170)`,
  breadcrumb→**red** `BREADCRUMB (170,0,0)`, and the breadcrumb now strokes **over** the route
  (was under) — so the trail behind the rider reads red and the route ahead reads blue.
- **Realistic GPS rate:** `GpxPlayer::poll` now throttles to ~1 Hz of *playback* time
  (`GPS_PERIOD_S = 1.0`, re-armed on `seek`/`play`) instead of emitting a fix every render
  frame — so the matcher / recorder / breadcrumb see a real-hardware cadence at any frame
  rate or replay speed. Tests: `poll_throttles_to_a_realistic_gps_rate`,
  `seek_re_arms_the_fix_throttle`.

## Deferred (noted, not built)

GPX `<time>` + filename timestamps (need a clock); dotted breadcrumb; streaming the breadcrumb
from `.obct` at deep zoom (the log is already on disk — a pure addition later); `.obct`
crash-recovery on boot; RideControl's bespoke hold-bar → shared `hold_hint` layer; FatFs
`TrackSink` + BLE upload on device.
