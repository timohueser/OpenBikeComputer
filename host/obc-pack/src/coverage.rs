//! Geometric coverage predicates for selecting complete source extracts.

use geos::{Geom as _, Geometry};
use obc_pbf::area::ring_to_coordseq;

use crate::geom::{geom_from_geos, Geom};

fn union(polys: &[&Geom]) -> Result<Geometry, String> {
    let polygons = polys
        .iter()
        .map(|geom| {
            let Geom::Polygon { exterior, interiors } = geom else { return Err("coverage is not a polygon".into()) };
            let exterior = Geometry::create_linear_ring(ring_to_coordseq(exterior)).map_err(|e| e.to_string())?;
            let interiors = interiors
                .iter()
                .map(|ring| Geometry::create_linear_ring(ring_to_coordseq(ring)))
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| e.to_string())?;
            Geometry::create_polygon(exterior, interiors).map_err(|e| e.to_string())
        })
        .collect::<Result<Vec<_>, String>>()?;
    Geometry::create_multipolygon(polygons).and_then(|g| g.unary_union()).map_err(|e| e.to_string())
}

/// Whether the source union contains the entire requested polygon, including its boundary.
pub fn covers_polygon(polys: &[&Geom], polygon: &Geom) -> Result<bool, String> {
    if polys.is_empty() {
        return Ok(false);
    }
    union(polys)?.covers(&union(&[polygon])?).map_err(|e| e.to_string())
}

/// Common ground. Empty ground is valid; geometry errors are not proof of coverage.
pub fn intersect_polygons(a: &[&Geom], b: &[&Geom]) -> Result<Vec<Geom>, String> {
    fn polygons(geom: Geom, out: &mut Vec<Geom>) {
        match geom {
            Geom::Polygon { .. } => out.push(geom),
            Geom::Multi(parts) => parts.into_iter().for_each(|part| polygons(part, out)),
            Geom::Line(_) | Geom::Empty => {}
        }
    }
    let intersection = union(a)?.intersection(&union(b)?).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    polygons(geom_from_geos(&intersection), &mut out);
    Ok(out)
}
