# UI framework — session handover

Snapshot of the on-device UI track and the plan for the next push: **real routes
on the device**. Read alongside `ui_framework_brief.md` (the engineering plan) and
`bikepacking-computer-ui-spec.md` (the product spec). Work lives on the
**`ui-framework`** branch.

## Where we are

Built and committed this session (host-only; firmware still deferred):

1. **Text** — `obc-render/src/text.rs`: `draw_text`/`text_width`, a `Font` ladder
   (Label/Body/Display mono stand-ins), `TextAlign`. Quantizes through the host
   `color_fn` like the map.
2. **Input** — `obc-app/src/input.rs`: the shared `Gestures` recognizer (raw
   `InputEvent`s + a millis clock → the five gestures + hold-progress, identical
   host/MCU). HAL reworked to `Button{Encoder,Back}` + `InputEvent::Turn`.
3. **Screen stack** — `obc-app/src/screen/`: an `enum Screen` dispatched by
   `match` (no `dyn`, no alloc), navigation as a returned `Transition`
   {None,Push,Pop,Replace,Home} over a `heapless::Vec`. `App` owns the stack +
   recognizer and drives `tick → handle_input → render_frame`. Overlays composite
   over the topmost opaque screen.
4. **Drawing surface** — `obc-render/src/canvas.rs`: `Canvas` (clear/fill/round/
   round_outline/hline/triangle/text, each taking a palette RGB565) + `rect()`.
   Screen draw code reads like a layout description.
5. **Screens** — Map (the refactored map render), Ride control (the reusable
   guarded hold-to-confirm), Home, Menu, **Route menu**. List screens share
   `screen::list_frame` + `window_start`/`scrollbar` (overflow handled). Palette
   tuned to the 64-color (RGB222) gamut.
6. **Flow** — Home `press` → Route menu → `press` loads a route → Map (riding).
   `App::new_idle()` = the device's real boot (`[Home]`, Idle); `App::new()` =
   the sim's map-viewer default (`[Home, Map]`).

Tooling: `obc-sim --script <tokens>` drives the app to any screen for a headless
`--png` snapshot (`r`/`l` turn, `p` press, `h` hold, `b` back, `B` back-hold);
`--boot` starts at Home. This is the UI-dev feedback loop — no window needed.

85 tests, clippy clean.

## Next: real routes on the device

Today a "route" is `route::Route { name, distance_km, climb_m }` — a static mock
list (`route::routes()`), no geometry. Loading one only sets `Activity.active_route`
(an index) + opens the Map; **nothing is drawn on the map and the camera doesn't
move.** The goal: load a route with real geometry, draw it on the Map (amber line),
center the camera on it, and feed the Elevation screen.

### Groundwork to lay first

1. **Route representation (with geometry).** Grow beyond the summary struct:
   - A polyline: ordered points in **microdegrees** (`(i32, i32)` lon/lat, the
     map/`Viewport` convention), plus per-point **elevation (m)** and cumulative
     distance.
   - Derive `distance_km` / `climb_m` from the geometry (don't store separately).
   - **Split the model** for the MCU: a lightweight **summary** (name + totals)
     for the Route-menu list, and the heavy **geometry** loaded only for the *one*
     active route. Geometry lives in a fixed-capacity buffer
     (`heapless::Vec<_, MAX_ROUTE_POINTS>`), filled on load — only one route is
     active at a time, so only one buffer is needed. Keep `Activity` Copy/small
     (it stays the summary/index); the geometry lives in a **route store** owned by
     `App` and reached through `Ctx`/`Render`.

2. **A route source.** Where geometry comes from:
   - **Sim:** reuse the existing GPX parser (`obc-sim/src/gpx.rs` `Track`, already
     used for replay) to turn a `.gpx` into route geometry. Quickest path to "real
     routes" on the host.
   - **Device (later):** routes sync over BLE into storage; `route::routes()` reads
     that. Keep callers behind `routes()` so this swap stays local.
   - Decide the on-device storage/format separately (out of scope for the first
     pass — mock/GPX is fine to start).

3. **Draw the active route on the Map.** Add a route-overlay draw to the renderer
   (sibling to `MapRenderer::draw_marker`): project the route points through the
   `Viewport` and stroke a polyline in amber (`palette::AMBER`). `MapScreen::draw`
   calls it when a route is active (geometry via `Render`). The renderer already
   has polyline drawing for map line features to crib from. Breadcrumb (the
   *recorded* track) is a later, separate overlay.

4. **Center the Map on load.** Route-menu `press` should also set the camera to the
   route (its bbox/start) before opening the Map. Today it only sets the index.

### Seams already in place to build on

- `route::Route` + `route::routes()` — the list interface; extend, keep callers off
  the representation.
- `Activity.active_route: Option<usize>` + `Activity::route()` — the "which route"
  state; the geometry store hangs off this.
- `RouteMenuScreen::handle` (`obc-app/src/screen/route_menu.rs`) — the load point
  (`press` → set route + `Replace(Map)`); add geometry-load + camera-center here.
- `Render` ctx (`screen/mod.rs`) — already carries `reader`/`renderer`/`state`; add
  the active-route geometry so `MapScreen::draw` can reach it.
- `--script`/`--boot` + the `Canvas`/`list_frame` helpers — for fast visual
  iteration and consistent chrome.

### Open questions

- Route data format + on-device storage (BLE sync target) — defer; start from GPX.
- `MAX_ROUTE_POINTS` budget (decimation for long routes?) — size against the
  renderer's existing buffer budgets in `obc-render/src/lib.rs`.
- Does loading mid-ride (Menu → Routes while riding) replace the route? (Spec open
  Q4 — still TBD; the boot flow is the clean path for now.)

## Run / iterate

```
# headless snapshot of any screen (read the PNG):
cargo run -p obc-sim -- ../freiburg.obcm --boot --script p --png /tmp/routes.png --scale 2
# interactive, booting at Home:
cargo run -p obc-sim -- ../freiburg.obcm --boot
# tests:
cargo test --workspace
```

File map: app/stack `obc-app/src/{app,activity,route}.rs` + `screen/`; shared
drawing `obc-render/src/{text,canvas}.rs`; sim host `obc-sim/src/{main,gui,device_input}.rs`.
