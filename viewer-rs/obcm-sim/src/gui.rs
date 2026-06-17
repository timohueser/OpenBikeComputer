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

use eframe::egui;
use obcm::Reader;
use obcm_app::{App, AppState, CameraMode, Fix};

use crate::framebuffer::Framebuffer;
use crate::sim_location::SimLocationSource;
use crate::Args;

/// Loosely clamp zoom (pixels per microdegree of latitude) so scroll can't drive
/// it to zero or infinity and produce a degenerate projection.
const MIN_ZOOM: f64 = 1e-6;
const MAX_ZOOM: f64 = 1e4;

/// Meters per microdegree of latitude — mirrors `obcm::render`'s private constant
/// so the control panel can present zoom in human-friendly meters-per-pixel.
const METERS_PER_MICRODEG_LAT: f64 = 0.111_320;

/// Practical bounds for the zoom slider, in meters per pixel: roughly a ~5 m to
/// ~4800 m screen span on the 240 px device. The mouse can still scroll past these
/// (the slider only writes back when dragged), so they don't cap the camera.
const MPP_MIN: f64 = 0.02;
const MPP_MAX: f64 = 20_000.0;

/// Zoom (px per microdegree-lat) → meters per pixel. Same relation as
/// [`obcm::Viewport::meters_per_pixel`], usable without a viewport.
fn zoom_to_mpp(zoom: f64) -> f64 {
    METERS_PER_MICRODEG_LAT / zoom
}

/// Meters per pixel → zoom (the inverse of [`zoom_to_mpp`]).
fn mpp_to_zoom(mpp: f64) -> f64 {
    METERS_PER_MICRODEG_LAT / mpp
}

/// A ground distance in meters as a short human string ("5 m", "2.5 km").
fn format_distance(m: f64) -> String {
    if m < 1.0 {
        format!("{m:.2} m")
    } else if m < 1000.0 {
        format!("{m:.0} m")
    } else {
        format!("{:.1} km", m / 1000.0)
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
    loc: SimLocationSource,
    fb: Framebuffer,
    dev_w: u32,
    dev_h: u32,
    scale: u32,
    true_color: bool,
    /// Editable mirrors for the control-panel widgets.
    panel: PanelState,
    /// Set when the Controls window is closed; quits the whole app next frame.
    quit: bool,
    texture: Option<egui::TextureHandle>,
    /// `--screenshot`: save the first composited frame here, then close.
    screenshot: Option<String>,
    screenshot_requested: bool,
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
            lat: cy as i32,
            lon: cx as i32,
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

        SimGui {
            app: App::new(state),
            loc,
            fb: Framebuffer::new(args.width, args.height),
            dev_w: args.width,
            dev_h: args.height,
            scale: args.scale,
            true_color: args.true_color,
            panel,
            quit: false,
            texture: None,
            screenshot: args.screenshot,
            screenshot_requested: false,
            bytes,
        }
    }

    /// Run the shared app for one frame into the framebuffer, then upload it.
    fn render_to_texture(&mut self, ctx: &egui::Context) {
        let reader = Reader::new(&self.bytes).expect("map validated in main()");
        let tc = self.true_color;
        self.app.tick(&mut self.loc);
        self.app.render_frame(
            &mut self.fb,
            &reader,
            self.dev_w as f64,
            self.dev_h as f64,
            |c| crate::color_of(c, tc),
        );

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
        scale: f64,
    ) {
        let (w, h) = (self.dev_w as f64, self.dev_h as f64);
        let st = &mut self.app.state;

        if resp.dragged() {
            let d = resp.drag_delta();
            let dpx = d.x as f64 / scale;
            let dpy = d.y as f64 / scale;
            let vp = st.viewport(w, h);
            // Convert the screen-space drag into a map delta through the inverse
            // projection (`to_map`), so panning follows the cursor even when the
            // view is rotated (heading-up) — a fixed `cam ± dx/zoom` would drift.
            let (lon0, lat0) = vp.to_map(w / 2.0, h / 2.0);
            let (lon1, lat1) = vp.to_map(w / 2.0 - dpx, h / 2.0 - dpy);
            st.cam_lon += lon1 - lon0;
            st.cam_lat += lat1 - lat0;
            st.mode = CameraMode::Free;
        }

        let scroll = ui.input(|i| i.smooth_scroll_delta.y);
        if scroll != 0.0 {
            if let Some(pos) = resp.hover_pos() {
                // Cursor in device pixels, the space `Viewport::to_map` expects.
                let local = pos - rect.min;
                let px = (local.x as f64 / scale).clamp(0.0, w);
                let py = (local.y as f64 / scale).clamp(0.0, h);
                let new_zoom = (st.zoom * (scroll as f64 * 0.005).exp()).clamp(MIN_ZOOM, MAX_ZOOM);

                // Keep the ground point under the cursor fixed across the zoom.
                let (olon, olat) = st.viewport(w, h).to_map(px, py);
                st.zoom = new_zoom;
                let (nlon, nlat) = st.viewport(w, h).to_map(px, py);
                st.cam_lon += olon - nlon;
                st.cam_lat += olat - nlat;
                st.mode = CameraMode::Free;
            }
        }
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
                .with_inner_size([300.0, 420.0]),
            |ctx, _class| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    ui.heading("Simulated device");
                    ui.add_space(6.0);

                    // Position — the GPS fix, edited in degrees (stored as µdeg).
                    egui::Grid::new("position").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
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
                    });

                    ui.add_space(6.0);
                    ui.separator();

                    // Heading — rides on Fix.course (degrees CW from north).
                    ui.label("Heading");
                    ui.add(
                        egui::Slider::new(&mut self.panel.heading_deg, 0.0..=360.0)
                            .suffix("°")
                            .step_by(1.0),
                    );

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
                    let span = zoom_to_mpp(self.app.state.zoom) * self.dev_w as f64;
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
                        self.panel.lat_deg = self.app.state.cam_lat / 1e6;
                        self.panel.lon_deg = self.app.state.cam_lon / 1e6;
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
}

impl eframe::App for SimGui {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
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
            self.handle_camera_input(ui, &resp, rect, disp_scale as f64);
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
        for &zoom in &[1e-3, 0.123, 1.0, 42.0, 1e3] {
            let back = mpp_to_zoom(zoom_to_mpp(zoom));
            assert!((back - zoom).abs() < zoom * 1e-9, "zoom {zoom} -> {back}");
        }
    }

    #[test]
    fn zoom_to_mpp_matches_viewport() {
        // The panel's conversion must agree with the renderer's own metric, or the
        // ground-span readout would lie about what's on screen.
        let vp = obcm::Viewport::new(240.0, 320.0, 0.0, 0.0, 0.5);
        assert!((zoom_to_mpp(0.5) as f32 - vp.meters_per_pixel()).abs() < 1e-6);
    }

    #[test]
    fn distance_formatting() {
        assert_eq!(format_distance(0.4), "0.40 m");
        assert_eq!(format_distance(5.0), "5 m");
        assert_eq!(format_distance(240.0), "240 m");
        assert_eq!(format_distance(2500.0), "2.5 km");
    }
}
