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

/// One immutable browsing generation. Each step reads at most one index node or POI chunk.
/// A new page repeats the spatial walk with an exclusive identity/order boundary.
#[derive(Debug)]
pub struct PlaceQuery {
    generation: u32,
    source_generation: Option<u32>,
    categories: PoiCategorySet,
    window: PlaceWindow,
    local: Option<(u8, u16)>,
    after: Option<PlaceKey>,
    backwards: bool,
    category: usize,
    route_chunk: usize,
    validated_route_chunk: Option<usize>,
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
            after: None,
            backwards: false,
            category: 0,
            route_chunk: 0,
            validated_route_chunk: None,
            stack: Vec::new(),
            leaf: None,
            started: false,
            more: false,
            coverage_complete: false,
            progress: QueryProgress::Pending,
        }
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
        self.validated_route_chunk = None;
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
                    return Ok(());
                }
                if self.validated_route_chunk != Some(self.route_chunk) {
                    let mut valid = false;
                    route.visit_chunk_points(self.route_chunk, &mut |points| valid = points.len() >= 2);
                    if !valid {
                        return Err(Error::BadOffset);
                    }
                    self.validated_route_chunk = Some(self.route_chunk);
                }
                crate::corridor::inflate_bbox(route.chunk_bbox(self.route_chunk), half_width_m as f32)
            }
        };
        self.coverage_complete &= contains(reader.bbox, search);
        let Some(category) = self.categories.iter().nth(self.category) else {
            if matches!(self.window, PlaceWindow::Corridor { .. }) {
                self.route_chunk += 1;
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
        if let Some(leaf) = self.leaf.take() {
            self.read_leaf(reader, entry, leaf, route, out)?;
            self.next_node();
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
    ) -> Result<(), Error> {
        let size = reader.tables.pois.chunk_size;
        let (start, end) = entry.chunk_range(leaf, size).ok_or(Error::BadOffset)?;
        if end > reader.src.len() || size < POI_RECORD_LEN {
            return Err(Error::BadOffset);
        }
        let (previous, following) = if let (PlaceWindow::Corridor { .. }, Some(path)) = (self.window, route) {
            (
                route_boundary(path, self.route_chunk.checked_sub(1), true)?,
                route_boundary(
                    path,
                    (self.route_chunk + 1 < path.chunk_count()).then_some(self.route_chunk + 1),
                    false,
                )?,
            )
        } else {
            (None, None)
        };
        let route_chunk = self.route_chunk;
        let is_corridor = matches!(self.window, PlaceWindow::Corridor { .. });
        let mut scan = |points: &[(i32, i32)]| -> Result<(), Error> {
            let mut record_error = None;
            reader
                .stream_poi_records(start, size / POI_RECORD_LEN, |bytes, off, lat, lon, subtype| {
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
                    if opening == OpeningStatus::Closed {
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
                            route_encounters(
                                points,
                                route.chunk_start_m(route_chunk),
                                (lon, lat),
                                half_width_m as f32,
                                previous,
                                following,
                                |along, offset| {
                                    if along < from_m || along > to_m {
                                        return;
                                    }
                                    hit.dist_along_m = along;
                                    hit.poi.distance_m = along - from_m;
                                    hit.offset_m = offset;
                                    self.consider(out, hit.clone());
                                },
                            );
                        }
                    }
                })
                .map_err(Error::Source)?;
            if let Some(error) = record_error {
                return Err(error);
            }
            Ok(())
        };
        if let (true, Some(path)) = (is_corridor, route) {
            let mut result = Err(Error::BadOffset);
            path.visit_chunk_points(route_chunk, &mut |points| {
                if points.len() >= 2 {
                    result = scan(points);
                }
            });
            result
        } else {
            scan(&[])
        }
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

type Segment = [(i32, i32); 2];

fn route_boundary(path: &dyn RoutePath, index: Option<usize>, last: bool) -> Result<Option<Segment>, Error> {
    let Some(index) = index else { return Ok(None) };
    let mut segment = None;
    path.visit_chunk_points(index, &mut |points| {
        if points.len() >= 2 {
            let at = if last { points.len() - 2 } else { 0 };
            segment = Some([points[at], points[at + 1]]);
        }
    });
    segment.map(Some).ok_or(Error::BadOffset)
}

/// Each local minimum on the route is a distinct encounter. Adjacent equal minima retain
/// the first segment, including across chunk seams; a later loop keeps its later route position.
#[allow(clippy::too_many_arguments)]
fn route_encounters(
    points: &[(i32, i32)],
    start_m: u32,
    place: (i32, i32),
    limit: f32,
    previous: Option<Segment>,
    following: Option<Segment>,
    mut visit: impl FnMut(u32, i32),
) {
    if points.len() < 2 {
        return;
    }
    let project = |segment: &[(i32, i32)]| crate::corridor::project_onto_chunk(segment, 0, place, f32::INFINITY);
    let distance = |segment: &[(i32, i32)]| project(segment).map_or(f32::INFINITY, |p| p.offset_m.abs());
    let mut prior = previous.as_ref().map_or(f32::INFINITY, |segment| distance(segment));
    let cl = cos_lat(points[0].1);
    let mut along = start_m as f32;
    for (index, segment) in points.windows(2).enumerate() {
        let next = if index + 2 < points.len() {
            distance(&points[index + 1..index + 3])
        } else {
            following.as_ref().map_or(f32::INFINITY, |segment| distance(segment))
        };
        if let Some(projection) = project(segment) {
            let offset = projection.offset_m.abs();
            if offset <= limit && offset < prior && offset <= next {
                visit((along + projection.dist_along_m).max(0.0) as u32, libm::roundf(projection.offset_m) as i32);
            }
            prior = offset;
        }
        let (dx, dy) = delta_m(segment[0], segment[1], cl);
        along += libm::sqrtf(dx * dx + dy * dy);
    }
}
