//! The terrain heights of a cell: the Catmull-Rom of `terrain/pages.wgsl` over the stored pages'
//! own 16-bit codes, at level 14 and then 13.
//!
//! The crate leaves out the 16-bit step of the viewer's synthesized pages (under 1 mm for 100 m of
//! relief over a level-18 node). It stops at level 13: where a zoom-12 tile knows no height, the
//! viewer goes on to coarser levels and the crate answers None.

use std::collections::{BTreeMap, BTreeSet};

use crate::frame::{CellKey, GlobalPoint, CELL_UNITS, FRAME_BITS};

/// Texels along a page edge.
pub const PAGE: usize = 256;
/// The code of an unknown height.
pub const UNKNOWN: u16 = 0xffff;
/// The levels of the stored pages, finest first: zoom-13 and zoom-12 tiles.
const LEVELS: [u8; 2] = [14, 13];

/// One stored page: 256 × 256 codes, row by row from north, as the viewer's `PageData` holds them.
/// A height is `offset + code × scale` in `f32`.
#[derive(Clone, Debug, PartialEq)]
pub struct StoredPage {
    pub level: u8,
    pub x: u32,
    pub y: u32,
    pub offset: f32,
    pub scale: f32,
    pub codes: Vec<u16>,
}

/// The terrain node of a page.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PageKey {
    pub level: u8,
    pub x: u32,
    pub y: u32,
}

impl PageKey {
    /// The stored tile (z, x, y) that holds the page, and its quadrant: 0 north-west, 1 north-east,
    /// 2 south-west, 3 south-east.
    pub fn tile(self) -> (u32, u32, u32, usize) {
        (u32::from(self.level) - 1, self.x >> 1, self.y >> 1, ((self.x & 1) | (self.y & 1) << 1) as usize)
    }
}

/// The height of pixel `p` of a Terrarium tile's RGBA bytes: unknown unless it is opaque.
fn terrarium(rgba: &[u8], p: usize) -> Option<f32> {
    let [r, g, b, a] = [rgba[p], rgba[p + 1], rgba[p + 2], rgba[p + 3]];
    (a == 255).then(|| f32::from(r) * 256.0 + f32::from(g) + f32::from(b) / 256.0 - 32768.0)
}

/// The pages of the four quadrants of a 512 px Terrarium tile z/x/y: the port of `tilePages` and
/// `pageData` of `stream/heights.ts`. A quadrant without a known height has no page.
pub fn pages_from_terrarium(
    rgba: &[u8],
    size: usize,
    z: u32,
    x: u32,
    y: u32,
) -> Result<[Option<StoredPage>; 4], String> {
    if size != 2 * PAGE || rgba.len() != 4 * size * size {
        return Err(format!("a terrain tile has {size} px and {} bytes, not {} px", rgba.len(), 2 * PAGE));
    }
    Ok(std::array::from_fn(|q| {
        // A height is exact in f32: 15 bits of metres and 8 of fraction.
        let heights: Vec<Option<f32>> = (0..PAGE * PAGE)
            .map(|t| terrarium(rgba, 4 * ((t / PAGE + (q >> 1) * PAGE) * size + t % PAGE + (q & 1) * PAGE)))
            .collect();
        let known = heights.iter().flatten();
        let min = known.clone().copied().reduce(f32::min)?;
        let max = known.copied().reduce(f32::max)?;
        let scale = ((f64::from(max) - f64::from(min)) / f64::from(UNKNOWN - 1)).max(1e-4);
        let codes = heights
            .iter()
            .map(|h| h.map_or(UNKNOWN, |h| ((f64::from(h) - f64::from(min)) / scale).round() as u16))
            .collect();
        let (dx, dy) = ((q & 1) as u32, (q >> 1) as u32);
        Some(StoredPage { level: (z + 1) as u8, x: 2 * x + dx, y: 2 * y + dy, offset: min, scale: scale as f32, codes })
    }))
}

/// The stored pages of a cell.
#[derive(Clone, Debug, Default)]
pub struct Terrain {
    pages: BTreeMap<PageKey, StoredPage>,
}

/// Catmull-Rom weights of the four texels about a part t, as `spline` of `pages.wgsl`.
fn spline(t: f64) -> [f64; 4] {
    let (t2, t3) = (t * t, t * t * t);
    [0.5 * (-t + 2.0 * t2 - t3), 0.5 * (2.0 - 5.0 * t2 + 3.0 * t3), 0.5 * (t + 4.0 * t2 - 3.0 * t3), 0.5 * (-t2 + t3)]
}

impl Terrain {
    /// The terrain of a cell from the pages that `wanted(key)` lists; it drops any other page, so
    /// the heights depend on the cell alone, whatever more a caller gives.
    pub fn new(key: CellKey, pages: Vec<StoredPage>) -> Terrain {
        let wanted: BTreeSet<PageKey> = Terrain::wanted(key).into_iter().collect();
        let pages = pages.into_iter().map(|page| (PageKey { level: page.level, x: page.x, y: page.y }, page));
        Terrain { pages: pages.filter(|(key, _)| wanted.contains(key)).collect() }
    }

    /// The pages that a cell reads: per level, each page that holds a texel of the cell grown by its
    /// margin and 2 texels, the reach of the bicubic. A caller gives each of them that exists.
    pub fn wanted(key: CellKey) -> Vec<PageKey> {
        let (o, m) = (key.origin(), key.margin());
        let mut out = Vec::new();
        for level in LEVELS {
            let texel = 1i64 << texel_shift(level);
            let (lo, hi) = (|v: i64| v - m - 2 * texel, |v: i64| v + CELL_UNITS + m + 2 * texel);
            let shift = texel_shift(level) + 8;
            let last = (1i64 << level) - 1;
            let span = |v: i64| (lo(v) >> shift).max(0)..=(hi(v) >> shift).min(last);
            for y in span(o.y) {
                out.extend(span(o.x).map(|x| PageKey { level, x: x as u32, y: y as u32 }));
            }
        }
        out
    }

    /// The height in metres at a point: the bicubic of the finest stored level whose 16 texels about
    /// the point are known. It steps where the level changes, as the GPU's does.
    pub fn height(&self, p: GlobalPoint) -> Option<f64> {
        LEVELS.iter().find_map(|&level| self.bicubic(level, p))
    }

    fn bicubic(&self, level: u8, p: GlobalPoint) -> Option<f64> {
        let s = texel_shift(level);
        let (vx, vy) = (p.x - (1 << (s - 1)), p.y - (1 << (s - 1)));
        let mask = (1i64 << s) - 1;
        let size = (1i64 << s) as f64;
        let (wx, wy) = (spline((vx & mask) as f64 / size), spline((vy & mask) as f64 / size));
        let (tx, ty) = (vx >> s, vy >> s);
        let mut page: Option<&StoredPage> = None;
        let mut sum = 0.0;
        for (j, wy) in wy.iter().enumerate() {
            for (i, wx) in wx.iter().enumerate() {
                let (x, y) = (tx + i as i64 - 1, ty + j as i64 - 1);
                if x < 0 || y < 0 {
                    return None;
                }
                let key = PageKey { level, x: (x >> 8) as u32, y: (y >> 8) as u32 };
                let held = match page {
                    Some(held) if held.x == key.x && held.y == key.y => held,
                    _ => *page.insert(self.pages.get(&key)?),
                };
                let code = held.codes[(y & 255) as usize * PAGE + (x & 255) as usize];
                if code == UNKNOWN {
                    return None;
                }
                let h = f64::from(held.offset + f32::from(code) * held.scale);
                sum += h * wx * wy;
            }
        }
        Some(sum)
    }
}

/// log2 of frame units per texel at a level: a texel is 2^(22 − level) world units.
fn texel_shift(level: u8) -> u32 {
    FRAME_BITS - 8 - u32::from(level)
}

#[cfg(test)]
mod tests {
    use super::*;

    const LEVEL: u8 = 14;
    const TEXEL: i64 = 1 << 12;

    /// A quadratic surface in texels of level 14 from (px, py) of page (0, 0) of the area.
    fn quadratic(x: f64, y: f64) -> f64 {
        1000.0 + 0.8 * x - 0.3 * y + 0.002 * x * x + 0.001 * x * y - 0.0015 * y * y
    }

    /// The pages of a level from `first` that hold `surface` at their texel centres.
    fn pages(level: u8, first: (u32, u32), count: u32, surface: impl Fn(f64, f64) -> f64) -> Vec<StoredPage> {
        let mut out = Vec::new();
        for py in first.1..first.1 + count {
            for px in first.0..first.0 + count {
                let heights: Vec<f32> = (0..PAGE * PAGE)
                    .map(|t| {
                        let (x, y) =
                            ((px - first.0) as usize * PAGE + t % PAGE, (py - first.1) as usize * PAGE + t / PAGE);
                        surface(x as f64, y as f64) as f32
                    })
                    .collect();
                let min = heights.iter().copied().reduce(f32::min).unwrap();
                let max = heights.iter().copied().reduce(f32::max).unwrap();
                let scale = ((f64::from(max) - f64::from(min)) / 65534.0).max(1e-4);
                let codes = heights.iter().map(|&h| ((f64::from(h) - f64::from(min)) / scale).round() as u16).collect();
                out.push(StoredPage { level, x: px, y: py, offset: min, scale: scale as f32, codes });
            }
        }
        out
    }

    const FIRST: (u32, u32) = (8572, 5776);
    /// The cell at the south-east corner of the first page: it wants the first 2 × 2 pages of level 14.
    const CELL: CellKey = CellKey { x: 8572 * 4 + 3, y: 5776 * 4 + 3 };

    /// The frame point at texel coordinates (x, y) of level 14 from the first page.
    fn at(x: f64, y: f64) -> GlobalPoint {
        let origin = |page: u32| i64::from(page) * 256 * TEXEL;
        // Texel centres lie half a texel past the texel's corner.
        GlobalPoint {
            x: origin(FIRST.0) + ((x + 0.5) * TEXEL as f64) as i64,
            y: origin(FIRST.1) + ((y + 0.5) * TEXEL as f64) as i64,
        }
    }

    #[test]
    fn the_bicubic_of_a_quadratic_is_the_quadratic_also_across_pages() {
        let terrain = Terrain::new(CELL, pages(LEVEL, FIRST, 2, quadratic));
        let step = 0.02;
        // At a texel centre, the texel's value.
        let centre = terrain.height(at(40.0, 70.0)).unwrap();
        assert!((centre - quadratic(40.0, 70.0)).abs() < step, "{centre}");
        // Between texels, and with the 16 texels across the page line at 256.
        for (x, y) in [(40.3, 70.75), (255.4, 100.0), (255.6, 255.2), (300.1, 254.9)] {
            let h = terrain.height(at(x, y)).unwrap();
            assert!((h - quadratic(x, y)).abs() < step, "({x}, {y}): {h} for {}", quadratic(x, y));
        }
    }

    #[test]
    fn an_unknown_texel_falls_back_a_level_and_none_past_level_13() {
        let mut fine = pages(LEVEL, FIRST, 1, quadratic);
        fine[0].codes[70 * PAGE + 40] = UNKNOWN;
        // Level 13 holds a plane of 500 m over the same ground.
        let coarse = pages(13, (FIRST.0 / 2, FIRST.1 / 2), 1, |_, _| 500.0);
        let terrain = Terrain::new(CELL, [fine.clone(), coarse.clone()].concat());
        assert!((terrain.height(at(40.2, 70.2)).unwrap() - 500.0).abs() < 1e-3);
        assert!((terrain.height(at(43.0, 70.0)).unwrap() - quadratic(43.0, 70.0)).abs() < 0.02);
        assert_eq!(Terrain::new(CELL, fine).height(at(40.2, 70.2)), None);
        assert_eq!(Terrain::new(CELL, Vec::new()).height(at(40.2, 70.2)), None);
        // A page that the cell does not want is dropped.
        let far = CellKey { x: CELL.x + 40, y: CELL.y };
        assert_eq!(Terrain::new(far, coarse).height(at(43.0, 70.0)), None);
    }

    #[test]
    fn terrarium_pages_are_those_of_heights_ts() {
        let size = 512;
        let mut rgba = vec![0u8; 4 * size * size];
        for j in 0..size {
            for i in 0..size {
                // 1000 m + i / 8 m + j / 256 m, in 1/256 m.
                let code = (32768 + 1000) * 256 + 32 * i as u32 + j as u32;
                let p = 4 * (j * size + i);
                rgba[p..p + 4].copy_from_slice(&[(code >> 16) as u8, (code >> 8) as u8, code as u8, 255]);
            }
        }
        rgba[3] = 0;
        let pages = pages_from_terrarium(&rgba, size, 13, 4286, 2888).unwrap();
        let west = pages[0].as_ref().unwrap();
        assert_eq!((west.level, west.x, west.y), (14, 8572, 5776));
        // Pixel (0, 0) is transparent: the least known height is at (0, 1).
        let (min, max): (f64, f64) = (1000.0 + 1.0 / 256.0, 1000.0 + 255.0 / 8.0 + 255.0 / 256.0);
        assert_eq!(west.offset, min as f32);
        let scale = (max - min) / 65534.0;
        assert_eq!(west.scale, scale as f32);
        assert_eq!(west.codes[0], UNKNOWN);
        assert_eq!(west.codes[PAGE], 0);
        assert_eq!(west.codes[PAGE * PAGE - 1], 65534);
        assert_eq!(west.codes[PAGE + 3], ((1000.0 + 3.0 / 8.0 + 1.0 / 256.0 - min) / scale).round() as u16);
        let south_east = pages[3].as_ref().unwrap();
        assert_eq!((south_east.x, south_east.y, south_east.offset), (8573, 5777, (1000.0 + 32.0 + 1.0) as f32));
        assert!(pages_from_terrarium(&rgba[..16], 2, 13, 0, 0).is_err());
        assert!(pages_from_terrarium(&vec![0; 4 * size * size], size, 13, 0, 0).unwrap().iter().all(Option::is_none));
    }

    #[test]
    fn a_cell_at_a_page_corner_wants_the_pages_about_it() {
        // Level-14 pages are 2^20 units, cells 2^18: this cell's origin is a corner of pages of both levels.
        let key = CellKey { x: 8572 * 4, y: 5776 * 4 };
        let wanted = Terrain::wanted(key);
        let of = |level: u8| wanted.iter().filter(|k| k.level == level).map(|k| (k.x, k.y)).collect::<Vec<_>>();
        assert_eq!(of(14), [(8571, 5775), (8572, 5775), (8571, 5776), (8572, 5776)]);
        assert_eq!(of(13), [(4285, 2887), (4286, 2887), (4285, 2888), (4286, 2888)]);
        assert_eq!(PageKey { level: 14, x: 8573, y: 5776 }.tile(), (13, 4286, 2888, 1));
    }
}
