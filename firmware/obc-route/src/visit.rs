//! Bounded visit construction. The owner keeps output A unsealed and reuses B for each leg.
//! This module owns route policy and composition. Platform adapters own reservations and searches.
use crate::convert::{ObcrEmitter, RouteStats};
use crate::reader::{decode_route_points_between_checked, WaypointCursor};
use crate::{RouteReader, MAX_POINTS_PER_CHUNK};
use heapless::Vec;
use obc_formats::bike::BikeType;
use obc_formats::io::{put_i16, put_i32, put_u16, put_u32, ByteSink, Error};
use obc_formats::obcm::{PoiMetadata, SourceId};
use obc_formats::obcr::{RouteSourceKey, VisitDescriptor, WaypointProvenance, HEADER_FULL_LEN, WAYPOINT_LEN};

pub const VISIT_FORWARD_M: u32 = 20_000;
pub const MAX_VISIT_SEARCHES: u8 = 2;
/// A mapped approach must land on the graph, not on a nearby disconnected road.
pub const APPROACH_TOLERANCE_M: f32 = 1.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VisitTarget {
    pub map: RouteSourceKey,
    pub metadata: PoiMetadata,
    pub display: (i32, i32),
}
impl VisitTarget {
    pub fn approach(self, map: RouteSourceKey, profile: BikeType) -> Option<(i32, i32)> {
        if self.map != map || !self.metadata.source.is_valid() {
            return None;
        }
        match self.metadata.approach {
            Some(a) => (a.source.is_valid() && a.profile_mask & (1 << profile as u8) != 0).then_some((a.lon, a.lat)),
            None => Some(self.display),
        }
    }
    /// Explicit approaches must be reached exactly. Other places use the bounded graph snap.
    pub fn validate_destination(self, src: &dyn obc_formats::io::ByteSource, profile: BikeType) -> Result<(), Error> {
        self.destination(src, profile).map(|_| ())
    }
    /// The actual final graph coordinate, validated against this map-bound place.
    pub fn destination(self, src: &dyn obc_formats::io::ByteSource, profile: BikeType) -> Result<(i32, i32), Error> {
        use crate::reader::{decode_chunk_from, parse_chunk_meta, read_header};
        use obc_formats::obcr::CHUNK_META_LEN;
        let approach = self.approach(self.map, profile).ok_or(Error::BadOffset)?;
        let h = read_header(src)?;
        let k = h.chunk_count.checked_sub(1).ok_or(Error::BadOffset)?;
        let offset =
            k.checked_mul(CHUNK_META_LEN as u32).and_then(|n| h.index_offset.checked_add(n)).ok_or(Error::BadOffset)?;
        let mut bytes = [0; CHUNK_META_LEN];
        src.read_at(u64::from(offset), &mut bytes)?;
        let meta = parse_chunk_meta(&bytes, src.len())?;
        if meta.point_count == 0 {
            return Err(Error::BadOffset);
        }
        let mut points = Vec::<crate::RoutePoint, MAX_POINTS_PER_CHUNK>::new();
        decode_chunk_from(src, &meta, meta.point_count as usize, &mut points)?;
        let p = points.last().ok_or(Error::BadOffset)?;
        let tolerance = if self.metadata.approach.is_some() { APPROACH_TOLERANCE_M } else { crate::nav::SNAP_RADIUS_M };
        if obc_map_scene::ground_dist_m((p.lon, p.lat), approach) > tolerance {
            return Err(Error::BadOffset);
        }
        Ok((p.lon, p.lat))
    }
}

/// Elevation confidence and arrival ascent measured from the same complete candidate geometry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VisitCosts {
    pub arrival_ascent_m: u32,
    pub rough_m: u32,
    pub unknown_m: u32,
    pub arrival_elevation_complete: bool,
    pub complete_elevation: bool,
}
impl VisitCosts {
    /// Must stay `#[inline(never)]`: the bounded chunk buffer lives in this popped frame.
    #[inline(never)]
    pub fn read(src: &dyn obc_formats::io::ByteSource, arrival: [u32; 2]) -> Result<Self, Error> {
        use crate::facts::FactsAccumulator;
        use crate::reader::{decode_chunk_from, parse_chunk_meta, read_header};
        use obc_formats::obcr::CHUNK_META_LEN;
        let h = read_header(src)?;
        if h.chunk_count == 0
            || h.chunk_count as usize > crate::MAX_ROUTE_CHUNKS
            || arrival[0] > arrival[1]
            || arrival[1] > h.total_distance_m
        {
            return Err(Error::BadOffset);
        }
        let mut full = FactsAccumulator::new(0, None, 0, h.total_distance_m);
        let mut to_stop = FactsAccumulator::new(0, None, arrival[0], arrival[1]);
        let mut points = Vec::<crate::RoutePoint, MAX_POINTS_PER_CHUNK>::new();
        let mut previous = None;
        let mut count = 0u32;
        for k in 0..h.chunk_count {
            let offset = k
                .checked_mul(CHUNK_META_LEN as u32)
                .and_then(|n| h.index_offset.checked_add(n))
                .ok_or(Error::BadOffset)?;
            let mut bytes = [0; CHUNK_META_LEN];
            src.read_at(u64::from(offset), &mut bytes)?;
            let meta = parse_chunk_meta(&bytes, src.len())?;
            if meta.point_count == 0 {
                return Err(Error::BadOffset);
            }
            points.clear();
            decode_chunk_from(src, &meta, meta.point_count as usize, &mut points)?;
            if let Some(last) = previous {
                let first = points[0];
                if last != (first.lon, first.lat, first.ele) {
                    return Err(Error::BadOffset);
                }
            }
            for &point in points.iter().skip(usize::from(k > 0)) {
                full.push(point, &mut |_| {});
                to_stop.push(point, &mut |_| {});
                count += 1;
                previous = Some((point.lon, point.lat, point.ele));
            }
        }
        let full = full.finish(h.total_distance_m)?;
        let to_stop = to_stop.finish(h.total_distance_m)?;
        if count != h.point_count || full.ascent_m != h.total_ascent_m || full.descent_m != h.total_descent_m {
            return Err(Error::BadOffset);
        }
        Ok(Self {
            arrival_ascent_m: to_stop.ascent_m,
            rough_m: full.rough_m(),
            unknown_m: full.surface_m[0],
            arrival_elevation_complete: to_stop.complete_elevation(),
            complete_elevation: full.complete_elevation(),
        })
    }
}

/// Choose the closest remaining route occurrence in whole metres. A tie keeps the earlier pass.
/// The prefix and tail stay on the original route, so no waypoint or loop is skipped.
pub fn visit_anchor(original: &RouteReader, progress_m: u32, target: (i32, i32)) -> Result<u32, Error> {
    if progress_m > original.total_distance_m
        || original.visit_descriptor()?.is_some_and(|v| progress_m < v.accepted_anchors_m[2])
        || original.has_unresolved_avoidance()
    {
        return Err(Error::BadOffset);
    }
    let mut waypoints = WaypointCursor::new(original.source())?;
    let mut previous = 0;
    while let Some(waypoint) = waypoints.next(original.source())? {
        if waypoint.dist_along_m < previous || waypoint.dist_along_m > original.total_distance_m {
            return Err(Error::BadOffset);
        }
        previous = waypoint.dist_along_m;
    }
    let end = progress_m.saturating_add(VISIT_FORWARD_M).min(original.total_distance_m);
    let origin = original.position_at(progress_m).ok_or(Error::BadOffset)?;
    let mut best = ((obc_map_scene::ground_dist_m((origin.lon, origin.lat), target) + 0.5) as u32, progress_m);
    let mut points = Vec::<_, MAX_POINTS_PER_CHUNK>::new();
    let cl = obc_map_scene::cos_lat(target.1);
    for (k, meta) in original.chunks().iter().enumerate() {
        if meta.cum_distance_m > end {
            break;
        }
        if original.chunks().get(k + 1).is_some_and(|next| next.cum_distance_m < progress_m) {
            continue;
        }
        original.decode_chunk(k, &mut points)?;
        let mut along = meta.cum_distance_m as f64;
        for pair in points.windows(2) {
            let a = (pair[0].lon, pair[0].lat);
            let b = (pair[1].lon, pair[1].lat);
            let length = obc_map_scene::ground_dist_m(a, b) as f64;
            if along + length >= progress_m as f64 && along <= end as f64 && length > 0.0 {
                let (t, _) = crate::geo::project_to_segment(a, b, target, cl);
                let at = (along + t as f64 * length).clamp(progress_m as f64, end as f64);
                let t = ((at - along) / length).clamp(0.0, 1.0);
                let point = (a.0 + ((b.0 - a.0) as f64 * t) as i32, a.1 + ((b.1 - a.1) as f64 * t) as i32);
                let distance = (obc_map_scene::ground_dist_m(point, target) + 0.5) as u32;
                if distance < best.0 {
                    best = (distance, at as u32);
                }
            }
            along += length;
        }
    }
    Ok(best.1)
}

/// The same bounded search counter serves visits and constrained replacements.
#[derive(Debug, Clone, Copy, Default)]
pub struct VisitChoice {
    searches: u8,
}
impl VisitChoice {
    pub fn new() -> Self {
        Self { searches: 0 }
    }
    pub fn search(&mut self) -> Result<(), Error> {
        self.search_with_limit(MAX_VISIT_SEARCHES)
    }
    pub fn search_easier(&mut self) -> Result<(), Error> {
        self.search_with_limit(crate::MAX_WAYPOINTS as u8 + 1)
    }
    fn search_with_limit(&mut self, limit: u8) -> Result<(), Error> {
        if self.searches >= limit {
            return Err(Error::TooLarge);
        }
        self.searches += 1;
        Ok(())
    }
    pub fn searches(&self) -> u8 {
        self.searches
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Begin,
    BeginPrefix,
    Prefix,
    Outbound,
    Return,
    Tail,
    Geometry,
    Waypoints,
    Descriptor,
    Done,
    RejectedGeometry,
}

/// Final emitter state lives in a named arena partition alongside the leg planner. No source,
/// route index or reservation is owned here. Each step writes one chunk or one waypoint record.
pub struct VisitBuilder {
    em: ObcrEmitter,
    easier: Option<crate::easier::Anchors>,
    descriptor: Option<VisitDescriptor>,
    original: RouteSourceKey,
    anchors: [u32; 3],
    accepted_rejoin_m: u32,
    phase: Phase,
    chunk: usize,
    segment_started: bool,
    last: Option<(i32, i32)>,
    last_ele: i16,
    seam_incomplete: bool,
    cursor: Option<WaypointCursor>,
    ordinal: u16,
    count: u16,
    waypoint_offset: u32,
    header: [u8; HEADER_FULL_LEN],
    stats: Option<RouteStats>,
}
impl VisitBuilder {
    pub fn new(
        original: RouteSourceKey,
        map: RouteSourceKey,
        departure_m: u32,
        rejoin_m: u32,
        target: SourceId,
        approach: (i32, i32),
    ) -> Result<Self, Error> {
        let mut slot = core::mem::MaybeUninit::uninit();
        unsafe {
            Self::init_in_place(slot.as_mut_ptr(), original, map, departure_m, rejoin_m, target, approach)?;
            Ok(slot.assume_init())
        }
    }

    /// Initialize or reset the persistent final emitter without a second emitter on the stack.
    /// On an invalid descriptor no bytes in `slot` are changed.
    ///
    /// # Safety
    /// `slot` must be aligned, writable and exclusively owned for a complete builder. The caller
    /// must discard any previous output before resetting it.
    pub unsafe fn init_in_place(
        slot: *mut Self,
        original: RouteSourceKey,
        map: RouteSourceKey,
        departure_m: u32,
        rejoin_m: u32,
        target: SourceId,
        approach: (i32, i32),
    ) -> Result<(), Error> {
        if !target.is_valid() || rejoin_m < departure_m {
            return Err(Error::BadOffset);
        }
        let descriptor = VisitDescriptor {
            original,
            original_anchors_m: [departure_m, departure_m, rejoin_m],
            accepted_anchors_m: [0; 3],
            target_id: target.0 & ((1 << 62) - 1),
            target_kind: (target.0 >> 62) as u8,
            target_lon: approach.0,
            target_lat: approach.1,
        };
        descriptor.encode().map_err(|_| Error::BadOffset)?;
        unsafe { Self::init(slot, original, map, Some(descriptor), [departure_m, departure_m, rejoin_m]) }
    }
    /// Initialize a single real connector followed by the existing accepted tail. No visit remains.
    ///
    /// # Safety
    /// The same placement and output ownership requirements as `init_in_place` apply.
    pub unsafe fn init_return_in_place(
        slot: *mut Self,
        original: RouteSourceKey,
        map: RouteSourceKey,
        rejoin_m: u32,
    ) -> Result<(), Error> {
        unsafe { Self::init(slot, original, map, None, [rejoin_m; 3]) }
    }
    unsafe fn init(
        slot: *mut Self,
        original: RouteSourceKey,
        map: RouteSourceKey,
        descriptor: Option<VisitDescriptor>,
        anchors: [u32; 3],
    ) -> Result<(), Error> {
        use core::ptr::addr_of_mut;
        unsafe {
            ObcrEmitter::init_in_place(addr_of_mut!((*slot).em));
            addr_of_mut!((*slot).descriptor).write(descriptor);
            addr_of_mut!((*slot).easier).write(None);
            addr_of_mut!((*slot).original).write(original);
            addr_of_mut!((*slot).anchors).write(anchors);
            addr_of_mut!((*slot).accepted_rejoin_m).write(0);
            addr_of_mut!((*slot).phase).write(Phase::Begin);
            addr_of_mut!((*slot).chunk).write(0);
            addr_of_mut!((*slot).segment_started).write(false);
            addr_of_mut!((*slot).last).write(None);
            addr_of_mut!((*slot).last_ele).write(i16::MIN);
            addr_of_mut!((*slot).seam_incomplete).write(false);
            addr_of_mut!((*slot).cursor).write(None);
            addr_of_mut!((*slot).ordinal).write(0);
            addr_of_mut!((*slot).count).write(0);
            addr_of_mut!((*slot).waypoint_offset).write(0);
            addr_of_mut!((*slot).header).write([0; HEADER_FULL_LEN]);
            addr_of_mut!((*slot).stats).write(None);
            let Self {
                em: _,
                easier: _,
                descriptor: _,
                original: _,
                anchors: _,
                accepted_rejoin_m: _,
                phase: _,
                chunk: _,
                segment_started: _,
                last: _,
                last_ele: _,
                seam_incomplete: _,
                cursor: _,
                ordinal: _,
                count: _,
                waypoint_offset: _,
                header: _,
                stats: _,
            } = &*slot;
            (*slot).em.set_attribution_map(Some(map));
            (*slot).em.set_flags(obc_formats::obcr::FLAG_ASSISTANT_CANDIDATE);
        }
        Ok(())
    }

    /// Reuse the same candidate emitter for a complete constrained replacement.
    /// # Safety
    /// The same placement and ownership requirements as `init_in_place` apply.
    pub unsafe fn init_easier_in_place(
        slot: *mut Self,
        original: RouteSourceKey,
        map: RouteSourceKey,
        progress: u32,
    ) -> Result<(), Error> {
        unsafe { Self::init(slot, original, map, None, [progress; 3]) }
    }
    pub fn prepare_easier(&mut self, original: &RouteReader) -> Result<(), Error> {
        self.easier = Some(crate::easier::Anchors::read(original, self.anchors[0])?);
        Ok(())
    }
    pub fn easier_leg(
        &mut self,
        original: &RouteReader,
        origin: (i32, i32),
    ) -> Result<Option<crate::easier::LegEndpoints>, Error> {
        let anchors = self.easier.as_mut().ok_or(Error::BadOffset)?;
        if anchors.finished {
            return Ok(None);
        }
        anchors.advance(original)?;
        let from = self.last.unwrap_or(origin);
        while obc_map_scene::ground_dist_m(from, anchors.target) <= APPROACH_TOLERANCE_M {
            anchors.reached(self.em.distance_m(), original.total_distance_m);
            if anchors.finished {
                self.phase = Phase::Geometry;
                return Ok(None);
            }
            anchors.advance(original)?;
        }
        Ok(Some((from, anchors.target)))
    }
    pub fn easier_finished(&self) -> bool {
        self.easier.as_ref().is_some_and(|a| a.finished)
    }

    pub fn begin(&mut self, sink: &mut dyn ByteSink) -> Result<(), Error> {
        if !matches!(self.phase, Phase::Begin | Phase::BeginPrefix) {
            return Err(Error::BadOffset);
        }
        ObcrEmitter::begin(sink)?;
        self.phase = if self.descriptor.is_some() {
            if self.phase == Phase::BeginPrefix || self.anchors[1] > self.anchors[0] {
                Phase::Prefix
            } else {
                Phase::Outbound
            }
        } else {
            Phase::Return
        };
        Ok(())
    }
    /// Retain the journey up to the single leave/rejoin anchor before the two excursion legs.
    pub fn keep_prefix(&mut self, anchor_m: u32) -> Result<(), Error> {
        if self.phase != Phase::Begin || anchor_m < self.anchors[0] {
            return Err(Error::BadOffset);
        }
        let descriptor = self.descriptor.as_mut().ok_or(Error::BadOffset)?;
        self.anchors[1] = anchor_m;
        self.anchors[2] = anchor_m;
        descriptor.original_anchors_m = self.anchors;
        self.phase = Phase::BeginPrefix;
        Ok(())
    }
    pub fn append_prefix_step(&mut self, original: &RouteReader, sink: &mut dyn ByteSink) -> Result<bool, Error> {
        if self.phase != Phase::Prefix {
            return Ok(true);
        }
        if self.append_chunk(original, self.anchors[0], self.anchors[1], sink)? {
            return Ok(false);
        }
        self.chunk = 0;
        self.segment_started = false;
        self.phase = Phase::Outbound;
        Ok(true)
    }
    pub fn arrival_m(&self) -> u32 {
        self.descriptor.map_or(0, |descriptor| descriptor.accepted_anchors_m[1])
    }
    pub fn original_anchors(&self) -> [u32; 3] {
        self.anchors
    }

    /// Bind the stop to the actual outbound graph endpoint before composing the route.
    pub fn resolve_destination(
        &mut self,
        target: VisitTarget,
        leg: &dyn obc_formats::io::ByteSource,
        profile: BikeType,
    ) -> Result<(), Error> {
        let descriptor = self.descriptor.as_mut().ok_or(Error::BadOffset)?;
        if self.phase != Phase::Outbound
            || self.chunk != 0
            || descriptor.target_kind != (target.metadata.source.0 >> 62) as u8
            || descriptor.target_id != target.metadata.source.0 & ((1 << 62) - 1)
            || self.em.attribution_map() != Some(target.map)
        {
            return Err(Error::BadOffset);
        }
        let (lon, lat) = target.destination(leg, profile)?;
        descriptor.target_lon = lon;
        descriptor.target_lat = lat;
        Ok(())
    }
    pub fn destination(&self) -> Option<(i32, i32)> {
        self.descriptor.map(|d| (d.target_lon, d.target_lat))
    }

    /// A decoded leg cannot join the next required coordinate. A source or sink failure does not
    /// set this state.
    pub fn rejected_geometry(&self) -> bool {
        self.phase == Phase::RejectedGeometry
    }

    /// Append the sealed outbound or return B. `true` releases B before the next leg starts.
    pub fn append_leg_step(&mut self, leg: &RouteReader, sink: &mut dyn ByteSink) -> Result<bool, Error> {
        if !matches!(self.phase, Phase::Outbound | Phase::Return) || leg.chunks().is_empty() {
            return Err(Error::BadOffset);
        }
        if self.append_chunk(leg, 0, leg.total_distance_m, sink)? {
            return Ok(false);
        }
        if let Some(anchors) = &mut self.easier {
            if self.last.is_none_or(|last| obc_map_scene::ground_dist_m(last, anchors.target) > APPROACH_TOLERANCE_M) {
                self.phase = Phase::RejectedGeometry;
                return Err(Error::BadOffset);
            }
            // The destination is the original route's terminal anchor.
            self.chunk = 0;
            self.segment_started = false;
            return Ok(true);
        }
        if self.phase == Phase::Outbound {
            let descriptor = self.descriptor.as_mut().ok_or(Error::BadOffset)?;
            if self.last.is_none_or(|p| {
                obc_map_scene::ground_dist_m(p, (descriptor.target_lon, descriptor.target_lat)) > APPROACH_TOLERANCE_M
            }) {
                self.phase = Phase::RejectedGeometry;
                return Err(Error::BadOffset);
            }
            descriptor.accepted_anchors_m[1] = self.em.distance_m();
            self.phase = Phase::Return;
        } else {
            self.accepted_rejoin_m = self.em.distance_m();
            if let Some(descriptor) = &mut self.descriptor {
                descriptor.accepted_anchors_m[2] = self.accepted_rejoin_m;
            }
            self.phase = Phase::Tail;
        }
        self.chunk = 0;
        self.segment_started = false;
        Ok(true)
    }

    pub fn finish_easier_leg(&mut self, original: &RouteReader) -> Result<(), Error> {
        let anchors = self.easier.as_mut().ok_or(Error::BadOffset)?;
        anchors.reached(self.em.distance_m(), original.total_distance_m);
        if anchors.finished {
            self.phase = Phase::Geometry;
        }
        Ok(())
    }

    /// Finish with the original tail, the stored waypoint section and the visit descriptor.
    /// `Some` is terminal. Any error requires discarding A.
    pub fn finish_step(
        &mut self,
        original: &RouteReader,
        sink: &mut dyn ByteSink,
    ) -> Result<Option<RouteStats>, Error> {
        let rejoin = self.anchors[2];
        match self.phase {
            Phase::Tail => {
                if original.visit_descriptor()?.is_some_and(|v| self.anchors[0] < v.accepted_anchors_m[2])
                    || original.has_unresolved_avoidance()
                    || rejoin > original.total_distance_m
                {
                    return Err(Error::BadOffset);
                }
                if !self.append_chunk(original, rejoin, original.total_distance_m, sink)? {
                    self.phase = Phase::Geometry;
                }
            }
            Phase::Geometry => {
                self.em.set_bike_type(original.bike_type());
                let mut capture = HeaderSink { sink, header: &mut self.header };
                self.stats = Some(self.em.finish(
                    &mut capture,
                    if self.easier.is_some() { "Easier route" } else { "Visit" },
                    &mut Vec::new(),
                )?);
                self.waypoint_offset = self.em.geometry_end();
                self.cursor = Some(WaypointCursor::new(original.source())?);
                self.phase = Phase::Waypoints;
            }
            Phase::Waypoints => {
                let Some(mut w) = self.cursor.as_mut().ok_or(Error::BadOffset)?.next(original.source())? else {
                    self.phase = Phase::Descriptor;
                    return Ok(None);
                };
                let ordinal = self.ordinal;
                self.ordinal = self.ordinal.checked_add(1).ok_or(Error::TooLarge)?;
                let departure = self.anchors[0];
                if w.dist_along_m < departure {
                    return Ok(None);
                }
                if (w.dist_along_m > self.anchors[1] && w.dist_along_m < rejoin)
                    || w.dist_along_m > original.total_distance_m
                {
                    return Err(Error::BadOffset);
                }
                w.provenance.get_or_insert(WaypointProvenance { source: self.original, ordinal });
                w.dist_along_m = if let Some(anchors) = &self.easier {
                    anchors.mapped(w.dist_along_m)?
                } else if w.dist_along_m < self.anchors[1] {
                    w.dist_along_m - departure
                } else if w.dist_along_m == original.total_distance_m {
                    self.em.distance_m()
                } else {
                    self.accepted_rejoin_m.saturating_add(w.dist_along_m - rejoin).min(self.em.distance_m())
                };
                // The retained tail has the same orientation and access geometry, so its signed
                // lateral offset is unchanged.
                let mut bytes = [0; WAYPOINT_LEN];
                put_u32(&mut bytes, 0, w.dist_along_m);
                put_i32(&mut bytes, 4, w.lon);
                put_i32(&mut bytes, 8, w.lat);
                put_i16(&mut bytes, 12, w.ele);
                bytes[14] = w.category_id;
                bytes[15] = w.name.len() as u8;
                put_i16(&mut bytes, 16, w.lateral_offset_m);
                bytes[20..20 + w.name.len()].copy_from_slice(w.name.as_bytes());
                bytes[44..80].copy_from_slice(&w.provenance.unwrap().encode());
                sink.write(&bytes)?;
                self.count = self.count.checked_add(1).ok_or(Error::TooLarge)?;
            }
            Phase::Descriptor => {
                let offset = self
                    .waypoint_offset
                    .checked_add(u32::from(self.count) * WAYPOINT_LEN as u32)
                    .ok_or(Error::TooLarge)?;
                if let Some(descriptor) = self.descriptor {
                    sink.write(&descriptor.encode().map_err(|_| Error::BadOffset)?)?;
                    self.header[118] = 1;
                    put_u32(&mut self.header, 120, offset);
                    put_u32(&mut self.header, 124, 80);
                }
                put_u32(&mut self.header, 112, if self.count == 0 { 0 } else { self.waypoint_offset });
                put_u16(&mut self.header, 116, self.count);
                sink.patch_at(0, &self.header)?;
                self.stats.as_mut().ok_or(Error::BadOffset)?.waypoint_count = self.count;
                self.phase = Phase::Done;
                return Ok(self.stats);
            }
            Phase::Done => return Ok(self.stats),
            _ => return Err(Error::BadOffset),
        }
        Ok(None)
    }

    fn append_chunk(
        &mut self,
        route: &RouteReader,
        from: u32,
        to: u32,
        sink: &mut dyn ByteSink,
    ) -> Result<bool, Error> {
        if self.chunk >= route.chunks().len() || route.chunks()[self.chunk].cum_distance_m > to {
            return Ok(false);
        }
        let mut points = Vec::<_, MAX_POINTS_PER_CHUNK>::new();
        // A stored total is floored to metres. Keep the final stored endpoint, not a second
        // sub-metre clip of it, or consecutive legs would no longer have the same seam.
        let upper = if to == route.total_distance_m { u32::MAX } else { to };
        let found = if route.total_distance_m == 0 && route.chunks()[self.chunk].point_count == 1 {
            route.decode_chunk(self.chunk, &mut points)?;
            Some(points.len())
        } else {
            decode_route_points_between_checked(route, self.chunk, from, upper, &mut points)?
        };
        self.chunk += 1;
        if found.is_none() {
            return Ok(true);
        }
        let seam = !self.segment_started && self.last.is_some();
        let gap =
            self.last.zip(points.first()).map_or(0.0, |(last, p)| obc_map_scene::ground_dist_m(last, (p.lon, p.lat)));
        let route_join = self.descriptor.is_some() && matches!(self.phase, Phase::Outbound | Phase::Tail);
        let tolerance = if route_join { crate::nav::SNAP_RADIUS_M } else { APPROACH_TOLERANCE_M };
        if seam && (points.is_empty() || gap > tolerance) {
            self.phase = Phase::RejectedGeometry;
            return Err(Error::BadOffset);
        }
        self.segment_started = true;
        let source_surface = route.attribution_map()? == self.em.attribution_map();
        for (i, p) in points.iter().enumerate() {
            let coord = (p.lon, p.lat);
            let connector = i == 0 && seam && gap > APPROACH_TOLERANCE_M;
            if i == 0 && ((seam && !connector) || self.last == Some(coord)) {
                // Coalesce sub-metre quantization at the existing endpoint and add no connector.
                self.seam_incomplete |= self.last != Some(coord) || self.last_ele != p.ele;
                continue;
            }
            // Imported route geometry can differ from the normal graph snap. Retain both ends
            // and charge the connection to the visit; it has no mapped surface or elevation.
            self.em.set_surface(if source_surface && !connector { p.surface } else { 0 });
            self.em.set_elevation_incomplete(p.elevation_incomplete || self.seam_incomplete || connector);
            self.em.push_retained(sink, p.lon, p.lat, p.ele)?;
            if connector && self.phase == Phase::Tail {
                self.accepted_rejoin_m = self.em.distance_m();
                if let Some(descriptor) = &mut self.descriptor {
                    descriptor.accepted_anchors_m[2] = self.accepted_rejoin_m;
                }
            }
            self.last = Some(coord);
            self.last_ele = p.ele;
            self.seam_incomplete = false;
        }
        Ok(true)
    }
}

struct HeaderSink<'a> {
    sink: &'a mut dyn ByteSink,
    header: &'a mut [u8; HEADER_FULL_LEN],
}
impl ByteSink for HeaderSink<'_> {
    fn write(&mut self, bytes: &[u8]) -> Result<(), Error> {
        self.sink.write(bytes)
    }
    fn patch_at(&mut self, at: u32, bytes: &[u8]) -> Result<(), Error> {
        if at != 0 || bytes.len() != self.header.len() {
            return Err(Error::BadOffset);
        }
        self.header.copy_from_slice(bytes);
        self.sink.patch_at(at, bytes)
    }
}
