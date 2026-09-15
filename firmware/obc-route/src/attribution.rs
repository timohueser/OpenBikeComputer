//! Conservative bounded GPX segment attribution. Ambiguity stays unknown.
use obc_formats::io::Error;
use obc_map_scene::{cos_lat, ground_dist_m, BBox};
use obc_reader::{NavEdgeCandidate, NavTileCache, Reader};

pub const ATTRIBUTION_DISTANCE_M: f32 = 8.0;
pub const ATTRIBUTION_MAX_SEGMENT_M: f32 = 100.0;
/// The snap-anchor spacing is at most 300 m. This query includes every candidate within 8 m.
const ATTRIBUTION_QUERY_M: f32 = 310.0;

/// Uses caller-owned graph cache. Work is at most three indexed candidate queries per segment.
/// Endpoints and midpoint must all identify the same unique edge. Arc direction and length must
/// agree with the GPX segment; crossing, parallel and off-network spans remain unknown.
pub fn attribute_segment(
    reader: &Reader,
    tiles: &mut NavTileCache,
    from: (i32, i32),
    to: (i32, i32),
) -> Result<u8, Error> {
    let distance = ground_dist_m(from, to);
    if !(1.0..=ATTRIBUTION_MAX_SEGMENT_M).contains(&distance) {
        return Ok(0);
    }
    let Some(a) = candidate(reader, tiles, from)? else { return Ok(0) };
    let Some(b) = candidate(reader, tiles, to)? else { return Ok(0) };
    if a.edge_id != b.edge_id {
        return Ok(0);
    }
    let mid = (from.0 + (to.0 - from.0) / 2, from.1 + (to.1 - from.1) / 2);
    let Some(m) = candidate(reader, tiles, mid)? else { return Ok(0) };
    let lo = a.from_a_m.min(b.from_a_m);
    let hi = a.from_a_m.max(b.from_a_m);
    if m.edge_id != a.edge_id
        || !(lo..=hi).contains(&m.from_a_m)
        || ((hi - lo) as f32 - distance).abs() > (distance * 0.1).max(2.0)
    {
        return Ok(0);
    }
    if [from, mid, to].iter().any(|&p| ambiguous_projection(reader, a.edge_id, p)) {
        return Ok(0);
    }
    Ok(a.way_kind >> 5)
}

fn candidate(reader: &Reader, tiles: &mut NavTileCache, p: (i32, i32)) -> Result<Option<NavEdgeCandidate>, Error> {
    let dy = (ATTRIBUTION_QUERY_M / 0.11132) as i32 + 1;
    let dx = (ATTRIBUTION_QUERY_M / (0.11132 * cos_lat(p.1).abs().max(0.01))) as i32 + 1;
    let view = BBox {
        min_lon: p.0.saturating_sub(dx),
        max_lon: p.0.saturating_add(dx),
        min_lat: p.1.saturating_sub(dy),
        max_lat: p.1.saturating_add(dy),
    };
    reader.unique_nav_edge_candidate_cached(&view, tiles, p, ATTRIBUTION_DISTANCE_M).map_err(|_| Error::Io)
}

/// A repeated crossing on one edge is as ambiguous as two parallel edges.
fn ambiguous_projection(reader: &Reader, edge_id: u32, p: (i32, i32)) -> bool {
    let mut points = heapless::Vec::<(i32, i32), 128>::new();
    if reader.nav_edge(edge_id, &mut points).is_none() {
        return true;
    }
    let mut first: Option<f32> = None;
    let mut along = 0.0;
    for pair in points.windows(2) {
        let length = ground_dist_m(pair[0], pair[1]);
        let (t, distance) = crate::geo::project_to_segment(pair[0], pair[1], p, cos_lat(p.1));
        if distance <= ATTRIBUTION_DISTANCE_M {
            let at = along + t * length;
            if first.is_some_and(|old| (at - old).abs() > 2.0 * ATTRIBUTION_DISTANCE_M) {
                return true;
            }
            first.get_or_insert(at);
        }
        along += length;
    }
    false
}
