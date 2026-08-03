//! Contour lines traced out of the baked OBCT terrain a run was given (epic #1068, EL10a #1094).
//!
//! Contours are **not a new kind of map content**. They are traced here, handed to the packer as
//! ordinary [`Geom::Line`] features with an ordinary style id, and from that moment on they are
//! indistinguishable from a road: the same per-LOD simplify, the same densify, the same quadtree
//! split, the same chunk budget. That is the whole point of the placement #1088 settled on — the
//! render path is provably unchanged, because nothing about it was told that contours exist.
//!
//! Three rules the rest of this module is an implementation of:
//!
//! 1. **One sampling truth.** Heights are read back through `obc_elevation::TerrainReader` — the
//!    same `no_std` reader the device runs — at exact lattice coordinates, where `OBCT_Spec.md` §5's
//!    bilinear collapses to the stored sample. This crate does not decode a container.
//! 2. **A hole is silence** (OBCT principle 6). A lattice cell any of whose four corners is unknown
//!    contributes nothing at all; a contour is never drawn across ground the DEM does not know. The
//!    reader is stricter still — it voids a query whose bilinear stencil touches `NODATA` even where
//!    the weight is zero — so coverage erodes by one posting around a hole rather than guessing at
//!    its edge, which is the conservative direction.
//! 3. **No private geometry pipeline.** The traced polylines are simplified **once** here, at the
//!    clamp (see [`crate::config::Contours::simplify_m`]), and then go into `ingested.features`
//!    where the ladder takes over. The clamp uses the packer's own
//!    [`topology_preserve_simplify`], not a second Douglas–Peucker.
//!
//! ## The trace
//!
//! Marching squares over the sample lattice, one level at a time. Each lattice cell contributes 0, 1
//! or 2 segments; each segment endpoint sits on a lattice *edge* and is materialised once, so the
//! two cells sharing an edge agree on the crossing by construction rather than by rounding. Segments
//! are then chained into the longest possible polylines.
//!
//! The bookkeeping that makes this cheap is the rolling edge window: a crossing on a horizontal edge
//! is shared by the cell row below and the cell row above, and one on a vertical edge by the two
//! cells beside it — so **two rows of horizontal ids and one row of vertical ids** is all the state a
//! sweep needs, `O(cols)` rather than the `O(rows × cols)` map a naive edge-keyed `HashMap` would
//! hold. Chaining is likewise map-free: a crossing point belongs to exactly two lattice cells, so its
//! degree can never exceed two and adjacency is a fixed two-slot array.
//!
//! Wide extracts are traced in horizontal **strips** so the resident sample window stays bounded;
//! strips overlap by one lattice row, so every cell is traced exactly once and no contour is left
//! with a gap. A contour crossing a strip boundary becomes two features that meet at a shared
//! vertex — which `merge_lines` stitches back together if the config asks it to, and which is in any
//! case what the quadtree would have done to a long line anyway.

use rayon::prelude::*;

use obc_elevation::grid::{lattice_coord, locate};
use obc_elevation::ElevationSource;
use obc_formats::obct::{GRID_ORIGIN, NODATA, WORLD_SIDE};
use obc_map_scene::M_PER_DEG;

use crate::config::{Config, ContourClass};
use crate::geom::{topology_preserve_simplify, Geom};
use crate::ingest::{IngestFeature, Ingested};
use crate::progress::{Phase, Progress};
use crate::terrain::TerrainSet;

/// Upper bound on the samples one strip holds resident. 4 M samples is 8 MB of `i16` — small next to
/// the ingested extract sitting beside it, and large enough that any single region cell, and the
/// whole of a typical alpine shard, is one strip with no seams at all.
const STRIP_SAMPLE_BUDGET: usize = 4 << 20;

/// Traced polylines shorter than this are dropped before they reach the packer. Two points is a
/// single cell crossing — a speck of terrain noise that would cost a feature header to say nothing.
const MIN_TRACED_VERTICES: usize = 3;

/// What tracing one level of one strip produced: the style id and `min_lod` its class packs under,
/// the **level in metres** the polylines are (OBCM v13 §5.2 carries it on every one of them), and
/// the µdeg polylines themselves.
type LevelTrace = (u8, usize, i16, Vec<Vec<(i32, i32)>>);

/// Trace contours from `terrain` and append them to `ingested` as ordinary line features.
///
/// A no-op — not merely a cheap one, but never entered — when the config does not ask for contours,
/// when the run was given no terrain, or when neither contour class carries a style rule.
/// `bbox` is the packer's global box, µdeg `(min_lon, min_lat, max_lon, max_lat)`.
pub(crate) fn add_contours(
    ingested: &mut Ingested,
    config: &Config,
    bbox: (i64, i64, i64, i64),
    terrain: Option<&TerrainSet>,
    progress: &Progress,
) -> Result<(), String> {
    let cfg = config.contours;
    if !cfg.enabled {
        return Ok(());
    }
    let Some(set) = terrain.filter(|s| !s.is_empty()) else {
        progress.warn("warning: contours are enabled but this run was given no terrain — none traced");
        return Ok(());
    };
    // A class with no style rule is not packed, so it is not traced either.
    let classes: Vec<(ContourClass, u8, usize)> = [ContourClass::Major, ContourClass::Index]
        .into_iter()
        .filter_map(|c| config.contour_style(c).map(|s| (c, s.id, s.min_lod)))
        .collect();
    if classes.is_empty() {
        progress.warn(
            "warning: contours are enabled but the config has no `features.contour.major` or \
             `features.contour.index` style — none packed",
        );
        return Ok(());
    }

    let Some(window) = trace_window(bbox, set) else { return Ok(()) };
    progress.stage(
        Phase::Contours,
        format!("Tracing contours every {} m (index every {})...", cfg.interval_m, cfg.index_every),
    );

    let strips = split_strips(window);
    // Sequential over strips (each holds its sample window resident), parallel over levels inside
    // one — which is where the work is: an alpine strip carries dozens of levels over one grid.
    let mut traced: Vec<(u8, usize, i16, Geom)> = Vec::new();
    let (mut lines, mut vertices) = (0usize, 0usize);
    for strip in &strips {
        progress.check()?;
        let grid = Grid::read(set, *strip)?;
        let Some((lo, hi)) = grid.range() else { continue };
        // Levels are absolute multiples of the interval, so which levels exist is a property of the
        // terrain and not of where a strip boundary happened to fall.
        let first = lo.div_euclid(cfg.interval_m) * cfg.interval_m;
        let levels: Vec<i32> = (0..)
            .map(|k| first + k * cfg.interval_m)
            .take_while(|&level| level <= hi)
            .filter(|&level| level >= lo)
            .collect();
        let index_step = cfg.interval_m * cfg.index_every as i32;
        let per_level: Vec<LevelTrace> = levels
            .par_iter()
            .filter_map(|&level| {
                let class = if level.rem_euclid(index_step) == 0 { ContourClass::Index } else { ContourClass::Major };
                let (_, style_id, min_lod) = *classes.iter().find(|(c, _, _)| *c == class)?;
                // The levels come off an `i16` DEM through an `i16` reader, so the wire field
                // (§5.2, metres) cannot overflow — but the trace walks them as `i32`, so the one
                // narrowing in the whole path is stated here rather than left to a cast.
                let wire = i16::try_from(level).ok()?;
                Some((style_id, min_lod, wire, march(&grid, level)))
            })
            .collect();
        for (style_id, min_lod, level, polylines) in per_level {
            for line in polylines {
                lines += 1;
                vertices += line.len();
                traced.push((style_id, min_lod, level, to_geom(&line)));
            }
        }
    }
    progress.check()?;

    // The clamp, in the packer's own simplifier: everything after this point is the ordinary ladder,
    // and the ladder's two finest tiers are finer than the DEM's posting can justify (#1088 §4.3).
    let kept: Vec<(u8, usize, i16, Geom)> = if cfg.simplify_m > 0.0 {
        let tol = cfg.simplify_m / M_PER_DEG;
        traced
            .into_par_iter()
            .filter_map(|(style_id, min_lod, level, geom)| {
                let simplified = topology_preserve_simplify(&geom, tol);
                (!simplified.is_empty()).then_some((style_id, min_lod, level, simplified))
            })
            .collect()
    } else {
        traced
    };

    let clamped: usize = kept.iter().map(|(_, _, _, g)| count_vertices(g)).sum();
    let n = kept.len();
    for (style_id, min_lod, level, geom) in kept {
        // The one place in the packer that fills `level`: the trace is the only stage that knows
        // one, and from here it is carried, never recomputed (v13, #1105).
        ingested.features.push(IngestFeature { style_id, min_lod, level: Some(level), geom });
    }
    progress.log(format!(
        "  traced {lines} contour line(s), {vertices} vertices -> {n} feature(s), {clamped} vertices after the \
         {} m clamp",
        cfg.simplify_m
    ));
    // The credit is a licence obligation and travels with the data, never retyped: it is one `const`
    // in `obc-dem`, and this is the point at which a `.obcm` starts carrying GLO-30-derived geometry.
    progress.log(format!("  contours derived from {}: {}", obc_dem::SOURCE_DATASET, obc_dem::COPERNICUS_ATTRIBUTION));
    Ok(())
}

// --- the lattice window ------------------------------------------------------------------------

/// The inclusive lattice rectangle a trace walks: rows are latitude indices, columns longitude.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Window {
    i0: u32,
    i1: u32,
    j0: u32,
    j1: u32,
    posting_log2: u8,
}

impl Window {
    fn rows(&self) -> usize {
        (self.i1 - self.i0) as usize + 1
    }

    fn cols(&self) -> usize {
        (self.j1 - self.j0) as usize + 1
    }
}

/// The lattice rectangle covering `bbox ∩ terrain coverage`, or `None` when they do not meet.
///
/// Rounded **outward** to whole lattice indices: the box's own edge is almost never a sample, and a
/// contour that leaves the map is clipped exactly once, by the quadtree root, like every other
/// feature.
fn trace_window(bbox: (i64, i64, i64, i64), set: &TerrainSet) -> Option<Window> {
    let posting_log2 = set.posting_log2()?;
    let (cmin_lon, cmin_lat, cmax_lon, cmax_lat) = set.coverage()?;
    let (min_lon, min_lat) = (bbox.0.max(cmin_lon), bbox.1.max(cmin_lat));
    // Coverage maxima are half-open (`OBCT_Spec.md` §4.2): the last sample sits one posting inside.
    let (max_lon, max_lat) = (bbox.2.min(cmax_lon - 1), bbox.3.min(cmax_lat - 1));
    if min_lon > max_lon || min_lat > max_lat {
        return None;
    }
    let (i0, j0) = (lattice_floor(min_lat, posting_log2), lattice_floor(min_lon, posting_log2));
    let (i1, j1) = (lattice_ceil(max_lat, posting_log2), lattice_ceil(max_lon, posting_log2));
    (i1 > i0 && j1 > j0).then_some(Window { i0, i1, j0, j1, posting_log2 })
}

/// The lattice index at or below `v_udeg`, clamped into the world box.
///
/// The index arithmetic is `obc_elevation::grid`'s, not a second copy of the shift: both axes locate
/// identically, so one coordinate passed as both answers for whichever axis the caller meant.
fn lattice_floor(v_udeg: i64, posting_log2: u8) -> u32 {
    let last = (WORLD_SIDE >> posting_log2) - 1;
    let clamped = v_udeg.clamp(GRID_ORIGIN as i64, GRID_ORIGIN as i64 + WORLD_SIDE as i64 - 1) as i32;
    locate(clamped, clamped, posting_log2).expect("a coordinate clamped into the world box locates").i.min(last)
}

/// The lattice index at or above `v_udeg`, clamped into the world box.
fn lattice_ceil(v_udeg: i64, posting_log2: u8) -> u32 {
    let last = (WORLD_SIDE >> posting_log2) - 1;
    let i = lattice_floor(v_udeg, posting_log2);
    if i64::from(lattice_coord(i, posting_log2)) == v_udeg {
        i
    } else {
        (i + 1).min(last)
    }
}

/// Cut `window` into row strips that overlap by one lattice row.
///
/// The overlap is what makes the seam invisible: strip `k` traces the cells between its first and
/// last row, and strip `k + 1` starts *on* that last row, so the cell straddling the boundary is
/// traced exactly once and no band of ground is skipped.
fn split_strips(window: Window) -> Vec<Window> {
    let cols = window.cols();
    // Two rows is one cell row — the smallest strip that traces anything.
    let per_strip = (STRIP_SAMPLE_BUDGET / cols.max(1)).max(2);
    let mut strips = Vec::new();
    let mut i0 = window.i0;
    while i0 < window.i1 {
        let i1 = (i0 + per_strip as u32 - 1).min(window.i1);
        strips.push(Window { i0, i1, ..window });
        i0 = i1;
    }
    strips
}

// --- the sample window -------------------------------------------------------------------------

/// One strip of the lattice, read out of the terrain set. `NODATA` marks every unknown sample —
/// including the ones the reader voided because their bilinear stencil touched a hole.
struct Grid {
    window: Window,
    cols: usize,
    /// Row-major, `rows * cols`.
    z: Vec<i16>,
}

impl Grid {
    fn read(set: &TerrainSet, window: Window) -> Result<Grid, String> {
        let (rows, cols) = (window.rows(), window.cols());
        let posting_log2 = window.posting_log2;
        // One sampler for the strip: it opens only the containers this rectangle touches, and its
        // tile cache is what turns a row sweep into a handful of reads.
        let mut sampler = set.sampler_for(Some((
            i64::from(lattice_coord(window.j0, posting_log2)),
            i64::from(lattice_coord(window.i0, posting_log2)),
            i64::from(lattice_coord(window.j1, posting_log2)),
            i64::from(lattice_coord(window.i1, posting_log2)),
        )))?;
        let mut z = Vec::with_capacity(rows * cols);
        for r in 0..rows {
            let lat = lattice_coord(window.i0 + r as u32, posting_log2);
            for c in 0..cols {
                let lon = lattice_coord(window.j0 + c as u32, posting_log2);
                z.push(sampler.sample(lat, lon).unwrap_or(NODATA));
            }
        }
        Ok(Grid { window, cols, z })
    }

    #[inline]
    fn at(&self, r: usize, c: usize) -> i16 {
        self.z[r * self.cols + c]
    }

    fn rows(&self) -> usize {
        self.window.rows()
    }

    /// The strip's elevation range, or `None` when it is entirely unknown.
    fn range(&self) -> Option<(i32, i32)> {
        let (lo, hi) = self
            .z
            .iter()
            .filter(|&&v| v != NODATA)
            .fold((i32::MAX, i32::MIN), |(lo, hi), &v| (lo.min(i32::from(v)), hi.max(i32::from(v))));
        (lo <= hi).then_some((lo, hi))
    }

    /// µdeg longitude of column `c`.
    #[inline]
    fn lon(&self, c: usize) -> i32 {
        lattice_coord(self.window.j0 + c as u32, self.window.posting_log2)
    }

    /// µdeg latitude of row `r`.
    #[inline]
    fn lat(&self, r: usize) -> i32 {
        lattice_coord(self.window.i0 + r as u32, self.window.posting_log2)
    }

    #[inline]
    fn posting(&self) -> f64 {
        f64::from(1u32 << self.window.posting_log2)
    }
}

// --- marching squares --------------------------------------------------------------------------

/// Sentinel for "this lattice edge has no crossing point yet".
const NO_POINT: u32 = u32::MAX;

/// Trace every contour of `grid` at `level` metres, as µdeg `(lon, lat)` polylines.
fn march(grid: &Grid, level: i32) -> Vec<Vec<(i32, i32)>> {
    let (rows, cols) = (grid.rows(), grid.cols);
    if rows < 2 || cols < 2 {
        return Vec::new();
    }
    let mut pts: Vec<(i32, i32)> = Vec::new();
    let mut segs: Vec<(u32, u32)> = Vec::new();

    // The rolling edge window (see the module header): horizontal-edge point ids for the lattice row
    // below and above the current cell row, and vertical-edge ids within it.
    let mut h_below = vec![NO_POINT; cols - 1];
    let mut h_above = vec![NO_POINT; cols - 1];
    let mut vertical = vec![NO_POINT; cols];

    let posting = grid.posting();
    // Where the level crosses between two samples, as a fraction of the posting.
    let frac = |a: i16, b: i16| -> f64 {
        let (a, b) = (f64::from(a), f64::from(b));
        if (b - a).abs() < f64::EPSILON {
            0.0
        } else {
            ((f64::from(level) - a) / (b - a)).clamp(0.0, 1.0)
        }
    };

    for r in 0..rows - 1 {
        h_above.iter_mut().for_each(|e| *e = NO_POINT);
        vertical.iter_mut().for_each(|e| *e = NO_POINT);
        for c in 0..cols - 1 {
            // bit0 = SW, bit1 = SE, bit2 = NE, bit3 = NW — the standard case numbering.
            let (a, b, d, e) = (grid.at(r, c), grid.at(r, c + 1), grid.at(r + 1, c + 1), grid.at(r + 1, c));
            if a == NODATA || b == NODATA || d == NODATA || e == NODATA {
                continue; // OBCT principle 6: a cell touching a hole is silent, whole
            }
            let above = |v: i16| i32::from(v) > level;
            let case = u8::from(above(a)) | u8::from(above(b)) << 1 | u8::from(above(d)) << 2 | u8::from(above(e)) << 3;
            if case == 0 || case == 15 {
                continue;
            }

            // Materialise a crossing at most once per lattice edge, so the two cells sharing it read
            // back the identical coordinate and the chain closes on equality rather than on epsilon.
            let south = || (grid.lon(c) + (frac(a, b) * posting).round() as i32, grid.lat(r));
            let north = || (grid.lon(c) + (frac(e, d) * posting).round() as i32, grid.lat(r + 1));
            let west = || (grid.lon(c), grid.lat(r) + (frac(a, e) * posting).round() as i32);
            let east = || (grid.lon(c + 1), grid.lat(r) + (frac(b, d) * posting).round() as i32);

            macro_rules! edge {
                ($slot:expr, $make:expr) => {{
                    if $slot == NO_POINT {
                        $slot = pts.len() as u32;
                        pts.push($make());
                    }
                    $slot
                }};
            }

            // Saddles (cases 5 and 10) are ambiguous by construction; the cell mean picks the pair,
            // which is the standard disambiguation and — being a function of the four corners alone —
            // is decided identically by both cells that share each edge.
            let sum = i32::from(a) + i32::from(b) + i32::from(d) + i32::from(e);
            let centre_above = f64::from(sum) / 4.0 > f64::from(level);
            match case {
                1 | 14 => {
                    let (p, q) = (edge!(vertical[c], west), edge!(h_below[c], south));
                    segs.push((p, q));
                }
                2 | 13 => {
                    let (p, q) = (edge!(h_below[c], south), edge!(vertical[c + 1], east));
                    segs.push((p, q));
                }
                3 | 12 => {
                    let (p, q) = (edge!(vertical[c], west), edge!(vertical[c + 1], east));
                    segs.push((p, q));
                }
                4 | 11 => {
                    let (p, q) = (edge!(vertical[c + 1], east), edge!(h_above[c], north));
                    segs.push((p, q));
                }
                6 | 9 => {
                    let (p, q) = (edge!(h_below[c], south), edge!(h_above[c], north));
                    segs.push((p, q));
                }
                7 | 8 => {
                    let (p, q) = (edge!(vertical[c], west), edge!(h_above[c], north));
                    segs.push((p, q));
                }
                5 | 10 => {
                    // Case 5 has SW+NE above; case 10 has SE+NW. Which pairing keeps the two "above"
                    // corners on the same side of the contour flips with the case *and* the centre.
                    let west_joins_north = (case == 5) == centre_above;
                    let (w, s) = (edge!(vertical[c], west), edge!(h_below[c], south));
                    let (e_, n) = (edge!(vertical[c + 1], east), edge!(h_above[c], north));
                    if west_joins_north {
                        segs.push((w, n));
                        segs.push((s, e_));
                    } else {
                        segs.push((w, s));
                        segs.push((e_, n));
                    }
                }
                _ => unreachable!("cases 0 and 15 return early; the rest are covered"),
            }
        }
        std::mem::swap(&mut h_below, &mut h_above);
    }

    chain(&pts, &segs)
}

/// Chain segments into the longest possible polylines.
///
/// A crossing point sits on one lattice edge, an edge belongs to at most two cells, and a cell emits
/// at most one segment through any one of its edges — so degree is at most two and adjacency is two
/// slots, no map and no sort. Open chains (a contour running off the strip, or up against a hole) are
/// walked from their free end first, so such a contour is one feature rather than two; whatever
/// remains is a closed loop, emitted with its first point repeated at the end.
fn chain(pts: &[(i32, i32)], segs: &[(u32, u32)]) -> Vec<Vec<(i32, i32)>> {
    let mut adj = vec![[NO_POINT; 2]; pts.len()];
    for (i, &(a, b)) in segs.iter().enumerate() {
        for p in [a, b] {
            let slots = &mut adj[p as usize];
            if slots[0] == NO_POINT {
                slots[0] = i as u32;
            } else {
                debug_assert_eq!(slots[1], NO_POINT, "a lattice crossing joins at most two segments");
                slots[1] = i as u32;
            }
        }
    }
    let mut used = vec![false; segs.len()];
    let mut out = Vec::new();
    let mut path: Vec<u32> = Vec::new();

    // Free ends first. Point ids are assigned in sweep order, so the output order — and therefore
    // the packed bytes — is the same on every machine.
    for p in 0..pts.len() as u32 {
        if adj[p as usize][1] != NO_POINT || adj[p as usize][0] == NO_POINT {
            continue;
        }
        walk(p, segs, &adj, &mut used, &mut path);
        emit(&path, pts, &mut out);
    }
    // What is left is loops.
    for s in 0..segs.len() {
        if used[s] {
            continue;
        }
        used[s] = true;
        let (a, b) = segs[s];
        walk(b, segs, &adj, &mut used, &mut path);
        path.insert(0, a);
        emit(&path, pts, &mut out);
    }
    out
}

/// Walk from `start` along unused segments, recording the point ids into `path`.
fn walk(start: u32, segs: &[(u32, u32)], adj: &[[u32; 2]], used: &mut [bool], path: &mut Vec<u32>) {
    path.clear();
    path.push(start);
    let mut cur = start;
    loop {
        let Some(&next) = adj[cur as usize].iter().find(|&&s| s != NO_POINT && !used[s as usize]) else {
            return;
        };
        used[next as usize] = true;
        let (a, b) = segs[next as usize];
        cur = if a == cur { b } else { a };
        path.push(cur);
    }
}

/// Materialise one walked path, dropping the specks.
fn emit(path: &[u32], pts: &[(i32, i32)], out: &mut Vec<Vec<(i32, i32)>>) {
    if path.len() < MIN_TRACED_VERTICES {
        return;
    }
    out.push(path.iter().map(|&p| pts[p as usize]).collect());
}

// --- handing over to the packer ------------------------------------------------------------------

/// µdeg `(lon, lat)` → the packer's degree-space line.
fn to_geom(line: &[(i32, i32)]) -> Geom {
    Geom::Line(line.iter().map(|&(lon, lat)| (f64::from(lon) / 1e6, f64::from(lat) / 1e6)).collect())
}

/// Vertices in a (possibly `Multi`) geometry — for the log line only.
fn count_vertices(g: &Geom) -> usize {
    match g {
        Geom::Line(c) => c.len(),
        Geom::Polygon { exterior, interiors } => exterior.len() + interiors.iter().map(Vec::len).sum::<usize>(),
        Geom::Multi(parts) => parts.iter().map(count_vertices).sum(),
        Geom::Empty => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a grid straight from heights, bypassing the terrain reader: these tests are about the
    /// marching-squares half, and a synthetic cone is a far sharper oracle than a real DEM.
    fn grid(rows: usize, cols: usize, f: impl Fn(usize, usize) -> i16) -> Grid {
        let mut z = Vec::with_capacity(rows * cols);
        for r in 0..rows {
            for c in 0..cols {
                z.push(f(r, c));
            }
        }
        // Posting 2^9 µdeg and an origin-anchored window: the arithmetic under test is the same at
        // any offset, and small indices keep the expected coordinates readable.
        Grid {
            window: Window {
                i0: 1000,
                i1: 1000 + rows as u32 - 1,
                j0: 2000,
                j1: 2000 + cols as u32 - 1,
                posting_log2: 9,
            },
            cols,
            z,
        }
    }

    fn closed(line: &[(i32, i32)]) -> bool {
        line.first() == line.last()
    }

    /// A single plane sloping east: one contour per level, crossing the whole grid, open at both
    /// ends and monotone in longitude.
    #[test]
    fn a_ramp_traces_one_open_line_per_level() {
        let g = grid(8, 8, |_, c| (c as i16) * 100);
        let lines = march(&g, 250);
        assert_eq!(lines.len(), 1, "one level, one crossing: {lines:#?}");
        let line = &lines[0];
        assert_eq!(line.len(), 8, "one crossing per lattice row");
        assert!(!closed(line), "a contour leaving the grid is an open chain");
        // 250 m sits half way between the 200 m and 300 m columns, so every crossing is at the same
        // longitude — the midpoint of columns 2 and 3.
        let x = g.lon(2) + (1 << 9) / 2;
        assert!(line.iter().all(|&(lon, _)| lon == x), "the ramp's contour is a straight meridian: {line:?}");
        // And it runs the full latitude span of the grid.
        let lats: Vec<i32> = line.iter().map(|&(_, lat)| lat).collect();
        assert_eq!(lats.first().min(lats.last()), Some(&g.lat(0)).min(Some(&g.lat(7))));
    }

    /// A cone: every level below the summit closes into a ring around it.
    #[test]
    fn a_cone_traces_closed_rings() {
        let g = grid(21, 21, |r, c| {
            let (dr, dc) = (r as f64 - 10.0, c as f64 - 10.0);
            (1000.0 - 40.0 * (dr * dr + dc * dc).sqrt()) as i16
        });
        for level in [700, 800, 900] {
            let lines = march(&g, level);
            assert_eq!(lines.len(), 1, "level {level} is one ring");
            assert!(closed(&lines[0]), "level {level} must close: {:?}", lines[0]);
        }
        // Higher levels enclose less ground, which is the whole reason nesting reads as landform.
        let span = |level| {
            let l = march(&g, level).remove(0);
            let (lo, hi) = l.iter().fold((i32::MAX, i32::MIN), |(lo, hi), &(x, _)| (lo.min(x), hi.max(x)));
            hi - lo
        };
        assert!(span(900) < span(700), "a higher contour must be the smaller ring");
    }

    /// A hole voids every cell that touches it, and nothing is drawn across it — but the contour
    /// resumes on the far side rather than being lost.
    #[test]
    fn nodata_is_silence_not_a_guess() {
        let g = grid(9, 9, |r, c| if r == 4 && c == 4 { NODATA } else { (c as i16) * 100 });
        let lines = march(&g, 250);
        let vertices: usize = lines.iter().map(Vec::len).sum();
        // The 250 m contour runs up column 2..3. The hole at (4,4) is nowhere near it, so it is
        // untouched — proof that voiding is local to the cells that touch the hole.
        assert_eq!(lines.len(), 1);
        assert_eq!(vertices, 9);

        // Put the hole *on* the contour and the line is cut in two, with no vertex bridging the gap.
        let g = grid(9, 9, |r, c| if r == 4 && (c == 2 || c == 3) { NODATA } else { (c as i16) * 100 });
        let lines = march(&g, 250);
        assert_eq!(lines.len(), 2, "the contour is cut, not bridged: {lines:#?}");
        assert!(lines.iter().all(|l| l.iter().all(|&(_, lat)| lat != g.lat(4))), "no vertex sits on the voided row");
    }

    /// The saddle cases emit both segments as **separate** branches, and which pair is joined is
    /// decided by the cell mean — a saddle resolved the other way would connect two hillsides that
    /// do not meet.
    #[test]
    fn a_saddle_is_two_branches_and_the_cell_mean_picks_which() {
        // Two high quadrants meeting corner to corner. The middle cell has 100 on one diagonal and 0
        // on the other — case 5, the ambiguous one — and a cell mean of exactly 50.
        let g = grid(4, 4, |r, c| if (r < 2) == (c < 2) { 100 } else { 0 });

        // The branch that reaches the west boundary is the probe: which way it turns at the saddle
        // is exactly the pairing the cell mean chose.
        let west_branch = |level: i32| -> (i32, i32) {
            let lines = march(&g, level);
            assert_eq!(lines.len(), 2, "a saddle is two branches, never one crossing: {lines:#?}");
            assert!(lines[0].iter().all(|p| !lines[1].contains(p)), "the branches must not share a vertex: {lines:#?}");
            let west = lines
                .iter()
                .find(|l| l.iter().any(|&(lon, _)| lon == g.lon(0)))
                .expect("one branch reaches the west edge");
            west.iter().fold((i32::MAX, i32::MIN), |(lo, hi), &(_, lat)| (lo.min(lat), hi.max(lat)))
        };

        // Level 50: the cell mean is exactly 50 and therefore **not** above it, so west joins south
        // and the branch turns down to the grid's southern row.
        assert_eq!(west_branch(50).0, g.lat(0), "mean not above ⇒ west joins south");
        // Level 40: the same four corners now have a mean above the level, and the pairing flips.
        assert_eq!(west_branch(40).1, g.lat(3), "mean above ⇒ west joins north");
    }

    /// Flat ground exactly at a level produces nothing: `>` is strict on both sides, so no cell has
    /// a mixed sign and no segment is emitted. (This is what keeps a plateau from ringing.)
    #[test]
    fn a_plateau_at_the_level_traces_nothing() {
        let g = grid(6, 6, |_, _| 500);
        assert!(march(&g, 500).is_empty());
        assert!(march(&g, 400).is_empty(), "a flat plateau crosses no level at all");
    }

    /// Strips overlap by exactly one lattice row, cover the window, and never skip a cell row.
    #[test]
    fn strips_overlap_by_one_row() {
        let w = Window { i0: 100, i1: 100 + 9_999, j0: 0, j1: 999, posting_log2: 9 };
        let strips = split_strips(w);
        assert!(strips.len() > 1, "10 M samples must not be one strip");
        assert_eq!(strips[0].i0, w.i0);
        assert_eq!(strips.last().unwrap().i1, w.i1);
        for pair in strips.windows(2) {
            assert_eq!(pair[0].i1, pair[1].i0, "the shared row is traced by exactly one of the two");
        }
        // A window that fits the budget is one strip — the common case, and the one with no seams.
        let small = Window { i0: 0, i1: 500, j0: 0, j1: 500, posting_log2: 9 };
        assert_eq!(split_strips(small).len(), 1);
    }

    /// A strip trace and a whole-window trace draw the same ground: the seam splits a contour into
    /// two features that meet at a shared vertex, it does not move or drop any of it.
    #[test]
    fn a_seam_splits_a_contour_without_moving_it() {
        let whole = grid(9, 8, |r, _| (r as i16) * 100);
        let one = march(&whole, 250);
        assert_eq!(one.len(), 1);

        // The same ground in two strips sharing lattice row 4.
        let mut halves: Vec<Vec<(i32, i32)>> = Vec::new();
        for (r0, r1) in [(0usize, 4usize), (4, 8)] {
            let mut part = grid(r1 - r0 + 1, 8, |r, _| ((r + r0) as i16) * 100);
            part.window.i0 = 1000 + r0 as u32;
            part.window.i1 = 1000 + r1 as u32;
            halves.extend(march(&part, 250));
        }
        assert_eq!(halves.len(), 1, "the 250 m contour lies in the lower strip only");
        assert_eq!(halves[0], one[0], "and is traced identically there");
    }
}
