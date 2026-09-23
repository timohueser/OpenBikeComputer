//! Fixed route trials and measured eligibility. Costs describe physical stored geometry.
use crate::{nav::Objective, RoutePoint, RouteReader, MAX_POINTS_PER_CHUNK};
use obc_elevation::{DeadBand, ElevationSource};
use obc_formats::{
    io::{ByteSource, Error},
    obcr::RouteSourceKey,
};
use obc_map_scene::ground_dist_m;

pub type LegEndpoints = ((i32, i32), (i32, i32));

pub const MIN_CLIMB_SAVING_M: u32 = 50;
pub const MIN_ROUGH_SAVING_M: u32 = 500;
pub const MIN_DISTANCE_SAVING_M: u32 = 500;
pub const MIN_DISTANCE_ALLOWANCE_M: u32 = 2_000;
pub const MIN_ASCENT_ALLOWANCE_M: u32 = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Goal {
    LessClimb,
    Smoother,
    Shorter,
}
impl Goal {
    pub const ALL: [Self; 3] = [Self::LessClimb, Self::Smoother, Self::Shorter];
    /// The goal a trial weights. `Profile` weights none and serves every goal.
    fn of(objective: Objective) -> Option<Self> {
        match objective {
            Objective::Profile => None,
            Objective::LessClimb | Objective::LeastClimb => Some(Self::LessClimb),
            Objective::Smoother | Objective::Smoothest => Some(Self::Smoother),
            Objective::Shorter | Objective::Shortest => Some(Self::Shorter),
        }
    }
    /// Every goal needs terrain along the original. Surface also needs the original attributed.
    pub fn evaluable(self, current: Costs) -> bool {
        current.elevation_complete && (self != Self::Smoother || current.surface_attributed)
    }
    pub fn saving(self, current: Costs, next: Costs) -> u32 {
        match self {
            Self::LessClimb => current.ascent_m.saturating_sub(next.ascent_m),
            Self::Smoother => current.rough_m.saturating_sub(next.rough_m),
            Self::Shorter => current.distance_m.saturating_sub(next.distance_m),
        }
    }
    /// A climb or distance saving must also be a share of the remaining route: 10 % of the climb and
    /// 3 % of the distance, chosen by the owner. On the same road an imported track still measures a
    /// little more climb and distance than a plan, and a long route would turn that into a saving.
    fn saves_enough(self, current: Costs, next: Costs) -> bool {
        self.saving(current, next)
            >= match self {
                Self::LessClimb => MIN_CLIMB_SAVING_M.max(current.ascent_m / 10),
                Self::Smoother => MIN_ROUGH_SAVING_M,
                Self::Shorter => MIN_DISTANCE_SAVING_M.max(current.distance_m / 100 * 3),
            }
    }
    pub fn eligible(self, current: Costs, next: Costs) -> bool {
        if !self.evaluable(current) || !next.elevation_complete || !self.saves_enough(current, next) {
            return false;
        }
        let distance_ok =
            next.distance_m.saturating_sub(current.distance_m) <= MIN_DISTANCE_ALLOWANCE_M.max(current.distance_m / 4);
        let ascent_ok =
            next.ascent_m.saturating_sub(current.ascent_m) <= MIN_ASCENT_ALLOWANCE_M.max(current.ascent_m / 4);
        match self {
            Self::LessClimb => distance_ok,
            Self::Smoother => {
                distance_ok && ascent_ok && next.surface_attributed && next.unknown_m <= current.unknown_m
            }
            Self::Shorter => ascent_ok,
        }
    }
}

/// The index in [`Objective::TRIALS`] that runs after trial `done`, or `None` when the batch is
/// complete. `result` is that trial's candidate. A trial whose goal is not evaluable does not run.
/// A milder twin runs only when its strong trial saved enough, because a milder weight rarely
/// saves more. The planner accepts paths up to 1.3x the optimum, so this is approximate.
pub fn next_trial(done: usize, result: Option<Costs>, current: Costs) -> Option<usize> {
    let objective = Objective::TRIALS[done];
    let strong = matches!(objective, Objective::LeastClimb | Objective::Smoothest | Objective::Shortest);
    let mut next = done + 1;
    if let (true, Some(goal), Some(result)) = (strong, Goal::of(objective), result) {
        if !goal.saves_enough(current, result) {
            next += 1;
        }
    }
    while Objective::TRIALS.get(next).is_some_and(|&o| Goal::of(o).is_some_and(|g| !g.evaluable(current))) {
        next += 1;
    }
    (next < Objective::TRIALS.len()).then_some(next)
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Costs {
    pub distance_m: u32,
    pub ascent_m: u32,
    pub rough_m: u32,
    pub unknown_m: u32,
    pub elevation_complete: bool,
    pub surface_attributed: bool,
}
impl Costs {
    /// The rest of `route` from `start_m`, measured by [`Stations`]. Surface comes from the stored
    /// attribution. The walk starts at the rider's chunk.
    pub fn remaining(
        route: &RouteReader,
        start_m: u32,
        map: RouteSourceKey,
        elev: &mut dyn ElevationSource,
    ) -> Result<Self, Error> {
        // Two sibling frames, so only one chunk buffer is on the stack at a time.
        let facts = route.interval_facts(start_m, route.total_distance_m)?;
        let (distance_m, ascent_m, elevation_complete) = Stations::remaining(route, start_m, elev)?;
        Ok(Self {
            distance_m,
            ascent_m,
            rough_m: facts.rough_m(),
            unknown_m: facts.surface_m[0],
            elevation_complete,
            surface_attributed: facts.attribution_map == Some(map),
        })
    }
    /// A complete candidate, measured by [`Stations`] like the original. Its surface figures come
    /// from `facts`, and the caller has checked that it is attributed to the current map.
    pub fn candidate(
        src: &dyn ByteSource,
        facts: crate::visit::VisitCosts,
        elev: &mut dyn ElevationSource,
    ) -> Result<Self, Error> {
        let h = crate::reader::read_header(src)?;
        let mut stations = Stations::new(elev);
        crate::reader::for_each_stored_point(src, &h, |p| stations.push((p.lon, p.lat)))?;
        let (distance_m, ascent_m, elevation_complete) = stations.finish();
        Ok(Self {
            distance_m,
            ascent_m,
            rough_m: facts.rough_m,
            unknown_m: facts.unknown_m,
            elevation_complete,
            surface_attributed: true,
        })
    }
}

/// Along-track spacing of the measuring stations. The planner stores a candidate point about every
/// 30 m on mountain roads and the terrain postings are about 40 m apart, so a finer spacing only
/// measures GPS noise and lateral offset on a slope.
pub const STATION_SPACING_M: f64 = 40.0;

/// Measures an original and a candidate the same way: a station every [`STATION_SPACING_M`] of
/// stored geometry and one at the end. Distance is the sum of the chords between stations, and
/// climb is the dead band over terrain at the stations. Neither the stored point density nor an
/// imported route's own heights enter the result.
struct Stations<'a> {
    elev: &'a mut dyn ElevationSource,
    band: DeadBand<f64>,
    complete: bool,
    distance: f64,
    along: f64,
    next: f64,
    previous: Option<(i32, i32)>,
    station: Option<(i32, i32)>,
}
impl<'a> Stations<'a> {
    fn new(elev: &'a mut dyn ElevationSource) -> Self {
        Self {
            elev,
            band: DeadBand::new(),
            complete: true,
            distance: 0.0,
            along: 0.0,
            next: STATION_SPACING_M,
            previous: None,
            station: None,
        }
    }
    #[inline(never)]
    fn remaining(
        route: &RouteReader,
        start_m: u32,
        elev: &'a mut dyn ElevationSource,
    ) -> Result<(u32, u32, bool), Error> {
        let start = route.position_at(start_m).ok_or(Error::BadOffset)?;
        let mut stations = Self::new(elev);
        stations.push((start.lon, start.lat));
        let first = route.chunks().iter().rposition(|c| c.cum_distance_m <= start_m).unwrap_or(0);
        let mut along = f64::from(route.chunks().get(first).ok_or(Error::BadOffset)?.cum_distance_m);
        let mut last = None;
        let mut points = heapless::Vec::<RoutePoint, MAX_POINTS_PER_CHUNK>::new();
        for k in first..route.chunks().len() {
            route.decode_chunk(k, &mut points)?;
            for p in points.iter().skip(usize::from(k > first)) {
                let p = (p.lon, p.lat);
                along += last.map_or(0.0, |l| f64::from(ground_dist_m(l, p)));
                last = Some(p);
                if along > f64::from(start_m) {
                    stations.push(p);
                }
            }
        }
        Ok(stations.finish())
    }
    fn push(&mut self, p: (i32, i32)) {
        let Some(a) = self.previous else {
            self.previous = Some(p);
            return self.station(p);
        };
        let length = f64::from(ground_dist_m(a, p));
        // `next` stays ahead of `along`, so a zero-length step adds no station.
        while self.next <= self.along + length {
            let t = (self.next - self.along) / length;
            let at = |s: i32, e: i32| s + (f64::from(e - s) * t) as i32;
            self.station((at(a.0, p.0), at(a.1, p.1)));
            self.next += STATION_SPACING_M;
        }
        self.along += length;
        self.previous = Some(p);
    }
    fn station(&mut self, q: (i32, i32)) {
        self.distance += self.station.map_or(0.0, |s| f64::from(ground_dist_m(s, q)));
        self.station = Some(q);
        match self.elev.sample(q.1, q.0) {
            Some(h) => self.band.push(f64::from(h)),
            None => {
                self.complete = false;
                self.band.pause();
            }
        }
    }
    fn finish(mut self) -> (u32, u32, bool) {
        if let Some(end) = self.previous.filter(|&end| self.station != Some(end)) {
            self.station(end);
        }
        (self.distance as u32, self.band.ascent() as u32, self.complete)
    }
}

/// Original distances are bounded constraints, not a second route or annotation cache.
/// Every stored remaining record consumes capacity, including records with the same anchor.
pub(crate) struct Anchors {
    distances: [u32; crate::MAX_WAYPOINTS],
    accepted: [u32; crate::MAX_WAYPOINTS],
    count: usize,
    next: usize,
    pub(crate) target: (i32, i32),
    target_m: u32,
    pub(crate) finished: bool,
}
impl Anchors {
    pub(crate) fn read(route: &crate::RouteReader, progress: u32) -> Result<Self, Error> {
        if progress >= route.total_distance_m
            || route.has_unresolved_avoidance()
            || route.visit_descriptor()?.is_some_and(|v| progress < v.accepted_anchors_m[2])
        {
            return Err(Error::BadOffset);
        }
        let mut result = Self {
            distances: [0; crate::MAX_WAYPOINTS],
            accepted: [0; crate::MAX_WAYPOINTS],
            count: 0,
            next: 0,
            target: (0, 0),
            target_m: 0,
            finished: false,
        };
        let mut cursor = crate::reader::WaypointCursor::new(route.source())?;
        let mut previous = 0;
        while let Some(w) = cursor.next(route.source())? {
            if w.dist_along_m < previous || w.dist_along_m > route.total_distance_m {
                return Err(Error::BadOffset);
            }
            previous = w.dist_along_m;
            if w.dist_along_m >= progress {
                if result.count == result.distances.len() {
                    return Err(Error::TooLarge);
                }
                result.distances[result.count] = w.dist_along_m;
                result.count += 1;
            }
        }
        result.advance(route)?;
        Ok(result)
    }
    pub(crate) fn advance(&mut self, route: &crate::RouteReader) -> Result<(), Error> {
        self.target_m = if self.next < self.count { self.distances[self.next] } else { route.total_distance_m };
        let p = route.position_at(self.target_m).ok_or(Error::BadOffset)?;
        self.target = (p.lon, p.lat);
        Ok(())
    }
    pub(crate) fn reached(&mut self, distance: u32, total: u32) {
        while self.next < self.count && self.distances[self.next] == self.target_m {
            self.accepted[self.next] = distance;
            self.next += 1;
        }
        self.finished = self.target_m == total;
    }
    pub(crate) fn mapped(&self, original: u32) -> Result<u32, Error> {
        self.distances[..self.count]
            .iter()
            .position(|&d| d == original)
            .map(|i| self.accepted[i])
            .ok_or(Error::BadOffset)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const CURRENT: Costs = Costs {
        distance_m: 10_000,
        ascent_m: 400,
        rough_m: 2_000,
        unknown_m: 100,
        elevation_complete: true,
        surface_attributed: true,
    };
    #[test]
    fn exact_gain_and_tradeoff_boundaries_require_known_comparable_facts() {
        let current = CURRENT;
        let mut next = current;
        next.ascent_m = 350;
        next.distance_m = 12_500;
        assert!(Goal::LessClimb.eligible(current, next));
        next.distance_m += 1;
        assert!(!Goal::LessClimb.eligible(current, next));
        next.distance_m -= 1;
        next.ascent_m += 1;
        assert!(!Goal::LessClimb.eligible(current, next));
        next = Costs { rough_m: 1_500, ascent_m: 500, distance_m: 12_500, ..current };
        assert!(Goal::Smoother.eligible(current, next));
        next.unknown_m += 1;
        assert!(!Goal::Smoother.eligible(current, next));
        next.unknown_m -= 1;
        next.ascent_m += 1;
        assert!(!Goal::Smoother.eligible(current, next));
        next = Costs { distance_m: 9_500, ascent_m: 500, ..current };
        assert!(Goal::Shorter.eligible(current, next));
        next.distance_m += 1;
        assert!(!Goal::Shorter.eligible(current, next));
        for goal in Goal::ALL {
            assert!(!goal.eligible(Costs { elevation_complete: false, ..current }, next));
            assert!(!goal.eligible(current, Costs { elevation_complete: false, ..next }));
        }
        assert!(
            !Goal::Smoother.eligible(Costs { surface_attributed: false, ..current }, Costs { rough_m: 0, ..current })
        );
        assert_eq!(Objective::TRIALS.len(), 7);
    }
    #[test]
    fn a_strong_trial_that_misses_its_threshold_skips_its_milder_twin() {
        let order = |current: Costs, result: Costs| {
            let mut run = heapless::Vec::<Objective, 7>::new();
            let mut done = Some(0);
            while let Some(i) = done {
                run.push(Objective::TRIALS[i]).unwrap();
                done = next_trial(i, Some(result), current);
            }
            run
        };
        use Objective::*;
        assert_eq!(order(CURRENT, CURRENT), [Profile, LeastClimb, Smoothest, Shortest]);
        let better = Costs { ascent_m: 300, rough_m: 1_000, distance_m: 9_000, ..CURRENT };
        assert_eq!(order(CURRENT, better), Objective::TRIALS);
        let imported = Costs { surface_attributed: false, ..CURRENT };
        assert_eq!(order(imported, better), [Profile, LeastClimb, LessClimb, Shortest, Shorter]);
        assert_eq!(order(Costs { elevation_complete: false, ..CURRENT }, better), [Profile]);
        assert_eq!(next_trial(1, None, CURRENT), Some(2), "a failed strong trial still runs its twin");
    }
}
