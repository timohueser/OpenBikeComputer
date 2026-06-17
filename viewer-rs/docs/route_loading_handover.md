# Route loading — session handover

Milestone 1 of route loading is built and verified on the **`ui-framework`** branch.
Read alongside `OBCR_Spec.md` (the route format) and `ui_framework_handover.md` (the UI
track this extends). Plan of record: `~/.claude/plans/happy-frolicking-tide.md`.

## What shipped (M1: static — draw + center + stats)

A route now flows **GPX → on-device-style conversion → compact `.obcr` → drawn on the
Map**, end to end, in the simulator, using device-ready abstractions.

1. **`obcm-route` crate** (`no_std`, the single source of truth, shared by sim + future
   firmware — there is **no** webapp/Python converter; GPX is converted where it lands):
   - `byte_io.rs` — `ByteSource`/`ByteSink` traits (random-access read / sequential
     write + header patch) + `SliceSource`. The seam that lets the same format code run
     over a host `Vec` and a device FatFs handle.
   - `reader.rs` — `RouteReader` (monomorphic: resident header + chunk index +
     `&dyn ByteSource`; decodes chunks on demand), `RouteSummary` (header-only, for the
     menu), `ChunkMeta`, `RoutePoint`.
   - `gpx.rs` — `GpxScanner`: a streaming, block-boundary-safe `<trkpt>`/`<ele>` scan
     (O(1) RAM for any route length).
   - `convert.rs` — `gpx_to_obcr`: one streaming pass → exact stats (haversine distance,
     hysteresis ascent/descent) + decimated, chunked, seam-sharing geometry; index
     collected in RAM, header patched last.
2. **`.obcr` format** (`OBCR_Spec.md`): `OBCM`-style microdegrees + delta geometry +
   **precomputed stats in the header**; chunked + offset-addressed so the reader streams
   only the chunks near the view.
3. **Renderer**: `MapRenderer::draw_route` (sibling of `draw_marker`) — visible-chunk
   query → decode → project → stroke amber, sub-pixel-decimated, seams closed.
4. **App**: the static `route::routes()` mock is gone. `App` owns a `Catalog`
   (`set_routes`); `Ctx`/`Render` carry the catalog + the active `RouteReader`. The Route
   menu lists real summaries, has an empty state, and on `press` centers the camera on
   the route bbox + sets `active_route`. `MapScreen::draw` strokes the active route.
5. **Sim "SD card"** (`obcm-sim/src/routes.rs`): `RouteStore` = a folder of `.obcr`
   files. `--routes-dir DIR` (default `routes/`), `--import GPX` (headless convert — the
   host run of the device's USB-drop path), and **drag-drop a `.gpx`** onto the window.
   The store backs `ByteSource`/`ByteSink` with `std` files; nothing above it knows it's
   a folder.

Verified: `cargo test --workspace` (clippy clean); `--import ../test.gpx` →
`16 km, +115/-19 m, 116 pts, 1 chunk` in an 846-byte file; headless `--boot --script p`
shows the route in the menu and `--script pp` draws it amber on the Freiburg map,
centered. Empty `--routes-dir` shows the "No routes yet" state.

## Next (M2: live position-on-route)

**The next session is scoped in `elevation_screen_handover.md`** (the Elevation screen +
the map-matching it needs). The format already carries what M2 needs —
`ChunkMeta.cum_distance_m`/`cum_ascent_m` are written now.

1. **Map-matching.** A monotonic cursor (chunk + segment) snapped to the nearest segment
   in a local window per `Fix`; off-track when nearest distance > a threshold. Advance
   forward-biased so it's O(window), not O(route).
2. **Remaining stats.** `total − cum_at_cursor` (O(1) from the index) → distance/climb
   left, feeding a new **Elevation** screen (the profile + remaining numbers).
3. **Route styling.** Distinguish travelled vs. ahead (and the off-track state) on the
   Map overlay; breadcrumb of the *recorded* track is a separate later overlay.
4. **Device.** BLE upload is the device twin of the sim's import; the converter +
   `ByteSource`/`ByteSink` are already firmware-ready — wire FatFs impls.

## Smaller follow-ups / decisions to revisit

- **Route name = GPX file stem.** Parse `<trk><name>` (or `<metadata><name>`) for a nicer
  title; the converter takes the name as a parameter, so it's a one-line change at the
  call site once parsed.
- **Route load enters the riding view** — `AppState::enter_riding_view` (Follow,
  heading-up, ~0.5 m/px seeded at the route start) rather than framing the whole route
  (done 2026-06-18). `m/px → zoom` via `obcm_render::zoom_for_mpp`.
- **Single resolution.** No separate zoomed-out overview LOD; the converter decimates to
  ~`MAX_SPAN_M` (1200 m) which keeps even a long route to a few hundred chunks. Add a
  coarse layer only if whole-route-zoomed-out draws get heavy.
- **`MAX_SPAN_M` assumes ≤ ~70° latitude** for `int16`-delta safety (documented in
  `convert.rs`); add densification if routes go further north.

## Run / iterate

```
# import a GPX into the sim's "SD card", then snapshot the menu + the drawn route:
cargo run -p obcm-sim -- ../freiburg.obcm --import ../test.gpx --routes-dir /tmp/routes
cargo run -p obcm-sim -- ../freiburg.obcm --boot --routes-dir /tmp/routes --script pp --png /tmp/route.png --scale 2
# interactive (drag-drop a .gpx onto the window):
cargo run -p obcm-sim -- ../freiburg.obcm --boot --routes-dir /tmp/routes
cargo test --workspace
```

File map: format/converter `obcm-route/src/{byte_io,reader,gpx,convert}.rs`; route draw
`obcm-render/src/lib.rs` (`draw_route`); app catalog/menu/map
`obcm-app/src/{route.rs, app.rs, screen/{mod,route_menu,map}.rs}`; sim store
`obcm-sim/src/{routes.rs, main.rs, gui.rs}`.
