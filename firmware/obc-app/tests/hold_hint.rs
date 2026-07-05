//! Wiring test for the global long-press hint in [`App::render_frame`]: holding the encoder swells a
//! black "frame bulge" into the right edge near the top, holding Back one near the bottom, and a
//! quick tap neither. Each held frame is compared to the idle frame so any standing chrome cancels
//! out and only the bulge's extra near-black pixels are measured.

use embedded_graphics::pixelcolor::Rgb888;
use obc_app::screen::palette;
use obc_app::{App, AppState, Button, InputClock};
use obc_reader::{rgb565_to_rgb888, MapCache, MapTables, Reader, SliceSource};

mod common;
use common::{build_min_obcm, down, keys, Buf};

/// True-color palette color the host `color_fn` resolves a hint hue to.
fn rgb(c: u16) -> Rgb888 {
    let (r, g, b) = rgb565_to_rgb888(c);
    Rgb888::new(r, g, b)
}

/// Render one frame of `app` over `bytes` into a fresh 240×320 buffer (true-color) —
/// the real device size, so each control's bulge lands in its own screen half (its
/// fixed base width can span more than half of a smaller buffer).
fn render(app: &mut App, bytes: &[u8]) -> Buf {
    let cache = MapCache::new();
    let src = SliceSource(bytes);
    let tables = MapTables::parse(&src).expect("valid v7 file");
    let reader = Reader::new(&src, &tables, &cache);
    let mut buf = Buf::new(240, 320);
    app.render_frame(&mut buf, &reader, None, 240.0, 320.0, rgb);
    buf
}

/// Hold `button` from 0 ms, then render the frame sampled at `at_ms`.
fn render_hold(bytes: &[u8], button: Button, at_ms: u32) -> Buf {
    let mut app = App::new(AppState::new(0, 0, 0.05));
    app.handle_input(InputClock(0), &mut keys(&[down(button)]));
    app.handle_input(InputClock(at_ms), &mut keys(&[]));
    render(&mut app, bytes)
}

#[test]
fn holding_a_button_bulges_its_edge_a_tap_does_nothing() {
    let bytes = build_min_obcm(0);
    let hud = rgb(palette::HUD); // the near-black bulge color

    // Idle baseline: any standing near-black chrome in the edge band (so the held
    // frames below measure only the bulge's *extra* pixels, not whatever the screen
    // already draws there).
    let mut app = App::new(AppState::new(0, 0, 0.05));
    app.handle_input(InputClock(0), &mut keys(&[]));
    let (i_top, i_bot) = render(&mut app, &bytes).edge_halves(hud);

    // Hold the encoder past the dead zone: a bulge swells the top half. Its base spans `2*base_half`
    // px, so the quartic shoulders can graze a few px past the midline — hence require the top-half
    // growth to dominate rather than the bottom count to be zero.
    let (e_top, e_bot) = render_hold(&bytes, Button::Encoder, 300).edge_halves(hud);
    assert!(e_top > i_top, "encoder hold ⇒ a bulge swells the top of the right edge");
    assert!(
        e_top - i_top > 10 * (e_bot - i_bot),
        "the encoder bulge belongs to the top half (top +{}, bottom +{})",
        e_top - i_top,
        e_bot - i_bot,
    );

    // Hold Back instead: a bulge in the *bottom* half, the top untouched.
    let (b_top, b_bot) = render_hold(&bytes, Button::Back, 300).edge_halves(hud);
    assert!(b_bot > i_bot, "Back hold ⇒ a bulge swells the bottom of the right edge");
    assert_eq!(b_top, i_top, "the Back bulge stays out of the top half");

    // Just-pressed, still inside the dead zone (50 ms of a 500 ms hold ⇒ 10% < DEAD):
    // a tap-length press swells nothing, so a quick click never flashes a bulge.
    let early = render_hold(&bytes, Button::Encoder, 50).edge_halves(hud);
    assert_eq!(early, (i_top, i_bot), "inside the dead zone a press shows no bulge");
}
