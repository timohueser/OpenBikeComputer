//! Shared helpers for the `obc-app` integration tests.
//!
//! Three families of fixtures were copy-pasted across the per-test files; this module
//! is the single source so identical names can't drift into different behaviour:
//!
//! - [`Buf`] — the recording `Rgb888` `DrawTarget` (was duplicated in `marker.rs`,
//!   `hold_hint.rs`, `overlay_plane.rs`, `screens.rs`). It carries the superset of the
//!   per-test accessors ([`Buf::count`], [`Buf::get`], [`Buf::edge_halves`]).
//! - [`build_min_obcm`] — the minimal flat-backdrop `.obcm` builder (was duplicated in
//!   `marker.rs` / `screens.rs`, and as a marker-less variant in `hold_hint.rs` /
//!   `overlay_plane.rs`; those now pass `0`).
//! - The scripted hardware: [`Keys`] / [`keys`] / [`down`] / [`up`] / [`turn`] / [`tap`]
//!   inputs, and the [`LocationSource`] stand-ins. The two replay disciplines that used
//!   to share names ("replay this fix forever" vs "emit it once") are now the distinct
//!   [`ReplayFix`] and [`OnceFix`], so a name means one thing.
//!
//! Not every test uses every helper, so `#[allow(dead_code)]` keeps the
//! unused-per-binary items from warning.

#![allow(dead_code)]

use std::collections::VecDeque;

use embedded_graphics::{pixelcolor::Rgb888, prelude::*, primitives::Rectangle};
use obc_app::{Button, ButtonEvent, Fix, InputEvent, InputSource, LocationSource};

// ---------------------------------------------------------------------------
// Recording DrawTarget.
// ---------------------------------------------------------------------------

/// A `w`×`h` `Rgb888` buffer implementing `DrawTarget`, with clipped writes.
pub struct Buf {
    pub w: i32,
    pub h: i32,
    pub px: Vec<Rgb888>,
}

impl Buf {
    pub fn new(w: i32, h: i32) -> Self {
        Buf { w, h, px: vec![Rgb888::BLACK; (w * h) as usize] }
    }
    pub fn get(&self, x: i32, y: i32) -> Rgb888 {
        self.px[(y * self.w + x) as usize]
    }
    pub fn count(&self, c: Rgb888) -> usize {
        self.px.iter().filter(|&&p| p == c).count()
    }
    pub fn put(&mut self, x: i32, y: i32, c: Rgb888) {
        if x >= 0 && y >= 0 && x < self.w && y < self.h {
            self.px[(y * self.w + x) as usize] = c;
        }
    }
    /// Count pixels of color `c` in the right-edge band, split by screen half: returns
    /// `(top_half, bottom_half)`. The bulge pokes in from `x = w`, so it always lands in
    /// `x >= w - 20`.
    pub fn edge_halves(&self, c: Rgb888) -> (usize, usize) {
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

// ---------------------------------------------------------------------------
// Minimal OBCM fixture.
// ---------------------------------------------------------------------------

/// A minimal valid v5 `.obcm`: one sea-backdrop style, one LOD with a single empty leaf
/// and no chunks. The map renders as a flat backdrop, so the only non-backdrop pixels
/// come from whatever is drawn on top — making overlays/markers trivial to detect.
/// `marker` is the header's marker color (pass `0` when the test ignores it).
pub fn build_min_obcm(marker: u16) -> Vec<u8> {
    let style_off: u32 = 32;
    // Style table: count=1, then (id=1, z=0, color=0x001F blue sea, weight=1, flags=0).
    let mut styles = vec![1u8];
    styles.push(1);
    styles.push(0);
    styles.extend_from_slice(&0x001Fu16.to_le_bytes());
    styles.push(1);
    styles.push(0); // flags byte

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
    f.push(5);
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

// ---------------------------------------------------------------------------
// Scripted hardware.
// ---------------------------------------------------------------------------

/// A scripted `InputSource` draining a queue of raw input events, one per `poll`.
pub struct Keys(pub VecDeque<InputEvent>);

impl InputSource for Keys {
    fn poll(&mut self) -> Option<InputEvent> {
        self.0.pop_front()
    }
}

/// Build a [`Keys`] source from a slice of events.
pub fn keys(evs: &[InputEvent]) -> Keys {
    Keys(evs.iter().copied().collect())
}

pub fn turn(n: i32) -> InputEvent {
    InputEvent::Turn(n)
}
pub fn down(b: Button) -> InputEvent {
    InputEvent::Button(ButtonEvent::Down(b))
}
pub fn up(b: Button) -> InputEvent {
    InputEvent::Button(ButtonEvent::Up(b))
}
/// A tap (down then up within the hold threshold) → a `Press` (Encoder) or `Back` gesture.
pub fn tap(b: Button) -> [InputEvent; 2] {
    [down(b), up(b)]
}

// ---------------------------------------------------------------------------
// Location sources. Two disciplines, kept under distinct names.
// ---------------------------------------------------------------------------

/// A `LocationSource` that replays the same fix on **every** poll — stands in for the
/// simulator's control-panel override (which holds the last value).
pub struct ReplayFix(pub Option<Fix>);
impl LocationSource for ReplayFix {
    fn poll(&mut self) -> Option<Fix> {
        self.0
    }
}

/// A `LocationSource` that emits its fix exactly **once**, then `None` — the real
/// one-fresh-fix-per-tick contract (no per-poll replay).
pub struct OnceFix(pub Option<Fix>);
impl LocationSource for OnceFix {
    fn poll(&mut self) -> Option<Fix> {
        self.0.take()
    }
}

/// A `LocationSource` that never has a fix.
pub struct NoFix;
impl LocationSource for NoFix {
    fn poll(&mut self) -> Option<Fix> {
        None
    }
}
