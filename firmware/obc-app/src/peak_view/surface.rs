//! Front-to-back traversal of baked geographic height bounds and bilinear terrain cells.

use super::{
    panorama::{view_sector_bounds, view_sectors, Panorama, COLUMNS, ROWS, SECTORS, SECTOR_COLUMNS as SECTOR},
    PeakViewPeak, PeakViewProfile,
};
use obc_elevation::surface::Patch;

const DRAW_RAYS: usize = SECTOR + 2;
// Fill the quarter-degree bearings between the picture's 3/8-degree samples only near named peaks.
const RAYS: usize = DRAW_RAYS + SECTOR;
const _: () = assert!(COLUMNS.is_multiple_of(SECTOR) && SECTORS == 64 && RAYS <= u32::BITS as usize);
// A cutoff row is `0..=ROWS`, which is what the host test build records it in.
const _: () = assert!(ROWS <= u8::MAX as usize);
const CURVATURE: f32 = 0.87 / (2.0 * 6_371_000.0);
const INVERSE_CURVATURE: f32 = -1.0 / (2.0 * CURVATURE);
const NO_DEPTH: u16 = u16::MAX;
// Depth-first traversal can leave three siblings pending at each world-grid level.
const STACK_CAPACITY: usize =
    3 * (obc_formats::obct::WORLD_SIDE.trailing_zeros() - obc_formats::obct::MIN_POSTING_LOG2 as u32) as usize + 1;

/// Sample indices use the geographic origin at -2^28 microdegrees.
#[derive(Clone, Copy)]
pub struct SurfaceLevel {
    pub min_y: u32,
    pub min_x: u32,
    pub rows: u32,
    pub columns: u32,
    pub posting_log2: u8,
    pub cell_log2: u8,
}

pub trait SurfaceTerrain {
    fn level(&self, index: usize) -> Option<SurfaceLevel>;
    /// Geographic-cell presence is separate from a present cell's unknown height bounds.
    fn cell_present(&mut self, _y: u32, _x: u32) -> bool {
        true
    }
    fn patch(&mut self, level: usize, y: u32, x: u32) -> Option<Patch>;
    /// Residuals relative to a node's corner-defined bilinear patch: metres and metres/interval.
    fn approximation(&mut self, _level: usize, _y: u32, _x: u32, _log2: u8) -> Option<(f32, f32)> {
        None
    }
    fn coarse_patch(&mut self, _level: usize, _y: u32, _x: u32, _log2: u8) -> Option<Patch> {
        None
    }
    /// Inclusive vertex maximum for a square of 2^log2 terrain cells. Unknown bounds are 32767.
    fn max_height(&mut self, level: usize, y: u32, x: u32, log2: u8) -> Option<i16>;
    /// Container-relative group of 2^log2 geographic cells, bounded at native resolution.
    fn max_cell_group(&mut self, _y: u32, _x: u32, _log2: u8) -> Option<i16> {
        None
    }
}

#[derive(Clone, Copy)]
struct Node {
    y: u32,
    x: u32,
    log2: u8,
    mask: u32,
}

#[derive(Clone, Copy)]
struct Intervals {
    near: [f32; RAYS],
    far: [f32; RAYS],
}

#[derive(Clone, Copy)]
struct PreparedPatch {
    surface: Patch,
    maximum: f32,
}

impl PreparedPatch {
    fn new(surface: Patch) -> Self {
        let maximum = surface
            .height
            .max(surface.height + surface.east)
            .max(surface.height + surface.north)
            .max(surface.height + surface.east + surface.north + surface.cross);
        Self { surface, maximum }
    }
}

/// Owns a progressively filled panorama and a bounded working set. Completed bearings need no reads.
pub struct Builder {
    pub panorama: Panorama,
    pub peaks: super::Candidates,
    profile: PeakViewProfile<'static>,
    stack: heapless::Vec<Node, STACK_CAPACITY>,
    sector: usize,
    priority_heading_q4: u16,
    reuse_halo: bool,
    labels_only: bool,
    retained: u64,
    finished: u64,
    ray_mask: u32,
    bearings: [u16; RAYS],
    level: usize,
    next_distance: f32,
    intervals: Intervals,
    geometry: SurfaceLevel,
    origin_y: f32,
    origin_x: f32,
    dy: [f32; RAYS],
    dx: [f32; RAYS],
    inv_dy: [f32; RAYS],
    inv_dx: [f32; RAYS],
    north: [f32; RAYS],
    east: [f32; RAYS],
    cutoff: [usize; RAYS],
    /// The first column of the sector [`finish_sector`](Self::finish_sector) last completed, and
    /// the skyline row of each of its columns. The drawn tones cannot answer this — a sunlit face
    /// and the sky share tone 0 — and the photo regression test measures the drawn skyline.
    ///
    /// Host test builds only, and one sector wide rather than the whole circle: a `Builder` lives
    /// in the device's panorama arena, which has about 1.5 KiB spare, and 960 rows of it would be
    /// most of that spent on a test.
    #[cfg(any(test, feature = "external-fixtures"))]
    sector_skyline: (u16, [u8; SECTOR]),
    depth: [[u16; ROWS]; DRAW_RAYS],
    tones: [[u8; ROWS]; DRAW_RAYS],
    thresholds: [f32; ROWS],
    catalogue_masks: [u64; RAYS],
    catalogue_bearings: [u16; 64],
    peak_slopes: [f32; 64],
    lat_scale: f32,
    lon_scale: f32,
    light_north: f32,
    light_east: f32,
    radians_per_row: f32,
    gradient_error_limit: f32,
}

impl Builder {
    pub fn new(profile: &PeakViewProfile<'_>) -> Self {
        let mut slot = core::mem::MaybeUninit::uninit();
        // SAFETY: exclusive, aligned storage, read only after initialization.
        unsafe {
            Self::init_at(slot.as_mut_ptr(), profile);
            slot.assume_init()
        }
    }

    /// # Safety
    /// `out` must be aligned, writable, exclusive storage for a Builder.
    pub unsafe fn init_at(out: *mut Self, profile: &PeakViewProfile<'_>) {
        use core::ptr::{addr_of_mut, write_bytes};
        // Scalar/array fields accept zero. Install reference-bearing fields before borrowing.
        unsafe {
            write_bytes(out.cast::<u8>(), 0, core::mem::size_of::<Self>());
            addr_of_mut!((*out).profile).write(profile.detached());
            addr_of_mut!((*out).peaks).write(heapless::Vec::from_slice(profile.peaks).expect("bounded peak catalogue"));
            addr_of_mut!((*out).stack).write(heapless::Vec::new());
        }
        let result = unsafe { &mut *out };
        result.lat_scale = 1e6 / 111_320.0;
        result.lon_scale = result.lat_scale / libm::cosf((profile.observer_lat as f32 / 1e6).to_radians());
        let light = 315.0f32.to_radians();
        result.light_north = libm::cosf(light);
        result.light_east = libm::sinf(light);
        result.peak_slopes.fill(f32::NEG_INFINITY);
        let (bottom, top) = profile.vertical_bounds_q4();
        result.radians_per_row = ((top - bottom) as f32 / (4.0 * ROWS as f32)).to_radians();
        for (row, threshold) in result.thresholds.iter_mut().enumerate() {
            let angle = (top as f32 - (row as f32 + 0.5) * (top - bottom) as f32 / ROWS as f32) / 4.0;
            *threshold = libm::tanf(angle.to_radians());
        }
        for (i, peak) in result.peaks.iter_mut().enumerate() {
            peak.visible = false;
            result.catalogue_bearings[i] = peak.azimuth_q4;
        }
        result.priority_heading_q4 = profile.default_heading_q4;
        result.sector = result.next_sector();
        result.begin_sector();
    }

    /// Replace hidden candidates and trace only their sight lines over the finished picture.
    pub fn refill(&mut self, candidates: &super::Candidates) {
        assert!(self.complete());
        self.peaks.clone_from(candidates);
        self.labels_only = true;
        self.retained = 0;
        self.peak_slopes.fill(f32::NEG_INFINITY);
        for (i, peak) in self.peaks.iter().enumerate() {
            self.catalogue_bearings[i] = peak.azimuth_q4;
            if peak.visible {
                self.retained |= 1 << i;
            }
        }
        if self.peaks.iter().all(|peak| peak.visible) {
            return;
        }
        self.finished = 0;
        self.reuse_halo = false;
        self.sector = self.next_sector();
        self.begin_sector();
    }

    #[cfg(test)]
    fn relocate(&mut self, lat: i32, lon: i32, elevation_m: i16) {
        assert_eq!(self.level, 0, "relocate before stepping the job");
        for (i, peak) in self.peaks.iter_mut().enumerate() {
            peak.project(lat, lon);
            self.catalogue_bearings[i] = peak.azimuth_q4;
        }
        self.profile.observer_lat = lat;
        self.profile.observer_lon = lon;
        self.profile.observer_elevation_m = elevation_m;
        self.lon_scale = self.lat_scale / libm::cosf((lat as f32 / 1e6).to_radians());
        self.begin_sector();
    }

    pub fn profile(&self) -> PeakViewProfile<'static> {
        self.profile
    }

    pub fn display_peaks(&self) -> impl Iterator<Item = PeakViewPeak> + '_ {
        self.peaks.iter().map(|peak| {
            let mut peak = *peak;
            peak.visible &= self.panorama.ready_at_bearing_q4(peak.azimuth_q4);
            peak
        })
    }

    pub fn complete(&self) -> bool {
        self.finished == u64::MAX
    }

    /// Every sector [`step`](Self::step) has finished so far, one bit per sector.
    #[cfg(any(test, feature = "external-fixtures"))]
    pub fn finished_sectors(&self) -> u64 {
        self.finished
    }

    /// The sector [`step`](Self::step) finished last: its first panorama column, and the topmost
    /// row terrain reached in each of its columns — [`ROWS`] where a column is all sky. Column
    /// zero points north and each one is `360 / COLUMNS` degrees wide.
    ///
    /// It holds one sector only, and the next one to finish overwrites it. A caller that wants the
    /// whole circle steps with a budget of one and reads this whenever
    /// [`finished_sectors`](Self::finished_sectors) changes.
    #[cfg(any(test, feature = "external-fixtures"))]
    pub fn last_sector_skyline(&self) -> (usize, [u8; SECTOR]) {
        (usize::from(self.sector_skyline.0), self.sector_skyline.1)
    }
    pub fn progress(&self) -> u8 {
        (self.panorama.finished.count_ones() * 100 / SECTORS as u32) as u8
    }

    /// Finish the current sector, then give an unfinished part of this view priority.
    pub fn set_heading(&mut self, heading_q4: u16) {
        self.priority_heading_q4 = heading_q4 % 1440;
    }

    /// Terrain, outlines and nearby catalogue rays are complete throughout this viewport.
    pub fn view_ready(&self, heading_q4: u16) -> bool {
        self.panorama.view_ready(heading_q4, self.profile.horizontal_fov_q4())
    }

    fn next_sector(&self) -> usize {
        let fov = self.profile.horizontal_fov_q4();
        let (low, high) = view_sector_bounds(self.priority_heading_q4, fov);
        // Grow both edges in 17-degree batches. Each batch keeps clockwise halo reuse.
        let buffer = (0..SECTORS as i32)
            .step_by(3)
            .flat_map(|offset| (high + 1 + offset..high + 4 + offset).chain(low - offset - 3..low - offset));
        view_sectors(self.priority_heading_q4, fov)
            .chain(buffer.map(|sector| sector.rem_euclid(SECTORS as i32) as usize))
            .find(|sector| self.finished & (1 << sector) == 0)
            .expect("an unfinished sector")
            * SECTOR
    }

    /// Each unit visits one hierarchy node, or its bounded 4x4-cell leaf.
    pub fn step(&mut self, terrain: &mut impl SurfaceTerrain, budget: usize) {
        for _ in 0..budget {
            if self.complete() {
                break;
            }
            if let Some(node) = self.stack.pop() {
                self.visit(terrain, node);
            } else if !self.begin_level(terrain) {
                if !self.labels_only {
                    self.finish_sector();
                    self.panorama.finished |= 1 << (self.sector / SECTOR);
                }
                self.finished |= 1 << (self.sector / SECTOR);
                if !self.complete() {
                    let next = self.next_sector();
                    self.reuse_halo = !self.labels_only && next == (self.sector + SECTOR) % COLUMNS;
                    self.sector = next;
                    self.begin_sector();
                }
            }
        }
    }

    fn begin_sector(&mut self) {
        self.level = 0;
        self.next_distance = 2.0;
        if !self.reuse_halo {
            self.depth.fill([NO_DEPTH; ROWS]);
            self.tones.fill([0; ROWS]);
        } else {
            // The previous sector already rendered these two overlapping bearings.
            self.depth[0] = self.depth[SECTOR];
            self.depth[1] = self.depth[SECTOR + 1];
            self.tones[0] = self.tones[SECTOR];
            self.tones[1] = self.tones[SECTOR + 1];
            self.depth[2..].fill([NO_DEPTH; ROWS]);
            self.tones[2..].fill([0; ROWS]);
        }
        let halo = self.reuse_halo.then(|| [self.cutoff[SECTOR], self.cutoff[SECTOR + 1]]);
        self.cutoff.fill(ROWS);
        // The two reused bearings keep their skyline as well as their pixels. Their rays are out of
        // the mask below, so nothing would fill it again.
        if let Some([first, second]) = halo {
            self.cutoff[0] = first;
            self.cutoff[1] = second;
        }
        self.ray_mask = if self.labels_only { 0 } else { (1 << DRAW_RAYS) - 1 };
        for ray in 0..RAYS {
            let (bearing_q8, catalogue) = if self.labels_only {
                (((self.sector * 3 / 2 + ray + 1440 - 3) % 1440) as i32 * 2, true)
            } else if ray < DRAW_RAYS && !self.labels_only {
                let column = (self.sector + COLUMNS + ray - 1) % COLUMNS;
                (column as i32 * 3, column.is_multiple_of(2))
            } else {
                let index = self.sector + ray - DRAW_RAYS;
                let quarter = 3 * (index / 2) + index % 2 + 1;
                (quarter as i32 * 2, true)
            };
            let bearing = bearing_q8 / 2;
            self.bearings[ray] = bearing as u16;
            let angle = (bearing_q8 as f32 / 8.0).to_radians();
            self.north[ray] = libm::cosf(angle);
            self.east[ray] = libm::sinf(angle);
            self.catalogue_masks[ray] = 0;
            if catalogue {
                for (i, target) in self.catalogue_bearings[..self.peaks.len()].iter().enumerate() {
                    let delta = (bearing - *target as i32 + 720).rem_euclid(1440) - 720;
                    if delta.abs() <= 2 && self.retained & (1 << i) == 0 {
                        self.catalogue_masks[ray] |= 1 << i;
                    }
                }
            }
            if (self.labels_only || ray >= DRAW_RAYS) && self.catalogue_masks[ray] != 0 {
                self.ray_mask |= 1 << ray;
            }
            if self.reuse_halo && ray < 2 {
                self.ray_mask &= !(1 << ray);
            }
        }
    }

    fn begin_level(&mut self, terrain: &impl SurfaceTerrain) -> bool {
        if self.next_distance >= 100_000.0 {
            return false;
        }
        let Some(g) = terrain.level(self.level) else {
            return false;
        };
        let max_distance = if terrain.level(self.level + 1).is_none() {
            100_000.0
        } else {
            (25_000.0 * (1u32 << g.posting_log2) as f32 / 512.0).min(100_000.0)
        };
        self.geometry = g;
        let posting = (1u32 << g.posting_log2) as f32;
        self.gradient_error_limit = 0.15 * posting / self.lat_scale.max(self.lon_scale);
        self.origin_y =
            (self.profile.observer_lat as i64 + (1 << 28) - ((g.min_y as i64) << g.posting_log2)) as f32 / posting;
        self.origin_x =
            (self.profile.observer_lon as i64 + (1 << 28) - ((g.min_x as i64) << g.posting_log2)) as f32 / posting;
        for ray in 0..RAYS {
            self.dy[ray] = self.north[ray] * self.lat_scale / posting;
            self.dx[ray] = self.east[ray] * self.lon_scale / posting;
            self.inv_dy[ray] = 1.0 / self.dy[ray];
            self.inv_dx[ray] = 1.0 / self.dx[ray];
        }
        let log2 = g.rows.max(g.columns).next_power_of_two().trailing_zeros() as u8;
        let mut node = Node { y: 0, x: 0, log2, mask: self.ray_mask };
        self.intervals = Intervals { near: [self.next_distance; RAYS], far: [max_distance; RAYS] };
        // Clip the initial rays to geographic coverage, including observers outside this file.
        for ray in 0..RAYS {
            if node.mask & (1 << ray) == 0 {
                continue;
            }
            // Label-only rays need the foreground and target, but nothing behind the last target.
            let mut limit = max_distance;
            if self.labels_only || ray >= DRAW_RAYS {
                let mut distance = 0u32;
                let mut mask = self.catalogue_masks[ray];
                while mask != 0 {
                    let i = mask.trailing_zeros() as usize;
                    mask &= mask - 1;
                    distance = distance.max(self.peaks[i].distance_m);
                }
                limit = limit.min(distance as f32);
                if limit <= self.next_distance {
                    node.mask &= !(1 << ray);
                    continue;
                }
                self.intervals.far[ray] = limit;
            }
            clip_inverse(
                &mut self.intervals.near[ray],
                &mut self.intervals.far[ray],
                self.origin_y,
                self.dy[ray],
                self.inv_dy[ray],
                0.0,
                g.rows as f32,
            );
            clip_inverse(
                &mut self.intervals.near[ray],
                &mut self.intervals.far[ray],
                self.origin_x,
                self.dx[ray],
                self.inv_dx[ray],
                0.0,
                g.columns as f32,
            );
            if self.intervals.near[ray] > self.next_distance || self.intervals.far[ray] < limit {
                self.record_missing(ray);
            }
            if self.intervals.near[ray] >= self.intervals.far[ray] {
                node.mask &= !(1 << ray);
            }
        }
        self.next_distance = max_distance;
        self.level += 1;
        assert!(self.stack.push(node).is_ok(), "bounded geographic hierarchy");
        true
    }

    #[inline(never)]
    fn visit(&mut self, terrain: &mut impl SurfaceTerrain, mut node: Node) {
        if node.mask == 0 {
            return;
        }
        let g = self.geometry;
        if (node.y << node.log2) >= g.rows || (node.x << node.log2) >= g.columns {
            return;
        }
        let mut intervals = self.intervals;
        let size = (1u32 << node.log2) as f32;
        let low_y = node.y as f32 * size;
        let low_x = node.x as f32 * size;
        for ray in active_rays(node.mask) {
            clip_inverse(
                &mut intervals.near[ray],
                &mut intervals.far[ray],
                self.origin_y,
                self.dy[ray],
                self.inv_dy[ray],
                low_y,
                low_y + size,
            );
            clip_inverse(
                &mut intervals.near[ray],
                &mut intervals.far[ray],
                self.origin_x,
                self.dx[ray],
                self.inv_dx[ray],
                low_x,
                low_x + size,
            );
        }
        let cell_log2 = g.cell_log2 - g.posting_log2;
        if node.log2 == cell_log2
            && !terrain.cell_present((g.min_y >> cell_log2) + node.y, (g.min_x >> cell_log2) + node.x)
        {
            for ray in active_rays(node.mask) {
                self.record_missing(ray);
            }
            return;
        }
        let maximum = if node.log2 > cell_log2 {
            terrain.max_cell_group(node.y, node.x, node.log2 - cell_log2)
        } else {
            terrain.max_height(
                self.level - 1,
                (g.min_y >> node.log2) + node.y,
                (g.min_x >> node.log2) + node.x,
                node.log2,
            )
        };
        if let Some(maximum) = maximum {
            for ray in active_rays(node.mask) {
                if self.hidden(ray, maximum as f32, intervals.near[ray], intervals.far[ray]) {
                    node.mask &= !(1 << ray);
                }
            }
        }
        if node.mask == 0 {
            return;
        }
        if node.log2 <= cell_log2 {
            self.merge_smooth(terrain, &mut node, &intervals);
            if node.mask == 0 {
                return;
            }
        }
        if node.log2 <= 2 {
            self.leaf(terrain, node, &intervals);
            return;
        }
        let half = (1u32 << (node.log2 - 1)) as f32;
        let mid_y = (node.y * 2 + 1) as f32 * half;
        let mid_x = (node.x * 2 + 1) as f32 * half;
        let quadrant = (u8::from(self.origin_y >= mid_y) << 1) | u8::from(self.origin_x >= mid_x);
        let mut masks = [0u32; 4];
        for ray in active_rays(node.mask) {
            let bit = 1 << ray;
            let cy = split_crossing(self.origin_y, self.dy[ray], self.inv_dy[ray], mid_y);
            let cx = split_crossing(self.origin_x, self.dx[ray], self.inv_dx[ray], mid_x);
            let near = intervals.near[ray];
            let far = intervals.far[ray];
            let mut child = (usize::from((near >= cy) == (self.dy[ray] > 0.0)) << 1)
                | usize::from((near >= cx) == (self.dx[ray] > 0.0));
            masks[child] |= bit;
            let cross_y = cy > near && cy < far;
            let cross_x = cx > near && cx < far;
            if cross_y && cross_x {
                if cy != cx {
                    child ^= if cy < cx { 2 } else { 1 };
                    masks[child] |= bit;
                    child ^= if cy < cx { 1 } else { 2 };
                } else {
                    child ^= 3;
                }
                masks[child] |= bit;
            } else if cross_y || cross_x {
                masks[child ^ if cross_y { 2 } else { 1 }] |= bit;
            }
        }
        for order in (0..4).rev() {
            let child = order ^ quadrant;
            let mask = masks[child as usize];
            if mask != 0 {
                let next = Node {
                    y: node.y * 2 + (child >> 1) as u32,
                    x: node.x * 2 + (child & 1) as u32,
                    log2: node.log2 - 1,
                    mask,
                };
                assert!(self.stack.push(next).is_ok(), "bounded geographic hierarchy");
            }
        }
    }

    fn merge_smooth(&mut self, terrain: &mut impl SurfaceTerrain, node: &mut Node, intervals: &Intervals) {
        let g = self.geometry;
        // A merged patch reads its corner nodes directly, so it cannot carry the clamp under the
        // rider. Leave the rider's own neighbourhood to `leaf`; its height-error gate refuses these
        // near nodes anyway.
        if self.touches_own_nodes(node.y << node.log2, node.x << node.log2, 1 << node.log2) {
            return;
        }
        let Some((height_error, gradient_error)) = terrain.approximation(
            self.level - 1,
            (g.min_y >> node.log2) + node.y,
            (g.min_x >> node.log2) + node.x,
            node.log2,
        ) else {
            return;
        };
        // Bound the change in both physical gradient components; normal errors must not be
        // hidden by a distant node's small angular height error.
        if gradient_error > self.gradient_error_limit {
            return;
        }
        let mut mask = 0;
        for ray in active_rays(node.mask) {
            if height_error > intervals.near[ray] * (self.radians_per_row * 0.35) {
                continue;
            }
            // Keep the entire catalogue sight line exact, including its foreground horizon.
            if self.catalogue_masks[ray] != 0 {
                continue;
            }
            mask |= 1 << ray;
        }
        if mask == 0 {
            return;
        }
        let y = node.y << node.log2;
        let x = node.x << node.log2;
        let Some(patch) = terrain.coarse_patch(self.level - 1, g.min_y + y, g.min_x + x, node.log2) else {
            return;
        };
        let size = (1u32 << node.log2) as f32;
        let mut prepared = PreparedPatch::new(patch);
        prepared.surface.east /= size;
        prepared.surface.north /= size;
        prepared.surface.cross /= size * size;
        for ray in active_rays(mask) {
            self.paint(ray, (y as i32, x as i32), size, prepared, intervals.near[ray], intervals.far[ray]);
        }
        node.mask &= !mask;
    }

    fn hidden(&self, ray: usize, maximum: f32, near: f32, far: f32) -> bool {
        let mut mask = self.catalogue_masks[ray];
        while mask != 0 {
            let i = mask.trailing_zeros() as usize;
            mask &= mask - 1;
            let d = self.peaks[i].distance_m as f32;
            if d >= near && d <= far {
                return false;
            }
        }
        if self.cutoff[ray] == 0 {
            return true;
        }
        let slope = self.thresholds[self.cutoff[ray] - 1];
        let d = (slope * INVERSE_CURVATURE).clamp(near, far);
        maximum + 1.0 < self.profile.observer_elevation_m as f32 + slope * d + CURVATURE * d * d
    }

    #[inline(never)]
    fn leaf(&mut self, terrain: &mut impl SurfaceTerrain, node: Node, intervals: &Intervals) {
        let g = self.geometry;
        let mut patches = [None; 16];
        let mut loaded = 0u16;
        for ray in active_rays(node.mask) {
            let mut near = intervals.near[ray];
            let far = intervals.far[ray];
            let low_y = (node.y << node.log2) as i32;
            let low_x = (node.x << node.log2) as i32;
            let size = 1i32 << node.log2;
            let mut y = ((self.origin_y + self.dy[ray] * near) as i32).clamp(low_y, low_y + size - 1);
            let mut x = ((self.origin_x + self.dx[ray] * near) as i32).clamp(low_x, low_x + size - 1);
            let step_y = if self.dy[ray] >= 0.0 { 1 } else { -1 };
            let step_x = if self.dx[ray] >= 0.0 { 1 } else { -1 };
            while near < far && y >= low_y && x >= low_x && y < low_y + size && x < low_x + size {
                let boundary_y = y + i32::from(step_y > 0);
                let boundary_x = x + i32::from(step_x > 0);
                let ty = if self.dy[ray].abs() < 1e-12 {
                    f32::INFINITY
                } else {
                    (boundary_y as f32 - self.origin_y) * self.inv_dy[ray]
                };
                let tx = if self.dx[ray].abs() < 1e-12 {
                    f32::INFINITY
                } else {
                    (boundary_x as f32 - self.origin_x) * self.inv_dx[ray]
                };
                let end = far.min(ty.min(tx));
                if end > near && y >= 0 && x >= 0 && (y as u32) < g.rows && (x as u32) < g.columns {
                    let index = ((y - low_y) * size + x - low_x) as usize;
                    if loaded & (1 << index) == 0 {
                        patches[index] = terrain
                            .patch(self.level - 1, g.min_y + y as u32, g.min_x + x as u32)
                            .map(|patch| PreparedPatch::new(self.clamp_own_nodes(patch, y, x)));
                        loaded |= 1 << index;
                    }
                    if let Some(patch) = patches[index] {
                        self.paint(ray, (y, x), 1.0, patch, near, end);
                    } else {
                        self.record_missing(ray);
                    }
                }
                if tx <= ty {
                    x += step_x;
                }
                if ty <= tx {
                    y += step_y;
                }
                near = near.max(end);
            }
        }
    }

    /// The ground under the rider is never above the rider.
    ///
    /// A lattice node can stand on the summit the rider is standing on, above an eye that the
    /// altimeter put below it, and the panorama came back blocked at 19 m by the rider's own
    /// mountain. The four lattice nodes of the cell the rider stands in are therefore clamped to
    /// the rider's own height. The clamp is on the nodes, not on the cell: every cell that uses one
    /// of those nodes sees the same clamped value, so the surface stays continuous and no edge of
    /// the own cell steps. Nothing inside the own cell is then above the eye, while a cell one
    /// posting out keeps its own far corners, so the ridge beyond the rider is still real. The cell
    /// stays foreground: it paints the ground below the eye and a summit 40 m away is still tested
    /// against it. Where the eye already stands on the cell top this changes nothing.
    fn clamp_own_nodes(&self, patch: Patch, y: i32, x: i32) -> Patch {
        if self.level != 1 {
            return patch;
        }
        let (oy, ox) = (libm::floorf(self.origin_y) as i32, libm::floorf(self.origin_x) as i32);
        let own = |ny: i32, nx: i32| (ny == oy || ny == oy + 1) && (nx == ox || nx == ox + 1);
        if !own(y, x) && !own(y, x + 1) && !own(y + 1, x) && !own(y + 1, x + 1) {
            return patch;
        }
        let ground = f32::from(self.profile.observer_elevation_m) - 2.0;
        let cap = |height: f32, ny, nx| if own(ny, nx) { height.min(ground) } else { height };
        let a = cap(patch.height, y, x);
        let b = cap(patch.height + patch.east, y, x + 1);
        let c = cap(patch.height + patch.north, y + 1, x);
        let d = cap(patch.height + patch.east + patch.north + patch.cross, y + 1, x + 1);
        Patch { height: a, east: b - a, north: c - a, cross: a - b - c + d }
    }

    /// Whether a node's corners include one of the clamped nodes under the rider.
    fn touches_own_nodes(&self, y: u32, x: u32, size: u32) -> bool {
        let spans = |origin: f32, low: u32| {
            let node = libm::floorf(origin) as i64;
            i64::from(low) <= node + 1 && i64::from(low + size) >= node
        };
        self.level == 1 && spans(self.origin_y, y) && spans(self.origin_x, x)
    }

    fn paint(&mut self, ray: usize, position: (i32, i32), size: f32, prepared: PreparedPatch, near: f32, far: f32) {
        let (y, x) = position;
        if self.hidden(ray, prepared.maximum, near, far) {
            return;
        }
        let patch = prepared.surface;
        let fy = (self.origin_y + self.dy[ray] * near - y as f32).clamp(0.0, size);
        let fx = (self.origin_x + self.dx[ray] * near - x as f32).clamp(0.0, size);
        let h = patch.height + patch.east * fx + patch.north * fy + patch.cross * fx * fy;
        let n = patch.north + patch.cross * fx;
        let e = patch.east + patch.cross * fy;
        let a = patch.cross * self.dx[ray] * self.dy[ray] - CURVATURE;
        let b = n * self.dy[ray] + e * self.dx[ray] - 2.0 * CURVATURE * near;
        let c = h - self.profile.observer_elevation_m as f32 - CURVATURE * near * near;
        let length = far - near;
        let start_derivative = b * near - c;
        let end_derivative = a * length * length + 2.0 * a * near * length + start_derivative;
        let end = if start_derivative > 0.0 && end_derivative < 0.0 {
            (sqrt((c - b * near + a * near * near) / a) - near).clamp(0.0, length)
        } else {
            length
        };
        let start_slope = c / near;
        let peak = start_slope.max((c + end * (b + a * end)) / (near + end));
        let north_scale = self.lat_scale / (1u32 << self.geometry.posting_log2) as f32;
        let east_scale = self.lon_scale / (1u32 << self.geometry.posting_log2) as f32;
        let bottom = self.cutoff[ray];
        let mut top = bottom;
        while top > 0 && peak >= self.thresholds[top - 1] {
            top -= 1;
        }
        if top < bottom && ray < DRAW_RAYS && !self.labels_only {
            let surface_at = |threshold: f32| {
                let offset = if start_slope >= threshold {
                    0.0
                } else {
                    intersection(a, b - threshold, c - threshold * near, end)
                };
                let distance = near + offset;
                let ns = (n + patch.cross * self.dx[ray] * offset) * north_scale;
                let es = (e + patch.cross * self.dy[ray] * offset) * east_scale;
                let diffuse = ((0.423 - 0.906 * (ns * self.light_north + es * self.light_east))
                    / sqrt(1.0 + ns * ns + es * es))
                .clamp(0.0, 1.0);
                let light = 0.16 + 0.84 * diffuse + ((distance - 3000.0) / 30000.0).clamp(0.0, 0.35);
                (distance, light)
            };
            let (near_depth, near_light) = surface_at(self.thresholds[bottom - 1]);
            let (far_depth, far_light) =
                if bottom - top == 1 { (near_depth, near_light) } else { surface_at(self.thresholds[top]) };
            let scale = 1.0 / ((bottom - top - 1).max(1) as f32);
            // Exact surface endpoints retain the skyline. Interpolate illumination and depth
            // within this one cell instead of solving the same surface for every pixel.
            for row in (top..bottom).rev() {
                let fraction = (bottom - 1 - row) as f32 * scale;
                let (distance, light) = if size > 1.0 {
                    surface_at(self.thresholds[row])
                } else {
                    (near_depth + (far_depth - near_depth) * fraction, near_light + (far_light - near_light) * fraction)
                };
                self.tones[ray][row] = if light >= 0.75 {
                    0
                } else if light >= 0.25 {
                    1
                } else {
                    2
                };
                self.depth[ray][row] = (distance * 0.5 + 0.5) as u16;
            }
        }
        self.cutoff[ray] = top;
        // Catalogue visibility never changes the measured terrain surface.
        let mut mask = self.catalogue_masks[ray];
        while mask != 0 {
            let i = mask.trailing_zeros() as usize;
            mask &= mask - 1;
            let d = self.peaks[i].distance_m as f32;
            if d < near || d > far {
                continue;
            }
            let offset = d - near;
            let slope = (c + offset * (b + a * offset)) / d;
            if slope < self.peak_slopes[i]
                || (slope == self.peak_slopes[i] && self.bearings[ray] >= self.peaks[i].azimuth_q4)
            {
                continue;
            }
            self.peak_slopes[i] = slope;
            // Only foreground can hide the target, including the front part of its own cell.
            let front_slope = if offset <= end { start_slope.max(slope) } else { peak };
            let horizon = if bottom < ROWS { self.thresholds[bottom] } else { f32::NEG_INFINITY };
            // Half a pixel keeps sampled summit coordinates stable at raster boundaries.
            let tolerance = 0.5 * self.radians_per_row * (1.0 + slope * slope);
            // Native DEM vertices can undershoot a narrow summit. Test its recorded elevation
            // against foreground terrain, while keeping the label anchored to the sampled surface.
            let target_slope = self.peaks[i]
                .elevation_m
                .map(|height| (f32::from(height) - f32::from(self.profile.observer_elevation_m)) / d - CURVATURE * d)
                .unwrap_or(slope)
                .max(slope);
            self.peaks[i].visible = target_slope + tolerance >= horizon.max(front_slope);
            self.peaks[i].angle_q4 = libm::roundf(libm::atanf(slope).to_degrees() * 4.0) as i16;
            self.peaks[i].azimuth_q4 = self.bearings[ray];
        }
    }

    fn record_missing(&mut self, ray: usize) {
        if self.labels_only {
            return;
        }
        let column = if ray < DRAW_RAYS && !self.labels_only {
            (self.sector + COLUMNS + ray - 1) % COLUMNS
        } else {
            super::panorama::column_of(self.bearings[ray])
        };
        self.panorama.mark_incomplete(column);
    }

    fn finish_sector(&mut self) {
        #[cfg(any(test, feature = "external-fixtures"))]
        {
            self.sector_skyline.0 = self.sector as u16;
            for ray in 1..=SECTOR {
                self.sector_skyline.1[ray - 1] = self.cutoff[ray] as u8;
            }
        }
        for ray in 1..=SECTOR {
            for row in 0..ROWS {
                let depth = self.depth[ray][row];
                let mut tone = self.tones[ray][row];
                if depth != NO_DEPTH
                    && row > 0
                    && [self.depth[ray - 1][row], self.depth[ray + 1][row], self.depth[ray][row - 1]]
                        .iter()
                        .any(|&other| other as u32 * 5 > depth as u32 * 6 && other as u32 > depth as u32 + 100)
                {
                    tone = 3;
                }
                self.panorama.set(self.sector + ray - 1, row, tone);
            }
        }
    }
}

fn active_rays(mut mask: u32) -> impl Iterator<Item = usize> {
    core::iter::from_fn(move || {
        if mask == 0 {
            return None;
        }
        let ray = mask.trailing_zeros() as usize;
        mask &= mask - 1;
        Some(ray)
    })
}

#[inline]
fn sqrt(value: f32) -> f32 {
    #[cfg(all(target_arch = "arm", target_abi = "eabihf"))]
    {
        let mut bits = value.to_bits();
        // SAFETY: the hard-float ARM target requires VFP instructions. The explicit
        // s0 clobber preserves live registers; this operation accesses no memory.
        unsafe {
            core::arch::asm!("vmov s0, {bits}", "vsqrt.f32 s0, s0", "vmov {bits}, s0",
                bits = inout(reg) bits, out("s0") _, options(pure, nomem, nostack));
        }
        f32::from_bits(bits)
    }
    #[cfg(not(all(target_arch = "arm", target_abi = "eabihf")))]
    {
        libm::sqrtf(value)
    }
}

fn intersection(a: f32, b: f32, c: f32, end: f32) -> f32 {
    if a.abs() < 1e-12 {
        return (-c / b).clamp(0.0, end);
    }
    let root = sqrt((b * b - 4.0 * a * c).max(0.0));
    let q = -0.5 * (b + libm::copysignf(root, b));
    let first = q / a;
    let second = c / q;
    let mut result = end;
    if first >= 0.0 && first <= end {
        result = first;
    }
    if second >= 0.0 && second <= end {
        result = result.min(second);
    }
    result
}

#[allow(clippy::too_many_arguments)]
fn clip_inverse(near: &mut f32, far: &mut f32, origin: f32, direction: f32, inverse: f32, low: f32, high: f32) {
    if direction.abs() < 1e-12 {
        if origin < low || origin >= high {
            *far = *near;
        }
    } else {
        let a = (low - origin) * inverse;
        let b = (high - origin) * inverse;
        *near = near.max(a.min(b));
        *far = far.min(a.max(b));
    }
}

fn split_crossing(origin: f32, direction: f32, inverse: f32, middle: f32) -> f32 {
    if direction.abs() >= 1e-12 {
        (middle - origin) * inverse
    } else if (origin >= middle) == (direction > 0.0) {
        f32::NEG_INFINITY
    } else {
        f32::INFINITY
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    static PROFILE: PeakViewProfile<'static> = PeakViewProfile {
        observer_lat: 0,
        observer_lon: 0,
        observer_elevation_m: 0,
        default_heading_q4: 0,
        fov_q4: 272,
        vertical_centre_q4: 50,
        vertical_span_q4: 201,
        peaks: &[],
    };

    struct Flat {
        height: i16,
        missing: bool,
        patches: usize,
    }
    impl SurfaceTerrain for Flat {
        fn level(&self, index: usize) -> Option<SurfaceLevel> {
            (index == 0).then_some(SurfaceLevel {
                min_y: (1 << 19) - 64,
                min_x: (1 << 19) - 64,
                rows: 128,
                columns: 128,
                posting_log2: 9,
                cell_log2: 16,
            })
        }
        fn patch(&mut self, _: usize, y: u32, x: u32) -> Option<Patch> {
            let minimum = (1 << 19) - 64;
            assert!((minimum..minimum + 128).contains(&y));
            assert!((minimum..minimum + 128).contains(&x));
            self.patches += 1;
            (!self.missing).then_some(Patch { height: self.height as f32, east: 0.0, north: 0.0, cross: 0.0 })
        }
        fn max_height(&mut self, _: usize, _: u32, _: u32, log2: u8) -> Option<i16> {
            assert!((2..=7).contains(&log2));
            Some(self.height)
        }
    }

    /// Bare ground at 0 m with one posting cell raised, to stand the observer on or beside it.
    struct Tower {
        cell: (u32, u32),
        height: f32,
    }
    impl SurfaceTerrain for Tower {
        fn level(&self, index: usize) -> Option<SurfaceLevel> {
            Flat { height: 0, missing: false, patches: 0 }.level(index)
        }
        fn patch(&mut self, _: usize, y: u32, x: u32) -> Option<Patch> {
            let height = if (y, x) == self.cell { self.height } else { 0.0 };
            Some(Patch { height, east: 0.0, north: 0.0, cross: 0.0 })
        }
        fn max_height(&mut self, _: usize, _: u32, _: u32, _: u8) -> Option<i16> {
            Some(self.height as i16)
        }
    }

    #[test]
    fn intersections_choose_the_first_crossing_without_cancellation() {
        assert!((intersection(1.0, -5.0, 6.0, 4.0) - 2.0).abs() < 1e-6);
        assert!((intersection(0.0, 2.0, -6.0, 4.0) - 3.0).abs() < 1e-6);
        assert!((intersection(1.0, -10000.0, 1.0, 1.0) - 0.0001).abs() < 1e-9);
        assert!((intersection(-0.5, 3.0, -4.0, 3.0) - 2.0).abs() < 1e-6);
    }

    #[test]
    fn bounded_geographic_traversal_handles_cardinal_rays_and_missing_coverage() {
        let mut terrain = Flat { height: 1000, missing: false, patches: 0 };
        let mut job = std::boxed::Box::new(Builder::new(&PROFILE));
        // The eye stands on the plane: `eye_ground` cannot put it 1000 m under the ground, where
        // the clamp under the rider would carve a pit into it.
        job.relocate(240, -300, 1002);
        job.step(&mut terrain, 1);
        assert!(!job.complete());
        while !job.complete() {
            job.step(&mut terrain, 31);
        }
        assert!(job.panorama.has_incomplete_coverage(), "the small synthetic terrain does not cover the full range");
        assert!(terrain.patches > 0);
        let horizon = (0..ROWS).find(|&row| job.panorama.tone(0, row) != 0).expect("the plane below the eye");
        for column in 0..COLUMNS {
            for row in 0..ROWS {
                let painted = job.panorama.tone(column, row) != 0;
                assert_eq!(painted, row >= horizon, "flat terrain at {column},{row}");
            }
        }
        let calls = terrain.patches;
        job.step(&mut terrain, 31);
        assert_eq!(terrain.patches, calls);

        let mut outside = std::boxed::Box::new(Builder::new(&PROFILE));
        outside.relocate(1_000_000, 1_000_000, 0);
        while !outside.complete() {
            outside.step(&mut terrain, 31);
        }
        assert!(outside.panorama.has_incomplete_coverage(), "clipped coverage cannot be reported as clear sky");
        assert_eq!(terrain.patches, calls, "coverage outside this file does not read patches");

        terrain.missing = true;
        let mut missing = std::boxed::Box::new(Builder::new(&PROFILE));
        while !missing.complete() {
            missing.step(&mut terrain, 31);
        }
        assert!(missing.panorama.has_incomplete_coverage());
        for column in 0..COLUMNS {
            for row in 0..ROWS {
                assert_eq!(missing.panorama.tone(column, row), 0);
            }
        }
    }

    /// The ground under the rider never occludes. A crest lifts the corners of the cell the rider
    /// stands in, and an eye below one of those corners saw nothing but its own mountain. Clamped,
    /// that cell paints exactly like bare ground, wherever in the cell the rider stands. Two cells
    /// out the same tower is ordinary terrain and paints its skyline.
    #[test]
    fn the_clamped_own_cell_paints_like_bare_ground_and_a_tower_two_cells_out_still_stands() {
        let own = (1 << 19, 1 << 19);
        let render = |cell, height, at: (i32, i32)| {
            let mut terrain = Tower { cell, height };
            let mut job = std::boxed::Box::new(Builder::new(&PROFILE));
            job.relocate(at.0, at.1, 2);
            while !job.complete() {
                job.step(&mut terrain, 63);
            }
            job
        };
        // Mid-cell, two metres into the cell, and exactly on the lattice node.
        for at in [(256, 256), (16, 16), (0, 0)] {
            let plain = render(own, 0.0, at);
            let inside = render(own, 60.0, at);
            for column in 0..COLUMNS {
                for row in 0..ROWS {
                    let (tone, bare) = (inside.panorama.tone(column, row), plain.panorama.tone(column, row));
                    assert_eq!(tone, bare, "observer at {at:?}, pixel {column},{row}");
                }
            }
        }
        // Two cells north the tower spans 85 to 142 m, so 60 m of it stands 30 degrees up.
        let skyline = |job: &Builder| (0..ROWS).find(|&row| job.panorama.tone(0, row) != 0).unwrap_or(ROWS);
        let plain = render(own, 0.0, (256, 256));
        let outside = render((own.0 + 2, own.1), 60.0, (256, 256));
        assert!(skyline(&outside) + 100 < skyline(&plain), "a tower two cells north rises over the horizon");
    }

    /// A dilated crest node row can stand 30 m over an eye the altimeter put 1.8 m north of it.
    /// Clamping the rider's own nodes opens that row where the rider stands, and only there: one
    /// posting out the same row is real terrain and keeps its skyline.
    #[test]
    fn a_lifted_node_row_under_the_rider_fills_no_column_and_leaves_the_ridge() {
        /// Every node at or south of `crest` stands 30 m up; north of it the ground is at 0 m.
        struct Crest {
            crest: u32,
        }
        impl SurfaceTerrain for Crest {
            fn level(&self, index: usize) -> Option<SurfaceLevel> {
                Flat { height: 0, missing: false, patches: 0 }.level(index)
            }
            fn patch(&mut self, _: usize, y: u32, _: u32) -> Option<Patch> {
                let node = |ny: u32| if ny <= self.crest { 30.0 } else { 0.0 };
                let (south, north) = (node(y), node(y + 1));
                Some(Patch { height: south, east: 0.0, north: north - south, cross: 0.0 })
            }
            fn max_height(&mut self, _: usize, _: u32, _: u32, _: u8) -> Option<i16> {
                Some(30)
            }
        }
        let mut job = std::boxed::Box::new(Builder::new(&PROFILE));
        // 1.8 m north of the crest node row, half a posting east of its node column.
        job.relocate(16, 256, 2);
        let mut terrain = Crest { crest: 1 << 19 };
        while !job.complete() {
            job.step(&mut terrain, 63);
        }
        let filled = (0..COLUMNS).filter(|&column| job.panorama.tone(column, 0) != 0).count();
        assert_eq!(filled, 0, "the ground under the rider cannot fill a column to the frame top");
        // Due south the crest stands 28 m over the eye 57 m out, a quarter of the way up the frame.
        let south = super::super::panorama::column_of(720);
        let skyline = (0..ROWS).find(|&row| job.panorama.tone(south, row) != 0).expect("the ridge");
        assert!((30..90).contains(&skyline), "the ridge beyond the clamped nodes is real, at row {skyline}");
    }

    /// A summit 40 m away lies inside the observer's own cell, so that cell has to be traversed for
    /// its sight line to be tested at all, and must not hide it once it is.
    #[test]
    fn a_summit_forty_metres_away_keeps_its_label() {
        fn label<T: SurfaceTerrain>(profile: &PeakViewProfile, terrain: &mut T) -> PeakViewPeak {
            let mut job = std::boxed::Box::new(Builder::new(profile));
            while !job.complete() {
                job.step(terrain, 63);
            }
            job.peaks[0]
        }
        let mut peak = PeakViewPeak { lat: 360, elevation_m: Some(0), ..PeakViewPeak::EMPTY };
        peak.project(0, 0);
        assert!((38..=42).contains(&peak.distance_m), "360 microdegrees north is about 40 m");
        let peaks = [peak];
        let profile = PeakViewProfile { peaks: &peaks, ..PeakViewProfile::at(0, 0, 2) };
        let mut bare = Flat { height: 0, missing: false, patches: 0 };
        let mut lifted = Tower { cell: (1 << 19, 1 << 19), height: 60.0 };
        for peak in [label(&profile, &mut bare), label(&profile, &mut lifted)] {
            assert!(peak.visible, "a summit inside the own cell keeps its label");
            assert!(peak.angle_q4 < 0, "ground 40 m away is below an eye 2 m up");
        }
    }

    #[test]
    fn all_64_refill_candidates_are_traced_without_changing_the_picture() {
        let mut terrain = Flat { height: 100, missing: false, patches: 0 };
        let mut job = std::boxed::Box::new(Builder::new(&PeakViewProfile::at(0, 0, 102)));
        while !job.complete() {
            job.step(&mut terrain, 512);
        }
        let picture = job.panorama.clone();
        let mut candidates = super::super::Candidates::new();
        for i in 0..64 {
            let mut peak = PeakViewPeak { lat: 4500, lon: i, ..PeakViewPeak::EMPTY };
            peak.project(0, 0);
            candidates.push(peak).unwrap();
        }
        job.refill(&candidates);
        assert!(!job.complete());
        while !job.complete() {
            job.step(&mut terrain, 512);
        }
        assert!(job.peaks[63].visible, "the high half of the catalogue mask is traced");
        for column in 0..COLUMNS {
            for row in 0..ROWS {
                assert_eq!(job.panorama.tone(column, row), picture.tone(column, row));
            }
        }
        assert_eq!(job.panorama.finished, picture.finished);
        assert_eq!(job.panorama.has_incomplete_coverage(), picture.has_incomplete_coverage());
    }

    #[test]
    fn foreground_terrain_occludes_summits_at_each_vertical_scale() {
        struct Ridge {
            near_x: i32,
            near_height: f32,
            summit_height: i16,
        }
        impl SurfaceTerrain for Ridge {
            fn level(&self, index: usize) -> Option<SurfaceLevel> {
                (index == 0).then_some(SurfaceLevel {
                    min_y: (1 << 19) - 64,
                    min_x: (1 << 19) - 64,
                    rows: 128,
                    columns: 128,
                    posting_log2: 9,
                    cell_log2: 16,
                })
            }
            fn patch(&mut self, _: usize, _: u32, x: u32) -> Option<Patch> {
                let height = |x| {
                    if x - (1 << 19) == self.near_x {
                        self.near_height
                    } else if x - (1 << 19) == 60 {
                        f32::from(self.summit_height)
                    } else {
                        0.0
                    }
                };
                let a = height(x as i32);
                Some(Patch { height: a, east: height(x as i32 + 1) - a, north: 0.0, cross: 0.0 })
            }
            fn max_height(&mut self, _: usize, _: u32, _: u32, _: u8) -> Option<i16> {
                Some(200)
            }
        }
        for vertical_scale_q8 in [320, 768] {
            // Include a ridge inside 200 m and a summit hidden by only about 0.27°.
            for (near_x, near_height, summit_height, recorded_height, visible) in [
                (2, 40.0, 200, Some(200), false),
                (20, 60.0, 160, Some(160), false),
                (20, 60.0, 200, Some(200), true),
                (20, 60.0, 200, Some(160), true),
                (20, 60.0, 160, Some(200), true),
                (20, 80.0, 160, Some(200), false),
                (20, 60.0, 160, None, false),
            ] {
                let mut peak = PeakViewPeak { lon: 60 * 512, elevation_m: recorded_height, ..PeakViewPeak::EMPTY };
                peak.project(0, 0);
                let peaks = [peak];
                let profile = PeakViewProfile {
                    // Straddle the horizon, so only the scale under test varies.
                    vertical_centre_q4: 0,
                    vertical_span_q4: (178 * 320 / vertical_scale_q8),
                    peaks: &peaks,
                    ..PeakViewProfile::at(0, 0, 2)
                };
                let mut job = std::boxed::Box::new(Builder::new(&profile));
                let mut terrain = Ridge { near_x, near_height, summit_height };
                while !job.complete() {
                    job.step(&mut terrain, 64);
                }
                assert_eq!(
                    job.peaks[0].visible, visible,
                    "summit {summit_height} m, foreground at {near_x}, scale {vertical_scale_q8}"
                );
                assert!(job.peaks[0].angle_q4 > 0, "the distant summit was sampled");
                let surface_angle = libm::atan2f(f32::from(summit_height), peak.distance_m as f32).to_degrees() * 4.0;
                assert!((f32::from(job.peaks[0].angle_q4) - surface_angle).abs() <= 1.0, "anchor stays on the DEM");
            }
        }
    }

    #[test]
    fn progressive_views_prioritize_turns_and_preserve_the_completed_panorama() {
        struct Hills;
        impl SurfaceTerrain for Hills {
            fn level(&self, index: usize) -> Option<SurfaceLevel> {
                Flat { height: 100, missing: false, patches: 0 }.level(index)
            }
            fn patch(&mut self, _: usize, y: u32, x: u32) -> Option<Patch> {
                let height = |y: u32, x: u32| {
                    50.0 + 30.0 * libm::sinf((x as i32 - (1 << 19)) as f32 * 0.35)
                        + 20.0 * libm::cosf((y as i32 - (1 << 19)) as f32 * 0.2)
                };
                let (a, b, c, d) = (height(y, x), height(y, x + 1), height(y + 1, x), height(y + 1, x + 1));
                Some(Patch { height: a, east: b - a, north: c - a, cross: a - b - c + d })
            }
            fn max_height(&mut self, _: usize, _: u32, _: u32, _: u8) -> Option<i16> {
                Some(100)
            }
        }
        let mut peaks = [
            PeakViewPeak { lat: 10_000, lon: -100, ..PeakViewPeak::EMPTY },
            PeakViewPeak { lat: -10_000, lon: 100, ..PeakViewPeak::EMPTY },
        ];
        for peak in &mut peaks {
            peak.project(0, 0);
        }
        let profile = PeakViewProfile { peaks: &peaks, ..PeakViewProfile::at(0, 0, 72) };
        let mut reference = std::boxed::Box::new(Builder::new(&profile));
        while !reference.complete() {
            reference.step(&mut Hills, 127);
        }

        let mut moving = std::boxed::Box::new(Builder::new(&PeakViewProfile { default_heading_q4: 1430, ..profile }));
        assert!(!moving.view_ready(1430));
        while !moving.view_ready(1430) {
            moving.step(&mut Hills, 1);
        }
        assert!(moving.progress() < 30, "the wrapped north view is shown before the full circle");
        assert!(!moving.view_ready(720), "pending terrain is not a ready blank view");
        moving.set_heading(720);
        while !moving.view_ready(720) {
            moving.step(&mut Hills, 1);
        }
        assert!(
            !moving.view_ready(360) && !moving.view_ready(1080),
            "a quick turn gets priority over unrelated bearings"
        );
        assert!(moving.view_ready(1430));
        while !moving.complete() {
            moving.step(&mut Hills, 127);
        }
        assert_eq!(moving.progress(), 100);
        assert_eq!(reference.peaks, moving.peaks);
        for x in 0..COLUMNS {
            for y in 0..ROWS {
                assert_eq!(reference.panorama.tone(x, y), moving.panorama.tone(x, y), "pixel {x},{y}");
            }
        }
    }

    #[test]
    fn background_work_buffers_both_sides_of_a_wrapped_view() {
        let profile = PeakViewProfile { default_heading_q4: 1430, ..PROFILE };
        let mut job = std::boxed::Box::new(Builder::new(&profile));
        let mut terrain = Flat { height: 100, missing: false, patches: 0 };
        while !job.view_ready(1430) {
            job.step(&mut terrain, 1);
        }
        let initial = job.panorama.finished.count_ones();
        while job.panorama.finished.count_ones() < initial + 6 {
            job.step(&mut terrain, 1);
        }
        let (low, high) = view_sector_bounds(1430, profile.horizontal_fov_q4());
        for sector in low - 3..=high + 3 {
            assert_ne!(job.panorama.finished & (1 << sector.rem_euclid(SECTORS as i32)), 0);
        }
        assert!(!job.view_ready(710), "buffer both edges before working on the opposite direction");
    }

    #[test]
    fn changing_the_height_datum_preserves_the_view_below_sea_level() {
        let render = |height| {
            let mut terrain = Flat { height, missing: false, patches: 0 };
            let mut job = std::boxed::Box::new(Builder::new(&PROFILE));
            job.relocate(0, 0, height + 2);
            while !job.complete() {
                job.step(&mut terrain, 31);
            }
            assert!(
                job.panorama.has_incomplete_coverage(),
                "the small synthetic terrain does not cover the full range"
            );
            job
        };
        let above = render(1000);
        let below = render(-100);
        for column in 0..COLUMNS {
            for row in 0..ROWS {
                assert_eq!(above.panorama.tone(column, row), below.panorama.tone(column, row));
            }
        }
    }
}
