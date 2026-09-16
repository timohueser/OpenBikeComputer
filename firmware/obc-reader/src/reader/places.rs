//! Bounded, opening-aware place pages over the existing POI quadtrees.

use super::poi::{decode_poi_name, PoiCatEntry};
use super::{Reader, BRANCH_BIT, EMPTY_LEAF};
use crate::hours::OpeningStatus;
use crate::{CorridorPoi, Error, Poi, PoiCategorySet, RoutePath};
use heapless::Vec;
use obc_formats::io::{rd_i32, rd_u16};
use obc_formats::obcm::{poi_category_of, PoiMetadata, SourceId, POI_RECORD_LEN};
use obc_map_scene::{cos_lat, delta_m, ground_dist_m_cl, BBox};

/// Resident page capacity; identity continuations expose all matching places.
pub const PLACE_PAGE_SIZE: usize = 8;

/// Opening status controls search membership, never route eligibility.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum HoursFilter {
    All,
    #[default]
    HideClosed,
}

impl HoursFilter {
    pub fn includes(self, opening: OpeningStatus) -> bool {
        self == Self::All || opening != OpeningStatus::Closed
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryProgress {
    Pending,
    Ready { more: bool, coverage_complete: bool },
    Failed(Error),
    Unavailable,
}

/// A continuation names facts, never a mutable row index.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct PlaceKey {
    pub distance_m: u32,
    pub source: SourceId,
    pub occurrence: u32,
}

#[derive(Debug, Clone, Copy)]
pub enum PlaceWindow {
    Nearby { position: (i32, i32), radius_m: u32 },
    Corridor { from_m: u32, to_m: u32, half_width_m: u16 },
}

#[derive(Debug, Clone, Copy)]
struct Branch {
    index: [u8; 4],
    quadrant: u8,
}

impl Branch {
    fn new(index: u32, quadrant: u8) -> Self {
        Self { index: index.to_le_bytes(), quadrant }
    }

    fn index(self) -> u32 {
        u32::from_le_bytes(self.index)
    }
}

/// A suspended POI record and its nearest point; geometry and POI bytes stay in their sources.
#[derive(Debug, Default)]
struct EncounterScan {
    record: usize,
    chunk: Option<usize>,
    backwards: bool,
    best: Option<crate::corridor::PathProjection>,
}

/// One immutable browsing generation. Each step reads at most one index node or POI chunk and
/// one route chunk. A pending encounter resumes before the spatial walk continues.
/// A new page repeats the spatial walk with an exclusive identity/order boundary.
#[derive(Debug)]
pub struct PlaceQuery {
    generation: u32,
    source_generation: Option<u32>,
    categories: PoiCategorySet,
    window: PlaceWindow,
    local: Option<(u8, u16)>,
    hours_filter: HoursFilter,
    after: Option<PlaceKey>,
    backwards: bool,
    category: usize,
    route_chunk: usize,
    route_validated: bool,
    encounter: EncounterScan,
    stack: Vec<Branch, 33>,
    leaf: Option<u32>,
    started: bool,
    more: bool,
    coverage_complete: bool,
    progress: QueryProgress,
}

impl PlaceQuery {
    pub fn new(generation: u32, categories: PoiCategorySet, window: PlaceWindow, local: Option<(u8, u16)>) -> Self {
        Self {
            generation,
            source_generation: None,
            categories,
            window,
            local,
            hours_filter: HoursFilter::HideClosed,
            after: None,
            backwards: false,
            category: 0,
            route_chunk: 0,
            route_validated: false,
            encounter: EncounterScan::default(),
            stack: Vec::new(),
            leaf: None,
            started: false,
            more: false,
            coverage_complete: true,
            progress: QueryProgress::Pending,
        }
    }

    pub fn with_hours_filter(mut self, filter: HoursFilter) -> Self {
        self.hours_filter = filter;
        self
    }

    pub fn progress(&self) -> QueryProgress {
        self.progress
    }

    pub fn cancel(&mut self) {
        self.progress = QueryProgress::Unavailable;
        self.stack.clear();
        self.leaf = None;
    }

    pub fn next_page(&mut self, after: PlaceKey) {
        self.page(after, false);
    }

    pub fn previous_page(&mut self, before: PlaceKey) {
        self.page(before, true);
    }

    fn page(&mut self, after: PlaceKey, backwards: bool) {
        if !matches!(self.progress, QueryProgress::Ready { .. }) {
            return;
        }
        self.backwards = backwards;
        self.after = Some(after);
        self.category = 0;
        self.route_chunk = 0;
        self.route_validated = false;
        self.encounter = EncounterScan::default();
        self.stack.clear();
        self.leaf = None;
        self.started = false;
        self.more = false;
        self.progress = QueryProgress::Pending;
    }

    pub fn key(&self, hit: &CorridorPoi) -> PlaceKey {
        PlaceKey {
            distance_m: hit.poi.distance_m,
            source: hit.poi.metadata.source,
            occurrence: if matches!(self.window, PlaceWindow::Corridor { .. }) { hit.dist_along_m } else { 0 },
        }
    }

    /// A changed source or caller generation cancels the old query rather than mixing pages.
    pub fn step<const N: usize>(
        &mut self,
        reader: &Reader,
        route: Option<&dyn RoutePath>,
        generation: u32,
        out: &mut Vec<CorridorPoi, N>,
    ) -> QueryProgress {
        if generation != self.generation || self.source_generation.is_some_and(|g| g != reader.tables.generation) {
            self.cancel();
            out.clear();
            return self.progress;
        }
        if self.progress != QueryProgress::Pending {
            return self.progress;
        }
        if N == 0 {
            self.progress = QueryProgress::Failed(Error::BadOffset);
            return self.progress;
        }
        self.source_generation = Some(reader.tables.generation);
        if let Err(error) = self.advance(reader, route, out) {
            self.progress = QueryProgress::Failed(error);
            out.clear();
        }
        self.progress
    }

    fn advance<const N: usize>(
        &mut self,
        reader: &Reader,
        route: Option<&dyn RoutePath>,
        out: &mut Vec<CorridorPoi, N>,
    ) -> Result<(), Error> {
        let search = match self.window {
            PlaceWindow::Nearby { position: (lon, lat), radius_m } => crate::corridor::inflate_bbox(
                BBox { min_lon: lon, max_lon: lon, min_lat: lat, max_lat: lat },
                radius_m as f32,
            ),
            PlaceWindow::Corridor { from_m, to_m, half_width_m } => {
                let Some(route) = route else {
                    self.progress = QueryProgress::Unavailable;
                    return Ok(());
                };
                if self.route_chunk >= route.chunk_count()
                    || route.chunk_start_m(self.route_chunk) > to_m
                    || (!self.backwards
                        && self.more
                        && out.last().is_some_and(|last| route.chunk_start_m(self.route_chunk) > last.dist_along_m))
                {
                    self.finish();
                    return Ok(());
                }
                if self.route_chunk + 1 < route.chunk_count() && route.chunk_start_m(self.route_chunk + 1) < from_m {
                    self.route_chunk += 1;
                    self.route_validated = false;
                    return Ok(());
                }
                if !self.route_validated {
                    let mut valid = false;
                    route.visit_chunk_points(self.route_chunk, &mut |points| valid = points.len() >= 2);
                    if !valid {
                        return Err(Error::BadOffset);
                    }
                    self.route_validated = true;
                }
                crate::corridor::inflate_bbox(route.chunk_bbox(self.route_chunk), half_width_m as f32)
            }
        };
        self.coverage_complete &= contains(reader.bbox, search);
        let Some(category) = self.categories.iter().nth(self.category) else {
            if matches!(self.window, PlaceWindow::Corridor { .. }) {
                self.route_chunk += 1;
                self.route_validated = false;
                self.category = 0;
                self.started = false;
            } else {
                self.finish();
            }
            return Ok(());
        };
        let Some(entry) = reader.tables.pois.entries.iter().find(|e| e.category_id == category.id() && !e.is_empty())
        else {
            self.next_category();
            return Ok(());
        };
        if !self.started {
            self.stack.push(Branch::new(0, 4)).map_err(|_| Error::BadOffset)?;
            self.started = true;
        }
        if let Some(leaf) = self.leaf {
            if self.read_leaf(reader, entry, leaf, route, out)? {
                self.leaf = None;
                self.encounter = EncounterScan::default();
                self.next_node();
            }
            return Ok(());
        }
        let Some(top) = self.stack.last() else {
            self.next_category();
            return Ok(());
        };
        let index = top.index() as usize;
        if index >= entry.node_count {
            return Err(Error::BadOffset);
        }
        let bbox = self.node_bbox(reader.bbox);
        if !bbox.intersects(&search) {
            self.next_node();
            return Ok(());
        }
        let value = reader.read_node(entry, index).map_err(Error::from)?;
        if value & BRANCH_BIT == 0 {
            if value == EMPTY_LEAF {
                self.next_node();
            } else {
                self.leaf = Some(value);
            }
        } else {
            let child = value & !BRANCH_BIT;
            if child <= index as u32 || child as usize + 3 >= entry.node_count {
                return Err(Error::BadOffset);
            }
            self.stack.push(Branch::new(child, 0)).map_err(|_| Error::BadOffset)?;
        }
        Ok(())
    }

    fn next_category(&mut self) {
        self.category += 1;
        self.started = false;
        self.stack.clear();
    }
    fn finish(&mut self) {
        self.progress = QueryProgress::Ready { more: self.more, coverage_complete: self.coverage_complete };
    }
    fn next_node(&mut self) {
        while let Some(mut node) = self.stack.pop() {
            if node.quadrant < 3 {
                node.index = (node.index() + 1).to_le_bytes();
                node.quadrant += 1;
                let _ = self.stack.push(node);
                break;
            }
        }
    }
    fn node_bbox(&self, mut bbox: BBox) -> BBox {
        for node in self.stack.iter().skip(1) {
            let lon = (i64::from(bbox.min_lon) + i64::from(bbox.max_lon)).div_euclid(2) as i32;
            let lat = (i64::from(bbox.min_lat) + i64::from(bbox.max_lat)).div_euclid(2) as i32;
            if node.quadrant & 1 == 0 {
                bbox.max_lon = lon;
            } else {
                bbox.min_lon = lon;
            }
            if node.quadrant < 2 {
                bbox.min_lat = lat;
            } else {
                bbox.max_lat = lat;
            }
        }
        bbox
    }

    fn read_leaf<const N: usize>(
        &mut self,
        reader: &Reader,
        entry: &PoiCatEntry,
        leaf: u32,
        route: Option<&dyn RoutePath>,
        out: &mut Vec<CorridorPoi, N>,
    ) -> Result<bool, Error> {
        let size = reader.tables.pois.chunk_size;
        let (start, end) = entry.chunk_range(leaf, size).ok_or(Error::BadOffset)?;
        if end > reader.src.len() || size < POI_RECORD_LEN {
            return Err(Error::BadOffset);
        }
        let route_chunk = self.route_chunk;
        let is_corridor = matches!(self.window, PlaceWindow::Corridor { .. });
        let mut cursor = core::mem::take(&mut self.encounter);
        let scan_chunk = cursor.chunk.unwrap_or(route_chunk);
        let mut paused = false;
        let mut scan = |points: &[(i32, i32)]| -> Result<(), Error> {
            let mut record_error = None;
            let mut record = 0;
            reader
                .stream_poi_records(start, size / POI_RECORD_LEN, |bytes, off, lat, lon, subtype| {
                    let index = record;
                    record += 1;
                    if paused || index < cursor.record {
                        return;
                    }

                    if poi_category_of(subtype).is_none_or(|c| c.id() != entry.category_id) {
                        record_error = Some(Error::BadOffset);
                        return;
                    }
                    let Some(metadata) = PoiMetadata::decode(&bytes[off + 36..off + 64]) else {
                        record_error = Some(Error::BadOffset);
                        return;
                    };
                    let hours_ref = rd_u16(bytes, off + 34);
                    let opening = match reader.try_poi_hours(hours_ref) {
                        Ok(schedule) => schedule.map_or(OpeningStatus::Unknown, |s| s.status(self.local)),
                        Err(error) => {
                            record_error = Some(error);
                            return;
                        }
                    };
                    if !self.hours_filter.includes(opening) {
                        cursor.record += 1;
                        return;
                    }
                    let mut hit = CorridorPoi {
                        poi: Poi {
                            opening,
                            metadata,
                            lat: rd_i32(bytes, off),
                            lon: rd_i32(bytes, off + 4),
                            subtype,
                            name: decode_poi_name(bytes, off),
                            hours_ref,
                            distance_m: 0,
                        },
                        dist_along_m: 0,
                        offset_m: 0,
                    };
                    match self.window {
                        PlaceWindow::Nearby { position, radius_m } => {
                            hit.poi.distance_m = ground_dist_m_cl(position, (lon, lat), cos_lat(position.1)) as u32;
                            if hit.poi.distance_m > radius_m {
                                return;
                            }
                            self.consider(out, hit);
                        }
                        PlaceWindow::Corridor { from_m, to_m, half_width_m } => {
                            let Some(route) = route else { return };
                            let first_chunk = route_chunk == 0 || route.chunk_start_m(route_chunk) < from_m;
                            let mut emit = |projection: crate::corridor::PathProjection| {
                                let along = projection.dist_along_m.max(0.0) as u32;
                                if along < from_m || along > to_m {
                                    return;
                                }
                                hit.dist_along_m = along;
                                hit.poi.distance_m = along - from_m;
                                hit.offset_m = libm::roundf(projection.offset_m) as i32;
                                self.consider(out, hit.clone());
                            };
                            let limit = half_width_m as f32;
                            if cursor.backwards {
                                let continues = preceding_pass(
                                    points,
                                    route.chunk_start_m(scan_chunk),
                                    (lon, lat),
                                    limit,
                                    &mut cursor.best,
                                );
                                if continues && scan_chunk > 0 {
                                    cursor.chunk = Some(scan_chunk - 1);
                                } else {
                                    cursor.chunk = None;
                                    cursor.backwards = false;
                                }
                                paused = true;
                                return;
                            }
                            // The anchor may start inside a pass owned by an earlier chunk. Resolve
                            // that pass before filtering its canonical nearest point by the window.
                            if cursor.chunk.is_none()
                                && first_chunk
                                && route_chunk > 0
                                && cursor.best.is_none()
                                && inside(points[0], (lon, lat), limit)
                            {
                                cursor.chunk = Some(route_chunk - 1);
                                cursor.backwards = true;
                                paused = true;
                                return;
                            }
                            let continuation = cursor.chunk.is_some();
                            let pending = pass_encounters(
                                points,
                                route.chunk_start_m(scan_chunk),
                                (lon, lat),
                                limit,
                                !first_chunk && !continuation && inside(points[0], (lon, lat), limit),
                                continuation,
                                &mut cursor.best,
                                &mut emit,
                            );
                            if pending && scan_chunk + 1 < route.chunk_count() {
                                cursor.chunk = Some(scan_chunk + 1);
                                paused = true;
                                return;
                            }
                            if let Some(best) = cursor.best.take() {
                                emit(best);
                            }
                            cursor.chunk = None;
                            if continuation {
                                paused = true;
                            }
                        }
                    }
                    cursor.record += 1;
                })
                .map_err(Error::Source)?;
            if let Some(error) = record_error {
                return Err(error);
            }
            Ok(())
        };
        let result = if let (true, Some(path)) = (is_corridor, route) {
            let mut result = Err(Error::BadOffset);
            path.visit_chunk_points(scan_chunk, &mut |points| {
                if points.len() >= 2 {
                    result = scan(points);
                }
            });
            result
        } else {
            scan(&[])
        };
        self.encounter = cursor;
        result.map(|()| !paused)
    }

    fn consider<const N: usize>(&mut self, out: &mut Vec<CorridorPoi, N>, hit: CorridorPoi) {
        let key = self.key(&hit);
        if self.after.is_some_and(|after| if self.backwards { key >= after } else { key <= after })
            || out.iter().any(|p| self.key(p) == key)
        {
            return;
        }
        let index = out.iter().position(|p| self.key(p) > key).unwrap_or(out.len());
        if out.is_full() {
            self.more = true;
            if self.backwards {
                if index == 0 {
                    return;
                }
                out.remove(0);
                let _ = out.insert(index - 1, hit);
                return;
            }
            if index == out.len() {
                return;
            }
            out.pop();
        }
        let _ = out.insert(index, hit);
    }
}

fn contains(outer: BBox, inner: BBox) -> bool {
    outer.min_lon <= inner.min_lon
        && outer.min_lat <= inner.min_lat
        && outer.max_lon >= inner.max_lon
        && outer.max_lat >= inner.max_lat
}

impl Reader<'_> {
    /// Refresh only status. Membership, geometry, order and continuation keys stay frozen.
    pub fn refresh_place_hours(&self, page: &mut [CorridorPoi], local: Option<(u8, u16)>) -> Result<(), Error> {
        for hit in page {
            hit.poi.opening = self
                .try_poi_hours(hit.poi.hours_ref)?
                .map_or(OpeningStatus::Unknown, |schedule| schedule.status(local));
        }
        Ok(())
    }

    pub fn poi_opening_status(&self, hours_ref: u16, local: Option<(u8, u16)>) -> OpeningStatus {
        self.poi_hours(hours_ref).map_or(OpeningStatus::Unknown, |schedule| schedule.status(local))
    }
}

fn inside(point: (i32, i32), place: (i32, i32), limit: f32) -> bool {
    ground_dist_m_cl(point, place, cos_lat(place.1)) <= limit
}

fn nearer(best: &mut Option<crate::corridor::PathProjection>, candidate: crate::corridor::PathProjection) {
    if best.is_none_or(|old| candidate.offset_m.abs() < old.offset_m.abs()) {
        *best = Some(candidate);
    }
}

fn projections(
    points: &[(i32, i32)],
    start_m: u32,
    place: (i32, i32),
    mut visit: impl FnMut((i32, i32), (i32, i32), crate::corridor::PathProjection),
) {
    let cl = cos_lat(points[0].1);
    let mut along = start_m as f32;
    for segment in points.windows(2) {
        if let Some(mut projection) = crate::corridor::project_onto_chunk(segment, 0, place, f32::INFINITY) {
            projection.dist_along_m += along;
            visit(segment[0], segment[1], projection);
        }
        let (dx, dy) = delta_m(segment[0], segment[1], cl);
        along += libm::sqrtf(dx * dx + dy * dy);
    }
}

/// Only the trailing pass connects to the next chunk. Earlier passes in this chunk are unrelated.
fn preceding_pass(
    points: &[(i32, i32)],
    start_m: u32,
    place: (i32, i32),
    limit: f32,
    best: &mut Option<crate::corridor::PathProjection>,
) -> bool {
    let mut trailing = None;
    let mut continues = inside(points[0], place, limit);
    projections(points, start_m, place, |a, b, projection| {
        if !inside(a, place, limit) {
            trailing = None;
            continues = false;
        }
        if projection.offset_m.abs() <= limit {
            nearer(&mut trailing, projection);
        }
        if !inside(b, place, limit) {
            trailing = None;
            continues = false;
        }
    });
    if let Some(projection) = trailing {
        if best.is_none_or(|old| projection.offset_m.abs() <= old.offset_m.abs()) {
            *best = Some(projection);
        }
    }
    continues
}

/// One occurrence is one continuous pass inside the radius. Emit its nearest projection, with
/// the earliest route position winning ties. A pass can continue through arbitrarily many chunks.
#[allow(clippy::too_many_arguments)]
fn pass_encounters(
    points: &[(i32, i32)],
    start_m: u32,
    place: (i32, i32),
    limit: f32,
    mut skip: bool,
    continuation: bool,
    best: &mut Option<crate::corridor::PathProjection>,
    mut emit: impl FnMut(crate::corridor::PathProjection),
) -> bool {
    let mut done = false;
    projections(points, start_m, place, |_, b, projection| {
        if done {
            return;
        }
        if !skip && projection.offset_m.abs() <= limit {
            nearer(best, projection);
        }
        if !inside(b, place, limit) {
            if let Some(projection) = best.take() {
                emit(projection);
            }
            skip = false;
            done = continuation;
        }
    });
    best.is_some()
}
