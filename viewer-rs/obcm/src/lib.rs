//! OBCM map format reader.
//!
//! `no_std + alloc` so the exact same parsing/query code runs in the desktop
//! simulator and in the nRF5340 firmware. Currently parses format **v2**; the
//! v3 LOD table (see docs/superpowers/specs/2026-06-16-obcm-lod-design.md) is an
//! additive parse step on top of this.
//!
//! All coordinates are integer microdegrees (1e-6 degrees), as stored in the
//! file. Projection to screen space is the renderer's job.

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;
use alloc::vec::Vec;

pub const HEADER_LEN: usize = 31;
const BRANCH_BIT: u32 = 0x8000_0000;
const EMPTY_LEAF: u32 = 0x7FFF_FFFF;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    TooShort,
    BadMagic,
    BadOffset,
}

/// Axis-aligned bounding box in microdegrees.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BBox {
    pub min_lon: i32,
    pub min_lat: i32,
    pub max_lon: i32,
    pub max_lat: i32,
}

impl BBox {
    #[inline]
    pub fn intersects(&self, o: &BBox) -> bool {
        !(self.max_lon < o.min_lon
            || self.min_lon > o.max_lon
            || self.max_lat < o.min_lat
            || self.min_lat > o.max_lat)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Style {
    pub id: u8,
    pub z_index: i8,
    pub color: u16, // RGB565
    pub weight: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Line,
    Polygon,
}

/// A decoded feature with coordinates in microdegrees.
#[derive(Debug, Clone)]
pub struct Feature {
    pub style_id: u8,
    pub kind: Kind,
    pub exterior: Vec<(i32, i32)>,
    pub interiors: Vec<Vec<(i32, i32)>>,
}

#[inline]
fn rd_u16(d: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([d[o], d[o + 1]])
}
#[inline]
fn rd_u32(d: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]])
}
#[inline]
fn rd_i32(d: &[u8], o: usize) -> i32 {
    i32::from_le_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]])
}

pub struct Reader<'a> {
    data: &'a [u8],
    pub version: u8,
    pub bbox: BBox,
    pub chunk_size: u16,
    index_offset: usize,
    index_len: usize, // number of u32 nodes
    data_start: usize,
    /// Styles indexed by id (0..=255) for O(1) lookup during rendering.
    styles: Vec<Option<Style>>,
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8]) -> Result<Reader<'a>, Error> {
        if data.len() < HEADER_LEN {
            return Err(Error::TooShort);
        }
        if &data[0..4] != b"OBCM" {
            return Err(Error::BadMagic);
        }
        let version = data[4];
        // File order is lat,lon,lat,lon (see serialize.py header pack).
        let min_lat = rd_i32(data, 5);
        let min_lon = rd_i32(data, 9);
        let max_lat = rd_i32(data, 13);
        let max_lon = rd_i32(data, 17);
        let style_offset = rd_u32(data, 21) as usize;
        let index_offset = rd_u32(data, 25) as usize;
        let chunk_size = rd_u16(data, 29);

        if style_offset < HEADER_LEN || index_offset < style_offset || index_offset > data.len() {
            return Err(Error::BadOffset);
        }

        let styles = parse_styles(data, style_offset);
        let index_len = discover_index_len(data, index_offset);
        let data_start = index_offset + index_len * 4;

        Ok(Reader {
            data,
            version,
            bbox: BBox { min_lon, min_lat, max_lon, max_lat },
            chunk_size,
            index_offset,
            index_len,
            data_start,
            styles,
        })
    }

    #[inline]
    pub fn style(&self, id: u8) -> Option<&Style> {
        self.styles.get(id as usize).and_then(|s| s.as_ref())
    }

    #[inline]
    fn node(&self, idx: usize) -> u32 {
        rd_u32(self.data, self.index_offset + idx * 4)
    }

    /// Collect (chunk_id, node_bbox) for every non-empty leaf overlapping `view`.
    pub fn query(&self, view: &BBox) -> Vec<(u32, BBox)> {
        let mut out = Vec::new();
        if self.index_len > 0 {
            self.query_rec(0, self.bbox, view, &mut out);
        }
        out
    }

    fn query_rec(&self, idx: usize, node: BBox, view: &BBox, out: &mut Vec<(u32, BBox)>) {
        if idx >= self.index_len || !node.intersects(view) {
            return;
        }
        let val = self.node(idx);
        if val & BRANCH_BIT == 0 {
            if val != EMPTY_LEAF {
                out.push((val, node));
            }
            return;
        }
        let child = (val & !BRANCH_BIT) as usize;
        // floor-division midpoints to match the Python packer's `//`.
        let mid_lon = (node.min_lon + node.max_lon).div_euclid(2);
        let mid_lat = (node.min_lat + node.max_lat).div_euclid(2);
        // NW, NE, SW, SE
        let kids = [
            BBox { min_lon: node.min_lon, min_lat: mid_lat, max_lon: mid_lon, max_lat: node.max_lat },
            BBox { min_lon: mid_lon, min_lat: mid_lat, max_lon: node.max_lon, max_lat: node.max_lat },
            BBox { min_lon: node.min_lon, min_lat: node.min_lat, max_lon: mid_lon, max_lat: mid_lat },
            BBox { min_lon: mid_lon, min_lat: node.min_lat, max_lon: node.max_lon, max_lat: mid_lat },
        ];
        for (i, kb) in kids.iter().enumerate() {
            self.query_rec(child + i, *kb, view, out);
        }
    }

    /// Decode all features in a chunk. `node` is the leaf's bbox from `query`.
    pub fn decode_chunk(&self, chunk_id: u32, node: &BBox) -> Vec<Feature> {
        let cs = self.chunk_size as usize;
        let start = self.data_start + (chunk_id as usize) * cs;
        let mut features = Vec::new();
        if start + cs > self.data.len() {
            return features;
        }
        let chunk = &self.data[start..start + cs];
        let anchor_base = (node.min_lon, node.min_lat);
        let mut off = 0usize;

        while off + 12 <= cs {
            if chunk[off] == 0xFF {
                break;
            }
            let style_id = chunk[off];
            let ext_pt_count = rd_u16(chunk, off + 1) as usize;
            let ax = rd_i32(chunk, off + 3);
            let ay = rd_i32(chunk, off + 7);
            let flags = chunk[off + 11];
            off += 12;

            let is_16 = flags & 0x01 != 0;
            let is_poly = flags & 0x02 != 0;
            let has_holes = flags & 0x04 != 0;
            let dsize = if is_16 { 2 } else { 1 };

            let anchor = (anchor_base.0 + ax, anchor_base.1 + ay);

            let mut exterior = Vec::with_capacity(ext_pt_count);
            off = read_ring(chunk, off, ext_pt_count, anchor, is_16, dsize, false, &mut exterior);

            let mut interiors = Vec::new();
            if is_poly && has_holes && off < cs {
                let hole_count = chunk[off] as usize;
                off += 1;
                for _ in 0..hole_count {
                    if off + 2 > cs {
                        break;
                    }
                    let hpc = rd_u16(chunk, off) as usize;
                    off += 2;
                    let mut hole = Vec::with_capacity(hpc);
                    off = read_ring(chunk, off, hpc, anchor, is_16, dsize, true, &mut hole);
                    interiors.push(hole);
                }
            }

            features.push(Feature {
                style_id,
                kind: if is_poly { Kind::Polygon } else { Kind::Line },
                exterior,
                interiors,
            });
        }
        features
    }
}

#[allow(clippy::too_many_arguments)]
fn read_ring(
    chunk: &[u8],
    mut off: usize,
    pt_count: usize,
    anchor: (i32, i32),
    is_16: bool,
    dsize: usize,
    is_hole: bool,
    out: &mut Vec<(i32, i32)>,
) -> usize {
    if pt_count == 0 {
        return off;
    }
    let (mut px, mut py) = anchor;
    let num_deltas = if is_hole {
        // holes store all points as deltas (first relative to anchor)
        pt_count
    } else {
        out.push(anchor);
        pt_count - 1
    };
    for _ in 0..num_deltas {
        if off + dsize * 2 > chunk.len() {
            break;
        }
        let (dx, dy) = if is_16 {
            (
                i16::from_le_bytes([chunk[off], chunk[off + 1]]) as i32,
                i16::from_le_bytes([chunk[off + 2], chunk[off + 3]]) as i32,
            )
        } else {
            (chunk[off] as i8 as i32, chunk[off + 1] as i8 as i32)
        };
        off += dsize * 2;
        px += dx;
        py += dy;
        out.push((px, py));
    }
    off
}

fn parse_styles(data: &[u8], style_offset: usize) -> Vec<Option<Style>> {
    let mut styles: Vec<Option<Style>> = (0..256).map(|_| None).collect();
    if style_offset >= data.len() {
        return styles;
    }
    let count = data[style_offset] as usize;
    let mut o = style_offset + 1;
    for _ in 0..count {
        if o + 5 > data.len() {
            break;
        }
        let id = data[o];
        let z_index = data[o + 1] as i8;
        let color = rd_u16(data, o + 2);
        let weight = data[o + 4];
        styles[id as usize] = Some(Style { id, z_index, color, weight });
        o += 5;
    }
    styles
}

/// v2 stores no explicit index length; discover it by walking the tree (v3 will
/// store it). Tracks the highest node index reached; caps iterations so a
/// malformed file can't loop forever.
fn discover_index_len(data: &[u8], index_offset: usize) -> usize {
    let avail = data.len().saturating_sub(index_offset) / 4;
    if avail == 0 {
        return 0;
    }
    let mut max_idx = 0usize;
    let mut stack = Vec::new();
    stack.push(0usize);
    let mut budget = avail; // each valid node is visited once
    while let Some(idx) = stack.pop() {
        if idx >= avail || budget == 0 {
            continue;
        }
        budget -= 1;
        if idx > max_idx {
            max_idx = idx;
        }
        let val = rd_u32(data, index_offset + idx * 4);
        if val & BRANCH_BIT != 0 {
            let child = (val & !BRANCH_BIT) as usize;
            for i in 0..4 {
                if child + i < avail {
                    stack.push(child + i);
                }
            }
        }
    }
    max_idx + 1
}

/// Expand an RGB565 color to RGB888 components.
#[inline]
pub fn rgb565_to_rgb888(c: u16) -> (u8, u8, u8) {
    let r5 = ((c >> 11) & 0x1F) as u8;
    let g6 = ((c >> 5) & 0x3F) as u8;
    let b5 = (c & 0x1F) as u8;
    ((r5 << 3) | (r5 >> 2), (g6 << 2) | (g6 >> 4), (b5 << 3) | (b5 >> 2))
}

/// Quantize an RGB565 color to the LS021B7DD02's 64-color (RGB222) palette,
/// returned expanded to RGB888 so it can be shown on a full-color preview while
/// matching what the device will actually display.
#[inline]
pub fn rgb565_to_device64(c: u16) -> (u8, u8, u8) {
    let (r, g, b) = rgb565_to_rgb888(c);
    // keep the top 2 bits of each channel, expand back (each step = 85)
    let q = |v: u8| (v >> 6) * 85;
    (q(r), q(g), q(b))
}
