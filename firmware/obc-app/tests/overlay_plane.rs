//! The map / overlay plane split ([`App::render_map`] + [`App::render_overlay`],
//! issue #45). Two contracts are pinned here:
//!
//! 1. **Compositing isolation** — `render_overlay` over an already-rendered map must
//!    touch *only* its own pixels (the hold bulge), never clear or repaint the map. We
//!    render the map, snapshot it, draw the overlay over it, and assert every changed
//!    pixel is in the right-edge overlay band and is the bulge colour — so the whole
//!    map area is byte-identical with and without the overlay.
//! 2. **Liveness** — `App::overlay_active()` is true exactly across a hold's
//!    charge → pop (and across an early-release retract), and false otherwise, so a
//!    host can repaint the overlay layer only when it would change a pixel.

use embedded_graphics::{pixelcolor::Rgb888, prelude::*, primitives::Rectangle};
use obc_app::screen::palette;
use obc_app::{App, AppState, Button, ButtonEvent, InputClock, InputEvent, InputSource};
use obc_reader::{rgb565_to_rgb888, Reader};

/// A minimal valid file: one sea-backdrop style, one empty LOD leaf, no chunks — the
/// map is a flat backdrop, so every non-sea pixel the overlay adds is its own.
fn build_min_obcm() -> Vec<u8> {
    let style_off: u32 = 32;
    let mut styles = vec![1u8, 1, 0];
    styles.extend_from_slice(&0x001Fu16.to_le_bytes());
    styles.push(1);
    styles.push(0);

    let lod_tab_off = style_off as usize + styles.len();
    let index_off = lod_tab_off + 18;

    let mut table = Vec::new();
    table.extend_from_slice(&f32::INFINITY.to_le_bytes());
    table.extend_from_slice(&(index_off as u32).to_le_bytes());
    table.extend_from_slice(&1u32.to_le_bytes());
    table.extend_from_slice(&16u16.to_le_bytes());
    table.extend_from_slice(&0u32.to_le_bytes());

    let index = 0x7FFF_FFFFu32.to_le_bytes();

    let mut f = Vec::new();
    f.extend_from_slice(b"OBCM");
    f.push(5);
    for v in [-1000i32, -1000, 1000, 1000] {
        f.extend_from_slice(&v.to_le_bytes());
    }
    f.extend_from_slice(&style_off.to_le_bytes());
    f.push(1);
    f.extend_from_slice(&(lod_tab_off as u32).to_le_bytes());
    f.extend_from_slice(&0u16.to_le_bytes()); // marker color (unused here)
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

/// A scripted `InputSource` draining a queue of events.
struct Keys(std::collections::VecDeque<InputEvent>);
impl InputSource for Keys {
    fn poll(&mut self) -> Option<InputEvent> {
        self.0.pop_front()
    }
}
fn keys(evs: &[InputEvent]) -> Keys {
    Keys(evs.iter().copied().collect())
}
fn down(b: Button) -> InputEvent {
    InputEvent::Button(ButtonEvent::Down(b))
}
fn up(b: Button) -> InputEvent {
    InputEvent::Button(ButtonEvent::Up(b))
}

/// True-color palette color the host `color_fn` resolves a hint hue to.
fn rgb(c: u16) -> Rgb888 {
    let (r, g, b) = rgb565_to_rgb888(c);
    Rgb888::new(r, g, b)
}

#[test]
fn render_overlay_touches_only_overlay_pixels() {
    let bytes = build_min_obcm();
    let reader = Reader::new(&bytes).expect("valid v5 file");
    let (w, h) = (240i32, 320i32);
    let hud = rgb(palette::HUD); // the near-black bulge color

    // Charge the encoder past the dead zone (300 ms of a 500 ms threshold) so the
    // overlay has a bulge to draw on the right edge.
    let mut app = App::new(AppState::new(0, 0, 0.05));
    app.handle_input(InputClock(0), &mut keys(&[down(Button::Encoder)]));
    app.handle_input(InputClock(300), &mut keys(&[]));

    // Render the map, snapshot it, then composite the overlay over the *same* buffer.
    let mut buf = Buf::new(w, h);
    app.render_map(&mut buf, &reader, None, w as f32, h as f32, rgb);
    let map_only = buf.px.clone();
    app.render_overlay(&mut buf, w as f32, h as f32, rgb);

    // Everything left of the right-edge band must be byte-identical: the overlay never
    // clears or repaints the map. Inside the band, the only changes are the bulge's own
    // HUD-coloured pixels — and there must be some, or we proved nothing.
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
    let bytes = build_min_obcm();
    let reader = Reader::new(&bytes).expect("valid v5 file");
    let (w, h) = (240i32, 320i32);

    // Same app state rendered two ways: the thin `render_frame` convenience vs. the
    // explicit `render_map` + `render_overlay` a dual-layer host would call. Draw order
    // is preserved, so the single-target results must be byte-identical.
    let make_app = || {
        let mut app = App::new(AppState::new(0, 0, 0.05));
        app.handle_input(InputClock(0), &mut keys(&[down(Button::Encoder)]));
        app.handle_input(InputClock(300), &mut keys(&[]));
        app
    };

    let mut whole = Buf::new(w, h);
    make_app().render_frame(&mut whole, &reader, None, w as f32, h as f32, rgb);

    let mut split = Buf::new(w, h);
    let mut app = make_app();
    app.render_map(&mut split, &reader, None, w as f32, h as f32, rgb);
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
    app.handle_input(InputClock(0), &mut keys(&[down(Button::Encoder)]));
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
    app.handle_input(InputClock(0), &mut keys(&[down(Button::Encoder)]));
    app.handle_input(InputClock(300), &mut keys(&[]));
    assert!(app.overlay_active(), "charging ⇒ overlay live");

    app.handle_input(InputClock(310), &mut keys(&[up(Button::Encoder)]));
    assert!(app.overlay_active(), "an early release retracts ⇒ still live");

    // Once the retract finishes (CANCEL_MS = 150) the overlay is quiet again.
    app.handle_input(InputClock(310 + 200), &mut keys(&[]));
    assert!(!app.overlay_active(), "overlay quiet once the retract finishes");
}
