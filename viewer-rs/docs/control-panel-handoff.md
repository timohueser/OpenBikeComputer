# Control-panel development — handoff guide

**Read this first if you're picking up the simulator control-panel work.** It is a
self-contained brief: where the code is, the exact APIs you'll touch, the Step 4
task, the verification workflow, and the hard-won gotchas. You should not need the
original chat transcript.

---

## 1. Where we are

We're building a **device emulator** around the existing OBCM map simulator. The
end goal: a separate "control panel" window that drives the simulated device —
GPS position, heading, zoom, and later physical buttons — so the simulator becomes
a faithful stand-in for the real nRF5340 + LS021B7DD02 hardware. The same app
logic runs on the real firmware; the simulator and firmware are just two *hosts*
behind a hardware-abstraction layer (HAL).

This was planned as 5 steps. **Steps 1–4 are done and verified.** Next up are the
*features the panel exists for* (see §9) and Step 5 polish.

| Step | What | Status |
|---|---|---|
| 1 | `obcm-app` crate: `Fix`, `LocationSource`/`InputSource` traits, `AppState` follow/free camera | ✅ done |
| 2 | In-house `Framebuffer` `DrawTarget` + `image`-crate PNG (replaced e-g-simulator's output) | ✅ done |
| 3 | eframe/egui host window; `App::render_frame` shared entry point; **SDL dropped**; bare-metal build proven | ✅ done |
| 4 | Control-panel window: lat/lon, heading, log-zoom (m/px + ground span), Follow/Free toggle | ✅ done |
| 5 | Docs/memory polish | partially done |

After Step 4 come the *features the panel exists for*: user-position marker, GPX
replay, virtual buttons (see §9).

**Step 4 as built** (`obcm-sim/src/gui.rs`): a second "Controls" immediate viewport
holds `PanelState` mirrors (`lat_deg`/`lon_deg`/`heading_deg`), pushed into
`SimLocationSource` each frame; zoom is a log slider in m/px that only writes back
on `resp.changed()` (so it never fights mouse scroll); Follow/Free are
`selectable_value`s bound to `AppState.mode`, and entering Follow snaps the fix to
the current camera center. Closing the Controls window quits the app (`quit` flag).
The central panel now fits the device image to the window at the largest integer
scale ≤ `--scale` (fixes the §6 bottom-clipping). Pure helpers (`zoom_to_mpp`,
`mpp_to_zoom`, `format_distance`) are unit-tested (obcm-sim now 7 tests).

---

## 2. Architecture & crate map

Dependency direction: **`obcm-sim → obcm-app → obcm`**. The firmware will be a
second host beside `obcm-sim`, reusing `obcm-app` + `obcm` unchanged.

```
obcm/          no_std reader + shared renderer (feature `render`)
  src/reader.rs    OBCM v3 parse, quadtree query, select_lod_for_mpp
  src/render.rs    Viewport (projection) + MapRenderer (generic over DrawTarget)
  src/color.rs     rgb565_to_rgb888 / rgb565_to_device64

obcm-app/      no_std app + HAL  (depends on obcm with default-features = false!)
  src/hal.rs       Fix, LocationSource, InputSource, Button, ButtonEvent
  src/app.rs       CameraMode, AppState, App (the per-frame driver)

obcm-sim/      eframe/egui desktop host
  src/main.rs          Args, parse_args, color_of, initial_camera, --png path, dispatch
  src/framebuffer.rs   Framebuffer: DrawTarget<Color=Rgb888> over a Vec<u8>
  src/sim_location.rs  SimLocationSource: the host's LocationSource (panel writes here)
  src/gui.rs           SimGui (eframe::App): screen window, mouse pan/zoom, --screenshot
```

**The HAL seam is the whole point.** The shared `App` reads the user's position
from a `LocationSource` and never knows whether it came from a panel slider, a GPX
file, or a real GPS chip. Step 4 only writes into `SimLocationSource` and flips
`AppState.mode` — **no changes to `obcm-app` or `obcm` should be needed.**

---

## 3. API cheat-sheet (exact signatures as they exist now)

### obcm-app
```rust
// hal.rs
pub struct Fix { pub lat: i32, pub lon: i32,           // microdegrees (1e-6°)
                 pub course: Option<f32>,              // deg CW from north, valid when moving
                 pub speed_mps: Option<f32> }
impl Fix { pub fn at(lat: i32, lon: i32) -> Self }     // stationary, no course/speed
pub trait LocationSource { fn poll(&mut self) -> Option<Fix>; }
pub trait InputSource    { fn poll(&mut self) -> Option<ButtonEvent>; }  // NOT consumed yet
pub enum Button { Up, Down, Left, Right, Select, Back } // provisional
pub enum ButtonEvent { Down(Button), Up(Button) }

// app.rs
pub enum CameraMode { Follow, Free }
pub struct AppState { pub cam_lon: f64, pub cam_lat: f64, pub zoom: f64,
                      pub mode: CameraMode, pub user_fix: Option<Fix> }
impl AppState {
    pub fn new(cam_lon: f64, cam_lat: f64, zoom: f64) -> Self;   // defaults to Follow
    pub fn update(&mut self, loc: &mut dyn LocationSource);      // Follow recenters on fix
    pub fn viewport(&self, w: f64, h: f64) -> Viewport;
}
pub struct App { pub state: AppState, /* renderer: MapRenderer */ }
impl App {
    pub fn new(state: AppState) -> Self;
    pub fn tick(&mut self, loc: &mut dyn LocationSource);        // = state.update
    pub fn render_frame<D: DrawTarget, F: Fn(u16)->D::Color>(
        &mut self, target: &mut D, reader: &Reader, w: f64, h: f64, color_fn: F) -> RenderStats;
}
```

### obcm (render)
```rust
pub struct Viewport { pub w, h, cam_lon, cam_lat, zoom, aspect: f64 }  // zoom = px per microdeg-lat
impl Viewport {
    pub fn to_map(&self, x: f64, y: f64) -> (f64, f64);   // screen px -> (lon, lat) microdeg
    pub fn meters_per_pixel(&self) -> f32;                // = 0.111_320 / zoom  (METERS_PER_MICRODEG_LAT)
}
```

### obcm-sim
```rust
// sim_location.rs  (REMOVE the `#[allow(dead_code)]` once the panel calls these)
impl SimLocationSource {
    pub fn new(fix: Option<Fix>) -> Self;
    pub fn current(&self) -> Option<Fix>;
    pub fn set_position(&mut self, lat: i32, lon: i32);   // microdegrees
    pub fn set_course(&mut self, deg: f32);
}
// main.rs
fn color_of(c: u16, true_color: bool) -> Rgb888;
fn initial_camera(reader: &Reader, width: u32) -> (f64, f64, f64); // (cam_lon, cam_lat, zoom)
struct Args { map, width, height, scale, png: Option<String>, screenshot: Option<String>, true_color }
```

`SimGui` (in `gui.rs`) currently holds: `bytes, app: App, loc: SimLocationSource,
fb: Framebuffer, dev_w, dev_h, scale, true_color, texture, screenshot, screenshot_requested`.
Its `update()` does: `render_to_texture(ctx)` → `CentralPanel` (frame `none`) shows
the screen image and calls `handle_camera_input` (drag pan / scroll zoom → Free
mode) → optional `--screenshot` → `ctx.request_repaint()`.

---

## 4. Step 4 task — the control panel

Add a **second OS window** (egui *immediate viewport*) titled "Controls" with:

1. **Center lat/lon** — two numeric inputs in **degrees** (`egui::DragValue`, ~5
   decimals). Internally everything is microdegrees `i32`; convert `deg = µdeg /
   1e6` for display and `µdeg = round(deg * 1e6)` on write.
2. **Heading** — 0–360° (a `Slider`, or a small custom dial if you want polish).
3. **Zoom** — a **logarithmic** slider (`Slider::new(...).logarithmic(true)`).
   Operate it on **meters-per-pixel** (or ground span), not raw `zoom`. Conversions:
   `mpp = 0.111_320 / zoom` and `zoom = 0.111_320 / mpp`. Show next to it both
   `m/px` and the on-screen ground span `≈ mpp * dev_w` (format as m or km).
4. **Camera mode** — a Follow/Free toggle (radio or `selectable_value`) bound to
   `AppState.mode`.

### Data flow (keep it one-directional and simple)
- Hold editable mirrors in `SimGui`, e.g. `panel: PanelState { lat_deg: f64,
  lon_deg: f64, heading_deg: f32 }`, seeded from the initial fix in `SimGui::new`.
- Each frame, draw the panel binding widgets to those mirrors + `app.state.zoom` +
  `app.state.mode`.
- After drawing, push the mirrors into the sources of truth:
  `self.loc.set_position((lat_deg*1e6) as i32, (lon_deg*1e6) as i32)` and
  `self.loc.set_course(heading_deg)`. `app.state.zoom`/`app.state.mode` are bound
  directly (they're `pub`).
- `app.tick(&mut self.loc)` already runs in `render_to_texture`; in **Follow**
  mode that recenters the camera on the fix, so editing lat/lon pans the map. In
  **Free** mode the mouse owns the camera and the fix is just recorded.
- When the user clicks **Follow**, also good UX to snap the panel's lat/lon mirrors
  back from `app.state` so the two views agree.

### egui immediate-viewport mechanics (egui 0.29)
```rust
ctx.show_viewport_immediate(
    egui::ViewportId::from_hash_of("controls"),
    egui::ViewportBuilder::default().with_title("Controls").with_inner_size([280.0, 360.0]),
    |ctx, _class| {
        egui::CentralPanel::default().show(ctx, |ui| {
            // widgets here; capture &mut self fields freely (ctx is a fresh per-viewport ctx)
        });
    },
);
```
Call it every frame from `SimGui::update` (immediate viewports are re-declared
each frame). Mutating `self.*` inside the closure is fine — the viewport `ctx` is
not part of `self`. If the user closes this window, decide whether to also close
the app or just stop showing it (check `ctx.input(|i| i.viewport().close_requested())`
inside the closure if you want to react).

---

## 5. Build / run / verify

**Run cargo from `viewer-rs/`.** Sample maps (`monaco.obcm`, `luxemburg.obcm`,
`freiburg.obcm`, `malta.obcm`, `kz.obcm`) live at the **repo root**, so use
`../monaco.obcm` from `viewer-rs/`. No SDL env vars needed anymore.

```sh
cd viewer-rs
cargo build --release -p obcm-sim
cargo clippy --workspace --all-targets      # must stay clean
cargo test --workspace                       # obcm 12, obcm-app 6, obcm-sim 7

# Firmware-readiness (must keep compiling — this is the foundation guarantee):
rustup target add thumbv8m.main-none-eabihf
cargo build -p obcm-app --target thumbv8m.main-none-eabihf
```

**Verifying the GUI without a window manager in the loop** (you cannot screenshot
the live window via computer-use — see §6):
```sh
# Pixel-exact framebuffer dump (proves the render pipeline):
./target/release/obcm-sim ../monaco.obcm --png /tmp/a.png
# Live composited frame via egui's own capture (proves texture upload + display):
./target/release/obcm-sim ../monaco.obcm --screenshot /tmp/b.png
```
Then open/Read the PNG. `--screenshot` opens the real window, grabs frame 1, saves,
and exits. **The control panel is a second viewport, so `--screenshot` only
captures the main screen window** — to see the panel you'll need the user to look,
or extend the screenshot logic to the controls viewport.

A regression guard worth keeping: `--png` output for `monaco.obcm` was
**pixel-identical** (`max_channel_diff = 0`) to the pre-refactor binary in both
default (64-color) and `--true-color` modes. Don't break that.

---

## 6. Gotchas / lessons learned

- **egui/eframe 0.29.1 API specifics** (these bit us):
  - `ViewportCommand::Screenshot` is a **unit variant** (no `UserData` arg).
  - Screenshot result arrives as `egui::Event::Screenshot { viewport_id, image: Arc<ColorImage> }`.
  - Build a texture image with `egui::ColorImage::from_rgb([w, h], &rgb_bytes)`.
  - Edge-to-edge panel: `CentralPanel::default().frame(egui::Frame::none())`.
  - Image widget: `egui::Image::new(egui::load::SizedTexture::from_handle(&tex))
    .fit_to_exact_size(size).texture_options(egui::TextureOptions::NEAREST)
    .sense(egui::Sense::click_and_drag())`; upload with `TextureHandle::set(image, NEAREST)`.
  - Scroll delta: `ui.input(|i| i.smooth_scroll_delta.y)`.
- **No SDL anymore.** Do not re-add `embedded-graphics-simulator` or the Homebrew
  `LIBRARY_PATH`/`DYLD_LIBRARY_PATH` exports. The window stack is pure-Rust eframe.
- **Bash cwd drifts** between tool calls (a heredoc that `cd`s into repo root
  leaves you there). Always `cd viewer-rs` (or use absolute paths) before cargo.
- **Window-fit clipping:** at default `--scale 3` the device (240×320) is a 960pt-tall
  window that winit clamps to laptop screen height, cropping the bottom ~40pt *in
  the window* (the render is complete; `--png` proves it). **Good thing to fix in
  Step 4 layout** — e.g. fit the screen image to available height preserving aspect
  with integer snap, and/or lower the default scale once the two windows coexist.
- **computer-use can't see the window:** the simulator is an unbundled cargo binary,
  so it can't be added to the computer-use app allowlist, and the compositor filter
  hides non-allowlisted apps. `screencapture` is also blocked (no Screen Recording
  permission for the terminal). Use `--screenshot` instead.
- **`--screenshot` can't capture the Controls viewport.** Confirmed: eframe/egui
  0.29 does **not** deliver `Event::Screenshot` for *immediate* child viewports —
  sending `ViewportCommand::Screenshot` to the panel's `ctx` polls forever and
  never fires (an attempt to auto-capture it was tried and reverted). `--screenshot`
  captures only the root (screen) window. **The control panel must be verified by a
  live look** — `cargo run --release -p obcm-sim -- ../monaco.obcm` and inspect the
  second "Controls" window.
- **Continuous repaint:** `SimGui::update` ends with `ctx.request_repaint()` so the
  loop runs every frame (needed for GPX animation later). Keep it.
- Everything internal is **microdegrees `i32`**; `course` is degrees CW from north.

---

## 7. Decisions already locked (do not re-litigate)

- **GUI = egui/eframe native** (not a browser panel, not hand-rolled SDL widgets).
- **The panel drives a simulated GPS fix; Follow mode derives the camera from it.**
  (We deliberately did *not* build a separate "map-center" control — position and
  the future user-marker are the same thing.)
- **Shared logic lives in `obcm-app`** (a real `no_std` crate, not inlined in the sim).
- **Heading rides on `Fix.course`.** The device's magnetometer/compass (valid when
  stationary) is a *separate* sensor — add it as its own optional trait when the
  marker lands, not now.

---

## 8. Definition of done for Step 4

- Second "Controls" window with center lat/lon, heading, log zoom (with m/px +
  ground-span readout), Follow/Free toggle.
- Editing the panel moves the simulated GPS / camera as described; Follow vs Free
  behave correctly with the existing mouse pan/zoom.
- `#[allow(dead_code)]` removed from `SimLocationSource` (setters now used).
- `cargo clippy --workspace --all-targets` clean; `cargo test --workspace` green;
  `obcm-app` still builds for `thumbv8m.main-none-eabihf`; `monaco.obcm --png`
  still pixel-identical.
- Verified via `--screenshot` (main window) and, ideally, a quick manual look at
  the panel.

---

## 9. Roadmap after Step 4 (so your panel design stays aligned)

These are the features the panel is scaffolding for — design the panel so they slot in:

1. **User-position marker** — draw a triangle at `AppState.user_fix` oriented along
   `course`. `user_fix` is already updated every frame in both modes. Likely a new
   draw step after `render_frame` (host-side overlay) or inside the renderer.
2. **GPX replay** — a new `LocationSource` that walks a parsed track over wall-clock
   time and returns interpolated `Fix`es (position + derived course). Panel gets
   play/pause/seek. The app doesn't change — it's just a different source plugged in.
3. **Virtual buttons** — implement `InputSource` (host side: keyboard + on-screen
   buttons), then thread it into `App::tick` (extend the signature) and add device
   UI-state handling in `obcm-app`. Revisit the provisional `Button` enum against
   the real hardware.
4. **Compass trait** — separate optional heading source for stationary orientation,
   fused with `course` for the marker.

The firmware track (parallel, separate effort): allocation-free chunk decode
(`heapless`) for the MCU, then the nRF5340 front-end (embassy + LS021B7DD02 driver)
as a second host implementing real `LocationSource` / `InputSource` / `DrawTarget`.
