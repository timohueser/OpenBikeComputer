//! One bounded stored-route composition step. Owners choose the segment and seam policy.
use crate::convert::ObcrWriter;
use crate::{RoutePoint, RouteReader};
use obc_formats::io::{ByteSink, Error};
use obc_map_scene::ground_dist_m;

#[derive(Clone, Copy)]
pub(crate) struct Blend {
    pub start: f32,
    pub end: f32,
    pub length: f32,
    pub valid: bool,
}
#[derive(Clone, Copy)]
pub(crate) enum Segment {
    Stored { lo: u32, hi: u32, retain_end: bool, read_to_end: bool },
    Leg(Blend),
    Prefix { through: usize },
}
#[derive(Clone, Copy)]
pub(crate) enum Seam {
    Exact,
    Joined { tolerance: f32 },
    ChunkAnchor,
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Source {
    Original,
    Leg,
}
#[derive(Clone, Copy)]
pub(crate) struct Part {
    pub source: Source,
    pub segment: Segment,
    pub seam: Seam,
    pub chunk: usize,
}
pub(crate) struct Composed {
    pub more: bool,
    pub first_m: Option<u32>,
    pub connector_m: Option<u32>,
}
pub(crate) struct Compose {
    pub writer: ObcrWriter,
    pub chunk: usize,
    pub last: Option<(i32, i32)>,
    last_ele: i16,
    incomplete: bool,
    started: bool,
    previous: Option<(i32, i32)>,
    along: f32,
    points: usize,
    pub rejected: bool,
    plan: heapless::Vec<Part, 3>,
    part: usize,
}
impl Compose {
    pub unsafe fn init_in_place(slot: *mut Self) {
        use core::ptr::addr_of_mut;
        unsafe {
            ObcrWriter::init_in_place(addr_of_mut!((*slot).writer));
            addr_of_mut!((*slot).chunk).write(0);
            addr_of_mut!((*slot).last).write(None);
            addr_of_mut!((*slot).last_ele).write(i16::MIN);
            addr_of_mut!((*slot).incomplete).write(false);
            addr_of_mut!((*slot).started).write(false);
            addr_of_mut!((*slot).previous).write(None);
            addr_of_mut!((*slot).along).write(0.0);
            addr_of_mut!((*slot).points).write(0);
            addr_of_mut!((*slot).rejected).write(false);
            addr_of_mut!((*slot).plan).write(heapless::Vec::new());
            addr_of_mut!((*slot).part).write(0);
            let Self {
                writer: _,
                chunk: _,
                last: _,
                last_ele: _,
                incomplete: _,
                started: _,
                previous: _,
                along: _,
                points: _,
                rejected: _,
                plan: _,
                part: _,
            } = &*slot;
        }
    }
    pub fn empty() -> Self {
        Self {
            writer: ObcrWriter::empty(),
            chunk: 0,
            last: None,
            last_ele: i16::MIN,
            incomplete: false,
            started: false,
            previous: None,
            along: 0.0,
            points: 0,
            rejected: false,
            plan: heapless::Vec::new(),
            part: 0,
        }
    }
    pub fn plan(&mut self, parts: &[Part]) -> Result<(), Error> {
        self.plan.clear();
        self.plan.extend_from_slice(parts).map_err(|_| Error::TooLarge)?;
        self.part = 0;
        self.reset_chunk(parts.first().map_or(0, |part| part.chunk));
        Ok(())
    }
    pub fn source(&self) -> Option<Source> {
        self.plan.get(self.part).map(|part| part.source)
    }
    pub fn step(&mut self, route: &RouteReader, sink: &mut dyn ByteSink) -> Result<Composed, Error> {
        let Some(part) = self.plan.get(self.part).copied() else {
            return Ok(Composed { more: false, first_m: None, connector_m: None });
        };
        let mut result = self.chunk_step(route, part.segment, part.seam, sink)?;
        if !result.more {
            self.part += 1;
            self.reset_chunk(self.plan.get(self.part).map_or(0, |part| part.chunk));
        }
        result.more = self.part < self.plan.len();
        Ok(result)
    }
    pub fn next_segment(&mut self, chunk: usize) {
        self.plan.clear();
        self.part = 0;
        self.reset_chunk(chunk);
    }
    fn reset_chunk(&mut self, chunk: usize) {
        self.chunk = chunk;
        self.started = false;
        self.previous = None;
        self.along = 0.0;
        self.points = 0;
    }
    fn chunk_step(
        &mut self,
        route: &RouteReader,
        segment: Segment,
        seam: Seam,
        sink: &mut dyn ByteSink,
    ) -> Result<Composed, Error> {
        let k = self.chunk;
        let mut result = Composed { more: false, first_m: None, connector_m: None };
        if k >= route.chunks().len()
            || matches!(segment, Segment::Stored { hi, read_to_end: false, .. } if route.chunks()[k].cum_distance_m > hi)
        {
            return Ok(result);
        }
        result.more = true;
        let mut start = None;
        let mut attributed = true;
        let mut push = |mut p: RoutePoint| -> Result<(), Error> {
            let coord = (p.lon, p.lat);
            if let Segment::Leg(blend) = segment {
                if let Some(previous) = self.previous {
                    if previous == coord {
                        return Ok(());
                    }
                    self.along += ground_dist_m(previous, coord);
                }
                self.previous = Some(coord);
                if p.elevation().is_none() {
                    p.ele = i16::MIN;
                } else if blend.valid {
                    let t = if blend.length > 1e-3 { (self.along / blend.length).clamp(0.0, 1.0) } else { 1.0 };
                    p.ele = libm::roundf(p.ele as f32 + (blend.start + (blend.end - blend.start) * t))
                        .clamp((i16::MIN + 1) as f32, i16::MAX as f32) as i16;
                }
            }
            let first = start.is_none();
            let (joined, gap) = match start {
                Some(s) => s,
                None => {
                    let joined = !self.started && self.last.is_some();
                    let gap = self.last.map_or(0.0, |last| ground_dist_m(last, coord));
                    if let Seam::Joined { tolerance } = seam {
                        if joined && gap > tolerance {
                            self.rejected = true;
                            return Err(Error::BadOffset);
                        }
                        attributed = route.attribution_map()? == self.writer.attribution_map();
                    }
                    self.started = true;
                    *start.insert((joined, gap))
                }
            };
            let connector =
                matches!(seam, Seam::Joined { .. }) && first && joined && gap > crate::visit::APPROACH_TOLERANCE_M;
            let coalesce =
                matches!(seam, Seam::Joined { .. }) && first && ((joined && !connector) || self.last == Some(coord));
            if coalesce {
                self.incomplete |= self.last != Some(coord) || self.last_ele != p.ele;
            } else {
                self.writer.set_surface(if attributed && !connector { p.surface } else { 0 });
                self.writer.set_elevation_incomplete(p.elevation_incomplete || self.incomplete || connector);
                if !matches!(seam, Seam::Exact) || self.last != Some(coord) {
                    self.writer.push_retained(sink, p.lon, p.lat, p.ele)?;
                    self.last = Some(coord);
                    self.last_ele = p.ele;
                    self.incomplete = false;
                }
                if connector {
                    result.connector_m = Some(self.writer.distance_m());
                }
            }
            result.first_m.get_or_insert(self.writer.distance_m());
            Ok(())
        };
        match segment {
            Segment::Stored { lo, hi, retain_end, .. } => {
                if retain_end && route.total_distance_m == 0 && route.chunks()[k].point_count == 1 {
                    route.with_chunk(k, |mut points| points.try_for_each(&mut push))??;
                } else {
                    route.clip_chunk(
                        k,
                        lo,
                        if retain_end && hi == route.total_distance_m { u32::MAX } else { hi },
                        &mut push,
                    )?;
                }
            }
            Segment::Leg(_) => {
                route.with_chunk(k, |mut points| points.try_for_each(&mut push))??;
            }
            Segment::Prefix { through } => {
                let mut count = self.points;
                let finished = route.with_chunk(k, |points| {
                    for p in points.skip(usize::from(k > 0)) {
                        push(p)?;
                        if count == through {
                            return Ok::<_, Error>(true);
                        }
                        count += 1;
                    }
                    Ok(false)
                })??;
                self.points = count;
                if finished {
                    result.more = false;
                }
            }
        };
        self.chunk += 1;
        Ok(result)
    }
}

pub(crate) enum WaypointMap<'a> {
    Splice { split: Option<u32>, rejoin: u32, tail: u32 },
    Visit { anchors: [u32; 3], tail: u32, easier: Option<&'a crate::easier::Anchors> },
}
impl WaypointMap<'_> {
    pub fn map(&self, at: u32, original_total: u32, total: u32) -> Result<Option<u32>, Error> {
        let (departure, head, inclusive, rejoin, tail, strict, easier) = match self {
            Self::Splice { split, rejoin, tail } => (0, *split, true, *rejoin, *tail, false, None),
            Self::Visit { anchors, tail, easier } => {
                (anchors[0], Some(anchors[1]), false, anchors[2], *tail, true, *easier)
            }
        };
        if at < departure {
            return Ok(None);
        }
        if strict && (head.is_some_and(|leave| at > leave && at < rejoin) || at > original_total) {
            return Err(Error::BadOffset);
        }
        if let Some(anchors) = easier {
            return anchors.mapped(at).map(Some);
        }
        if head.is_some_and(|end| at < end || (inclusive && at == end)) {
            return Ok(Some(at - departure));
        }
        if !strict && at < rejoin {
            return Ok(None);
        }
        Ok(Some(if at >= original_total { total } else { tail.saturating_add(at - rejoin).min(total) }))
    }
}
