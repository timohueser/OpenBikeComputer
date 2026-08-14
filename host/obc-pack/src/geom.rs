//! Geometry for the packer: an owned geometry type, its bounds, and the GEOS
//! bridge used for boundary clipping.
//!
//! Coordinates are f64 lon/lat degrees; the quadtree clips in degree space, then
//! the serializer rounds to microdegrees.

use std::collections::HashMap;

use geos::{CoordSeq, Geom as _, Geometry, GeometryTypes};
use obc_map_scene::M_PER_DEG;
use rayon::prelude::*;

use crate::serialize::{Feature, Kind};

/// Axis-aligned bounds in degrees: (min_lon, min_lat, max_lon, max_lat).
pub type Bounds = (f64, f64, f64, f64);

/// `Multi` only arises from a clip result and is flattened away before storage.
#[derive(Debug, Clone)]
pub enum Geom {
    Line(Vec<(f64, f64)>),
    Polygon { exterior: Vec<(f64, f64)>, interiors: Vec<Vec<(f64, f64)>> },
    Multi(Vec<Geom>),
    Empty,
}

impl Geom {
    /// Panics on `Empty` — callers guard with `is_empty` first.
    pub fn bounds(&self) -> Bounds {
        let mut b = (f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
        fn fold(g: &Geom, b: &mut Bounds) {
            match g {
                Geom::Line(c) => {
                    for &(x, y) in c {
                        widen(b, x, y);
                    }
                }
                Geom::Polygon { exterior, interiors } => {
                    for &(x, y) in exterior {
                        widen(b, x, y);
                    }
                    for ring in interiors {
                        for &(x, y) in ring {
                            widen(b, x, y);
                        }
                    }
                }
                Geom::Multi(parts) => {
                    for p in parts {
                        fold(p, b);
                    }
                }
                Geom::Empty => {}
            }
        }
        fold(self, &mut b);
        b
    }

    pub fn is_empty(&self) -> bool {
        match self {
            Geom::Empty => true,
            Geom::Line(c) => c.is_empty(),
            Geom::Polygon { exterior, .. } => exterior.is_empty(),
            Geom::Multi(parts) => parts.iter().all(Geom::is_empty),
        }
    }

    /// Whether every non-empty part is linework. Clip results may wrap lines in `Multi`, so callers
    /// selecting a line-only operation must look through that container rather than match `Line`
    /// only.
    pub fn is_lineal(&self) -> bool {
        match self {
            Geom::Line(_) => true,
            Geom::Multi(parts) => !parts.is_empty() && parts.iter().all(Geom::is_lineal),
            Geom::Polygon { .. } | Geom::Empty => false,
        }
    }
}

#[inline]
fn widen(b: &mut Bounds, x: f64, y: f64) {
    if x < b.0 {
        b.0 = x;
    }
    if y < b.1 {
        b.1 = y;
    }
    if x > b.2 {
        b.2 = x;
    }
    if y > b.3 {
        b.3 = y;
    }
}

/// Pixels-per-degree at latitude `lat` (degrees) and scale `mpp` (meters/pixel):
/// longitude carries the `cos(lat)` foreshortening, latitude does not. The
/// `cos` is floored at 0.01 so a near-polar feature can't blow the scale up.
#[inline]
fn px_per_deg(lat: f64, mpp: f64) -> (f64, f64) {
    let cos_lat = lat.to_radians().cos().abs().max(0.01);
    (M_PER_DEG * cos_lat / mpp, M_PER_DEG / mpp)
}

/// Absolute area of a closed ring in degrees², via the shoelace formula. The
/// ring may or may not repeat its first vertex (the index wrap closes it);
/// fewer than three vertices enclose no area.
fn ring_area_deg2(ring: &[(f64, f64)]) -> f64 {
    if ring.len() < 3 {
        return 0.0;
    }
    let mut a = 0.0;
    for i in 0..ring.len() {
        let (x1, y1) = ring[i];
        let (x2, y2) = ring[(i + 1) % ring.len()];
        a += x1 * y2 - x2 * y1;
    }
    (a * 0.5).abs()
}

/// Minimum-area cull for a coarse LOD: `true` when a **polygon** feature's
/// projected area (exterior minus holes) is below `min_area_px` square pixels at
/// `mpp` meters-per-pixel, so it should be dropped from this tier.
///
/// **Lines are never culled here.** OSM ways are fragmented — one road is stored
/// as many short segments — so an extent test on each segment drops a road's
/// shortest links and leaves it patched with holes. There is no per-segment size
/// that means anything *before* the ways are stitched back together. Once
/// [`crate::merge::merge_lines_with`] has stitched a class into connected
/// polylines a record's length does mean something, and a tier may then drop the
/// short leftovers by their own measure — see [`line_below`], which is the only
/// place a line is ever culled.
/// A `Multi` culls only if *every* non-empty part is a cullable polygon.
///
/// Degrees convert to meters with [`M_PER_DEG`] and the `cos(lat)` longitude
/// foreshortening at the feature's mid-latitude. `min_area_px <= 0`, a
/// non-positive `mpp`, and empty/line geometry never cull.
/// A geometry's projected area in **square pixels** at `mpp` — the quantity
/// [`footprint_below`] compares against a threshold, exposed for the callers that need the
/// number rather than the verdict (the coverage pass ranks faces by it). Lines and empties
/// have no area; a `Multi` sums its parts. Same mid-latitude foreshortening basis, so a face
/// and a whole feature are measured on the same scale.
pub fn footprint_area_px(g: &Geom, mpp: f64) -> f64 {
    if mpp <= 0.0 || g.is_empty() {
        return 0.0;
    }
    match g {
        Geom::Empty | Geom::Line(_) => 0.0,
        Geom::Multi(parts) => parts.iter().map(|p| footprint_area_px(p, mpp)).sum(),
        Geom::Polygon { exterior, interiors } => {
            let (_, miny, _, maxy) = g.bounds();
            let (lon_ppd, lat_ppd) = px_per_deg(0.5 * (miny + maxy), mpp);
            let mut area = ring_area_deg2(exterior);
            for hole in interiors {
                area -= ring_area_deg2(hole);
            }
            area.max(0.0) * lon_ppd * lat_ppd
        }
    }
}

pub fn footprint_below(g: &Geom, mpp: f64, min_area_px: f64) -> bool {
    if min_area_px <= 0.0 || mpp <= 0.0 || g.is_empty() {
        return false;
    }
    match g {
        // A road is many short ways; culling by segment extent punches holes in it.
        Geom::Empty | Geom::Line(_) => false,
        Geom::Multi(parts) => {
            let mut any = false;
            for p in parts.iter().filter(|p| !p.is_empty()) {
                any = true;
                if !footprint_below(p, mpp, min_area_px) {
                    return false;
                }
            }
            any
        }
        Geom::Polygon { exterior, interiors } => {
            let (_, miny, _, maxy) = g.bounds();
            let (lon_ppd, lat_ppd) = px_per_deg(0.5 * (miny + maxy), mpp);
            let mut area = ring_area_deg2(exterior);
            for hole in interiors {
                area -= ring_area_deg2(hole);
            }
            area.max(0.0) * lon_ppd * lat_ppd < min_area_px
        }
    }
}

/// A line geometry's length in **kilometres**. Polygons and empties have no length (`0.0`); a
/// `Multi` sums its parts, so one feature's several strands measure as the one road they are.
///
/// Degrees convert with [`M_PER_DEG`] and the `cos(lat)` longitude foreshortening at each
/// *segment's* mid-latitude — the same basis as [`footprint_area_px`], taken per segment rather
/// than per feature because a line may run far enough north-south for one factor to be wrong.
pub fn line_length_km(g: &Geom) -> f64 {
    match g {
        Geom::Empty | Geom::Polygon { .. } => 0.0,
        Geom::Multi(parts) => parts.iter().map(line_length_km).sum(),
        Geom::Line(c) => {
            let deg: f64 = c
                .windows(2)
                .map(|w| {
                    let ((x1, y1), (x2, y2)) = (w[0], w[1]);
                    let cos_lat = (0.5 * (y1 + y2)).to_radians().cos().abs().max(0.01);
                    ((x2 - x1) * cos_lat).hypot(y2 - y1)
                })
                .sum();
            deg * M_PER_DEG / 1000.0
        }
    }
}

/// Post-stitch length cull for a coarse LOD: `true` when a **line** feature is shorter than
/// `min_km`, so this tier should drop it.
///
/// The counterpart to [`footprint_below`], and subject to the same caveat from the other side:
/// this is only meaningful **after** [`crate::merge::merge_lines_with`] has stitched a class's
/// fragments into connected polylines. Run on raw OSM ways it would do exactly the damage that
/// function's docs warn about. Run on stitched records it drops the short *leftovers* — the
/// junction stubs and roundabout arms that no through-line could absorb — and keeps the
/// long-distance skeleton, which is the whole reason a road class is on a far-zoom tier.
///
/// Never culls a polygon, so a `Multi` carrying any polygon part is safe by construction:
/// `min_km <= 0` (the default ⇒ off), empties and anything with a polygon in it all return
/// `false`.
pub fn line_below(g: &Geom, min_km: f64) -> bool {
    if min_km <= 0.0 || g.is_empty() {
        return false;
    }
    fn line_only(g: &Geom) -> bool {
        match g {
            Geom::Line(_) => true,
            Geom::Multi(parts) => parts.iter().all(|p| p.is_empty() || line_only(p)),
            Geom::Empty | Geom::Polygon { .. } => false,
        }
    }
    line_only(g) && line_length_km(g) < min_km
}

/// Drop interior rings (holes) whose projected area is below `min_area_px` square pixels at `mpp`,
/// returning the count removed. A hole smaller than a pixel is invisible — the fill paints straight
/// over it — so dropping it is a pixel-exact no-op that frees **a ring plus its vertices** in the
/// render's frame scratch (and shaves bytes on disk). The exterior is never touched, and a polygon
/// is never emptied (a kept polygon keeps its outline); lines and empties pass through. Same
/// disabled/degenerate guards and mid-latitude foreshortening basis as [`footprint_below`], so a
/// hole and a standalone polygon of equal area cull at the same scale. `Multi` recurses per part.
///
/// Paired with [`footprint_below`]: the cull drops whole sub-pixel polygons, this trims sub-pixel
/// holes out of the ones that survive (e.g. a big farmland face pocked with tiny unmapped islands).
pub fn strip_small_holes(g: &mut Geom, mpp: f64, min_area_px: f64) -> usize {
    if min_area_px <= 0.0 || mpp <= 0.0 {
        return 0;
    }
    match g {
        Geom::Empty | Geom::Line(_) => 0,
        Geom::Multi(parts) => parts.iter_mut().map(|p| strip_small_holes(p, mpp, min_area_px)).sum(),
        Geom::Polygon { exterior, interiors } => {
            if interiors.is_empty() {
                return 0;
            }
            // Foreshorten at the exterior's mid-latitude — holes live inside it, so its bounds set
            // the scale (matching `footprint_below`'s whole-geometry `bounds()` mid-lat).
            let (mut miny, mut maxy) = (f64::INFINITY, f64::NEG_INFINITY);
            for &(_, y) in exterior.iter() {
                miny = miny.min(y);
                maxy = maxy.max(y);
            }
            let (lon_ppd, lat_ppd) = px_per_deg(0.5 * (miny + maxy), mpp);
            let before = interiors.len();
            interiors.retain(|hole| ring_area_deg2(hole) * lon_ppd * lat_ppd >= min_area_px);
            before - interiors.len()
        }
    }
}

/// Drop a polygon's smallest holes until it fits `max_rings` rings (exterior +
/// holes) total, returning the count removed. The reader decodes a feature's rings
/// into a fixed `heapless` buffer and discards the whole feature past its capacity
/// ([`obc_reader::MAX_FEAT_RINGS`]) — so shipping the largest `max_rings - 1` holes
/// keeps the feature (and its most visible clearings) instead of losing all of it.
/// Kept holes stay in their original order, so emission is deterministic. Lines and
/// within-cap polygons pass through untouched; `Multi` recurses per part.
pub fn trim_excess_holes(g: &mut Geom, max_rings: usize) -> usize {
    let cap = max_rings.saturating_sub(1);
    match g {
        Geom::Empty | Geom::Line(_) => 0,
        Geom::Multi(parts) => parts.iter_mut().map(|p| trim_excess_holes(p, max_rings)).sum(),
        Geom::Polygon { interiors, .. } => {
            if interiors.len() <= cap {
                return 0;
            }
            // Rank holes by area, largest first (stable, so equal areas keep input
            // order), and keep the top `cap` at their original positions.
            let mut order: Vec<usize> = (0..interiors.len()).collect();
            order.sort_by(|&a, &b| ring_area_deg2(&interiors[b]).total_cmp(&ring_area_deg2(&interiors[a])));
            let mut keep = vec![false; interiors.len()];
            for &i in order.iter().take(cap) {
                keep[i] = true;
            }
            let dropped = interiors.len() - cap;
            let mut i = 0;
            interiors.retain(|_| {
                let k = keep[i];
                i += 1;
                k
            });
            dropped
        }
    }
}

/// Only `Line`/`Polygon` reach here (post-flatten).
pub fn to_feature(style_id: u8, g: &Geom) -> Option<Feature> {
    match g {
        Geom::Line(c) => Some(Feature { style_id, kind: Kind::Line, rings: vec![c.clone()] }),
        Geom::Polygon { exterior, interiors } => {
            let mut rings = Vec::with_capacity(1 + interiors.len());
            rings.push(exterior.clone());
            rings.extend(interiors.iter().cloned());
            Some(Feature { style_id, kind: Kind::Polygon, rings })
        }
        _ => None,
    }
}

/// Vertices `densify` will insert between two µdeg points (`steps - 1`).
#[inline]
fn densify_extra(p1: (i64, i64), p2: (i64, i64)) -> usize {
    let max_dist = (p2.0 - p1.0).abs().max((p2.1 - p1.1).abs());
    if max_dist > crate::serialize::MAX_SEGMENT {
        (max_dist / crate::serialize::MAX_SEGMENT) as usize
    } else {
        0
    }
}

/// Whether some cyclic exterior start lets every polygon hole choose a cyclic start whose first
/// delta fits the serializer. OBCM holes have no independent anchor, and adding intermediate points
/// to this jump changes the ring topology. A false result therefore means the quadtree must clip
/// the polygon into smaller pieces.
pub(crate) fn hole_anchors_encodable(g: &Geom) -> bool {
    let Geom::Polygon { exterior, interiors } = g else { return true };
    if interiors.is_empty() {
        return true;
    }
    let exterior = crate::serialize::canonical_ring_udeg(exterior, true);
    let interiors: Vec<_> = interiors.iter().map(|ring| crate::serialize::canonical_ring_udeg(ring, true)).collect();
    crate::serialize::best_exterior_anchor(&exterior, &interiors)
        .is_some_and(|(_, distance)| distance <= crate::serialize::MAX_HOLE_ANCHOR_DELTA)
}

/// Upper bound on the bytes `pack_feature` will emit for this geometry, for
/// quadtree chunk-size accounting: `12 + pts*4` plus the hole bookkeeping bytes,
/// where `pts` counts the **densified ring-edge** vertices using the same µdeg rounding and
/// `MAX_SEGMENT` walk as the serializer. Budgeting raw vertices instead would under-count features with
/// long segments (clipped land rectangles, coarse-LOD lines), and a leaf whose
/// real bytes overflow the chunk gets features silently dropped at pack time.
/// Always ≥ the packed size: deltas are budgeted at the 16-bit worst case, and
/// the exterior's anchor vertex is counted although it packs into the header.
///
/// The leading `12` is now a deliberate **overestimate**: v11 writes a 7-byte compact header for
/// the common feature (§5), and the budget must stay ≥ the feature's real bytes *plus its share of
/// the chunk's one sentinel byte*. The 4-byte anchor-vertex overcount already covers that +1 per
/// feature on its own, so the wide-header figure is kept as headroom rather than tightened. Under
/// v10's padded chunks an overestimate wasted file bytes; with tight chunks it costs nothing but a
/// marginally earlier leaf split.
pub fn packed_size_budget(g: &Geom) -> usize {
    const NO_HOLES: &[Vec<(f64, f64)>] = &[];
    let (exterior, interiors) = match g {
        Geom::Line(c) => (c.as_slice(), NO_HOLES),
        Geom::Polygon { exterior, interiors } => (exterior.as_slice(), interiors.as_slice()),
        _ => return 0,
    };
    let udeg = |ring: &[(f64, f64)]| -> Vec<(i64, i64)> {
        ring.iter().map(|&(x, y)| (crate::serialize::to_udeg(x), crate::serialize::to_udeg(y))).collect()
    };
    let ext = udeg(exterior);
    if ext.is_empty() {
        return 0;
    }
    // Count every cyclic edge for polygons. `pack_feature` rotates hole rings and leaves the final
    // closing edge implicit, so counting the full cycle is a conservative upper bound independent
    // of which vertex becomes first. A repeated GEOS closure merely contributes one zero edge.
    let ring_pts = |pts: &[(i64, i64)], closed: bool| -> usize {
        if pts.is_empty() {
            return 0;
        }
        let mut n = pts.len();
        for pair in pts.windows(2) {
            n += densify_extra(pair[0], pair[1]);
        }
        if closed {
            n += densify_extra(*pts.last().unwrap(), pts[0]);
        }
        n
    };
    let closed = matches!(g, Geom::Polygon { .. });
    let mut pts = ring_pts(&ext, closed);
    for hole in interiors {
        pts += ring_pts(&udeg(hole), true);
    }
    // Hole bookkeeping: 1 count byte + a 2-byte pt_count per hole.
    let hole_overhead = if interiors.is_empty() { 0 } else { 1 + 2 * interiors.len() };
    12 + pts * 4 + hole_overhead
}

// --- GEOS bridge -----------------------------------------------------------

pub(crate) fn ring_to_coordseq(coords: &[(f64, f64)]) -> CoordSeq {
    let buf: Vec<[f64; 2]> = coords.iter().map(|&(x, y)| [x, y]).collect();
    CoordSeq::new_from_vec(&buf).expect("coordseq")
}

fn to_geos(g: &Geom) -> Geometry {
    match g {
        Geom::Line(c) => Geometry::create_line_string(ring_to_coordseq(c)).expect("linestring"),
        Geom::Polygon { exterior, interiors } => {
            let ext = Geometry::create_linear_ring(ring_to_coordseq(exterior)).expect("ext ring");
            let holes = interiors
                .iter()
                .map(|r| Geometry::create_linear_ring(ring_to_coordseq(r)).expect("hole ring"))
                .collect();
            Geometry::create_polygon(ext, holes).expect("polygon")
        }
        // Only simple geoms are ever clipped (clip happens before flatten).
        _ => unreachable!("to_geos on non-simple geom"),
    }
}

/// Read a LineString/LinearRing's coordinate sequence into owned `(x, y)` pairs.
/// Works on the borrowed `ConstGeometry` that ring accessors return.
fn read_coords<G: geos::Geom>(g: &G) -> Vec<(f64, f64)> {
    let cs = g.get_coord_seq().expect("coord seq");
    let n = cs.size().expect("size");
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        out.push((cs.get_x(i).expect("x"), cs.get_y(i).expect("y")));
    }
    out
}

pub(crate) fn from_geos<G: Geom_>(g: &G) -> Geom {
    if g.is_empty().unwrap_or(true) {
        return Geom::Empty;
    }
    match g.geometry_type() {
        Ok(GeometryTypes::LineString) | Ok(GeometryTypes::LinearRing) => Geom::Line(read_coords(g)),
        Ok(GeometryTypes::Polygon) => {
            let ext = read_coords(&g.get_exterior_ring().expect("ext"));
            let nholes = g.get_num_interior_rings().expect("nholes");
            let interiors = (0..nholes).map(|i| read_coords(&g.get_interior_ring_n(i).expect("hole"))).collect();
            Geom::Polygon { exterior: ext, interiors }
        }
        Ok(GeometryTypes::MultiLineString)
        | Ok(GeometryTypes::MultiPolygon)
        | Ok(GeometryTypes::GeometryCollection) => {
            let n = g.get_num_geometries().expect("n geoms");
            let parts = (0..n).map(|i| from_geos(&g.get_geometry_n(i).expect("geom n"))).collect();
            Geom::Multi(parts)
        }
        // Points (incl. inside a GeometryCollection) carry no renderable line/area
        // — dropped.
        _ => Geom::Empty,
    }
}

/// Lets `from_geos` accept both owned `Geometry` and the borrowed `ConstGeometry`
/// that ring/sub-geometry accessors return.
pub(crate) trait Geom_: geos::Geom {}
impl Geom_ for Geometry {}
impl Geom_ for geos::ConstGeometry<'_> {}

/// Public entry to the generic `from_geos`, for reading an owned GEOS result back
/// into a [`Geom`].
pub(crate) fn geom_from_geos(g: &Geometry) -> Geom {
    from_geos(g)
}

/// GEOS `TopologyPreservingSimplifier`, not plain Douglas–Peucker, so a simplified
/// ring can't self-intersect. `tol` is in degrees (`simplify_m / M_PER_DEG`).
/// Empty/failed → [`Geom::Empty`] (the quadtree drops it).
pub fn topology_preserve_simplify(geom: &Geom, tol: f64) -> Geom {
    match to_geos(geom).topology_preserve_simplify(tol) {
        Ok(s) => from_geos(&s),
        Err(_) => Geom::Empty,
    }
}

/// Whether a ring assembles into a **valid** polygon (GEOS `is_valid`). Matches
/// osmium's assembler: a self-intersecting ring, a degenerate ring, or any
/// construction error is rejected → skip.
pub fn polygon_is_valid(exterior: &[(f64, f64)], interiors: &[Vec<(f64, f64)>]) -> bool {
    // A linear ring needs ≥4 positions (≥3 distinct + closing); fewer make GEOS error.
    if exterior.len() < 4 {
        return false;
    }
    let Ok(ext) = Geometry::create_linear_ring(ring_to_coordseq(exterior)) else {
        return false;
    };
    let mut holes = Vec::with_capacity(interiors.len());
    for r in interiors {
        let Ok(ring) = Geometry::create_linear_ring(ring_to_coordseq(r)) else {
            return false;
        };
        holes.push(ring);
    }
    match Geometry::create_polygon(ext, holes) {
        Ok(p) => p.is_valid().unwrap_or(false),
        Err(_) => false,
    }
}

/// Collect every [`Geom::Polygon`] out of a (possibly `Multi`/nested) geometry,
/// dropping anything non-polygonal.
pub(crate) fn collect_polygons(g: Geom, out: &mut Vec<Geom>) {
    match g {
        p @ Geom::Polygon { .. } => out.push(p),
        Geom::Multi(parts) => {
            for p in parts {
                collect_polygons(p, out);
            }
        }
        _ => {}
    }
}

/// Assemble a multipolygon/boundary relation's member ways into polygons-with-holes.
///
/// `members` is each member way's resolved coordinate list. GEOS `build_area`,
/// fed the members as a `MultiLineString`, stitches fragments sharing endpoint
/// nodes into closed rings and applies the even-odd nesting rule (odd-depth ring =
/// hole of the outer containing it; member roles are not trusted). Returns one
/// [`Geom::Polygon`] per outer with its nested holes, each gated on
/// [`polygon_is_valid`]. Un-assemblable/invalid geometry returns empty (osmium
/// drops broken relations too).
///
/// Two-tier: `build_area` on the raw linework, and if that yields nothing, retry
/// after noding — splitting members that cross or self-touch mid-segment so
/// polygonize can find the faces. Only messy relations pay the extra cost.
pub fn assemble_multipolygon(members: &[Vec<(f64, f64)>]) -> Vec<Geom> {
    let polys = build_area_from_members(members, false);
    if !polys.is_empty() {
        return polys;
    }
    build_area_from_members(members, true)
}

/// Build polygons from member-way linework via GEOS `build_area`. `node_first`
/// planar-nodes the linework first (repair path for crossing/self-touching members).
fn build_area_from_members(members: &[Vec<(f64, f64)>], node_first: bool) -> Vec<Geom> {
    let lines: Vec<Geometry> = members
        .iter()
        .filter(|m| m.len() >= 2)
        .filter_map(|m| Geometry::create_line_string(ring_to_coordseq(m)).ok())
        .collect();
    if lines.is_empty() {
        return Vec::new();
    }
    let Ok(mls) = Geometry::create_multiline_string(lines) else {
        return Vec::new();
    };
    let noded = if node_first { mls.node() } else { Ok(mls) };
    let assembled = noded.and_then(|g| g.build_area());
    let Ok(area) = assembled else {
        return Vec::new();
    };
    let mut polys = Vec::new();
    collect_polygons(from_geos(&area), &mut polys);
    // Keep only rings osmium would accept (same guard as the closed-way path).
    polys.retain(|g| match g {
        Geom::Polygon { exterior, interiors } => polygon_is_valid(exterior, interiors),
        _ => false,
    });
    polys
}

/// A clip box as a GEOS polygon, ccw ring order
/// `(maxx,miny),(maxx,maxy),(minx,maxy),(minx,miny)`, closed. Shared by
/// [`clip_to_box`] and [`crate::land`] so both build the identical ring.
pub(crate) fn box_polygon((minx, miny, maxx, maxy): (f64, f64, f64, f64)) -> Result<Geometry, geos::Error> {
    let ring = [(maxx, miny), (maxx, maxy), (minx, maxy), (minx, miny), (maxx, miny)];
    let lr = Geometry::create_linear_ring(ring_to_coordseq(&ring))?;
    Geometry::create_polygon(lr, vec![])
}

/// Fallible [`Geom::Polygon`] → GEOS polygon: unlike [`to_geos`] (which `expect`s),
/// any ring that won't assemble into a valid `LinearRing`/`Polygon` yields `None`.
/// A non-polygon geometry is also `None`. Used by [`union_polygons`], where a GEOS
/// failure must fall back to passthrough, never panic the pack.
pub(crate) fn try_polygon_to_geos(g: &Geom) -> Option<Geometry> {
    let Geom::Polygon { exterior, interiors } = g else {
        return None;
    };
    // A linear ring needs ≥4 positions (≥3 distinct + closing) or GEOS errors.
    if exterior.len() < 4 {
        return None;
    }
    let ext = Geometry::create_linear_ring(ring_to_coordseq(exterior)).ok()?;
    let mut holes = Vec::with_capacity(interiors.len());
    for r in interiors {
        if r.len() < 4 {
            return None;
        }
        holes.push(Geometry::create_linear_ring(ring_to_coordseq(r)).ok()?);
    }
    Geometry::create_polygon(ext, holes).ok()
}

/// Dissolve a set of fill **polygons** into their union.
///
/// Rather than one global `unary_union` over the whole set, the polygons are first
/// split into **vertex-sharing connected components** (see [`vertex_components`])
/// and each component unioned independently, in parallel; a component of one
/// polygon — the common case, since most fills have no same-class neighbour —
/// passes straight through with **no GEOS round-trip at all**. This is far cheaper
/// than the global union: the isolated majority skip the overlay machinery
/// entirely, and the surviving clusters (adjacent parcels, tiled landuse) are small
/// and union in parallel instead of as one giant serial call.
///
/// Adjacency is decided by a shared vertex because separate OSM ways that abut
/// reference the *same* boundary nodes, so their rings carry bit-identical
/// coordinates there. This is a near-exact decomposition of the global union: the
/// only merges it can miss are two same-class polygons that overlap without sharing
/// a node (a rare mapping artefact), which stay as two parts instead of one — and
/// since the class merges only fills that render pixel-identically (same z/color/
/// priority, no outline), two undissolved same-color parts paint exactly as their
/// union would, so the miss is invisible.
///
/// Returns the flattened [`Geom::Polygon`] parts (a ring of parcels around an
/// unmapped centre keeps its interior ring); `None` only on an empty input. A
/// component whose GEOS union fails falls back to passing its own polygons through
/// unmerged, so map content is never dropped. Order is deterministic: components
/// are emitted by ascending smallest-member index. Each component builds, unions,
/// and reads back its GEOS geometries **wholly on one thread** (no `geos::Geometry`
/// — which is `!Send` — ever crosses a thread boundary), so the parallel map is
/// safe, and the clustering itself is pure-Rust on plain coordinates.
pub fn union_polygons(polys: &[&Geom]) -> Option<Vec<Geom>> {
    if polys.is_empty() {
        return None;
    }
    let components = vertex_components(polys);
    let out: Vec<Geom> = components
        .par_iter()
        .map(|comp| {
            if comp.len() == 1 {
                // Isolated fill: no neighbour to dissolve into — pass it through
                // untouched, skipping GEOS entirely.
                return vec![polys[comp[0]].clone()];
            }
            let refs: Vec<&Geom> = comp.iter().map(|&i| polys[i]).collect();
            union_all(&refs).unwrap_or_else(|| refs.iter().map(|g| (*g).clone()).collect())
        })
        .flatten()
        .collect();
    (!out.is_empty()).then_some(out)
}

/// Union a set of polygons with a single global GEOS `unary_union` (the
/// cascaded/STRtree union). Returns the flattened [`Geom::Polygon`] parts, or
/// `None` on any GEOS failure or empty result. This is the per-component worker
/// behind [`union_polygons`]; it builds, unions, and reads back wholly on the
/// calling thread (`geos::Geometry` is `!Send`).
///
/// Public because a caller that needs a *true* union must not go through
/// [`union_polygons`]: that one clusters by shared vertices first, which is right
/// for dissolving abutting map fills (they share OSM nodes) and wrong for polygons
/// that genuinely overlap without touching a vertex — two such polygons would come
/// back unmerged, and an even-odd reading of the result would count their overlap
/// as a hole. The bakery's source-coverage test (`obc-bake`'s `coverage`) needs the
/// real thing, over a handful of country outlines.
pub fn union_all(polys: &[&Geom]) -> Option<Vec<Geom>> {
    let mut geoms = Vec::with_capacity(polys.len());
    for g in polys {
        geoms.push(try_polygon_to_geos(g)?);
    }
    if geoms.is_empty() {
        return None;
    }
    let collection = Geometry::create_multipolygon(geoms).ok()?;
    let unioned = collection.unary_union().ok()?;
    let mut out = Vec::new();
    collect_polygons(from_geos(&unioned), &mut out);
    (!out.is_empty()).then_some(out)
}

/// Partition polygon indices `0..polys.len()` into connected components where two
/// polygons are linked iff they share a vertex. Abutting OSM ways reference the
/// same boundary nodes, so their rings carry bit-identical coordinates along the
/// shared edge; quantising to ~1 cm (`1e7` scale) before hashing absorbs any float
/// noise while keeping genuinely distinct nodes apart. A union-find over "these
/// indices touched the same grid point" yields the components in one pass.
///
/// Output is deterministic regardless of link order: components are grouped by
/// walking indices `0..n`, so each component's members are ascending and the
/// components themselves are ordered by their smallest member.
fn vertex_components(polys: &[&Geom]) -> Vec<Vec<usize>> {
    let n = polys.len();
    let mut parent: Vec<usize> = (0..n).collect();
    fn find(parent: &mut [usize], mut x: usize) -> usize {
        while parent[x] != x {
            parent[x] = parent[parent[x]]; // path halving
            x = parent[x];
        }
        x
    }
    fn union(parent: &mut [usize], a: usize, b: usize) {
        let (ra, rb) = (find(parent, a), find(parent, b));
        if ra != rb {
            parent[ra.max(rb)] = ra.min(rb);
        }
    }
    // Quantise to ~1 cm so exact shared nodes collide but distinct ones don't.
    const SCALE: f64 = 1e7;
    let key = |(x, y): (f64, f64)| ((x * SCALE).round() as i64, (y * SCALE).round() as i64);
    // First polygon index seen at each grid point; later arrivals link to it.
    let total_verts: usize = polys
        .iter()
        .map(|g| match g {
            Geom::Polygon { exterior, interiors } => exterior.len() + interiors.iter().map(Vec::len).sum::<usize>(),
            _ => 0,
        })
        .sum();
    let mut seen: HashMap<(i64, i64), usize> = HashMap::with_capacity(total_verts);
    for (i, g) in polys.iter().enumerate() {
        if let Geom::Polygon { exterior, interiors } = g {
            for &v in exterior.iter().chain(interiors.iter().flatten()) {
                match seen.entry(key(v)) {
                    std::collections::hash_map::Entry::Occupied(e) => union(&mut parent, i, *e.get()),
                    std::collections::hash_map::Entry::Vacant(e) => {
                        e.insert(i);
                    }
                }
            }
        }
    }
    // Group indices by root, preserving ascending order within and across groups.
    let mut groups: HashMap<usize, Vec<usize>> = HashMap::new();
    let mut roots_in_order: Vec<usize> = Vec::new();
    for i in 0..n {
        let r = find(&mut parent, i);
        let g = groups.entry(r).or_default();
        if g.is_empty() {
            roots_in_order.push(r);
        }
        g.push(i);
    }
    roots_in_order.into_iter().map(|r| groups.remove(&r).unwrap()).collect()
}

/// Stitch a set of **lines** into maximal-length polylines via GEOS `line_merge`
/// (the LineMerger): joins linestrings sharing an endpoint at a degree-2 node,
/// dropping the duplicated join vertex, and stops at degree-≥3 junctions and where
/// two lines only cross without a shared vertex — so an OSM way split into many
/// segments recombines, but distinct roads meeting at a junction stay separate.
/// Returns the flattened [`Geom::Line`] parts of the result (disjoint members pass
/// straight through as their own lines), or `None` on any GEOS failure or an empty
/// result — so the caller passes the group through unmerged rather than drop map
/// content. Builds, merges, and reads back **wholly on the calling thread**: no
/// `geos::Geometry` ever crosses a thread boundary (it is `!Send`), so this is safe
/// to call from a rayon worker (see [`union_polygons`]).
pub fn merge_lines_geos(lines: &[&Geom]) -> Option<Vec<Geom>> {
    let mut geoms = Vec::with_capacity(lines.len());
    for g in lines {
        // A caller in `merge.rs` only ever hands us `Geom::Line`s; a degenerate
        // <2-vertex line can't form a linestring, so bail to the unmerged fallback.
        let Geom::Line(c) = g else { return None };
        if c.len() < 2 {
            return None;
        }
        geoms.push(Geometry::create_line_string(ring_to_coordseq(c)).ok()?);
    }
    if geoms.is_empty() {
        return None;
    }
    let collection = Geometry::create_multiline_string(geoms).ok()?;
    let merged = collection.line_merge().ok()?;
    let mut out = Vec::new();
    collect_lines(from_geos(&merged), &mut out);
    (!out.is_empty()).then_some(out)
}

/// Flatten a `line_merge` result into its [`Geom::Line`] parts (a single merged
/// chain reads back as one `Line`; several as a `Multi` of them).
pub(crate) fn collect_lines(g: Geom, out: &mut Vec<Geom>) {
    match g {
        l @ Geom::Line(_) => out.push(l),
        Geom::Multi(parts) => {
            for p in parts {
                collect_lines(p, out);
            }
        }
        _ => {}
    }
}

// --- the GEOS polygonal-coverage API, via `geos-sys` -----------------------
//
// The two entry points [`crate::coverage`] needs that the safe `geos` crate does not wrap.

/// `GEOSCoverageSimplifyVW` / `GEOSCoverageIsValid`, spoken to directly.
///
/// `geos` 11.1 wraps `GEOSCoverageUnion` but neither of these two, and its `Geometry` keeps
/// its raw pointer private (the `AsRaw` trait is crate-private), so a wrapped geometry cannot
/// be handed to a C entry point the crate does not already cover. This module therefore talks
/// to `geos::sys` — the same `geos-sys` the safe crate itself is built on, so no new
/// dependency and no second version of libGEOS — and takes [`Geom`] in and out.
///
/// # Safety story
///
/// Every raw pointer is created, used and destroyed **inside one call on one thread**, under a
/// context handle created first and dropped last, and the RAII guards below free their
/// geometry on every exit path (early return, `?`, panic). Nothing GEOS-owned escapes: results
/// are copied into owned Rust [`Geom`]s before the guards run, and no pointer is ever shared
/// between threads or stored. Every `unsafe` block in the packer's coverage path is in here.
mod coverage_api {
    use std::ffi::c_int;
    use std::ptr;

    use geos::sys::{
        GEOSContextHandle_t, GEOSCoordSeq_create_r, GEOSCoordSeq_destroy_r, GEOSCoordSeq_getSize_r,
        GEOSCoordSeq_getXY_r, GEOSCoordSeq_setXY_r, GEOSCoordSequence, GEOSCoverageIsValid_r, GEOSCoverageSimplifyVW_r,
        GEOSGeomTypeId_r, GEOSGeomTypes_GEOS_GEOMETRYCOLLECTION, GEOSGeomTypes_GEOS_LINEARRING,
        GEOSGeomTypes_GEOS_LINESTRING, GEOSGeomTypes_GEOS_MULTIPOLYGON, GEOSGeomTypes_GEOS_POLYGON,
        GEOSGeom_createCollection_r, GEOSGeom_createLinearRing_r, GEOSGeom_createPolygon_r, GEOSGeom_destroy_r,
        GEOSGeom_getCoordSeq_r, GEOSGeometry, GEOSGetExteriorRing_r, GEOSGetGeometryN_r, GEOSGetInteriorRingN_r,
        GEOSGetNumGeometries_r, GEOSGetNumInteriorRings_r, GEOS_finish_r, GEOS_init_r, GEOSisEmpty_r,
    };

    use super::Geom;

    // GEOS type ids, renamed to Rust casing so they can be matched on.
    const TYPE_LINESTRING: u32 = GEOSGeomTypes_GEOS_LINESTRING;
    const TYPE_LINEARRING: u32 = GEOSGeomTypes_GEOS_LINEARRING;
    const TYPE_POLYGON: u32 = GEOSGeomTypes_GEOS_POLYGON;
    const TYPE_MULTIPOLYGON: u32 = GEOSGeomTypes_GEOS_MULTIPOLYGON;
    const TYPE_COLLECTION: u32 = GEOSGeomTypes_GEOS_GEOMETRYCOLLECTION;

    /// A GEOS context handle, freed on drop. Declared before every geometry that uses it, so
    /// Rust's reverse-declaration drop order destroys the geometries first.
    struct Context(GEOSContextHandle_t);

    impl Context {
        fn new() -> Option<Context> {
            // SAFETY: `GEOS_init_r` takes no arguments and returns an owned handle or null.
            let handle = unsafe { GEOS_init_r() };
            (!handle.is_null()).then_some(Context(handle))
        }

        fn raw(&self) -> GEOSContextHandle_t {
            self.0
        }
    }

    impl Drop for Context {
        fn drop(&mut self) {
            // SAFETY: `self.0` is a non-null handle from `GEOS_init_r`, freed exactly once
            // (`Context` is neither `Copy` nor `Clone`), after every geometry built under it.
            unsafe { GEOS_finish_r(self.0) };
        }
    }

    /// An owned GEOS geometry, destroyed on drop.
    struct Owned<'c> {
        ctx: &'c Context,
        ptr: *mut GEOSGeometry,
    }

    impl<'c> Owned<'c> {
        /// Adopt a freshly created geometry; `None` (nothing to free) if GEOS returned null.
        fn new(ctx: &'c Context, ptr: *mut GEOSGeometry) -> Option<Owned<'c>> {
            (!ptr.is_null()).then_some(Owned { ctx, ptr })
        }
    }

    impl Drop for Owned<'_> {
        fn drop(&mut self) {
            // SAFETY: `ptr` is non-null (checked in `new`), owned by this guard alone, and
            // destroyed exactly once — the guard is consumed by drop and never copied.
            unsafe { GEOSGeom_destroy_r(self.ctx.raw(), self.ptr) };
        }
    }

    /// The children of a collection under construction: freed on drop until
    /// [`Nursery::release`] hands them to GEOS.
    struct Nursery<'c> {
        ctx: &'c Context,
        ptrs: Vec<*mut GEOSGeometry>,
    }

    impl<'c> Nursery<'c> {
        fn new(ctx: &'c Context, capacity: usize) -> Nursery<'c> {
            Nursery { ctx, ptrs: Vec::with_capacity(capacity) }
        }

        fn push(&mut self, ptr: *mut GEOSGeometry) -> Option<()> {
            if ptr.is_null() {
                return None;
            }
            self.ptrs.push(ptr);
            Some(())
        }

        /// Give up ownership: the returned pointers are GEOS's problem from here on.
        fn release(&mut self) -> Vec<*mut GEOSGeometry> {
            std::mem::take(&mut self.ptrs)
        }
    }

    impl Drop for Nursery<'_> {
        fn drop(&mut self) {
            for &p in &self.ptrs {
                // SAFETY: every pointer in `ptrs` is a non-null geometry this nursery owns
                // (`push` rejects null) and has not been released to GEOS.
                unsafe { GEOSGeom_destroy_r(self.ctx.raw(), p) };
            }
        }
    }

    /// One closed ring as a GEOS `LinearRing`, or null. A ring that does not repeat its first
    /// vertex is closed here rather than rejected — GEOS refuses an open ring outright.
    fn build_ring(ctx: &Context, coords: &[(f64, f64)]) -> *mut GEOSGeometry {
        if coords.len() < 3 {
            return ptr::null_mut();
        }
        let closed = coords.first() == coords.last();
        let n = coords.len() + usize::from(!closed);
        if n < 4 {
            return ptr::null_mut();
        }
        // SAFETY: `n` fits the sequence created immediately below, every index written is
        // `< n`, and the sequence is either handed to `GEOSGeom_createLinearRing_r` (which
        // takes ownership) or destroyed here.
        unsafe {
            let seq: *mut GEOSCoordSequence = GEOSCoordSeq_create_r(ctx.raw(), n as u32, 2);
            if seq.is_null() {
                return ptr::null_mut();
            }
            for (i, &(x, y)) in coords.iter().enumerate() {
                if GEOSCoordSeq_setXY_r(ctx.raw(), seq, i as u32, x, y) == 0 {
                    GEOSCoordSeq_destroy_r(ctx.raw(), seq);
                    return ptr::null_mut();
                }
            }
            if !closed {
                let (x, y) = coords[0];
                if GEOSCoordSeq_setXY_r(ctx.raw(), seq, (n - 1) as u32, x, y) == 0 {
                    GEOSCoordSeq_destroy_r(ctx.raw(), seq);
                    return ptr::null_mut();
                }
            }
            GEOSGeom_createLinearRing_r(ctx.raw(), seq)
        }
    }

    /// One [`Geom::Polygon`] as a GEOS polygon, or null (a non-polygon, or a ring GEOS
    /// rejects).
    fn build_polygon(ctx: &Context, g: &Geom) -> *mut GEOSGeometry {
        let Geom::Polygon { exterior, interiors } = g else {
            return ptr::null_mut();
        };
        let shell = build_ring(ctx, exterior);
        if shell.is_null() {
            return ptr::null_mut();
        }
        let mut holes = Nursery::new(ctx, interiors.len());
        for r in interiors {
            if holes.push(build_ring(ctx, r)).is_none() {
                // SAFETY: `shell` is a live, unshared ring this function owns; the nursery
                // frees the holes built so far.
                unsafe { GEOSGeom_destroy_r(ctx.raw(), shell) };
                return ptr::null_mut();
            }
        }
        let mut raw = holes.release();
        // An empty `Vec`'s `as_mut_ptr` is a dangling (aligned, non-null) pointer, and handing one
        // to a C function is a promise we cannot keep even where it is only read `0` times. A
        // hole-less polygon passes a real null instead.
        let holes_ptr = if raw.is_empty() { ptr::null_mut() } else { raw.as_mut_ptr() };
        // SAFETY: `shell` and every hole are live rings owned here; `GEOSGeom_createPolygon_r`
        // takes ownership of all of them (the `raw` *array* stays ours) and returns null on
        // failure — the documented C-API leak on that path is accepted rather than risking a
        // double free.
        unsafe { GEOSGeom_createPolygon_r(ctx.raw(), shell, holes_ptr, raw.len() as u32) }
    }

    /// The polygons as one GEOS `GeometryCollection` — the shape both coverage entry points
    /// take. Element order is the input order, which is what carries class identity back.
    fn build_collection<'c>(ctx: &'c Context, polys: &[&Geom]) -> Option<Owned<'c>> {
        let mut nursery = Nursery::new(ctx, polys.len());
        for g in polys {
            nursery.push(build_polygon(ctx, g))?;
        }
        let mut raw = nursery.release();
        // Same dangling-pointer rule as `build_polygon`: an empty collection passes null, not the
        // aligned nothing an empty `Vec` hands out.
        let members = if raw.is_empty() { ptr::null_mut() } else { raw.as_mut_ptr() };
        // SAFETY: every element is a live polygon owned here; `GEOSGeom_createCollection_r`
        // takes ownership of them (the array stays ours). Same accepted-leak note as above on
        // the null path.
        let ptr =
            unsafe { GEOSGeom_createCollection_r(ctx.raw(), TYPE_COLLECTION as c_int, members, raw.len() as u32) };
        Owned::new(ctx, ptr)
    }

    /// Read a `LineString`/`LinearRing`'s coordinates into owned pairs.
    ///
    /// # Safety
    /// `g` must be a live geometry created under `ctx` whose type carries a coordinate
    /// sequence (line string or linear ring).
    unsafe fn read_ring(ctx: &Context, g: *const GEOSGeometry) -> Option<Vec<(f64, f64)>> {
        let seq = GEOSGeom_getCoordSeq_r(ctx.raw(), g);
        if seq.is_null() {
            return None;
        }
        let mut size: u32 = 0;
        if GEOSCoordSeq_getSize_r(ctx.raw(), seq, &mut size) == 0 {
            return None;
        }
        let mut out = Vec::with_capacity(size as usize);
        for i in 0..size {
            let (mut x, mut y) = (0.0, 0.0);
            if GEOSCoordSeq_getXY_r(ctx.raw(), seq, i, &mut x, &mut y) == 0 {
                return None;
            }
            out.push((x, y));
        }
        Some(out)
    }

    /// Read a GEOS result into a [`Geom`]; anything non-areal and non-linear reads as
    /// [`Geom::Empty`], exactly like the safe path's `from_geos`.
    ///
    /// # Safety
    /// `g` must be a live geometry created under `ctx`. Sub-geometry accessors return
    /// pointers *borrowed* from `g`, which stays alive for the whole walk, and are never
    /// destroyed here.
    unsafe fn read_geom(ctx: &Context, g: *const GEOSGeometry) -> Option<Geom> {
        // 1 = empty, 2 = exception. They must not be conflated: an exception means GEOS could not
        // answer, and reading that as "empty" would silently delete this element's content while
        // the call as a whole still reported success. `None` instead, which fails the whole call
        // and takes the caller's never-drop fallback.
        match GEOSisEmpty_r(ctx.raw(), g) {
            0 => {}
            1 => return Some(Geom::Empty),
            _ => return None,
        }
        let type_id = GEOSGeomTypeId_r(ctx.raw(), g);
        if type_id < 0 {
            return None;
        }
        match type_id as u32 {
            TYPE_POLYGON => {
                let ext = GEOSGetExteriorRing_r(ctx.raw(), g);
                if ext.is_null() {
                    return None;
                }
                let exterior = read_ring(ctx, ext)?;
                let nholes = GEOSGetNumInteriorRings_r(ctx.raw(), g);
                if nholes < 0 {
                    return None;
                }
                let mut interiors = Vec::with_capacity(nholes as usize);
                for i in 0..nholes {
                    let hole = GEOSGetInteriorRingN_r(ctx.raw(), g, i);
                    if hole.is_null() {
                        return None;
                    }
                    interiors.push(read_ring(ctx, hole)?);
                }
                Some(Geom::Polygon { exterior, interiors })
            }
            TYPE_LINESTRING | TYPE_LINEARRING => Some(Geom::Line(read_ring(ctx, g)?)),
            TYPE_MULTIPOLYGON | TYPE_COLLECTION => {
                let n = GEOSGetNumGeometries_r(ctx.raw(), g);
                if n < 0 {
                    return None;
                }
                let mut parts = Vec::with_capacity(n as usize);
                for i in 0..n {
                    let child = GEOSGetGeometryN_r(ctx.raw(), g, i);
                    if child.is_null() {
                        return None;
                    }
                    parts.push(read_geom(ctx, child)?);
                }
                Some(Geom::Multi(parts))
            }
            _ => Some(Geom::Empty),
        }
    }

    /// Simplify a polygonal **coverage** with `GEOSCoverageSimplifyVW`: every edge shared by
    /// two elements is simplified **once**, so neighbours stay glued at any tolerance — the
    /// whole reason this pass exists (per-feature simplify moves each copy of a shared
    /// boundary independently and tears the seam open).
    ///
    /// `tol` is in degrees, like [`super::topology_preserve_simplify`]. `preserve_boundary`
    /// pins the coverage's outer edge; the packer passes `false`, because the outer edge of a
    /// tier's fills is a coastline or a landuse rim like any other line and must simplify at
    /// the tier's tolerance too — pinning it would leave un-simplified vertex noise around
    /// every cluster at the coarsest zooms.
    ///
    /// Returns one [`Geom`] per input element **in input order** (GEOS preserves it, which is
    /// how the caller keeps each element's style), or `None` on any GEOS failure or a
    /// element-count mismatch — the caller then falls back to the per-feature path and drops
    /// nothing.
    pub fn coverage_simplify_vw(polys: &[&Geom], tol: f64, preserve_boundary: bool) -> Option<Vec<Geom>> {
        if polys.is_empty() {
            return None;
        }
        let ctx = Context::new()?;
        let input = build_collection(&ctx, polys)?;
        // SAFETY: `input.ptr` is a live collection built under `ctx`; the result is adopted by
        // an `Owned` guard (or is null, which `Owned::new` reports as `None`).
        let simplified = unsafe {
            let ptr = GEOSCoverageSimplifyVW_r(ctx.raw(), input.ptr, tol, c_int::from(preserve_boundary));
            Owned::new(&ctx, ptr)?
        };
        // SAFETY: `simplified.ptr` is live for the whole read; child pointers are borrowed
        // from it and never freed here.
        unsafe {
            let n = GEOSGetNumGeometries_r(ctx.raw(), simplified.ptr);
            if n < 0 || n as usize != polys.len() {
                return None;
            }
            let mut out = Vec::with_capacity(polys.len());
            for i in 0..n {
                let child = GEOSGetGeometryN_r(ctx.raw(), simplified.ptr, i);
                if child.is_null() {
                    return None;
                }
                out.push(read_geom(&ctx, child)?);
            }
            Some(out)
        }
    }

    /// Whether these polygons form a **valid coverage** (`GEOSCoverageIsValid`): interiors
    /// disjoint and every shared edge vertex-for-vertex identical on both sides. `gap_width`
    /// is the narrow-gap width to also report as invalid; `0.0` checks overlaps and mismatched
    /// edges only. A GEOS failure reads as invalid — the caller's fallback is the safe answer.
    pub fn coverage_is_valid(polys: &[&Geom], gap_width: f64) -> bool {
        if polys.is_empty() {
            return false;
        }
        let Some(ctx) = Context::new() else { return false };
        let Some(input) = build_collection(&ctx, polys) else { return false };
        let mut invalid_edges: *mut GEOSGeometry = ptr::null_mut();
        // SAFETY: `input.ptr` is a live collection under `ctx`; `invalid_edges` is a valid
        // out-pointer, and the geometry GEOS may write into it is owned by us and freed here.
        let rc = unsafe { GEOSCoverageIsValid_r(ctx.raw(), input.ptr, gap_width, &mut invalid_edges) };
        if !invalid_edges.is_null() {
            // SAFETY: a live geometry GEOS just handed us, destroyed exactly once.
            unsafe { GEOSGeom_destroy_r(ctx.raw(), invalid_edges) };
        }
        rc == 1
    }
}

pub use coverage_api::{coverage_is_valid, coverage_simplify_vw};

/// Clip `geom` to the node box (integer microdegrees → degrees) via GEOS
/// `intersection`.
///
/// Real OSM data occasionally arrives here **invalid** — the DACH bake's first casualty was a
/// merged fill whose union left a hole no shell contains (`TopologyException: unable to assign
/// free hole to a shell`), which GEOS refuses to intersect. The repair runs **only on the failure
/// path**: a geometry today's code clips cleanly takes exactly the code it always took, so no
/// previously-baked cell can move a byte — `make_valid` is reached solely where the bake used to
/// die, and its output is deterministic, so the recovered cell is too.
pub fn clip_to_box(geom: &Geom, bbox: (i64, i64, i64, i64)) -> Geom {
    let (minx, miny, maxx, maxy) = (bbox.0 as f64 / 1e6, bbox.1 as f64 / 1e6, bbox.2 as f64 / 1e6, bbox.3 as f64 / 1e6);
    let box_geom = box_polygon((minx, miny, maxx, maxy)).expect("box polygon");
    let g = to_geos(geom);
    let clipped = match g.intersection(&box_geom) {
        Ok(c) => c,
        Err(first) => {
            let valid = g
                .make_valid()
                .unwrap_or_else(|e| panic!("intersection failed ({first}) and make_valid failed too ({e})"));
            valid
                .intersection(&box_geom)
                .unwrap_or_else(|e| panic!("intersection failed even after make_valid ({first}; then {e})"))
        }
    };
    from_geos(&clipped)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ring(pts: &[(f64, f64)]) -> Vec<(f64, f64)> {
        pts.to_vec()
    }

    // R1 outer (way 101) + inner (way 102): concentric squares.
    fn r1_outer() -> Vec<(f64, f64)> {
        ring(&[(7.800, 47.990), (7.804, 47.990), (7.804, 47.994), (7.800, 47.994), (7.800, 47.990)])
    }
    fn r1_inner() -> Vec<(f64, f64)> {
        ring(&[(7.801, 47.991), (7.803, 47.991), (7.803, 47.993), (7.801, 47.993), (7.801, 47.991)])
    }
    // R2 outer A (way 103) + outer B (way 104): two disjoint squares.
    fn r2_a() -> Vec<(f64, f64)> {
        ring(&[(7.806, 47.990), (7.808, 47.990), (7.808, 47.992), (7.806, 47.992), (7.806, 47.990)])
    }
    fn r2_b() -> Vec<(f64, f64)> {
        ring(&[(7.810, 47.990), (7.812, 47.990), (7.812, 47.992), (7.810, 47.992), (7.810, 47.990)])
    }

    #[test]
    fn assemble_r1_lake_with_island() {
        let polys = assemble_multipolygon(&[r1_outer(), r1_inner()]);
        assert_eq!(polys.len(), 1, "R1 → exactly one polygon");
        match &polys[0] {
            Geom::Polygon { exterior, interiors } => {
                assert!(exterior.len() >= 4, "outer is a closed ring");
                assert_eq!(interiors.len(), 1, "the island is one hole");
            }
            other => panic!("expected a polygon, got {other:?}"),
        }
    }

    #[test]
    fn assemble_r2_two_outers() {
        let polys = assemble_multipolygon(&[r2_a(), r2_b()]);
        assert_eq!(polys.len(), 2, "R2 → two disjoint polygons");
        for p in &polys {
            match p {
                Geom::Polygon { interiors, .. } => assert!(interiors.is_empty(), "no holes"),
                other => panic!("expected a polygon, got {other:?}"),
            }
        }
    }

    /// Feeding the inner ring first must still yield the lake-with-hole
    /// (build_area classifies by geometry, not member order/role).
    #[test]
    fn assemble_is_order_independent() {
        let polys = assemble_multipolygon(&[r1_inner(), r1_outer()]);
        assert_eq!(polys.len(), 1);
        if let Geom::Polygon { interiors, .. } = &polys[0] {
            assert_eq!(interiors.len(), 1);
        } else {
            panic!("expected polygon");
        }
    }

    /// A single un-closeable fragment assembles to nothing (skip-and-warn parity).
    #[test]
    fn assemble_open_fragment_is_dropped() {
        let open = ring(&[(7.800, 47.990), (7.804, 47.990)]);
        assert!(assemble_multipolygon(&[open]).is_empty());
    }

    // --- clip_to_box ---------------------------------------------------------

    fn bounds_of(g: &Geom) -> (f64, f64, f64, f64) {
        g.bounds()
    }

    /// **The DACH bake's crash, pinned.** A merged fill can arrive with a hole no shell contains
    /// (`TopologyException: unable to assign free hole to a shell` at 10.4874°E 50.0780°N), which
    /// GEOS refuses to intersect. The clip must repair-and-retry instead of dying: one degenerate
    /// polygon out of a country's millions must never cost the whole bake. (Probe-verified: this
    /// fixture genuinely takes the `Err` path — a panic stubbed into the `Ok` arm does not fire.)
    #[test]
    fn clip_repairs_a_free_hole_instead_of_panicking() {
        // A shell with an interior ring entirely OUTSIDE it — the "free hole" GEOS cannot assign.
        let invalid = Geom::Polygon {
            exterior: ring(&[(0.0000, 0.0000), (0.0004, 0.0000), (0.0004, 0.0004), (0.0000, 0.0004), (0.0000, 0.0000)]),
            interiors: vec![ring(&[
                (0.0008, 0.0008),
                (0.0009, 0.0008),
                (0.0009, 0.0009),
                (0.0008, 0.0009),
                (0.0008, 0.0008),
            ])],
        };
        let clipped = clip_to_box(&invalid, (0, 0, 1000, 1000));
        // The repaired shell survives the clip; where exactly GEOS puts the freed hole is its
        // business — what this pins is "a valid, in-box result, not a panic".
        let (minx, miny, maxx, maxy) = bounds_of(&clipped);
        assert!(minx >= 0.0 && miny >= 0.0 && maxx <= 0.001 && maxy <= 0.001, "{clipped:?}");
        match &clipped {
            Geom::Polygon { exterior, .. } => assert!(exterior.len() >= 4),
            Geom::Multi(parts) => assert!(!parts.is_empty()),
            other => panic!("the shell vanished: {other:?}"),
        }
    }

    /// Pins the `bbox / 1e6` µdeg→deg scaling: box `(0,0,1000,1000)` µdeg is
    /// `0.0..0.001` deg, so a line poking out both sides clips at x=0 and x=0.001.
    #[test]
    fn clip_line_straddling_both_edges() {
        let line = Geom::Line(vec![(-0.0005, 0.0005), (0.0015, 0.0005)]);
        let clipped = clip_to_box(&line, (0, 0, 1000, 1000));
        match clipped {
            Geom::Line(pts) => {
                assert_eq!(pts.len(), 2, "a single crossing yields one 2-point segment");
                let mut xs = [pts[0].0, pts[1].0];
                xs.sort_by(|a, b| a.partial_cmp(b).unwrap());
                assert!((xs[0] - 0.0).abs() < 1e-12, "left cut at x=0, got {}", xs[0]);
                assert!((xs[1] - 0.001).abs() < 1e-12, "right cut at x=0.001, got {}", xs[1]);
                assert!((pts[0].1 - 0.0005).abs() < 1e-12 && (pts[1].1 - 0.0005).abs() < 1e-12, "y unchanged");
            }
            other => panic!("expected a clipped LineString, got {other:?}"),
        }
    }

    /// A polygon larger than the box clips to exactly the box. Guards the polygon
    /// branch of `from_geos` (exterior ring read-back) and the clip-box ring.
    #[test]
    fn clip_polygon_to_box_yields_box() {
        let big = Geom::Polygon {
            exterior: vec![
                (-0.0005, -0.0005),
                (0.0015, -0.0005),
                (0.0015, 0.0015),
                (-0.0005, 0.0015),
                (-0.0005, -0.0005),
            ],
            interiors: vec![],
        };
        let clipped = clip_to_box(&big, (0, 0, 1000, 1000));
        match &clipped {
            Geom::Polygon { exterior, interiors } => {
                assert!(interiors.is_empty(), "clipping a hole-free poly to a box stays hole-free");
                let (minx, miny, maxx, maxy) = bounds_of(&clipped);
                assert!(minx.abs() < 1e-12 && miny.abs() < 1e-12, "min corner at the box origin");
                assert!((maxx - 0.001).abs() < 1e-12 && (maxy - 0.001).abs() < 1e-12, "max corner at the box max");
                assert!(exterior.len() >= 4, "a closed clipped ring");
            }
            other => panic!("expected a clipped Polygon, got {other:?}"),
        }
    }

    /// A line that leaves and re-enters the box clips to a `MultiLineString`, which
    /// `from_geos` must turn into a `Geom::Multi` — the Multi-flattening path the
    /// quadtree relies on. The line dips below the box in the middle, so two inside
    /// segments survive.
    #[test]
    fn clip_line_reentering_box_is_multi() {
        let line = Geom::Line(vec![
            (-0.0002, 0.0005), // start left, outside
            (0.0003, 0.0005),  // inside
            (0.0005, -0.0002), // dip below the box (outside)
            (0.0007, 0.0005),  // back inside
            (0.0012, 0.0005),  // exit right, outside
        ]);
        let clipped = clip_to_box(&line, (0, 0, 1000, 1000));
        match clipped {
            Geom::Multi(parts) => {
                let lines: Vec<_> = parts.iter().filter(|p| matches!(p, Geom::Line(_))).collect();
                assert_eq!(lines.len(), 2, "two disjoint segments survive inside the box");
                for p in &lines {
                    let (minx, miny, maxx, maxy) = bounds_of(p);
                    assert!(minx >= -1e-9 && miny >= -1e-9, "segment stays inside the box (min)");
                    assert!(maxx <= 0.001 + 1e-9 && maxy <= 0.001 + 1e-9, "segment stays inside the box (max)");
                }
            }
            other => panic!("expected a Multi from a re-entering line, got {other:?}"),
        }
    }

    /// A feature entirely outside the box clips to `Empty` (the quadtree drops it).
    #[test]
    fn clip_disjoint_is_empty() {
        let line = Geom::Line(vec![(0.005, 0.005), (0.006, 0.006)]);
        let clipped = clip_to_box(&line, (0, 0, 1000, 1000));
        assert!(clipped.is_empty(), "a disjoint feature clips to Empty");
    }

    // --- topology_preserve_simplify -----------------------------------------

    /// A 3-point line whose middle vertex sits 0.0001° off the chord keeps all 3
    /// points when `tol` is below that deviation and drops the middle vertex when
    /// `tol` is above it — `tol` bites exactly at the deviation boundary.
    #[test]
    fn simplify_keeps_survivor_drops_redundant() {
        let line = Geom::Line(vec![(0.0, 0.0), (0.001, 0.0001), (0.002, 0.0)]);

        let tight = topology_preserve_simplify(&line, 0.00001);
        match tight {
            Geom::Line(pts) => assert_eq!(pts.len(), 3, "tol below deviation keeps the middle vertex"),
            other => panic!("expected a Line, got {other:?}"),
        }

        let loose = topology_preserve_simplify(&line, 0.001);
        match loose {
            Geom::Line(pts) => {
                assert_eq!(pts.len(), 2, "tol above deviation drops the redundant middle vertex");
                assert!((pts[0].0 - 0.0).abs() < 1e-12 && (pts[1].0 - 0.002).abs() < 1e-12, "endpoints preserved");
            }
            other => panic!("expected a Line, got {other:?}"),
        }
    }

    /// Why this is `TopologyPreservingSimplifier` and not plain Douglas–Peucker: a
    /// concave staple polygon simplified hard must NOT self-intersect. Plain DP can
    /// pull an edge across the notch and produce an invalid ring, which
    /// `polygon_is_valid` and the device renderer both assume never happens.
    #[test]
    fn simplify_is_topology_preserving_stays_valid() {
        // A fat C with a deep notch; naive DP could collapse the notch walls.
        let staple = Geom::Polygon {
            exterior: vec![
                (0.0, 0.0),
                (0.003, 0.0),
                (0.003, 0.003),
                (0.002, 0.003),
                (0.002, 0.001),
                (0.001, 0.001),
                (0.001, 0.003),
                (0.0, 0.003),
                (0.0, 0.0),
            ],
            interiors: vec![],
        };
        let out = topology_preserve_simplify(&staple, 0.0005);
        match out {
            Geom::Polygon { ref exterior, ref interiors } => {
                assert!(!exterior.is_empty(), "the polygon survives simplification");
                assert!(
                    polygon_is_valid(exterior, interiors),
                    "a topology-preserving result stays non-self-intersecting"
                );
            }
            other => panic!("expected a Polygon, got {other:?}"),
        }
    }

    /// `tol` is degrees, but the LOD config specifies simplify tolerance in meters,
    /// converted at the call site as `simplify_m / M_PER_DEG`. Getting the conversion
    /// wrong would simplify ~111 000× too aggressively and flatten every road. A
    /// ~22 m deviation survives a 10 m tolerance and is dropped by a 50 m one.
    #[test]
    fn simplify_meters_to_degrees_scale() {
        const M_PER_DEG: f64 = 111_320.0;
        let tol_deg = |m: f64| m / M_PER_DEG;

        let dev_deg = 0.0002; // ~22.3 m off the chord
        let line = Geom::Line(vec![(0.0, 0.0), (0.001, dev_deg), (0.002, 0.0)]);
        assert!(dev_deg * M_PER_DEG > 10.0 && dev_deg * M_PER_DEG < 50.0, "fixture deviation is between 10 m and 50 m");

        match topology_preserve_simplify(&line, tol_deg(10.0)) {
            Geom::Line(pts) => assert_eq!(pts.len(), 3, "10 m tol keeps the ~22 m deviation"),
            other => panic!("expected a Line, got {other:?}"),
        }
        match topology_preserve_simplify(&line, tol_deg(50.0)) {
            Geom::Line(pts) => assert_eq!(pts.len(), 2, "50 m tol drops the ~22 m deviation"),
            other => panic!("expected a Line, got {other:?}"),
        }
    }

    // --- assemble_multipolygon node-repair tier -----------------------------

    /// Forces the repair path: a self-crossing bow-tie yields nothing from the fast
    /// `build_area` pass, so the `node_first=true` retry must node the linework and
    /// recover the two faces.
    #[test]
    fn assemble_self_touching_uses_node_repair() {
        let bowtie = ring(&[(0.0, 0.0), (0.002, 0.002), (0.002, 0.0), (0.0, 0.002), (0.0, 0.0)]);
        let polys = assemble_multipolygon(&[bowtie]);
        assert_eq!(polys.len(), 2, "the node-repair tier recovers both faces of the bow-tie");
        for p in &polys {
            match p {
                Geom::Polygon { exterior, interiors } => {
                    assert!(exterior.len() >= 4, "each recovered face is a closed ring");
                    assert!(interiors.is_empty(), "the triangles have no holes");
                }
                other => panic!("expected a polygon, got {other:?}"),
            }
        }
    }

    // --- polygon_is_valid ---------------------------------------------------

    /// A simple square is valid; a self-intersecting bow-tie is not (osmium drops
    /// it); a <4-position ring can't form a linear ring and is rejected.
    #[test]
    fn polygon_is_valid_rejects_self_intersection_and_degenerate() {
        let square = ring(&[(0.0, 0.0), (0.002, 0.0), (0.002, 0.002), (0.0, 0.002), (0.0, 0.0)]);
        assert!(polygon_is_valid(&square, &[]), "a simple closed square is valid");

        let bowtie = ring(&[(0.0, 0.0), (0.002, 0.002), (0.002, 0.0), (0.0, 0.002), (0.0, 0.0)]);
        assert!(!polygon_is_valid(&bowtie, &[]), "a self-intersecting building ring is rejected");

        let degenerate = ring(&[(0.0, 0.0), (0.002, 0.0), (0.0, 0.0)]);
        assert!(!polygon_is_valid(&degenerate, &[]), "a <4-position ring can't form a polygon");
    }

    // --- footprint_below (coarse-LOD cull) ----------------------------------

    /// A square `side` degrees on a side, anchored near the equator so the `cos`
    /// foreshortening is ≈1 and the math is easy to reason about.
    fn square(side: f64) -> Geom {
        Geom::Polygon {
            exterior: ring(&[(0.0, 0.0), (side, 0.0), (side, side), (0.0, side), (0.0, 0.0)]),
            interiors: vec![],
        }
    }

    /// `trim_excess_holes` keeps the largest `max_rings - 1` holes in their
    /// original order and reports the drop count; within-cap polygons and lines
    /// pass through untouched.
    #[test]
    fn trim_excess_holes_drops_smallest_keeps_order() {
        let hole = |x0: f64, s: f64| ring(&[(x0, 0.0), (x0 + s, 0.0), (x0 + s, s), (x0, s), (x0, 0.0)]);
        // Areas: mid, small, big — with cap 3 (exterior + 2 holes) the small one goes.
        let mut g = Geom::Polygon {
            exterior: hole(0.0, 1.0),
            interiors: vec![hole(0.1, 0.02), hole(0.2, 0.01), hole(0.3, 0.03)],
        };
        assert_eq!(trim_excess_holes(&mut g, 3), 1);
        let Geom::Polygon { interiors, .. } = &g else { panic!("still a polygon") };
        assert_eq!(interiors.len(), 2);
        assert_eq!((interiors[0][0].0, interiors[1][0].0), (0.1, 0.3), "survivors keep input order");
        assert_eq!(trim_excess_holes(&mut g, 3), 0, "a within-cap polygon is untouched");
        let mut l = Geom::Line(ring(&[(0.0, 0.0), (1.0, 1.0)]));
        assert_eq!(trim_excess_holes(&mut l, 3), 0, "lines pass through");
    }

    /// `ring_area_deg2` is the plain shoelace area and ignores winding /
    /// closure: a 0.002°×0.002° square is 4e-6 deg² either way round.
    #[test]
    fn ring_area_is_shoelace_and_winding_agnostic() {
        let cw = ring(&[(0.0, 0.0), (0.0, 0.002), (0.002, 0.002), (0.002, 0.0), (0.0, 0.0)]);
        let ccw = ring(&[(0.0, 0.0), (0.002, 0.0), (0.002, 0.002), (0.0, 0.002)]); // unclosed
        assert!((ring_area_deg2(&cw) - 4e-6).abs() < 1e-12);
        assert!((ring_area_deg2(&ccw) - 4e-6).abs() < 1e-12, "closure vertex is optional");
    }

    /// At 18 m/px, a ~111 m square (≈6 px/side, ~38 px²) is kept but a ~22 m
    /// square (≈1.2 px/side, ~1.5 px²) is dropped by a 4 px² threshold.
    #[test]
    fn polygon_culled_below_area_threshold() {
        assert!(!footprint_below(&square(0.001), 18.0, 4.0), "a ~38 px² wood stays");
        assert!(footprint_below(&square(0.0002), 18.0, 4.0), "a ~1.5 px² sliver goes");
    }

    /// The same tier's `max_mpp` sets the real-world cut: at 120 m/px a field
    /// that easily survives at 18 m/px is dropped, so the cut widens as the tier
    /// coarsens even at one `min_area_px`.
    #[test]
    fn coarser_mpp_widens_the_real_world_cut() {
        let field = square(0.001); // ~111 m
        assert!(!footprint_below(&field, 18.0, 4.0), "kept at the region tier");
        assert!(footprint_below(&field, 120.0, 4.0), "dropped at the country tier");
    }

    /// Lines are never culled by the area test — a road is stored as many short
    /// ways, and dropping the shortest ones patches holes into it. Even a tiny
    /// sub-pixel segment survives; zoomed-out line density is a `min_lod` concern.
    #[test]
    fn lines_are_never_culled() {
        let road = Geom::Line(ring(&[(0.0, 0.0), (0.02, 0.0)])); // ~2.2 km segment
        let stub = Geom::Line(ring(&[(0.0, 0.0), (0.0001, 0.0001)])); // ~11 m segment of a longer road
        assert!(!footprint_below(&road, 18.0, 4.0), "a long segment stays");
        assert!(!footprint_below(&stub, 18.0, 4.0), "a short segment stays too — no road holes");
    }

    /// A disabled threshold (`<= 0`) and empty geometry never cull, so the
    /// packer stays byte-identical when the field is absent.
    #[test]
    fn disabled_and_empty_never_cull() {
        assert!(!footprint_below(&square(0.0002), 18.0, 0.0), "min_area_px 0 is off");
        assert!(!footprint_below(&square(0.0002), 18.0, -1.0), "negative is off");
        assert!(!footprint_below(&Geom::Empty, 18.0, 4.0), "empty is never a cull");
        assert!(!footprint_below(&square(0.001), 0.0, 4.0), "non-positive mpp is off");
    }

    /// Longitude foreshortening shrinks a high-latitude polygon's projected area,
    /// so a square that survives at the equator can be culled at 60°N (where a
    /// degree of longitude is half as wide).
    #[test]
    fn latitude_foreshortening_shrinks_projected_area() {
        let side = 0.0004_f64; // ~6.1 px² at the equator, ~3.1 px² at 60°N — straddles the 4 px² cut
        let equator = square(side);
        let north = Geom::Polygon {
            exterior: ring(&[(0.0, 60.0), (side, 60.0), (side, 60.0 + side), (0.0, 60.0 + side), (0.0, 60.0)]),
            interiors: vec![],
        };
        assert!(!footprint_below(&equator, 18.0, 4.0), "kept at the equator");
        assert!(footprint_below(&north, 18.0, 4.0), "same size, culled at 60°N");
    }

    // --- strip_small_holes (sub-pixel hole trim) ----------------------------

    /// The exterior of a big 0.01° face, holed by one ~1375 px² courtyard and one ~1.5 px² island at
    /// 18 m/px.
    fn holed(interiors: Vec<Vec<(f64, f64)>>) -> Geom {
        Geom::Polygon { exterior: ring(&[(0.0, 0.0), (0.01, 0.0), (0.01, 0.01), (0.0, 0.01), (0.0, 0.0)]), interiors }
    }
    fn big_hole() -> Vec<(f64, f64)> {
        ring(&[(0.002, 0.002), (0.008, 0.002), (0.008, 0.008), (0.002, 0.008), (0.002, 0.002)])
    }
    fn tiny_hole() -> Vec<(f64, f64)> {
        ring(&[(0.0011, 0.0011), (0.0013, 0.0011), (0.0013, 0.0013), (0.0011, 0.0013), (0.0011, 0.0011)])
    }

    /// A sub-pixel hole is dropped, a supra-pixel hole survives untouched, and the exterior is never
    /// modified — same 4 px² threshold that culls a standalone ~1.5 px² square in the tests above.
    #[test]
    fn strip_small_holes_drops_only_subpixel_holes() {
        let mut g = holed(vec![big_hole(), tiny_hole()]);
        assert_eq!(strip_small_holes(&mut g, 18.0, 4.0), 1, "exactly the sub-pixel hole is removed");
        match &g {
            Geom::Polygon { exterior, interiors } => {
                assert_eq!(exterior.len(), 5, "the exterior ring is untouched");
                assert_eq!(interiors.as_slice(), &[big_hole()], "the big courtyard survives, unchanged");
            }
            other => panic!("expected a polygon, got {other:?}"),
        }
    }

    /// Disabled (`min_area_px <= 0`) or a non-positive mpp trims nothing, so the packer stays
    /// byte-identical when the knob is off — the same off-contract as the footprint cull.
    #[test]
    fn strip_small_holes_disabled_is_a_noop() {
        let mut g = holed(vec![tiny_hole()]);
        assert_eq!(strip_small_holes(&mut g, 18.0, 0.0), 0, "min_area_px 0 is off");
        assert_eq!(strip_small_holes(&mut g, 0.0, 4.0), 0, "non-positive mpp is off");
        match &g {
            Geom::Polygon { interiors, .. } => assert_eq!(interiors.len(), 1, "the tiny hole is still present"),
            other => panic!("expected a polygon, got {other:?}"),
        }
    }

    /// Lines, hole-free polygons, and empties have nothing to strip; a `Multi` recurses per part.
    #[test]
    fn strip_small_holes_ignores_lines_and_hole_free() {
        assert_eq!(strip_small_holes(&mut Geom::Line(ring(&[(0.0, 0.0), (0.01, 0.0)])), 18.0, 4.0), 0);
        assert_eq!(strip_small_holes(&mut square(0.001), 18.0, 4.0), 0, "a solid polygon has no holes to trim");
        let mut multi = Geom::Multi(vec![holed(vec![tiny_hole()]), Geom::Line(ring(&[(0.0, 0.0), (0.01, 0.0)]))]);
        assert_eq!(strip_small_holes(&mut multi, 18.0, 4.0), 1, "Multi recurses: the polygon part's tiny hole goes");
    }

    /// A degree of latitude is [`M_PER_DEG`]; a degree of longitude is that times `cos(lat)`. At
    /// 48° N the two axes must therefore measure differently for the same degree span, which is
    /// the whole reason the length is not a plain euclidean distance in degrees.
    #[test]
    fn line_length_km_uses_latitude_foreshortening() {
        let north = Geom::Line(vec![(7.8, 47.99), (7.8, 48.00)]);
        assert!((line_length_km(&north) - 1.1132).abs() < 1e-3, "0.01° of latitude is 1.1132 km");
        let east = Geom::Line(vec![(7.8, 48.0), (7.81, 48.0)]);
        let expect = 1.1132 * 48.0_f64.to_radians().cos();
        assert!((line_length_km(&east) - expect).abs() < 1e-3, "0.01° of longitude is that shrunk by cos(48°)");
        // Segments accumulate, and a Multi is one road measured across its strands.
        let bent = Geom::Line(vec![(7.8, 47.99), (7.8, 48.00), (7.8, 48.01)]);
        assert!((line_length_km(&bent) - 2.0 * 1.1132).abs() < 1e-3);
        let multi = Geom::Multi(vec![north.clone(), north.clone()]);
        assert!((line_length_km(&multi) - 2.0 * 1.1132).abs() < 1e-3);
    }

    /// Areas are not lengths: a polygon has no length, so the cull can never reach one.
    #[test]
    fn line_length_km_ignores_polygons_and_empties() {
        assert_eq!(line_length_km(&square(0.01)), 0.0);
        assert_eq!(line_length_km(&Geom::Empty), 0.0);
        assert_eq!(line_length_km(&Geom::Line(vec![])), 0.0, "a degenerate line has no segments");
    }

    /// The threshold is a strict `<`, and `0.0` is off — the default, and what makes a tier
    /// without the knob pack exactly as it did before.
    #[test]
    fn line_below_thresholds_and_disabled() {
        let km = |n: f64| Geom::Line(vec![(7.8, 48.0), (7.8, 48.0 + n / 111.32)]);
        assert!(line_below(&km(0.4), 0.5), "shorter than the threshold culls");
        assert!(!line_below(&km(0.6), 0.5), "longer does not");
        assert!(!line_below(&km(0.4), 0.0), "0 km is off");
        assert!(!line_below(&km(0.4), -1.0), "so is a negative");
        assert!(!line_below(&Geom::Empty, 0.5), "an empty is not a short line");
    }

    /// The safety property the pipeline leans on: a polygon is never culled by length, and a
    /// `Multi` carrying one is safe by the same token however short its line parts are.
    #[test]
    fn line_below_never_culls_a_polygon() {
        let tiny = square(0.0001);
        assert!(!line_below(&tiny, 1000.0), "a polygon has no length but is never 'short'");
        let stub = Geom::Line(vec![(7.8, 48.0), (7.8001, 48.0)]);
        assert!(line_below(&stub, 0.5), "the bare stub culls");
        assert!(!line_below(&Geom::Multi(vec![stub.clone(), tiny]), 0.5), "but not once a polygon rides along");
        assert!(line_below(&Geom::Multi(vec![stub.clone(), Geom::Empty]), 0.5), "empties do not protect it");
        assert!(line_below(&Geom::Multi(vec![stub.clone(), stub]), 0.5), "two stubs still measure under 0.5 km");
    }
}
