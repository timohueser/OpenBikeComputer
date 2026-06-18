//! The eframe host window — the device "screen".
//!
//! This is the desktop counterpart to the firmware's main loop: each frame it
//! polls the [`SimLocationSource`], advances the shared [`obcm_app::App`], renders
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
use obcm_reader::Reader;
use obcm_app::{App, AppState, CameraMode, Fix};
use obcm_route::RouteReader;

use crate::baro::BaroSensor;
use crate::device_input::DeviceInput;
use crate::framebuffer::Framebuffer;
use crate::gpx::Track;
use crate::gpx_player::GpxPlayer;
use crate::routes::RouteStore;
use crate::sim_location::SimLocationSource;
use crate::Args;

mod panel;
mod units;

/// The control panel's editable mirrors. The simulated GPS fix is stored in the
/// [`SimLocationSource`] as integer microdegrees + a `course`; egui widgets need
/// `&mut` floats, so the panel edits these and pushes them into the source each
/// frame (see [`SimGui::show_control_panel`]).
struct PanelState {
    lat_deg: f64,
    lon_deg: f64,
    heading_deg: f32,
}

/// Launch the simulator window. Owns the map bytes for the process lifetime; the
/// [`Reader`] is a cheap view rebuilt each frame over them.
pub fn run(bytes: Vec<u8>, args: Args) -> Result<(), eframe::Error> {
    let win = [(args.width * args.scale) as f32, (args.height * args.scale) as f32];
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("OBCM Simulator")
            .with_inner_size(win),
        ..Default::default()
    };
    eframe::run_native(
        "OBCM Simulator",
        options,
        Box::new(move |_cc| Ok(Box::new(SimGui::new(bytes, args)) as Box<dyn eframe::App>)),
    )
}

struct SimGui {
    /// Map file bytes; `Reader` borrows these each frame.
    bytes: Vec<u8>,
    app: App,
    /// The routes folder (the device-SD stand-in): the menu catalog + active geometry.
    store: RouteStore,
    loc: SimLocationSource,
    fb: Framebuffer,
    dev_w: u32,
    dev_h: u32,
    scale: u32,
    true_color: bool,
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
    last_stats: obcm_render::RenderStats,
    /// This frame's device-control keyboard state, read globally at the top of `update`
    /// (before any widget can take focus and swallow the keys), then applied in
    /// [`show_device_controls`](Self::show_device_controls). Turn is edge (detents this
    /// frame); the buttons are held state.
    kbd_turn: i32,
    kbd_enc: bool,
    kbd_back: bool,
}

impl SimGui {
    fn new(bytes: Vec<u8>, args: Args) -> Self {
        let (cx, cy, zoom) = {
            let reader = Reader::new(&bytes).expect("map validated in main()");
            crate::initial_camera(&reader, args.width)
        };
        let mut state = AppState::new(cx, cy, zoom);
        // Start in Free so the mouse drives the camera; the Follow toggle lands
        // with the control panel. The fix is still seeded (map center) so the loop
        // and the future user marker have something to track.
        state.mode = CameraMode::Free;
        // `--heading` opens in heading-up with that course; otherwise north-up.
        state.heading_up = args.heading.is_some();
        let loc = SimLocationSource::new(Some(Fix {
            lat: cy,
            lon: cx,
            course: args.heading,
            speed_mps: None,
        }));

        // Seed the panel mirrors from the initial fix so the widgets open showing
        // the device's actual starting position/heading.
        let panel = match loc.current() {
            Some(f) => PanelState {
                lat_deg: f.lat as f64 / 1e6,
                lon_deg: f.lon as f64 / 1e6,
                heading_deg: f.course.unwrap_or(0.0),
            },
            None => PanelState { lat_deg: 0.0, lon_deg: 0.0, heading_deg: 0.0 },
        };

        // `--boot` opens at the device's Home/Idle state to walk the full flow;
        // otherwise the sim opens on the map (its map-viewer default).
        let app = if args.boot { App::new_idle(state) } else { App::new(state) };
        let store = RouteStore::open(args.routes_dir.clone().unwrap_or_else(|| "routes".to_string()));
        let mut gui = SimGui {
            app,
            store,
            loc,
            fb: Framebuffer::new(args.width, args.height),
            dev_w: args.width,
            dev_h: args.height,
            scale: args.scale,
            true_color: args.true_color,
            panel,
            input: DeviceInput::new(),
            gpx: None,
            baro: BaroSensor::new(),
            gpx_label: None,
            gpx_error: None,
            quit: false,
            texture: None,
            screenshot: args.screenshot,
            screenshot_requested: false,
            bytes,
            last_stats: obcm_render::RenderStats::default(),
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

    /// Parse a GPX file and load it as the active replay (paused at the start),
    /// or record the error for the panel to show.
    fn load_gpx(&mut self, path: &Path) {
        match Track::load(path) {
            Ok(track) => {
                let player = GpxPlayer::new(track);
                let name =
                    path.file_name().and_then(|n| n.to_str()).unwrap_or("track.gpx").to_string();
                self.gpx_label = Some(format!(
                    "{name} — {} pts, {}",
                    player.point_count(),
                    units::format_clock(player.duration())
                ));
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
        let reader = Reader::new(&self.bytes).expect("map validated in main()");
        let tc = self.true_color;

        // Open the active route's geometry *before* ticking, so the map-matcher gets it
        // (reloads only when the selection changes; the firmware does the same off its SD
        // card). It stays borrowed through `tick` + `render_frame` below.
        self.store.sync_active(self.app.activity.active_route);
        let route_src = self.store.active_source();
        let route = route_src.as_ref().and_then(|s| RouteReader::open(s).ok());

        // Drive the app from whichever location source is active. A loaded GPX replay
        // takes over from the manual panel fix (just as the device's GPS would); we
        // advance it by this frame's wall-clock time before ticking, and feed the
        // barometer the track's elevation on its own (asynchronous) cadence.
        if let Some(player) = self.gpx.as_mut() {
            // Advance + tick on the playback clock (shared with the headless replay).
            let dt = ctx.input(|i| i.stable_dt) as f64;
            crate::replay_step(&mut self.app, player, &mut self.baro, dt, route.as_ref());
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
            self.app.tick(now_ms, &mut self.loc, None, route.as_ref());
        }

        let stats = self.app.render_frame(
            &mut self.fb,
            &reader,
            route.as_ref(),
            self.dev_w as f32,
            self.dev_h as f32,
            |c| crate::color_of(c, tc),
        );
        self.last_stats = stats;

        let image =
            egui::ColorImage::from_rgb([self.dev_w as usize, self.dev_h as usize], self.fb.as_rgb888());
        let opts = egui::TextureOptions::NEAREST;
        match &mut self.texture {
            Some(t) => t.set(image, opts),
            None => self.texture = Some(ctx.load_texture("screen", image, opts)),
        }
    }

    /// Apply mouse pan/scroll-zoom over the screen `rect`, switching to Free mode.
    /// `scale` is the *displayed* device-pixels-to-screen-points factor (the image
    /// is fit to the window, so it can differ from the requested `--scale`).
    fn handle_camera_input(
        &mut self,
        ui: &egui::Ui,
        resp: &egui::Response,
        rect: egui::Rect,
        scale: f32,
    ) {
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
}

impl eframe::App for SimGui {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Read the device-control keyboard shortcuts *first*, before any widget (the
        // device-screen image, the panel) is laid out and can take focus and swallow the
        // keys — so they drive the encoder/Back whether the screen or the control panel is
        // focused. Turn keys are consumed (one detent per press); the Enter/Backspace
        // button state is the live held state. Applied in `show_device_controls`.
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
                        eprintln!("imported {} | {} km, +{} m", path.display(), (s.total_distance_m + 500) / 1000, s.total_ascent_m);
                    }
                    Err(e) => self.gpx_error = Some(e),
                }
                self.app.set_routes(self.store.catalog());
            }
        }

        self.render_to_texture(ctx);

        // No frame margin: the device screen fills its window edge-to-edge.
        egui::CentralPanel::default().frame(egui::Frame::none()).show(ctx, |ui| {
            let tex = self.texture.as_ref().expect("texture uploaded this frame");
            // Fit the device image to the window at the largest *integer* scale
            // that fits (kept ≥1 so the pixel grid stays crisp, capped at the
            // requested `--scale`). This avoids winit clipping the bottom when the
            // requested scale makes the window taller than the screen.
            let avail = ui.available_size();
            let fit = (avail.x / self.dev_w as f32).min(avail.y / self.dev_h as f32);
            let disp_scale = fit.floor().clamp(1.0, self.scale as f32);
            let size = egui::vec2(self.dev_w as f32 * disp_scale, self.dev_h as f32 * disp_scale);
            let resp = ui.add(
                egui::Image::new(egui::load::SizedTexture::from_handle(tex))
                    .fit_to_exact_size(size)
                    .texture_options(egui::TextureOptions::NEAREST)
                    .sense(egui::Sense::click_and_drag()),
            );
            let rect = resp.rect;
            self.handle_camera_input(ui, &resp, rect, disp_scale);
        });

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
