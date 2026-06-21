//! Format-contract tests for the OBCR reader.
//!
//! Each test builds a synthetic `.obcr` byte buffer with a small handwritten builder
//! that mirrors `OBCR_Spec.md` exactly, then asserts the reader parses it back.
//! Building the bytes here (rather than via the converter) pins the reader to the
//! spec independently: if either drifts, these break.

use obc_route::{
    Error, RouteIndex, RoutePoint, RouteReader, RouteSummary, SliceSource, CHUNK_META_LEN,
    HEADER_LEN, MAX_POINTS_PER_CHUNK,
};

/// A chunk to encode: its absolute points (lon, lat, ele) plus the cumulative stats
/// at its first point.
struct ChunkIn {
    points: Vec<(i32, i32, i16)>,
    cum_distance_m: u32,
    cum_ascent_m: u32,
}

/// Build a `.obcr` from chunks, mirroring the spec's byte layout. `start` is the
/// first route point; `totals` is (distance_m, ascent_m, descent_m).
fn build_route(
    name: &str,
    start: (i32, i32),
    totals: (u32, u32, u32),
    ele_range: (i16, i16),
    chunks: &[ChunkIn],
) -> Vec<u8> {
    let all = || chunks.iter().flat_map(|c| c.points.iter().copied());
    let min_lon = all().map(|p| p.0).min().unwrap();
    let min_lat = all().map(|p| p.1).min().unwrap();
    let max_lon = all().map(|p| p.0).max().unwrap();
    let max_lat = all().map(|p| p.1).max().unwrap();
    // Distinct points: seams (each chunk's first == previous chunk's last) count once.
    let distinct: usize =
        chunks.iter().map(|c| c.points.len()).sum::<usize>() - chunks.len().saturating_sub(1);

    let index_offset = HEADER_LEN;
    let data_offset = index_offset + chunks.len() * CHUNK_META_LEN;

    let mut metas: Vec<u8> = Vec::new();
    let mut data: Vec<u8> = Vec::new();
    let mut cursor = data_offset;
    for ch in chunks {
        let p = &ch.points;
        let anchor = p[0];
        let (cmin_lon, cmin_lat) =
            (p.iter().map(|q| q.0).min().unwrap(), p.iter().map(|q| q.1).min().unwrap());
        let (cmax_lon, cmax_lat) =
            (p.iter().map(|q| q.0).max().unwrap(), p.iter().map(|q| q.1).max().unwrap());

        let mut body: Vec<u8> = Vec::new();
        for w in p.windows(2) {
            let (a, b) = (w[0], w[1]);
            body.extend_from_slice(&((b.0 - a.0) as i16).to_le_bytes());
            body.extend_from_slice(&((b.1 - a.1) as i16).to_le_bytes());
            body.extend_from_slice(&b.2.to_le_bytes());
        }

        // ChunkMeta (44 bytes).
        metas.extend_from_slice(&cmin_lon.to_le_bytes());
        metas.extend_from_slice(&cmin_lat.to_le_bytes());
        metas.extend_from_slice(&cmax_lon.to_le_bytes());
        metas.extend_from_slice(&cmax_lat.to_le_bytes());
        metas.extend_from_slice(&anchor.0.to_le_bytes());
        metas.extend_from_slice(&anchor.1.to_le_bytes());
        metas.extend_from_slice(&anchor.2.to_le_bytes());
        metas.extend_from_slice(&(p.len() as u16).to_le_bytes());
        metas.extend_from_slice(&ch.cum_distance_m.to_le_bytes());
        metas.extend_from_slice(&ch.cum_ascent_m.to_le_bytes());
        metas.extend_from_slice(&(cursor as u32).to_le_bytes());
        metas.extend_from_slice(&(body.len() as u32).to_le_bytes());

        cursor += body.len();
        data.extend_from_slice(&body);
    }
    assert_eq!(metas.len(), chunks.len() * CHUNK_META_LEN);

    // Header (112 bytes).
    let mut f: Vec<u8> = Vec::new();
    f.extend_from_slice(b"OBCR");
    f.push(1); // version
    f.push(0); // flags
    f.push(name.len() as u8);
    f.push(0); // reserved
    f.extend_from_slice(&min_lon.to_le_bytes());
    f.extend_from_slice(&min_lat.to_le_bytes());
    f.extend_from_slice(&max_lon.to_le_bytes());
    f.extend_from_slice(&max_lat.to_le_bytes());
    f.extend_from_slice(&start.0.to_le_bytes());
    f.extend_from_slice(&start.1.to_le_bytes());
    f.extend_from_slice(&(distinct as u32).to_le_bytes());
    f.extend_from_slice(&totals.0.to_le_bytes());
    f.extend_from_slice(&totals.1.to_le_bytes());
    f.extend_from_slice(&totals.2.to_le_bytes());
    f.extend_from_slice(&ele_range.0.to_le_bytes());
    f.extend_from_slice(&ele_range.1.to_le_bytes());
    f.extend_from_slice(&(chunks.len() as u32).to_le_bytes());
    f.extend_from_slice(&(index_offset as u32).to_le_bytes());
    f.extend_from_slice(&(data_offset as u32).to_le_bytes());
    let mut name_field = [0u8; 48];
    name_field[..name.len()].copy_from_slice(name.as_bytes());
    f.extend_from_slice(&name_field);
    assert_eq!(f.len(), HEADER_LEN, "header must be 112 bytes");

    f.extend_from_slice(&metas);
    f.extend_from_slice(&data);
    f
}

/// Two seam-sharing chunks: chunk 0 ends at (40,40,210), chunk 1 begins there.
fn two_chunk_route() -> Vec<u8> {
    build_route(
        "Black Forest",
        (10, 10),
        (12_340, 678, 540),
        (200, 240),
        &[
            ChunkIn {
                points: vec![(10, 10, 200), (20, 25, 205), (40, 40, 210)],
                cum_distance_m: 0,
                cum_ascent_m: 0,
            },
            ChunkIn {
                points: vec![(40, 40, 210), (60, 30, 230), (90, 70, 225)],
                cum_distance_m: 6000,
                cum_ascent_m: 300,
            },
        ],
    )
}

fn decode(r: &RouteReader, k: usize) -> Vec<RoutePoint> {
    let mut out = heapless::Vec::<_, MAX_POINTS_PER_CHUNK>::new();
    r.decode_chunk(k, &mut out).unwrap();
    out.to_vec()
}

#[test]
fn header_and_summary() {
    let bytes = two_chunk_route();
    let src = SliceSource(&bytes);
    let ridx = RouteIndex::read(&src).unwrap();
    let r = RouteReader::new(&ridx, &src);

    assert_eq!(r.name(), "Black Forest");
    assert_eq!(r.bbox.min_lon, 10);
    assert_eq!(r.bbox.max_lon, 90);
    assert_eq!(r.bbox.max_lat, 70);
    assert_eq!(r.start_lon, 10);
    assert_eq!(r.start_lat, 10);
    assert_eq!(r.total_distance_m, 12_340);
    assert_eq!(r.total_ascent_m, 678);
    assert_eq!(r.total_descent_m, 540);
    assert_eq!(r.min_ele_m, 200);
    assert_eq!(r.max_ele_m, 240);
    assert_eq!(r.point_count, 5); // 6 points, one shared seam

    let s = r.summary();
    assert_eq!(s.name, "Black Forest");
    assert_eq!(s.distance_km, 12); // 12_340 m rounds to 12 km
    assert_eq!(s.climb_m, 678);
    assert_eq!(s.start_lon, 10);

    // RouteSummary::read parses the same fields from the header alone.
    let s2 = RouteSummary::read(&src).unwrap();
    assert_eq!(s2.name, "Black Forest");
    assert_eq!(s2.distance_km, 12);
    assert_eq!(s2.bbox.max_lon, 90);
}

#[test]
fn chunk_index_and_decode() {
    let bytes = two_chunk_route();
    let src = SliceSource(&bytes);
    let ridx = RouteIndex::read(&src).unwrap();
    let r = RouteReader::new(&ridx, &src);

    assert_eq!(r.chunks().len(), 2);
    assert_eq!(r.chunks()[1].cum_distance_m, 6000);
    assert_eq!(r.chunks()[1].cum_ascent_m, 300);

    let c0 = decode(&r, 0);
    assert_eq!(
        c0,
        vec![
            RoutePoint { lon: 10, lat: 10, ele: 200 },
            RoutePoint { lon: 20, lat: 25, ele: 205 },
            RoutePoint { lon: 40, lat: 40, ele: 210 },
        ]
    );

    let c1 = decode(&r, 1);
    // Seam: chunk 1's first point == chunk 0's last point.
    assert_eq!(c1[0], *c0.last().unwrap());
    assert_eq!(c1.last().unwrap(), &RoutePoint { lon: 90, lat: 70, ele: 225 });
}

#[test]
fn visible_chunk_query() {
    let bytes = two_chunk_route();
    let src = SliceSource(&bytes);
    let ridx = RouteIndex::read(&src).unwrap();
    let r = RouteReader::new(&ridx, &src);

    // A view around (10,10) overlaps only chunk 0 (bbox 10..40).
    let view = obc_route::BBox { min_lon: 0, min_lat: 0, max_lon: 30, max_lat: 30 };
    let mut hit = Vec::new();
    r.for_each_visible_chunk(&view, |k, _| hit.push(k));
    assert_eq!(hit, vec![0]);

    // A view around (80,60) overlaps only chunk 1 (bbox 40..90).
    let view = obc_route::BBox { min_lon: 70, min_lat: 50, max_lon: 100, max_lat: 80 };
    let mut hit = Vec::new();
    r.for_each_visible_chunk(&view, |k, _| hit.push(k));
    assert_eq!(hit, vec![1]);
}

#[test]
fn rejects_bad_input() {
    let err = |b: &[u8]| {
        let src = SliceSource(b);
        match RouteIndex::read(&src) {
            Ok(_) => panic!("expected Err"),
            Err(e) => e,
        }
    };

    assert_eq!(err(&[0u8; 8]), Error::BadOffset); // shorter than the header

    let mut bytes = two_chunk_route();
    bytes[0] = b'X';
    assert_eq!(err(&bytes), Error::BadMagic);

    let mut bytes = two_chunk_route();
    bytes[4] = 2; // unsupported version
    assert_eq!(err(&bytes), Error::BadVersion);
}
