//! The map / overlay plane split ([`App::render_map`] + [`App::render_overlay`], issue #45). Two
//! contracts:
//!
//! 1. **Compositing isolation** — `render_overlay` over an already-rendered map touches only its own
//!    pixels (the hold bulge), never clearing or repainting the map.
//! 2. **Liveness** — `App::overlay_active()` is true exactly across a hold's charge → pop (and an
//!    early-release retract), so a host repaints the overlay layer only when it would change a pixel.

use embedded_graphics::pixelcolor::Rgb888;
use obc_app::screen::palette;
use obc_app::{App, AppState};
use obc_ports::{Button, InputClock};
use obc_reader::{rgb565_to_rgb888, MapCache, MapTables, Reader, SliceSource};

mod common;
use common::{build_min_obcm, down, keys, up, Buf};

/// True-color palette color the host `color_fn` resolves a hint hue to.
fn rgb(c: u16) -> Rgb888 {
    let (r, g, b) = rgb565_to_rgb888(c);
    Rgb888::new(r, g, b)
}

#[test]
fn render_overlay_touches_only_overlay_pixels() {
    let bytes = build_min_obcm(0);
    let cache = MapCache::new();
    let src = SliceSource(&bytes);
    let tables = MapTables::parse(&src).expect("valid v7 file");
    let reader = Reader::new(&src, &tables, &cache);
    let (w, h) = (240i32, 320i32);
    let hud = rgb(palette::HUD); // the near-black bulge color

    // Charge Select past the dead zone (300 ms of a 500 ms threshold) so the
    // overlay has a bulge to draw on the right edge.
    let mut app = App::new(AppState::new(0, 0, 0.05));
    app.handle_input(InputClock(0), &mut keys(&[down(Button::Select)]));
    app.handle_input(InputClock(300), &mut keys(&[]));

    // Render the map, snapshot it, then composite the overlay over the *same* buffer.
    let mut buf = Buf::new(w, h);
    let mut scratch = Box::new(obc_render::RenderScratch::new());
    app.render_map(Some(&mut scratch), &mut buf, &reader, None, w as f32, h as f32, rgb);
    let map_only = buf.px.clone();
    app.render_overlay(&mut buf, w as f32, h as f32, rgb);

    // Everything left of the right-edge band must be byte-identical; inside the band, the only
    // changes are the bulge's own HUD-coloured pixels — and there must be some.
    let band_x = w - 20; // the bulge pokes in from x = w, at most pop_depth (12) px
    let mut changed_in_band = 0usize;
    for y in 0..h {
        for x in 0..w {
            let i = (y * w + x) as usize;
            let (before, after) = (map_only[i], buf.px[i]);
            if before == after {
                continue;
            }
            assert!(x >= band_x, "overlay changed a map pixel at ({x},{y}) outside the band");
            assert_eq!(after, hud, "overlay wrote a non-bulge colour at ({x},{y})");
            changed_in_band += 1;
        }
    }
    assert!(changed_in_band > 0, "the charged overlay must actually draw its bulge");
}

#[test]
fn render_frame_equals_map_then_overlay() {
    let bytes = build_min_obcm(0);
    let cache = MapCache::new();
    let src = SliceSource(&bytes);
    let tables = MapTables::parse(&src).expect("valid v7 file");
    let reader = Reader::new(&src, &tables, &cache);
    let (w, h) = (240i32, 320i32);

    // The thin `render_frame` convenience vs. the explicit `render_map` + `render_overlay` a
    // dual-layer host calls. Draw order is preserved, so the results must be byte-identical.
    let make_app = || {
        let mut app = App::new(AppState::new(0, 0, 0.05));
        app.handle_input(InputClock(0), &mut keys(&[down(Button::Select)]));
        app.handle_input(InputClock(300), &mut keys(&[]));
        app
    };

    let mut scratch = Box::new(obc_render::RenderScratch::new());
    let mut whole = Buf::new(w, h);
    make_app().render_frame(Some(&mut scratch), &mut whole, &reader, None, w as f32, h as f32, rgb);

    let mut split = Buf::new(w, h);
    let mut app = make_app();
    app.render_map(Some(&mut scratch), &mut split, &reader, None, w as f32, h as f32, rgb);
    app.render_overlay(&mut split, w as f32, h as f32, rgb);

    assert!(whole.px == split.px, "render_frame must equal render_map then render_overlay");
}

#[test]
fn overlay_active_is_true_exactly_across_a_completed_hold() {
    let mut app = App::new(AppState::new(0, 0, 0.05));

    // At rest there is nothing to draw.
    app.handle_input(InputClock(0), &mut keys(&[]));
    assert!(!app.overlay_active(), "no hold ⇒ the overlay is quiet");

    // A press still inside the dead zone (50 ms of a 500 ms hold) draws nothing.
    app.handle_input(InputClock(0), &mut keys(&[down(Button::Select)]));
    app.handle_input(InputClock(50), &mut keys(&[]));
    assert!(!app.overlay_active(), "inside the dead zone ⇒ still quiet");

    // Charging past the dead zone ⇒ the bulge is live.
    app.handle_input(InputClock(300), &mut keys(&[]));
    assert!(app.overlay_active(), "charging past the dead zone ⇒ overlay live");

    // The hold crosses its threshold and fires ⇒ the confirm pop is live.
    app.handle_input(InputClock(600), &mut keys(&[]));
    assert!(app.overlay_active(), "the confirm pop is live");

    // Once the pop has run its course (POP_MS = 220) the overlay is quiet again.
    app.handle_input(InputClock(900), &mut keys(&[]));
    assert!(!app.overlay_active(), "overlay quiet once the pop ends");
}

#[test]
fn overlay_active_spans_an_early_release_retract() {
    let mut app = App::new(AppState::new(0, 0, 0.05));

    // Charge past the dead zone, then release before the threshold ⇒ the bulge retracts.
    app.handle_input(InputClock(0), &mut keys(&[down(Button::Select)]));
    app.handle_input(InputClock(300), &mut keys(&[]));
    assert!(app.overlay_active(), "charging ⇒ overlay live");

    app.handle_input(InputClock(310), &mut keys(&[up(Button::Select)]));
    assert!(app.overlay_active(), "an early release retracts ⇒ still live");

    // Once the retract finishes (CANCEL_MS = 150) the overlay is quiet again.
    app.handle_input(InputClock(310 + 200), &mut keys(&[]));
    assert!(!app.overlay_active(), "overlay quiet once the retract finishes");
}

// --- The optional scratch (#1146 P2) ---

/// The **`None` arm**, which nothing else on the host side reaches: every host renders with a
/// scratch, while the board hands `None` on every chrome frame — that is the whole point of making
/// it optional, since a menu frame drawn while a route search owns the arena must still draw.
#[test]
fn a_chrome_frame_renders_identically_with_no_scratch_at_all() {
    let bytes = build_min_obcm(0);
    let cache = MapCache::new();
    let src = SliceSource(&bytes);
    let tables = MapTables::parse(&src).expect("valid v7 file");
    let reader = Reader::new(&src, &tables, &cache);
    let (w, h) = (240i32, 320i32);

    // Home (the device's real boot state) is a chrome base: nothing in its draw path touches the
    // render scratch.
    let make = || App::new_idle(AppState::new(0, 0, 0.05));
    assert!(!make().base_draws_map(), "Home draws no map");

    let mut scratch = Box::new(obc_render::RenderScratch::new());
    let mut lent = Buf::new(w, h);
    make().render_map(Some(&mut scratch), &mut lent, &reader, None, w as f32, h as f32, rgb);

    let mut bare = Buf::new(w, h);
    make().render_map(None, &mut bare, &reader, None, w as f32, h as f32, rgb);

    assert!(bare.px == lent.px, "a chrome frame must not depend on the host lending its scratch");
}

/// And the arm's failure half: `None` under a map-drawing base is a **caller bug**, so the map is
/// skipped rather than invented — loudly in debug (the `debug_assert` in `draw_map_scene`), quietly
/// on a shipping board, where the frame degrades to its own chrome instead of faulting mid-ride.
/// Neither half had any coverage, and the release half has none by construction on the board.
#[test]
fn a_map_base_without_a_scratch_skips_the_map_instead_of_inventing_one() {
    let bytes = build_min_obcm(0);
    let cache = MapCache::new();
    let src = SliceSource(&bytes);
    let tables = MapTables::parse(&src).expect("valid v7 file");
    let reader = Reader::new(&src, &tables, &cache);
    let (w, h) = (240i32, 320i32);

    // `App::new` is the map-first constructor: stack `[Home, Map]`.
    let mut app = App::new(AppState::new(0, 0, 0.05));
    assert!(app.base_draws_map(), "the riding Map is a map base");

    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {})); // the debug assert is expected: don't print its backtrace
    let rendered = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut buf = Buf::new(w, h);
        let stats = app.render_map(None, &mut buf, &reader, None, w as f32, h as f32, rgb);
        (buf, stats)
    }));
    std::panic::set_hook(previous);

    #[cfg(debug_assertions)]
    assert!(rendered.is_err(), "a debug build must say so loudly rather than skip in silence");
    if let Ok((buf, stats)) = rendered {
        // The shipping-board half: no panic, nothing collected, and — because `MapScreen::draw`
        // gives up with the scene rather than half-drawing it — the target is left exactly as it
        // was found. On the board that *is* the graceful degradation: the previous frame stays on
        // the reflective glass, which is what every other transient render failure does too.
        assert_eq!(stats.features_drawn, 0, "nothing may be drawn from a scratch that was not lent");
        assert!(
            buf.px.iter().all(|p| *p == Rgb888::new(0, 0, 0)),
            "the plane must be left untouched, not half-painted"
        );
    }
}
