//! Geometry for the quadtree port: a small owned geometry type, its bounds, and
//! the GEOS bridge used for boundary clipping. Mirrors the shapely geometries
//! `quadtree.py` passes around — simple types (`LineString`/`Polygon`) plus the
//! multi-containers that `intersection` can return and `_flatten_and_process`
//! splits apart.
//!
//! Coordinates are f64 lon/lat (degrees), exactly as in the oracle: the quadtree
//! does its overlap/containment tests and its clip in degree space, then the
//! serializer rounds to microdegrees.

use geos::{CoordSeq, Geom as _, Geometry, GeometryTypes};

use crate::serialize::{Feature, Kind};

/// Axis-aligned bounds in degrees: (min_lon, min_lat, max_lon, max_lat).
pub type Bounds = (f64, f64, f64, f64);

/// An in-flight geometry. Input features are `Line`/`Polygon`; `Multi` only
/// arises from a clip result and is flattened away before storage.
#[derive(Debug, Clone)]
pub enum Geom {
    Line(Vec<(f64, f64)>),
    Polygon { exterior: Vec<(f64, f64)>, interiors: Vec<Vec<(f64, f64)>> },
    Multi(Vec<Geom>),
    Empty,
}

impl Geom {
    /// Bounds over every vertex. Mirrors shapely's `geom.bounds` (min/max of
    /// coordinates). Panics on `Empty` — callers guard with `is_empty` first,
    /// exactly as the oracle does.
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

/// A simple geometry stored in a leaf, paired with the [`Feature`] the serializer
/// consumes. Only `Line`/`Polygon` reach here (post-flatten).
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

/// Point count used for chunk-size accounting (`12 + pt_count*4`): exterior +
/// every interior ring for a polygon, else the vertex count. Mirrors
/// `quadtree.py::_process_clipped`.
pub fn pt_count(g: &Geom) -> usize {
    match g {
        Geom::Line(c) => c.len(),
        Geom::Polygon { exterior, interiors } => {
            exterior.len() + interiors.iter().map(Vec::len).sum::<usize>()
        }
        _ => 0,
    }
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

fn read_ring(g: &geos::ConstGeometry) -> Vec<(f64, f64)> {
    let cs = g.get_coord_seq().expect("coord seq");
    let n = cs.size().expect("size");
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        out.push((cs.get_x(i).expect("x"), cs.get_y(i).expect("y")));
    }
    out
}

fn from_geos<G: Geom_>(g: &G) -> Geom {
    if g.is_empty().unwrap_or(true) {
        return Geom::Empty;
    }
    match g.geometry_type() {
        Ok(GeometryTypes::LineString) | Ok(GeometryTypes::LinearRing) => {
            let cs = g.get_coord_seq().expect("coord seq");
            let n = cs.size().expect("size");
            let mut out = Vec::with_capacity(n);
            for i in 0..n {
                out.push((cs.get_x(i).expect("x"), cs.get_y(i).expect("y")));
            }
            Geom::Line(out)
        }
        Ok(GeometryTypes::Polygon) => {
            let ext = read_ring(&g.get_exterior_ring().expect("ext"));
            let nholes = g.get_num_interior_rings().expect("nholes");
            let interiors = (0..nholes)
                .map(|i| read_ring(&g.get_interior_ring_n(i).expect("hole")))
                .collect();
            Geom::Polygon { exterior: ext, interiors }
        }
        Ok(GeometryTypes::MultiLineString)
        | Ok(GeometryTypes::MultiPolygon)
        | Ok(GeometryTypes::GeometryCollection) => {
            let n = g.get_num_geometries().expect("n geoms");
            let parts = (0..n)
                .map(|i| from_geos(&g.get_geometry_n(i).expect("geom n")))
                .collect();
            Geom::Multi(parts)
        }
        // Points (incl. inside a GeometryCollection) carry no renderable line/area
        // — the oracle drops them.
        _ => Geom::Empty,
    }
}

/// Trait alias so `from_geos` accepts both owned `Geometry` and the borrowed
/// `ConstGeometry` returned by ring/sub-geometry accessors.
trait Geom_: geos::Geom {}
impl Geom_ for Geometry {}
impl Geom_ for geos::ConstGeometry<'_> {}

/// Convert an owned GEOS [`Geometry`] back into a [`Geom`] (the public entry to
/// the generic `from_geos`). Used by [`crate::land`] to read a clipped land
/// polygon out of GEOS before reprojecting it.
pub(crate) fn geom_from_geos(g: &Geometry) -> Geom {
    from_geos(g)
}

/// Topology-preserving simplify — the algorithm shapely's `geom.simplify(tol)`
/// uses by default (`preserve_topology=True` ⇒ GEOS `TopologyPreservingSimplifier`),
/// **not** plain Douglas–Peucker (the geos crate's `simplify`). `tol` is in
/// degrees (`simplify_m / 111320.0`). An empty/failed result becomes
/// [`Geom::Empty`], which the quadtree drops — matching `pack.py`'s
/// `if geom.is_empty: continue`.
pub fn topology_preserve_simplify(geom: &Geom, tol: f64) -> Geom {
    match to_geos(geom).topology_preserve_simplify(tol) {
        Ok(s) => from_geos(&s),
        Err(_) => Geom::Empty,
    }
}

/// Whether a closed-way ring assembles into a **valid** polygon. Mirrors osmium's
/// area assembler, which rejects a self-intersecting closed way (it yields zero
/// rings, so `ingest.py::area()` emits nothing). We approximate that rejection
/// with GEOS `is_valid`; a degenerate ring (too few points) or any construction
/// error also counts as invalid → skip. Stage-3 closed-way polygons have no
/// holes, but `interiors` is accepted for symmetry with [`Geom::Polygon`].
pub fn polygon_is_valid(exterior: &[(f64, f64)], interiors: &[Vec<(f64, f64)>]) -> bool {
    // A linear ring needs ≥4 positions (≥3 distinct + closing); fewer can't form
    // a polygon and GEOS would error.
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
/// dropping anything non-polygonal. Used to unpack a `build_area` result (a
/// `Polygon`, `MultiPolygon`, or degenerate `GeometryCollection`) and, in
/// [`crate::land`], a clipped-land intersection result.
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

/// Assemble a multipolygon/boundary relation's member-way geometries into
/// polygons-with-holes — the Stage-4 counterpart of osmium's `AreaManager`.
///
/// `members` is each member way's coordinate list (already resolved against the
/// node store). The job mirrors osmium's `Assembler`: stitch the (often open)
/// member-way fragments into closed rings, then apply the **even-odd nesting
/// rule** (a ring nested at odd depth is a hole of the outer that contains it —
/// landmine §3.1; roles are *not* trusted). GEOS `build_area` does exactly this:
/// its own doc example turns `GEOMETRYCOLLECTION(outer, inner)` into a
/// polygon-with-hole, and disjoint outers into a `MultiPolygon`. We feed it the
/// members as a `MultiLineString`; `build_area` extracts + polygonizes the
/// linework, so fragments sharing endpoint nodes are joined for free.
///
/// Returns one [`Geom::Polygon`] per assembled outer ring (with its directly
/// nested holes). Returns empty on un-assemblable or invalid geometry — osmium
/// silently drops broken relations (landmine §3.6), and the oracle then emits
/// nothing, so an empty result is the parity-correct outcome. Each polygon is run
/// through [`polygon_is_valid`], matching the closed-way path.
///
/// Two-tier: try `build_area` on the raw linework (the clean common case), and if
/// that yields nothing, retry after **noding** the linework (`node`) — that splits
/// members that cross or self-touch mid-segment so polygonize can find the faces,
/// which is the repair osmium's assembler does for the handful of messy relations
/// real extracts contain. Only those pay the extra cost; clean relations take the
/// fast path unchanged.
pub fn assemble_multipolygon(members: &[Vec<(f64, f64)>]) -> Vec<Geom> {
    let polys = build_area_from_members(members, false);
    if !polys.is_empty() {
        return polys;
    }
    build_area_from_members(members, true)
}

/// Build polygons from member-way linework via GEOS `build_area`. `node_first`
/// planar-nodes the linework before assembling (the repair path for crossing /
/// self-touching members).
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

/// Clip `geom` to the node box (integer microdegrees → degrees), via the SAME
/// `intersection` shapely uses (not `clip_by_rect`). The clip box ring matches
/// shapely's `box(minx,miny,maxx,maxy)` order to minimise vertex-order drift.
pub fn clip_to_box(geom: &Geom, bbox: (i64, i64, i64, i64)) -> Geom {
    let (minx, miny, maxx, maxy) =
        (bbox.0 as f64 / 1e6, bbox.1 as f64 / 1e6, bbox.2 as f64 / 1e6, bbox.3 as f64 / 1e6);
    // shapely box() ccw ring: (maxx,miny),(maxx,maxy),(minx,maxy),(minx,miny), closed.
    let ring = [
        (maxx, miny),
        (maxx, maxy),
        (minx, maxy),
        (minx, miny),
        (maxx, miny),
    ];
    let box_geom = Geometry::create_polygon(
        Geometry::create_linear_ring(ring_to_coordseq(&ring)).expect("box ring"),
        vec![],
    )
    .expect("box polygon");
    let clipped = to_geos(geom).intersection(&box_geom).expect("intersection");
    from_geos(&clipped)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// R1/R2 from `tiny.osm` (lon,lat closed rings). The probe the handover §7.2
    /// mandates *before* wiring: confirm GEOS `build_area` gives R1 → one polygon
    /// with one hole (lake + island, even-odd rule), R2 → two disjoint polygons
    /// (one relation → many outers), so the assembler choice is empirically sound.
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

    /// Order/role independence: feeding the inner ring first must still yield the
    /// lake-with-hole (build_area classifies by geometry, not member order/role).
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
}
