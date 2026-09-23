//! Measured facts for one exact route parse and a clipped interval of its stored geometry.
use crate::{RoutePoint, RouteReader, MAX_POINTS_PER_CHUNK};
use heapless::Vec;
use obc_elevation::DeadBand;
use obc_formats::{io::Error, obcr::FACTS_POLICY};
use obc_map_scene::ground_dist_m;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IntervalFacts {
    /// In-process identity of the immutable RouteIndex. Durable users also bind the source key/CRC.
    pub route_identity: u32,
    pub policy: u16,
    pub attribution_map: Option<obc_formats::obcr::RouteSourceKey>,
    pub start_m: u32,
    pub end_m: u32,
    pub ascent_m: u32,
    pub descent_m: u32,
    pub elevation_known_m: u32,
    /// Unknown, paved, compacted, gravel, dirt, rough, cobbles, grass.
    pub surface_m: [u32; 8],
}
impl IntervalFacts {
    pub fn distance_m(&self) -> u32 {
        self.end_m - self.start_m
    }
    pub fn complete_elevation(&self) -> bool {
        self.elevation_known_m == self.distance_m()
    }
    pub fn complete_surface(&self) -> bool {
        self.surface_m[0] == 0
    }
    pub fn surface_current_for(&self, map: obc_formats::obcr::RouteSourceKey) -> bool {
        self.complete_surface() && self.attribution_map == Some(map)
    }
    pub fn rough_m(&self) -> u32 {
        self.surface_m[3..].iter().sum()
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GradeSample {
    pub start_m: u32,
    pub end_m: u32,
    /// Actual endpoint rise divided by physical segment distance, never a min/max envelope.
    pub grade_percent: Option<f32>,
}

impl RouteReader<'_> {
    /// Stream bounded chunk scratch once. Integer-prefix clipping makes adjacent intervals conserve
    /// distance, surface and booked ascent exactly. Missing endpoints invalidate the entire segment.
    pub fn interval_facts(&self, start_m: u32, end_m: u32) -> Result<IntervalFacts, Error> {
        self.interval_facts_with_grades(start_m, end_m, |_| {})
    }

    #[inline(never)]
    pub fn interval_facts_with_grades(
        &self,
        start_m: u32,
        end_m: u32,
        mut grade: impl FnMut(GradeSample),
    ) -> Result<IntervalFacts, Error> {
        let start_m = start_m.min(self.total_distance_m);
        let end_m = end_m.min(self.total_distance_m);
        if start_m > end_m {
            return Err(Error::BadOffset);
        }
        let mut accumulator = FactsAccumulator::new(self.identity(), self.attribution_map()?, start_m, end_m);
        let mut buf = Vec::<RoutePoint, MAX_POINTS_PER_CHUNK>::new();
        for k in 0..self.chunks().len() {
            self.decode_chunk(k, &mut buf)?;
            for &p in buf.iter().skip(usize::from(k > 0)) {
                accumulator.push(p, &mut grade);
            }
        }
        accumulator.finish(self.total_distance_m)
    }

    /// Facts from `start_m` to each of `ends`, in one walk that stops after the farthest end.
    /// A stopped walk cannot check the stored total, so a caller checks it with
    /// [`interval_facts`](Self::interval_facts) first.
    #[inline(never)]
    pub fn interval_facts_to<const N: usize>(
        &self,
        start_m: u32,
        ends: &[u32],
    ) -> Result<Vec<IntervalFacts, N>, Error> {
        let start_m = start_m.min(self.total_distance_m);
        let map = self.attribution_map()?;
        let mut accumulators = Vec::<FactsAccumulator, N>::new();
        for &end in ends {
            let end_m = end.min(self.total_distance_m);
            if start_m > end_m {
                return Err(Error::BadOffset);
            }
            let accumulator = FactsAccumulator::new(self.identity(), map, start_m, end_m);
            accumulators.push(accumulator).map_err(|_| Error::BadOffset)?;
        }
        let farthest = ends.iter().max().map_or(0, |&end| end.min(self.total_distance_m));
        let mut buf = Vec::<RoutePoint, MAX_POINTS_PER_CHUNK>::new();
        for k in 0..self.chunks().len() {
            let Some(walked) = accumulators.first().map(|a| a.distance as u32) else { break };
            if walked >= farthest {
                break;
            }
            self.decode_chunk(k, &mut buf)?;
            for &p in buf.iter().skip(usize::from(k > 0)) {
                for accumulator in &mut accumulators {
                    accumulator.push(p, &mut |_| {});
                }
            }
        }
        Ok(accumulators.into_iter().map(|a| a.facts).collect())
    }
}

pub(crate) struct FactsAccumulator {
    facts: IntervalFacts,
    previous: Option<RoutePoint>,
    distance: f64,
    band: DeadBand<f64>,
}
impl FactsAccumulator {
    pub(crate) fn new(identity: u32, map: Option<obc_formats::obcr::RouteSourceKey>, start_m: u32, end_m: u32) -> Self {
        Self {
            facts: IntervalFacts {
                route_identity: identity,
                policy: FACTS_POLICY,
                attribution_map: map,
                start_m,
                end_m,
                ascent_m: 0,
                descent_m: 0,
                elevation_known_m: 0,
                surface_m: [0; 8],
            },
            previous: None,
            distance: 0.0,
            band: DeadBand::new(),
        }
    }
    pub(crate) fn push(&mut self, p: RoutePoint, grade: &mut impl FnMut(GradeSample)) {
        let before_ascent = self.band.ascent() as u32;
        let before_descent = self.band.descent() as u32;
        let next_distance =
            self.distance + self.previous.map_or(0.0, |a| ground_dist_m((a.lon, a.lat), (p.lon, p.lat)) as f64);
        if p.elevation_incomplete {
            self.band.pause();
        }
        if let Some(e) = p.elevation() {
            if self.previous.is_none() || next_distance as u32 > self.distance as u32 {
                self.band.push(e as f64);
            }
        } else {
            self.band.pause();
        }
        if let Some(a) = self.previous {
            let seg_start = self.distance as u32;
            let length = ground_dist_m((a.lon, a.lat), (p.lon, p.lat));
            self.distance += length as f64;
            let seg_end = self.distance as u32;
            let lo = self.facts.start_m.max(seg_start);
            let hi = self.facts.end_m.min(seg_end);
            if hi > lo {
                let known = !p.elevation_incomplete && a.elevation().is_some() && p.elevation().is_some();
                self.facts.surface_m[p.surface as usize] += hi - lo;
                if known {
                    self.facts.elevation_known_m += hi - lo;
                }
                let clip = |n: u32| -> u32 {
                    let prefix = |at: u32| u64::from(n) * u64::from(at - seg_start) / u64::from(seg_end - seg_start);
                    (prefix(hi) - prefix(lo)) as u32
                };
                self.facts.ascent_m += clip(self.band.ascent() as u32 - before_ascent);
                self.facts.descent_m += clip(self.band.descent() as u32 - before_descent);
                grade(GradeSample {
                    start_m: lo,
                    end_m: hi,
                    grade_percent: known.then(|| (p.ele as f32 - a.ele as f32) * 100.0 / length),
                });
            }
        }
        self.previous = Some(p);
    }
    pub(crate) fn finish(self, total_distance_m: u32) -> Result<IntervalFacts, Error> {
        if self.distance as u32 != total_distance_m {
            return Err(Error::BadOffset);
        }
        Ok(self.facts)
    }
}
