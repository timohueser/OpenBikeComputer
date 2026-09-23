//! One frozen interval on the complete accepted journey's distance axis.

use crate::{ClimbSeg, Climbs, IntervalFacts, RouteReader, Waypoint, WaypointCursor};
use obc_formats::io::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AheadRange {
    FiveKm,
    TenKm,
}

impl AheadRange {
    pub const fn meters(self) -> u32 {
        match self {
            Self::FiveKm => 5_000,
            Self::TenKm => 10_000,
        }
    }
}

/// The parse identity changes when the immutable source is reopened or replaced. Progress does
/// not change this key: membership stays frozen until the rider refreshes the window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RouteWindow {
    pub identity: u32,
    pub start_m: u32,
    pub end_m: u32,
}

impl RouteWindow {
    pub fn new(route: &RouteReader, anchor_m: u32, range: AheadRange) -> Self {
        let start_m = anchor_m.min(route.total_distance_m);
        Self {
            identity: route.identity(),
            start_m,
            end_m: start_m.saturating_add(range.meters()).min(route.total_distance_m),
        }
    }

    pub fn contains(self, distance_m: u32) -> bool {
        (self.start_m..=self.end_m).contains(&distance_m)
    }

    pub fn matches(self, route: &RouteReader) -> bool {
        self.identity == route.identity()
    }

    pub fn facts(self, route: &RouteReader) -> Result<IntervalFacts, Error> {
        if !self.matches(route) {
            return Err(Error::BadOffset);
        }
        route.interval_facts(self.start_m, self.end_m)
    }

    /// Return the complete climb, including its part outside this window.
    pub fn climb(self, climbs: &Climbs) -> Option<&ClimbSeg> {
        climbs.ahead(self.start_m, self.end_m)
    }

    /// The next authored marker can lie beyond the displayed window. Generic and categorized
    /// markers have the same provenance; unnamed records remain legitimate authored markers.
    pub fn next_waypoint(self, route: &RouteReader) -> Result<Option<Waypoint>, Error> {
        if !self.matches(route) {
            return Err(Error::BadOffset);
        }
        let mut cursor = WaypointCursor::new(route.source())?;
        while let Some(waypoint) = cursor.next(route.source())? {
            if waypoint.dist_along_m >= self.start_m {
                return Ok(Some(waypoint));
            }
        }
        Ok(None)
    }
}
