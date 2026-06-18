//! Wiring test for the global long-press hint in [`App::render_frame`]: holding the
//! encoder paints an amber pill on the right edge, holding Back a teal one, a quick
//! tap paints neither, and each pill sits in its own half of the edge (encoder up
//! top, Back below). Renders against a tiny in-memory `DrawTarget` over a minimal
//! `.obcm` whose map is a flat sea backdrop, so the only edge pixels are the hint.

use embedded_graphics::{pixelcolor::Rgb888, prelude::*, primitives::Rectangle};
use obcm_app::screen::palette;
use obcm_app::{App, AppState, Button, ButtonEvent, InputClock, InputEvent, InputSource};
use obcm_reader::{rgb565_to_rgb888, Reader};

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
    /// returns `(top_half, bottom_half)`. The hint pills live at `x >= w - 20`.
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

/// Render one frame of `app` over `bytes` into a fresh 120×120 buffer (true-color).
fn render(app: &mut App, bytes: &[u8]) -> Buf {
    let reader = Reader::new(bytes).expect("valid v5 file");
    let mut buf = Buf::new(120, 120);
    app.render_frame(&mut buf, &reader, None, 120.0, 120.0, |c| rgb(c));
    buf
}

#[test]
fn holding_a_button_paints_its_edge_pill_a_tap_paints_nothing() {
    let bytes = build_min_obcm();
    let amber = rgb(palette::AMBER);
    let teal = rgb(palette::TEAL);

    // Idle: no held button → no hint pixels on the edge at all.
    let mut app = App::new(AppState::new(0, 0, 0.05));
    app.handle_input(InputClock(0), &mut keys(&[]));
    let idle = render(&mut app, &bytes);
    assert_eq!(idle.edge_halves(amber), (0, 0), "idle ⇒ no encoder pill");
    assert_eq!(idle.edge_halves(teal), (0, 0), "idle ⇒ no Back pill");

    // Hold the encoder past the dead zone (down at 0, sampled mid-hold at 300 ms of a
    // 500 ms threshold): an amber pill fills in the *top* half, no teal anywhere.
    let mut app = App::new(AppState::new(0, 0, 0.05));
    app.handle_input(InputClock(0), &mut keys(&[down(Button::Encoder)]));
    app.handle_input(InputClock(300), &mut keys(&[]));
    let held = render(&mut app, &bytes);
    let (a_top, a_bot) = held.edge_halves(amber);
    assert!(a_top > 0, "encoder hold ⇒ amber pill on the right edge");
    assert_eq!(a_bot, 0, "the encoder pill stays in the top half");
    assert_eq!(held.edge_halves(teal), (0, 0), "encoder hold doesn't paint the Back pill");

    // Hold Back instead: a teal pill in the *bottom* half, no amber.
    let mut app = App::new(AppState::new(0, 0, 0.05));
    app.handle_input(InputClock(0), &mut keys(&[down(Button::Back)]));
    app.handle_input(InputClock(300), &mut keys(&[]));
    let held = render(&mut app, &bytes);
    let (t_top, t_bot) = held.edge_halves(teal);
    assert!(t_bot > 0, "Back hold ⇒ teal pill on the right edge");
    assert_eq!(t_top, 0, "the Back pill stays in the bottom half");
    assert_eq!(held.edge_halves(amber), (0, 0), "Back hold doesn't paint the encoder pill");

    // Just-pressed, still inside the dead zone (50 ms of a 500 ms hold ⇒ 10% < DEAD):
    // a tap-length press shows nothing, so a quick click never flashes a pill.
    let mut app = App::new(AppState::new(0, 0, 0.05));
    app.handle_input(InputClock(0), &mut keys(&[down(Button::Encoder)]));
    app.handle_input(InputClock(50), &mut keys(&[]));
    let early = render(&mut app, &bytes);
    assert_eq!(early.edge_halves(amber), (0, 0), "inside the dead zone a press shows nothing");
}
