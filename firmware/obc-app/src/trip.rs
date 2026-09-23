//! Trips — the grouped-route folders shown above the loose routes in the Route menu.
//!
//! A trip is a small metadata object ([`obc_route::TripMeta`], `TP{id}.OBT` on the device) that
//! references one route object id per day, in ride order. The app resolves those ids against its
//! resident route [`Catalog`](crate::route::Catalog) into a [`TripSummary`]: the stage indices into
//! the catalog, in ride order, plus the summed distance and climb over the resolvable stages.
//!
//! A stage is a day: day `k` is `stage_ids[k]`. The day rules of `obc-ble-interface-spec.md` §7.7
//! (next day, ticks, dates) read a [`TripSummary`] and the device's [`TripProgress`] for it.
//!
//! A route a stored trip references is filed and shows only inside its folder. A dangling ref
//! resolves to nothing and drops from `stage_indices`, but a trip whose every ref dangles still
//! lists, so it can be deleted on-device.

use heapless::{String, Vec};

use obc_formats::obcr::NAME_CAP;
use obc_formats::ride::TripRef;
use obc_route::MAX_TRIP_DAYS;

use crate::route::RouteSummary;
use crate::CatalogObjectId;

/// Maximum trips the resident menu catalog holds. Each [`TripSummary`] costs a name and two small
/// stage `Vec`s, so the table is a couple of KB of static RAM.
pub const MAX_TRIPS: usize = 16;

/// The app's resident trip catalog: the folders the Route menu lists above the unfiled routes.
pub type Trips = heapless::Vec<TripSummary, MAX_TRIPS>;

/// A host-scanned trip handed to [`App::set_trips`](crate::App::set_trips): the trip's durable
/// object id, its key, name and start date, and its day route ids in ride order, as stored. The host
/// owns only the raw metadata; the app resolves the ids against the live route catalog.
#[derive(Debug, Clone, Copy)]
pub struct TripInput<'a> {
    pub id: CatalogObjectId,
    pub key: u64,
    pub name: &'a str,
    /// Days since 1970-01-01; 0 = no start date.
    pub start_date: u16,
    pub stage_ids: &'a [CatalogObjectId],
}

/// A resolved trip: its identity and name, the route object ids it references, the resolved catalog
/// indices in ride order, and the summed stats over the resolvable stages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TripSummary {
    /// The trip's durable object id (its own device counter, separate from routes/rides).
    pub id: CatalogObjectId,
    /// The phone's stable trip key. It survives a re-upload, so progress and rides key on it.
    pub key: u64,
    pub name: String<NAME_CAP>,
    /// Days since 1970-01-01; 0 = no start date.
    pub start_date: u16,
    /// The stage route ids as stored, in ride order. They are the resolution source of truth on a
    /// catalog rescan, and for a fully-dangling trip the only thing left to key a delete on.
    pub stage_ids: Vec<CatalogObjectId, MAX_TRIP_DAYS>,
    /// The resolved catalog indices, ride order — one per resolvable stage, so a dangling id makes
    /// this shorter than [`stage_ids`](TripSummary::stage_ids).
    pub stage_indices: Vec<u16, MAX_TRIP_DAYS>,
    /// Summed distance over the resolvable stages, km — the catalog's display unit.
    pub distance_km: u32,
    pub climb_m: u32,
}

impl TripSummary {
    /// Whether every stored stage dangled. An empty folder is still listed, so it can be deleted
    /// on-device.
    pub fn is_empty_folder(&self) -> bool {
        self.stage_indices.is_empty()
    }

    /// Build a resolved trip from a host [`TripInput`] against the route catalog: `catalog[i]` is
    /// the summary whose durable id is `catalog_ids[i]`. A dangling stage id is dropped from the
    /// resolved list but stays in `stage_ids`.
    pub fn resolve(input: &TripInput, catalog: &[RouteSummary], catalog_ids: &[CatalogObjectId]) -> TripSummary {
        let mut name = String::new();
        let _ = name.push_str(truncate_on_char_boundary(input.name, NAME_CAP));

        let mut stage_ids = Vec::new();
        let mut stage_indices = Vec::new();
        let mut distance_km = 0u32;
        let mut climb_m = 0u32;
        for &sid in input.stage_ids.iter().take(MAX_TRIP_DAYS) {
            let _ = stage_ids.push(sid);
            if let Some(idx) = catalog_ids.iter().position(|&x| x == sid) {
                let _ = stage_indices.push(idx as u16);
                if let Some(r) = catalog.get(idx) {
                    distance_km = distance_km.saturating_add(r.distance_km);
                    climb_m = climb_m.saturating_add(r.climb_m);
                }
            }
        }
        TripSummary {
            id: input.id,
            key: input.key,
            name,
            start_date: input.start_date,
            stage_ids,
            stage_indices,
            distance_km,
            climb_m,
        }
    }

    /// Re-resolve this trip's [`stage_indices`](TripSummary::stage_indices) and stats from
    /// [`stage_ids`](TripSummary::stage_ids). A route rescan calls it, so a route that appeared or
    /// vanished re-files without the host re-feeding the trips.
    pub fn reresolve(&mut self, catalog: &[RouteSummary], catalog_ids: &[CatalogObjectId]) {
        self.stage_indices.clear();
        self.distance_km = 0;
        self.climb_m = 0;
        for &sid in self.stage_ids.iter() {
            if let Some(idx) = catalog_ids.iter().position(|&x| x == sid) {
                let _ = self.stage_indices.push(idx as u16);
                if let Some(r) = catalog.get(idx) {
                    self.distance_km = self.distance_km.saturating_add(r.distance_km);
                    self.climb_m = self.climb_m.saturating_add(r.climb_m);
                }
            }
        }
    }
}

/// The trip day whose route is `route`. A route is in at most one trip.
pub fn trip_day(trips: &[TripSummary], route: CatalogObjectId) -> Option<TripRef> {
    trips.iter().find_map(|trip| {
        let day = trip.stage_ids.iter().position(|&id| id == route)?;
        TripRef::new(trip.key, day as u8, trip.stage_ids.len() as u8)
    })
}

/// A route object as the store holds it. A replace keeps the id and bumps the revision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RouteVersion {
    pub id: CatalogObjectId,
    pub revision: u64,
}

/// The device's own progress through one trip: the device writes it at Finish and the phone never
/// sees it. It is keyed on the trip key, so it survives a re-upload of the same trip.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TripProgress {
    pub key: u64,
    /// The day that contains the position.
    pub day: u16,
    /// That day's route when the record was written.
    pub day_route: RouteVersion,
    /// Metres into that day's route.
    pub metres: u32,
    /// The last finished day; `None` before the first Finish.
    pub last_finished: Option<u16>,
    /// The date each day was finished, in days since 1970-01-01; 0 = none. A finish without a
    /// trusted clock records no date.
    pub dates: [u16; MAX_TRIP_DAYS],
}

impl TripSummary {
    fn own<'p>(&self, progress: Option<&'p TripProgress>) -> Option<&'p TripProgress> {
        progress.filter(|p| p.key == self.key)
    }

    /// A record day that is still a day of this trip. A re-upload with fewer days drops the rest,
    /// so old progress never reads as a finished trip.
    fn in_trip(&self, day: u16) -> Option<u16> {
        (usize::from(day) < self.stage_ids.len()).then_some(day)
    }

    /// The day to ride next: `max(last finished + 1, day of the position)`. `None` when no day is
    /// left.
    pub fn next_day(&self, progress: Option<&TripProgress>) -> Option<u16> {
        let next = self.own(progress).map_or(0, |p| {
            let after_finish = p.last_finished.and_then(|d| self.in_trip(d)).map_or(0, |d| d + 1);
            after_finish.max(self.in_trip(p.day).unwrap_or(0))
        });
        self.in_trip(next)
    }

    /// Metres into the position's day. They count only while the trip names the same route, at
    /// the same revision, for that day; `revision_of` gives the store's current revision of a
    /// route. Otherwise the position is the day start.
    pub fn position_m(
        &self,
        progress: Option<&TripProgress>,
        revision_of: impl Fn(CatalogObjectId) -> Option<u64>,
    ) -> u32 {
        match self.own(progress) {
            Some(p)
                if self.stage_ids.get(usize::from(p.day)) == Some(&p.day_route.id)
                    && revision_of(p.day_route.id) == Some(p.day_route.revision) =>
            {
                p.metres
            }
            _ => 0,
        }
    }

    /// Whether day `k` is ticked: it is finished, or its end is behind the position.
    pub fn is_ticked(&self, k: u16, progress: Option<&TripProgress>) -> bool {
        self.own(progress).is_some_and(|p| {
            self.in_trip(p.day).is_some_and(|d| k < d)
                || p.last_finished.and_then(|d| self.in_trip(d)).is_some_and(|d| k <= d)
        })
    }

    /// The date of day `k`, in days since 1970-01-01. Dates follow the rides: the latest dated day
    /// `j ≤ k` gives `date(j) + (k − j)`. Before any dated ride, it is `start date + k`. `None`
    /// when neither exists.
    pub fn day_date(&self, k: u16, progress: Option<&TripProgress>) -> Option<u16> {
        let ridden = self.own(progress).and_then(|p| {
            let last = usize::from(k).min(MAX_TRIP_DAYS - 1);
            (0..=last).rev().find(|&j| p.dates[j] != 0).map(|j| p.dates[j].saturating_add(k - j as u16))
        });
        ridden.or((self.start_date != 0).then(|| self.start_date.saturating_add(k)))
    }
}

/// The longest prefix of `s` that fits in `cap` bytes without splitting a multi-byte char.
fn truncate_on_char_boundary(s: &str, cap: usize) -> &str {
    let mut end = s.len().min(cap);
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: u64 = 0xA1;
    /// 2025-09-29, a Monday.
    const MON: u16 = 20_360;

    /// Three days on routes 10, 20 and 30.
    fn trip(start_date: u16) -> TripSummary {
        let input = TripInput { id: 1, key: KEY, name: "Alps", start_date, stage_ids: &[10, 20, 30] };
        TripSummary::resolve(&input, &[], &[])
    }

    fn progress(day: u16, last_finished: Option<u16>, dates: &[u16]) -> TripProgress {
        let mut all = [0; MAX_TRIP_DAYS];
        all[..dates.len()].copy_from_slice(dates);
        TripProgress {
            key: KEY,
            day,
            day_route: RouteVersion { id: [10, 20, 30][usize::from(day)], revision: 1 },
            metres: 54_000,
            last_finished,
            dates: all,
        }
    }

    #[test]
    fn next_day_is_the_later_of_the_day_after_the_last_finish_and_the_position() {
        let t = trip(0);
        assert_eq!(t.next_day(None), Some(0));
        // Stopped short of the end of Day 2 and finished: Day 3 is next.
        assert_eq!(t.next_day(Some(&progress(1, Some(1), &[]))), Some(2));
        // Rode on into Day 3 before the finish of Day 2: still Day 3.
        assert_eq!(t.next_day(Some(&progress(2, Some(1), &[]))), Some(2));
        // The position is in Day 2, and nothing is finished yet.
        assert_eq!(t.next_day(Some(&progress(1, None, &[]))), Some(1));
        assert_eq!(t.next_day(Some(&progress(2, Some(2), &[]))), None, "the trip is done");
        let other = TripProgress { key: KEY + 1, ..progress(2, Some(1), &[]) };
        assert_eq!(t.next_day(Some(&other)), Some(0), "another trip's progress does not count");
    }

    #[test]
    fn a_day_is_ticked_when_finished_or_when_its_end_is_behind_the_position() {
        let t = trip(0);
        assert!(!t.is_ticked(0, None));
        // Rode into Day 3 without a finish: Days 1 and 2 are behind the position.
        let p = progress(2, None, &[]);
        assert_eq!([0, 1, 2].map(|k| t.is_ticked(k, Some(&p))), [true, true, false]);
        // Stopped inside Day 2 and finished: Day 2 is ticked, though its end is ahead.
        let p = progress(1, Some(1), &[]);
        assert_eq!([0, 1, 2].map(|k| t.is_ticked(k, Some(&p))), [true, true, false]);
    }

    #[test]
    fn dates_follow_the_rides() {
        assert_eq!(trip(0).day_date(1, None), None, "no start date and no ride: no weekday");
        let t = trip(MON);
        assert_eq!([0, 1, 2].map(|k| t.day_date(k, None)), [Some(MON), Some(MON + 1), Some(MON + 2)]);
        // Day 2 was ridden on Wednesday, not Tuesday, so Day 3 is Thursday.
        let p = progress(1, Some(1), &[MON, MON + 2]);
        assert_eq!([0, 1, 2].map(|k| t.day_date(k, Some(&p))), [Some(MON), Some(MON + 2), Some(MON + 3)]);
        // A finish without a trusted clock carries the last dated day forward.
        let p = progress(2, Some(1), &[MON + 1]);
        assert_eq!(trip(0).day_date(2, Some(&p)), Some(MON + 3));
    }

    #[test]
    fn a_changed_day_route_resets_the_position_to_the_day_start() {
        let p = progress(1, Some(0), &[]);
        assert_eq!(trip(0).position_m(Some(&p), |_| Some(1)), 54_000);
        // The same route id replaced in place: a new revision, other geometry.
        assert_eq!(trip(0).position_m(Some(&p), |_| Some(2)), 0);
        let input = TripInput { id: 1, key: KEY, name: "Alps", start_date: 0, stage_ids: &[10, 21, 30] };
        let reuploaded = TripSummary::resolve(&input, &[], &[]);
        assert_eq!(reuploaded.position_m(Some(&p), |_| Some(1)), 0);
        assert_eq!(reuploaded.next_day(Some(&p)), Some(1), "the last finished day stays");
    }

    #[test]
    fn progress_past_a_shorter_reupload_is_dropped_not_done() {
        // Recorded on a five-day version: in Day 5, Day 4 finished. The trip now has three days.
        let t = trip(0);
        let p = TripProgress { day: 4, last_finished: Some(3), ..progress(0, None, &[]) };
        assert_eq!(t.next_day(Some(&p)), Some(0));
        assert!(!(0..3).any(|k| t.is_ticked(k, Some(&p))));
        let p = TripProgress { day: 1, last_finished: Some(3), ..progress(1, None, &[]) };
        assert_eq!(t.next_day(Some(&p)), Some(1), "the position in Day 2 still counts");
    }
}
