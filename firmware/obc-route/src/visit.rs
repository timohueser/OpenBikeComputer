//! Bounded visit construction. The owner keeps output A unsealed and reuses B for each leg.
//! This module owns route policy and composition; platform adapters own reservations and searches.
use crate::convert::{ObcrEmitter, RouteStats};
use crate::reader::{decode_route_points_between_checked, WaypointCursor};
use crate::{RouteReader, MAX_POINTS_PER_CHUNK};
use heapless::Vec;
use obc_formats::io::{put_i16, put_i32, put_u16, put_u32, ByteSink, Error};
use obc_formats::obcm::{PoiMetadata, SourceId};
use obc_formats::obcr::{RouteSourceKey, VisitDescriptor, WaypointProvenance, HEADER_FULL_LEN, WAYPOINT_LEN};

pub const FORWARD_REJOIN_M: u32 = 1_000;
pub const MAX_VISIT_SEARCHES: u8 = 6;
/// A mapped approach must land on the graph, not on a nearby disconnected road.
pub const APPROACH_TOLERANCE_M: f32 = 1.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VisitTarget {
    pub map: RouteSourceKey,
    pub metadata: PoiMetadata,
    pub display: (i32, i32),
}
impl VisitTarget {
    pub fn approach(self, map: RouteSourceKey, profile: u8) -> Option<(i32, i32)> {
        let a = self.metadata.approach?;
        (self.map == map
            && self.metadata.source.is_valid()
            && a.source.is_valid()
            && profile < 8
            && a.profile_mask & (1 << profile) != 0)
            .then_some((a.lon, a.lat))
    }
}

/// Original route constraints are scanned from the complete stored section, never the UI window.
/// A forward join stops at the first remaining annotation's on-route access anchor.
pub fn forward_rejoin(original: &RouteReader, departure_m: u32) -> Result<Option<u32>, Error> {
    if departure_m >= original.total_distance_m
        || original.visit_descriptor()?.is_some_and(|v| departure_m < v.accepted_anchors_m[2])
        || original.has_unresolved_avoidance()
    {
        return Ok(None);
    }
    let mut limit = original.total_distance_m;
    let mut cursor = WaypointCursor::new(original.source())?;
    let mut previous = 0;
    while let Some(w) = cursor.next(original.source())? {
        if w.dist_along_m < previous || w.dist_along_m > original.total_distance_m {
            return Err(Error::BadOffset);
        }
        previous = w.dist_along_m;
        if w.dist_along_m >= departure_m {
            limit = limit.min(w.dist_along_m);
        }
    }
    let at = departure_m.saturating_add(FORWARD_REJOIN_M).min(limit);
    Ok((at > departure_m && original.position_at(at).is_some()).then_some(at))
}

/// One materialized variant at a time. The second may replace the first only on measured cost.
#[derive(Debug, Clone, Copy)]
pub struct VisitChoice {
    pub departure_m: u32,
    pub forward_m: Option<u32>,
    outbound_cost: Option<u32>,
    searches: u8,
}
impl VisitChoice {
    pub fn new(departure_m: u32, forward_m: Option<u32>) -> Self {
        Self { departure_m, forward_m, outbound_cost: None, searches: 0 }
    }
    pub fn search(&mut self) -> Result<(), Error> {
        if self.searches >= MAX_VISIT_SEARCHES {
            return Err(Error::TooLarge);
        }
        self.searches += 1;
        Ok(())
    }
    pub fn searches(&self) -> u8 {
        self.searches
    }
    pub fn remember_out_and_back(&mut self, distance_m: u32) {
        self.outbound_cost = Some(distance_m);
    }
    pub fn prefer_forward(&self, distance_m: u32) -> bool {
        self.outbound_cost.is_some_and(|baseline| distance_m < baseline)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Begin,
    Outbound,
    Return,
    Tail,
    Geometry,
    Waypoints,
    Descriptor,
    Done,
}

/// Final emitter state lives in a named arena partition alongside the leg planner. No source,
/// route index or reservation is owned here. Each step writes one chunk or one waypoint record.
pub struct VisitBuilder {
    em: ObcrEmitter,
    descriptor: VisitDescriptor,
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
    /// must discard any previous output before resetting it; this value owns no external resources.
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
        use core::ptr::addr_of_mut;
        unsafe {
            ObcrEmitter::init_in_place(addr_of_mut!((*slot).em));
            addr_of_mut!((*slot).descriptor).write(descriptor);
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
                descriptor: _,
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

    pub fn begin(&mut self, sink: &mut dyn ByteSink) -> Result<(), Error> {
        if self.phase != Phase::Begin {
            return Err(Error::BadOffset);
        }
        ObcrEmitter::begin(sink)?;
        self.phase = Phase::Outbound;
        Ok(())
    }
    pub fn descriptor(&self) -> VisitDescriptor {
        self.descriptor
    }

    /// Append the sealed outbound or return B. `true` releases B before the next leg starts.
    pub fn append_leg_step(&mut self, leg: &RouteReader, sink: &mut dyn ByteSink) -> Result<bool, Error> {
        if !matches!(self.phase, Phase::Outbound | Phase::Return) || leg.chunks().is_empty() {
            return Err(Error::BadOffset);
        }
        if self.append_chunk(leg, 0, leg.total_distance_m, sink)? {
            return Ok(false);
        }
        if self.phase == Phase::Outbound {
            if self.last.is_none_or(|p| {
                obc_map_scene::ground_dist_m(p, (self.descriptor.target_lon, self.descriptor.target_lat))
                    > APPROACH_TOLERANCE_M
            }) {
                return Err(Error::BadOffset);
            }
            self.descriptor.accepted_anchors_m[1] = self.em.distance_m();
            self.phase = Phase::Return;
        } else {
            self.descriptor.accepted_anchors_m[2] = self.em.distance_m();
            self.phase = Phase::Tail;
        }
        self.chunk = 0;
        self.segment_started = false;
        Ok(true)
    }

    /// Finish with the original tail, complete stored waypoint section and visit descriptor.
    /// `Some` is terminal; any error requires discarding A.
    pub fn finish_step(
        &mut self,
        original: &RouteReader,
        sink: &mut dyn ByteSink,
    ) -> Result<Option<RouteStats>, Error> {
        let rejoin = self.descriptor.original_anchors_m[2];
        match self.phase {
            Phase::Tail => {
                if original
                    .visit_descriptor()?
                    .is_some_and(|v| self.descriptor.original_anchors_m[0] < v.accepted_anchors_m[2])
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
                let mut capture = HeaderSink { sink, header: &mut self.header };
                self.stats = Some(self.em.finish(&mut capture, "Visit", &mut Vec::new())?);
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
                let departure = self.descriptor.original_anchors_m[0];
                if w.dist_along_m < departure {
                    return Ok(None);
                }
                if w.dist_along_m < rejoin || w.dist_along_m > original.total_distance_m {
                    return Err(Error::BadOffset);
                }
                w.provenance.get_or_insert(WaypointProvenance { source: self.descriptor.original, ordinal });
                w.dist_along_m = if w.dist_along_m == original.total_distance_m {
                    self.em.distance_m()
                } else {
                    self.descriptor.accepted_anchors_m[2]
                        .saturating_add(w.dist_along_m - rejoin)
                        .min(self.em.distance_m())
                };
                // The retained tail has the same orientation and access geometry, so its signed
                // lateral offset is unchanged. The annotation's display coordinate is never routed to.
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
                sink.write(&self.descriptor.encode().map_err(|_| Error::BadOffset)?)?;
                put_u32(&mut self.header, 112, if self.count == 0 { 0 } else { self.waypoint_offset });
                put_u16(&mut self.header, 116, self.count);
                self.header[118] = 1;
                put_u32(&mut self.header, 120, offset);
                put_u32(&mut self.header, 124, 80);
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
        if self.chunk >= route.chunks().len() {
            return Ok(false);
        }
        let mut points = Vec::<_, MAX_POINTS_PER_CHUNK>::new();
        // A stored total is floored to metres. Keep the final stored endpoint, not a second
        // sub-metre clip of it, or consecutive legs would no longer have the same seam.
        let upper = if to == route.total_distance_m { u32::MAX } else { to };
        let found = decode_route_points_between_checked(route, self.chunk, from, upper, &mut points)?;
        self.chunk += 1;
        if found.is_none() {
            return Ok(true);
        }
        if !self.segment_started && self.last.is_some_and(|last| points.first().is_none_or(|p| last != (p.lon, p.lat)))
        {
            return Err(Error::BadOffset);
        }
        self.segment_started = true;
        let source_surface = route.attribution_map()? == self.em.attribution_map();
        for (i, p) in points.iter().enumerate() {
            let coord = (p.lon, p.lat);
            if i == 0 && self.last == Some(coord) {
                self.seam_incomplete |= self.last_ele != p.ele;
                continue;
            }
            self.em.set_surface(if source_surface { p.surface } else { 0 });
            self.em.set_elevation_incomplete(p.elevation_incomplete || self.seam_incomplete);
            self.em.push_retained(sink, p.lon, p.lat, p.ele)?;
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
