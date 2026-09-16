//! Wiring test for the global long-press hint in [`App::render_frame`]: holding Select swells a
//! black "frame bulge" into the right edge near the top, holding Back one near the bottom, and a
//! quick tap neither. Each held frame is compared to the idle frame so any standing chrome cancels
//! out and only the bulge's extra near-black pixels are measured.

use embedded_graphics::pixelcolor::Rgb888;
use obc_app::screen::palette;
use obc_app::{App, AppState, Chord, InputPlane};
use obc_ports::{Button, InputClock};
use obc_reader::{rgb565_to_rgb888, MapCache, MapTables, Reader, SliceSource};

use crate::common::{build_min_obcm, down, keys, up, Buf};

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
    let mut scratch = Box::new(obc_render::RenderScratch::new());
    app.render_frame(Some(&mut scratch), &mut buf, &reader, None, 240.0, 320.0, rgb);
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

    // Select and Back occupy opposite screen halves.
    let (e_top, e_bot) = render_hold(&bytes, Button::Select, 300).edge_halves(hud);
    assert!(e_top > i_top, "Select hold ⇒ a bulge swells the top of the right edge");
    assert_eq!(e_bot, i_bot, "the Select bulge stays out of the bottom half");

    // Hold Back instead: a bulge in the *bottom* half, the top untouched.
    let (b_top, b_bot) = render_hold(&bytes, Button::Back, 300).edge_halves(hud);
    assert!(b_bot > i_bot, "Back hold ⇒ a bulge swells the bottom of the right edge");
    assert_eq!(b_top, i_top, "the Back bulge stays out of the top half");

    // Just-pressed, still inside the dead zone (50 ms of a 500 ms hold ⇒ 10% < DEAD):
    // a tap-length press swells nothing, so a quick click never flashes a bulge.
    let early = render_hold(&bytes, Button::Select, 50).edge_halves(hud);
    assert_eq!(early, (i_top, i_bot), "inside the dead zone a press shows no bulge");
}

fn overlay(plane: &InputPlane) -> Buf {
    let mut buf = Buf::new(240, 320);
    plane.render_overlay(&mut buf, 240.0, 320.0, rgb);
    buf
}

#[test]
fn select_and_back_have_mirrored_shapes() {
    let held = |button| {
        let mut plane = InputPlane::new();
        plane.recognize(InputClock(0), &mut keys(&[down(button)]), |_| {});
        plane.recognize(InputClock(300), &mut keys(&[]), |_| {});
        overlay(&plane)
    };
    let select = held(Button::Select);
    let back = held(Button::Back);
    assert!(select.count(rgb(palette::HUD)) > 0);
    for y in 0..320 {
        for x in 0..240 {
            assert_eq!(select.get(x, y), back.get(x, 319 - y), "mirror at ({x}, {y})");
        }
    }
}

#[test]
fn assistant_hints_charge_pop_and_retract_together_on_both_input_paths() {
    let hud = rgb(palette::HUD);
    for (first, second) in [(Button::Up, Button::Select), (Button::Select, Button::Up)] {
        for release_at in [100, 340, 600] {
            let mut plane = InputPlane::new();
            let mut app = App::new_idle(AppState::new(0, 0, 0.05));
            for (t, evs) in [
                (0, vec![down(first)]),
                (40, vec![down(second)]),
                (release_at - 1, vec![]),
                (release_at, vec![up(first), up(second)]),
                (release_at + 50, vec![]),
                (release_at + 250, vec![]),
            ] {
                let chord = plane.recognize(InputClock(t), &mut keys(&evs), |_| panic!("constituent gesture"));
                app.handle_input(InputClock(t), &mut keys(&evs));
                if t == release_at - 1 && release_at == 600 {
                    assert_eq!(chord, Some(Chord::Assistant));
                }
                let buf = overlay(&plane);
                let mut single = Buf::new(240, 320);
                app.render_overlay(&mut single, 240.0, 320.0, rgb);
                assert_eq!(buf.px, single.px, "input paths at {t}");
                let visible = release_at > 100 && t >= release_at - 1 && t < release_at + 250;
                assert_eq!(buf.count(hud) > 0, visible, "visible at {t}, release {release_at}");
                assert_eq!(plane.overlay_active(), visible);
                let rows = plane.overlay_rows(240, 320);
                for y in 0..320 {
                    for x in 0..240 {
                        assert_eq!(buf.get(x, y), buf.get(239 - x, y), "paired hints at {t}");
                        if buf.get(x, y) == hud {
                            assert!(!(12..228).contains(&x), "only edge pixels");
                            assert!(y < 160, "no Back hint");
                            let (start, count) = rows.expect("visible hints need dirty rows");
                            assert!((start..start + count).contains(&(y as u16)));
                        }
                    }
                }
            }
        }
    }
}
