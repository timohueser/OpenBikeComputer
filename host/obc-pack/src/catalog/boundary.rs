//! Region **outlines** for the `schema_version 2` catalog: an Osmosis/Geofabrik
//! `.poly` file in, a handful of simplified microdegree rings out
//! ([`OBCC_Spec.md` §11.8](../../../../specs/OBCC_Spec.md)).
//!
//! A v2 region is a cell-set selection, and a cell set draws as a staircase. Users
//! expect a border, so each named region ships the border — simplified hard, because
//! the catalog is fetched before anything else and a full-resolution Germany outline
//! is megabytes. The source is the region's own Geofabrik `.poly` (the same file that
//! defines the extract), so the outline and the coverage come from one statement of
//! what the region *is*.
//!
//! Two properties are load-bearing:
//!
//! - **Presentation only.** The outline never decides a cell set (that is stored,
//!   `OBCC_Spec.md` §11.7), never prices a selection, and is never a packer input
//!   bbox. A simplification error here must not be able to drop a cell, which is why
//!   it cannot be in the derivation path of one.
//! - **Deterministic.** Same `.poly`, same tolerance, same bytes: rings are ordered
//!   by a content-derived key rather than by GEOS output order, and every coordinate
//!   is the same `(deg * 1e6).round()` the packer uses everywhere else
//!   ([`OBCA_Spec.md` §3.2](../../../../specs/OBCA_Spec.md)).

use crate::geom::{assemble_multipolygon, collect_polygons, topology_preserve_simplify, Geom};

/// One closed ring of `[lat, lon]` integer microdegree pairs.
pub type Ring = Vec<[i32; 2]>;

/// Default simplification tolerance, in microdegrees (0.002° ≈ 150–220 m at DACH
/// latitudes). Chosen so a country outline lands in the low single-digit kilobytes —
/// the budget `OBCC_Spec.md` §11.8 sets — while still reading as that country's
/// border at every zoom the builder draws it at.
pub const DEFAULT_TOLERANCE_UDEG: i32 = 2_000;

/// A ring straight out of the `.poly` file: degrees, and whether the file marked it
/// as subtracted (`!`).
#[derive(Debug, Clone, PartialEq)]
struct PolyRing {
    /// Osmosis marks a subtracted ring with a leading `!`. Kept for the diagnostic
    /// below; the actual hole assignment is GEOS's even-odd nesting rule, which is
    /// what the packer already trusts for OSM multipolygon relations and what makes
    /// a mis-flagged file assemble correctly anyway.
    subtracted: bool,
    /// `(lon, lat)` degrees, closed.
    points: Vec<(f64, f64)>,
}

/// Parse an Osmosis polygon filter file (Geofabrik's `<region>.poly`).
///
/// ```text
/// austria                     ← file name line, ignored
/// 1                           ← section id; `!1` would mark a subtracted ring
///    9.5307E0   4.6372E1      ← lon lat, whitespace separated
///    …
/// END                         ← end of section
/// END                         ← end of file
/// ```
///
/// Strict on purpose: a `.poly` is machine-written, and a silently half-parsed
/// outline would draw a region as a wedge of itself with no error anywhere.
fn parse_poly(text: &str) -> Result<Vec<PolyRing>, String> {
    let mut rings = Vec::new();
    let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty()).enumerate();
    let (_, _name) = lines.next().ok_or_else(|| "empty .poly file".to_string())?;

    let mut current: Option<PolyRing> = None;
    let mut ended = false;
    for (n, line) in lines {
        let lineno = n + 1;
        if ended {
            return Err(format!("line {lineno}: content after the file's final END"));
        }
        match &mut current {
            // Between sections: either a section header or the file's final END.
            None => {
                if line.eq_ignore_ascii_case("END") {
                    ended = true;
                } else {
                    current = Some(PolyRing { subtracted: line.starts_with('!'), points: Vec::new() });
                }
            }
            Some(ring) => {
                if line.eq_ignore_ascii_case("END") {
                    let mut ring = current.take().expect("inside a section");
                    close_ring(&mut ring.points);
                    if ring.points.len() < 4 {
                        return Err(format!("line {lineno}: a ring needs at least 3 distinct points"));
                    }
                    rings.push(ring);
                    continue;
                }
                let mut it = line.split_whitespace();
                let (Some(x), Some(y), None) = (it.next(), it.next(), it.next()) else {
                    return Err(format!("line {lineno}: `{line}` is not a `lon lat` coordinate pair"));
                };
                let lon: f64 = x.parse().map_err(|_| format!("line {lineno}: `{x}` is not a number"))?;
                let lat: f64 = y.parse().map_err(|_| format!("line {lineno}: `{y}` is not a number"))?;
                if !lon.is_finite() || !lat.is_finite() || lon.abs() > 180.0 || lat.abs() > 90.0 {
                    return Err(format!("line {lineno}: ({lon}, {lat}) is not a geographic coordinate"));
                }
                ring.points.push((lon, lat));
            }
        }
    }
    if current.is_some() {
        return Err("unterminated ring — the last section has no END".to_string());
    }
    if !ended {
        return Err("no final END — the file is truncated".to_string());
    }
    if rings.is_empty() {
        return Err("no rings — nothing to draw".to_string());
    }
    Ok(rings)
}

/// Close a ring if the file left it open. Geofabrik writes closed rings, but the
/// format does not require it and an unclosed ring is not a linear ring to GEOS.
fn close_ring(points: &mut Vec<(f64, f64)>) {
    dedup_consecutive(points);
    if let (Some(&first), Some(&last)) = (points.first(), points.last()) {
        if first != last {
            points.push(first);
        }
    }
}

fn dedup_consecutive<T: PartialEq>(points: &mut Vec<T>) {
    points.dedup();
}

/// A region's outline: assembled, simplified, rounded to microdegrees, ordered.
///
/// `tolerance_udeg` is the GEOS `TopologyPreservingSimplifier` tolerance in
/// microdegrees — topology-preserving rather than plain Douglas–Peucker, so a
/// simplified border cannot cross itself and an island cannot swallow its neighbour.
///
/// Ring order is `[exterior, its holes…]` per polygon, polygons ordered by their
/// own minimum corner. For the single-exterior case that is exactly
/// `OBCC_Spec.md` §11.8's "first ring exterior, the rest holes"; a region that is
/// genuinely several pieces (islands, exclaves) repeats the pattern, which draws
/// correctly under both stroke-every-ring and even-odd fill.
pub fn simplified_rings(poly_text: &str, tolerance_udeg: i32) -> Result<Vec<Ring>, String> {
    if tolerance_udeg <= 0 {
        return Err(format!("boundary tolerance {tolerance_udeg} µdeg must be positive"));
    }
    let parsed = parse_poly(poly_text)?;
    let members: Vec<Vec<(f64, f64)>> = parsed.iter().map(|r| r.points.clone()).collect();

    // Even-odd assembly, the same primitive the packer uses for OSM multipolygon
    // relations: nesting decides what is a hole, so a `.poly` whose `!` flags
    // disagree with its geometry still assembles into the shape it draws as.
    let assembled = assemble_multipolygon(&members);
    if assembled.is_empty() {
        return Err(format!(
            "the {} ring(s) in this .poly do not assemble into a polygon — is the outline self-intersecting?",
            parsed.len()
        ));
    }

    let tol_deg = f64::from(tolerance_udeg) / 1e6;
    let mut polygons: Vec<Vec<Ring>> = Vec::new();
    for polygon in &assembled {
        let simplified = topology_preserve_simplify(polygon, tol_deg);
        let mut parts = Vec::new();
        collect_polygons(simplified, &mut parts);
        for part in parts {
            let Geom::Polygon { exterior, interiors } = part else { continue };
            let mut rings = Vec::new();
            if let Some(ring) = to_udeg_ring(&exterior) {
                rings.push(ring);
            } else {
                // The exterior collapsed at this tolerance: the whole piece is
                // sub-tolerance (a rock in the North Sea), so its holes are moot too.
                continue;
            }
            let mut holes: Vec<Ring> = interiors.iter().filter_map(|r| to_udeg_ring(r)).collect();
            holes.sort_by_key(ring_key);
            rings.extend(holes);
            polygons.push(rings);
        }
    }
    if polygons.is_empty() {
        return Err(format!(
            "every ring collapsed at a {tolerance_udeg} µdeg tolerance — the region is smaller than its own outline \
             tolerance"
        ));
    }

    // Content-derived order, so the output cannot depend on GEOS's traversal.
    polygons.sort_by_key(|rings| ring_key(&rings[0]));
    Ok(polygons.into_iter().flatten().collect())
}

/// `(deg * 1e6).round()`, the packer's one rounding, then drop the duplicates that
/// rounding creates and re-close. `None` if what survives is not a ring.
fn to_udeg_ring(points: &[(f64, f64)]) -> Option<Ring> {
    let mut ring: Ring = points.iter().map(|&(lon, lat)| [udeg(lat), udeg(lon)]).collect();
    dedup_consecutive(&mut ring);
    if let (Some(&first), Some(&last)) = (ring.first(), ring.last()) {
        if first != last {
            ring.push(first);
        }
    }
    // 3 distinct points + the closing repeat is the smallest thing that has an inside.
    (ring.len() >= 4).then_some(ring)
}

fn udeg(deg: f64) -> i32 {
    (deg * 1e6).round() as i32
}

/// Sort key for a ring: its minimum corner, then its extent, then its length. Every
/// component is content, so two runs over one `.poly` order rings identically.
fn ring_key(ring: &Ring) -> (i32, i32, i32, i32, usize) {
    let min_lat = ring.iter().map(|p| p[0]).min().unwrap_or(0);
    let min_lon = ring.iter().map(|p| p[1]).min().unwrap_or(0);
    let max_lat = ring.iter().map(|p| p[0]).max().unwrap_or(0);
    let max_lon = ring.iter().map(|p| p[1]).max().unwrap_or(0);
    (min_lat, min_lon, max_lat, max_lon, ring.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A square with a square hole, plus a detached island — the three shapes a
    /// country outline is made of.
    const SQUARE_WITH_HOLE_AND_ISLAND: &str = "\
region
1
   7.0E0   47.0E0
   8.0E0   47.0E0
   8.0E0   48.0E0
   7.0E0   48.0E0
   7.0E0   47.0E0
END
!2
   7.4E0   47.4E0
   7.6E0   47.4E0
   7.6E0   47.6E0
   7.4E0   47.6E0
   7.4E0   47.4E0
END
3
   9.0E0   49.0E0
   9.5E0   49.0E0
   9.5E0   49.5E0
   9.0E0   49.0E0
END
END
";

    fn ring_of(lon: (f64, f64), lat: (f64, f64)) -> String {
        format!(
            "   {}E0   {}E0\n   {}E0   {}E0\n   {}E0   {}E0\n   {}E0   {}E0\n   {}E0   {}E0\n",
            lon.0, lat.0, lon.1, lat.0, lon.1, lat.1, lon.0, lat.1, lon.0, lat.0
        )
    }

    fn simple_poly() -> String {
        format!("region\n1\n{}END\nEND\n", ring_of((7.0, 8.0), (47.0, 48.0)))
    }

    #[test]
    fn parses_the_osmosis_shape() {
        let rings = parse_poly(SQUARE_WITH_HOLE_AND_ISLAND).expect("parses");
        assert_eq!(rings.len(), 3);
        assert!(!rings[0].subtracted);
        assert!(rings[1].subtracted, "a `!` section is the file's hole marker");
        assert_eq!(rings[0].points.first(), Some(&(7.0, 47.0)));
        assert_eq!(rings[0].points.last(), Some(&(7.0, 47.0)), "rings come back closed");
    }

    #[test]
    fn an_unclosed_ring_is_closed_rather_than_rejected() {
        let text = "region\n1\n   7.0 47.0\n   8.0 47.0\n   8.0 48.0\nEND\nEND\n";
        let rings = parse_poly(text).expect("parses");
        assert_eq!(rings[0].points.len(), 4);
        assert_eq!(rings[0].points.first(), rings[0].points.last());
    }

    #[test]
    fn malformed_poly_files_fail_loudly() {
        for (bad, want) in [
            ("", "empty"),
            ("region\n", "no final END"),
            ("region\n1\n   7.0 47.0\n", "unterminated"),
            ("region\n1\n   7.0 47.0\n   8.0 47.0\nEND\nEND\n", "at least 3"),
            ("region\n1\n   7.0\nEND\nEND\n", "coordinate pair"),
            ("region\n1\n   east 47.0\nEND\nEND\n", "not a number"),
            ("region\n1\n   700.0 47.0\nEND\nEND\n", "not a geographic coordinate"),
            ("region\nEND\n1\n", "after the file's final END"),
            ("region\nEND\n", "no rings"),
        ] {
            let err = parse_poly(bad).expect_err(&format!("{bad:?} must fail"));
            assert!(err.contains(want), "{bad:?}: got `{err}`, wanted `{want}`");
        }
    }

    #[test]
    fn a_hole_stays_a_hole_and_an_island_stays_a_ring() {
        let rings = simplified_rings(SQUARE_WITH_HOLE_AND_ISLAND, 100).expect("outline");
        assert_eq!(rings.len(), 3, "exterior + hole + island: {rings:?}");
        // Rings are `[lat, lon]`, matching the OBCM header and v1's bbox.
        assert!(rings.iter().all(|r| r.first() == r.last()), "every ring is closed");
        let island = rings.iter().find(|r| r.iter().all(|p| p[0] >= 49_000_000)).expect("the island");
        assert_eq!(island.len(), 4, "a triangle is 3 points plus the closing repeat");
    }

    #[test]
    fn coordinates_are_microdegrees_lat_then_lon() {
        let rings = simplified_rings(&simple_poly(), 100).expect("outline");
        let exterior = &rings[0];
        assert!(exterior.iter().all(|p| (47_000_000..=48_000_000).contains(&p[0])), "{exterior:?}");
        assert!(exterior.iter().all(|p| (7_000_000..=8_000_000).contains(&p[1])), "{exterior:?}");
    }

    #[test]
    fn simplification_drops_points_and_keeps_the_shape() {
        // A staircase along the north edge: 20 tiny steps a 0.05° tolerance flattens.
        let mut body = String::from("   7.0E0   47.0E0\n   9.0E0   47.0E0\n");
        for k in 0..20 {
            let lon = 9.0 - f64::from(k) * 0.1;
            body.push_str(&format!("   {lon}   {}\n", 48.0 + if k % 2 == 0 { 0.001 } else { 0.0 }));
        }
        body.push_str("   7.0E0   48.0E0\n   7.0E0   47.0E0\n");
        let text = format!("region\n1\n{body}END\nEND\n");

        let tight = simplified_rings(&text, 10).expect("tight");
        let loose = simplified_rings(&text, 50_000).expect("loose");
        assert!(
            loose[0].len() < tight[0].len(),
            "a looser tolerance must drop points: {} vs {}",
            loose[0].len(),
            tight[0].len()
        );
        assert!(loose[0].len() >= 4, "but never below a drawable ring");
    }

    #[test]
    fn the_same_poly_and_tolerance_produce_the_same_rings() {
        let a = simplified_rings(SQUARE_WITH_HOLE_AND_ISLAND, 500).expect("first");
        let b = simplified_rings(SQUARE_WITH_HOLE_AND_ISLAND, 500).expect("second");
        assert_eq!(a, b, "byte-identical output is OBCC principle 4");

        // Same shape, rings written in a different order: the output order is
        // content-derived, so it must not move.
        let reordered = "\
region
3
   9.0E0   49.0E0
   9.5E0   49.0E0
   9.5E0   49.5E0
   9.0E0   49.0E0
END
1
   7.0E0   47.0E0
   8.0E0   47.0E0
   8.0E0   48.0E0
   7.0E0   48.0E0
   7.0E0   47.0E0
END
!2
   7.4E0   47.4E0
   7.6E0   47.4E0
   7.6E0   47.6E0
   7.4E0   47.6E0
   7.4E0   47.4E0
END
END
";
        assert_eq!(simplified_rings(reordered, 500).expect("reordered"), a, "ring order must not follow the file");
    }

    #[test]
    fn simplification_cannot_erase_the_region() {
        // Four degrees of tolerance against a 1° square: `TopologyPreservingSimplifier`
        // still returns a drawable ring, which is exactly why it is used here rather
        // than plain Douglas–Peucker — an outline that vanished would render as "no
        // such region" instead of as a coarse border. (The "every ring collapsed"
        // guard remains for a GEOS failure, which is a different thing.)
        let rings = simplified_rings(&simple_poly(), 4_000_000).expect("the outline survives");
        assert_eq!(rings.len(), 1);
        assert!(rings[0].len() >= 4, "{:?}", rings[0]);
        assert!(simplified_rings(&simple_poly(), 0).is_err(), "a zero tolerance is not a tolerance");
        assert!(simplified_rings(&simple_poly(), -1).is_err());
    }

    #[test]
    fn a_country_scale_outline_is_a_few_kilobytes() {
        // A 400-point circle stands in for a border; at the default tolerance it must
        // land inside §11.8's few-kilobyte budget.
        let mut body = String::new();
        for k in 0..400 {
            let a = f64::from(k) / 400.0 * std::f64::consts::TAU;
            body.push_str(&format!("   {:.6}   {:.6}\n", 8.0 + 2.0 * a.cos(), 47.0 + 2.0 * a.sin()));
        }
        let text = format!("region\n1\n{body}END\nEND\n");
        let rings = simplified_rings(&text, DEFAULT_TOLERANCE_UDEG).expect("outline");
        let bytes = serde_json::to_string(&rings).expect("json").len();
        assert!(bytes < 4 * 1024, "{bytes} bytes for one region outline is not a few KB");
    }
}
