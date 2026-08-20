//! Screen-space semantic land-cover generalisation for the coarse map tiers.
//!
//! This is the production form of the interactive semantic-coverage prototype.  It deliberately
//! uses the same algorithm and constants: source polygons are sampled at 4x screen resolution,
//! accumulated into 2-pixel cells, assigned by a local Potts/coverage energy over overlapping
//! 10-cell windows, vectorised as one shared coverage, relaxed three times, and cleaned with a
//! final one-cell coverage VW pass.  Hydrography is a separate one-pixel mask and never enters
//! the categorical allocator.
//!
//! The grid is anchored in projected metres rather than to a viewport.  One shared ladder is built
//! for the complete extract before canonical cells are clipped from it, so adjacent output cells
//! have identical seams. The emitted geometry remains ordinary OBCM polygons; the device has no
//! semantic-grid code.

use std::collections::{BinaryHeap, HashMap, VecDeque};

use geos::{Geom as _, Geometry};
use obc_map_scene::M_PER_DEG;

use crate::config::Lod;
#[cfg(any(debug_assertions, test))]
use crate::geom::coverage_is_valid;
use crate::geom::{
    collect_polygons, coverage_simplify_vw, from_geos, ring_to_coordseq, topology_preserve_simplify,
    try_polygon_to_geos, Geom,
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
// Stay below one rendered pixel before relaxation. The final pass reaches one two-pixel semantic
// cell; its shared-graph simplifier protects planarity and minimum face size directly rather than
// weakening the approved block-scale geometry when one face is fragile.
const INITIAL_VW_PX: f64 = 0.45 * CELL_PX as f64;
const SMOOTH_LIMIT_PX: f64 = 0.8;
const SMOOTH_STEP: f64 = 0.34;
const SMOOTH_PASSES: usize = 3;
const FINAL_VW_PX: f64 = CELL_PX as f64;
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
    pub smoothed_faces: usize,
    pub smoothing_shared_edges: usize,
    pub smoothing_retries: usize,
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
            "  semantic grid: {} source polygon(s), {} cell(s), {} thematic + {} water point(s); smoothing: {} face(s), {} shared edge(s), {} retry(ies)",
            level.stats.source_polygons,
            level.stats.cells,
            level.stats.thematic_points,
            level.stats.water_points,
            level.stats.smoothed_faces,
            level.stats.smoothing_shared_edges,
            level.stats.smoothing_retries,
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
    let simplified = simplify_semantic_coverage(&raw, INITIAL_VW_PX * mpp)?;
    let (relaxed, smoothing) = smooth_coverage(simplified, &grid, SMOOTH_LIMIT_PX * mpp)?;
    let smoothed = simplify_shared_coverage(&relaxed, &grid, FINAL_VW_PX * mpp)?;

    // A sub-pixel pond is not a useful area feature at overview scale. Keep every component at
    // its real location, but remove components too small to cover a stable screen mark. The old
    // implementation preserved their aggregate area by gathering unrelated water pixels into one
    // invented block per 20×20 region; those blocks were the conspicuous square "pools" alongside
    // rivers. Rivers themselves remain independent line geometry and are never filtered here.
    if mpp > 50.0 {
        let min_pixels = if mpp > 120.0 {
            4
        } else if mpp > 80.0 {
            3
        } else {
            2
        };
        remove_tiny_water_components(&mut water, grid.cols * CELL_PX, grid.rows * CELL_PX, min_pixels);
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
    let stats = SemanticStats {
        source_polygons: sources.len(),
        cells: grid.cols * grid.rows,
        thematic_points,
        water_points,
        smoothed_faces: smoothing.faces,
        smoothing_shared_edges: smoothing.shared_edges,
        smoothing_retries: smoothing.retries,
    };
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
            raster_polygon(exterior, interiors, projection, tile_left, tile_top, sub_m, width, height, target, value);
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
fn raster_polygon(
    exterior: &[(f64, f64)],
    interiors: &[Vec<(f64, f64)>],
    projection: Projection,
    tile_left: f64,
    tile_top: f64,
    sub_m: f64,
    width: usize,
    height: usize,
    target: &mut [u8],
    value: u8,
) {
    if exterior.len() < 3 {
        return;
    }
    let rings: Vec<Vec<(f64, f64)>> = std::iter::once(exterior)
        .chain(interiors.iter().map(Vec::as_slice))
        .filter(|ring| ring.len() >= 3)
        .map(|ring| {
            ring.iter()
                .map(|&(lon, lat)| {
                    let (x, y) = projection.project(lon, lat);
                    ((x - tile_left) / sub_m, (tile_top - y) / sub_m)
                })
                .collect()
        })
        .collect();
    let min_y = rings.iter().flatten().map(|p| p.1).fold(f64::INFINITY, f64::min);
    let max_y = rings.iter().flatten().map(|p| p.1).fold(f64::NEG_INFINITY, f64::max);
    let row0 = (min_y - 0.5).ceil().max(0.0) as usize;
    let row1 = (max_y - 0.5).ceil().max(0.0).min(height as f64) as usize;
    let mut crossings = Vec::with_capacity(rings.iter().map(Vec::len).sum());
    for row in row0..row1 {
        let scan_y = row as f64 + 0.5;
        crossings.clear();
        for points in &rings {
            for i in 0..points.len() {
                let a = points[i];
                let b = points[(i + 1) % points.len()];
                if (a.1 <= scan_y && scan_y < b.1) || (b.1 <= scan_y && scan_y < a.1) {
                    crossings.push(a.0 + (scan_y - a.1) * (b.0 - a.0) / (b.1 - a.1));
                }
            }
        }
        crossings.sort_by(f64::total_cmp);
        for pair in crossings.as_chunks::<2>().0 {
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
            let prior_class = prior_class_at(prior, grid, x, y);
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
                    let prior_class = prior_class_at(prior, grid, x, y);
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

fn prior_class_at(prior: Option<&SemanticLabels>, grid: &Grid, x: usize, y: usize) -> Option<u8> {
    let prior = prior?;
    let cx = grid.left + (x as f64 + 0.5) * grid.cell_m;
    let cy = grid.top() - (y as f64 + 0.5) * grid.cell_m;
    prior.grid.cell_at(cx, cy).map(|(px, py)| prior.labels[prior.grid.index(px, py)])
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

fn simplify_owned_coverage(polys: &[(u8, Geom)], tolerance: f64) -> Result<Vec<(u8, Geom)>, String> {
    let refs: Vec<&Geom> = polys.iter().map(|(_, geom)| geom).collect();
    // `vectorize_classes` polygonizes one shared line network, so its faces are a coverage by
    // construction. Validating that dense, unsimplified grid here made a country-scale bake spend
    // minutes intersecting millions of redundant collinear segments. Retain the expensive audit in
    // tests/debug builds, while production relies on the construction invariant.
    #[cfg(debug_assertions)]
    if !coverage_is_valid(&refs, 0.0) {
        return Err("semantic vectorization did not form a valid coverage".into());
    }
    let simplified = coverage_simplify_vw(&refs, tolerance, false).ok_or("semantic coverage VW failed")?;
    // GEOSCoverageSimplifyVW preserves the input coverage by contract. Audit that contract in
    // tests/debug builds without making every production bake re-run GEOS's global segment
    // intersection machinery over the complete extract.
    #[cfg(debug_assertions)]
    {
        let refs: Vec<&Geom> = simplified.iter().collect();
        if !coverage_is_valid(&refs, 0.0) {
            return Err("semantic coverage VW returned an invalid coverage".into());
        }
    }
    Ok(polys.iter().zip(simplified).map(|((class, _), geom)| (*class, geom)).collect())
}

/// Simplify the shared coverage without ever letting an orthogonal raster face collapse to three
/// corners. Triangles cannot originate in `vectorize_classes`: every non-empty grid component has
/// at least four turns, and smoothing moves those vertices without deleting them. If GEOS removes
/// one anyway, retry the complete coverage at a lower tolerance so both copies of every shared edge
/// still move together. The unsimplified input is the final, topology-safe fallback.
fn simplify_semantic_coverage(polys: &[(u8, Geom)], tolerance: f64) -> Result<Vec<(u8, Geom)>, String> {
    if polys.iter().any(|(_, geom)| has_triangular_face(geom)) {
        return Err("semantic coverage contains a triangular face before simplification".into());
    }
    let mut attempt = tolerance;
    for _ in 0..8 {
        let simplified = simplify_owned_coverage(polys, attempt)?;
        let triangles = simplified.iter().map(|(_, geom)| triangular_face_count(geom)).sum::<usize>();
        if triangles == 0 {
            return Ok(simplified);
        }
        attempt *= 0.75;
    }
    Ok(polys.to_vec())
}

fn has_triangular_face(geom: &Geom) -> bool {
    triangular_face_count(geom) != 0
}

fn triangular_face_count(geom: &Geom) -> usize {
    match geom {
        Geom::Polygon { exterior, .. } => {
            usize::from(exterior.len().saturating_sub(usize::from(exterior.first() == exterior.last())) == 3)
        }
        Geom::Multi(parts) => parts.iter().map(triangular_face_count).sum(),
        Geom::Line(_) | Geom::Empty => 0,
    }
}

#[derive(Debug, Default, Clone, Copy)]
struct SmoothStats {
    faces: usize,
    shared_edges: usize,
    retries: usize,
}

type PointKey = (u64, u64);

#[derive(Debug, Clone)]
struct SharedVertex {
    original: (f64, f64),
    current: (f64, f64),
    neighbours: Vec<PointKey>,
}

fn point_key((x, y): (f64, f64)) -> PointKey {
    // GEOS can turn +0 into -0 while preserving the same coordinate. Canonicalize the only two
    // distinct bit patterns that compare equal as floats; every other finite coordinate is exact.
    (if x == 0.0 { 0 } else { x.to_bits() }, if y == 0.0 { 0 } else { y.to_bits() })
}

fn ring_vertices(ring: &[(f64, f64)]) -> &[(f64, f64)] {
    if ring.len() > 1 && ring.first() == ring.last() {
        &ring[..ring.len() - 1]
    } else {
        ring
    }
}

fn insert_ring_graph(ring: &[(f64, f64)], graph: &mut HashMap<PointKey, SharedVertex>) {
    let ring = ring_vertices(ring);
    for (a, b) in ring.iter().copied().zip(ring.iter().copied().cycle().skip(1)).take(ring.len()) {
        if a == b {
            continue;
        }
        let ak = point_key(a);
        let bk = point_key(b);
        let av = graph.entry(ak).or_insert_with(|| SharedVertex { original: a, current: a, neighbours: Vec::new() });
        if !av.neighbours.contains(&bk) {
            av.neighbours.push(bk);
        }
        let bv = graph.entry(bk).or_insert_with(|| SharedVertex { original: b, current: b, neighbours: Vec::new() });
        if !bv.neighbours.contains(&ak) {
            bv.neighbours.push(ak);
        }
    }
}

fn insert_geom_graph(geom: &Geom, graph: &mut HashMap<PointKey, SharedVertex>) {
    match geom {
        Geom::Polygon { exterior, interiors } => {
            insert_ring_graph(exterior, graph);
            for hole in interiors {
                insert_ring_graph(hole, graph);
            }
        }
        Geom::Multi(parts) => {
            for part in parts {
                insert_geom_graph(part, graph);
            }
        }
        Geom::Line(_) | Geom::Empty => {}
    }
}

fn map_shared_geom(geom: &Geom, graph: &HashMap<PointKey, SharedVertex>) -> Geom {
    match geom {
        Geom::Line(points) => Geom::Line(points.iter().map(|&point| graph[&point_key(point)].current).collect()),
        Geom::Polygon { exterior, interiors } => Geom::Polygon {
            exterior: exterior.iter().map(|&point| graph[&point_key(point)].current).collect(),
            interiors: interiors
                .iter()
                .map(|ring| ring.iter().map(|&point| graph[&point_key(point)].current).collect())
                .collect(),
        },
        Geom::Multi(parts) => Geom::Multi(parts.iter().map(|part| map_shared_geom(part, graph)).collect()),
        Geom::Empty => Geom::Empty,
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct OrdF64(f64);

impl Eq for OrdF64 {}

impl PartialOrd for OrdF64 {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for OrdF64 {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.0.total_cmp(&other.0)
    }
}

#[derive(Debug, Clone)]
struct SimplifyVertex {
    point: (f64, f64),
    neighbours: Vec<PointKey>,
    active: bool,
    revision: u32,
}

fn collect_ring_keys(geom: &Geom, rings: &mut Vec<Vec<PointKey>>) {
    match geom {
        Geom::Polygon { exterior, interiors } => {
            rings.push(ring_vertices(exterior).iter().copied().map(point_key).collect());
            for ring in interiors {
                rings.push(ring_vertices(ring).iter().copied().map(point_key).collect());
            }
        }
        Geom::Multi(parts) => {
            for part in parts {
                collect_ring_keys(part, rings);
            }
        }
        Geom::Line(_) | Geom::Empty => {}
    }
}

fn simplified_ring(ring: &[(f64, f64)], graph: &HashMap<PointKey, SimplifyVertex>) -> Vec<(f64, f64)> {
    let closed = ring.len() > 1 && ring.first() == ring.last();
    let mut out: Vec<_> =
        ring_vertices(ring).iter().copied().filter(|&point| graph[&point_key(point)].active).collect();
    if closed && !out.is_empty() {
        out.push(out[0]);
    }
    out
}

fn map_simplified_geom(geom: &Geom, graph: &HashMap<PointKey, SimplifyVertex>) -> Geom {
    match geom {
        Geom::Polygon { exterior, interiors } => Geom::Polygon {
            exterior: simplified_ring(exterior, graph),
            interiors: interiors.iter().map(|ring| simplified_ring(ring, graph)).collect(),
        },
        Geom::Multi(parts) => Geom::Multi(parts.iter().map(|part| map_simplified_geom(part, graph)).collect()),
        Geom::Line(points) => Geom::Line(simplified_ring(points, graph)),
        Geom::Empty => Geom::Empty,
    }
}

fn vertex_error(graph: &HashMap<PointKey, SimplifyVertex>, key: PointKey) -> Option<f64> {
    let vertex = graph.get(&key)?;
    if !vertex.active || vertex.neighbours.len() != 2 {
        return None;
    }
    let a = graph.get(&vertex.neighbours[0])?;
    let b = graph.get(&vertex.neighbours[1])?;
    if !a.active || !b.active || vertex.neighbours[0] == vertex.neighbours[1] {
        return None;
    }
    let base = (b.point.0 - a.point.0).hypot(b.point.1 - a.point.1);
    if base == 0.0 {
        return None;
    }
    let twice_area = ((vertex.point.0 - a.point.0) * (b.point.1 - a.point.1)
        - (vertex.point.1 - a.point.1) * (b.point.0 - a.point.0))
        .abs();
    Some(twice_area / base)
}

fn replace_neighbour(neighbours: &mut [PointKey], old: PointKey, new: PointKey) {
    let index = neighbours.iter().position(|&candidate| candidate == old).expect("shared graph edge is symmetric");
    neighbours[index] = new;
}

#[derive(Clone, Copy)]
struct PlanarSegment {
    a_key: PointKey,
    b_key: PointKey,
    a: (f64, f64),
    b: (f64, f64),
}

fn cross(a: (f64, f64), b: (f64, f64), c: (f64, f64)) -> f64 {
    (b.0 - a.0) * (c.1 - a.1) - (b.1 - a.1) * (c.0 - a.0)
}

fn point_on_segment(point: (f64, f64), a: (f64, f64), b: (f64, f64), epsilon: f64) -> bool {
    cross(a, b, point).abs() <= epsilon
        && point.0 >= a.0.min(b.0) - epsilon
        && point.0 <= a.0.max(b.0) + epsilon
        && point.1 >= a.1.min(b.1) - epsilon
        && point.1 <= a.1.max(b.1) + epsilon
}

fn segments_conflict(first: PlanarSegment, second: PlanarSegment, epsilon: f64) -> bool {
    let shared = if first.a_key == second.a_key || first.a_key == second.b_key {
        Some((first.a_key, first.b))
    } else if first.b_key == second.a_key || first.b_key == second.b_key {
        Some((first.b_key, first.a))
    } else {
        None
    };
    if let Some((shared_key, first_other)) = shared {
        let shared_point = if first.a_key == shared_key { first.a } else { first.b };
        let second_other = if second.a_key == shared_key { second.b } else { second.a };
        // Ordinary graph edges may meet at any angle. Collinear edges on the same ray overlap for
        // a non-zero length, however, which means a removed chord skipped a live vertex.
        if cross(shared_point, first_other, second_other).abs() > epsilon {
            return false;
        }
        let first_vec = (first_other.0 - shared_point.0, first_other.1 - shared_point.1);
        let second_vec = (second_other.0 - shared_point.0, second_other.1 - shared_point.1);
        return first_vec.0 * second_vec.0 + first_vec.1 * second_vec.1 > epsilon;
    }

    let bbox_overlaps = first.a.0.min(first.b.0) <= second.a.0.max(second.b.0) + epsilon
        && first.a.0.max(first.b.0) + epsilon >= second.a.0.min(second.b.0)
        && first.a.1.min(first.b.1) <= second.a.1.max(second.b.1) + epsilon
        && first.a.1.max(first.b.1) + epsilon >= second.a.1.min(second.b.1);
    if !bbox_overlaps {
        return false;
    }
    let (o1, o2) = (cross(first.a, first.b, second.a), cross(first.a, first.b, second.b));
    let (o3, o4) = (cross(second.a, second.b, first.a), cross(second.a, second.b, first.b));
    (o1 > epsilon && o2 < -epsilon || o1 < -epsilon && o2 > epsilon)
        && (o3 > epsilon && o4 < -epsilon || o3 < -epsilon && o4 > epsilon)
        || o1.abs() <= epsilon && point_on_segment(second.a, first.a, first.b, epsilon)
        || o2.abs() <= epsilon && point_on_segment(second.b, first.a, first.b, epsilon)
        || o3.abs() <= epsilon && point_on_segment(first.a, second.a, second.b, epsilon)
        || o4.abs() <= epsilon && point_on_segment(first.b, second.a, second.b, epsilon)
}

struct SegmentIndex {
    bucket_m: f64,
    bucket_cols: usize,
    bucket_rows: usize,
    left: f64,
    bottom: f64,
    epsilon: f64,
    buckets: Vec<Vec<usize>>,
    segments: Vec<PlanarSegment>,
    seen: Vec<usize>,
    stamp: usize,
}

impl SegmentIndex {
    const BUCKET_CELLS: usize = 8;

    fn new(graph: &HashMap<PointKey, SimplifyVertex>, grid: &Grid) -> Self {
        let mut index = Self {
            bucket_m: grid.cell_m * Self::BUCKET_CELLS as f64,
            bucket_cols: grid.cols.div_ceil(Self::BUCKET_CELLS).max(1),
            bucket_rows: grid.rows.div_ceil(Self::BUCKET_CELLS).max(1),
            left: grid.left,
            bottom: grid.bottom,
            epsilon: grid.cell_m * grid.cell_m * 1e-10,
            buckets: vec![
                Vec::new();
                grid.cols.div_ceil(Self::BUCKET_CELLS).max(1) * grid.rows.div_ceil(Self::BUCKET_CELLS).max(1)
            ],
            segments: Vec::new(),
            seen: Vec::new(),
            stamp: 0,
        };
        for (&a_key, vertex) in graph {
            for &b_key in &vertex.neighbours {
                if a_key < b_key {
                    index.insert(PlanarSegment { a_key, b_key, a: vertex.point, b: graph[&b_key].point });
                }
            }
        }
        index
    }

    fn bucket_x(&self, x: f64) -> usize {
        (((x - self.left) / self.bucket_m).floor() as isize).clamp(0, self.bucket_cols as isize - 1) as usize
    }

    fn bucket_y(&self, y: f64) -> usize {
        (((y - self.bottom) / self.bucket_m).floor() as isize).clamp(0, self.bucket_rows as isize - 1) as usize
    }

    fn bucket_bounds(&self, segment: PlanarSegment) -> (usize, usize, usize, usize) {
        (
            self.bucket_x(segment.a.0.min(segment.b.0)),
            self.bucket_x(segment.a.0.max(segment.b.0)),
            self.bucket_y(segment.a.1.min(segment.b.1)),
            self.bucket_y(segment.a.1.max(segment.b.1)),
        )
    }

    fn insert(&mut self, segment: PlanarSegment) {
        let (x0, x1, y0, y1) = self.bucket_bounds(segment);
        let id = self.segments.len();
        self.segments.push(segment);
        self.seen.push(0);
        for y in y0..=y1 {
            for x in x0..=x1 {
                self.buckets[y * self.bucket_cols + x].push(id);
            }
        }
    }

    /// Whether a proposed replacement chord would stop the current active graph being planar.
    /// Records for superseded edges stay in the buckets, but the live graph makes them free to
    /// discard here; this keeps updates O(1) and avoids a second mutable spatial data structure.
    fn conflicts(
        &mut self,
        candidate: PlanarSegment,
        removed: PointKey,
        graph: &HashMap<PointKey, SimplifyVertex>,
    ) -> bool {
        self.stamp = self.stamp.wrapping_add(1).max(1);
        if self.stamp == 1 {
            self.seen.fill(0);
        }
        let (x0, x1, y0, y1) = self.bucket_bounds(candidate);
        for y in y0..=y1 {
            for x in x0..=x1 {
                for &id in &self.buckets[y * self.bucket_cols + x] {
                    if self.seen[id] == self.stamp {
                        continue;
                    }
                    self.seen[id] = self.stamp;
                    let edge = self.segments[id];
                    if edge.a_key == removed || edge.b_key == removed {
                        continue;
                    }
                    let Some(a) = graph.get(&edge.a_key) else { continue };
                    let Some(b) = graph.get(&edge.b_key) else { continue };
                    if !a.active || !b.active || !a.neighbours.contains(&edge.b_key) {
                        continue;
                    }
                    if segments_conflict(candidate, edge, self.epsilon) {
                        return true;
                    }
                }
            }
        }
        false
    }
}

/// A shared boundary remains a valid categorical coverage exactly while its active graph stays
/// planar. Check that directly with a coarse uniform index: every unique edge is compared only to
/// edges whose bounding boxes touch the same eight-cell bucket. This catches chord crossings,
/// T-junctions, and overlaps without GEOS rebuilding and cross-validating every polygon pair.
fn shared_graph_is_planar(graph: &HashMap<PointKey, SimplifyVertex>, grid: &Grid) -> bool {
    let mut index = SegmentIndex {
        bucket_m: grid.cell_m * SegmentIndex::BUCKET_CELLS as f64,
        bucket_cols: grid.cols.div_ceil(SegmentIndex::BUCKET_CELLS).max(1),
        bucket_rows: grid.rows.div_ceil(SegmentIndex::BUCKET_CELLS).max(1),
        left: grid.left,
        bottom: grid.bottom,
        epsilon: grid.cell_m * grid.cell_m * 1e-10,
        buckets: vec![
            Vec::new();
            grid.cols.div_ceil(SegmentIndex::BUCKET_CELLS).max(1)
                * grid.rows.div_ceil(SegmentIndex::BUCKET_CELLS).max(1)
        ],
        segments: Vec::new(),
        seen: Vec::new(),
        stamp: 0,
    };
    for (&a_key, vertex) in graph {
        if !vertex.active {
            continue;
        }
        for &b_key in &vertex.neighbours {
            if a_key >= b_key || !graph[&b_key].active {
                continue;
            }
            let segment = PlanarSegment { a_key, b_key, a: vertex.point, b: graph[&b_key].point };
            if index.conflicts(segment, (u64::MAX, u64::MAX), graph) {
                return false;
            }
            index.insert(segment);
        }
    }
    true
}

/// Simplify the already-smoothed categorical coverage directly on its shared planar graph.
///
/// GEOS coverage VW is topology-safe but permits a four-corner raster face to become a triangle.
/// Retrying the *entire* country at a lower tolerance protected that face at the cost of roughly
/// half again as many points everywhere else. Here a degree-two vertex is removed once globally,
/// so both rings sharing an edge receive the identical chord. Junctions and the canonical frame
/// are pinned, and every ring keeps at least four distinct vertices. A spatially indexed planarity
/// audit rejects crossings, overlaps, and T-junctions; debug builds additionally cross-check the
/// accepted graph with GEOS's independent polygon and coverage validators.
fn simplify_shared_coverage(source: &[(u8, Geom)], grid: &Grid, tolerance: f64) -> Result<Vec<(u8, Geom)>, String> {
    let mut shared = HashMap::new();
    for (_, geom) in source {
        insert_geom_graph(geom, &mut shared);
    }

    let mut rings = Vec::new();
    for (_, geom) in source {
        collect_ring_keys(geom, &mut rings);
    }
    let mut memberships: HashMap<PointKey, Vec<usize>> = HashMap::new();
    for (ring_id, ring) in rings.iter().enumerate() {
        for &key in ring {
            let owners = memberships.entry(key).or_default();
            if !owners.contains(&ring_id) {
                owners.push(ring_id);
            }
        }
    }

    let epsilon = (grid.cols.max(grid.rows) as f64 * grid.cell_m) * 1e-10;
    let right = grid.left + grid.cols as f64 * grid.cell_m;
    let top = grid.top();
    let on_frame = |(x, y): (f64, f64)| {
        (x - grid.left).abs() <= epsilon
            || (x - right).abs() <= epsilon
            || (y - grid.bottom).abs() <= epsilon
            || (y - top).abs() <= epsilon
    };

    let mut graph: HashMap<_, _> = shared
        .iter()
        .map(|(&key, vertex)| {
            (
                key,
                SimplifyVertex {
                    point: vertex.original,
                    neighbours: vertex.neighbours.clone(),
                    active: true,
                    revision: 0,
                },
            )
        })
        .collect();
    let mut edge_index = SegmentIndex::new(&graph, grid);
    let mut ring_counts: Vec<_> = rings.iter().map(Vec::len).collect();
    let mut heap = BinaryHeap::new();
    for (&key, vertex) in &graph {
        if vertex.neighbours.len() == 2 && !on_frame(vertex.point) {
            if let Some(error) = vertex_error(&graph, key) {
                heap.push(std::cmp::Reverse((OrdF64(error), key, vertex.revision)));
            }
        }
    }

    while let Some(std::cmp::Reverse((OrdF64(error), key, revision))) = heap.pop() {
        if error > tolerance {
            break;
        }
        let Some(vertex) = graph.get(&key) else { continue };
        if !vertex.active || vertex.revision != revision || vertex.neighbours.len() != 2 {
            continue;
        }
        let Some(current_error) = vertex_error(&graph, key) else { continue };
        if current_error != error {
            continue;
        }
        let owners = memberships.get(&key).map(Vec::as_slice).unwrap_or(&[]);
        if owners.iter().any(|&ring_id| ring_counts[ring_id] <= 4) {
            continue;
        }
        let [a, b] = [vertex.neighbours[0], vertex.neighbours[1]];
        if graph[&a].neighbours.contains(&b) || graph[&b].neighbours.contains(&a) {
            continue;
        }
        let chord = PlanarSegment { a_key: a, b_key: b, a: graph[&a].point, b: graph[&b].point };
        if edge_index.conflicts(chord, key, &graph) {
            continue;
        }

        graph.get_mut(&key).expect("candidate exists").active = false;
        for &ring_id in owners {
            ring_counts[ring_id] -= 1;
        }
        replace_neighbour(&mut graph.get_mut(&a).expect("first neighbour exists").neighbours, key, b);
        replace_neighbour(&mut graph.get_mut(&b).expect("second neighbour exists").neighbours, key, a);
        edge_index.insert(chord);
        for neighbour in [a, b] {
            let node = graph.get_mut(&neighbour).expect("updated neighbour exists");
            node.revision = node.revision.wrapping_add(1);
            if node.neighbours.len() == 2 && !on_frame(node.point) {
                let revision = node.revision;
                if let Some(error) = vertex_error(&graph, neighbour) {
                    heap.push(std::cmp::Reverse((OrdF64(error), neighbour, revision)));
                }
            }
        }
    }

    let planar = shared_graph_is_planar(&graph, grid);
    let simplified: Vec<_> = source.iter().map(|(class, geom)| (*class, map_simplified_geom(geom, &graph))).collect();
    #[cfg(debug_assertions)]
    if planar {
        let refs: Vec<_> = simplified.iter().map(|(_, geom)| geom).collect();
        debug_assert!(
            simplified.iter().all(|(_, geom)| polygonal_geom_is_valid(geom)) && coverage_is_valid(&refs, 0.0),
            "planar shared-graph simplification must remain a valid coverage"
        );
    }
    if planar {
        return Ok(simplified);
    }

    Ok(source.to_vec())
}

fn polygonal_geom_is_valid(geom: &Geom) -> bool {
    try_polygon_to_geos(geom).is_some_and(|geos| geos.is_valid().unwrap_or(false))
}

fn smooth_coverage(source: Vec<(u8, Geom)>, grid: &Grid, limit: f64) -> Result<(Vec<(u8, Geom)>, SmoothStats), String> {
    let mut graph = HashMap::new();
    for (_, geom) in &source {
        insert_geom_graph(geom, &mut graph);
    }
    let shared_edges = graph.values().map(|vertex| vertex.neighbours.len()).sum::<usize>() / 2;
    let epsilon = (grid.cols.max(grid.rows) as f64 * grid.cell_m) * 1e-10;
    let right = grid.left + grid.cols as f64 * grid.cell_m;
    let top = grid.top();
    let on_frame = |(x, y): (f64, f64)| {
        (x - grid.left).abs() <= epsilon
            || (x - right).abs() <= epsilon
            || (y - grid.bottom).abs() <= epsilon
            || (y - top).abs() <= epsilon
    };

    // The GEOS implementation first unioned every boundary, line-merged the result, moved each
    // degree-two chain vertex, unioned again, and polygonized. The source is already a valid shared
    // coverage: doing the same Laplacian update directly on its unique vertex graph preserves both
    // copies of every edge and keeps each class attached to its face. No regional overlay is needed.
    for attempt in 0..4 {
        let attempt_limit = limit * 0.5f64.powi(attempt as i32);
        for vertex in graph.values_mut() {
            vertex.current = vertex.original;
        }
        for _ in 0..SMOOTH_PASSES {
            let current: HashMap<_, _> = graph.iter().map(|(&key, vertex)| (key, vertex.current)).collect();
            for vertex in graph.values_mut() {
                // Degree != 2 is a chain endpoint or junction. Moving it would detach incident
                // chains; frame vertices likewise define the exact coverage extent.
                if vertex.neighbours.len() != 2 || on_frame(vertex.original) {
                    continue;
                }
                let prev = current[&vertex.neighbours[0]];
                let following = current[&vertex.neighbours[1]];
                let target = ((prev.0 + following.0) * 0.5, (prev.1 + following.1) * 0.5);
                let mut candidate = (
                    vertex.current.0 + SMOOTH_STEP * (target.0 - vertex.current.0),
                    vertex.current.1 + SMOOTH_STEP * (target.1 - vertex.current.1),
                );
                let displacement = (candidate.0 - vertex.original.0, candidate.1 - vertex.original.1);
                let length = displacement.0.hypot(displacement.1);
                if length > attempt_limit {
                    candidate = (
                        vertex.original.0 + displacement.0 * attempt_limit / length,
                        vertex.original.1 + displacement.1 * attempt_limit / length,
                    );
                }
                vertex.current = candidate;
            }
        }
        let moved: Vec<_> = source.iter().map(|(class, geom)| (*class, map_shared_geom(geom, &graph))).collect();
        if moved.iter().all(|(_, geom)| polygonal_geom_is_valid(geom)) {
            return Ok((moved, SmoothStats { faces: source.len(), shared_edges, retries: attempt }));
        }
    }

    // Smoothing is cosmetic. If even a one-eighth displacement makes a face invalid, retain the
    // already-valid shared coverage rather than polygonizing a crossed network and guessing labels.
    let faces = source.len();
    Ok((source, SmoothStats { faces, shared_edges, retries: 4 }))
}

fn remove_tiny_water_components(mask: &mut [bool], width: usize, height: usize, min_pixels: usize) {
    if mask.len() != width * height {
        return;
    }
    let mut seen = vec![false; mask.len()];
    let mut queue = VecDeque::new();
    for start in 0..mask.len() {
        if !mask[start] || seen[start] {
            continue;
        }
        seen[start] = true;
        queue.push_back(start);
        let mut members = Vec::new();
        while let Some(index) = queue.pop_front() {
            members.push(index);
            let x = index % width;
            let y = index / width;
            for ny in y.saturating_sub(1)..=(y + 1).min(height - 1) {
                for nx in x.saturating_sub(1)..=(x + 1).min(width - 1) {
                    let ni = ny * width + nx;
                    if mask[ni] && !seen[ni] {
                        seen[ni] = true;
                        queue.push_back(ni);
                    }
                }
            }
        }
        if members.len() < min_pixels {
            for index in members {
                mask[index] = false;
            }
        }
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
    fn overlapping_polygon_hole_does_not_erase_prior_coverage() {
        let projection = Projection { x_scale: 1.0, y_scale: 1.0 };
        let solid = Geom::Polygon { exterior: vec![(1.0, 1.0), (7.0, 1.0), (7.0, 7.0), (1.0, 7.0)], interiors: vec![] };
        let with_hole = Geom::Polygon {
            exterior: vec![(2.0, 2.0), (6.0, 2.0), (6.0, 6.0), (2.0, 6.0)],
            interiors: vec![vec![(3.0, 3.0), (5.0, 3.0), (5.0, 5.0), (3.0, 5.0)]],
        };

        let mut hole_only = [0u8; 8 * 8];
        raster_geom(&with_hole, projection, 0.0, 8.0, 1.0, 8, 8, &mut hole_only, 1);
        assert_eq!(hole_only[4 * 8 + 4], 0, "a standalone polygon keeps its hole");

        let mut overlap = [0u8; 8 * 8];
        raster_geom(&solid, projection, 0.0, 8.0, 1.0, 8, 8, &mut overlap, 1);
        raster_geom(&with_hole, projection, 0.0, 8.0, 1.0, 8, 8, &mut overlap, 1);
        assert_eq!(overlap[4 * 8 + 4], 1, "a later polygon's hole cannot erase an earlier polygon");
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
    fn coverage_tolerance_does_not_turn_diagonal_cells_into_triangles() {
        let grid = Grid { left: 0.0, bottom: 0.0, cols: 31, rows: 31, cell_m: 2.0 };
        let forest = SemanticClass::Forest as u8;
        let base = SemanticClass::Base as u8;
        let mut labels = vec![forest; grid.cols * grid.rows];
        for y in 0..24 {
            labels[grid.index(5 + y / 2, y)] = base;
        }
        for y in 20..27 {
            for x in 12..=20 {
                labels[grid.index(x, y)] = base;
            }
        }

        let raw = vectorize_labels(&labels, &grid).unwrap();
        let simplified = simplify_semantic_coverage(&raw, INITIAL_VW_PX).unwrap();
        let (relaxed, _) = smooth_coverage(simplified, &grid, SMOOTH_LIMIT_PX).unwrap();
        let unsafe_faces = simplify_owned_coverage(&relaxed, CELL_PX as f64).unwrap();
        let safe_faces = simplify_semantic_coverage(&relaxed, CELL_PX as f64).unwrap();
        let shared_faces = simplify_shared_coverage(&relaxed, &grid, CELL_PX as f64).unwrap();
        let exterior_lengths = |faces: &[(u8, Geom)]| {
            faces
                .iter()
                .filter_map(|(class, geom)| match geom {
                    Geom::Polygon { exterior, .. } if *class == base => Some(exterior.len()),
                    _ => None,
                })
                .collect::<Vec<_>>()
        };
        assert!(exterior_lengths(&unsafe_faces).contains(&4), "the fixture must exercise the old triangle collapse");
        assert!(
            exterior_lengths(&safe_faces).into_iter().all(|vertices| vertices >= 5),
            "every raster cell keeps at least four distinct corners"
        );
        assert!(
            exterior_lengths(&shared_faces).into_iter().all(|vertices| vertices >= 5),
            "shared-graph simplification keeps every raster face non-triangular"
        );
        let refs: Vec<_> = shared_faces.iter().map(|(_, geom)| geom).collect();
        assert!(coverage_is_valid(&refs, 0.0));
        assert!(shared_faces.iter().all(|(_, geom)| polygonal_geom_is_valid(geom)));
        assert!(
            shared_faces.iter().map(|(_, geom)| point_count(geom)).sum::<usize>()
                <= safe_faces.iter().map(|(_, geom)| point_count(geom)).sum::<usize>(),
            "local constraints must be no less efficient than the global tolerance fallback"
        );
    }

    #[test]
    fn smoothed_face_ownership_comes_from_boundary_not_sample_point() {
        let square = |x0: f64, y0: f64, x1: f64, y1: f64| vec![(x0, y0), (x1, y0), (x1, y1), (x0, y1), (x0, y0)];
        let forest =
            Geom::Polygon { exterior: square(0.0, 0.0, 10.0, 10.0), interiors: vec![square(4.0, 4.0, 6.0, 6.0)] };
        let urban = Geom::Polygon { exterior: square(4.0, 4.0, 6.0, 6.0), interiors: vec![] };
        let rebuilt_face = Geom::Polygon { exterior: square(0.0, 0.0, 10.0, 10.0), interiors: vec![] };
        let source_geos = [try_polygon_to_geos(&forest).unwrap(), try_polygon_to_geos(&urban).unwrap()];
        let face = try_polygon_to_geos(&rebuilt_face).unwrap();

        let sample = face.point_on_surface().unwrap();
        assert!(
            source_geos[1].covers(&sample).unwrap(),
            "the old point lookup would assign this mostly-forest face to Urban"
        );
        let grid = Grid { left: 0.0, bottom: 0.0, cols: 10, rows: 10, cell_m: 1.0 };
        let (smoothed, _) = smooth_coverage(
            vec![(SemanticClass::Forest as u8, forest), (SemanticClass::Urban as u8, urban)],
            &grid,
            0.5,
        )
        .unwrap();
        assert_eq!(smoothed.iter().map(|(class, _)| *class).collect::<Vec<_>>(), vec![3, 4]);
    }

    #[test]
    fn ordinary_boundary_motion_preserves_face_ownership_without_overlay() {
        let shared = [(5.0, 0.0), (5.0, 4.0), (6.0, 5.0), (5.0, 6.0), (5.0, 10.0)];
        let forest = Geom::Polygon {
            exterior: vec![(0.0, 0.0), shared[0], shared[1], shared[2], shared[3], shared[4], (0.0, 10.0), (0.0, 0.0)],
            interiors: vec![],
        };
        let urban = Geom::Polygon {
            exterior: vec![shared[0], (10.0, 0.0), (10.0, 10.0), shared[4], shared[3], shared[2], shared[1], shared[0]],
            interiors: vec![],
        };
        let grid = Grid { left: 0.0, bottom: 0.0, cols: 10, rows: 10, cell_m: 1.0 };
        let (smoothed, stats) = smooth_coverage(
            vec![(SemanticClass::Forest as u8, forest), (SemanticClass::Urban as u8, urban)],
            &grid,
            0.5,
        )
        .unwrap();

        assert_eq!(smoothed.iter().map(|(class, _)| *class).collect::<Vec<_>>(), vec![3, 4]);
        assert!(stats.shared_edges > 0);
        let Geom::Polygon { exterior: forest_ring, .. } = &smoothed[0].1 else { panic!("expected forest polygon") };
        let Geom::Polygon { exterior: urban_ring, .. } = &smoothed[1].1 else { panic!("expected urban polygon") };
        assert_ne!(forest_ring[3], shared[2], "the ordinary degree-two kink should actually move");
        assert_eq!(forest_ring[1..=5], urban_ring[3..=7].iter().copied().rev().collect::<Vec<_>>());
    }

    #[test]
    fn tiny_water_is_removed_without_moving_larger_components() {
        let mut water = vec![false; 8 * 4];
        water[1] = true;
        for (x, y) in [(4, 1), (5, 1), (5, 2)] {
            water[y * 8 + x] = true;
        }
        remove_tiny_water_components(&mut water, 8, 4, 3);
        assert!(!water[1], "the isolated sub-pixel pond is removed");
        assert!(water[12] && water[13] && water[21], "the larger component stays in place");
    }
}
