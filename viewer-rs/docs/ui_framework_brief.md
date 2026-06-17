# UI framework brief

The next big track after the renderer work: the on-device UI **beyond the map**
for a **bikepacking computer / GPS activity recorder** — input handling,
navigation, screens, menus, and a UI framework that is modular and easy to
extend. MCU firmware is deferred (no nRF54L DK / panel yet), so this is built and
validated **entirely on the desktop host**: `obcm-app` already runs the same
`App::tick`/`render_frame` on both targets, so this is real device-UI progress
that de-risks the eventual firmware bring-up.

**Prime directive: this is a v1 draft and will keep changing.** Control flow,
layout, and the screen set are explicitly provisional, and more screens/states
are coming. Optimize the framework for *cheap change* — adding, removing,
reordering, and re-binding screens must each be a small, local edit.

Upstream design inputs (product intent — this brief is the engineering plan that
implements them):
- `docs/bikepacking-computer-ui-spec.md` — full v1 control spec (modes, control
  flow, per-screen gesture table, tracking semantics, palette).
- `docs/bikepacking_portrait_screens.html` — style mock (elevation + route menu).

## Hardware

- **MCU:** Nordic **nRF54L** (Cortex-M33, BLE).
- **Display:** 240×320 portrait, **64-color reflective MIP** panel
  (LS021B7DD02 class). Matte, sunlight-readable, holds its image without power,
  no smooth gradients. Design for it: flat fills, **dither** for shading, crisp
  **1px** linework, maximize contrast, **redraw only on change** (the panel holds
  its image — this is the main power lever).
- **Input:** one **rotary encoder with push** + one **Back** button. No touch.
- **Connectivity:** BLE companion app — routes/tiles in, recorded tracks out.

## Input model — five gestures

| Gesture | Notation |
|---|---|
| rotate encoder | `turn` |
| encoder short press | `press` |
| encoder long press | `hold` |
| Back short press | `back` |
| Back long press | `back-hold` |

Globals: **`back-hold` toggles the Menu** from any main screen (Home, Map,
Elevation); **`hold` toggles Pan mode** on the Map. Long-press detection and
hold-progress live in one shared layer (a millis clock, identical host + MCU), so
every screen sees the same five gestures plus a hold-progress value.

## Operating modes & tracking

Three modes: **Idle** (Home screensaver) · **Riding** (Map or Elevation, tracking
on) · **Paused** (Ride control overlay).

- Tracking **starts** on route load (Route menu `press`).
- **Pause** (`press` on Map/Elevation) stops recording immediately; movement while
  paused is not recorded (gap preserved — intended).
- **Resume** continues the same track. **Finish** saves the track → Home.
  **Discard** deletes it → Home.
- No dynamic rerouting in v1 — just an "off route · N m" readout.

This implies new app state: an **`Activity`/tracking model** (accumulates
distance / time / climb from `Fix`es; carries the mode). It belongs in
`obcm-app`, fed by `App::tick`, read by the screens.

## Screens & gesture bindings

~9 screens. Authoritative bindings (from the spec §5, **with the Ride-control
correction we agreed**: Resume = `press`, Finish/Discard = `hold`):

| Screen | `turn` | `press` | `hold` | `back` | `back-hold` |
|---|---|---|---|---|---|
| Home | – | → Route menu | – | – | → Menu |
| Route menu | scroll routes | load route → Map (tracking on) | – | → caller | – |
| Map / Follow | zoom | pause → Ride control | → Pan mode | → Elevation | → Menu |
| Pan mode | pan along axis | toggle axis (L-R ↔ U-D) | exit → Follow | recenter | – |
| Elevation | scrub cursor | pause → Ride control | – | → Map | → Menu |
| Ride control | choose option | **activate if instant** (Resume) | **confirm if guarded** (Finish/Discard) | Resume (cancel) | – |
| Menu | scroll (Routes/Settings) | open selected | – | → caller | → Shutdown |
| Settings | move / adjust | change / toggle | – | → Menu | – |
| Shutdown | move (Off/Cancel) | confirm (Power off = guarded) | – | cancel → Menu | – |

Note: **Map is map-only — no status bar, no speedometer.** Current and average
speed live on the Elevation screen. The dark HUD title strip on Elevation/menus is
per-screen chrome (part of the visual language), not a global status bar.

## Guarded actions & the hold ring

Decided: irreversible actions are **hold-to-confirm**, signalled by a **chunky
segmented pixel ring** (legible on the matte panel, on-aesthetic). This is a
**reusable per-item property**, not a one-off:

- Each selectable item carries `guard: bool`.
- A list/option screen routes `press` → activate non-guarded items; `hold` →
  activate guarded items (ring fills with hold-progress; release early cancels).
- Guarded items render the ring as a **static dim segmented ring** ("needs a
  hold") that fills (warning-red for destructive) while held; non-guarded items
  render none.

Apply it wherever an action is irreversible — Ride control (Finish, Discard),
Shutdown (Power off), and future destructive settings — for one consistent
language. (Mock shown in chat 2026-06-17.)

## Visual language

"Explorer's field map" — *a touch* of video-game so the low res reads as
intentional, but no game chrome (no health/XP). Wood-and-parchment frame on menus;
parchment body + dark HUD strip on Elevation.

- Palette (tune to the 64-color gamut): parchment `#EADFC0`, parchment-shade
  `#DFD0AB`, wood `#5B3F28`/`#2E251A`, ink `#2C2114`, amber accent `#E3A52B`
  (active route + "you"), land/forest/water `#7C9A63`/`#4F6B43`/`#33575B`,
  breadcrumb `#6E8FA0`, warning `#C0492E`.
- **Pixel/bitmap fonts** — Silkscreen (headers) + VT323 (numbers) as stand-ins;
  on device, a converted pixel font (m5x7 / m3x6 / Pixellari / monogram).
- Dither for shading; crisp 1px lines; flat fills only.

## Architecture

Constraint: `no_std` + zero-alloc (it runs on the MCU), so **no retained widget
tree** (heap + dynamic dispatch). The model below keeps it modular *and*
MCU-legal, and maps directly onto the spec's flow (Menu and Ride control are
overlays that "return to caller" — that *is* a push/pop stack).

- **Screens are an enum, dispatched by `match`:**
  `enum Screen { Home(..), Map(..), Elevation(..), RouteMenu(..), RideControl(..), Menu(..), Settings(..), Shutdown(..), Pan(..) }`.
  Each variant is its own module with typed state.
- **A tiny screen contract** (static dispatch, no `dyn`):
  ```text
  fn handle(&mut self, g: Gesture, ctx: &mut Ctx) -> Transition;
  fn draw<D: DrawTarget>(&self, target: &mut D, ctx: &Ctx);
  ```
- **Navigation is a return value:**
  `enum Transition { None, Push(Screen), Pop, Replace(Screen) }`, applied by a
  `heapless::Vec<Screen, N>` stack. `back` that pops the top is the guaranteed
  escape; overlays Push and `back`/Resume Pop back to the caller automatically.
- **`Ctx`** carries the shared bits a screen needs: the `Reader`, the
  `Activity`/mode state, the millis clock, hold-progress, and the `MapRenderer`.
- **`MapScreen` = a refactor of today's `App::render_frame`** — the map becomes
  the Riding screen; `App` shrinks to: poll inputs → top screen `handle` → apply
  `Transition` → top screen `draw`, with the dirty flag gating redraw.

### Adding a screen (the modularity test)

1. add a module with a state struct + `handle`/`draw`;
2. add one `Screen` enum variant;
3. push it from wherever it's reached.
No other file changes — no central dispatch table, no trait objects, no alloc.
Re-binding a gesture is a one-line edit in that screen's `handle`. Reordering /
removing screens touches only the variant and its push sites.

### HAL changes (`obcm-app/src/hal.rs`)

Today: `LocationSource`/`Fix` plus `Button`/`ButtonEvent`/`InputSource` *defined
but unused* (`App::tick` only takes `LocationSource`). Needed: encoder detents +
encoder/Back button edges + a **millis clock**; a shared layer turning those into
the five `Gesture`s + hold-progress; and consuming `InputSource` in `App::tick`
(its doc comment already anticipates this).

### Prerequisite: text rendering

No text/font rendering exists in the shared crate yet (only the marker polygons).
Wire embedded-graphics fonts — ultimately a converted **pixel font** — and confirm
they render correctly through the quantizing `color_fn` on a 64-color target.
**Nothing menu-shaped is buildable until this lands**, so it is slice 1.

## First vertical slice (host-only, no DK/display)

1. **Text rendering** in the shared crate, verified via headless `--png`.
2. **Gesture plumbing:** HAL encoder/Back + millis clock → shared
   short/long/hold-progress layer → `Gesture`; consume in `App::tick`. Keyboard /
   scroll-wheel → raw input in `obcm-sim` (a real device-input emulation path; the
   existing egui debug panel stays as a separate dev tool).
3. **Screen stack** with `Transition` + 2–3 screens: `MapScreen` (Follow; `press`
   = pause), `RideControl` (Resume = press, Finish/Discard = guarded hold ring),
   and a stub `Menu`. Prove overlay push/pop + return-to-caller.
4. **Tests** mirroring `obcm-render/tests/priority.rs`: feed `Gesture`s, assert
   `Transition`s and that a guarded action requires a completed hold; `--png`
   snapshots of the map and Ride control.

Proves the whole foundation before any firmware.

## Open questions (carry-forward; mostly answered by the spec)

The spec answered most of my earlier questions (Save/Discard → Home; main menu =
Routes/Settings; map `back` → Elevation). Remaining, from spec §10 + ours:

1. Mid-ride route change (Menu → Routes while riding) — replace route + restart
   tracking, or block? (TBD)
2. Final Elevation stat set, or allow swapping a tile for live grade / ETA?
3. Settings contents (placeholder for v1).
4. Power-on resume of an in-progress route (assumed yes).
5. Hold-threshold ms and whether fast encoder spins accelerate zoom/scroll.
6. On-device pixel font choice (start with a built-in mono font, swap to a pixel
   font once the look is dialed in).
