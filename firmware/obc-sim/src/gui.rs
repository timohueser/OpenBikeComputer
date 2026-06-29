//! The eframe host window — the device "screen".
//!
//! This is the desktop counterpart to the firmware's main loop: each frame it
//! polls the [`SimLocationSource`], advances the shared [`obc_app::App`], renders
//! the firmware-identical path into a [`Framebuffer`], and blits that buffer to a
//! GPU texture shown at integer scale (nearest-neighbor, so the device's hard
//! pixel grid stays crisp). The firmware does the same with a real GPS driver and
//! the LS021B7DD02 panel instead of these two host pieces.
//!
//! Mouse drag pans and scroll zooms (about the cursor); both switch the camera to
//! [`CameraMode::Free`]. The second "Controls" viewport that drives the simulated
//! GPS fix and the emulated encoder lives in [`panel`]; the pure zoom / formatting
//! helpers it needs live in [`units`].

use std::path::Path;

use eframe::egui;
use obc_app::{App, AppState, Button, CameraMode, Dirty, Fix, InputClock, RideClock, Sensors, SettingsStore};
use obc_reader::{MapCache, MapTables, Reader, SliceSource};
use obc_route::{RouteIndex, RouteReader};

use obc_replay::{gpx::Track, BaroSensor, GpxPlayer};

use crate::device_input::DeviceInput;
use crate::framebuffer::Framebuffer;
use crate::present::Present;
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

/// The control panel's editable mirrors. The simulated GPS fix is stored in the
/// [`SimLocationSource`] as integer microdegrees + a `course`; egui widgets need
/// `&mut` floats, so the panel edits these and pushes them into the source each
/// frame (see [`SimGui::show_control_panel`]).
struct PanelState {
    lat_deg: f64,
    lon_deg: f64,
    heading_deg: f32,
    /// The "Compass" slider — the magnetometer heading used to orient a heading-up map while
    /// the rider is stopped (when the GPS course drops to `None`). Pushed into [`SimCompass`].
    compass_deg: f32,
}

/// In-progress 1:1 size calibration: a reference bar is drawn in the device window;
/// the user measures it with a ruler and types the millimetres here. `Some` while the
/// calibration screen is up (see [`SimGui::show_calibration`]).
#[derive(Default)]
struct CalibState {
    measured_mm: String,
}

/// Launch the simulator window. Owns the map bytes for the process lifetime; the
/// [`Reader`] is a cheap view rebuilt each frame over them.
#[cfg(not(target_arch = "wasm32"))]
pub fn run(bytes: Vec<u8>, args: Args) -> Result<(), eframe::Error> {
    // The window wraps the whole device (housing + screen + a little backdrop) at
    // `--scale`, not just the screen, so the body has room around the framebuffer.
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

/// Web entry point: mount the *same* `SimGui` app on the page's `<canvas>` via
/// eframe's WebGL runner. Called from the wasm `main` (see `main.rs`). The app is
/// identical to the native sim and to the firmware's render path, so the project
/// site's embedded demo stays current with the code rather than drifting like a
/// screenshot.
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

struct SimGui {
    /// Map file bytes; `Reader` borrows these each frame.
    bytes: Vec<u8>,
    /// The immutable map tables (style table + LOD pyramid), parsed once at startup and borrowed by
    /// the cheap per-frame `Reader` — mirroring the device, which parses them once at boot (#179).
    map_tables: MapTables,
    /// The streamed-map cache (issue #37), kept for the whole session and reused across frames —
    /// exactly as the device holds one in its reserved region. A persistent cache lets a chunk read one frame
    /// hit the next, so the "Map SD" stats track real device behaviour (a panned-into view warms
    /// up, then settles to 100% hit) rather than the cold ≤75% a per-frame cache would show.
    map_cache: MapCache,
    app: App,
    /// The routes folder (the device-SD stand-in): the menu catalog + active geometry.
    store: RouteStore,
    /// The tracks folder (device-SD `/tracks` stand-in): the in-progress `.obct` ride log +
    /// saved `.gpx` files. Reconciled to the app's tracking session each frame.
    tracks: TrackStore,
    /// The persisted-settings store (device-RRAM stand-in): seeds the app at boot and is written
    /// whenever the settings screens change something, so settings survive a relaunch.
    settings_store: FileSettingsStore,
    loc: SimLocationSource,
    fb: Framebuffer,
    /// The self-diffing present backend (epic #199 / issue #200): diffs each rendered frame against
    /// a per-row hash store, pushes only the changed spans into its own buffer (uploaded to the
    /// texture), and runs an exact-diff oracle. Its [`stats`](Present::stats) feed the panel.
    present: Present,
    dev_w: u32,
    dev_h: u32,
    scale: u32,
    true_color: bool,
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
    /// The loaded GPX replay, if any. When `Some`, it drives the fix instead of
    /// the manual [`SimLocationSource`] (the device's GPS would likewise override
    /// any manual override). `None` = manual control via the panel sliders.
    gpx: Option<GpxPlayer>,
    /// Simulated barometer, fed the replay's elevation on its own cadence (asynchronous
    /// to the GPS fix) — the device's pressure altimeter stand-in.
    baro: BaroSensor,
    /// Simulated electronic compass (the panel's "Compass" slider) — sets the heading-up
    /// orientation while stopped, when the GPS has no course. The device's magnetometer stand-in.
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
    /// The shared render-on-demand dirty signal ([`App::take_dirty`], issue #47), drained
    /// once per frame and shown in the stats panel. The sim redraws continuously (it also
    /// animates host chrome and replays GPX), so this is informational — a live readout of the
    /// signal the firmware gates its map/overlay renders on. (Mouse pan/zoom mutates the camera
    /// outside the app's input path, so it isn't reflected; on the device every camera change
    /// goes through a gesture or a fix, which is.)
    last_dirty: Dirty,
    /// The device body color drawn by the housing chrome. Switchable live in the
    /// control panel; defaults to slate (or `--colorway`). Purely cosmetic host chrome.
    colorway: Colorway,
    /// This frame's device-control keyboard state, read globally at the top of `update`
    /// (before any widget can take focus and swallow the keys), then folded into the
    /// on-housing controls in [`show_device_image`](Self::show_device_image). Turn is
    /// edge (detents this frame); the buttons are held state.
    kbd_turn: i32,
    kbd_enc: bool,
    kbd_back: bool,
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
        // Start in Free so the mouse drives the camera; the Follow toggle lands
        // with the control panel. The fix is still seeded (map center) so the loop
        // and the future user marker have something to track.
        state.mode = CameraMode::Free;
        // `--heading` opens in heading-up with that course; otherwise north-up.
        state.heading_up = args.heading.is_some();
        let loc = SimLocationSource::new(Some(Fix { lat: cy, lon: cx, course: args.heading, speed_mps: None }));

        // Seed the panel mirrors from the initial fix so the widgets open showing
        // the device's actual starting position/heading.
        let panel = match loc.current() {
            Some(f) => PanelState {
                lat_deg: f.lat as f64 / 1e6,
                lon_deg: f.lon as f64 / 1e6,
                heading_deg: f.course.unwrap_or(0.0),
                compass_deg: f.course.unwrap_or(0.0),
            },
            None => PanelState { lat_deg: 0.0, lon_deg: 0.0, heading_deg: 0.0, compass_deg: 0.0 },
        };

        // Boot at the device's real power-on state (Home / Idle, no route): pressing
        // the encoder walks Home → Route menu → Map, exactly like the device. (The
        // headless `--png` path and the web demo open straight on the map instead.)
        let mut app = if args.start_on_map { App::new(state) } else { App::new_idle(state) };
        if let Some(seed) = args.home_seed {
            app.reseed_home(seed);
        }
        let store = RouteStore::open(args.routes_dir());
        let tracks = TrackStore::open(args.tracks_dir());
        // Seed the live settings from the persisted store (the device's RRAM stand-in), falling
        // back to defaults on a first run / unreadable file — exactly the device's boot path.
        let mut settings_store = FileSettingsStore::open(args.settings_path());
        app.set_settings(settings_store.load().unwrap_or_default());
        // Load any saved 1:1 calibration; `--physical` only takes effect if we have one,
        // and `--calibrate` opens the calibration screen straight away.
        let points_per_mm = crate::calib::load();
        let physical = args.physical && points_per_mm.is_some();
        // Housing body color: `--colorway NAME`, else the slate default.
        let colorway = args.colorway.as_deref().and_then(Colorway::from_label).unwrap_or(Colorway::Slate);
        let mut gui = SimGui {
            app,
            store,
            tracks,
            settings_store,
            loc,
            fb: Framebuffer::new(args.width, args.height),
            present: Present::new(args.width, args.height),
            dev_w: args.width,
            dev_h: args.height,
            scale: args.scale,
            true_color: args.true_color,
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
            colorway,
            kbd_turn: 0,
            kbd_enc: false,
            kbd_back: false,
        };
        // Hand the Route menu its catalog (the folder scan); refreshed on GPX import.
        gui.app.set_routes(gui.store.catalog());
        // `--gpx` opens with a track loaded (paused at the start); press play in
        // the panel to replay it.
        if let Some(path) = &args.gpx {
            gui.load_gpx(Path::new(path));
        }
        gui
    }

    /// Web constructor: build the sim from the demo map baked into the wasm binary
    /// and web-flavoured defaults (in-memory route/track stores, no native file
    /// dialog or 1:1 calibration). Everything below this — the app, the render path,
    /// the control panel — is shared verbatim with the native sim.
    #[cfg(target_arch = "wasm32")]
    pub(crate) fn new_web() -> Self {
        let bytes = include_bytes!("../assets/grimsel.obcm").to_vec();
        let mut g = SimGui::new(bytes, crate::Args::web_default());

        // Select the embedded demo route (catalog index 0) so its line + ride stats show,
        // and open a tracking session so the breadcrumb + climb totals accumulate.
        if !g.store.catalog().is_empty() {
            g.app.activity.active_route = Some(0);
            g.app.activity.start_session();
        }
        // Auto-play the embedded ride (the Grimselpass climb, Guttannen → summit) so the
        // page opens on a moving map. `render_to_texture` restarts it at the summit (see there).
        if let Ok(track) = Track::parse(include_str!("../assets/grimsel-climb.gpx")) {
            let mut player = GpxPlayer::new(track);
            // The GPX is distance-timed at a ~12 km/h base, so the multiplier reads as
            // "N× a normal climbing pace"; 3× keeps the map moving without a blur.
            player.set_speed(3.0);
            player.play();
            g.gpx = Some(player);
        }
        // Follow the rider heading-up (map rotates so travel is always up), and tighten the
        // fit-to-whole-tile zoom to a riding view so the route's switchbacks are visible.
        g.app.state.mode = CameraMode::Follow;
        g.app.state.heading_up = true;
        g.app.state.zoom *= 12.0;
        g
    }

    /// Parse a GPX file and load it as the active replay (paused at the start),
    /// or record the error for the panel to show. Only reachable from native entry
    /// points (CLI `--gpx`, file-dialog); the web build has no loader wired yet.
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

    /// Run the shared app for one frame into the framebuffer, then upload it.
    fn render_to_texture(&mut self, ctx: &egui::Context) {
        // Reuse the session-long cache (see the field doc) — the same cross-frame reuse the
        // device gets, so the "Map SD" stats panel mirrors on-glass behaviour.
        let map_src = SliceSource(&self.bytes);
        let reader = Reader::new(&map_src, &self.map_tables, &self.map_cache);
        let tc = self.true_color;

        // Open the active route's geometry *before* ticking, so the map-matcher gets it
        // (reloads only when the selection changes; the firmware does the same off its SD
        // card). It stays borrowed through `tick` + `render_frame` below.
        self.store.sync_active(self.app.activity.active_route);
        let route_src = self.store.active_source();
        let route_index = route_src.as_ref().and_then(|s| RouteIndex::read(s).ok());
        let route = match (route_index.as_ref(), route_src.as_ref()) {
            (Some(idx), Some(s)) => Some(RouteReader::new(idx, s)),
            _ => None,
        };

        // Reconcile the ride log to the app's tracking session (open / finalise-to-GPX /
        // discard) before ticking — the device does the same off its SD card.
        crate::reconcile_tracks(&mut self.app, &mut self.tracks);

        // Drive the app from whichever location source is active. A loaded GPX replay
        // takes over from the manual panel fix (just as the device's GPS would); we
        // advance it by this frame's wall-clock time before ticking, and feed the
        // barometer the track's elevation on its own (asynchronous) cadence.
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
            // Web demo: restart the climb when it reaches the summit so the page stays
            // "alive". It's point-to-point (not a loop), so also bump the tracking session
            // to clear the breadcrumb + ride totals — the rider snaps back to Guttannen for
            // a fresh lap instead of dragging a trail across the map.
            #[cfg(target_arch = "wasm32")]
            if !player.is_playing() {
                player.play();
                self.app.activity.start_session();
            }
            // Reflect the replayed fix in the panel mirrors so the (disabled)
            // position/heading widgets show the live values, and so manual control
            // resumes from here if the track is ejected.
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
                compass: Some(&mut self.compass),
                track: self.tracks.sink(),
                // The battery is set once from `--battery` (default 75 %); no live sim gauge.
                fuel: None,
            };
            self.app.tick(RideClock(now_ms), sensors, route.as_ref());
        }

        // Time the whole frame draw (render + route/overlays) and fold it into the stats
        // as `render_us` — `obc-render` is no_std and clockless, so the host fills it (the
        // device will use the DWT cycle counter). Surfaced in the control panel's stats.
        let t0 = web_time::Instant::now();
        let mut stats =
            self.app.render_frame(&mut self.fb, &reader, route.as_ref(), self.dev_w as f32, self.dev_h as f32, |c| {
                crate::color_of(c, tc)
            });
        stats.render_us = t0.elapsed().as_micros() as u32;
        self.last_stats = stats;
        // Drain the shared dirty signal for the stats readout. The sim renders unconditionally
        // (above), so this doesn't gate drawing — it just surfaces what the firmware *would*
        // have re-rendered this frame, so the render-on-demand logic can be watched live.
        self.last_dirty = self.app.take_dirty();

        // Present through the self-diffing backend (epic #199 / issue #200): it diffs the rendered
        // frame against its per-row hash store, pushes only the changed spans into its own buffer
        // (asserting an exact full-frame diff agrees), and hands that buffer back. Uploading *it* —
        // reconstructed from partial pushes, not a whole-frame copy — means a diff bug would show as
        // a stale row on glass. The push metric (`present.stats`) lands in the render-stats panel.
        let presented = self.present.present(self.fb.as_rgb888());
        let image = egui::ColorImage::from_rgb([self.dev_w as usize, self.dev_h as usize], presented);
        let opts = egui::TextureOptions::NEAREST;
        match &mut self.texture {
            Some(t) => t.set(image, opts),
            None => self.texture = Some(ctx.load_texture("screen", image, opts)),
        }
    }

    /// Apply mouse pan/scroll-zoom over the screen `rect`, switching to Free mode.
    /// `scale` is the *displayed* device-pixels-to-screen-points factor (the image
    /// is fit to the window, so it can differ from the requested `--scale`).
    /// Native-only: the web demo disables screen pan/zoom (no touchscreen feel).
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

    /// Draw the device — the housing chrome plus the framebuffer blitted into its
    /// screen cutout — centred, at either the integer fit scale (default) or the
    /// panel's true physical size when 1:1 is on and calibrated.
    fn show_device_image(&mut self, ctx: &egui::Context) {
        // Native frames the device in the reference render's charcoal backdrop; the web
        // demo drops it (transparent) so the device sits straight on the page background.
        #[cfg(not(target_arch = "wasm32"))]
        let frame = egui::Frame::none().fill(housing::background());
        #[cfg(target_arch = "wasm32")]
        let frame = egui::Frame::none().fill(egui::Color32::TRANSPARENT);
        egui::CentralPanel::default().frame(frame).show(ctx, |ui| {
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

            // The device's own controls live *on the housing* now: click the encoder /
            // Back, or scroll over the wheel to turn it. Hit-test their rects, fold in the
            // keyboard (read at the top of `update`), and run the shared recognizer — the
            // same path the firmware uses with real GPIO.
            let enc = ui.interact(lo.encoder, egui::Id::new("dev_encoder"), egui::Sense::click());
            let back = ui.interact(lo.back, egui::Id::new("dev_back"), egui::Sense::click());
            let enc = enc.on_hover_cursor(egui::CursorIcon::PointingHand);
            let back = back.on_hover_cursor(egui::CursorIcon::PointingHand);
            if enc.hovered() {
                let dy = ui.input(|i| i.smooth_scroll_delta.y);
                if dy != 0.0 {
                    self.input.scroll(dy);
                }
            }
            self.input.turn(self.kbd_turn);
            let enc_down = enc.is_pointer_button_down_on() || self.kbd_enc;
            let back_down = back.is_pointer_button_down_on() || self.kbd_back;
            self.input.set_button(Button::Encoder, enc_down);
            self.input.set_button(Button::Back, back_down);
            let now = self.input.now_ms();
            self.app.handle_input(InputClock(now), &mut self.input);
            // Persist settings the moment a settings screen changes one (the device's
            // save-on-dirty path) so they survive a relaunch.
            if self.app.take_settings_dirty() {
                self.settings_store.save(self.app.settings());
            }

            // Mirror the live control state onto the housing so the encoder/Back animate.
            // The knurl eases toward the new angle so each detent reads as a little turn.
            let knob_angle =
                ui.ctx().animate_value_with_time(egui::Id::new("knurl_phase"), self.input.knob_angle(), 0.12);
            let ctrl = housing::ControlVisual { knob_angle, encoder_down: enc_down, back_down };
            let palette = self.colorway.palette();

            // Paint the housing, then blit the framebuffer into its screen rect, corners
            // rounded to follow the bezel (revealing it behind). Clone the painter so the
            // borrow of `ui` is released before `ui.put` takes it.
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
            // Native: mouse drag pans / scroll zooms the map. The web demo is
            // encoder-driven only — no screen pan/zoom, so it never feels like a
            // touchscreen (the device has none).
            #[cfg(not(target_arch = "wasm32"))]
            self.handle_camera_input(ui, &resp, resp.rect, disp_scale);
            #[cfg(target_arch = "wasm32")]
            let _ = &resp;
        });
    }

    /// The 1:1 calibration screen: draw a reference bar of a known point-width; the user
    /// measures it with a ruler and types the length → points-per-mm. Saved (so it's
    /// one-time) and 1:1 switches on. `calib` is taken out of `self` so the egui closure
    /// borrows only locals.
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
        // Either way the window wraps the whole device (housing + screen + backdrop), so
        // the body fits around the framebuffer.
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
    // Web: clear the canvas to transparent so the page background shows around the
    // device (the central panel is transparent there too — see `show_device_image`).
    #[cfg(target_arch = "wasm32")]
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        egui::Color32::TRANSPARENT.to_normalized_gamma_f32()
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Read the device-control keyboard shortcuts *first*, before any widget can take
        // focus and swallow the keys — so they drive the encoder/Back whether the screen or
        // the control panel is focused. Turn keys are consumed (one detent per press); the
        // Enter/Backspace state is the live held state. Applied in `show_device_image`.
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

        // Drag-and-drop a `.gpx` onto the window to import it (the device's USB-drop
        // path): convert into the routes folder and refresh the Route-menu catalog.
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
                self.app.set_routes(self.store.catalog());
            }
        }

        self.render_to_texture(ctx);

        // The device window shows either the live screen or the size-calibration UI.
        if self.calib.is_some() {
            self.show_calibration(ctx);
        } else {
            self.show_device_image(ctx);
        }
        self.apply_physical_resize(ctx);

        // The Controls window is a development tool (a second OS window driving the
        // simulated GPS / encoder). The web demo shows only the device itself —
        // housing, screen and on-housing buttons — so it's native-only.
        #[cfg(not(target_arch = "wasm32"))]
        self.show_control_panel(ctx);

        if self.screenshot.is_some() {
            self.run_screenshot(ctx);
        }

        // Closing the Controls window quits the simulator (otherwise a controls-less
        // window lingers with no way to drive the fix).
        if self.quit {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }

        // Drive the loop continuously so control-panel / GPX changes show up
        // without needing a mouse event to wake the window.
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
    /// `--screenshot` flow: request a capture of the composited viewport on the
    /// first frame, save it when egui delivers it next frame, then close. This
    /// captures what the GPU actually displays (texture upload + draw), not just
    /// the framebuffer the headless `--png` path dumps.
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
