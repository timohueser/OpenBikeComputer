//! Full-polygon coverage predicates, without rendering geometry.

use crate::area::{collect_polygons, Polygon};
use geos::{Geom as _, Geometry};

fn union(polys: &[&Polygon]) -> Result<Geometry, String> {
    let polygons =
        polys.iter().map(|polygon| polygon.geometry()).collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())?;
    Geometry::create_multipolygon(polygons).and_then(|g| g.unary_union()).map_err(|e| e.to_string())
}

/// A true union, including overlaps without shared vertices. Empty or invalid ground has no result.
pub fn union_all(polys: &[&Polygon]) -> Option<Vec<Polygon>> {
    if polys.is_empty() || polys.iter().any(|p| p.exterior.len() < 4 || p.interiors.iter().any(|r| r.len() < 4)) {
        return None;
    }
    let mut out = Vec::new();
    collect_polygons(&union(polys).ok()?, &mut out);
    (!out.is_empty()).then_some(out)
}

/// Whether the source union contains the entire requested polygon, including its boundary.
pub fn covers_polygon(polys: &[&Polygon], polygon: &Polygon) -> Result<bool, String> {
    if polys.is_empty() {
        return Ok(false);
    }
    union(polys)?.covers(&union(&[polygon])?).map_err(|e| e.to_string())
}

/// Common ground. Empty ground is valid; geometry errors are not proof of coverage.
pub fn intersect_polygons(a: &[&Polygon], b: &[&Polygon]) -> Result<Vec<Polygon>, String> {
    let intersection = union(a)?.intersection(&union(b)?).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    collect_polygons(&intersection, &mut out);
    Ok(out)
}
