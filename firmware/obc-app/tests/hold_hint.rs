//! Wiring test for the global long-press hint in [`App::render_frame`]: holding the
//! encoder swells a black "frame bulge" into the right edge near the top, holding
//! Back swells one near the bottom, and a quick tap swells neither. Renders against a
//! tiny in-memory `DrawTarget` over a minimal `.obcm` whose map is a flat sea
//! backdrop, then compares each held frame to the idle frame so any standing chrome
//! cancels out and only the bulge's extra near-black pixels are measured.

use embedded_graphics::{pixelcolor::Rgb888, prelude::*, primitives::Rectangle};
use obc_app::screen::palette;
use obc_app::{App, AppState, Button, ButtonEvent, InputClock, InputEvent, InputSource};
use obc_reader::{rgb565_to_rgb888, Reader};

/// A minimal valid file: one sea-backdrop style, one empty LOD leaf, no chunks — the
/// map is a flat backdrop, so every non-sea pixel comes from the overlay.
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
    /// Count pixels of color `c` in the right-edge band, split by screen half:
    /// returns `(top_half, bottom_half)`. The bulge pokes in from `x = w`, so it
    /// always lands in `x >= w - 20`.
    fn edge_halves(&self, c: Rgb888) -> (usize, usize) {
        let (mut top, mut bot) = (0, 0);
        for y in 0..self.h {
            for x in (self.w - 20).max(0)..self.w {
                if self.px[(y * self.w + x) as usize] == c {
                    if y < self.h / 2 {
                        top += 1;
                    } else {
                        bot += 1;
                    }
                }
            }
        }
        (top, bot)
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

/// True-color palette color the host `color_fn` resolves a hint hue to.
fn rgb(c: u16) -> Rgb888 {
    let (r, g, b) = rgb565_to_rgb888(c);
    Rgb888::new(r, g, b)
}

/// Render one frame of `app` over `bytes` into a fresh 240×320 buffer (true-color) —
/// the real device size, so each control's bulge lands in its own screen half (its
/// fixed base width can span more than half of a smaller buffer).
fn render(app: &mut App, bytes: &[u8]) -> Buf {
    let reader = Reader::new(bytes).expect("valid v5 file");
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
    let bytes = build_min_obcm();
    let hud = rgb(palette::HUD); // the near-black bulge color

    // Idle baseline: any standing near-black chrome in the edge band (so the held
    // frames below measure only the bulge's *extra* pixels, not whatever the screen
    // already draws there).
    let mut app = App::new(AppState::new(0, 0, 0.05));
    app.handle_input(InputClock(0), &mut keys(&[]));
    let (i_top, i_bot) = render(&mut app, &bytes).edge_halves(hud);

    // Hold the encoder past the dead zone (300 ms of a 500 ms threshold): a bulge
    // swells the *top* half of the right edge, the bottom half is untouched.
    let (e_top, e_bot) = render_hold(&bytes, Button::Encoder, 300).edge_halves(hud);
    assert!(e_top > i_top, "encoder hold ⇒ a bulge swells the top of the right edge");
    assert_eq!(e_bot, i_bot, "the encoder bulge stays out of the bottom half");

    // Hold Back instead: a bulge in the *bottom* half, the top untouched.
    let (b_top, b_bot) = render_hold(&bytes, Button::Back, 300).edge_halves(hud);
    assert!(b_bot > i_bot, "Back hold ⇒ a bulge swells the bottom of the right edge");
    assert_eq!(b_top, i_top, "the Back bulge stays out of the top half");

    // Just-pressed, still inside the dead zone (50 ms of a 500 ms hold ⇒ 10% < DEAD):
    // a tap-length press swells nothing, so a quick click never flashes a bulge.
    let early = render_hold(&bytes, Button::Encoder, 50).edge_halves(hud);
    assert_eq!(early, (i_top, i_bot), "inside the dead zone a press shows no bulge");
}
