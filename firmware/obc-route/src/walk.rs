//! The route walk: a chunk's points decoded lazily from its 7-byte records, each point's
//! along-route distance, and the clip to a distance interval. Every read-side summary is a fold
//! over this one walk.

use obc_formats::io::{rd_i16, ByteSource, Error};
use obc_formats::obcr::POINT_RECORD_LEN;
use obc_map_scene::ground_dist_m;

use crate::reader::{ChunkMeta, RoutePoint, MAX_POINTS_PER_CHUNK};

pub(crate) const BODY_CAP: usize = (MAX_POINTS_PER_CHUNK - 1) * POINT_RECORD_LEN;

/// One chunk's stored records, read in a single `read_at` and validated up front, so a chunk
/// decodes whole or not at all. Filled in place: returned by value, the buffer is copied.
pub(crate) struct Records {
    anchor: RoutePoint,
    point_count: u16,
    body: [u8; BODY_CAP],
}

impl Records {
    pub(crate) fn new() -> Self {
        Records { anchor: chunk_anchor(&ChunkMeta::EMPTY), point_count: 0, body: [0; BODY_CAP] }
    }

    /// Read chunk `m` in place of the chunk held. On an error it holds no points.
    pub(crate) fn read(&mut self, src: &dyn ByteSource, m: &ChunkMeta) -> Result<(), Error> {
        (self.anchor, self.point_count) = (chunk_anchor(m), 0);
        let body = self.body.get_mut(..m.byte_len as usize).ok_or(Error::TooLarge)?;
        if !body.is_empty() {
            src.read_at(m.byte_offset.into(), body)?;
        }
        validate(body)?;
        self.point_count = m.point_count;
        Ok(())
    }

    /// The validated record bytes after the anchor.
    pub(crate) fn body(&self) -> &[u8] {
        &self.body[..usize::from(self.point_count.saturating_sub(1)) * POINT_RECORD_LEN]
    }

    pub(crate) fn points(&self) -> ChunkPoints<'_> {
        ChunkPoints::records(
            (self.point_count > 0).then_some(self.anchor),
            (self.anchor.lon, self.anchor.lat),
            self.body(),
        )
    }
}

/// A chunk's first point. The format stores no surface for it.
pub(crate) fn chunk_anchor(m: &ChunkMeta) -> RoutePoint {
    RoutePoint { lon: m.anchor_lon, lat: m.anchor_lat, ele: m.anchor_ele, surface: 0, elevation_incomplete: false }
}

/// Whole records whose reserved flag bits are clear.
pub(crate) fn validate(body: &[u8]) -> Result<(), Error> {
    let (records, rest) = body.as_chunks::<POINT_RECORD_LEN>();
    if !rest.is_empty() || records.iter().any(|r| r[6] & !15 != 0) {
        return Err(Error::BadOffset);
    }
    Ok(())
}

/// A chunk's points in route order, decoded one record at a time from the route cache or the
/// source.
pub struct ChunkPoints<'a> {
    anchor: Option<RoutePoint>,
    records: core::slice::Iter<'a, [u8; POINT_RECORD_LEN]>,
    at: (i32, i32),
}

impl<'a> ChunkPoints<'a> {
    /// `anchor`, then the points `body`'s records step to from `at`. Records are deltas from the
    /// point before them, so `at` is that point.
    pub(crate) fn records(anchor: Option<RoutePoint>, at: (i32, i32), body: &'a [u8]) -> Self {
        ChunkPoints { anchor, records: body.as_chunks().0.iter(), at }
    }
}

impl Iterator for ChunkPoints<'_> {
    type Item = RoutePoint;

    fn next(&mut self) -> Option<RoutePoint> {
        if let Some(p) = self.anchor.take() {
            return Some(p);
        }
        let r = self.records.next()?;
        Some(step(&mut self.at, r))
    }

    /// Internal iteration (`for_each`, `fold`) visits the anchor once, then runs one tight loop
    /// over the records.
    #[inline]
    fn fold<B, F: FnMut(B, RoutePoint) -> B>(mut self, init: B, mut f: F) -> B {
        let mut acc = init;
        if let Some(p) = self.anchor.take() {
            acc = f(acc, p);
        }
        for r in self.records {
            acc = f(acc, step(&mut self.at, r));
        }
        acc
    }
}

/// The point record `r` steps to from `at`, which it then becomes.
#[inline]
fn step(at: &mut (i32, i32), r: &[u8; POINT_RECORD_LEN]) -> RoutePoint {
    *at = (at.0.wrapping_add(rd_i16(r, 0).into()), at.1.wrapping_add(rd_i16(r, 2).into()));
    RoutePoint { lon: at.0, lat: at.1, ele: rd_i16(r, 4), surface: r[6] & 7, elevation_incomplete: r[6] & 8 != 0 }
}

/// A point on the walk.
#[derive(Clone, Copy)]
pub(crate) struct Step {
    pub(crate) p: RoutePoint,
    /// Metres from the route start.
    pub(crate) along: f64,
    /// Length of the segment that ends at `p`; zero at a chunk's anchor.
    pub(crate) seg: f64,
}

/// The points of a chunk that starts `cum_m` along the route, with their along-route distance.
/// The distance restarts at each chunk's stored `cum_distance_m`, so it cannot drift, and sums the
/// converter's `ground_dist_m` in `f64`. A seam vertex is walked twice, once per chunk.
pub(crate) fn walk(cum_m: u32, points: impl Iterator<Item = RoutePoint>) -> impl Iterator<Item = Step> {
    let mut previous: Option<Step> = None;
    points.map(move |p| {
        let step = match previous {
            Some(a) => {
                let seg = ground_dist_m((a.p.lon, a.p.lat), (p.lon, p.lat)) as f64;
                Step { p, along: a.along + seg, seg }
            }
            None => Step { p, along: cum_m as f64, seg: 0.0 },
        };
        previous = Some(step);
        step
    })
}

/// Clip one chunk's walk of `n` points to the inclusive interval `[lo, hi]`: the points inside it,
/// the first and last interpolated onto the boundary. A chunk that misses the interval, or has no
/// segment, emits nothing. Returns whether it emitted.
pub(crate) fn clip(
    steps: impl Iterator<Item = Step>,
    n: usize,
    lo: u32,
    hi: u32,
    emit: &mut dyn FnMut(RoutePoint) -> Result<(), Error>,
) -> Result<bool, Error> {
    let (lo, hi) = (lo as f64, hi as f64);
    let mut previous: Option<Step> = None;
    let mut started = false;
    for (i, b) in steps.enumerate() {
        let Some(a) = previous.replace(b) else { continue };
        if b.along >= lo && a.along <= hi {
            if !started {
                let t = if b.seg > 1e-3 { ((lo - a.along) / b.seg).clamp(0.0, 1.0) } else { 0.0 };
                emit(interpolate_point(a.p, b.p, t as f32))?;
                started = true;
            }
            // Interior points stay as stored; only the stretch's last point is interpolated.
            if b.along > hi || i + 1 == n {
                let t = if b.seg > 1e-3 { ((hi - a.along) / b.seg).clamp(0.0, 1.0) } else { 1.0 };
                emit(interpolate_point(a.p, b.p, t as f32))?;
                return Ok(true);
            }
            emit(b.p)?;
        }
        if b.along > hi {
            break;
        }
    }
    Ok(started)
}

/// The point a fraction `t` of the way from `a` to `b`. Its elevation is unknown inside a segment
/// whose ends are not both known, or whose elevation is incomplete.
pub(crate) fn interpolate_point(a: RoutePoint, b: RoutePoint, t: f32) -> RoutePoint {
    RoutePoint {
        lon: a.lon + libm::roundf((b.lon - a.lon) as f32 * t) as i32,
        lat: a.lat + libm::roundf((b.lat - a.lat) as f32 * t) as i32,
        ele: if t <= 0.0 {
            a.ele
        } else if t >= 1.0 {
            b.ele
        } else if a.elevation().is_none() || b.elevation().is_none() || b.elevation_incomplete {
            i16::MIN
        } else {
            libm::roundf(a.ele as f32 + (i32::from(b.ele) - i32::from(a.ele)) as f32 * t) as i16
        },
        surface: b.surface,
        elevation_incomplete: b.elevation_incomplete,
    }
}

/// The column of `cols` that fraction `frac` of a span falls in.
pub(crate) fn column(frac: f64, cols: usize) -> usize {
    ((frac * (cols - 1) as f64) as usize).min(cols - 1)
}
