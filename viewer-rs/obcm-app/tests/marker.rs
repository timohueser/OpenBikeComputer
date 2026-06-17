//! Wiring test for the user-position marker overlay in [`App::render_frame`]:
//! it draws the marker (resolved through the host `color_fn`) only when a fix is
//! present, and the dot-vs-chevron branch follows `Fix.course`. Renders against a
//! tiny in-memory `DrawTarget` over a hand-built minimal v4 `.obcm`.

use embedded_graphics::{pixelcolor::Rgb888, prelude::*, primitives::Rectangle};
use obcm::{rgb565_to_rgb888, Reader};
use obcm_app::{App, AppState, CameraMode, Fix, LocationSource};

/// Marker color baked into the test file (RGB565 red → Rgb888 (255,0,0)).
const MARKER_565: u16 = 0xF800;
const RED: Rgb888 = Rgb888::new(255, 0, 0);

/// A `LocationSource` replaying one scripted fix (the control-panel stand-in).
struct Fixed(Option<Fix>);
impl LocationSource for Fixed {
    fn poll(&mut self) -> Option<Fix> {
        self.0
    }
}

/// A minimal valid v4 file: one sea-backdrop style, one LOD with a single empty
/// leaf and no chunks. The map renders as a flat backdrop, so the only non-bg
/// pixels come from the marker — making it trivial to detect.
fn build_min_obcm(marker: u16) -> Vec<u8> {
    let style_off: u32 = 32;
    // Style table: count=1, then (id=1, z=0, color=0x001F blue sea, weight=1).
    let mut styles = vec![1u8];
    styles.push(1);
    styles.push(0);
    styles.extend_from_slice(&0x001Fu16.to_le_bytes());
    styles.push(1);

    let lod_tab_off = style_off as usize + styles.len();
    let index_off = lod_tab_off + 18; // one 18-byte LOD entry

    // LOD entry: max_mpp=+inf, index_off, node_count=1, chunk_size=16, chunk_count=0.
    let mut table = Vec::new();
    table.extend_from_slice(&f32::INFINITY.to_le_bytes());
    table.extend_from_slice(&(index_off as u32).to_le_bytes());
    table.extend_from_slice(&1u32.to_le_bytes());
    table.extend_from_slice(&16u16.to_le_bytes());
    table.extend_from_slice(&0u32.to_le_bytes());

    // Index: a single empty leaf (no chunk).
    let index = 0x7FFF_FFFFu32.to_le_bytes();

    let mut f = Vec::new();
    f.extend_from_slice(b"OBCM");
    f.push(4);
    for v in [-1000i32, -1000, 1000, 1000] {
        f.extend_from_slice(&v.to_le_bytes()); // bbox: min_lat, min_lon, max_lat, max_lon
    }
    f.extend_from_slice(&style_off.to_le_bytes());
    f.push(1); // lod count
    f.extend_from_slice(&(lod_tab_off as u32).to_le_bytes());
    f.extend_from_slice(&marker.to_le_bytes());
    f.extend_from_slice(&styles);
    f.extend_from_slice(&table);
    f.extend_from_slice(&index);
    f
}

/// A `w`×`h` Rgb888 buffer implementing `DrawTarget`, with clipped writes.
struct Buf {
    w: i32,
    h: i32,
    px: Vec<Rgb888>,
}
impl Buf {
    fn new(w: i32, h: i32) -> Self {
        Buf { w, h, px: vec![Rgb888::BLACK; (w * h) as usize] }
    }
    fn count(&self, c: Rgb888) -> usize {
        self.px.iter().filter(|&&p| p == c).count()
    }
    fn put(&mut self, x: i32, y: i32, c: Rgb888) {
        if x >= 0 && y >= 0 && x < self.w && y < self.h {
            self.px[(y * self.w + x) as usize] = c;
        }
    }
}
impl OriginDimensions for Buf {
    fn size(&self) -> Size {
        Size::new(self.w as u32, self.h as u32)
    }
}
impl DrawTarget for Buf {
    type Color = Rgb888;
    type Error = core::convert::Infallible;
    fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Pixel<Self::Color>>,
    {
        for Pixel(p, c) in pixels {
            self.put(p.x, p.y, c);
        }
        Ok(())
    }
    fn fill_solid(&mut self, area: &Rectangle, color: Self::Color) -> Result<(), Self::Error> {
        let clip = area.intersection(&self.bounding_box());
        if let Some(br) = clip.bottom_right() {
            for y in clip.top_left.y..=br.y {
                for x in clip.top_left.x..=br.x {
                    self.put(x, y, color);
                }
            }
        }
        Ok(())
    }
}

/// Render one frame of `app` against `bytes` into a fresh 120×120 buffer, with a
/// true-color `color_fn` (so the RGB565 marker red shows up as Rgb888 red).
fn render(app: &mut App, bytes: &[u8]) -> Buf {
    let reader = Reader::new(bytes).expect("valid v4 file");
    let mut buf = Buf::new(120, 120);
    app.render_frame(&mut buf, &reader, 120.0, 120.0, |c| {
        let (r, g, b) = rgb565_to_rgb888(c);
        Rgb888::new(r, g, b)
    });
    buf
}

#[test]
fn marker_drawn_only_when_a_fix_is_set() {
    let bytes = build_min_obcm(MARKER_565);
    let mut app = App::new(AppState::new(0.0, 0.0, 0.05));

    // No fix yet → backdrop only, no marker pixels.
    assert_eq!(render(&mut app, &bytes).count(RED), 0, "no fix ⇒ no marker");

    // A fix at the camera center → the marker is drawn.
    app.tick(&mut Fixed(Some(Fix::at(0, 0))));
    assert!(render(&mut app, &bytes).count(RED) > 0, "fix ⇒ marker drawn");
}

#[test]
fn dot_and_chevron_glyphs_differ_by_course() {
    let bytes = build_min_obcm(MARKER_565);
    let mut app = App::new(AppState::new(0.0, 0.0, 0.05));
    // Free keeps the camera pinned at (0,0) so both fixes project to the center.
    app.state.mode = CameraMode::Free;

    // Stationary (course None) → diamond dot.
    app.tick(&mut Fixed(Some(Fix { lat: 0, lon: 0, course: None, speed_mps: None })));
    let dot = render(&mut app, &bytes).count(RED);

    // Moving (course Some) → directional chevron, a different glyph.
    app.tick(&mut Fixed(Some(Fix { lat: 0, lon: 0, course: Some(0.0), speed_mps: Some(5.0) })));
    let chevron = render(&mut app, &bytes).count(RED);

    assert!(dot > 0 && chevron > 0, "both glyphs paint pixels");
    assert_ne!(dot, chevron, "the dot and chevron are distinct shapes");
}
