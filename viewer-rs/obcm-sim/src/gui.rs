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
//! [`CameraMode::Free`]. A second "Controls" viewport (see [`SimGui::show_control_panel`])
//! drives the simulated GPS fix — position, heading, zoom — and toggles Follow/Free.

use std::path::Path;

use eframe::egui;
use obcm_reader::Reader;
use obcm_app::{App, AppState, Button, CameraMode, Fix};
use obcm_route::RouteReader;

use crate::device_input::{label, DeviceInput};
use crate::framebuffer::Framebuffer;
use crate::gpx::Track;
use crate::gpx_player::GpxPlayer;
use crate::routes::RouteStore;
use crate::sim_location::SimLocationSource;
use crate::Args;

// Control-panel knob colors (host chrome — picked for the egui panel, not the
// device screen, so they need not pass through the 64-color quantization).
const KNOB_FILL: egui::Color32 = egui::Color32::from_rgb(70, 60, 48);
const KNOB_EDGE: egui::Color32 = egui::Color32::from_rgb(150, 120, 80);
const NOTCH: egui::Color32 = egui::Color32::from_rgb(234, 223, 192);
const AMBER: egui::Color32 = egui::Color32::from_rgb(227, 165, 43);

/// Points along a clockwise arc from 12 o'clock, sweeping `progress` (0–1) of a
/// full turn at radius `r` — the encoder hold-progress ring around the knob.
fn arc_points(center: egui::Pos2, r: f32, progress: f32) -> Vec<egui::Pos2> {
    use std::f32::consts::{FRAC_PI_2, TAU};
    let sweep = progress.clamp(0.0, 1.0) * TAU;
    let n = ((sweep / 0.25).ceil() as usize).max(2);
    (0..=n)
        .map(|i| {
            let a = -FRAC_PI_2 + sweep * (i as f32 / n as f32);
            center + egui::Vec2::angled(a) * r
        })
        .collect()
}

/// Loosely clamp zoom (pixels per microdegree of latitude) so scroll can't drive
/// it to zero or infinity and produce a degenerate projection.
const MIN_ZOOM: f32 = 1e-6;
const MAX_ZOOM: f32 = 1e4;

/// Meters per microdegree of latitude — mirrors `obcm_render`'s private constant
/// so the control panel can present zoom in human-friendly meters-per-pixel.
const METERS_PER_MICRODEG_LAT: f32 = 0.111_320;

/// Practical bounds for the zoom slider, in meters per pixel: roughly a ~5 m to
/// ~4800 m screen span on the 240 px device. The mouse can still scroll past these
/// (the slider only writes back when dragged), so they don't cap the camera.
const MPP_MIN: f32 = 0.02;
const MPP_MAX: f32 = 20_000.0;

/// Zoom (px per microdegree-lat) → meters per pixel. Same relation as
/// [`obcm_render::Viewport::meters_per_pixel`], usable without a viewport.
fn zoom_to_mpp(zoom: f32) -> f32 {
    METERS_PER_MICRODEG_LAT / zoom
}

/// Meters per pixel → zoom (the inverse of [`zoom_to_mpp`]).
fn mpp_to_zoom(mpp: f32) -> f32 {
    METERS_PER_MICRODEG_LAT / mpp
}

/// A ground distance in meters as a short human string ("5 m", "2.5 km").
fn format_distance(m: f32) -> String {
    if m < 1.0 {
        format!("{m:.2} m")
    } else if m < 1000.0 {
        format!("{m:.0} m")
    } else {
        format!("{:.1} km", m / 1000.0)
    }
}

/// Seconds as a playback clock: `M:SS`, or `H:MM:SS` past an hour. Used for the
/// GPX scrubber's position/duration readout.
fn format_clock(sec: f64) -> String {
    let s = sec.max(0.0) as u64;
    let (h, m, s) = (s / 3600, (s % 3600) / 60, s % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

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
            gpx_label: None,
            gpx_error: None,
            quit: false,
            texture: None,
            screenshot: args.screenshot,
            screenshot_requested: false,
            bytes,
            last_stats: obcm_render::RenderStats::default(),
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
                    format_clock(player.duration())
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

        // Drive the app from whichever location source is active. A loaded GPX
        // replay takes over from the manual panel fix (just as the device's GPS
        // would); we advance it by this frame's wall-clock time before ticking.
        if let Some(player) = self.gpx.as_mut() {
            let dt = ctx.input(|i| i.stable_dt) as f64;
            player.advance(dt);
            self.app.tick(player);
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
            self.app.tick(&mut self.loc);
        }

        // Open the active route's geometry (reloads only when the selection changes),
        // so the Map can stream it — the firmware does the same from its SD card.
        self.store.sync_active(self.app.activity.active_route);
        let route_src = self.store.active_source();
        let route = route_src.as_ref().and_then(|s| RouteReader::open(s).ok());

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
                let new_zoom = (st.zoom * (scroll * 0.005).exp()).clamp(MIN_ZOOM, MAX_ZOOM);

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

    /// The device's own controls — a rotary **encoder** (turn + push) and a
    /// **Back** button — emulated to resemble the real interface. Turn the knob by
    /// scrolling over it or dragging around it; PUSH / BACK are press-and-hold
    /// (held past the threshold they become `Hold` / `Back-hold`); the keyboard
    /// mirrors all of it. Raw events feed [`App::handle_input`], which runs the
    /// shared recognizer and drives the screen stack — so the encoder actually
    /// zooms the map, pauses into Ride control, etc. The recognized gesture and
    /// hold-progress are read back for the live readout.
    fn show_device_controls(&mut self, ui: &mut egui::Ui) {
        ui.label(egui::RichText::new("Device controls — encoder + Back").strong());
        ui.label(egui::RichText::new("scroll or drag the knob to turn").weak().size(11.0));
        ui.add_space(4.0);

        // --- Knob: a round encoder with a pointer notch + hold-progress ring. ---
        let knob_angle = self.input.knob_angle();
        let enc_progress = self.app.encoder_hold_progress();
        const SZ: f32 = 110.0;
        let resp = ui
            .vertical_centered(|ui| {
                let (rect, resp) = ui.allocate_exact_size(egui::vec2(SZ, SZ), egui::Sense::drag());
                let painter = ui.painter_at(rect);
                let center = rect.center();
                let radius = SZ * 0.42;
                painter.circle_filled(center, radius, KNOB_FILL);
                painter.circle_stroke(center, radius, egui::Stroke::new(2.0, KNOB_EDGE));
                // Pointer notch shows the knob's rotation.
                let notch = center + egui::Vec2::angled(knob_angle) * (radius - 7.0);
                painter.line_segment([center, notch], egui::Stroke::new(3.0, NOTCH));
                painter.circle_filled(notch, 4.0, NOTCH);
                // Encoder hold-progress arc — previews the guarded-action confirm ring.
                if enc_progress > 0.0 {
                    painter.add(egui::Shape::line(
                        arc_points(center, radius + 6.0, enc_progress),
                        egui::Stroke::new(4.0, AMBER),
                    ));
                }
                resp
            })
            .inner;

        if resp.dragged() {
            if let Some(p) = resp.interact_pointer_pos() {
                self.input.drag_to((p - resp.rect.center()).angle());
            }
        }
        if resp.drag_stopped() {
            self.input.end_drag();
        }
        if resp.hovered() {
            let dy = ui.input(|i| i.smooth_scroll_delta.y);
            if dy != 0.0 {
                self.input.scroll(dy);
            }
        }

        // --- PUSH (encoder) / BACK buttons — press-and-hold. ---
        ui.add_space(6.0);
        let (push_resp, back_resp) = ui
            .horizontal(|ui| {
                let p = ui.add_sized([100.0, 32.0], egui::Button::new("PUSH"));
                let b = ui.add_sized([100.0, 32.0], egui::Button::new("BACK"));
                (p, b)
            })
            .inner;

        // --- Keyboard mirror: ←/→ (or [ ] / , .) turn; Enter push; Backspace back. ---
        let (enc_key, back_key, turn_keys) = ui.input(|i| {
            let mut t = 0;
            if i.key_pressed(egui::Key::ArrowRight)
                || i.key_pressed(egui::Key::CloseBracket)
                || i.key_pressed(egui::Key::Period)
            {
                t += 1;
            }
            if i.key_pressed(egui::Key::ArrowLeft)
                || i.key_pressed(egui::Key::OpenBracket)
                || i.key_pressed(egui::Key::Comma)
            {
                t -= 1;
            }
            (i.key_down(egui::Key::Enter), i.key_down(egui::Key::Backspace), t)
        });
        self.input.turn(turn_keys);
        self.input.set_button(Button::Encoder, push_resp.is_pointer_button_down_on() || enc_key);
        self.input.set_button(Button::Back, back_resp.is_pointer_button_down_on() || back_key);

        // Run this frame's raw events through the shared recognizer + screen stack
        // (the exact path the firmware uses), firing long-press at its threshold.
        let now = self.input.now_ms();
        self.app.handle_input(now, &mut self.input);

        // --- Live readout (read back from the app). ---
        ui.add_space(8.0);
        let last = self.app.last_gesture().map(label).unwrap_or_else(|| "(none)".to_owned());
        ui.label(egui::RichText::new(format!("Last gesture:  {last}")).size(16.0).strong());

        let ep = self.app.encoder_hold_progress();
        let bp = self.app.back_hold_progress();
        if ep > 0.0 {
            ui.add(egui::ProgressBar::new(ep).desired_width(220.0).text("encoder hold"));
        }
        if bp > 0.0 {
            ui.add(egui::ProgressBar::new(bp).desired_width(220.0).text("back hold"));
        }

        ui.add_space(2.0);
        ui.label(
            egui::RichText::new("keys: Left/Right turn · Enter push · Backspace back  (hold for long-press)")
                .weak()
                .size(11.0),
        );
    }

    /// Draw the "Controls" window — a second OS window (egui immediate viewport)
    /// that drives the simulated GPS fix. Re-declared every frame; the widgets
    /// edit the panel mirrors / `AppState`, then we push the mirrors into the
    /// [`SimLocationSource`] so the next [`App::tick`] picks them up.
    fn show_control_panel(&mut self, ctx: &egui::Context) {
        ctx.show_viewport_immediate(
            egui::ViewportId::from_hash_of("controls"),
            egui::ViewportBuilder::default()
                .with_title("Controls")
                .with_inner_size([360.0, 880.0]),
            |ctx, _class| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    ui.heading("Simulated device");
                    ui.add_space(6.0);

                    // The device's own controls (encoder + Back) → gesture readout.
                    self.show_device_controls(ui);
                    ui.add_space(6.0);
                    ui.separator();
                    ui.add_space(6.0);

                    // Let sliders span the panel width instead of egui's narrow
                    // default — leaves room for the value box on the right.
                    ui.spacing_mut().slider_width = (ui.available_width() - 90.0).max(140.0);

                    // While a GPX track is loaded it owns the fix (like the device's
                    // GPS would), so the manual position/heading inputs are shown
                    // read-only and just track the replay. Camera/zoom/orientation
                    // stay live.
                    let replaying = self.gpx.is_some();

                    // Position — the GPS fix, edited in degrees (stored as µdeg).
                    ui.add_enabled_ui(!replaying, |ui| {
                        egui::Grid::new("position").num_columns(2).spacing([8.0, 6.0]).show(
                            ui,
                            |ui| {
                                ui.label("Latitude");
                                ui.add(
                                    egui::DragValue::new(&mut self.panel.lat_deg)
                                        .speed(1e-4)
                                        .range(-90.0..=90.0)
                                        .max_decimals(6)
                                        .suffix("°"),
                                );
                                ui.end_row();
                                ui.label("Longitude");
                                ui.add(
                                    egui::DragValue::new(&mut self.panel.lon_deg)
                                        .speed(1e-4)
                                        .range(-180.0..=180.0)
                                        .max_decimals(6)
                                        .suffix("°"),
                                );
                                ui.end_row();
                            },
                        );
                    });

                    ui.add_space(6.0);
                    ui.separator();

                    // Heading — rides on Fix.course (degrees CW from north).
                    ui.add_enabled_ui(!replaying, |ui| {
                        ui.label("Heading");
                        ui.add(
                            egui::Slider::new(&mut self.panel.heading_deg, 0.0..=360.0)
                                .suffix("°")
                                .step_by(1.0),
                        );
                    });

                    ui.add_space(6.0);
                    ui.separator();

                    // Zoom — operated in meters-per-pixel on a log scale. Only
                    // write back when the user drags it, so it never fights the
                    // mouse scroll (which can range past the slider's bounds).
                    ui.label("Zoom");
                    let mut mpp = zoom_to_mpp(self.app.state.zoom);
                    let resp = ui.add(
                        egui::Slider::new(&mut mpp, MPP_MIN..=MPP_MAX)
                            .logarithmic(true)
                            .custom_formatter(|n, _| {
                                let v = if n < 1.0 {
                                    format!("{n:.3}")
                                } else if n < 100.0 {
                                    format!("{n:.1}")
                                } else {
                                    format!("{n:.0}")
                                };
                                format!("{v} m/px")
                            }),
                    );
                    if resp.changed() {
                        self.app.state.zoom = mpp_to_zoom(mpp).clamp(MIN_ZOOM, MAX_ZOOM);
                    }
                    let span = zoom_to_mpp(self.app.state.zoom) * self.dev_w as f32;
                    ui.label(format!("{} across screen", format_distance(span)));

                    ui.add_space(6.0);
                    ui.separator();

                    // Camera mode.
                    ui.label("Camera");
                    let prev_mode = self.app.state.mode;
                    ui.horizontal(|ui| {
                        ui.selectable_value(&mut self.app.state.mode, CameraMode::Follow, "Follow");
                        ui.selectable_value(&mut self.app.state.mode, CameraMode::Free, "Free");
                    });
                    // Entering Follow: snap the fix onto the current camera center
                    // so the view doesn't jump (in Free the mouse moved the camera
                    // away from the fix) and the panel reads the followed point.
                    if prev_mode == CameraMode::Free && self.app.state.mode == CameraMode::Follow {
                        self.panel.lat_deg = self.app.state.cam_lat as f64 / 1e6;
                        self.panel.lon_deg = self.app.state.cam_lon as f64 / 1e6;
                    }

                    ui.add_space(6.0);
                    ui.separator();

                    // Orientation — north-up vs heading-up (rotates the map so the
                    // Heading above points to the top of the screen). Independent
                    // of the camera mode.
                    ui.label("Orientation");
                    ui.horizontal(|ui| {
                        ui.selectable_value(&mut self.app.state.heading_up, false, "North-up");
                        ui.selectable_value(&mut self.app.state.heading_up, true, "Heading-up");
                    });

                    ui.add_space(6.0);
                    ui.separator();

                    // GPX replay — load a recorded track and play it back as a
                    // simulated GPS sensor (position + derived course/speed). The
                    // player is the active `LocationSource` while a track is loaded.
                    ui.label("GPX replay");
                    if ui.button("Load GPX…").clicked() {
                        if let Some(path) =
                            rfd::FileDialog::new().add_filter("GPX track", &["gpx"]).pick_file()
                        {
                            self.load_gpx(&path);
                        }
                    }
                    self.show_gpx_controls(ui);

                    ui.add_space(6.0);
                    ui.separator();

                    ui.collapsing("Render Stats", |ui| {
                        let s = &self.last_stats;

                        egui::Grid::new("render_stats").num_columns(2).spacing([12.0, 4.0]).show(ui, |ui| {
                            ui.label("LOD");
                            ui.label(format!("{}", s.lod));
                            ui.end_row();

                            ui.label("Chunks");
                            ui.label(format!("{}", s.chunks_visited));
                            ui.end_row();

                            ui.label("Features");
                            ui.label(format!("{} / {} drawn", s.features_drawn, s.features_tried));
                            ui.end_row();

                            ui.label("Dropped");
                            let drop_color = if s.features_dropped > 0 {
                                egui::Color32::from_rgb(220, 80, 80)
                            } else {
                                ui.visuals().text_color()
                            };
                            ui.colored_label(drop_color, format!("{}", s.features_dropped));
                            ui.end_row();

                            ui.label("Points");
                            ui.label(format!("{} / {} drawn", s.points_drawn, s.points_tried));
                            ui.end_row();
                        });

                        ui.add_space(4.0);
                        ui.label("Buffer utilization");

                        // Span buffer bar
                        let span_pct = s.span_utilization;
                        ui.horizontal(|ui| {
                            ui.label("Spans");
                            let bar = egui::ProgressBar::new(span_pct)
                                .text(format!("{:.0}%", span_pct * 100.0));
                            ui.add(bar);
                        });

                        // Points buffer bar
                        let pt_pct = s.point_utilization;
                        ui.horizontal(|ui| {
                            ui.label("Points");
                            let bar = egui::ProgressBar::new(pt_pct)
                                .text(format!("{:.0}%", pt_pct * 100.0));
                            ui.add(bar);
                        });

                        // Rings buffer bar
                        let ring_pct = s.ring_utilization;
                        ui.horizontal(|ui| {
                            ui.label("Rings");
                            let bar = egui::ProgressBar::new(ring_pct)
                                .text(format!("{:.0}%", ring_pct * 100.0));
                            ui.add(bar);
                        });
                    });

                    if ctx.input(|i| i.viewport().close_requested()) {
                        self.quit = true;
                    }
                });
            },
        );

        // Push the mirrors into the location source (the app reads them next tick).
        self.loc.set_position(
            (self.panel.lat_deg * 1e6).round() as i32,
            (self.panel.lon_deg * 1e6).round() as i32,
        );
        self.loc.set_course(self.panel.heading_deg);
    }

    /// The loaded-track controls: play/pause (auto-follows), a seek scrubber, and
    /// a 1×–10× speed slider. Shows the load error (or nothing) when no track is
    /// loaded. Split out of [`show_control_panel`] so the "eject" mutation of
    /// `self.gpx` doesn't tangle with the active `&mut` borrow of the player.
    fn show_gpx_controls(&mut self, ui: &mut egui::Ui) {
        let Some(player) = self.gpx.as_mut() else {
            if let Some(err) = &self.gpx_error {
                ui.colored_label(egui::Color32::from_rgb(220, 80, 80), err);
            }
            return;
        };

        if let Some(label) = &self.gpx_label {
            ui.label(label);
        }

        let dur = player.duration();
        let mut eject = false;

        ui.horizontal(|ui| {
            let play_label = if player.is_playing() { "⏸ Pause" } else { "▶ Play" };
            if ui.button(play_label).clicked() {
                player.toggle();
                // Pressing play tracks the moving fix; the user can still switch
                // back to Free to pan around mid-playback.
                if player.is_playing() {
                    self.app.state.mode = CameraMode::Follow;
                }
            }
            if ui.button("⏏ Eject").clicked() {
                eject = true;
            }
        });

        if dur > 0.0 {
            // Scrubber — seek anywhere in the track, playing or paused.
            let mut t = player.time();
            let resp =
                ui.add(egui::Slider::new(&mut t, 0.0..=dur).show_value(false).text("seek"));
            if resp.changed() {
                player.seek(t);
            }
            ui.label(format!("{} / {}", format_clock(player.time()), format_clock(dur)));

            // Playback speed — real time (1×) up to 10×.
            let mut speed = player.speed();
            if ui.add(egui::Slider::new(&mut speed, 1.0..=10.0).suffix("×")).changed() {
                player.set_speed(speed);
            }
        } else {
            ui.label("track has no duration to replay");
        }

        if eject {
            self.gpx = None;
            self.gpx_label = None;
        }
    }
}

impl eframe::App for SimGui {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zoom_mpp_roundtrips() {
        for &zoom in &[1e-3_f32, 0.123, 1.0, 42.0, 1e3] {
            let back = mpp_to_zoom(zoom_to_mpp(zoom));
            assert!((back - zoom).abs() < zoom * 1e-5, "zoom {zoom} -> {back}");
        }
    }

    #[test]
    fn zoom_to_mpp_matches_viewport() {
        // The panel's conversion must agree with the renderer's own metric, or the
        // ground-span readout would lie about what's on screen.
        let vp = obcm_render::Viewport::new(240.0, 320.0, 0, 0, 0.5);
        assert!((zoom_to_mpp(0.5) - vp.meters_per_pixel()).abs() < 1e-5);
    }

    #[test]
    fn distance_formatting() {
        assert_eq!(format_distance(0.4), "0.40 m");
        assert_eq!(format_distance(5.0), "5 m");
        assert_eq!(format_distance(240.0), "240 m");
        assert_eq!(format_distance(2500.0), "2.5 km");
    }
}
