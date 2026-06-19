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

fn ring_to_coordseq(coords: &[(f64, f64)]) -> CoordSeq {
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
