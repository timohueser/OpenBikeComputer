//! Screen-space semantic land-cover generalisation for the coarse map tiers.
//!
//! This is the production form of the interactive semantic-coverage prototype.  It deliberately
//! uses the same algorithm and constants: source polygons are sampled at 4x screen resolution,
//! accumulated into 2-pixel cells, assigned by a local Potts/coverage energy over overlapping
//! 10-cell windows, vectorised as one shared coverage, relaxed three times, and cleaned with a
//! final sub-pixel coverage VW pass.  Hydrography is a separate one-pixel mask and never enters
//! the categorical allocator.
//!
//! The grid is anchored in projected metres rather than to a viewport.  One shared ladder is built
//! for the complete extract before canonical cells are clipped from it, so adjacent output cells
//! have identical seams. The emitted geometry remains ordinary OBCM polygons; the device has no
//! semantic-grid code.

use std::collections::VecDeque;

use geos::{Geom as _, Geometry};
use obc_map_scene::M_PER_DEG;

use crate::config::Lod;
use crate::geom::{
    collect_lines, collect_polygons, coverage_is_valid, coverage_simplify_vw, from_geos, ring_to_coordseq,
    topology_preserve_simplify, try_polygon_to_geos, union_all, Geom,
};
use crate::ingest::IngestFeature;
use crate::progress::Progress;

const CLASSES: usize = 5;
const SOURCE_SCALE: usize = 4;
const CELL_PX: usize = 2;
const SUBSAMPLES: usize = SOURCE_SCALE * CELL_PX;
const WINDOW_CELLS: usize = 10;
const WINDOW_HALF: usize = WINDOW_CELLS / 2;
const ICM_PASSES: usize = 12;
const DATA_WEIGHT: f64 = 4.0;
const BOUNDARY_WEIGHT: f64 = 1.20;
const QUOTA_WEIGHT: f64 = 0.85;
const RARE_PENALTY: f64 = 8.0;
const INITIAL_VW_PX: f64 = 1.1 * CELL_PX as f64;
const SMOOTH_LIMIT_PX: f64 = 0.8;
const SMOOTH_STEP: f64 = 0.34;
const SMOOTH_PASSES: usize = 3;
const FINAL_VW_PX: f64 = 0.45;
const RASTER_TILE_CELLS: usize = 128;

/// The exact categorical order used by the prototype.  Later classes paint over earlier ones.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum SemanticClass {
    Base = 0,
    Farmland = 1,
    Grass = 2,
    Forest = 3,
    Urban = 4,
    Water = 5,
}

/// Style classification and canonical output styles derived from the typed map config.
#[derive(Clone)]
pub struct SemanticScheme {
    by_style: [Option<SemanticClass>; 255],
    output_style: [Option<u8>; 6],
}

impl SemanticScheme {
    pub(crate) fn new() -> Self {
        Self { by_style: [None; 255], output_style: [None; 6] }
    }

    pub(crate) fn insert(&mut self, style_id: u8, class: SemanticClass) {
        self.by_style[style_id as usize] = Some(class);
        let slot = &mut self.output_style[class as usize];
        *slot = Some(slot.map_or(style_id, |current| current.min(style_id)));
    }

    #[inline]
    pub fn class_of(&self, style_id: u8) -> Option<SemanticClass> {
        self.by_style.get(style_id as usize).copied().flatten()
    }

    #[inline]
    fn style_for(&self, class: SemanticClass) -> Option<u8> {
        self.output_style[class as usize]
    }
}

#[derive(Clone, Copy, Debug)]
struct Projection {
    x_scale: f64,
    y_scale: f64,
}

impl Projection {
    fn for_bbox(bbox: (i64, i64, i64, i64)) -> Self {
        let mid_lat = (bbox.1 + bbox.3) as f64 * 0.5 / 1e6;
        Self { x_scale: M_PER_DEG * mid_lat.to_radians().cos().abs().max(0.01), y_scale: M_PER_DEG }
    }

    #[inline]
    fn project(self, lon: f64, lat: f64) -> (f64, f64) {
        (lon * self.x_scale, lat * self.y_scale)
    }

    #[inline]
    fn unproject(self, x: f64, y: f64) -> (f64, f64) {
        (x / self.x_scale, y / self.y_scale)
    }
}

#[derive(Clone, Debug)]
struct Grid {
    left: f64,
    bottom: f64,
    cols: usize,
    rows: usize,
    cell_m: f64,
}

impl Grid {
    fn new(projection: Projection, bbox: (i64, i64, i64, i64), mpp: f64) -> Result<Self, String> {
        let cell_m = CELL_PX as f64 * mpp;
        if !(cell_m.is_finite() && cell_m > 0.0) {
            return Err(format!("semantic grid mpp must be finite and positive, got {mpp}"));
        }
        let (min_x, min_y) = projection.project(bbox.0 as f64 / 1e6, bbox.1 as f64 / 1e6);
        let (max_x, max_y) = projection.project(bbox.2 as f64 / 1e6, bbox.3 as f64 / 1e6);
        // Anchoring makes every cell clipped from this shared extract agree on the same grid.
        let left = (min_x / cell_m).floor() * cell_m;
        let bottom = (min_y / cell_m).floor() * cell_m;
        let cols = ((max_x - left) / cell_m).ceil().max(1.0) as usize;
        let rows = ((max_y - bottom) / cell_m).ceil().max(1.0) as usize;
        Ok(Self { left, bottom, cols, rows, cell_m })
    }

    #[inline]
    fn top(&self) -> f64 {
        self.bottom + self.rows as f64 * self.cell_m
    }

    #[inline]
    fn index(&self, x: usize, y: usize) -> usize {
        y * self.cols + x
    }

    fn cell_at(&self, x: f64, y: f64) -> Option<(usize, usize)> {
        let ix = ((x - self.left) / self.cell_m).floor() as isize;
        let iy = ((self.top() - y) / self.cell_m).floor() as isize;
        (ix >= 0 && iy >= 0 && ix < self.cols as isize && iy < self.rows as isize).then_some((ix as usize, iy as usize))
    }
}

/// The previous (finer) rung's categorical result, used as the prototype's 35% soft anchor.
#[derive(Clone)]
pub struct SemanticLabels {
    grid: Grid,
    labels: Vec<u8>,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct SemanticStats {
    pub source_polygons: usize,
    pub cells: usize,
    pub thematic_points: usize,
    pub water_points: usize,
}

pub struct SemanticLod {
    pub features: Vec<(u8, Geom)>,
    pub labels: SemanticLabels,
    pub stats: SemanticStats,
}

pub type SemanticLevels = Vec<Option<Vec<(u8, Geom)>>>;

/// Build the complete semantic ladder finer-to-coarser, preserving the prototype's 35% soft
/// anchor between adjacent rungs. The returned vector has one slot per configured LOD; ordinary
/// tiers are `None`, while an infinite fallback reuses the coarsest finite geometry verbatim.
///
/// Both the monolithic packer and the canonical-cell cutter call this function. Keeping the ladder
/// construction here prevents the two production paths from quietly acquiring different semantic
/// transitions.
pub fn build_semantic_levels(
    features: &[IngestFeature],
    lods: &[Lod],
    scheme: &SemanticScheme,
    bbox: (i64, i64, i64, i64),
    progress: &Progress,
) -> Result<SemanticLevels, String> {
    let mut levels = vec![None; lods.len()];
    let mut indices: Vec<usize> = lods
        .iter()
        .enumerate()
        .filter_map(|(index, lod)| (lod.semantic_coverage && lod.max_mpp.is_some()).then_some(index))
        .collect();
    indices.sort_by(|&a, &b| {
        lods[a].max_mpp.expect("finite semantic rung").total_cmp(&lods[b].max_mpp.expect("finite semantic rung"))
    });

    let mut prior = None;
    for &index in &indices {
        let lod = &lods[index];
        let nominal_mpp = lod.simplify_m;
        progress.stage(
            crate::progress::Phase::Quadtree,
            format!(
                "Building semantic coverage LOD {index} ({nominal_mpp} m/px grid, shown through {} m/px)...",
                lod.max_mpp.expect("finite semantic rung")
            ),
        );
        let eligible: Vec<(u8, Geom)> = features
            .iter()
            .filter(|feature| feature.min_lod <= index)
            .map(|feature| (feature.style_id, feature.geom.clone()))
            .collect();
        let level = build_semantic_lod(&eligible, scheme, bbox, nominal_mpp, prior.as_ref(), progress)?;
        progress.log(format!(
            "  semantic grid: {} source polygon(s), {} cell(s), {} thematic + {} water point(s)",
            level.stats.source_polygons, level.stats.cells, level.stats.thematic_points, level.stats.water_points
        ));
        prior = Some(level.labels);
        levels[index] = Some(level.features);
    }

    if let Some(&coarsest_finite) = indices.last() {
        let fallback = levels[coarsest_finite].clone();
        for (index, lod) in lods.iter().enumerate() {
            if lod.semantic_coverage && lod.max_mpp.is_none() {
                levels[index] = fallback.clone();
            }
        }
    }
    Ok(levels)
}

struct Source<'a> {
    class: SemanticClass,
    geom: &'a Geom,
    bounds: (f64, f64, f64, f64),
}

#[derive(Clone, Copy, Default)]
struct Support([u8; CLASSES]);

/// Build one faithful semantic rung.  `features` may contain lines and unrelated polygons; only
/// classified polygons are sampled, and only the replacement thematic/water polygons are returned.
pub fn build_semantic_lod(
    features: &[(u8, Geom)],
    scheme: &SemanticScheme,
    bbox: (i64, i64, i64, i64),
    mpp: f64,
    prior: Option<&SemanticLabels>,
    progress: &Progress,
) -> Result<SemanticLod, String> {
    let projection = Projection::for_bbox(bbox);
    let grid = Grid::new(projection, bbox, mpp)?;
    let mut sources = Vec::new();
    for (style_id, geom) in features {
        let Some(class) = scheme.class_of(*style_id) else { continue };
        if class == SemanticClass::Base || !matches!(geom, Geom::Polygon { .. } | Geom::Multi(_)) || geom.is_empty() {
            continue;
        }
        let b = geom.bounds();
        let p0 = projection.project(b.0, b.1);
        let p1 = projection.project(b.2, b.3);
        sources.push(Source { class, geom, bounds: (p0.0, p0.1, p1.0, p1.1) });
    }

    let (support, mut water) = raster_support(&sources, &grid, projection, progress)?;
    let labels = adaptive_labels(&support, &grid, prior);
    let raw = vectorize_labels(&labels, &grid)?;
    let simplified = simplify_owned_coverage(raw, INITIAL_VW_PX * mpp)?;
    let smoothed = smooth_coverage(simplified, &grid, SMOOTH_LIMIT_PX * mpp, FINAL_VW_PX * mpp)?;

    if mpp > 50.0 {
        conserve_small_water(&mut water, grid.cols * CELL_PX, grid.rows * CELL_PX);
    }
    let water_grid = Grid {
        left: grid.left,
        bottom: grid.bottom,
        cols: grid.cols * CELL_PX,
        rows: grid.rows * CELL_PX,
        cell_m: mpp,
    };
    let water_labels: Vec<u8> = water.into_iter().map(u8::from).collect();
    let mut water_polys: Vec<Geom> = vectorize_classes(&water_labels, &water_grid, 2)?
        .into_iter()
        .filter_map(|(class, geom)| (class == 1).then_some(geom))
        .collect();
    let water_tol_px = if mpp <= 50.0 {
        0.55
    } else if mpp <= 120.0 {
        1.15
    } else {
        1.4
    };
    for geom in &mut water_polys {
        *geom = topology_preserve_simplify(geom, water_tol_px * mpp);
    }

    let mut emitted_m = Vec::new();
    for (class, geom) in smoothed {
        if class == SemanticClass::Base as u8 {
            continue;
        }
        let class = match class {
            1 => SemanticClass::Farmland,
            2 => SemanticClass::Grass,
            3 => SemanticClass::Forest,
            4 => SemanticClass::Urban,
            _ => continue,
        };
        if let Some(style_id) = scheme.style_for(class) {
            emitted_m.push((style_id, geom));
        }
    }
    if let Some(style_id) = scheme.style_for(SemanticClass::Water) {
        emitted_m.extend(water_polys.into_iter().map(|geom| (style_id, geom)));
    }

    let thematic_points = emitted_m
        .iter()
        .filter(|(style, _)| scheme.class_of(*style) != Some(SemanticClass::Water))
        .map(|(_, g)| point_count(g))
        .sum();
    let water_points = emitted_m
        .iter()
        .filter(|(style, _)| scheme.class_of(*style) == Some(SemanticClass::Water))
        .map(|(_, g)| point_count(g))
        .sum();
    let features = emitted_m
        .into_iter()
        .map(|(style, geom)| (style, map_coords(geom, |x, y| projection.unproject(x, y))))
        .collect();
    let stats =
        SemanticStats { source_polygons: sources.len(), cells: grid.cols * grid.rows, thematic_points, water_points };
    Ok(SemanticLod { features, labels: SemanticLabels { grid, labels }, stats })
}

fn raster_support(
    sources: &[Source<'_>],
    grid: &Grid,
    projection: Projection,
    progress: &Progress,
) -> Result<(Vec<Support>, Vec<bool>), String> {
    let tile_cols = grid.cols.div_ceil(RASTER_TILE_CELLS);
    let tile_rows = grid.rows.div_ceil(RASTER_TILE_CELLS);
    let mut members = vec![Vec::<usize>::new(); tile_cols * tile_rows];
    let right = grid.left + grid.cols as f64 * grid.cell_m;
    let top = grid.top();
    for (index, source) in sources.iter().enumerate() {
        if source.bounds.2 < grid.left
            || source.bounds.0 > right
            || source.bounds.3 < grid.bottom
            || source.bounds.1 > top
        {
            continue;
        }
        let x0 = (((source.bounds.0 - grid.left) / grid.cell_m).floor().max(0.0) as usize / RASTER_TILE_CELLS)
            .min(tile_cols - 1);
        let x1 = (((source.bounds.2 - grid.left) / grid.cell_m).floor().max(0.0) as usize / RASTER_TILE_CELLS)
            .min(tile_cols - 1);
        let y0_cell = ((top - source.bounds.3) / grid.cell_m).floor().max(0.0) as usize;
        let y1_cell = ((top - source.bounds.1) / grid.cell_m).floor().max(0.0) as usize;
        let y0 = (y0_cell / RASTER_TILE_CELLS).min(tile_rows - 1);
        let y1 = (y1_cell / RASTER_TILE_CELLS).min(tile_rows - 1);
        for ty in y0..=y1 {
            for tx in x0..=x1 {
                members[ty * tile_cols + tx].push(index);
            }
        }
    }

    let mut support = vec![Support::default(); grid.cols * grid.rows];
    let water_cols = grid.cols * CELL_PX;
    let mut water = vec![false; water_cols * grid.rows * CELL_PX];
    for ty in 0..tile_rows {
        progress.check()?;
        for tx in 0..tile_cols {
            let cell_x0 = tx * RASTER_TILE_CELLS;
            let cell_y0 = ty * RASTER_TILE_CELLS;
            let cells_w = (grid.cols - cell_x0).min(RASTER_TILE_CELLS);
            let cells_h = (grid.rows - cell_y0).min(RASTER_TILE_CELLS);
            let width = cells_w * SUBSAMPLES;
            let height = cells_h * SUBSAMPLES;
            let mut labels = vec![0u8; width * height];
            let mut water_hi = vec![0u8; width * height];
            let tile_left = grid.left + cell_x0 as f64 * grid.cell_m;
            let tile_top = grid.top() - cell_y0 as f64 * grid.cell_m;
            let sub_m = grid.cell_m / SUBSAMPLES as f64;
            let tile_members = &members[ty * tile_cols + tx];
            for class in [SemanticClass::Farmland, SemanticClass::Grass, SemanticClass::Forest, SemanticClass::Urban] {
                for &index in tile_members {
                    let source = &sources[index];
                    if source.class == class {
                        raster_geom(
                            source.geom,
                            projection,
                            tile_left,
                            tile_top,
                            sub_m,
                            width,
                            height,
                            &mut labels,
                            class as u8,
                        );
                    }
                }
            }
            for &index in tile_members {
                let source = &sources[index];
                if source.class == SemanticClass::Water {
                    raster_geom(source.geom, projection, tile_left, tile_top, sub_m, width, height, &mut water_hi, 1);
                }
            }

            for cy in 0..cells_h {
                for cx in 0..cells_w {
                    let mut counts = [0u8; CLASSES];
                    for sy in 0..SUBSAMPLES {
                        let row = (cy * SUBSAMPLES + sy) * width + cx * SUBSAMPLES;
                        for sx in 0..SUBSAMPLES {
                            counts[labels[row + sx] as usize] += 1;
                        }
                    }
                    support[grid.index(cell_x0 + cx, cell_y0 + cy)] = Support(counts);
                    for py in 0..CELL_PX {
                        for px in 0..CELL_PX {
                            let mut wet = 0usize;
                            for sy in 0..SOURCE_SCALE {
                                let row = (cy * SUBSAMPLES + py * SOURCE_SCALE + sy) * width
                                    + cx * SUBSAMPLES
                                    + px * SOURCE_SCALE;
                                for sx in 0..SOURCE_SCALE {
                                    wet += usize::from(water_hi[row + sx] != 0);
                                }
                            }
                            // Prototype threshold: mean >= 0.18 over sixteen samples => at least 3.
                            let gx = (cell_x0 + cx) * CELL_PX + px;
                            let gy = (cell_y0 + cy) * CELL_PX + py;
                            water[gy * water_cols + gx] = wet >= 3;
                        }
                    }
                }
            }
        }
    }
    Ok((support, water))
}

#[allow(clippy::too_many_arguments)]
fn raster_geom(
    geom: &Geom,
    projection: Projection,
    tile_left: f64,
    tile_top: f64,
    sub_m: f64,
    width: usize,
    height: usize,
    target: &mut [u8],
    value: u8,
) {
    match geom {
        Geom::Polygon { exterior, interiors } => {
            raster_ring(exterior, projection, tile_left, tile_top, sub_m, width, height, target, value);
            for hole in interiors {
                raster_ring(hole, projection, tile_left, tile_top, sub_m, width, height, target, 0);
            }
        }
        Geom::Multi(parts) => {
            for part in parts {
                raster_geom(part, projection, tile_left, tile_top, sub_m, width, height, target, value);
            }
        }
        _ => {}
    }
}

#[allow(clippy::too_many_arguments)]
fn raster_ring(
    ring: &[(f64, f64)],
    projection: Projection,
    tile_left: f64,
    tile_top: f64,
    sub_m: f64,
    width: usize,
    height: usize,
    target: &mut [u8],
    value: u8,
) {
    if ring.len() < 3 {
        return;
    }
    let points: Vec<(f64, f64)> = ring
        .iter()
        .map(|&(lon, lat)| {
            let (x, y) = projection.project(lon, lat);
            ((x - tile_left) / sub_m, (tile_top - y) / sub_m)
        })
        .collect();
    let min_y = points.iter().map(|p| p.1).fold(f64::INFINITY, f64::min);
    let max_y = points.iter().map(|p| p.1).fold(f64::NEG_INFINITY, f64::max);
    let row0 = (min_y - 0.5).ceil().max(0.0) as usize;
    let row1 = (max_y - 0.5).ceil().max(0.0).min(height as f64) as usize;
    let mut crossings = Vec::with_capacity(points.len());
    for row in row0..row1 {
        let scan_y = row as f64 + 0.5;
        crossings.clear();
        for i in 0..points.len() {
            let a = points[i];
            let b = points[(i + 1) % points.len()];
            if (a.1 <= scan_y && scan_y < b.1) || (b.1 <= scan_y && scan_y < a.1) {
                crossings.push(a.0 + (scan_y - a.1) * (b.0 - a.0) / (b.1 - a.1));
            }
        }
        crossings.sort_by(f64::total_cmp);
        for pair in crossings.chunks_exact(2) {
            let x0 = (pair[0] - 0.5).ceil().max(0.0) as usize;
            let x1 = (pair[1] - 0.5).ceil().max(0.0).min(width as f64) as usize;
            if x0 < x1 {
                target[row * width + x0..row * width + x1].fill(value);
            }
        }
    }
}

#[derive(Clone)]
struct Window {
    target: [f64; CLASSES],
    counts: [f64; CLASSES],
    valid: bool,
}

struct WindowLayout {
    ox: usize,
    oy: usize,
    nx: usize,
    ny: usize,
    base: usize,
}

fn adaptive_labels(support: &[Support], grid: &Grid, prior: Option<&SemanticLabels>) -> Vec<u8> {
    let mut labels = vec![0u8; support.len()];
    for y in 0..grid.rows {
        for x in 0..grid.cols {
            let i = grid.index(x, y);
            let prior_class = prior.and_then(|p| {
                let cx = grid.left + (x as f64 + 0.5) * grid.cell_m;
                let cy = grid.top() - (y as f64 + 0.5) * grid.cell_m;
                p.grid.cell_at(cx, cy).map(|(px, py)| p.labels[p.grid.index(px, py)])
            });
            let mut best = 0usize;
            let mut best_value = f64::NEG_INFINITY;
            for class in 0..CLASSES {
                let value = evidence(support[i], class, prior_class);
                if value > best_value {
                    best = class;
                    best_value = value;
                }
            }
            labels[i] = best as u8;
        }
    }

    let mut layouts = Vec::new();
    let mut windows = Vec::new();
    for (oy, ox) in [(0, 0), (0, WINDOW_HALF), (WINDOW_HALF, 0), (WINDOW_HALF, WINDOW_HALF)] {
        let nx = grid.cols.saturating_sub(ox).div_ceil(WINDOW_CELLS);
        let ny = grid.rows.saturating_sub(oy).div_ceil(WINDOW_CELLS);
        let base = windows.len();
        windows
            .resize(windows.len() + nx * ny, Window { target: [0.0; CLASSES], counts: [0.0; CLASSES], valid: false });
        layouts.push(WindowLayout { ox, oy, nx, ny, base });
    }
    for layout in &layouts {
        for by in 0..layout.ny {
            for bx in 0..layout.nx {
                let x0 = layout.ox + bx * WINDOW_CELLS;
                let y0 = layout.oy + by * WINDOW_CELLS;
                let x1 = (x0 + WINDOW_CELLS).min(grid.cols);
                let y1 = (y0 + WINDOW_CELLS).min(grid.rows);
                let index = layout.base + by * layout.nx + bx;
                if (x1 - x0) * (y1 - y0) < 16 {
                    continue;
                }
                windows[index].valid = true;
                for y in y0..y1 {
                    for x in x0..x1 {
                        let i = grid.index(x, y);
                        for class in 0..CLASSES {
                            windows[index].target[class] += support[i].0[class] as f64 / 64.0;
                        }
                        windows[index].counts[labels[i] as usize] += 1.0;
                    }
                }
            }
        }
    }

    for _ in 0..ICM_PASSES {
        let mut changed = 0usize;
        for parity in 0..2 {
            for y in 0..grid.rows {
                for x in (((parity as isize - y as isize) & 1) as usize..grid.cols).step_by(2) {
                    let i = grid.index(x, y);
                    let old = labels[i] as usize;
                    let mut neighbours = [u8::MAX; 4];
                    let mut n_len = 0usize;
                    for candidate in [
                        x.checked_sub(1).map(|nx| grid.index(nx, y)),
                        (x + 1 < grid.cols).then(|| grid.index(x + 1, y)),
                        y.checked_sub(1).map(|ny| grid.index(x, ny)),
                        (y + 1 < grid.rows).then(|| grid.index(x, y + 1)),
                    ]
                    .into_iter()
                    .flatten()
                    {
                        neighbours[n_len] = labels[candidate];
                        n_len += 1;
                    }
                    let prior_class = prior.and_then(|p| {
                        let cx = grid.left + (x as f64 + 0.5) * grid.cell_m;
                        let cy = grid.top() - (y as f64 + 0.5) * grid.cell_m;
                        p.grid.cell_at(cx, cy).map(|(px, py)| p.labels[p.grid.index(px, py)])
                    });
                    let mut candidates = [false; CLASSES];
                    candidates[old] = true;
                    for &n in &neighbours[..n_len] {
                        candidates[n as usize] = true;
                    }
                    let mut strongest = [0usize, 1usize];
                    if evidence(support[i], strongest[1], prior_class) > evidence(support[i], strongest[0], prior_class)
                    {
                        strongest.swap(0, 1);
                    }
                    for class in 2..CLASSES {
                        let e = evidence(support[i], class, prior_class);
                        if e > evidence(support[i], strongest[0], prior_class) {
                            strongest[1] = strongest[0];
                            strongest[0] = class;
                        } else if e > evidence(support[i], strongest[1], prior_class) {
                            strongest[1] = class;
                        }
                    }
                    candidates[strongest[0]] = true;
                    candidates[strongest[1]] = true;
                    let memberships = memberships(x, y, &layouts, &windows);
                    let mut best = old;
                    let mut best_delta = 0.0;
                    for (new, &is_candidate) in candidates.iter().enumerate() {
                        if new == old || !is_candidate {
                            continue;
                        }
                        let mut delta = DATA_WEIGHT
                            * (evidence(support[i], old, prior_class) - evidence(support[i], new, prior_class));
                        let before_edges = neighbours[..n_len].iter().filter(|&&n| n as usize != old).count();
                        let after_edges = neighbours[..n_len].iter().filter(|&&n| n as usize != new).count();
                        delta += BOUNDARY_WEIGHT * (after_edges as f64 - before_edges as f64);
                        for &wi in &memberships {
                            let Some(wi) = wi else { continue };
                            let w = &windows[wi];
                            let before =
                                (w.counts[old] - w.target[old]).powi(2) + (w.counts[new] - w.target[new]).powi(2);
                            let after = (w.counts[old] - 1.0 - w.target[old]).powi(2)
                                + (w.counts[new] + 1.0 - w.target[new]).powi(2);
                            let target_sum: f64 = w.target.iter().sum();
                            delta += QUOTA_WEIGHT * (after - before) / target_sum.max(1.0);
                            if w.target[old] >= 0.8 && w.counts[old] <= 1.0 {
                                delta += RARE_PENALTY;
                            }
                        }
                        if delta < best_delta - 1e-9 {
                            best = new;
                            best_delta = delta;
                        }
                    }
                    if best != old {
                        labels[i] = best as u8;
                        for wi in memberships.into_iter().flatten() {
                            windows[wi].counts[old] -= 1.0;
                            windows[wi].counts[best] += 1.0;
                        }
                        changed += 1;
                    }
                }
            }
        }
        if changed == 0 {
            break;
        }
    }
    labels
}

#[inline]
fn evidence(support: Support, class: usize, prior: Option<u8>) -> f64 {
    let source = support.0[class] as f64 / 64.0;
    match prior {
        Some(prior) => 0.65 * source + 0.35 * f64::from(prior as usize == class),
        None => source,
    }
}

fn memberships(x: usize, y: usize, layouts: &[WindowLayout], windows: &[Window]) -> [Option<usize>; 4] {
    let mut out = [None; 4];
    for (slot, layout) in layouts.iter().enumerate() {
        if x < layout.ox || y < layout.oy {
            continue;
        }
        let bx = (x - layout.ox) / WINDOW_CELLS;
        let by = (y - layout.oy) / WINDOW_CELLS;
        if bx < layout.nx && by < layout.ny {
            let index = layout.base + by * layout.nx + bx;
            if windows[index].valid {
                out[slot] = Some(index);
            }
        }
    }
    out
}

fn vectorize_labels(labels: &[u8], grid: &Grid) -> Result<Vec<(u8, Geom)>, String> {
    vectorize_classes(labels, grid, CLASSES)
}

fn vectorize_classes(labels: &[u8], grid: &Grid, classes: usize) -> Result<Vec<(u8, Geom)>, String> {
    if labels.len() != grid.cols * grid.rows {
        return Err("semantic label grid has the wrong size".into());
    }
    let mut horizontal = vec![false; (grid.rows + 1) * grid.cols];
    let mut vertical = vec![false; grid.rows * (grid.cols + 1)];
    for y in 0..=grid.rows {
        for x in 0..grid.cols {
            horizontal[y * grid.cols + x] =
                y == 0 || y == grid.rows || labels[(y - 1) * grid.cols + x] != labels[y * grid.cols + x];
        }
    }
    for y in 0..grid.rows {
        for x in 0..=grid.cols {
            vertical[y * (grid.cols + 1) + x] =
                x == 0 || x == grid.cols || labels[y * grid.cols + x - 1] != labels[y * grid.cols + x];
        }
    }
    let mut lines = Vec::new();
    for y in 0..=grid.rows {
        let mut x = 0usize;
        while x < grid.cols {
            if !horizontal[y * grid.cols + x] {
                x += 1;
                continue;
            }
            let start = x;
            x += 1;
            while x < grid.cols && horizontal[y * grid.cols + x] {
                let junction = (y > 0 && vertical[(y - 1) * (grid.cols + 1) + x])
                    || (y < grid.rows && vertical[y * (grid.cols + 1) + x]);
                if junction {
                    break;
                }
                x += 1;
            }
            let yy = grid.top() - y as f64 * grid.cell_m;
            let coords = [(grid.left + start as f64 * grid.cell_m, yy), (grid.left + x as f64 * grid.cell_m, yy)];
            lines.push(Geometry::create_line_string(ring_to_coordseq(&coords)).map_err(|e| e.to_string())?);
        }
    }
    for x in 0..=grid.cols {
        let mut y = 0usize;
        while y < grid.rows {
            if !vertical[y * (grid.cols + 1) + x] {
                y += 1;
                continue;
            }
            let start = y;
            y += 1;
            while y < grid.rows && vertical[y * (grid.cols + 1) + x] {
                let junction =
                    (x > 0 && horizontal[y * grid.cols + x - 1]) || (x < grid.cols && horizontal[y * grid.cols + x]);
                if junction {
                    break;
                }
                y += 1;
            }
            let xx = grid.left + x as f64 * grid.cell_m;
            let coords = [(xx, grid.top() - start as f64 * grid.cell_m), (xx, grid.top() - y as f64 * grid.cell_m)];
            lines.push(Geometry::create_line_string(ring_to_coordseq(&coords)).map_err(|e| e.to_string())?);
        }
    }
    let polygonized = Geometry::polygonize(&lines).map_err(|e| format!("semantic polygonize: {e}"))?;
    let mut faces = Vec::new();
    collect_polygons(from_geos(&polygonized), &mut faces);
    let mut out = Vec::with_capacity(faces.len());
    for face in faces {
        let geos = try_polygon_to_geos(&face).ok_or("semantic polygonize emitted an invalid face")?;
        let point = geos.point_on_surface().map_err(|e| e.to_string())?;
        let (x, y) = (point.get_x().map_err(|e| e.to_string())?, point.get_y().map_err(|e| e.to_string())?);
        let (ix, iy) = grid.cell_at(x, y).ok_or("semantic face lies outside its source grid")?;
        let class = labels[grid.index(ix, iy)];
        if class as usize >= classes {
            return Err(format!("semantic face has unknown class {class}"));
        }
        out.push((class, face));
    }
    Ok(out)
}

fn simplify_owned_coverage(polys: Vec<(u8, Geom)>, tolerance: f64) -> Result<Vec<(u8, Geom)>, String> {
    let refs: Vec<&Geom> = polys.iter().map(|(_, geom)| geom).collect();
    if !coverage_is_valid(&refs, 0.0) {
        return Err("semantic vectorization did not form a valid coverage".into());
    }
    let simplified = coverage_simplify_vw(&refs, tolerance, false).ok_or("semantic coverage VW failed")?;
    let refs: Vec<&Geom> = simplified.iter().collect();
    if !coverage_is_valid(&refs, 0.0) {
        return Err("semantic coverage VW returned an invalid coverage".into());
    }
    Ok(polys.into_iter().zip(simplified).map(|((class, _), geom)| (class, geom)).collect())
}

fn smooth_coverage(
    source: Vec<(u8, Geom)>,
    grid: &Grid,
    limit: f64,
    final_tolerance: f64,
) -> Result<Vec<(u8, Geom)>, String> {
    let mut boundaries = Vec::with_capacity(source.len());
    for (_, geom) in &source {
        boundaries.push(
            try_polygon_to_geos(geom)
                .ok_or("semantic smoothing received an invalid polygon")?
                .boundary()
                .map_err(|e| e.to_string())?,
        );
    }
    let network = Geometry::create_geometry_collection(boundaries)
        .and_then(|collection| collection.unary_union())
        .and_then(|network| network.line_merge())
        .map_err(|e| format!("semantic boundary merge: {e}"))?;
    let mut chains = Vec::new();
    collect_lines(from_geos(&network), &mut chains);
    let epsilon = (grid.cols.max(grid.rows) as f64 * grid.cell_m) * 1e-10;
    let right = grid.left + grid.cols as f64 * grid.cell_m;
    let top = grid.top();
    let on_frame = |(x, y): (f64, f64)| {
        (x - grid.left).abs() <= epsilon
            || (x - right).abs() <= epsilon
            || (y - grid.bottom).abs() <= epsilon
            || (y - top).abs() <= epsilon
    };
    let mut relaxed_geos = Vec::with_capacity(chains.len());
    for chain in chains {
        let Geom::Line(original) = chain else { continue };
        if original.len() < 3 {
            relaxed_geos.push(Geometry::create_line_string(ring_to_coordseq(&original)).map_err(|e| e.to_string())?);
            continue;
        }
        let mut points = original.clone();
        let closed = distance(original[0], *original.last().expect("non-empty")) <= epsilon;
        let stop = points.len() - usize::from(closed);
        for _ in 0..SMOOTH_PASSES {
            let mut next = points.clone();
            for i in 0..stop {
                if (!closed && (i == 0 || i + 1 == stop)) || on_frame(original[i]) {
                    continue;
                }
                let prev = points[(i + stop - 1) % stop];
                let following = points[(i + 1) % stop];
                let target = ((prev.0 + following.0) * 0.5, (prev.1 + following.1) * 0.5);
                let mut candidate = (
                    points[i].0 + SMOOTH_STEP * (target.0 - points[i].0),
                    points[i].1 + SMOOTH_STEP * (target.1 - points[i].1),
                );
                let displacement = (candidate.0 - original[i].0, candidate.1 - original[i].1);
                let length = displacement.0.hypot(displacement.1);
                if length > limit {
                    candidate = (
                        original[i].0 + displacement.0 * limit / length,
                        original[i].1 + displacement.1 * limit / length,
                    );
                }
                next[i] = candidate;
            }
            if closed {
                next[stop] = next[0];
            }
            points = next;
        }
        relaxed_geos.push(Geometry::create_line_string(ring_to_coordseq(&points)).map_err(|e| e.to_string())?);
    }
    let rebuilt_network = Geometry::create_geometry_collection(relaxed_geos)
        .and_then(|collection| collection.unary_union())
        .map_err(|e| format!("semantic relaxed network: {e}"))?;
    let polygonized =
        Geometry::polygonize(&[rebuilt_network]).map_err(|e| format!("semantic relaxed polygonize: {e}"))?;
    let mut faces = Vec::new();
    collect_polygons(from_geos(&polygonized), &mut faces);

    let mut by_class: [Vec<Geom>; CLASSES] = std::array::from_fn(|_| Vec::new());
    for face in faces {
        let geos = try_polygon_to_geos(&face).ok_or("semantic smoothing emitted an invalid face")?;
        let point = geos.point_on_surface().map_err(|e| e.to_string())?;
        let mut owner = None;
        // Prototype assigns in reverse categorical paint order.
        for (class, source_geom) in source.iter().rev() {
            let candidate = try_polygon_to_geos(source_geom).ok_or("semantic source polygon became invalid")?;
            if candidate.covers(&point).map_err(|e| e.to_string())? {
                owner = Some(*class as usize);
                break;
            }
        }
        let owner = owner.ok_or("semantic smoothing could not assign a rebuilt face")?;
        by_class[owner].push(face);
    }
    let mut rebuilt = Vec::new();
    for (class, group) in by_class.iter().enumerate() {
        if group.is_empty() {
            continue;
        }
        let refs: Vec<&Geom> = group.iter().collect();
        let unioned = union_all(&refs).ok_or("semantic class dissolve failed")?;
        rebuilt.extend(unioned.into_iter().map(|geom| (class as u8, geom)));
    }
    simplify_owned_coverage(rebuilt, final_tolerance)
}

fn conserve_small_water(mask: &mut [bool], width: usize, height: usize) {
    if mask.len() != width * height {
        return;
    }
    let mut component = vec![u32::MAX; mask.len()];
    let mut sizes = Vec::<usize>::new();
    let mut queue = VecDeque::new();
    for start in 0..mask.len() {
        if !mask[start] || component[start] != u32::MAX {
            continue;
        }
        let id = sizes.len() as u32;
        component[start] = id;
        queue.push_back(start);
        let mut size = 0usize;
        while let Some(index) = queue.pop_front() {
            size += 1;
            let x = index % width;
            let y = index / width;
            for ny in y.saturating_sub(1)..=(y + 1).min(height - 1) {
                for nx in x.saturating_sub(1)..=(x + 1).min(width - 1) {
                    let ni = ny * width + nx;
                    if mask[ni] && component[ni] == u32::MAX {
                        component[ni] = id;
                        queue.push_back(ni);
                    }
                }
            }
        }
        sizes.push(size);
    }
    let large: Vec<bool> = component.iter().map(|&id| id != u32::MAX && sizes[id as usize] >= 64).collect();
    let small: Vec<bool> = mask.iter().zip(&large).map(|(&wet, &is_large)| wet && !is_large).collect();
    let mut compact = vec![false; mask.len()];
    const BLOCK: usize = 20;
    for y0 in (0..height).step_by(BLOCK) {
        for x0 in (0..width).step_by(BLOCK) {
            let y1 = (y0 + BLOCK).min(height);
            let x1 = (x0 + BLOCK).min(width);
            let mut count = 0usize;
            let (mut sum_x, mut sum_y) = (0.0, 0.0);
            for y in y0..y1 {
                for x in x0..x1 {
                    if small[y * width + x] {
                        count += 1;
                        sum_x += (x - x0) as f64;
                        sum_y += (y - y0) as f64;
                    }
                }
            }
            if count == 0 {
                continue;
            }
            let center = (sum_x / count as f64, sum_y / count as f64);
            let mut free = Vec::new();
            for y in y0..y1 {
                for x in x0..x1 {
                    let index = y * width + x;
                    if !large[index] {
                        let dx = (x - x0) as f64 - center.0;
                        let dy = (y - y0) as f64 - center.1;
                        free.push((dx * dx + dy * dy, index));
                    }
                }
            }
            free.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
            for &(_, index) in free.iter().take(count) {
                compact[index] = true;
            }
        }
    }
    for i in 0..mask.len() {
        mask[i] = large[i] || compact[i];
    }
}

fn map_coords(geom: Geom, mut f: impl FnMut(f64, f64) -> (f64, f64) + Copy) -> Geom {
    match geom {
        Geom::Line(points) => Geom::Line(points.into_iter().map(|(x, y)| f(x, y)).collect()),
        Geom::Polygon { exterior, interiors } => Geom::Polygon {
            exterior: exterior.into_iter().map(|(x, y)| f(x, y)).collect(),
            interiors: interiors.into_iter().map(|ring| ring.into_iter().map(|(x, y)| f(x, y)).collect()).collect(),
        },
        Geom::Multi(parts) => Geom::Multi(parts.into_iter().map(|part| map_coords(part, f)).collect()),
        Geom::Empty => Geom::Empty,
    }
}

fn point_count(geom: &Geom) -> usize {
    match geom {
        Geom::Line(points) => points.len().saturating_sub(usize::from(points.first() == points.last())),
        Geom::Polygon { exterior, interiors } => {
            exterior.len().saturating_sub(1) + interiors.iter().map(|ring| ring.len().saturating_sub(1)).sum::<usize>()
        }
        Geom::Multi(parts) => parts.iter().map(point_count).sum(),
        Geom::Empty => 0,
    }
}

#[inline]
fn distance(a: (f64, f64), b: (f64, f64)) -> f64 {
    (a.0 - b.0).hypot(a.1 - b.1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conservation_keeps_a_rare_supported_class() {
        let grid = Grid { left: 0.0, bottom: 0.0, cols: 10, rows: 10, cell_m: 2.0 };
        let mut support = vec![Support([64, 0, 0, 0, 0]); 100];
        support[grid.index(5, 5)] = Support([12, 0, 0, 52, 0]);
        let labels = adaptive_labels(&support, &grid, None);
        assert!(labels.contains(&(SemanticClass::Forest as u8)));
    }

    #[test]
    fn vectorized_grid_is_a_valid_shared_coverage() {
        let grid = Grid { left: 0.0, bottom: 0.0, cols: 4, rows: 3, cell_m: 10.0 };
        let labels = vec![0, 0, 3, 3, 0, 1, 3, 3, 0, 1, 1, 3];
        let polys = vectorize_labels(&labels, &grid).unwrap();
        let refs: Vec<&Geom> = polys.iter().map(|(_, geom)| geom).collect();
        assert!(coverage_is_valid(&refs, 0.0));
        assert_eq!(polys.iter().map(|(class, _)| *class).collect::<std::collections::BTreeSet<_>>().len(), 3);
    }

    #[test]
    fn water_consolidation_preserves_cell_count() {
        let mut water = vec![false; 40 * 20];
        for (x, y) in [(1, 1), (5, 3), (18, 19), (24, 4), (39, 19)] {
            water[y * 40 + x] = true;
        }
        let before = water.iter().filter(|&&v| v).count();
        conserve_small_water(&mut water, 40, 20);
        assert_eq!(water.iter().filter(|&&v| v).count(), before);
    }
}
