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
        let mut facts = IntervalFacts {
            route_identity: self.identity(),
            policy: FACTS_POLICY,
            attribution_map: self.attribution_map()?,
            start_m,
            end_m,
            ascent_m: 0,
            descent_m: 0,
            elevation_known_m: 0,
            surface_m: [0; 8],
        };
        let mut buf = Vec::<RoutePoint, MAX_POINTS_PER_CHUNK>::new();
        let mut previous: Option<RoutePoint> = None;
        let mut distance = 0.0f64;
        let mut band = DeadBand::<f64>::new();
        for k in 0..self.chunks().len() {
            self.decode_chunk(k, &mut buf)?;
            for &p in buf.iter().skip(usize::from(k > 0)) {
                let before_ascent = band.ascent() as u32;
                let before_descent = band.descent() as u32;
                let next_distance =
                    distance + previous.map_or(0.0, |a| ground_dist_m((a.lon, a.lat), (p.lon, p.lat)) as f64);
                if p.elevation_incomplete {
                    band.pause();
                }
                if let Some(e) = p.elevation() {
                    if previous.is_none() || next_distance as u32 > distance as u32 {
                        band.push(e as f64);
                    }
                } else {
                    band.pause();
                }
                if let Some(a) = previous {
                    let seg_start = distance as u32;
                    let length = ground_dist_m((a.lon, a.lat), (p.lon, p.lat));
                    distance += length as f64;
                    let seg_end = distance as u32;
                    let lo = start_m.max(seg_start);
                    let hi = end_m.min(seg_end);
                    if hi > lo {
                        let known = !p.elevation_incomplete && a.elevation().is_some() && p.elevation().is_some();
                        facts.surface_m[p.surface as usize] += hi - lo;
                        if known {
                            facts.elevation_known_m += hi - lo;
                        }
                        let clip = |n: u32| -> u32 {
                            let prefix =
                                |at: u32| u64::from(n) * u64::from(at - seg_start) / u64::from(seg_end - seg_start);
                            (prefix(hi) - prefix(lo)) as u32
                        };
                        facts.ascent_m += clip(band.ascent() as u32 - before_ascent);
                        facts.descent_m += clip(band.descent() as u32 - before_descent);
                        grade(GradeSample {
                            start_m: lo,
                            end_m: hi,
                            grade_percent: known.then(|| (p.ele as f32 - a.ele as f32) * 100.0 / length),
                        });
                    }
                }
                previous = Some(p);
            }
        }
        // A mismatch is stale/corrupt metadata, never a confident complete result.
        if distance as u32 != self.total_distance_m {
            return Err(Error::BadOffset);
        }
        Ok(facts)
    }
}
