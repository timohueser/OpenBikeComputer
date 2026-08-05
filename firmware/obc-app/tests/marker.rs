//! Wiring test for the user-position marker overlay in [`App::render_frame`]:
//! it draws the marker (resolved through the host `color_fn`) only when a fix is
//! present, and the dot-vs-chevron branch follows `Fix.course`. Renders against a
//! tiny in-memory `DrawTarget` over a hand-built minimal v5 `.obcm`.

use embedded_graphics::pixelcolor::Rgb888;
use obc_app::{App, AppState, CameraMode};
use obc_ports::{Fix, RideClock, Sensors};
use obc_reader::{rgb565_to_rgb888, MapCache, MapTables, Reader, SliceSource};

mod common;
use common::{build_min_obcm, Buf, ReplayFix};

/// Marker color baked into the test file (RGB565 red → Rgb888 (255,0,0)).
const MARKER_565: u16 = 0xF800;
const RED: Rgb888 = Rgb888::new(255, 0, 0);

/// Render one frame of `app` against `bytes` into a fresh 120×120 buffer, with a
/// true-color `color_fn` (so the RGB565 marker red shows up as Rgb888 red).
fn render(app: &mut App, bytes: &[u8]) -> Buf {
    let cache = MapCache::new();
    let src = SliceSource(bytes);
    let tables = MapTables::parse(&src).expect("valid v7 file");
    let reader = Reader::new(&src, &tables, &cache);
    let mut buf = Buf::new(120, 120);
    let mut scratch = Box::new(obc_render::RenderScratch::new());
    app.render_frame(Some(&mut scratch), &mut buf, &reader, None, 120.0, 120.0, |c| {
        let (r, g, b) = rgb565_to_rgb888(c);
        Rgb888::new(r, g, b)
    });
    buf
}

#[test]
fn marker_drawn_only_when_a_fix_is_set() {
    let bytes = build_min_obcm(MARKER_565);
    let mut app = App::new(AppState::new(0, 0, 0.05));

    // No fix yet → backdrop only, no marker pixels.
    assert_eq!(render(&mut app, &bytes).count(RED), 0, "no fix ⇒ no marker");

    // A fix at the camera center → the marker is drawn.
    app.tick(RideClock(0), Sensors::new(&mut ReplayFix(Some(Fix::at(0, 0)))), None);
    assert!(render(&mut app, &bytes).count(RED) > 0, "fix ⇒ marker drawn");
}

#[test]
fn dot_and_chevron_glyphs_differ_by_course() {
    let bytes = build_min_obcm(MARKER_565);
    let mut app = App::new(AppState::new(0, 0, 0.05));
    // Free keeps the camera pinned at (0,0) so both fixes project to the center.
    app.state.mode = CameraMode::Free;

    // Stationary (course None) → diamond dot.
    app.tick(
        RideClock(0),
        Sensors::new(&mut ReplayFix(Some(Fix { lat: 0, lon: 0, course: None, speed_mps: None }))),
        None,
    );
    let dot = render(&mut app, &bytes).count(RED);

    // Moving (course Some) → directional chevron, a different glyph.
    app.tick(
        RideClock(0),
        Sensors::new(&mut ReplayFix(Some(Fix { lat: 0, lon: 0, course: Some(0.0), speed_mps: Some(5.0) }))),
        None,
    );
    let chevron = render(&mut app, &bytes).count(RED);

    assert!(dot > 0 && chevron > 0, "both glyphs paint pixels");
    assert_ne!(dot, chevron, "the dot and chevron are distinct shapes");
}
