//! OBCR route reader: header, chunk index, and on-demand chunk decode.
//!
//! [`RouteReader`] loads the fixed header and the (small) chunk index into RAM, then
//! pulls individual geometry chunks through the [`ByteSource`] only when asked — a
//! hundreds-of-km route never has to be resident. It is **monomorphic** (holds a
//! `&dyn ByteSource`), so it can be threaded through the app/render layers without
//! making them generic.

use heapless::{String, Vec};

use crate::byte_io::{ByteSource, Error};
use obcm_reader::BBox;

/// Fixed header length (see `OBCR_Spec.md` §1).
pub const HEADER_LEN: usize = 112;
/// Per-chunk index entry length (§2).
pub const CHUNK_META_LEN: usize = 44;
/// Capacity of the inline route-name field, bytes.
pub const NAME_CAP: usize = 48;
/// Resident chunk-index capacity. The converter raises its per-chunk point budget to
/// keep any route under this, so the index always fits RAM (≈ 22 KB at the cap).
pub const MAX_ROUTE_CHUNKS: usize = 512;
/// Max points a single chunk may hold (bounds the per-chunk decode buffer).
pub const MAX_POINTS_PER_CHUNK: usize = 256;

const MAGIC: &[u8; 4] = b"OBCR";
const VERSION: u8 = 1;

/// One decoded route point: position in microdegrees + elevation in meters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RoutePoint {
    pub lon: i32,
    pub lat: i32,
    pub ele: i16,
}

/// One chunk's index entry — its bbox (for viewport query), the absolute anchor it
/// decodes from, and the cumulative stats at its first point (for remaining-distance/
/// climb). See `OBCR_Spec.md` §2.
#[derive(Debug, Clone, Copy)]
pub struct ChunkMeta {
    pub bbox: BBox,
    pub anchor_lon: i32,
    pub anchor_lat: i32,
    pub anchor_ele: i16,
    pub point_count: u16,
    pub cum_distance_m: u32,
    pub cum_ascent_m: u32,
    pub byte_offset: u32,
    pub byte_len: u32,
}

/// The lightweight route description for the Route menu: everything the list needs,
/// readable from the header alone (no chunk index) — so a catalog scan is one small
/// read per file.
#[derive(Debug, Clone)]
pub struct RouteSummary {
    pub name: String<NAME_CAP>,
    /// Total distance, km (rounded) — the v1 stat display unit.
    pub distance_km: u32,
    /// Total ascent, m.
    pub climb_m: u32,
    pub bbox: BBox,
    /// First route point, for centering the camera on load.
    pub start_lon: i32,
    pub start_lat: i32,
}

impl RouteSummary {
    /// Read just the header into a summary — cheap enough to call for every file when
    /// building the Route-menu catalog.
    pub fn read(src: &dyn ByteSource) -> Result<RouteSummary, Error> {
        let h = read_header(src)?;
        Ok(RouteSummary {
            name: h.name,
            distance_km: (h.total_distance_m + 500) / 1000,
            climb_m: h.total_ascent_m,
            bbox: h.bbox,
            start_lon: h.start_lon,
            start_lat: h.start_lat,
        })
    }
}

/// A parsed route, ready to query and decode. Holds the resident header + chunk index
/// plus a shared borrow of the byte source the chunks stream from.
pub struct RouteReader<'a> {
    src: &'a dyn ByteSource,
    pub bbox: BBox,
    pub start_lon: i32,
    pub start_lat: i32,
    pub point_count: u32,
    pub total_distance_m: u32,
    pub total_ascent_m: u32,
    pub total_descent_m: u32,
    pub min_ele_m: i16,
    pub max_ele_m: i16,
    name: String<NAME_CAP>,
    index: Vec<ChunkMeta, MAX_ROUTE_CHUNKS>,
}

impl<'a> RouteReader<'a> {
    /// Parse the header and chunk index from `src`. Validates magic/version and that
    /// every chunk lies within the source and within the resident buffers.
    pub fn open(src: &'a dyn ByteSource) -> Result<RouteReader<'a>, Error> {
        let h = read_header(src)?;
        if h.chunk_count as usize > MAX_ROUTE_CHUNKS {
            return Err(Error::TooLarge);
        }

        let mut index = Vec::new();
        let mut meta = [0u8; CHUNK_META_LEN];
        for k in 0..h.chunk_count {
            let off = h.index_offset + k * CHUNK_META_LEN as u32;
            src.read_at(off, &mut meta)?;
            let point_count = rd_u16(&meta, 26);
            if point_count as usize > MAX_POINTS_PER_CHUNK {
                return Err(Error::TooLarge);
            }
            let cm = ChunkMeta {
                bbox: BBox {
                    min_lon: rd_i32(&meta, 0),
                    min_lat: rd_i32(&meta, 4),
                    max_lon: rd_i32(&meta, 8),
                    max_lat: rd_i32(&meta, 12),
                },
                anchor_lon: rd_i32(&meta, 16),
                anchor_lat: rd_i32(&meta, 20),
                anchor_ele: rd_i16(&meta, 24),
                point_count,
                cum_distance_m: rd_u32(&meta, 28),
                cum_ascent_m: rd_u32(&meta, 32),
                byte_offset: rd_u32(&meta, 36),
                byte_len: rd_u32(&meta, 40),
            };
            // Bounds-check the chunk's data region up front (no per-decode checks).
            let end = cm.byte_offset.checked_add(cm.byte_len).ok_or(Error::BadOffset)?;
            if end > src.len() {
                return Err(Error::BadOffset);
            }
            index.push(cm).map_err(|_| Error::TooLarge)?;
        }

        Ok(RouteReader {
            src,
            bbox: h.bbox,
            start_lon: h.start_lon,
            start_lat: h.start_lat,
            point_count: h.point_count,
            total_distance_m: h.total_distance_m,
            total_ascent_m: h.total_ascent_m,
            total_descent_m: h.total_descent_m,
            min_ele_m: h.min_ele_m,
            max_ele_m: h.max_ele_m,
            name: h.name,
            index,
        })
    }

    /// The route name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The chunk index (in route order).
    pub fn chunks(&self) -> &[ChunkMeta] {
        &self.index
    }

    /// Cumulative ascent (m) at `dist_m` along the route — the climbing done by that
    /// point. Linearly interpolates the per-chunk [`cum_ascent_m`](ChunkMeta::cum_ascent_m)
    /// the format stores at each chunk's first point, so "climbed"/"to climb" can be read
    /// at any position (the Elevation cursor, later the matched ride position) without
    /// re-summing the geometry. Clamped to `0..=total_ascent_m`.
    pub fn ascent_to(&self, dist_m: u32) -> u32 {
        let chunks = &self.index;
        if chunks.is_empty() {
            return 0;
        }
        // The last chunk whose first point is at or before `dist_m`.
        let mut k = 0;
        while k + 1 < chunks.len() && chunks[k + 1].cum_distance_m <= dist_m {
            k += 1;
        }
        let (d0, asc0) = (chunks[k].cum_distance_m, chunks[k].cum_ascent_m);
        // The segment runs to the next chunk's anchor, or to the route end past the last.
        let (d1, asc1) = match chunks.get(k + 1) {
            Some(next) => (next.cum_distance_m, next.cum_ascent_m),
            None => (self.total_distance_m, self.total_ascent_m),
        };
        if dist_m <= d0 || d1 <= d0 {
            return asc0;
        }
        if dist_m >= d1 {
            return asc1;
        }
        let t = (dist_m - d0) as f32 / (d1 - d0) as f32;
        asc0 + (t * (asc1 - asc0) as f32) as u32
    }

    /// A [`RouteSummary`] for this route (for the menu / centering).
    pub fn summary(&self) -> RouteSummary {
        RouteSummary {
            name: self.name.clone(),
            distance_km: (self.total_distance_m + 500) / 1000,
            climb_m: self.total_ascent_m,
            bbox: self.bbox,
            start_lon: self.start_lon,
            start_lat: self.start_lat,
        }
    }

    /// Decode chunk `k` into `out` (cleared first): its anchor followed by each
    /// delta-stepped point. The chunk's last point equals chunk `k+1`'s anchor (seam
    /// sharing), so adjacent chunks stitch without a gap.
    pub fn decode_chunk(
        &self,
        k: usize,
        out: &mut Vec<RoutePoint, MAX_POINTS_PER_CHUNK>,
    ) -> Result<(), Error> {
        out.clear();
        let m = self.index.get(k).ok_or(Error::BadOffset)?;
        let n = m.point_count as usize;
        if n == 0 {
            return Ok(());
        }
        let _ = out.push(RoutePoint { lon: m.anchor_lon, lat: m.anchor_lat, ele: m.anchor_ele });

        // Remaining n-1 points are fixed 6-byte records; read the chunk in one go.
        let want = (n - 1) * 6;
        let mut buf = [0u8; (MAX_POINTS_PER_CHUNK - 1) * 6];
        let bytes = buf.get_mut(..want).ok_or(Error::TooLarge)?;
        if want > 0 {
            self.src.read_at(m.byte_offset, bytes)?;
        }

        let (mut lon, mut lat) = (m.anchor_lon, m.anchor_lat);
        let mut o = 0;
        for _ in 1..n {
            lon += rd_i16(bytes, o) as i32;
            lat += rd_i16(bytes, o + 2) as i32;
            let ele = rd_i16(bytes, o + 4);
            o += 6;
            let _ = out.push(RoutePoint { lon, lat, ele });
        }
        Ok(())
    }

    /// Visit each chunk whose bbox intersects `view`, in route order, passing its
    /// index `k` and `ChunkMeta`. The caller decodes the ones it wants with
    /// [`decode_chunk`](Self::decode_chunk) into its own reused buffer — keeping the
    /// streaming draw allocation-free.
    pub fn for_each_visible_chunk<F: FnMut(usize, &ChunkMeta)>(&self, view: &BBox, mut f: F) {
        for (k, cm) in self.index.iter().enumerate() {
            if cm.bbox.intersects(view) {
                f(k, cm);
            }
        }
    }
}

/// Parsed header fields (shared by [`RouteReader::open`] and [`RouteSummary::read`]).
struct Header {
    bbox: BBox,
    start_lon: i32,
    start_lat: i32,
    point_count: u32,
    total_distance_m: u32,
    total_ascent_m: u32,
    total_descent_m: u32,
    min_ele_m: i16,
    max_ele_m: i16,
    chunk_count: u32,
    index_offset: u32,
    name: String<NAME_CAP>,
}

fn read_header(src: &dyn ByteSource) -> Result<Header, Error> {
    let mut h = [0u8; HEADER_LEN];
    src.read_at(0, &mut h).map_err(|_| Error::BadOffset)?;
    if &h[0..4] != MAGIC {
        return Err(Error::BadMagic);
    }
    if h[4] != VERSION {
        return Err(Error::BadVersion);
    }
    let name_len = (h[6] as usize).min(NAME_CAP);
    let mut name = String::new();
    if let Ok(s) = core::str::from_utf8(&h[64..64 + name_len]) {
        let _ = name.push_str(s);
    }
    Ok(Header {
        bbox: BBox {
            min_lon: rd_i32(&h, 8),
            min_lat: rd_i32(&h, 12),
            max_lon: rd_i32(&h, 16),
            max_lat: rd_i32(&h, 20),
        },
        start_lon: rd_i32(&h, 24),
        start_lat: rd_i32(&h, 28),
        point_count: rd_u32(&h, 32),
        total_distance_m: rd_u32(&h, 36),
        total_ascent_m: rd_u32(&h, 40),
        total_descent_m: rd_u32(&h, 44),
        min_ele_m: rd_i16(&h, 48),
        max_ele_m: rd_i16(&h, 50),
        chunk_count: rd_u32(&h, 52),
        index_offset: rd_u32(&h, 56),
        name,
    })
}

// Little-endian field readers over an already-read, correctly sized buffer.
#[inline]
fn rd_i16(b: &[u8], o: usize) -> i16 {
    i16::from_le_bytes([b[o], b[o + 1]])
}
#[inline]
fn rd_u16(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
#[inline]
fn rd_i32(b: &[u8], o: usize) -> i32 {
    i32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}
#[inline]
fn rd_u32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}
