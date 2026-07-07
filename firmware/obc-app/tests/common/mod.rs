//! Shared helpers for the `obc-app` integration tests:
//!
//! - [`Buf`] — a recording `Rgb888` `DrawTarget` with per-test accessors ([`Buf::count`],
//!   [`Buf::get`], [`Buf::edge_halves`]).
//! - [`build_min_obcm`] — the minimal flat-backdrop `.obcm` builder.
//! - The scripted hardware: [`Keys`] / [`keys`] / [`down`] / [`up`] / [`turn`] / [`tap`] inputs, and
//!   the [`LocationSource`] stand-ins [`ReplayFix`] (replay forever) vs [`OnceFix`] (emit once).
//!
//! `#[allow(dead_code)]` keeps unused-per-binary items from warning.

#![allow(dead_code)]

use std::collections::VecDeque;

use embedded_graphics::{pixelcolor::Rgb888, prelude::*, primitives::Rectangle};
use obc_app::{Button, ButtonEvent, Fix, InputEvent, InputSource, LocationSource};

// Recording DrawTarget.

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

// Minimal OBCM fixture.

/// A minimal valid v10 `.obcm`: one sea-backdrop style, one LOD with a single empty leaf and no
/// chunks, an empty POI directory (six empty categories), and an empty hours pool. It renders as a
/// flat backdrop, so the only non-backdrop pixels come from whatever is drawn on top — making
/// overlays/markers trivial to detect. `marker` is the header's marker color (pass `0` when ignored).
pub fn build_min_obcm(marker: u16) -> Vec<u8> {
    build_min_obcm_profiles(marker, &["Default"])
}

/// [`build_min_obcm`] with a caller-chosen §8.6 profile table (1..=8 names, every multiplier the
/// neutral 1.0×) — for the N5 bike-type tests, which need a map carrying several named profiles.
pub fn build_min_obcm_profiles(marker: u16, profiles: &[&str]) -> Vec<u8> {
    // v8 header is 40 bytes; the style table follows immediately.
    let style_off: u32 = 40;
    // Style table (v10, 8-byte record): count=1, then (id=1, z=0, color=0x001F blue sea, weight=1,
    // flags=0, color2=0x0000 — solid, no secondary color).
    let mut styles = vec![1u8];
    styles.push(1);
    styles.push(0);
    styles.extend_from_slice(&0x001Fu16.to_le_bytes());
    styles.push(1);
    styles.push(0); // flags byte
    styles.extend_from_slice(&0x0000u16.to_le_bytes()); // color2 (absent ⇒ 0x0000)

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

    // POI section starts right after the index (no LOD chunks here). Empty directory:
    // count=6, chunk_size=512, six 13-byte entries (all node_count/chunk_count 0), then the two
    // v7 pool fields (hours_pool_offset u32 + hours_pool_count u16), then an empty hours pool
    // (a bare `count 0`). The directory length is 3 + 6*13 + 6 = 87.
    let poi_section_off = index_off + index.len();
    let dir_len = 3 + 6 * 13 + 6;
    let after_dir = (poi_section_off + dir_len) as u32; // where the empty pool's `count` sits
    let mut poi_dir = vec![6u8]; // category_count
    poi_dir.extend_from_slice(&512u16.to_le_bytes()); // shared chunk_size
    for id in 1u8..=6 {
        poi_dir.push(id);
        poi_dir.extend_from_slice(&after_dir.to_le_bytes()); // index_offset (zero-length here)
        poi_dir.extend_from_slice(&0u32.to_le_bytes()); // node_count
        poi_dir.extend_from_slice(&0u32.to_le_bytes()); // chunk_count
    }
    poi_dir.extend_from_slice(&after_dir.to_le_bytes()); // hours_pool_offset
    poi_dir.extend_from_slice(&0u16.to_le_bytes()); // hours_pool_count = 0
    poi_dir.extend_from_slice(&0u16.to_le_bytes()); // the empty pool's own `count u16` = 0

    // Empty v9 nav section at the tail: the 28-byte directory + the always-present §8.6 profile
    // table (the caller's names, every multiplier 16 = 1.0×). Zero-length index + edge pool
    // "start" just past the profile table.
    let nav_section_off = poi_section_off + poi_dir.len();
    let profile_table_off = (nav_section_off + 28) as u32;
    let mut profile_table = Vec::new();
    for name in profiles {
        let base = profile_table.len();
        profile_table.extend_from_slice(name.as_bytes());
        profile_table.resize(base + 12, 0xFF); // 0xFF-padded 12-byte name
        profile_table.extend_from_slice(&[16u8; 32]); // highway multipliers (1.0×)
        profile_table.extend_from_slice(&[16u8; 8]); // surface multipliers (1.0×)
    }
    let after_nav = profile_table_off + profile_table.len() as u32;
    let mut nav_dir = Vec::new();
    nav_dir.extend_from_slice(&after_nav.to_le_bytes()); // index_offset (zero-length)
    nav_dir.extend_from_slice(&0u32.to_le_bytes()); // index_node_count
    nav_dir.extend_from_slice(&0u32.to_le_bytes()); // node_chunk_count
    nav_dir.extend_from_slice(&after_nav.to_le_bytes()); // edge_pool_offset (zero-length)
    nav_dir.extend_from_slice(&0u32.to_le_bytes()); // edge_chunk_count
    nav_dir.extend_from_slice(&512u16.to_le_bytes()); // chunk_size (pinned)
    nav_dir.extend_from_slice(&profile_table_off.to_le_bytes()); // profile_table_offset
    nav_dir.push(profiles.len() as u8); // profile_count
    nav_dir.push(0); // reserved
    nav_dir.extend_from_slice(&profile_table);

    let mut f = Vec::new();
    f.extend_from_slice(b"OBCM");
    f.push(10);
    for v in [-1000i32, -1000, 1000, 1000] {
        f.extend_from_slice(&v.to_le_bytes()); // bbox: min_lat, min_lon, max_lat, max_lon
    }
    f.extend_from_slice(&style_off.to_le_bytes());
    f.push(1); // lod count
    f.extend_from_slice(&(lod_tab_off as u32).to_le_bytes());
    f.extend_from_slice(&marker.to_le_bytes());
    f.extend_from_slice(&(poi_section_off as u32).to_le_bytes());
    f.extend_from_slice(&(nav_section_off as u32).to_le_bytes());
    f.extend_from_slice(&styles);
    f.extend_from_slice(&table);
    f.extend_from_slice(&index);
    f.extend_from_slice(&poi_dir);
    f.extend_from_slice(&nav_dir);
    f
}

// Scripted hardware.

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

// Location sources. Two disciplines, kept under distinct names.

/// A `LocationSource` that replays the same fix on every poll — stands in for the simulator's
/// control-panel override.
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
