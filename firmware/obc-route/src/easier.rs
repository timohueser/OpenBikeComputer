//! Fixed route trials and measured eligibility. Costs describe physical stored geometry.
use crate::{nav::Objective, IntervalFacts};
use obc_formats::{io::Error, obcr::RouteSourceKey};

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
    pub fn saving(self, current: Costs, next: Costs) -> u32 {
        match self {
            Self::LessClimb => current.ascent_m.saturating_sub(next.ascent_m),
            Self::Smoother => current.rough_m.saturating_sub(next.rough_m),
            Self::Shorter => current.distance_m.saturating_sub(next.distance_m),
        }
    }
    pub fn eligible(self, current: Costs, next: Costs) -> bool {
        if !current.elevation_complete || !next.elevation_complete {
            return false;
        }
        let distance_ok =
            next.distance_m.saturating_sub(current.distance_m) <= MIN_DISTANCE_ALLOWANCE_M.max(current.distance_m / 4);
        let ascent_ok =
            next.ascent_m.saturating_sub(current.ascent_m) <= MIN_ASCENT_ALLOWANCE_M.max(current.ascent_m / 4);
        match self {
            Self::LessClimb => self.saving(current, next) >= MIN_CLIMB_SAVING_M && distance_ok,
            Self::Smoother => {
                self.saving(current, next) >= MIN_ROUGH_SAVING_M
                    && distance_ok
                    && ascent_ok
                    && current.surface_attributed
                    && next.surface_attributed
                    && next.unknown_m <= current.unknown_m
            }
            Self::Shorter => self.saving(current, next) >= MIN_DISTANCE_SAVING_M && ascent_ok,
        }
    }
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
    pub fn from_facts(f: IntervalFacts, map: RouteSourceKey) -> Self {
        Self {
            distance_m: f.distance_m(),
            ascent_m: f.ascent_m,
            rough_m: f.rough_m(),
            unknown_m: f.surface_m[0],
            elevation_complete: f.complete_elevation(),
            surface_attributed: f.attribution_map == Some(map),
        }
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

/// A deterministic winner descriptor; the owner reconstructs its exact candidate before review.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Choice {
    pub objective: Objective,
    pub costs: Costs,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_gain_and_tradeoff_boundaries_require_known_comparable_facts() {
        let current = Costs {
            distance_m: 10_000,
            ascent_m: 400,
            rough_m: 2_000,
            unknown_m: 100,
            elevation_complete: true,
            surface_attributed: true,
        };
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
}
