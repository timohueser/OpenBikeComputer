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
//! [`CameraMode::Free`]. The control panel (next step) adds GPS/heading/zoom
//! widgets in a second viewport and a Follow toggle.

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
        let loc = SimLocationSource::new(Some(Fix::at(cy as i32, cx as i32)));

        SimGui {
            app: App::new(state),
            loc,
            fb: Framebuffer::new(args.width, args.height),
            dev_w: args.width,
            dev_h: args.height,
            scale: args.scale,
            true_color: args.true_color,
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
    fn handle_camera_input(&mut self, ui: &egui::Ui, resp: &egui::Response, rect: egui::Rect) {
        let scale = self.scale as f64;
        let (w, h) = (self.dev_w as f64, self.dev_h as f64);
        let st = &mut self.app.state;

        if resp.dragged() {
            let d = resp.drag_delta();
            let vp = st.viewport(w, h);
            st.cam_lon -= (d.x as f64 / scale) / (vp.zoom * vp.aspect);
            st.cam_lat += (d.y as f64 / scale) / vp.zoom;
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
}

impl eframe::App for SimGui {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.render_to_texture(ctx);

        // No frame margin: the device screen fills its window edge-to-edge.
        egui::CentralPanel::default().frame(egui::Frame::none()).show(ctx, |ui| {
            let tex = self.texture.as_ref().expect("texture uploaded this frame");
            let size = egui::vec2((self.dev_w * self.scale) as f32, (self.dev_h * self.scale) as f32);
            let resp = ui.add(
                egui::Image::new(egui::load::SizedTexture::from_handle(tex))
                    .fit_to_exact_size(size)
                    .texture_options(egui::TextureOptions::NEAREST)
                    .sense(egui::Sense::click_and_drag()),
            );
            let rect = resp.rect;
            self.handle_camera_input(ui, &resp, rect);
        });

        if self.screenshot.is_some() {
            self.run_screenshot(ctx);
        }

        // Drive the loop continuously so future control-panel / GPX changes show
        // up without needing a mouse event to wake the window.
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
