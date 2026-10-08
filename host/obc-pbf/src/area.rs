//! Relation linework and polygon rings, without rendering classes.

use geos::{CoordSeq, Geom as _, Geometry, GeometryTypes};

#[derive(Debug, Clone)]
pub struct Polygon {
    pub exterior: Vec<(f64, f64)>,
    pub interiors: Vec<Vec<(f64, f64)>>,
}

impl Polygon {
    pub(crate) fn geometry(&self) -> Result<Geometry, geos::Error> {
        let exterior = Geometry::create_linear_ring(ring_to_coordseq(&self.exterior))?;
        let interiors = self
            .interiors
            .iter()
            .map(|ring| Geometry::create_linear_ring(ring_to_coordseq(ring)))
            .collect::<Result<Vec<_>, _>>()?;
        Geometry::create_polygon(exterior, interiors)
    }
}

/// Simplify a source polygon without changing its topology. A failed result has no polygons.
pub fn topology_preserve_simplify(polygon: &Polygon, tolerance: f64) -> Vec<Polygon> {
    let mut out = Vec::new();
    if let Ok(geometry) = polygon.geometry().and_then(|g| g.topology_preserve_simplify(tolerance)) {
        collect_polygons(&geometry, &mut out);
    }
    out
}

pub fn ring_to_coordseq(coords: &[(f64, f64)]) -> CoordSeq {
    let buf: Vec<[f64; 2]> = coords.iter().map(|&(x, y)| [x, y]).collect();
    CoordSeq::new_from_vec(&buf).expect("coordseq")
}

/// Read a LineString or LinearRing's coordinate sequence into owned `(x, y)` pairs. Works on the
/// borrowed `ConstGeometry` that ring accessors return.
pub fn read_coords<G: geos::Geom>(g: &G) -> Vec<(f64, f64)> {
    let cs = g.get_coord_seq().expect("coord seq");
    let n = cs.size().expect("size");
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        out.push((cs.get_x(i).expect("x"), cs.get_y(i).expect("y")));
    }
    out
}

/// Whether a ring assembles into a valid polygon. Matches osmium's assembler: a self-intersecting
/// ring, a degenerate ring, or any construction error is rejected.
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

/// Assemble a multipolygon or boundary relation's member ways into polygons-with-holes.
///
/// `members` is each member way's resolved coordinate list. GEOS `build_area`, fed them as a
/// `MultiLineString`, stitches fragments sharing endpoint nodes into closed rings and applies the
/// even-odd nesting rule, so member roles are not trusted. Each result is gated on
/// [`polygon_is_valid`]; un-assemblable or invalid geometry returns empty, as osmium also drops
/// broken relations.
///
/// Two-tier: `build_area` on the raw linework, then a retry after noding — splitting members that
/// cross or self-touch mid-segment — so only messy relations pay the extra cost.
pub fn assemble_multipolygon(members: &[Vec<(f64, f64)>]) -> Vec<Polygon> {
    let polys = build_area_from_members(members, false);
    if !polys.is_empty() {
        return polys;
    }
    build_area_from_members(members, true)
}

/// Build polygons from member-way linework via GEOS `build_area`. `node_first`
/// planar-nodes the linework first (repair path for crossing/self-touching members).
fn build_area_from_members(members: &[Vec<(f64, f64)>], node_first: bool) -> Vec<Polygon> {
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
    collect_polygons(&area, &mut polys);
    // Keep only rings osmium would accept (same guard as the closed-way path).
    polys.retain(|g| polygon_is_valid(&g.exterior, &g.interiors));
    polys
}

pub(crate) fn collect_polygons<G: geos::Geom>(g: &G, out: &mut Vec<Polygon>) {
    if g.is_empty().unwrap_or(true) {
        return;
    }
    match g.geometry_type() {
        Ok(GeometryTypes::Polygon) => {
            let exterior = read_coords(&g.get_exterior_ring().expect("ext"));
            let holes = g.get_num_interior_rings().expect("nholes");
            let interiors = (0..holes).map(|i| read_coords(&g.get_interior_ring_n(i).expect("hole"))).collect();
            out.push(Polygon { exterior, interiors });
        }
        Ok(GeometryTypes::MultiLineString | GeometryTypes::MultiPolygon | GeometryTypes::GeometryCollection) => {
            for i in 0..g.get_num_geometries().expect("n geoms") {
                collect_polygons(&g.get_geometry_n(i).expect("geom n"), out);
            }
        }
        _ => {}
    }
}
