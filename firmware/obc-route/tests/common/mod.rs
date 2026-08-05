//! Shared helpers for the `obc-route` integration tests.
//!
//! The `VecSink` `ByteSink`, the GPX→OBCR `convert` helper, the single-chunk `decode`
//! helper and the stitched-polyline `route_points` reader were copy-pasted across
//! `convert.rs`, `format.rs`, `matcher.rs`, `profile.rs`, `track.rs`, `nav.rs` and
//! `detour.rs`; this module is the single source. Alongside them lives [`build_obcr`],
//! the hand-rolled OBCR writer the format-contract tests use as an **independent
//! oracle** — see its own note on why it never calls the production emitter. Not every
//! test uses every helper, so `#[allow(dead_code)]` keeps the unused-per-binary ones
//! quiet.

#![allow(dead_code)]

use obc_formats::io::{put_i16, put_i32, put_u16, put_u32, ByteSink, Error, SliceSource};
use obc_formats::obcr::{
    CHUNK_META_LEN, HEADER_FULL_LEN, NAME_CAP, POINT_RECORD_LEN, VERSION, WAYPOINT_LEN, WAYPOINT_NAME_OFF,
};
use obc_route::{RouteIndex, RoutePoint, RouteReader, MAX_POINTS_PER_CHUNK};

/// A `ByteSink` over a growable `Vec` — the host's "write the whole file to RAM"
/// backing (the device uses a FatFs-backed sink instead).
#[derive(Default)]
pub struct VecSink {
    pub buf: Vec<u8>,
}

impl ByteSink for VecSink {
    fn write(&mut self, b: &[u8]) -> Result<(), Error> {
        self.buf.extend_from_slice(b);
        Ok(())
    }
    fn patch_at(&mut self, off: u32, b: &[u8]) -> Result<(), Error> {
        let o = off as usize;
        self.buf[o..o + b.len()].copy_from_slice(b);
        Ok(())
    }
}

/// Convert an in-memory GPX string to `.obcr` bytes via the public converter.
pub fn convert(name: &str, gpx: &str) -> Vec<u8> {
    let src = SliceSource(gpx.as_bytes());
    let mut sink = VecSink::default();
    obc_route::gpx_to_obcr(&src, name, &mut sink).unwrap();
    sink.buf
}

/// Decode chunk `k` of `r` to an owned point vector.
pub fn decode(r: &RouteReader, k: usize) -> Vec<RoutePoint> {
    let mut out = heapless::Vec::<_, MAX_POINTS_PER_CHUNK>::new();
    r.decode_chunk(k, &mut out).unwrap();
    out.to_vec()
}

/// Decode an `.obcr`'s full point list, every chunk stitched in route order. Chunks repeat their
/// seam point, so each chunk after the first contributes all but its head.
pub fn route_points(obcr: &[u8]) -> Vec<RoutePoint> {
    let src = SliceSource(obcr);
    let idx = RouteIndex::read(&src).expect("the emitted OBCR parses");
    let r = RouteReader::new(&idx, &src);
    let mut pts = Vec::new();
    for k in 0..idx.chunks().len() {
        let chunk = decode(&r, k);
        let skip = usize::from(k > 0);
        pts.extend_from_slice(&chunk[skip..]);
    }
    pts
}

// ---------------------------------------------------------------------------------------------
// The hand-rolled OBCR writer — the format tests' independent oracle.
//
// This deliberately does **not** go through `gpx_to_obcr` / `ObcrEmitter`: the converter decides
// chunk boundaries, decimation and totals itself, and a fixture built with it could only ever
// prove the reader agrees with the writer. Emitting the bytes here against `OBCR_Spec.md` §1–§4
// instead pins *both* sides to the spec — if either drifts, these tests break — and lets a test
// place points in specific chunks with specific cumulative distances, or lie about a field on
// purpose. Only the fields the reader reads are populated.
// ---------------------------------------------------------------------------------------------

/// One chunk to encode: its absolute points (`(lon, lat, ele)`, microdegrees + metres) plus the
/// cumulative stats stamped at its first point (the values the reader re-anchors to).
pub struct ChunkIn {
    pub points: Vec<(i32, i32, i16)>,
    pub cum_distance_m: u32,
    pub cum_ascent_m: u32,
}

/// A waypoint record to hand-encode: `(dist_along_m, lon, lat, ele, category, name_len,
/// lateral_offset_m, name_bytes)` — `name_len` is passed explicitly so a test can lie with it.
pub type WpRec<'a> = (u32, i32, i32, i16, u8, u8, i16, &'a [u8]);

/// Where the chunk-meta index sits relative to the geometry. Both orderings are legal (§1 puts
/// the two offsets in the header precisely so they can move), and the tests use both: one pins
/// the index at a known absolute offset so it can corrupt a meta field by hand, the others want
/// the geometry to start immediately after the header.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum IndexPlacement {
    /// Metas immediately after the 128-byte header, geometry after them.
    BeforeData,
    /// Geometry immediately after the header, metas after it.
    AfterData,
}

/// The absolute file byte range a chunk's data region occupies — so a counting `ByteSource` test
/// can name "chunk `k`'s bytes" precisely and prove a skipped chunk was never read.
#[derive(Clone, Copy)]
pub struct ChunkExtent {
    pub start: u32,
    pub end: u32,
}

/// What [`build_obcr`] should write. Everything optional is derived from the geometry when left
/// `None`, so a test only spells out the fields it actually cares about (`..Default::default()`).
pub struct RouteSpec<'a> {
    /// Route name; also the header's name-length byte.
    pub name: &'a str,
    /// The chunks, in route order. At least one.
    pub chunks: &'a [ChunkIn],
    /// Header totals `(distance_m, ascent_m, descent_m)`.
    pub totals: (u32, u32, u32),
    /// Index/geometry ordering — see [`IndexPlacement`].
    pub index: IndexPlacement,
    /// Header start point; defaults to the first chunk's first point.
    pub start: Option<(i32, i32)>,
    /// Header `(min_ele, max_ele)`; defaults to the geometry's own range. Overridable because a
    /// route may legitimately declare a wider y-axis than its decimated points reach.
    pub ele_range: Option<(i16, i16)>,
    /// Do consecutive chunks repeat their seam point? When true the header's point count counts
    /// each seam once, the way the converter writes it.
    pub seam_shared: bool,
    /// The §1.1 waypoint extension: `None` writes the "no table" zeros, `Some(recs)` appends a
    /// §4 table at the end of the file and points the header at it (`Some(&[])` still writes the
    /// offset — an empty-but-present table).
    pub waypoints: Option<&'a [WpRec<'a>]>,
}

impl Default for RouteSpec<'_> {
    fn default() -> Self {
        RouteSpec {
            name: "test",
            chunks: &[],
            totals: (0, 0, 0),
            index: IndexPlacement::AfterData,
            start: None,
            ele_range: None,
            seam_shared: false,
            waypoints: None,
        }
    }
}

/// Serialize `spec` into an in-memory `.obcr`, returning the bytes and each chunk's data-region
/// byte extent. A chunk's body is `(point_count - 1)` fixed 6-byte delta records — the anchor
/// lives in the chunk-meta, not the body — exactly what `decode_chunk` expects.
pub fn build_obcr(spec: &RouteSpec) -> (Vec<u8>, Vec<ChunkExtent>) {
    let chunks = spec.chunks;
    assert!(!chunks.is_empty(), "a route needs at least one chunk");
    assert!(chunks.iter().all(|c| !c.points.is_empty()), "a chunk needs at least one point");

    let all = || chunks.iter().flat_map(|c| c.points.iter().copied());
    let (min_lon, min_lat) = (all().map(|p| p.0).min().unwrap(), all().map(|p| p.1).min().unwrap());
    let (max_lon, max_lat) = (all().map(|p| p.0).max().unwrap(), all().map(|p| p.1).max().unwrap());
    let ele_range =
        spec.ele_range.unwrap_or_else(|| (all().map(|p| p.2).min().unwrap(), all().map(|p| p.2).max().unwrap()));
    let start = spec.start.unwrap_or((chunks[0].points[0].0, chunks[0].points[0].1));

    let total_points: usize = chunks.iter().map(|c| c.points.len()).sum();
    let point_count = if spec.seam_shared { total_points - (chunks.len() - 1) } else { total_points };

    let metas_len = chunks.len() * CHUNK_META_LEN;
    let data_len: usize = chunks.iter().map(|c| (c.points.len() - 1) * POINT_RECORD_LEN).sum();
    let (index_offset, data_offset) = match spec.index {
        IndexPlacement::BeforeData => (HEADER_FULL_LEN, HEADER_FULL_LEN + metas_len),
        IndexPlacement::AfterData => (HEADER_FULL_LEN + data_len, HEADER_FULL_LEN),
    };

    let mut metas: Vec<u8> = Vec::with_capacity(metas_len);
    let mut data: Vec<u8> = Vec::with_capacity(data_len);
    let mut extents: Vec<ChunkExtent> = Vec::with_capacity(chunks.len());
    let mut cursor = data_offset as u32;
    for ch in chunks {
        let p = &ch.points;
        let anchor = p[0];
        let (cmin_lon, cmin_lat) = (p.iter().map(|q| q.0).min().unwrap(), p.iter().map(|q| q.1).min().unwrap());
        let (cmax_lon, cmax_lat) = (p.iter().map(|q| q.0).max().unwrap(), p.iter().map(|q| q.1).max().unwrap());

        let mut body: Vec<u8> = Vec::new();
        for w in p.windows(2) {
            let (a, b) = (w[0], w[1]);
            body.extend_from_slice(&((b.0 - a.0) as i16).to_le_bytes());
            body.extend_from_slice(&((b.1 - a.1) as i16).to_le_bytes());
            body.extend_from_slice(&b.2.to_le_bytes());
        }
        let body_len = body.len() as u32;

        let mut m = [0u8; CHUNK_META_LEN];
        put_i32(&mut m, 0, cmin_lon);
        put_i32(&mut m, 4, cmin_lat);
        put_i32(&mut m, 8, cmax_lon);
        put_i32(&mut m, 12, cmax_lat);
        put_i32(&mut m, 16, anchor.0);
        put_i32(&mut m, 20, anchor.1);
        put_i16(&mut m, 24, anchor.2);
        put_u16(&mut m, 26, p.len() as u16);
        put_u32(&mut m, 28, ch.cum_distance_m);
        put_u32(&mut m, 32, ch.cum_ascent_m);
        put_u32(&mut m, 36, cursor);
        put_u32(&mut m, 40, body_len);
        metas.extend_from_slice(&m);

        extents.push(ChunkExtent { start: cursor, end: cursor + body_len });
        cursor += body_len;
        data.extend_from_slice(&body);
    }
    assert_eq!(metas.len(), metas_len);
    assert_eq!(data.len(), data_len);

    // Header: the 112-byte core plus the §1.1 waypoint extension.
    let mut h = [0u8; HEADER_FULL_LEN];
    h[0..4].copy_from_slice(b"OBCR");
    h[4] = VERSION;
    h[5] = 0; // flags
    h[6] = spec.name.len() as u8;
    h[7] = 0; // reserved
    put_i32(&mut h, 8, min_lon);
    put_i32(&mut h, 12, min_lat);
    put_i32(&mut h, 16, max_lon);
    put_i32(&mut h, 20, max_lat);
    put_i32(&mut h, 24, start.0);
    put_i32(&mut h, 28, start.1);
    put_u32(&mut h, 32, point_count as u32);
    put_u32(&mut h, 36, spec.totals.0);
    put_u32(&mut h, 40, spec.totals.1);
    put_u32(&mut h, 44, spec.totals.2);
    put_i16(&mut h, 48, ele_range.0);
    put_i16(&mut h, 50, ele_range.1);
    put_u32(&mut h, 52, chunks.len() as u32);
    put_u32(&mut h, 56, index_offset as u32);
    put_u32(&mut h, 60, data_offset as u32);
    assert!(spec.name.len() <= NAME_CAP, "the name field is {NAME_CAP} bytes");
    h[64..64 + spec.name.len()].copy_from_slice(spec.name.as_bytes());
    if let Some(wps) = spec.waypoints {
        put_u32(&mut h, 112, (HEADER_FULL_LEN + metas_len + data_len) as u32);
        put_u16(&mut h, 116, wps.len() as u16);
    }

    let mut f: Vec<u8> = Vec::with_capacity(HEADER_FULL_LEN + metas_len + data_len);
    f.extend_from_slice(&h);
    match spec.index {
        IndexPlacement::BeforeData => {
            f.extend_from_slice(&metas);
            f.extend_from_slice(&data);
        }
        IndexPlacement::AfterData => {
            f.extend_from_slice(&data);
            f.extend_from_slice(&metas);
        }
    }

    // §4 waypoint records, 44 bytes each, at the tail the header now points at.
    for &(along, lon, lat, ele, category, name_len, offset, name) in spec.waypoints.unwrap_or(&[]) {
        let mut rec = [0u8; WAYPOINT_LEN];
        put_u32(&mut rec, 0, along);
        put_i32(&mut rec, 4, lon);
        put_i32(&mut rec, 8, lat);
        put_i16(&mut rec, 12, ele);
        rec[14] = category;
        rec[15] = name_len;
        put_i16(&mut rec, 16, offset);
        // rec[18..20] reserved, zero
        rec[WAYPOINT_NAME_OFF..WAYPOINT_NAME_OFF + name.len()].copy_from_slice(name);
        f.extend_from_slice(&rec);
    }

    (f, extents)
}
