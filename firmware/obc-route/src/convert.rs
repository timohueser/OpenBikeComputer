//! GPX → OBCR conversion (`no_std`).
//!
//! A single streaming pass over the GPX track points ([`GpxScanner`]) that
//! simultaneously: accumulates **exact** ride stats (distance via incremental
//! equirectangular segments, ascent/descent via a smoothed-elevation hysteresis),
//! **decimates** the geometry for storage (1-step-lookahead perpendicular distance,
//! plus a max span that also keeps deltas inside `int16`), and **chunks** the kept
//! points — streaming each finished chunk out through the [`ByteSink`] while keeping
//! only a bounded index in RAM. The header is written last (`patch_at(0, …)`) once the
//! offsets and totals are known. See `OBCR_Spec.md` §4.

use heapless::Vec;

use crate::byte_io::{ByteSink, ByteSource, Error};
use crate::deadband::DeadBand;
use crate::geo::{cos_lat, delta_m, seg_dist_m};
use crate::gpx::GpxScanner;
use crate::reader::{
    ChunkMeta, CHUNK_META_LEN, HEADER_LEN, MAX_POINTS_PER_CHUNK, MAX_ROUTE_CHUNKS, NAME_CAP,
};
use obc_reader::codec::{put_i16, put_i32, put_u16, put_u32};
use obc_reader::BBox;

/// Decimation tolerance: drop a vertex within this perpendicular distance of the chord.
const EPSILON_M: f32 = 1.0;
/// Force a kept vertex at least this often. Also bounds stored deltas to the `int16`
/// range (≈3.6 km lat; ≈3.6 km·cos(lat) lon — safe to ~70° latitude).
const MAX_SPAN_M: f32 = 1200.0;

/// Max bytes of one chunk's record body (`(points-1) × 6`).
const BODY_CAP: usize = (MAX_POINTS_PER_CHUNK - 1) * 6;

/// Stats computed during conversion (also written into the header).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RouteStats {
    pub point_count: u32,
    pub chunk_count: u32,
    pub total_distance_m: u32,
    pub total_ascent_m: u32,
    pub total_descent_m: u32,
    pub min_ele_m: i16,
    pub max_ele_m: i16,
}

/// Convert a GPX byte source into a `.obcr` written to `sink`, naming the route
/// `name`. Returns the computed [`RouteStats`].
pub fn gpx_to_obcr(
    src: &dyn ByteSource,
    name: &str,
    sink: &mut dyn ByteSink,
) -> Result<RouteStats, Error> {
    // Reserve the header; the body follows immediately (data_offset = HEADER_LEN).
    sink.write(&[0u8; HEADER_LEN])?;

    let mut enc = Encoder::new(HEADER_LEN as u32);
    let mut scan = GpxScanner::new(src);

    // Running stats. `cum_dist` accumulates in `f64`: each per-segment distance is a small
    // `f32` (see `geo`), but a long route's running total needs the dynamic range — `f32`'s
    // ~7 significant digits resolve a 300 km total to only ~3 cm and could drift over
    // thousands of segments.
    let mut cum_dist = 0f64;
    // Dead-banded ascent/descent (shared with the elevation profile + app climb).
    let mut elev = DeadBand::<f64>::new();
    let mut prev: Option<(i32, i32)> = None;
    let mut last_ele = 0f32;
    let mut min_ele = i16::MAX;
    let mut max_ele = i16::MIN;
    let mut bbox: Option<BBox> = None;
    let mut start = (0i32, 0i32);
    let mut emitted = 0u32;

    // Decimation state (1-step lookahead).
    let mut last_kept: Option<Cand> = None;
    let mut pending: Option<Cand> = None;

    while let Some(p) = scan.next_point()? {
        // Distance from the previous raw point.
        if let Some(pr) = prev {
            cum_dist += seg_dist_m(pr, (p.lon, p.lat)) as f64;
        } else {
            start = (p.lon, p.lat);
        }
        prev = Some((p.lon, p.lat));

        // Elevation: carry the last known value when a point lacks <ele>.
        if let Some(e) = p.ele {
            last_ele = e;
            min_ele = min_ele.min(round_i16(e as f64));
            max_ele = max_ele.max(round_i16(e as f64));
        }
        elev.push(last_ele as f64);

        bbox = Some(grow(bbox, p.lon, p.lat));

        // Feed the decimator; each "kept" point is emitted to the encoder.
        let c = Cand {
            lon: p.lon,
            lat: p.lat,
            ele: round_i16(last_ele as f64),
            cum_d: cum_dist as u32,
            cum_a: elev.ascent() as u32,
        };
        match (last_kept, pending) {
            (None, _) => {
                enc.emit(sink, c)?;
                emitted += 1;
                last_kept = Some(c);
            }
            (Some(_), None) => pending = Some(c),
            (Some(lk), Some(pd)) => {
                let perp = perp_dist_m(lk, c, pd);
                let span = (c.cum_d - lk.cum_d) as f32;
                if perp > EPSILON_M || span > MAX_SPAN_M {
                    enc.emit(sink, pd)?;
                    emitted += 1;
                    last_kept = Some(pd);
                }
                pending = Some(c);
            }
        }
    }
    // The final point is always kept.
    if let Some(pd) = pending {
        enc.emit(sink, pd)?;
        emitted += 1;
    }

    if emitted == 0 {
        return Err(Error::Empty);
    }

    enc.finish(sink)?;
    let index_offset = enc.write_index(sink)?;

    let bbox = bbox.unwrap_or(BBox { min_lon: 0, min_lat: 0, max_lon: 0, max_lat: 0 });
    if min_ele > max_ele {
        min_ele = 0;
        max_ele = 0;
    }
    let stats = RouteStats {
        point_count: emitted,
        chunk_count: enc.index.len() as u32,
        total_distance_m: cum_dist as u32,
        total_ascent_m: elev.ascent() as u32,
        total_descent_m: elev.descent() as u32,
        min_ele_m: min_ele,
        max_ele_m: max_ele,
    };

    let header = build_header(name, &bbox, start, index_offset, &stats);
    sink.patch_at(0, &header)?;
    Ok(stats)
}

/// A decimation candidate: a kept point with its cumulative stats.
#[derive(Debug, Clone, Copy)]
struct Cand {
    lon: i32,
    lat: i32,
    ele: i16,
    cum_d: u32,
    cum_a: u32,
}

/// Accumulates kept points into seam-sharing chunks, streaming each finished chunk's
/// body out and collecting its `ChunkMeta` in a bounded resident index.
struct Encoder {
    index: Vec<ChunkMeta, MAX_ROUTE_CHUNKS>,
    cur: Vec<(i32, i32, i16), MAX_POINTS_PER_CHUNK>,
    data_pos: u32,
    chunk_start_dist: u32,
    chunk_start_ascent: u32,
}

impl Encoder {
    fn new(data_offset: u32) -> Self {
        Encoder {
            index: Vec::new(),
            cur: Vec::new(),
            data_pos: data_offset,
            chunk_start_dist: 0,
            chunk_start_ascent: 0,
        }
    }

    fn emit(&mut self, sink: &mut dyn ByteSink, c: Cand) -> Result<(), Error> {
        if self.cur.is_empty() {
            self.chunk_start_dist = c.cum_d;
            self.chunk_start_ascent = c.cum_a;
        }
        let _ = self.cur.push((c.lon, c.lat, c.ele));
        if self.cur.len() == MAX_POINTS_PER_CHUNK {
            self.finalize(sink)?;
            // Reseed the next chunk with this point as the shared seam / anchor.
            self.chunk_start_dist = c.cum_d;
            self.chunk_start_ascent = c.cum_a;
            let _ = self.cur.push((c.lon, c.lat, c.ele));
        }
        Ok(())
    }

    /// Flush the trailing chunk (skipping a lone seam point already in the prior chunk).
    fn finish(&mut self, sink: &mut dyn ByteSink) -> Result<(), Error> {
        if self.cur.len() >= 2 || (self.index.is_empty() && !self.cur.is_empty()) {
            self.finalize(sink)?;
        }
        Ok(())
    }

    fn finalize(&mut self, sink: &mut dyn ByteSink) -> Result<(), Error> {
        let n = self.cur.len();
        if n == 0 {
            return Ok(());
        }
        let (ax, ay, ae) = self.cur[0];
        let mut bbox = BBox { min_lon: ax, min_lat: ay, max_lon: ax, max_lat: ay };
        let mut body: Vec<u8, BODY_CAP> = Vec::new();
        for i in 1..n {
            let (x, y, e) = self.cur[i];
            let (px, py, _) = self.cur[i - 1];
            let _ = body.extend_from_slice(&((x - px) as i16).to_le_bytes());
            let _ = body.extend_from_slice(&((y - py) as i16).to_le_bytes());
            let _ = body.extend_from_slice(&e.to_le_bytes());
        }
        for &(x, y, _) in &self.cur {
            bbox.min_lon = bbox.min_lon.min(x);
            bbox.min_lat = bbox.min_lat.min(y);
            bbox.max_lon = bbox.max_lon.max(x);
            bbox.max_lat = bbox.max_lat.max(y);
        }
        sink.write(&body)?;
        let meta = ChunkMeta {
            bbox,
            anchor_lon: ax,
            anchor_lat: ay,
            anchor_ele: ae,
            point_count: n as u16,
            cum_distance_m: self.chunk_start_dist,
            cum_ascent_m: self.chunk_start_ascent,
            byte_offset: self.data_pos,
            byte_len: body.len() as u32,
        };
        self.data_pos += body.len() as u32;
        self.index.push(meta).map_err(|_| Error::TooLarge)?;
        self.cur.clear();
        Ok(())
    }

    /// Write the chunk index after the chunk bodies; returns its file offset.
    fn write_index(&mut self, sink: &mut dyn ByteSink) -> Result<u32, Error> {
        let index_offset = self.data_pos;
        let mut m = [0u8; CHUNK_META_LEN];
        for cm in &self.index {
            put_i32(&mut m, 0, cm.bbox.min_lon);
            put_i32(&mut m, 4, cm.bbox.min_lat);
            put_i32(&mut m, 8, cm.bbox.max_lon);
            put_i32(&mut m, 12, cm.bbox.max_lat);
            put_i32(&mut m, 16, cm.anchor_lon);
            put_i32(&mut m, 20, cm.anchor_lat);
            put_i16(&mut m, 24, cm.anchor_ele);
            put_u16(&mut m, 26, cm.point_count);
            put_u32(&mut m, 28, cm.cum_distance_m);
            put_u32(&mut m, 32, cm.cum_ascent_m);
            put_u32(&mut m, 36, cm.byte_offset);
            put_u32(&mut m, 40, cm.byte_len);
            sink.write(&m)?;
        }
        Ok(index_offset)
    }
}

fn build_header(
    name: &str,
    bbox: &BBox,
    start: (i32, i32),
    index_offset: u32,
    s: &RouteStats,
) -> [u8; HEADER_LEN] {
    let mut h = [0u8; HEADER_LEN];
    h[0..4].copy_from_slice(b"OBCR");
    h[4] = 1; // version
              // h[5] flags = 0, h[7] reserved = 0

    // Name truncated to NAME_CAP on a char boundary.
    let mut nlen = 0;
    for (i, ch) in name.char_indices() {
        if i + ch.len_utf8() > NAME_CAP {
            break;
        }
        nlen = i + ch.len_utf8();
    }
    h[6] = nlen as u8;
    h[64..64 + nlen].copy_from_slice(&name.as_bytes()[..nlen]);

    put_i32(&mut h, 8, bbox.min_lon);
    put_i32(&mut h, 12, bbox.min_lat);
    put_i32(&mut h, 16, bbox.max_lon);
    put_i32(&mut h, 20, bbox.max_lat);
    put_i32(&mut h, 24, start.0);
    put_i32(&mut h, 28, start.1);
    put_u32(&mut h, 32, s.point_count);
    put_u32(&mut h, 36, s.total_distance_m);
    put_u32(&mut h, 40, s.total_ascent_m);
    put_u32(&mut h, 44, s.total_descent_m);
    put_i16(&mut h, 48, s.min_ele_m);
    put_i16(&mut h, 50, s.max_ele_m);
    put_u32(&mut h, 52, s.chunk_count);
    put_u32(&mut h, 56, index_offset);
    put_u32(&mut h, 60, HEADER_LEN as u32); // data_offset
    h
}

// geometry helpers (local equirectangular meters)
// Segment distance / projection live in `geo` (shared with the elevation profile).

/// Perpendicular distance (m) from point `p` to the chord `a → c`.
fn perp_dist_m(a: Cand, c: Cand, p: Cand) -> f32 {
    let cl = cos_lat(a.lat);
    let (cx, cy) = delta_m((a.lon, a.lat), (c.lon, c.lat), cl);
    let (px, py) = delta_m((a.lon, a.lat), (p.lon, p.lat), cl);
    let len2 = cx * cx + cy * cy;
    if len2 <= 1e-9 {
        return libm::sqrtf(px * px + py * py);
    }
    (cx * py - cy * px).abs() / libm::sqrtf(len2)
}

fn grow(b: Option<BBox>, lon: i32, lat: i32) -> BBox {
    match b {
        None => BBox { min_lon: lon, min_lat: lat, max_lon: lon, max_lat: lat },
        Some(b) => BBox {
            min_lon: b.min_lon.min(lon),
            min_lat: b.min_lat.min(lat),
            max_lon: b.max_lon.max(lon),
            max_lat: b.max_lat.max(lat),
        },
    }
}

fn round_i16(m: f64) -> i16 {
    libm::round(m).clamp(i16::MIN as f64, i16::MAX as f64) as i16
}
