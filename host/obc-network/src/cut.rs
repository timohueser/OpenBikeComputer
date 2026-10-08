//! Source-topology seam cuts and per-cell graph construction.

use crate::nav::{self, CutRun, JunctionKey, NavGraph};
use obc_map_core::grid::{on_grid_boundary, segment_crossing, Axis, CellId, GRID_ORIGIN};
use obc_map_core::progress::Progress;
use obc_places::metadata::Poi;
use obc_places::routing::RoutableWay;
use std::collections::HashMap;

/// How many refinement passes the boundary-vertex insertion makes before it gives up.
///
/// One pass inserts every crossing of the original segment. A second is needed only because a
/// crossing coordinate is rounded to the µdeg grid, which can move it across another line by at most
/// half a microdegree. The cap exists so a pathological input cannot spin, and [`prepare_nav`] counts
/// the non-convergences.
const MAX_CUT_REFINE: usize = 4;

/// Bucket POIs by the one cell whose half-open square contains them. Indices into `pois`, in input
/// order, so a cell's records are ordered deterministically.
pub fn bucket_pois(pois: &[Poi], cell_log2: u32) -> HashMap<(i64, i64), Vec<u32>> {
    let mut out: HashMap<(i64, i64), Vec<u32>> = HashMap::new();
    for (k, p) in pois.iter().enumerate() {
        let cell = CellId::containing(cell_log2, p.lat_udeg as i64, p.lon_udeg as i64);
        out.entry((cell.i, cell.j)).or_default().push(k as u32);
    }
    out
}

/// One source way with a boundary vertex inserted at every crossing of the nav band's grid.
struct PreparedWay {
    keys: Vec<JunctionKey>,
    /// µdeg `(lon, lat)`, parallel to `keys`.
    coords: Vec<(i32, i32)>,
    kind: u8,
}

/// The whole extract's routable ways, cut-ready: boundary vertices inserted, junction touch counts
/// taken over the **source snapshot**, and an index from cell to the ways that reach it.
pub struct NavCut {
    log2: u32,
    ways: Vec<PreparedWay>,
    /// OSM node id to how many routable ways of the source touch it. Junction-ness is classified
    /// from this, never from the ways that survive inside a cell.
    touch: HashMap<i64, u32>,
    cells: HashMap<(i64, i64), Vec<u32>>,
}

/// Prepare the nav cut: insert the deterministic boundary junctions and index the ways by cell.
///
/// The insertion is the heart of the seam contract. For every routable way and every cell-edge line
/// it crosses, a vertex is materialised at the crossing coordinate from [`segment_crossing`] — exact
/// `i128` interpolation with banker's rounding over canonically ordered endpoints. Both neighbours
/// run that computation over the same two source vertices and the same line, so they mint the same
/// integer pair, which is what lets an assembler unify the two stubs by exact coordinate equality
/// and nothing weaker.
///
/// A vertex that already lies exactly on a line is itself the boundary junction: no interpolation
/// and no new key, because [`NavCut::cell_graph`]'s predicate tests the coordinate.
///
/// A [`RoutableWay`] whose `coords` and `node_ids` are not two parallel lists of at least two
/// entries is rejected rather than indexed: every step below reads the two positionally, so a
/// malformed one would panic deep inside the cut.
pub fn prepare_nav(ways: &[RoutableWay], log2: u32, progress: &Progress) -> Result<NavCut, String> {
    let mut touch: HashMap<i64, u32> = HashMap::new();
    for w in ways {
        if w.coords.len() < 2 || w.node_ids.len() != w.coords.len() {
            return Err(format!(
                "malformed routable way: {} coordinate(s) and {} node id(s) — a nav way needs at least two of \
                 each, paired",
                w.coords.len(),
                w.node_ids.len()
            ));
        }
        for &nid in &w.node_ids {
            *touch.entry(nid).or_insert(0) += 1;
        }
    }
    let mut prepared = Vec::with_capacity(ways.len());
    let mut cells: HashMap<(i64, i64), Vec<u32>> = HashMap::new();
    let mut inserted = 0usize;
    let mut unconverged = 0usize;
    for w in ways {
        let mut keys: Vec<JunctionKey> = Vec::with_capacity(w.coords.len());
        let mut coords: Vec<(i32, i32)> = Vec::with_capacity(w.coords.len());
        keys.push(JunctionKey::Osm(w.node_ids[0]));
        coords.push(w.coords[0]);
        for k in 0..w.coords.len() - 1 {
            let (cuts, converged) = segment_cuts(w.coords[k], w.coords[k + 1], log2);
            if !converged {
                unconverged += 1;
            }
            for c in cuts {
                keys.push(JunctionKey::Boundary(c.0, c.1));
                coords.push(c);
                inserted += 1;
            }
            keys.push(JunctionKey::Osm(w.node_ids[k + 1]));
            coords.push(w.coords[k + 1]);
        }
        let idx = prepared.len() as u32;
        let mut owners: Vec<(i64, i64)> = coords.windows(2).map(|s| segment_owner(s[0], s[1], log2)).collect();
        owners.sort_unstable();
        owners.dedup();
        for o in owners {
            cells.entry(o).or_default().push(idx);
        }
        prepared.push(PreparedWay { keys, coords, kind: w.kind });
    }
    progress.log(format!("nav cut: {inserted} boundary junction(s) inserted across {} routable way(s)", ways.len()));
    if unconverged > 0 {
        // Never observed; a segment whose rounded crossings keep landing across another line would
        // leave sub-µdeg geometry on the wrong side of an edge. Loud rather than silent.
        progress.warn(format!(
            "warning: {unconverged} segment(s) did not converge in {MAX_CUT_REFINE} boundary-cut refinements"
        ));
    }
    Ok(NavCut { log2, ways: prepared, touch, cells })
}

/// Every grid line of size `2^log2` strictly between `v0` and `v1`, ascending.
fn lines_strictly_between(v0: i64, v1: i64, log2: u32) -> impl Iterator<Item = i64> {
    let s = 1i64 << log2;
    let (lo, hi) = (v0.min(v1), v0.max(v1));
    let first = GRID_ORIGIN + ((lo - GRID_ORIGIN).div_euclid(s) + 1) * s;
    std::iter::successors(Some(first), move |v| Some(v + s)).take_while(move |v| *v < hi)
}

/// The boundary junctions on segment `a`-`b` (µdeg `(lon, lat)`), ordered along the segment.
///
/// Returns `(cuts, converged)`. A crossing coordinate is rounded to the µdeg grid, which can push it
/// across a different line by half a microdegree, so the segment is re-scanned until no proper
/// crossing is left, or [`MAX_CUT_REFINE`] passes have run.
fn segment_cuts(a: (i32, i32), b: (i32, i32), log2: u32) -> (Vec<(i32, i32)>, bool) {
    // Fast path, and the overwhelmingly common one: an OSM segment is metres long and crosses
    // nothing, so the first pass is also the only pass.
    let first = crossings(a, b, log2);
    if first.is_empty() {
        return (Vec::new(), true);
    }
    let mut chain = Vec::with_capacity(first.len() + 2);
    chain.push(a);
    chain.extend(first);
    chain.push(b);
    let mut converged = false;
    for _ in 1..MAX_CUT_REFINE {
        let mut next: Vec<(i32, i32)> = Vec::with_capacity(chain.len() + 4);
        next.push(chain[0]);
        let mut added = 0usize;
        for w in chain.windows(2) {
            let cuts = crossings(w[0], w[1], log2);
            added += cuts.len();
            next.extend(cuts);
            next.push(w[1]);
        }
        chain = next;
        if added == 0 {
            converged = true;
            break;
        }
    }
    let n = chain.len();
    (chain[1..n - 1].to_vec(), converged)
}

/// The proper crossings of one segment with the grid, ordered along the segment. Endpoints and
/// duplicates are excluded: a vertex already on a line needs no interpolation.
fn crossings(a: (i32, i32), b: (i32, i32), log2: u32) -> Vec<(i32, i32)> {
    // The formula is written in (lat, lon); the packer's coordinates are (lon, lat).
    let (p, q) = ((a.1 as i64, a.0 as i64), (b.1 as i64, b.0 as i64));
    // Each crossing carries its position along the segment as an exact rational `num/den`, so
    // crossings of the two axes sort into one order without a float anywhere.
    let mut found: Vec<(i128, i128, (i32, i32))> = Vec::new();
    for c in lines_strictly_between(p.0, q.0, log2) {
        if let Some((lat, lon)) = segment_crossing(p, q, Axis::Lat, c) {
            found.push(((c - p.0) as i128, (q.0 - p.0) as i128, (lon as i32, lat as i32)));
        }
    }
    for c in lines_strictly_between(p.1, q.1, log2) {
        if let Some((lat, lon)) = segment_crossing(p, q, Axis::Lon, c) {
            found.push(((c - p.1) as i128, (q.1 - p.1) as i128, (lon as i32, lat as i32)));
        }
    }
    for f in &mut found {
        if f.1 < 0 {
            (f.0, f.1) = (-f.0, -f.1);
        }
    }
    found.sort_by(|x, y| (x.0 * y.1).cmp(&(y.0 * x.1)).then(x.2.cmp(&y.2)));
    let mut out: Vec<(i32, i32)> = Vec::with_capacity(found.len());
    for (_, _, pt) in found {
        if pt == a || pt == b || out.last() == Some(&pt) {
            continue;
        }
        out.push(pt);
    }
    out
}

/// The cell that owns segment `(a, b)` — valid once the segment crosses no grid line.
///
/// Per axis it is `div_euclid(min - origin, S)`, the half-open convention read off the segment: a
/// segment sitting exactly on an edge line belongs to the cell for which that line is a `min` edge,
/// so it is written once and never twice.
fn segment_owner(a: (i32, i32), b: (i32, i32), log2: u32) -> (i64, i64) {
    let s = 1i64 << log2;
    ((a.1.min(b.1) as i64 - GRID_ORIGIN).div_euclid(s), (a.0.min(b.0) as i64 - GRID_ORIGIN).div_euclid(s))
}

impl NavCut {
    /// The runs of source ways this cell owns: maximal chains of segments whose owner is `cell`. A
    /// run therefore ends only at a boundary junction or at a way's own end, which is why
    /// [`nav::build_graph_cut`] can treat every run endpoint as a junction.
    fn cell_runs(&self, cell: CellId) -> Vec<CutRun> {
        let key = (cell.i, cell.j);
        let mut runs = Vec::new();
        let Some(ways) = self.cells.get(&key) else { return runs };
        for &wi in ways {
            let w = &self.ways[wi as usize];
            let mut start: Option<usize> = None;
            for k in 0..w.coords.len() - 1 {
                let mine = segment_owner(w.coords[k], w.coords[k + 1], self.log2) == key;
                match (mine, start) {
                    (true, None) => start = Some(k),
                    (false, Some(s)) => {
                        runs.push(CutRun {
                            keys: w.keys[s..=k].to_vec(),
                            coords: w.coords[s..=k].to_vec(),
                            kind: w.kind,
                        });
                        start = None;
                    }
                    _ => {}
                }
            }
            if let Some(s) = start {
                runs.push(CutRun { keys: w.keys[s..].to_vec(), coords: w.coords[s..].to_vec(), kind: w.kind });
            }
        }
        runs
    }

    /// This cell's nav graph: junction-ness from the source snapshot plus every vertex on a boundary
    /// line, and pruning restricted to strictly interior components.
    pub fn cell_graph(&self, cell: CellId, min_component_edges: usize) -> NavGraph {
        let runs = self.cell_runs(cell);
        let log2 = self.log2;
        let is_junction = |key: JunctionKey, coord: (i32, i32)| match key {
            // Minted on the edge line: a junction in both neighbours, by construction.
            JunctionKey::Boundary(..) => true,
            // A real OSM node is a junction if the source's way set makes it one, or if it sits
            // exactly on a boundary line.
            JunctionKey::Osm(id) => {
                on_grid_boundary(coord.1 as i64, coord.0 as i64, log2) || self.touch.get(&id).copied().unwrap_or(0) >= 2
            }
        };
        let on_boundary = |coord: (i32, i32)| on_grid_boundary(coord.1 as i64, coord.0 as i64, log2);
        let (graph, _stats) = nav::build_graph_cut(&runs, min_component_edges, &is_junction, &on_boundary);
        graph
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use obc_map_core::grid::{on_grid_line, GRID_ORIGIN};

    const LOG2: u32 = 18;
    const S: i64 = 1 << LOG2;

    /// A coordinate on the `2^18` grid: cell (i, j)'s min corner plus offsets, as `(lon, lat)`.
    fn at(i: i64, j: i64, dlat: i64, dlon: i64) -> (i32, i32) {
        (((GRID_ORIGIN + j * S + dlon) as i32), ((GRID_ORIGIN + i * S + dlat) as i32))
    }

    /// A `(lon, lat)` point from absolute µdeg.
    fn pt(lat: i64, lon: i64) -> (i32, i32) {
        (lon as i32, lat as i32)
    }

    /// The lon line between cells `(_, 100)` and `(_, 101)` — every seam test's shared edge.
    const fn seam_lon() -> i64 {
        GRID_ORIGIN + 101 * S
    }

    /// A latitude inside row 100, well away from its own edges.
    const fn row_lat() -> i64 {
        GRID_ORIGIN + 100 * S + 5_000
    }

    #[test]
    fn lines_between_are_the_grid_lines() {
        let lo = GRID_ORIGIN + 5 * S;
        let got: Vec<i64> = lines_strictly_between(lo + 10, lo + 3 * S - 10, LOG2).collect();
        assert_eq!(got, vec![lo + S, lo + 2 * S]);
        // Endpoints exactly on lines are excluded: such a vertex IS the junction.
        let got: Vec<i64> = lines_strictly_between(lo, lo + S, LOG2).collect();
        assert!(got.is_empty(), "no line strictly between two adjacent lines");
        // Direction-independent.
        let a: Vec<i64> = lines_strictly_between(lo + 10, lo + 2 * S, LOG2).collect();
        let b: Vec<i64> = lines_strictly_between(lo + 2 * S, lo + 10, LOG2).collect();
        assert_eq!(a, b);
    }

    /// One crossing gives one boundary vertex, on the line, and the reversed segment gives the same
    /// one.
    #[test]
    fn one_crossing_one_boundary_vertex() {
        let a = pt(row_lat(), seam_lon() - 5_000);
        let b = pt(row_lat(), seam_lon() + 5_000); // same lat, crosses only the shared lon line
        let (cuts, ok) = segment_cuts(a, b, LOG2);
        assert!(ok);
        assert_eq!(cuts.len(), 1, "one line crossed ⇒ one junction");
        assert_eq!(cuts[0].0 as i64, seam_lon(), "on the line, exactly");
        assert!(on_grid_line(cuts[0].0 as i64, LOG2));
        let (rev, _) = segment_cuts(b, a, LOG2);
        assert_eq!(rev, cuts, "a reversed segment cuts at the identical coordinate");
    }

    /// A long diagonal crosses several lines of both axes; the cuts come out ordered along the
    /// segment, all on lines, and each in the right cell.
    #[test]
    fn multiple_crossings_are_ordered_along_the_segment() {
        let a = at(100, 100, 1_000, 1_000);
        let b = at(103, 102, 1_000, 1_000);
        let (cuts, ok) = segment_cuts(a, b, LOG2);
        assert!(ok, "converged");
        assert_eq!(cuts.len(), 3 + 2, "three lat lines + two lon lines");
        for c in &cuts {
            assert!(on_grid_boundary(c.1 as i64, c.0 as i64, LOG2), "every cut is on a grid line");
        }
        // Monotone in both axes (the segment is), so ordering along it is ordering per axis.
        assert!(cuts.windows(2).all(|w| w[0].0 <= w[1].0 && w[0].1 <= w[1].1), "ordered: {cuts:?}");
        let (rev, _) = segment_cuts(b, a, LOG2);
        let mut rev_sorted = rev;
        rev_sorted.sort();
        let mut fwd_sorted = cuts;
        fwd_sorted.sort();
        assert_eq!(rev_sorted, fwd_sorted, "direction changes the order, never the coordinates");
    }

    /// Segment ownership is the half-open rule read off a segment, including the collinear case: a
    /// segment lying on a line belongs to the cell above or east of it, once.
    #[test]
    fn segment_ownership_is_half_open() {
        let inside = (at(100, 100, 10, 10), at(100, 100, 20, 20));
        assert_eq!(segment_owner(inside.0, inside.1, LOG2), (100, 100));
        // Touching the min edge from inside.
        let on_min = (at(100, 100, 0, 10), at(100, 100, 50, 20));
        assert_eq!(segment_owner(on_min.0, on_min.1, LOG2), (100, 100));
        // Touching the max edge from inside ⇒ still this cell.
        let to_max = (at(100, 100, S - 50, 10), at(101, 100, 0, 20));
        assert_eq!(segment_owner(to_max.0, to_max.1, LOG2), (100, 100));
        // Wholly on the shared lon line ⇒ the cell for which it is a `min` edge.
        let along = (at(100, 101, 10, 0), at(100, 101, 20, 0));
        assert_eq!(segment_owner(along.0, along.1, LOG2), (100, 101));
        // Just past the line ⇒ the next cell.
        let past = (at(100, 101, 10, 1), at(100, 101, 20, 2));
        assert_eq!(segment_owner(past.0, past.1, LOG2), (100, 101));
        let before = (at(100, 100, 10, S - 2), at(100, 100, 20, S - 1));
        assert_eq!(segment_owner(before.0, before.1, LOG2), (100, 100));
    }

    fn way(nodes: &[(i64, (i32, i32))]) -> RoutableWay {
        RoutableWay {
            node_ids: nodes.iter().map(|(id, _)| *id).collect(),
            coords: nodes.iter().map(|(_, c)| *c).collect(),
            kind: 7,
        }
    }

    /// The seam property at the level of one prepared way: both cells see a junction at the same
    /// coordinate on the shared edge, and each carries its own stub inward.
    #[test]
    fn neighbours_agree_on_the_boundary_junction() {
        // One short road running west to east across the line between cells (100, 100) and
        // (100, 101). Short on purpose: a way spanning a whole cell would be split by the `i16`
        // bound into pieces, which is orthogonal to what this test is about.
        let seam = seam_lon();
        let w = way(&[(1, pt(row_lat(), seam - 5_000)), (2, pt(row_lat(), seam + 5_000))]);
        let prep = prepare_nav(&[w], LOG2, &Progress::silent()).expect("prepare");
        let west = CellId::new(LOG2, 100, 100).unwrap();
        let east = CellId::new(LOG2, 100, 101).unwrap();
        let gw = prep.cell_graph(west, 50);
        let ge = prep.cell_graph(east, 50);
        assert_eq!(gw.edges.len(), 1, "the western stub");
        assert_eq!(ge.edges.len(), 1, "the eastern stub");
        let on_seam = |g: &NavGraph| -> Vec<(i32, i32)> {
            g.nodes.iter().filter(|n| n.coord.0 as i64 == seam).map(|n| n.coord).collect()
        };
        let (a, b) = (on_seam(&gw), on_seam(&ge));
        assert_eq!(a.len(), 1, "exactly one boundary junction per side");
        assert_eq!(a, b, "and it is the SAME coordinate — this is what an assembler unifies");
        // Each side's stub really does reach the seam.
        assert_eq!(gw.edges[0].polyline.last().map(|p| p.0 as i64), Some(seam));
        assert_eq!(ge.edges[0].polyline.first().map(|p| p.0 as i64), Some(seam));
    }

    /// A vertex that already sits exactly on the line is the junction: no interpolation, no extra
    /// node.
    #[test]
    fn a_vertex_on_the_line_is_the_junction() {
        let seam = seam_lon();
        let w = way(&[
            (1, pt(row_lat(), seam - 5_000)),
            (2, pt(row_lat(), seam)), // an OSM node sitting exactly on the edge line
            (3, pt(row_lat(), seam + 5_000)),
        ]);
        let prep = prepare_nav(&[w], LOG2, &Progress::silent()).expect("prepare");
        assert_eq!(prep.ways[0].coords.len(), 3, "nothing was inserted");
        assert!(matches!(prep.ways[0].keys[1], JunctionKey::Osm(2)), "the OSM node keeps its identity");
        let gw = prep.cell_graph(CellId::new(LOG2, 100, 100).unwrap(), 50);
        let ge = prep.cell_graph(CellId::new(LOG2, 100, 101).unwrap(), 50);
        for g in [&gw, &ge] {
            assert_eq!(g.edges.len(), 1);
            assert!(g.nodes.iter().any(|n| n.coord.0 as i64 == seam), "both sides carry the on-line junction");
        }
    }

    /// Island pruning is strictly interior: a stub touching the cell edge survives however small,
    /// while an equally small component in the middle of the cell does not.
    #[test]
    fn pruning_spares_components_touching_the_edge() {
        // A boundary-crossing stub (one edge per side) and a tiny interior islet (one edge).
        let seam = seam_lon();
        let crossing = way(&[(1, pt(row_lat(), seam - 5_000)), (2, pt(row_lat(), seam + 5_000))]);
        let islet = way(&[(10, at(100, 100, 100_000, 100_000)), (11, at(100, 100, 100_500, 100_500))]);
        let prep = prepare_nav(&[crossing, islet], LOG2, &Progress::silent()).expect("prepare");
        let g = prep.cell_graph(CellId::new(LOG2, 100, 100).unwrap(), 50);
        assert!(g.nodes.iter().any(|n| n.coord.0 as i64 == seam), "the boundary stub survived pruning");
        assert!(
            !g.nodes.iter().any(|n| n.coord.1 as i64 == GRID_ORIGIN + 100 * S + 100_000),
            "the interior islet was pruned"
        );
    }
}
