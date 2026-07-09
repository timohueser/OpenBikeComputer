//! The eframe host window — the device "screen".
//!
//! The desktop counterpart to the firmware's main loop: each frame it polls the
//! [`SimLocationSource`], advances the shared [`obc_app::App`], renders the
//! firmware-identical path into the [`Present`] backend's resident device-64 frame, pushes it
//! through the [`obc_platform::DisplayDriver`] seam (the same seam the firmware presents through),
//! and blits the reconstructed texture to a GPU texture at integer scale (nearest-neighbor, so the
//! pixel grid stays crisp). The firmware does the same with a real GPS driver and the LS021B7DD02
//! panel. Because the GUI now goes through the device-64 backend, the interactive window shows the
//! panel's true 64-colour gamut; `--true-color` (the un-quantized reference) stays on the headless
//! `--png` path.
//!
//! The second "Controls" viewport (the simulated GPS fix + emulated encoder) lives
//! in [`panel`]; its zoom / formatting helpers live in [`units`].

use std::path::Path;

use eframe::egui;
use embedded_graphics::pixelcolor::{raw::RawU16, Rgb565};
use obc_app::{App, AppState, Button, CameraMode, Dirty, Fix, InputClock, RideClock, Sensors, SettingsStore};
use obc_platform::{DisplayDriver, FbDevice64};
use obc_reader::{MapCache, MapTables, Reader, SliceSource};
use obc_route::{RouteIndex, RouteReader};

use obc_replay::{gpx::Track, BaroSensor, GpxPlayer};

use crate::device_input::DeviceInput;
use crate::present::Present;
use crate::rides::RideStore;
use crate::routes::RouteStore;
use crate::settings_store::FileSettingsStore;
use crate::sim_compass::SimCompass;
use crate::sim_location::SimLocationSource;
use crate::track::TrackStore;
use crate::Args;

mod housing;
mod panel;
mod units;

use housing::Colorway;

/// The GPX playback time (seconds) a guided-tour baseline seeks to: mid the demo route's first climb
/// (~progress 5 km on `grimsel-climb.gpx`), so a climb is active for "See the climb ahead" and the
/// map sits in the switchbacks. The map-matcher re-locks from this teleport within a few frames, so
/// no fast-forward from the start is needed.
#[cfg(target_arch = "wasm32")]
const TOUR_BASELINE_S: f64 = 1500.0;

/// The control panel's editable mirrors. The [`SimLocationSource`] stores the fix as
/// integer microdegrees + `course`; egui widgets need `&mut` floats, so the panel edits
/// these and pushes them into the source each frame.
struct PanelState {
    lat_deg: f64,
    lon_deg: f64,
    heading_deg: f32,
    /// The "Compass" slider — the magnetometer heading orienting a heading-up map while the
    /// rider is stopped (GPS course drops to `None`). Pushed into [`SimCompass`].
    compass_deg: f32,
    /// The injected BLE link state (epic #447): the host→app seam the sim drives from the control
    /// panel, pushed into the app each frame via [`obc_app::App::set_ble_status`]. It's the whole
    /// [`obc_app::BleStatus`] — not just a `connected` bool — so P2/P4 can add passkey / upload
    /// injection by extending this field and its widgets with no restructuring.
    ble: obc_app::BleStatus,
    /// The "Inject upload" combo's selected catalog row (epic #447, P4) — which route the panel's
    /// upload-injection buttons duplicate (new) or rewrite (replace-by-id).
    upload_sel: usize,
}

/// In-progress 1:1 size calibration: the user measures the on-screen reference bar and
/// types the millimetres here. `Some` while the calibration screen is up.
#[derive(Default)]
struct CalibState {
    measured_mm: String,
}

/// Launch the simulator window. Owns the map bytes for the process lifetime; the
/// [`Reader`] is a cheap view rebuilt each frame over them.
#[cfg(not(target_arch = "wasm32"))]
pub fn run(bytes: Vec<u8>, args: Args) -> Result<(), eframe::Error> {
    // The window wraps the whole device (housing + screen + a little backdrop) at `--scale`,
    // so the body has room around the framebuffer.
    let dev = housing::HousingStyle::default().window_size_px(egui::vec2(args.width as f32, args.height as f32));
    let win = [dev.x * args.scale as f32, dev.y * args.scale as f32];
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_title("OBC Simulator").with_inner_size(win),
        ..Default::default()
    };
    eframe::run_native(
        "OBC Simulator",
        options,
        Box::new(move |_cc| Ok(Box::new(SimGui::new(bytes, args)) as Box<dyn eframe::App>)),
    )
}

/// Web entry point: mount the *same* `SimGui` app on the page's `<canvas>` via eframe's
/// WebGL runner. The app is identical to the native sim and the firmware's render path,
/// so the site's embedded demo can't drift from the code.
#[cfg(target_arch = "wasm32")]
pub fn run_web() {
    use eframe::wasm_bindgen::JsCast as _;

    // Surface Rust panics in the browser console instead of an opaque trap.
    console_error_panic_hook::set_once();
    eframe::WebLogger::init(log::LevelFilter::Warn).ok();

    wasm_bindgen_futures::spawn_local(async {
        let document = eframe::web_sys::window().expect("no window").document().expect("no document");
        let canvas = document
            .get_element_by_id("device_canvas")
            .expect("index.html is missing <canvas id=\"device_canvas\">")
            .dyn_into::<eframe::web_sys::HtmlCanvasElement>()
            .expect("#device_canvas is not a <canvas>");

        let result = eframe::WebRunner::new()
            .start(
                canvas,
                eframe::WebOptions::default(),
                Box::new(|_cc| Ok(Box::new(SimGui::new_web()) as Box<dyn eframe::App>)),
            )
            .await;

        // Clear the "loading…" placeholder once egui owns the canvas (or show why not).
        if let Some(el) = document.get_element_by_id("loading_text") {
            match result {
                Ok(_) => el.remove(),
                Err(e) => el.set_inner_html(&format!("<p>Failed to start: {e:?}</p>")),
            }
        }
    });
}

/// One frame's device-control hit-test, produced while the housing is drawn
/// ([`SimGui::show_device_image`]) and consumed by [`SimGui::apply_device_input`]. Keeps the draw
/// free of input side effects: it only reports geometry, the caller drives the recognizer.
struct DeviceHit {
    /// Encoder pressed this frame (housing hit-test OR the Enter key).
    enc_down: bool,
    /// Back pressed this frame (housing hit-test OR the Backspace key).
    back_down: bool,
    /// Encoder-wheel scroll delta — non-zero only while the wheel was hovered (zero otherwise).
    scroll_dy: f32,
}

struct SimGui {
    /// Map file bytes; `Reader` borrows these each frame.
    bytes: Vec<u8>,
    /// The immutable map tables (style table + LOD pyramid), parsed once at startup and borrowed
    /// by the cheap per-frame `Reader` — mirroring the device, which parses them once at boot.
    map_tables: MapTables,
    /// The streamed-map cache, kept for the whole session and reused across frames (as the device
    /// holds one in its reserved region). Cross-frame reuse lets a panned-into view warm to 100%
    /// hit, so the "Map SD" stats track real device behaviour rather than a cold ≤75%.
    map_cache: MapCache,
    app: App,
    /// The routes folder (the device-SD stand-in): the menu catalog + active geometry.
    store: RouteStore,
    /// The tracks folder as the **ride catalog** (device-SD `/tracks` stand-in): the `RD{id}.ORD`
    /// summaries + synced flags the Rides screen lists (#454). Rescanned when a ride is saved/deleted.
    ride_store: RideStore,
    /// The tracks folder (device-SD `/tracks` stand-in): the `.obct` ride log + saved `RD{id}.ORD` /
    /// `.gpx`. Reconciled to the app's tracking session each frame.
    tracks: TrackStore,
    /// The persisted-settings store (device-RRAM stand-in): seeds the app at boot, written on
    /// each settings change so they survive a relaunch.
    settings_store: FileSettingsStore,
    loc: SimLocationSource,
    /// The simulator's [`obc_platform::DisplayDriver`] backend: the app renders the whole frame into
    /// its resident device-64 plane ([`fb_mut`](DisplayDriver::fb_mut)), then [`present`] self-diffs
    /// it (pushing only the changed spans into the uploaded texture) under an exact-diff oracle. Its
    /// [`stats`](Present::stats) feed the panel.
    present: Present,
    dev_w: u32,
    dev_h: u32,
    scale: u32,
    /// Saved display calibration (egui points per millimetre), or `None` until the
    /// user calibrates. Loaded from / written to [`crate::calib`].
    points_per_mm: Option<f32>,
    /// Render the device at the panel's true physical size (needs `points_per_mm`).
    physical: bool,
    /// Snap the window to the device's 1:1 size next frame (set when 1:1 turns on).
    physical_resize_pending: bool,
    /// `Some` while the size-calibration screen is shown instead of the device image.
    calib: Option<CalibState>,
    /// Last calibration-save error, surfaced in the control panel.
    calib_error: Option<String>,
    /// Editable mirrors for the control-panel widgets.
    panel: PanelState,
    /// Emulated device controls (encoder knob + Back) → shared gesture recognizer.
    input: DeviceInput,
    /// The loaded GPX replay, if any. When `Some`, it drives the fix instead of the manual
    /// [`SimLocationSource`] (as the device's GPS would). `None` = manual panel control.
    gpx: Option<GpxPlayer>,
    /// Simulated barometer (device altimeter stand-in), fed the replay's elevation on its own
    /// cadence (asynchronous to the GPS fix).
    baro: BaroSensor,
    /// Simulated compass (device magnetometer stand-in) — the panel's "Compass" slider, orienting
    /// a heading-up map while stopped when the GPS has no course.
    compass: SimCompass,
    /// A short "name — N pts, M:SS" status line for the loaded track.
    gpx_label: Option<String>,
    /// The last GPX load error, shown in the panel until the next successful load.
    gpx_error: Option<String>,
    /// Set when the Controls window is closed; quits the whole app next frame.
    quit: bool,
    texture: Option<egui::TextureHandle>,
    /// `--screenshot`: save the first composited frame here, then close.
    screenshot: Option<String>,
    screenshot_requested: bool,
    last_stats: obc_render::RenderStats,
    /// The shared render-on-demand dirty signal ([`App::take_dirty`]), drained once per frame for
    /// the stats panel. The sim always redraws, so this is informational — a live readout of the
    /// signal the firmware gates its renders on. (Mouse pan/zoom bypasses the app's input path, so
    /// it isn't reflected; on the device every camera change goes through a gesture or a fix.)
    last_dirty: Dirty,
    /// An in-flight route plan (#499): the resumable planner, stepped **once per frame** in
    /// [`render_to_texture`] so the GUI stays interactive while a route computes — exactly the
    /// board's one-step-per-pass shape. `None` when nothing is planning; a drained cancel
    /// (Back on the planning screen) simply drops it — the sim's sink is in-memory, so there is
    /// no partial file to delete.
    nav_plan: Option<crate::NavPlan>,
    /// The device body color drawn by the housing chrome. Switchable in the control panel.
    colorway: Colorway,
    /// This frame's device-control keyboard state, read at the top of `update` (before a widget
    /// can take focus and swallow the keys), then folded into the on-housing controls. Turn is
    /// edge (detents this frame); the buttons are held state.
    kbd_turn: i32,
    kbd_enc: bool,
    kbd_back: bool,
    /// Web guided-tour mode (the landing page's feature demos): when set, the ambient auto-restart
    /// loop is suppressed so the page's demo engine controls playback. Only ever true on wasm; the
    /// native build never toggles it (the field stays for a single unconditional initializer).
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    tour_active: bool,
}

impl SimGui {
    fn new(bytes: Vec<u8>, args: Args) -> Self {
        let map_tables = MapTables::parse(&SliceSource(&bytes)).expect("map validated in main()");
        let (cx, cy, zoom) = {
            let cache = MapCache::new();
            let src = SliceSource(&bytes);
            let reader = Reader::new(&src, &map_tables, &cache);
            crate::initial_camera(&reader, args.width)
        };
        let mut state = AppState::new(cx, cy, zoom);
        if let Some(b) = args.battery {
            state.battery_pct = b;
        }
        // Start in Free so the mouse drives the camera; the fix is still seeded (map center)
        // so the loop and user marker have something to track.
        state.mode = CameraMode::Free;
        state.heading_up = args.heading.is_some();
        let loc = SimLocationSource::new(Some(Fix { lat: cy, lon: cx, course: args.heading, speed_mps: None }));

        // Seed the panel mirrors from the initial fix so the widgets open at the real position.
        let panel = match loc.current() {
            Some(f) => PanelState {
                lat_deg: f.lat as f64 / 1e6,
                lon_deg: f.lon as f64 / 1e6,
                heading_deg: f.course.unwrap_or(0.0),
                compass_deg: f.course.unwrap_or(0.0),
                ble: obc_app::BleStatus::DISCONNECTED,
                upload_sel: 0,
            },
            None => PanelState {
                lat_deg: 0.0,
                lon_deg: 0.0,
                heading_deg: 0.0,
                compass_deg: 0.0,
                ble: obc_app::BleStatus::DISCONNECTED,
                upload_sel: 0,
            },
        };

        // Boot at the device's real power-on state (Home / Idle, no route) unless `start_on_map`
        // (the headless `--png` path and web demo open straight on the map).
        let mut app = if args.start_on_map { App::new(state) } else { App::new_idle(state) };
        if let Some(seed) = args.home_seed {
            app.reseed_home(seed);
        }
        let store = RouteStore::open(args.routes_dir());
        let ride_store = RideStore::open(args.tracks_dir());
        let tracks = TrackStore::open(args.tracks_dir());
        // Seed the live settings from the persisted store, falling back to defaults on a first
        // run / unreadable file — the device's boot path.
        let mut settings_store = FileSettingsStore::open(args.settings_path());
        app.set_settings(settings_store.load().unwrap_or_default());
        // Mirror the map's §8.6 routing-profile names into the app for the Bike-type screen +
        // created-route overview label (N5). The map is loaded once in the sim, so this is a one-shot
        // (a device re-runs it on every map load).
        app.set_nav_profiles(map_tables.nav_profiles());
        // `--physical` only takes effect with a saved calibration; `--calibrate` opens the screen.
        let points_per_mm = crate::calib::load();
        let physical = args.physical && points_per_mm.is_some();
        let colorway = args.colorway.as_deref().and_then(Colorway::from_label).unwrap_or(Colorway::Coral);
        let mut gui = SimGui {
            app,
            store,
            ride_store,
            tracks,
            settings_store,
            loc,
            present: Present::new(args.width, args.height),
            dev_w: args.width,
            dev_h: args.height,
            scale: args.scale,
            points_per_mm,
            physical,
            physical_resize_pending: physical,
            calib: args.calibrate.then(CalibState::default),
            calib_error: None,
            panel,
            input: DeviceInput::new(),
            gpx: None,
            baro: BaroSensor::new(),
            compass: SimCompass::new(),
            gpx_label: None,
            gpx_error: None,
            quit: false,
            texture: None,
            screenshot: args.screenshot,
            screenshot_requested: false,
            bytes,
            map_tables,
            map_cache: MapCache::new(),
            last_stats: obc_render::RenderStats::default(),
            last_dirty: Dirty::CLEAN,
            nav_plan: None,
            colorway,
            kbd_turn: 0,
            kbd_enc: false,
            kbd_back: false,
            tour_active: false,
        };
        gui.app.set_routes_with_ids(gui.store.catalog(), gui.store.ids());
        gui.app.set_rides(gui.ride_store.catalog(), gui.ride_store.ids());
        // `--gpx` opens with a track loaded, paused at the start.
        if let Some(path) = &args.gpx {
            gui.load_gpx(Path::new(path));
        }
        gui
    }

    /// Web constructor: build the sim from the demo map baked into the wasm binary and
    /// web-flavoured defaults (in-memory stores, no file dialog or 1:1 calibration).
    /// Everything below — app, render path, control panel — is shared with the native sim.
    #[cfg(target_arch = "wasm32")]
    pub(crate) fn new_web() -> Self {
        let bytes = include_bytes!("../assets/grimsel.obcm").to_vec();
        let mut g = SimGui::new(bytes, crate::Args::web_default());

        // Select the embedded demo route and open a session so its line + ride stats show.
        if !g.store.catalog().is_empty() {
            g.app.activity.active_route = Some(0);
            g.app.activity.start_session();
        }
        // Auto-play the embedded ride so the page opens on a moving map. `render_to_texture`
        // restarts it at the summit (see there).
        if let Ok(track) = Track::parse(include_str!("../assets/grimsel-climb.gpx")) {
            let mut player = GpxPlayer::new(track);
            // 3× a normal climbing pace keeps the map moving without a blur.
            player.set_speed(3.0);
            player.play();
            g.gpx = Some(player);
        }
        g.app.state.mode = CameraMode::Follow;
        g.app.state.heading_up = true;
        // Tighten the fit-to-whole-tile zoom to a riding view so the switchbacks are visible.
        g.app.state.zoom *= 12.0;
        g
    }

    /// Drain the landing page's guided-tour command queue and apply each command to the live sim —
    /// gestures through the app's deterministic [`apply_gesture`](obc_app::App::apply_gesture) seam,
    /// playback through the shared [`GpxPlayer`], and `enter`/`exit` toggling the demo baseline.
    #[cfg(target_arch = "wasm32")]
    fn apply_tour_cmds(&mut self) {
        use crate::web_api::TourCmd;
        for cmd in crate::web_api::drain_cmds() {
            match cmd {
                TourCmd::Gesture(g) => self.app.apply_gesture(g),
                TourCmd::Play => {
                    if let Some(p) = self.gpx.as_mut() {
                        p.play();
                    }
                }
                TourCmd::Pause => {
                    if let Some(p) = self.gpx.as_mut() {
                        p.pause();
                    }
                }
                TourCmd::Seek(t) => {
                    if let Some(p) = self.gpx.as_mut() {
                        p.seek(t);
                    }
                }
                TourCmd::Enter => self.enter_tour_baseline(),
                TourCmd::Exit => self.exit_tour(),
                TourCmd::Ambient => self.enter_ambient(),
            }
        }
    }

    /// Enter guided-tour mode and reset to a clean demo baseline. Rebuilds the app to a fresh
    /// `[Home, Map]` riding session on the demo route so a previous demo can't leak in — a created
    /// reroute activates a new route, and an interrupted loop can leave the stack deep in a menu.
    /// Uses **Manual** climb mode: the Climb screen stays in the Back-cycle (so "See the climb ahead"
    /// can walk Map → Statistics → Climb) but Auto's auto-switch never yanks the starting screen off
    /// the Map mid-climb — the demos need a deterministic Map baseline. Seeks the ride to a mid-climb
    /// spot (so a climb is active and the map sits in the switchbacks); the matcher re-locks within a
    /// few frames, so no long fast-forward is needed.
    #[cfg(target_arch = "wasm32")]
    fn enter_tour_baseline(&mut self) {
        use obc_app::settings::{ClimbMode, Settings};

        self.tour_active = true;

        // Same opening camera + riding setup as `new_web`, rebuilt from scratch each entry.
        let (cx, cy, zoom) = {
            let src = SliceSource(&self.bytes);
            let reader = Reader::new(&src, &self.map_tables, &self.map_cache);
            crate::initial_camera(&reader, self.dev_w)
        };
        let mut state = AppState::new(cx, cy, zoom * 12.0);
        state.mode = CameraMode::Follow;
        state.heading_up = true;
        let mut app = App::new(state);
        app.set_nav_profiles(self.map_tables.nav_profiles());
        app.set_routes_with_ids(self.store.catalog(), self.store.ids());
        app.set_settings(Settings { climb_mode: ClimbMode::Manual, ..Settings::default() });
        if !self.store.catalog().is_empty() {
            app.activity.active_route = Some(0);
            app.activity.start_session();
        }
        self.app = app;

        if let Some(p) = self.gpx.as_mut() {
            p.seek(TOUR_BASELINE_S);
            p.play();
        }
    }

    /// Leave guided-tour mode ("take control"): hand the device back to the visitor where the demo
    /// left it (they keep driving with the keys / buttons) and restore the ambient auto-restart.
    #[cfg(target_arch = "wasm32")]
    fn exit_tour(&mut self) {
        self.tour_active = false;
        if let Some(p) = self.gpx.as_mut() {
            p.play();
        }
    }

    /// Reset to the ambient "just riding" state — the live Grimsel climb the page opens on, with the
    /// visitor's controls enabled (`tour_active = false`, so the manual buttons drive it and the
    /// summit auto-restart runs). Rebuilds the app like [`enter_tour_baseline`] but under **default**
    /// settings (Auto climb mode, as the page shipped) and seeks the ride back to the start, so the
    /// first carousel page ("Ride it yourself") always lands on a clean live ride — even after a demo
    /// rebuilt the app onto a computed reroute.
    #[cfg(target_arch = "wasm32")]
    fn enter_ambient(&mut self) {
        use obc_app::settings::Settings;

        self.tour_active = false;

        let (cx, cy, zoom) = {
            let src = SliceSource(&self.bytes);
            let reader = Reader::new(&src, &self.map_tables, &self.map_cache);
            crate::initial_camera(&reader, self.dev_w)
        };
        let mut state = AppState::new(cx, cy, zoom * 12.0);
        state.mode = CameraMode::Follow;
        state.heading_up = true;
        let mut app = App::new(state);
        app.set_nav_profiles(self.map_tables.nav_profiles());
        app.set_routes_with_ids(self.store.catalog(), self.store.ids());
        app.set_settings(Settings::default());
        if !self.store.catalog().is_empty() {
            app.activity.active_route = Some(0);
            app.activity.start_session();
        }
        self.app = app;

        if let Some(p) = self.gpx.as_mut() {
            p.seek(0.0);
            p.play();
        }
    }

    /// Parse a GPX file and load it as the active replay (paused at the start), or record the
    /// error for the panel. Native-only (CLI `--gpx`, file-dialog).
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    fn load_gpx(&mut self, path: &Path) {
        match Track::load(path) {
            Ok(track) => {
                let player = GpxPlayer::new(track);
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("track.gpx").to_string();
                self.gpx_label =
                    Some(format!("{name} — {} pts, {}", player.point_count(), units::format_clock(player.duration())));
                self.gpx = Some(player);
                self.gpx_error = None;
            }
            Err(e) => {
                self.gpx = None;
                self.gpx_label = None;
                self.gpx_error = Some(e);
            }
        }
    }

    /// Run the shared app for one frame into the backend's device-64 frame, present it through the
    /// seam, then upload the reconstructed texture.
    fn render_to_texture(&mut self, ctx: &egui::Context) {
        // Reuse the session-long cache (see the field doc) so the "Map SD" stats mirror glass.
        let map_src = SliceSource(&self.bytes);
        let reader = Reader::new(&map_src, &self.map_tables, &self.map_cache);

        // Feed the host→app BLE seam (epic #447): the control panel's injected link state, pushed
        // every frame exactly as the board's ride loop feeds its `ble::state` snapshot. Cheap and
        // idempotent — an unchanged status repaints nothing.
        self.app.set_ble_status(self.panel.ble);

        // Drain a hold-to-delete request from the Route menu (epic #447 P6) *before* the store is
        // borrowed for geometry: delete the route file and re-feed the id-carrying catalog — the
        // same rescan sequence the panel's "Store changed" button runs, so the app's P3 remap keeps
        // `active_route` + the menu highlight on the right routes. (The device routes this through
        // `ObjectStore`; the sim deletes the file directly.)
        if let Some(id) = self.app.take_route_delete() {
            if self.store.delete_by_id(id) {
                self.app.set_routes_with_ids(self.store.catalog(), self.store.ids());
            }
        }

        // Drain a hold-to-delete request from the Rides screen (#454): delete the `RD{id}.ORD` +
        // sidecar flag and re-feed the ride catalog, mirroring the route delete above (the device
        // routes this through `ObjectStore`; the sim deletes the file directly).
        if let Some(id) = self.app.take_ride_delete() {
            if self.ride_store.delete_by_id(id) {
                self.app.set_rides(self.ride_store.catalog(), self.ride_store.ids());
            }
        }

        // The resumable route planner (#499). A drained create-route request starts a plan; a
        // drained cancel (Back on the planning screen) drops it (in-memory sink — nothing to
        // delete); otherwise the in-flight plan runs **one bounded step this frame**, keeping
        // the GUI fully interactive (the spinner animates, input works) while the route
        // computes — the board's one-step-per-pass shape. A terminal outcome commits + answers
        // before `sync_active` below, so a successful plan's activated route streams open this
        // same frame.
        if let Some(req) = self.app.take_nav_request() {
            // Plan under the rider's bike-type setting (N5 §8.6); the router falls back to profile 0
            // for an index past the map's profile count.
            self.nav_plan = Some(crate::NavPlan::start(&req, self.app.settings().bike_profile_idx));
        }
        if self.app.take_nav_cancel() {
            self.nav_plan = None;
        }
        let step = self.nav_plan.as_mut().map(|plan| {
            let map_src = SliceSource(&self.bytes);
            let nav_reader = Reader::new(&map_src, &self.map_tables, &self.map_cache);
            plan.step(&nav_reader)
        });
        match step {
            None | Some(obc_route::Step::Running) => {}
            Some(obc_route::Step::Done(stats)) => {
                let plan = self.nav_plan.take().expect("just stepped it");
                crate::finish_nav_plan(&mut self.app, &mut self.store, Ok(stats), plan.bytes(), plan.tile_stats());
            }
            Some(obc_route::Step::Failed(e)) => {
                let plan = self.nav_plan.take().expect("just stepped it");
                crate::finish_nav_plan(&mut self.app, &mut self.store, Err(e), plan.bytes(), plan.tile_stats());
            }
        }

        // A ride finishing this frame writes a fresh `RD{id}.ORD` — rescan the tracks folder and
        // re-feed the Rides menu so it appears without a relaunch (the device's store-changed rescan).
        let ride_saved = self.app.activity.has_track_action();

        // Open the active route's geometry *before* ticking so the map-matcher gets it (reloads
        // only on selection change). It stays borrowed through `tick` + `render_frame` below.
        self.store.sync_active(self.app.activity.active_route);
        let route_src = self.store.active_source();
        let route_index = route_src.as_ref().and_then(|s| RouteIndex::read(s).ok());
        let route = match (route_index.as_ref(), route_src.as_ref()) {
            (Some(idx), Some(s)) => Some(RouteReader::new(idx, s)),
            _ => None,
        };

        // Reconcile the ride log to the app's tracking session before ticking.
        crate::reconcile_tracks(&mut self.app, &mut self.tracks);
        // If that reconcile just finalised a ride, rescan the tracks folder and re-feed the Rides
        // menu so the new `RD{id}.ORD` shows up live (#454).
        if ride_saved {
            self.ride_store.rescan();
            self.app.set_rides(self.ride_store.catalog(), self.ride_store.ids());
        }

        // Drive the app from whichever location source is active. A loaded GPX replay takes over
        // from the manual panel fix (as the device's GPS would).
        if let Some(player) = self.gpx.as_mut() {
            // Advance + tick on the playback clock (shared with the headless replay).
            let dt = ctx.input(|i| i.stable_dt) as f64;
            crate::replay_step(
                &mut self.app,
                player,
                &mut self.baro,
                Some(&mut self.compass),
                dt,
                route.as_ref(),
                self.tracks.sink(),
            );
            // Web demo: restart the climb at the summit so the page stays alive. It's
            // point-to-point, so bump the tracking session to clear the breadcrumb + totals
            // (a fresh lap instead of dragging a trail across the map). Suppressed while a guided
            // tour is running — the page's demo engine owns playback + baseline resets then, so an
            // ambient restart mid-demo would fight it (a `start_session` could reset progress off the
            // climb the "See the climb ahead" demo is standing on).
            #[cfg(target_arch = "wasm32")]
            if !self.tour_active && !player.is_playing() {
                player.play();
                self.app.activity.start_session();
            }
            // Reflect the replayed fix in the panel mirrors, so manual control resumes from
            // here if the track is ejected.
            if let Some(f) = self.app.state.user_fix {
                self.panel.lat_deg = f.lat as f64 / 1e6;
                self.panel.lon_deg = f.lon as f64 / 1e6;
                if let Some(c) = f.course {
                    self.panel.heading_deg = c;
                }
            }
        } else {
            // Manual panel control: no barometer, wall-clock for any moving-time.
            self.baro.clear();
            let now_ms = self.input.now_ms();
            let sensors = Sensors {
                loc: &mut self.loc,
                altimeter: None,
                // No thermometer in manual control — BMP581 temperature is device-only.
                temperature: None,
                // No GPS time in the sim — the clock stays whatever was set by hand.
                clock: None,
                compass: Some(&mut self.compass),
                track: self.tracks.sink(),
                // Battery is set once from `--battery`; no live sim gauge.
                fuel: None,
            };
            self.app.tick(RideClock(now_ms), sensors, route.as_ref());
        }

        // Drain the Bluetooth screen's Forget-phone request (epic #447, P8): the sim's "bond" is
        // the injected panel flag, so forgetting just clears it — the next seam feed shows
        // Paired: no, exactly as the board's RRAM clear + status publish would.
        if self.app.take_ble_forget() {
            self.panel.ble.paired = false;
        }

        // Time the whole frame draw into `render_us` (`obc-render` is clockless, so the host
        // fills it; the device uses the DWT cycle counter). Render the whole frame straight into the
        // backend's resident device-64 plane — the device's own color path (`Rgb565` → device-64
        // pack), exactly as the firmware's map plane draws into its `FbDevice64`.
        let t0 = web_time::Instant::now();
        let (dev_w, dev_h) = (self.dev_w, self.dev_h);
        let mut fbdev = FbDevice64::new(self.present.fb_mut(), dev_w, dev_h);
        let mut stats = self.app.render_frame(&mut fbdev, &reader, route.as_ref(), dev_w as f32, dev_h as f32, |c| {
            Rgb565::from(RawU16::new(c))
        });
        stats.render_us = t0.elapsed().as_micros() as u32;
        self.last_stats = stats;
        // Drain the shared dirty signal for the stats readout (the sim always redraws, so this
        // doesn't gate drawing).
        self.last_dirty = self.app.take_dirty();

        // Present through the seam: the backend self-diffs the resident frame and pushes only the
        // changed spans into its reconstructed texture (under the exact-diff oracle). Uploading *that*
        // — not a whole-frame copy — means a diff bug shows as a stale row on glass, not just a failed
        // assert. The async seam completes synchronously on the host, driven by a minimal block-on.
        pollster::block_on(self.present.present(None));
        let image = egui::ColorImage::from_rgb([dev_w as usize, dev_h as usize], self.present.texture());
        let opts = egui::TextureOptions::NEAREST;
        match &mut self.texture {
            Some(t) => t.set(image, opts),
            None => self.texture = Some(ctx.load_texture("screen", image, opts)),
        }
    }

    /// Apply mouse pan/scroll-zoom over the screen `rect`, switching to Free mode. `scale` is
    /// the *displayed* device-pixels-to-screen-points factor (the image is fit to the window,
    /// so it can differ from the requested `--scale`). Native-only.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    fn handle_camera_input(&mut self, ui: &egui::Ui, resp: &egui::Response, rect: egui::Rect, scale: f32) {
        let (w, h) = (self.dev_w as f32, self.dev_h as f32);
        let st = &mut self.app.state;

        if resp.dragged() {
            let d = resp.drag_delta();
            let dpx = d.x / scale;
            let dpy = d.y / scale;
            let vp = st.viewport(w, h);
            // Convert the screen-space drag into a map delta through the inverse
            // projection (`to_map`), so panning follows the cursor even when the
            // view is rotated (heading-up) — a fixed `cam ± dx/zoom` would drift.
            let (lon0, lat0) = vp.to_map(w / 2.0, h / 2.0);
            let (lon1, lat1) = vp.to_map(w / 2.0 - dpx, h / 2.0 - dpy);
            st.cam_lon = st.cam_lon.wrapping_add(lon1.wrapping_sub(lon0));
            st.cam_lat = st.cam_lat.wrapping_add(lat1.wrapping_sub(lat0));
            st.mode = CameraMode::Free;
        }

        let scroll = ui.input(|i| i.smooth_scroll_delta.y);
        if scroll != 0.0 {
            if let Some(pos) = resp.hover_pos() {
                // Cursor in device pixels, the space `Viewport::to_map` expects.
                let local = pos - rect.min;
                let px = (local.x / scale).clamp(0.0, w);
                let py = (local.y / scale).clamp(0.0, h);
                let new_zoom = (st.zoom * (scroll * 0.005).exp()).clamp(units::MIN_ZOOM, units::MAX_ZOOM);

                // Keep the ground point under the cursor fixed across the zoom.
                let (olon, olat) = st.viewport(w, h).to_map(px, py);
                st.zoom = new_zoom;
                let (nlon, nlat) = st.viewport(w, h).to_map(px, py);
                st.cam_lon = st.cam_lon.wrapping_add(olon.wrapping_sub(nlon));
                st.cam_lat = st.cam_lat.wrapping_add(olat.wrapping_sub(nlat));
                st.mode = CameraMode::Free;
            }
        }
    }

    /// Draw the device — housing chrome plus the framebuffer blitted into its screen cutout —
    /// centred, at either the integer fit scale (default) or the panel's true physical size
    /// when 1:1 is on and calibrated. Reports this frame's device-control hit-test ([`DeviceHit`])
    /// so the caller can fold it into the shared input recognizer via [`apply_device_input`]; the
    /// drawing itself has no input side effects.
    #[must_use]
    fn show_device_image(&mut self, ctx: &egui::Context) -> DeviceHit {
        // Native frames the device in a charcoal backdrop; the web demo is transparent so the
        // device sits straight on the page background.
        #[cfg(not(target_arch = "wasm32"))]
        let frame = egui::Frame::none().fill(housing::background());
        #[cfg(target_arch = "wasm32")]
        let frame = egui::Frame::none().fill(egui::Color32::TRANSPARENT);
        egui::CentralPanel::default()
            .frame(frame)
            .show(ctx, |ui| {
                let tex = self.texture.clone().expect("texture uploaded this frame");
                let style = housing::HousingStyle::default();
                let screen = egui::vec2(self.dev_w as f32, self.dev_h as f32);
                let disp_scale = match (self.physical, self.points_per_mm) {
                    // 1:1 — points per device pixel so 240 px spans the panel's real width.
                    // Fractional on purpose (the size isn't a whole multiple of the grid);
                    // NEAREST keeps it crisp at the cost of a slightly uneven pixel grid.
                    (true, Some(ppm)) => (crate::calib::PANEL_W_MM * ppm / self.dev_w as f32).max(0.05),
                    // Otherwise: largest integer scale at which the whole *device* fits,
                    // capped at `--scale`, ≥1 — keeps the screen at a crisp whole multiple.
                    _ => {
                        let avail = ui.available_size();
                        let dev = style.device_size_px(screen);
                        let fit = (avail.x / dev.x).min(avail.y / dev.y);
                        fit.floor().clamp(1.0, self.scale as f32)
                    }
                };
                let lo = style.layout(ui.available_rect_before_wrap(), disp_scale, screen);

                // The device controls live on the housing: click the encoder / Back, or scroll over
                // the wheel to turn it. Hit-test their rects here (drawing only); the keyboard fold-in
                // and shared recognizer run in `apply_device_input` from the returned `DeviceHit`.
                let enc = ui.interact(lo.encoder, egui::Id::new("dev_encoder"), egui::Sense::click());
                let back = ui.interact(lo.back, egui::Id::new("dev_back"), egui::Sense::click());
                let enc = enc.on_hover_cursor(egui::CursorIcon::PointingHand);
                let back = back.on_hover_cursor(egui::CursorIcon::PointingHand);
                // Wheel scroll is only picked up while hovering the encoder (zero otherwise), matching
                // the inline behaviour; the delta is applied in `apply_device_input`.
                let scroll_dy = if enc.hovered() { ui.input(|i| i.smooth_scroll_delta.y) } else { 0.0 };
                let enc_down = enc.is_pointer_button_down_on() || self.kbd_enc;
                let back_down = back.is_pointer_button_down_on() || self.kbd_back;

                // Mirror the live control state onto the housing. The knurl eases toward the current
                // angle so each detent reads as a little turn.
                let knob_angle =
                    ui.ctx().animate_value_with_time(egui::Id::new("knurl_phase"), self.input.knob_angle(), 0.12);
                let ctrl = housing::ControlVisual { knob_angle, encoder_down: enc_down, back_down };
                let palette = self.colorway.palette();

                // Paint the housing, then blit the framebuffer into its screen rect, corners rounded
                // to follow the bezel. Clone the painter so `ui`'s borrow is released before `ui.put`.
                let painter = ui.painter().clone();
                housing::draw(&painter, &lo, &style, &palette, &ctrl);
                let resp = ui.put(
                    lo.screen,
                    egui::Image::new(egui::load::SizedTexture::from_handle(&tex))
                        .fit_to_exact_size(lo.screen.size())
                        .texture_options(egui::TextureOptions::NEAREST)
                        .rounding(egui::Rounding::same(style.screen_radius_pts(disp_scale)))
                        .sense(egui::Sense::click_and_drag()),
                );
                // Native: mouse drag pans / scroll zooms. The web demo is encoder-driven only — no
                // screen pan/zoom, so it never feels like a touchscreen (the device has none).
                #[cfg(not(target_arch = "wasm32"))]
                self.handle_camera_input(ui, &resp, resp.rect, disp_scale);
                #[cfg(target_arch = "wasm32")]
                let _ = &resp;

                DeviceHit { enc_down, back_down, scroll_dy }
            })
            .inner
    }

    /// Fold a frame's device-control hit-test ([`show_device_image`]'s [`DeviceHit`]) into the
    /// shared input recognizer — the same path the firmware runs with real GPIO — and persist
    /// settings on the dirty edge. Split out of the draw so drawing reports geometry only; the same
    /// events reach [`handle_input`](obc_app::App::handle_input) in the same order, with the same
    /// coordinates, they did inline.
    fn apply_device_input(&mut self, hit: DeviceHit) {
        // Encoder-wheel scroll → detents (non-zero only when the wheel was hovered this frame).
        if hit.scroll_dy != 0.0 {
            self.input.scroll(hit.scroll_dy);
        }
        self.input.turn(self.kbd_turn);
        self.input.set_button(Button::Encoder, hit.enc_down);
        self.input.set_button(Button::Back, hit.back_down);
        let now = self.input.now_ms();
        self.app.handle_input(InputClock(now), &mut self.input);
        // Persist on the settings-dirty edge (the device's save-on-dirty path).
        if self.app.take_settings_dirty() {
            self.settings_store.save(self.app.settings());
        }
    }

    /// The 1:1 calibration screen: draw a reference bar of a known point-width; the user
    /// measures it and types the length → points-per-mm. `calib` is taken out of `self` so the
    /// egui closure borrows only locals.
    fn show_calibration(&mut self, ctx: &egui::Context) {
        let Some(mut calib) = self.calib.take() else { return };
        let mut save_ppm: Option<f32> = None;
        let mut cancel = false;
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.add_space(24.0);
            ui.vertical_centered(|ui| {
                ui.heading("Actual-size calibration");
                ui.add_space(8.0);
                ui.label("Hold a ruler to the screen, measure the bar between the two ticks,");
                ui.label("then type its length. Saved once and reused on every launch.");
                ui.add_space(22.0);

                // Reference bar: a known width in points (clamped to the window). The user
                // measures its physical length, so points-per-mm = drawn width / mm.
                let bar_w = crate::calib::REF_BAR_POINTS.min(ui.available_width() - 48.0).max(60.0);
                let (rect, _) = ui.allocate_exact_size(egui::vec2(bar_w, 34.0), egui::Sense::hover());
                let p = ui.painter_at(rect);
                let col = ui.visuals().strong_text_color();
                let y = rect.center().y;
                p.line_segment([egui::pos2(rect.left(), y), egui::pos2(rect.right(), y)], egui::Stroke::new(3.0, col));
                for x in [rect.left(), rect.right()] {
                    p.line_segment(
                        [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
                        egui::Stroke::new(2.0, col),
                    );
                }

                ui.add_space(22.0);
                ui.horizontal(|ui| {
                    ui.label("Measured length:");
                    ui.add(egui::TextEdit::singleline(&mut calib.measured_mm).desired_width(70.0));
                    ui.label("mm");
                });
                let parsed = calib.measured_mm.trim().parse::<f32>().ok().filter(|v| v.is_finite() && *v > 1.0);

                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if ui.add_enabled(parsed.is_some(), egui::Button::new("Save")).clicked() {
                        save_ppm = parsed.map(|mm| bar_w / mm);
                    }
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                });
                if parsed.is_none() && !calib.measured_mm.trim().is_empty() {
                    ui.colored_label(egui::Color32::from_rgb(220, 80, 80), "enter a length in mm (> 1)");
                }
            });
        });

        match save_ppm {
            Some(ppm) => match crate::calib::save(ppm) {
                Ok(()) => {
                    self.points_per_mm = Some(ppm);
                    self.physical = true;
                    self.physical_resize_pending = true;
                    self.calib_error = None;
                    // `calib` stays taken (None) → leave the calibration screen.
                }
                Err(e) => {
                    self.calib_error = Some(e);
                    self.calib = Some(calib); // keep the screen up to retry
                }
            },
            None if !cancel => self.calib = Some(calib), // still editing
            None => {}                                   // cancelled → leave the screen
        }
    }

    /// Snap the device window to match the current mode once, when 1:1 is toggled:
    /// the panel's true size in physical mode, the `--scale` default otherwise.
    fn apply_physical_resize(&mut self, ctx: &egui::Context) {
        if !std::mem::take(&mut self.physical_resize_pending) {
            return;
        }
        let dev = housing::HousingStyle::default().window_size_px(egui::vec2(self.dev_w as f32, self.dev_h as f32));
        let size = match (self.physical, self.points_per_mm) {
            (true, Some(ppm)) => {
                let s = crate::calib::PANEL_W_MM * ppm / self.dev_w as f32;
                egui::vec2(dev.x * s, dev.y * s)
            }
            // 1:1 off → back to the requested `--scale` window.
            _ => egui::vec2(dev.x * self.scale as f32, dev.y * self.scale as f32),
        };
        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(size));
    }
}

impl eframe::App for SimGui {
    // Web: clear the canvas to transparent so the page background shows around the device.
    #[cfg(target_arch = "wasm32")]
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        egui::Color32::TRANSPARENT.to_normalized_gamma_f32()
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Read the device-control keyboard shortcuts *first*, before a widget can take focus and
        // swallow the keys. Turn keys are consumed (one detent per press); Enter/Backspace is the
        // live held state. Applied in `show_device_image`.
        let (kt, ke, kb) = ctx.input_mut(|i| {
            let mut t = 0;
            if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowRight)
                || i.consume_key(egui::Modifiers::NONE, egui::Key::CloseBracket)
                || i.consume_key(egui::Modifiers::NONE, egui::Key::Period)
            {
                t += 1;
            }
            if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowLeft)
                || i.consume_key(egui::Modifiers::NONE, egui::Key::OpenBracket)
                || i.consume_key(egui::Modifiers::NONE, egui::Key::Comma)
            {
                t -= 1;
            }
            (t, i.key_down(egui::Key::Enter), i.key_down(egui::Key::Backspace))
        });
        self.kbd_turn = kt;
        self.kbd_enc = ke;
        self.kbd_back = kb;

        // Drag-and-drop a `.gpx` onto the window to import it (the device's USB-drop path).
        let dropped: Vec<std::path::PathBuf> =
            ctx.input(|i| i.raw.dropped_files.iter().filter_map(|f| f.path.clone()).collect());
        for path in dropped {
            let is_gpx = path.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("gpx"));
            if is_gpx {
                match self.store.import_gpx(&path) {
                    Ok(s) => {
                        self.gpx_error = None;
                        eprintln!(
                            "imported {} | {} km, +{} m",
                            path.display(),
                            (s.total_distance_m + 500) / 1000,
                            s.total_ascent_m
                        );
                    }
                    Err(e) => self.gpx_error = Some(e),
                }
                self.app.set_routes_with_ids(self.store.catalog(), self.store.ids());
            }
        }

        // Drain the landing page's guided-tour command queue before the frame ticks, so a demo's
        // gesture / playback / baseline commands land this frame (the app's real state machine + the
        // real route planner run in `render_to_texture` below, exactly as an on-device input would).
        #[cfg(target_arch = "wasm32")]
        self.apply_tour_cmds();

        self.render_to_texture(ctx);

        // Publish the top screen's name for the page's demo engine to poll (it advances a step only
        // once the sim reached the target screen — no fixed sleeps, and it waits out the planner).
        #[cfg(target_arch = "wasm32")]
        crate::web_api::publish_screen(self.app.top_screen().name());

        // The device window shows either the live screen or the size-calibration UI. Drawing the
        // device only reports its hit-test; folding that into the input recognizer + saving
        // settings happens right after, out of the draw.
        if self.calib.is_some() {
            self.show_calibration(ctx);
        } else {
            let hit = self.show_device_image(ctx);
            self.apply_device_input(hit);
        }
        self.apply_physical_resize(ctx);

        // The Controls window is a native-only development tool; the web demo shows only the device.
        #[cfg(not(target_arch = "wasm32"))]
        self.show_control_panel(ctx);

        if self.screenshot.is_some() {
            self.run_screenshot(ctx);
        }

        // Closing the Controls window quits (otherwise a controls-less window lingers with no
        // way to drive the fix).
        if self.quit {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }

        // Repaint continuously so control-panel / GPX changes show without a mouse event.
        ctx.request_repaint();
    }
}

/// Save an egui `ColorImage` (the captured frame) to a PNG.
fn save_color_image(img: &egui::ColorImage, path: &str) -> Result<(), String> {
    let (w, h) = (img.size[0] as u32, img.size[1] as u32);
    let mut rgba = Vec::with_capacity((w * h * 4) as usize);
    for p in &img.pixels {
        rgba.extend_from_slice(&[p.r(), p.g(), p.b(), p.a()]);
    }
    image::RgbaImage::from_raw(w, h, rgba)
        .ok_or_else(|| "screenshot size mismatch".to_string())?
        .save(path)
        .map_err(|e| format!("screenshot save failed: {e}"))
}

impl SimGui {
    /// `--screenshot` flow: request a viewport capture on the first frame, save it when egui
    /// delivers it next frame, then close. Captures what the GPU actually displays (texture
    /// upload + draw), not just the framebuffer the headless `--png` path dumps.
    fn run_screenshot(&mut self, ctx: &egui::Context) {
        if !self.screenshot_requested {
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot);
            self.screenshot_requested = true;
            return;
        }
        let shot = ctx.input(|i| {
            i.events.iter().find_map(|e| match e {
                egui::Event::Screenshot { image, .. } => Some(image.clone()),
                _ => None,
            })
        });
        if let Some(image) = shot {
            if let Some(path) = &self.screenshot {
                match save_color_image(&image, path) {
                    Ok(()) => eprintln!("wrote {path}"),
                    Err(e) => eprintln!("{e}"),
                }
            }
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}
