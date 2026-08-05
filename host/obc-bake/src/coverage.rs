//! What ground a source extract actually covers, and which grid cells that ground
//! touches — the geometry half of the cell bake.
//!
//! Two questions decide almost everything about a scoped cell bake, and both are
//! answered here rather than guessed:
//!
//! - **Which cells does a named region select?** Per band, exactly those whose
//!   square intersects the region's coverage polygon
//!   ([`OBCA_Spec.md` §1.2](../../../specs/OBCA_Spec.md)). Not its bounding box: a
//!   box around Germany reaches into four other countries, and every cell out there
//!   would be baked, published, and empty.
//! - **Is a baked cell canonical or `partial`?** A cell is canonical iff its whole
//!   square lies inside the coverage of the sources it was baked from (§3.7). Again
//!   polygon, not box — the box test would call a cell on the Czech border canonical
//!   because Germany's bbox contains it, which is precisely the lie D3 exists to
//!   prevent.
//!
//! The coverage geometry is the region's Geofabrik `.poly`, read at **full
//! resolution** through [`obc_pack::catalog::boundary::poly_rings`]. The catalog's
//! drawable outline comes from the same file simplified hard, and `OBCC_Spec.md`
//! §7 forbids *that* one from deciding a cell set — a simplification error must
//! not be able to drop an edge cell. Same source file, two readings, and only the
//! unsimplified one is load-bearing.
//!
//! # Why the coverage test is conservative in one direction only
//!
//! A Geofabrik extract carries a **complete-ways overhang** past its polygon (~1–3
//! km, measured in the epic's S0 spike), so the ground it really covers is slightly
//! larger than the polygon says. Testing against the polygon alone therefore marks a
//! handful of border cells `partial` that a generous test would call canonical. That
//! is the safe error: `partial` under-claims coverage and the builder warns; the
//! opposite would publish a cell with a missing sliver as canonical.
//!
//! # Determinism
//!
//! Every predicate here is integer arithmetic on microdegrees — ray casting, segment
//! crossings, cell walks — so two runs agree exactly. The one float step is the GEOS
//! union of several sources' polygons ([`Coverage::union`]), which runs on sorted
//! input and is rounded to microdegrees before any decision is taken.

use std::collections::BTreeSet;

use obc_pack::catalog::boundary::poly_rings;
use obc_pack::geom::{assemble_multipolygon, union_all, Geom};
use obc_pack::grid::{axis_cells, segment_crossing, Axis, CellId, UBox, GRID_ORIGIN};

/// A closed ring in microdegrees, `(lat, lon)`.
type URing = Vec<(i64, i64)>;

/// The ground one or more source extracts cover.
#[derive(Clone, Debug)]
pub struct Coverage {
    /// Degrees, `(lon, lat)` — the packer's own order, kept so [`Coverage::union`]
    /// can hand them straight to GEOS.
    polys: Vec<Geom>,
    /// The same rings in integer microdegrees, `(lat, lon)`, closed. Every decision
    /// below is taken on these.
    rings: Vec<URing>,
    bbox: UBox,
}

impl Coverage {
    /// Read an Osmosis/Geofabrik `.poly` at full resolution.
    ///
    /// The rings are assembled with the packer's even-odd multipolygon rule — the
    /// same one it uses for OSM relations — so a `.poly` whose `!` hole markers
    /// disagree with its own nesting still yields the shape it draws as.
    pub fn parse_poly(text: &str) -> Result<Self, String> {
        let members = poly_rings(text)?;
        let polys = assemble_multipolygon(&members);
        if polys.is_empty() {
            return Err("the .poly's rings do not assemble into a polygon".into());
        }
        Ok(Self::from_polys(polys))
    }

    /// The combined coverage of several sources.
    ///
    /// A **true** union (`geom::union_all`), not a concatenation: two extracts
    /// overlap along their shared border, and an even-odd reading of overlapping
    /// rings would count the overlap as a *hole* — i.e. would report the one strip
    /// of ground co-baking exists to complete as uncovered. Returns `None` if GEOS
    /// cannot union them, which the caller must treat as "nothing is canonical".
    pub fn union(parts: &[&Coverage]) -> Option<Self> {
        let polys: Vec<&Geom> = parts.iter().flat_map(|c| c.polys.iter()).collect();
        match polys.len() {
            0 => None,
            1 => Some(parts[0].clone()),
            _ => union_all(&polys).map(Self::from_polys),
        }
    }

    fn from_polys(polys: Vec<Geom>) -> Self {
        let mut rings: Vec<URing> = Vec::new();
        for poly in &polys {
            collect_rings(poly, &mut rings);
        }
        let mut bbox = (i64::MAX, i64::MAX, i64::MIN, i64::MIN);
        for ring in &rings {
            for &(lat, lon) in ring {
                bbox.0 = bbox.0.min(lon);
                bbox.1 = bbox.1.min(lat);
                bbox.2 = bbox.2.max(lon);
                bbox.3 = bbox.3.max(lat);
            }
        }
        Coverage { polys, rings, bbox }
    }

    /// The ground this coverage encloses, km².
    ///
    /// The denominator of every density figure the bakery reports, so it is the
    /// **spherical** polygon area rather than a shoelace scaled by one cosine:
    /// Switzerland spans two degrees of latitude and a single-cosine approximation is
    /// already 4 % out over that, which is the same order as the per-cell overhead
    /// `OBCA_Spec.md` §1.5 asks a producer to budget for. Holes subtract.
    pub fn area_km2(&self) -> f64 {
        fn area_of(geom: &Geom) -> f64 {
            match geom {
                Geom::Polygon { exterior, interiors } => {
                    ring_area_km2(exterior).abs() - interiors.iter().map(|r| ring_area_km2(r).abs()).sum::<f64>()
                }
                Geom::Multi(parts) => parts.iter().map(area_of).sum(),
                Geom::Line(_) | Geom::Empty => 0.0,
            }
        }
        self.polys.iter().map(area_of).sum::<f64>().max(0.0)
    }

    /// Whether `(lat, lon)` is inside the coverage — even-odd ray casting, exact in
    /// `i128`, no float and no epsilon.
    pub fn contains(&self, lat: i64, lon: i64) -> bool {
        let mut inside = false;
        for ring in &self.rings {
            for edge in ring.windows(2) {
                let (a, b) = (edge[0], edge[1]);
                // A half-open latitude straddle, so a vertex exactly at `lat` is
                // counted by exactly one of its two edges.
                if (a.0 > lat) == (b.0 > lat) {
                    continue;
                }
                // The ray runs east, so the crossing counts when its longitude is
                // strictly greater than `lon`. Compare without dividing:
                //   lon < a.lon + (lat − a.lat)·(b.lon − a.lon) / (b.lat − a.lat)
                let dlat = i128::from(b.0 - a.0);
                let lhs = i128::from(lon - a.1) * dlat;
                let rhs = i128::from(lat - a.0) * i128::from(b.1 - a.1);
                let crosses = if dlat > 0 { lhs < rhs } else { lhs > rhs };
                if crosses {
                    inside = !inside;
                }
            }
        }
        inside
    }

    /// Every cell of size `2^log2` whose square the coverage's **boundary** passes
    /// through, ascending.
    ///
    /// This is the edge set of the coverage, and it does double duty: a cell in it is
    /// partly in and partly out (so it is selected, and it is not canonical from this
    /// source alone), and a cell *not* in it is wholly one or the other, which one
    /// decided by a single point test.
    pub fn boundary_cells(&self, log2: u32) -> BTreeSet<CellId> {
        let mut out = BTreeSet::new();
        for ring in &self.rings {
            for edge in ring.windows(2) {
                segment_cells(edge[0], edge[1], log2, &mut out);
            }
        }
        out
    }

    /// Every cell of size `2^log2` whose square intersects the coverage
    /// (`OBCA_Spec.md` §1.2's coverage rule), ascending.
    ///
    /// Exactly the boundary cells plus the cells whose square is wholly inside — and
    /// "wholly inside" needs only a centre test, because a cell the boundary misses
    /// cannot be partly in and partly out.
    pub fn cells(&self, log2: u32) -> BTreeSet<CellId> {
        let mut out = self.boundary_cells(log2);
        let s = 1i64 << log2;
        let n = axis_cells(log2);
        let index = |v: i64| (v - GRID_ORIGIN).div_euclid(s).clamp(0, n - 1);
        for i in index(self.bbox.1)..=index(self.bbox.3) {
            for j in index(self.bbox.0)..=index(self.bbox.2) {
                let Ok(cell) = CellId::new(log2, i, j) else { continue };
                if out.contains(&cell) {
                    continue;
                }
                let (min_lon, min_lat, _, _) = cell.square();
                if self.contains(min_lat + s / 2, min_lon + s / 2) {
                    out.insert(cell);
                }
            }
        }
        out
    }

    /// Whether the coverage contains a cell's **whole** square — the canonical /
    /// `partial` decision of `OBCA_Spec.md` §3.7.
    ///
    /// `boundary` is [`Coverage::boundary_cells`] for the cell's size, passed in
    /// because a bake asks this of thousands of cells and the edge set is one walk.
    pub fn covers(&self, cell: CellId, boundary: &BTreeSet<CellId>) -> bool {
        if boundary.contains(&cell) {
            return false;
        }
        let (min_lon, min_lat, _, _) = cell.square();
        let half = cell.size() / 2;
        self.contains(min_lat + half, min_lon + half)
    }
}

/// Flatten a polygon's exterior and interiors into closed microdegree rings.
///
/// A hole is a ring like any other here: even-odd ray casting counts it, so the
/// inside of a hole comes out *outside* the coverage, which is what a hole means.
fn collect_rings(geom: &Geom, out: &mut Vec<URing>) {
    match geom {
        Geom::Polygon { exterior, interiors } => {
            for ring in std::iter::once(exterior).chain(interiors) {
                if let Some(closed) = to_udeg_ring(ring) {
                    out.push(closed);
                }
            }
        }
        Geom::Multi(parts) => {
            for p in parts {
                collect_rings(p, out);
            }
        }
        // A coverage polygon that GEOS handed back as a line has no inside.
        Geom::Line(_) | Geom::Empty => {}
    }
}

/// `(lon, lat)` degrees → a closed `(lat, lon)` microdegree ring, with the duplicate
/// points rounding creates removed. `None` if what survives has no inside.
fn to_udeg_ring(points: &[(f64, f64)]) -> Option<URing> {
    let udeg = |v: f64| (v * 1e6).round() as i64;
    let mut ring: URing = points.iter().map(|&(lon, lat)| (udeg(lat), udeg(lon))).collect();
    ring.dedup();
    if let (Some(&first), Some(&last)) = (ring.first(), ring.last()) {
        if first != last {
            ring.push(first);
        }
    }
    (ring.len() >= 4).then_some(ring)
}

/// Every cell of size `2^log2` the segment `a`–`b` (µdeg `(lat, lon)`) passes
/// through.
///
/// The segment is split at every grid line it crosses — the same exact `i128`
/// interpolation the cutter mints boundary junctions with — and each piece is
/// attributed by its midpoint, which is interior to that piece and therefore in
/// exactly one cell. The endpoints' own cells are added too, so a segment that only
/// grazes a corner is still counted.
fn segment_cells(a: (i64, i64), b: (i64, i64), log2: u32, out: &mut BTreeSet<CellId>) {
    let push = |out: &mut BTreeSet<CellId>, lat: i64, lon: i64| {
        let c = CellId::containing(log2, lat, lon);
        if let Ok(c) = CellId::new(log2, c.i, c.j) {
            out.insert(c);
        }
    };
    push(out, a.0, a.1);
    push(out, b.0, b.1);

    let mut cuts: Vec<(i64, i64)> = Vec::new();
    for c in lines_strictly_between(a.0, b.0, log2) {
        if let Some(p) = segment_crossing(a, b, Axis::Lat, c) {
            cuts.push(p);
        }
    }
    for c in lines_strictly_between(a.1, b.1, log2) {
        if let Some(p) = segment_crossing(a, b, Axis::Lon, c) {
            cuts.push(p);
        }
    }
    if cuts.is_empty() {
        return;
    }
    // Order along the segment. A straight segment is monotone on both axes, so the
    // axis it travels furthest on is a total order for the points on it.
    let (dlat, dlon) = (b.0 - a.0, b.1 - a.1);
    let key = |p: &(i64, i64)| if dlat.abs() >= dlon.abs() { dlat.signum() * p.0 } else { dlon.signum() * p.1 };
    cuts.sort_by_key(key);
    cuts.dedup();

    let mut chain = Vec::with_capacity(cuts.len() + 2);
    chain.push(a);
    chain.extend(cuts);
    chain.push(b);
    for w in chain.windows(2) {
        // Midpoints of the halves as well as of the whole piece: a piece can be one
        // µdeg long, where the single midpoint rounds onto an endpoint.
        push(out, (w[0].0 + w[1].0).div_euclid(2), (w[0].1 + w[1].1).div_euclid(2));
    }
}

/// The signed spherical area of one closed ring of `(lon, lat)` degrees, km².
///
/// `A = R²/2 · Σ (λ₁ − λ₀)·(sin φ₀ + sin φ₁)` — the standard spherical-excess form,
/// exact on a sphere for a ring of great-circle-ish edges at these scales.
fn ring_area_km2(points: &[(f64, f64)]) -> f64 {
    /// Mean Earth radius, km (IUGG R₁).
    const R: f64 = 6371.0088;
    let mut sum = 0.0;
    for w in points.windows(2) {
        let ((lon0, lat0), (lon1, lat1)) = (w[0], w[1]);
        sum += (lon1 - lon0).to_radians() * (lat0.to_radians().sin() + lat1.to_radians().sin());
    }
    R * R / 2.0 * sum
}

/// Every grid line of size `2^log2` strictly between `v0` and `v1`, ascending.
fn lines_strictly_between(v0: i64, v1: i64, log2: u32) -> impl Iterator<Item = i64> {
    let s = 1i64 << log2;
    let (lo, hi) = (v0.min(v1), v0.max(v1));
    let first = GRID_ORIGIN + ((lo - GRID_ORIGIN).div_euclid(s) + 1) * s;
    std::iter::successors(Some(first), move |v| Some(v + s)).take_while(move |v| *v < hi)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An axis-aligned `.poly` in degrees.
    fn box_poly(w: f64, s: f64, e: f64, n: f64) -> String {
        format!("region\n1\n   {w} {s}\n   {e} {s}\n   {e} {n}\n   {w} {n}\n   {w} {s}\nEND\nEND\n")
    }

    const LOG2: u32 = 18;
    const S: i64 = 1 << LOG2;

    /// A cell's square in degrees, so a test can build a `.poly` that lines up with
    /// the grid exactly.
    fn square_deg(cell: CellId) -> (f64, f64, f64, f64) {
        let (min_lon, min_lat, max_lon, max_lat) = cell.square();
        (min_lon as f64 / 1e6, min_lat as f64 / 1e6, max_lon as f64 / 1e6, max_lat as f64 / 1e6)
    }

    #[test]
    fn a_polygon_selects_the_cells_it_touches_and_no_others() {
        // A box strictly inside one cell touches exactly that cell.
        let cell = CellId::parse("18/1204/1052").unwrap();
        let (w, s, e, n) = square_deg(cell);
        let inset = 0.01;
        let cov = Coverage::parse_poly(&box_poly(w + inset, s + inset, e - inset, n - inset)).unwrap();
        assert_eq!(cov.cells(LOG2), BTreeSet::from([cell]));

        // Widened past the shared eastern edge, it also takes the neighbour.
        let east = CellId::new(LOG2, cell.i, cell.j + 1).unwrap();
        let cov = Coverage::parse_poly(&box_poly(w + inset, s + inset, e + inset, n - inset)).unwrap();
        assert_eq!(cov.cells(LOG2), BTreeSet::from([cell, east]));
    }

    #[test]
    fn a_polygon_larger_than_a_cell_fills_its_interior() {
        // Three by three cells, so the middle one is interior — reachable only by the
        // centre test, since no ring segment passes through it.
        let base = CellId::parse("18/1204/1052").unwrap();
        let (w, s, _, _) = square_deg(base);
        let side = S as f64 / 1e6;
        let cov = Coverage::parse_poly(&box_poly(w + 0.01, s + 0.01, w + 3.0 * side - 0.01, s + 3.0 * side - 0.01))
            .expect("poly");
        let cells = cov.cells(LOG2);
        assert_eq!(cells.len(), 9, "3x3 cells: {cells:?}");
        let middle = CellId::new(LOG2, base.i + 1, base.j + 1).unwrap();
        assert!(cells.contains(&middle));
        // …and only that middle one is fully covered.
        let boundary = cov.boundary_cells(LOG2);
        let canonical: Vec<CellId> = cells.iter().copied().filter(|c| cov.covers(*c, &boundary)).collect();
        assert_eq!(canonical, vec![middle], "a cell the border crosses is never canonical");
    }

    #[test]
    fn a_hole_is_not_covered() {
        let base = CellId::parse("18/1204/1052").unwrap();
        let (w, s, _, _) = square_deg(base);
        let side = S as f64 / 1e6;
        // A 3x3 box with the middle cell punched out.
        let outer = format!(
            "   {} {}\n   {} {}\n   {} {}\n   {} {}\n   {} {}\n",
            w + 0.01,
            s + 0.01,
            w + 3.0 * side - 0.01,
            s + 0.01,
            w + 3.0 * side - 0.01,
            s + 3.0 * side - 0.01,
            w + 0.01,
            s + 3.0 * side - 0.01,
            w + 0.01,
            s + 0.01
        );
        let (hw, hs, he, hn) = (w + side + 0.01, s + side + 0.01, w + 2.0 * side - 0.01, s + 2.0 * side - 0.01);
        let hole = format!("   {hw} {hs}\n   {he} {hs}\n   {he} {hn}\n   {hw} {hn}\n   {hw} {hs}\n");
        let cov = Coverage::parse_poly(&format!("region\n1\n{outer}END\n!2\n{hole}END\nEND\n")).expect("poly");
        let middle = CellId::new(LOG2, base.i + 1, base.j + 1).unwrap();
        let boundary = cov.boundary_cells(LOG2);
        assert!(!cov.covers(middle, &boundary), "the punched-out middle is not covered");
        // The hole's own border still makes the cell part of the selection.
        assert!(cov.cells(LOG2).contains(&middle));
    }

    /// The co-baked border case, which is the whole point of `union`: neither half
    /// covers the straddled cell, and together they do.
    #[test]
    fn two_abutting_sources_together_cover_the_cell_they_straddle() {
        let cell = CellId::parse("18/1204/1052").unwrap();
        let (w, s, e, n) = square_deg(cell);
        let mid = (w + e) / 2.0;
        let west = Coverage::parse_poly(&box_poly(w - 0.1, s - 0.1, mid, n + 0.1)).unwrap();
        let east = Coverage::parse_poly(&box_poly(mid, s - 0.1, e + 0.1, n + 0.1)).unwrap();
        assert!(!west.covers(cell, &west.boundary_cells(LOG2)), "the western half alone leaves a sliver");
        assert!(!east.covers(cell, &east.boundary_cells(LOG2)));

        let both = Coverage::union(&[&west, &east]).expect("GEOS unions two abutting boxes");
        assert!(both.covers(cell, &both.boundary_cells(LOG2)), "co-baked, the cell is canonical");
    }

    /// The bug a concatenation instead of a union would have: two *overlapping*
    /// sources must not read as a hole where they overlap.
    #[test]
    fn overlapping_sources_do_not_cancel_each_other_out() {
        let cell = CellId::parse("18/1204/1052").unwrap();
        let (w, s, e, n) = square_deg(cell);
        let a = Coverage::parse_poly(&box_poly(w - 0.1, s - 0.1, e, n + 0.1)).unwrap();
        let b = Coverage::parse_poly(&box_poly(w, s - 0.1, e + 0.1, n + 0.1)).unwrap();
        let both = Coverage::union(&[&a, &b]).expect("union");
        assert!(both.contains((s * 1e6) as i64 + S / 2, ((w + e) / 2.0 * 1e6) as i64 + 1), "the overlap is inside");
        assert!(both.covers(cell, &both.boundary_cells(LOG2)));
    }

    #[test]
    fn a_segment_crossing_several_lines_is_walked_cell_by_cell() {
        let base = CellId::parse("18/1204/1052").unwrap();
        let (min_lon, min_lat, _, _) = base.square();
        // An exact 45° diagonal passes through cell *corners*, so it touches only the
        // three cells on the diagonal — the neighbours meet it at a single point, and
        // the half-open square gives that point to exactly one of them.
        let mut out = BTreeSet::new();
        let ends = ((min_lat + 10, min_lon + 10), (min_lat + 2 * S + 10, min_lon + 2 * S + 10));
        segment_cells(ends.0, ends.1, LOG2, &mut out);
        let diagonal: BTreeSet<CellId> = (0..=2).map(|k| CellId::new(LOG2, base.i + k, base.j + k).unwrap()).collect();
        assert_eq!(out, diagonal, "a corner-to-corner diagonal touches only the diagonal");

        // A shallower one really does pass through the cells in between.
        let mut out = BTreeSet::new();
        segment_cells((min_lat + 10, min_lon + 10), (min_lat + 2 * S + 10, min_lon + S / 2), LOG2, &mut out);
        assert!(out.contains(&CellId::new(LOG2, base.i + 1, base.j).unwrap()), "the cell between the ends: {out:?}");
        assert_eq!(out.len(), 3, "one column, three rows: {out:?}");

        // Direction must not change the answer.
        let mut reversed = BTreeSet::new();
        segment_cells((min_lat + 2 * S + 10, min_lon + S / 2), (min_lat + 10, min_lon + 10), LOG2, &mut reversed);
        assert_eq!(out, reversed);
    }

    /// The density denominator. A one-degree square at 47°N is ≈ 111.3 km tall and
    /// ≈ 76 km wide, so ≈ 8 460 km²; the union of two of them is twice that, and the
    /// *overlap* of two overlapping ones is counted once.
    #[test]
    fn covered_ground_is_a_spherical_area() {
        let one = Coverage::parse_poly(&box_poly(7.0, 47.0, 8.0, 48.0)).unwrap();
        let area = one.area_km2();
        assert!((8_300.0..8_600.0).contains(&area), "{area} km² for a 1° square at 47°N");

        let disjoint = Coverage::parse_poly(&box_poly(10.0, 47.0, 11.0, 48.0)).unwrap();
        let both = Coverage::union(&[&one, &disjoint]).expect("union");
        assert!((both.area_km2() - 2.0 * area).abs() < 1.0, "two disjoint squares add up");

        let overlapping = Coverage::parse_poly(&box_poly(7.5, 47.0, 8.5, 48.0)).unwrap();
        let merged = Coverage::union(&[&one, &overlapping]).expect("union");
        assert!((merged.area_km2() - 1.5 * area).abs() < 5.0, "shared ground is counted once: {}", merged.area_km2());

        // A hole subtracts, so the coverage's area is the ground it really covers.
        let hole = "region\n1\n   7.0 47.0\n   8.0 47.0\n   8.0 48.0\n   7.0 48.0\n   7.0 47.0\nEND\n!2\n   7.25 \
                    47.25\n   7.75 47.25\n   7.75 47.75\n   7.25 47.75\n   7.25 47.25\nEND\nEND\n";
        let punched = Coverage::parse_poly(hole).expect("poly");
        assert!((punched.area_km2() - 0.75 * area).abs() < 5.0, "{}", punched.area_km2());
    }

    #[test]
    fn a_malformed_poly_is_an_error_not_an_empty_coverage() {
        assert!(Coverage::parse_poly("").is_err());
        assert!(Coverage::parse_poly("region\n1\n   7.0 47.0\nEND\nEND\n").is_err());
    }
}
