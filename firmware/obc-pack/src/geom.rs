//! Geometry for the packer: an owned geometry type, its bounds, and the GEOS
//! bridge used for boundary clipping.
//!
//! Coordinates are f64 lon/lat degrees; the quadtree clips in degree space, then
//! the serializer rounds to microdegrees.

use geos::{CoordSeq, Geom as _, Geometry, GeometryTypes};

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

/// Point count for chunk-size accounting (`12 + pt_count*4`): a polygon counts
/// exterior + every interior ring.
pub fn pt_count(g: &Geom) -> usize {
    match g {
        Geom::Line(c) => c.len(),
        Geom::Polygon { exterior, interiors } => exterior.len() + interiors.iter().map(Vec::len).sum::<usize>(),
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

fn from_geos<G: Geom_>(g: &G) -> Geom {
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
trait Geom_: geos::Geom {}
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

/// Clip `geom` to the node box (integer microdegrees → degrees) via GEOS
/// `intersection`.
pub fn clip_to_box(geom: &Geom, bbox: (i64, i64, i64, i64)) -> Geom {
    let (minx, miny, maxx, maxy) = (bbox.0 as f64 / 1e6, bbox.1 as f64 / 1e6, bbox.2 as f64 / 1e6, bbox.3 as f64 / 1e6);
    let box_geom = box_polygon((minx, miny, maxx, maxy)).expect("box polygon");
    let clipped = to_geos(geom).intersection(&box_geom).expect("intersection");
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
}
