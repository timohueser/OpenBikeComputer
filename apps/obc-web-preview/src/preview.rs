//! The preview core: one [`MapPreview`] owns a demo map, a camera over it, and the
//! device-resolution RGBA frame the page blits.
//!
//! Deliberately **not** an app. [`obc-web-demo`](../obc_web_demo/index.html) boots the whole
//! firmware — screens, replay, planner — because the landing page is selling the device. A preset
//! card is selling the *cartography*, so this draws the map layer and nothing else: no status bar,
//! no rider marker, no route ink to argue with the styling. What you see is what the preset's
//! config told the packer to keep and what `obc-render` does with it, at the panel's own 240×320.
//!
//! Target-independent on purpose: everything here compiles and is tested natively (`cargo test`
//! from the repo root); only the thin `#[wasm_bindgen]` surface in `lib.rs` is wasm-only.

use embedded_graphics::pixelcolor::Rgb888;
use obc_host_core::RgbaFrame;
use obc_reader::{rgb565_to_device64, BBox, Error as ReadError, MapCache, MapTables, Reader, SliceSource};
use obc_render::{mpp_for_zoom, zoom_for_mpp, MapRenderer, Viewport};

/// The preview resolution — the one [`obc_display`] frame authority, not re-declared literals.
/// A preview is exactly the panel: 240×320, same pixels, same LOD choice at the same ground scale.
pub const FRAME_W: u32 = obc_display::ls021::FRAME_W as u32;
pub const FRAME_H: u32 = obc_display::ls021::FRAME_H as u32;

/// Fallback backdrop when a map carries no backdrop style — the same constant the Map screen
/// falls back to (`obc_app::screen::map::DEFAULT_BG_RGB565`), duplicated rather than exported
/// because pulling `obc-app` in for one `u16` would put the whole app in this bundle.
const DEFAULT_BG_RGB565: u16 = 0x2104;

/// Zoom-in limit, in ground metres per pixel. Past this the finest LOD is being magnified rather
/// than read, and a preview stops saying anything about the preset.
const MIN_MPP: f32 = 2.0;

// There is no zoom-*out* constant: the opening view already fits the whole crop, so the widest
// useful view is the one the card starts on. Anything past it frames backdrop the packer was
// never asked to fill, which reads as a broken preview rather than a small map.

/// Why a preview could not be opened. Unlike the conversion bridge's vocabulary, none of these
/// are things a *visitor* did: a preview map is a build artifact this repo committed, so every
/// variant here means the site is misdeployed and the message says so.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreviewErrorCode {
    /// The bytes are not an OBCM map, or not a version this build reads.
    NotAMap,
    /// A read ran off the end — a truncated download or a half-written asset.
    Truncated,
    /// A defect in the bridge itself.
    Internal,
}

impl PreviewErrorCode {
    /// The stable kebab-case identifier the browser wrapper re-exports.
    pub fn as_str(self) -> &'static str {
        match self {
            PreviewErrorCode::NotAMap => "not-a-map",
            PreviewErrorCode::Truncated => "truncated",
            PreviewErrorCode::Internal => "internal",
        }
    }
}

/// A preview that could not be opened: a stable [`PreviewErrorCode`] plus prose.
#[derive(Debug, Clone)]
pub struct PreviewFailure {
    pub code: PreviewErrorCode,
    pub message: String,
}

/// Map the reader's vocabulary onto ours. Exhaustive by hand so a new [`ReadError`] variant breaks
/// this build instead of quietly inheriting someone else's wording.
fn from_read(err: ReadError) -> PreviewFailure {
    let (code, message) = match err {
        ReadError::BadMagic => (PreviewErrorCode::NotAMap, "These bytes are not an OBCM map."),
        ReadError::BadVersion => (
            PreviewErrorCode::NotAMap,
            "This preview map was packed for a different OBCM version than this build reads — re-run builder/bake-previews.sh.",
        ),
        ReadError::TooShort | ReadError::BadOffset => {
            (PreviewErrorCode::Truncated, "This preview map is truncated — the download did not complete.")
        }
        // Neither can happen over an in-memory slice (`SliceSource` cannot fail, and nothing else
        // holds the cache during `parse`), so there is no better sentence to write than the truth.
        ReadError::Source(_) | ReadError::CacheBusy => {
            (PreviewErrorCode::Internal, "The preview map could not be read.")
        }
    };
    PreviewFailure { code, message: message.to_string() }
}

/// A camera over the map: centre in microdegrees, zoom in pixels per microdegree of latitude
/// (`obc-render`'s own unit — see [`Viewport`]).
#[derive(Debug, Clone, Copy, PartialEq)]
struct Camera {
    lon: i32,
    lat: i32,
    zoom: f32,
}

/// One demo map, open and renderable. Construct with [`MapPreview::open`], move the camera, ask
/// for [`frame`](MapPreview::frame).
pub struct MapPreview {
    /// The map file's bytes. `Reader` is a cheap view rebuilt over them per render.
    bytes: Vec<u8>,
    tables: MapTables,
    /// Boxed: the chunk cache is ≈277 KB, far past a wasm stack frame (#661).
    cache: Box<MapCache>,
    /// Boxed for the same reason — ≈90 KB of render scratch.
    renderer: Box<MapRenderer>,
    frame: RgbaFrame,
    cam: Camera,
    /// The opening camera [`reset`](MapPreview::reset) returns to, and the widest view the zoom
    /// clamp allows.
    home: Camera,
    /// The ground `home` frames — the box panning is held inside, so a drag cannot leave the
    /// crop the demo map was cut from.
    home_bbox: BBox,
    /// True until the next [`frame`](MapPreview::frame) call has to redraw. Lets a page poll
    /// without repainting a frame nothing moved.
    dirty: bool,
}

impl MapPreview {
    /// Open a demo `.obcm`, parking the camera on the whole map.
    pub fn open(bytes: Vec<u8>) -> Result<MapPreview, PreviewFailure> {
        let tables = MapTables::parse(&SliceSource(&bytes)).map_err(from_read)?;
        let map_bbox = tables.bbox;
        let home = fit(map_bbox);
        Ok(MapPreview {
            bytes,
            tables,
            cache: MapCache::new_boxed(),
            renderer: Box::new(MapRenderer::new()),
            frame: RgbaFrame::new(FRAME_W, FRAME_H),
            cam: home,
            home,
            home_bbox: map_bbox,
            dirty: true,
        })
    }

    /// Re-aim the opening view at an explicit box (microdegrees), and go there.
    ///
    /// This is how every card ends up framing **the same ground**. A packed map's own header bbox
    /// is not that box: `obc-pack` completes ways that leave the crop, so the header always sits
    /// somewhat wider than the extract it was cut from — and by a different amount per preset,
    /// because a preset that keeps more features completes more ways. Fitting the header would
    /// therefore hand each preset a slightly different camera, which is exactly the comparison
    /// this feature exists to avoid. The site passes the one pinned extract bbox instead.
    pub fn fit_bbox(&mut self, min_lon: i32, min_lat: i32, max_lon: i32, max_lat: i32) {
        self.home_bbox = BBox { min_lon, min_lat, max_lon, max_lat };
        self.home = fit(self.home_bbox);
        self.reset();
    }

    /// Back to the opening view.
    pub fn reset(&mut self) {
        self.set_camera(self.home);
    }

    /// Drag the map by a screen-space delta in pixels: `pan(10, 0)` moves the *content* 10 px
    /// right, which is what a pointer drag means.
    pub fn pan(&mut self, dx_px: f32, dy_px: f32) {
        let vp = self.viewport();
        // A screen delta is a ground delta divided by zoom; longitude carries the aspect
        // correction the projection applied. North-up only, so no rotation to undo.
        let d_lon = -dx_px / (self.cam.zoom * vp.aspect);
        let d_lat = dy_px / self.cam.zoom;
        self.set_camera(Camera {
            lon: self.cam.lon.saturating_add(d_lon as i32),
            lat: self.cam.lat.saturating_add(d_lat as i32),
            ..self.cam
        });
    }

    /// Scale the zoom by `factor` (`>1` zooms in), clamped to the preview's own limits.
    pub fn zoom_by(&mut self, factor: f32) {
        if !factor.is_finite() || factor <= 0.0 {
            return;
        }
        self.set_camera(Camera { zoom: self.cam.zoom * factor, ..self.cam });
    }

    /// Ground metres per pixel at the current zoom — the scale label a card can show, and the
    /// number that decides which LOD the renderer reads.
    pub fn meters_per_pixel(&self) -> f32 {
        mpp_for_zoom(self.cam.zoom)
    }

    /// Whether the camera has moved since the last [`frame`](MapPreview::frame).
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Render if needed and return the RGBA bytes (`FRAME_W * FRAME_H * 4`, opaque alpha) —
    /// exactly the layout `ImageData` expects.
    pub fn frame(&mut self) -> &[u8] {
        if self.dirty {
            self.draw();
            self.dirty = false;
        }
        self.frame.as_rgba()
    }

    /// Clamp a proposed camera into the map and the zoom limits, and mark the frame dirty if it
    /// actually moved. Everything that moves the camera goes through here, so the limits cannot
    /// be bypassed by adding another mover.
    fn set_camera(&mut self, next: Camera) {
        // `home.zoom` can exceed the zoom-in limit for a very small crop; the opening view wins,
        // or `open` would show something `reset` cannot return to.
        let zoom = next.zoom.clamp(self.home.zoom, zoom_for_mpp(MIN_MPP).max(self.home.zoom));
        // Keep the camera inside the crop: panning past the edge frames backdrop, and a preview
        // that can be dragged into nothing reads as a broken preview.
        let b = self.home_bbox;
        let clamped =
            Camera { lon: next.lon.clamp(b.min_lon, b.max_lon), lat: next.lat.clamp(b.min_lat, b.max_lat), zoom };
        if clamped != self.cam {
            self.cam = clamped;
            self.dirty = true;
        }
    }

    fn viewport(&self) -> Viewport {
        Viewport::new(FRAME_W as f32, FRAME_H as f32, self.cam.lon, self.cam.lat, self.cam.zoom)
    }

    /// One render pass: the map's own backdrop, then the base map, through the same
    /// [`MapRenderer`] the firmware's Map screen calls and the same RGB565 → RGB222 → RGB888
    /// quantization the panel imposes. The 64-colour step is not cosmetic here — a preview that
    /// showed true colour would flatter every preset with shades the device cannot draw.
    fn draw(&mut self) {
        let src = SliceSource(&self.bytes);
        let reader = Reader::new(&src, &self.tables, &self.cache);
        let bg565 = reader.backdrop_style().map_or(DEFAULT_BG_RGB565, |s| s.color);
        let vp = Viewport::new(FRAME_W as f32, FRAME_H as f32, self.cam.lon, self.cam.lat, self.cam.zoom);
        self.renderer.render(&mut self.frame, &reader, &vp, device_color(bg565), device_color);
    }
}

/// RGB565 → the panel's 64-colour RGB222 → RGB888, the one quantization the device applies.
fn device_color(c: u16) -> Rgb888 {
    let (r, g, b) = rgb565_to_device64(c);
    Rgb888::new(r, g, b)
}

/// The camera that fits `b` in the frame: centred on it, zoomed to whichever axis binds.
/// Longitude carries the projection's `cos(lat)` correction, so the aspect is read off a probe
/// viewport rather than recomputed here — one Earth model, one place.
fn fit(b: BBox) -> Camera {
    let lon = ((b.min_lon as i64 + b.max_lon as i64) / 2) as i32;
    let lat = ((b.min_lat as i64 + b.max_lat as i64) / 2) as i32;
    let span_lon = (b.max_lon as i64 - b.min_lon as i64).max(1) as f32;
    let span_lat = (b.max_lat as i64 - b.min_lat as i64).max(1) as f32;
    let aspect = Viewport::new(FRAME_W as f32, FRAME_H as f32, lon, lat, 1.0).aspect;
    let by_width = FRAME_W as f32 / (span_lon * aspect);
    let by_height = FRAME_H as f32 / span_lat;
    Camera { lon, lat, zoom: by_width.min(by_height) }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The committed demo map for the default preset — the same bytes the site fetches. Its
    /// presence is itself part of the contract: `bake-previews.sh` produces one per preset, and a
    /// deleted or unbuilt asset fails this build rather than a visitor's page.
    const DEMO: &[u8] = include_bytes!("../../../builder/app/public/preview/bikepacking.obcm");

    fn demo() -> MapPreview {
        MapPreview::open(DEMO.to_vec()).expect("the committed demo map is a valid OBCM")
    }

    fn open_err(bytes: Vec<u8>) -> PreviewFailure {
        match MapPreview::open(bytes) {
            Err(e) => e,
            Ok(_) => panic!("opened a preview over bytes that are not a demo map"),
        }
    }

    /// The two ways a deploy goes wrong get different words: the wrong file, and a file that
    /// arrived half-written. Both are the site's fault, and the messages say so.
    #[test]
    fn rejects_bytes_that_are_not_a_map() {
        let not_a_map = open_err(b"not an obcm file at all, but long enough to hold a header".to_vec());
        assert_eq!(not_a_map.code, PreviewErrorCode::NotAMap);
        assert_eq!(not_a_map.code.as_str(), "not-a-map");

        let truncated = open_err(DEMO[..DEMO.len() / 2].to_vec());
        assert_eq!(truncated.code, PreviewErrorCode::Truncated);
        assert_eq!(truncated.code.as_str(), "truncated");
    }

    /// The opening frame draws real cartography, not an empty backdrop. Counting distinct colours
    /// is the cheapest honest check: a map that failed to decode clears to one colour and stops.
    #[test]
    fn the_opening_frame_draws_the_map() {
        let mut p = demo();
        assert!(p.is_dirty(), "a freshly opened preview owes its first frame");
        let colors: std::collections::HashSet<[u8; 3]> =
            p.frame().chunks_exact(4).map(|px| [px[0], px[1], px[2]]).collect();
        assert!(colors.len() >= 4, "only {} colours — the demo map rendered nothing", colors.len());
        assert!(!p.is_dirty(), "the frame is clean until the camera moves");
    }

    /// Every pixel is opaque and the buffer is exactly panel-sized — the two things
    /// `putImageData` relies on.
    #[test]
    fn the_frame_is_panel_sized_and_opaque() {
        let mut p = demo();
        let buf = p.frame();
        assert_eq!(buf.len(), (FRAME_W * FRAME_H * 4) as usize);
        assert!(buf.iter().skip(3).step_by(4).all(|&a| a == 0xFF));
    }

    /// The opening view fits the whole map: both spans land inside the frame, and at least one
    /// of them fills it (otherwise the fit left slack it did not have to).
    #[test]
    fn the_opening_camera_fits_the_whole_map() {
        let p = demo();
        let vp = p.viewport();
        let b = p.tables.bbox;
        let (x0, y0) = vp.to_screen(b.min_lon, b.max_lat);
        let (x1, y1) = vp.to_screen(b.max_lon, b.min_lat);
        assert!(x0 >= 0 && x1 <= FRAME_W as i32, "longitude span {x0}..{x1} does not fit");
        assert!(y0 >= 0 && y1 <= FRAME_H as i32, "latitude span {y0}..{y1} does not fit");
        let tight = (x1 - x0) as u32 + 2 >= FRAME_W || (y1 - y0) as u32 + 2 >= FRAME_H;
        assert!(tight, "the fit is loose on both axes: {}x{}", x1 - x0, y1 - y0);
    }

    /// Zooming in is bounded by the ground-scale floor, and zooming out by the opening view — so
    /// no amount of scrolling reaches a magnified blur or a rectangle of backdrop.
    #[test]
    fn zoom_is_clamped_at_both_ends() {
        let mut p = demo();
        let opening = p.meters_per_pixel();
        for _ in 0..50 {
            p.zoom_by(2.0);
        }
        assert!(p.meters_per_pixel() >= MIN_MPP - 0.001, "zoomed past the floor: {}", p.meters_per_pixel());
        for _ in 0..50 {
            p.zoom_by(0.5);
        }
        assert!(
            p.meters_per_pixel() <= opening + 0.001,
            "zoomed out past the opening view: {} vs {opening}",
            p.meters_per_pixel()
        );
    }

    /// Panning stops at the crop's edges rather than sailing off into empty coordinates.
    #[test]
    fn panning_is_clamped_to_the_crop() {
        let mut p = demo();
        for _ in 0..2000 {
            p.pan(-100.0, -100.0);
        }
        let b = p.home_bbox;
        assert!(p.cam.lon <= b.max_lon && p.cam.lat >= b.min_lat);
        for _ in 0..4000 {
            p.pan(100.0, 100.0);
        }
        assert!(p.cam.lon >= b.min_lon && p.cam.lat <= b.max_lat);
    }

    /// `fit_bbox` frames the box it was handed, not the map's own header bbox — the property that
    /// makes every preset card show the same ground. The site passes the one pinned crop; here
    /// the check is that an arbitrary sub-box lands inside the frame and fills an axis.
    #[test]
    fn fit_bbox_frames_the_box_it_is_given() {
        let mut p = demo();
        let b = p.tables.bbox;
        let inset_lon = (b.max_lon - b.min_lon) / 4;
        let inset_lat = (b.max_lat - b.min_lat) / 4;
        let want = BBox {
            min_lon: b.min_lon + inset_lon,
            min_lat: b.min_lat + inset_lat,
            max_lon: b.max_lon - inset_lon,
            max_lat: b.max_lat - inset_lat,
        };
        p.fit_bbox(want.min_lon, want.min_lat, want.max_lon, want.max_lat);
        let vp = p.viewport();
        let (x0, y0) = vp.to_screen(want.min_lon, want.max_lat);
        let (x1, y1) = vp.to_screen(want.max_lon, want.min_lat);
        assert!(x0 >= 0 && x1 <= FRAME_W as i32, "longitude span {x0}..{x1} does not fit");
        assert!(y0 >= 0 && y1 <= FRAME_H as i32, "latitude span {y0}..{y1} does not fit");
        let tight = (x1 - x0) as u32 + 2 >= FRAME_W || (y1 - y0) as u32 + 2 >= FRAME_H;
        assert!(tight, "the fit is loose on both axes: {}x{}", x1 - x0, y1 - y0);
        // …and the new opening view is now the widest one available.
        p.zoom_by(0.25);
        assert_eq!(p.cam, p.home, "zooming out past the new opening view moved the camera");
    }

    /// A camera move dirties the frame; a no-op move (already clamped to an edge) does not, so a
    /// page polling `is_dirty` does not repaint on every stray pointer event.
    #[test]
    fn only_a_real_move_dirties_the_frame() {
        let mut p = demo();
        p.frame();
        p.pan(20.0, 0.0);
        assert!(p.is_dirty());
        p.frame();
        p.zoom_by(1.0);
        assert!(!p.is_dirty(), "a 1x zoom moved nothing");
        p.reset();
        p.frame();
        assert!(!p.is_dirty());
    }

    /// `reset` returns exactly to the opening camera after arbitrary interaction — the "start
    /// over" affordance cannot drift.
    #[test]
    fn reset_returns_to_the_opening_camera() {
        let mut p = demo();
        let opening = p.cam;
        p.zoom_by(3.0);
        p.pan(-40.0, 25.0);
        assert_ne!(p.cam, opening);
        p.reset();
        assert_eq!(p.cam, opening);
    }
}
