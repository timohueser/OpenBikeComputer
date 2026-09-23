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

pub use obc_formats::trip_progress::{RouteVersion, TripProgress};

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
    /// Bit `k` is set when day `k` resolves, so [`days`](TripSummary::days) can pair each resolved
    /// index with its day.
    resolved: u32,
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

        let mut trip = TripSummary {
            id: input.id,
            key: input.key,
            name,
            start_date: input.start_date,
            stage_ids: input.stage_ids.iter().take(MAX_TRIP_DAYS).copied().collect(),
            stage_indices: Vec::new(),
            resolved: 0,
            distance_km: 0,
            climb_m: 0,
        };
        trip.reresolve(catalog, catalog_ids);
        trip
    }

    /// Re-resolve this trip's [`stage_indices`](TripSummary::stage_indices) and stats from
    /// [`stage_ids`](TripSummary::stage_ids). A route rescan calls it, so a route that appeared or
    /// vanished re-files without the host re-feeding the trips.
    pub fn reresolve(&mut self, catalog: &[RouteSummary], catalog_ids: &[CatalogObjectId]) {
        self.stage_indices.clear();
        self.resolved = 0;
        self.distance_km = 0;
        self.climb_m = 0;
        for (day, &sid) in self.stage_ids.iter().enumerate() {
            if let Some(idx) = catalog_ids.iter().position(|&x| x == sid) {
                let _ = self.stage_indices.push(idx as u16);
                self.resolved |= 1 << day;
                if let Some(r) = catalog.get(idx) {
                    self.distance_km = self.distance_km.saturating_add(r.distance_km);
                    self.climb_m = self.climb_m.saturating_add(r.climb_m);
                }
            }
        }
    }
}

impl TripSummary {
    /// The length of the routes of the days after day `day`: `catalog[i]` is the route at catalog
    /// index `i`. Transfers between days are not ridden, so they do not count. The catalog holds
    /// whole kilometres, so each later day adds up to 500 m of error.
    pub fn later_m(&self, day: u16, catalog: &[RouteSummary]) -> u32 {
        self.days()
            .filter(|&(k, _)| k > day)
            .filter_map(|(_, i)| catalog.get(usize::from(i)))
            .fold(0, |m, r| m.saturating_add(r.distance_km.saturating_mul(1000)))
    }
}

/// How a trip day loads: [`TripSummary::load_day`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DayLoad {
    AsIs,
    /// The rest of the day before, `[from_m, to_m]` on its route, and then the day from `join_m`.
    Rest {
        from_m: u32,
        to_m: u32,
        join_m: u32,
    },
}

impl DayLoad {
    /// The loaded route's length, in km, for a day whose own route is `day_km` long.
    pub fn distance_km(self, day_km: u32) -> u32 {
        match self {
            DayLoad::AsIs => day_km,
            DayLoad::Rest { from_m, to_m, join_m } => {
                ((to_m - from_m) + (day_km * 1000).saturating_sub(join_m) + 500) / 1000
            }
        }
    }
}

/// A rest of the day before this short counts as ridden, so the next day loads as it is. It covers
/// a Finish a few metres before the day's end.
pub const REST_MIN_M: u32 = 500;

/// A day that starts more than this, straight line, from the end of the day before follows a
/// transfer (spec §7.7). The phone uses the same value.
pub const TRANSFER_MIN_M: u32 = 200;

/// Where the active trip's next day meets the day before on the trip's line. The host reads it from
/// the trip object and the day before's route after each catalog read; without it the next day
/// loads as it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DayJoin {
    pub key: u64,
    /// The next day, or the second day while the first is next.
    pub day: u16,
    /// Where the day before leaves the line, clamped to the length of its route.
    pub leave_m: u32,
    /// Where the next day joins the line.
    pub join_m: u32,
    /// Straight-line metres from the last point of the day before to the first point of the day:
    /// [`gap_m`].
    pub gap_m: u32,
    /// The same facts one day on. A Finish moves the next day on, and these load that day before
    /// the catalog read that follows the Finish.
    pub after: Option<Join>,
}

/// Where a day meets the day before on the trip's line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Join {
    pub leave_m: u32,
    pub join_m: u32,
    pub gap_m: u32,
}

impl DayJoin {
    /// Where `day` of trip `key` meets the day before.
    fn for_day(&self, key: u64, day: u16) -> Option<Join> {
        if key != self.key {
            return None;
        }
        match day.checked_sub(self.day) {
            Some(0) => Some(Join { leave_m: self.leave_m, join_m: self.join_m, gap_m: self.gap_m }),
            Some(1) => self.after,
            _ => None,
        }
    }
}

/// The straight-line gap from `end` to `start`, `(lon, lat)` µdeg, rounded up so a gap a fraction
/// past [`TRANSFER_MIN_M`] is a transfer, as the phone reads it.
pub fn gap_m(end: (i32, i32), start: (i32, i32)) -> u32 {
    let d = obc_map_scene::ground_dist_m(end, start);
    let m = d as u32;
    m + u32::from((m as f32) < d)
}

/// A position on a trip: a day, that day's route, and metres into it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TripPosition {
    pub day: u16,
    pub route: CatalogObjectId,
    pub metres: u32,
}

/// The active trip's next day: `(trip, day, catalog index)`. The active trip is the trip of the
/// latest progress record, while it has a day left whose route the store holds.
pub fn next_trip_day<'t>(trips: &'t [TripSummary], progress: &[TripProgress]) -> Option<(&'t TripSummary, u16, u16)> {
    let (trip, record) = progress.iter().rev().find_map(|p| Some((trips.iter().find(|t| t.key == p.key)?, p)))?;
    let day = trip.next_day(Some(record))?;
    let (_, index) = trip.days().find(|&(k, _)| k == day)?;
    Some((trip, day, index))
}

/// The trip day whose route is `route`. A route is in at most one trip.
pub fn trip_day(trips: &[TripSummary], route: CatalogObjectId) -> Option<TripRef> {
    trips.iter().find_map(|trip| {
        let day = trip.stage_ids.iter().position(|&id| id == route)?;
        TripRef::new(trip.key, day as u8, trip.stage_ids.len() as u8)
    })
}

const _: () = assert!(MAX_TRIP_DAYS <= u32::BITS as usize, "TripSummary::resolved is a u32 day mask");

impl TripSummary {
    /// The resolved days in ride order: `(day, catalog index)`. A dangling day is skipped, and the
    /// days after it keep their own numbers.
    pub fn days(&self) -> impl Iterator<Item = (u16, u16)> + '_ {
        (0..self.stage_ids.len() as u16)
            .filter(|&k| self.resolved & (1 << k) != 0)
            .zip(self.stage_indices.iter().copied())
    }

    /// This trip's record among the device's progress records.
    pub fn progress_in<'p>(&self, records: &'p [TripProgress]) -> Option<&'p TripProgress> {
        records.iter().find(|p| p.key == self.key)
    }

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

    /// Metres into the position's day. They count only while the trip names the same route for that
    /// day; the store already read them as 0 when that route has another revision now.
    pub fn position_m(&self, progress: Option<&TripProgress>) -> u32 {
        self.own(progress)
            .filter(|p| self.stage_ids.get(usize::from(p.day)) == Some(&p.day_route.id))
            .map_or(0, |p| p.metres)
    }

    /// The record a Finish of a ride on day `day` writes: the position moves to `at`, and the ride's
    /// day is finished on `today` (days since 1970-01-01; 0 without a trusted clock). A ride that
    /// ends in the rest of the day before, before it joins its day, finishes the day before.
    pub fn finish(&self, old: Option<&TripProgress>, day: u16, at: TripPosition, today: u16) -> TripProgress {
        let ridden = day.min(at.day);
        let mut dates = self.own(old).map_or([0; MAX_TRIP_DAYS], |p| p.dates);
        if let Some(date) = dates.get_mut(usize::from(ridden)).filter(|_| today != 0) {
            *date = today;
        }
        TripProgress {
            key: self.key,
            day: at.day,
            // Revision 0: the store stamps the revision it holds when it writes the record.
            day_route: RouteVersion { id: at.route, revision: 0 },
            metres: at.metres,
            last_finished: Some(ridden),
            dates,
        }
    }

    /// The record a ride that starts on this trip writes. It is the trip's record as it is, so only
    /// its place changes: the latest record names the active trip. A trip without a record gets
    /// one without progress.
    pub fn start(&self, old: Option<&TripProgress>) -> TripProgress {
        self.own(old).cloned().unwrap_or_else(|| TripProgress {
            key: self.key,
            day: 0,
            day_route: RouteVersion { id: self.stage_ids.first().copied().unwrap_or(0), revision: 0 },
            metres: 0,
            last_finished: None,
            dates: [0; MAX_TRIP_DAYS],
        })
    }

    /// How day `day` loads. When the position is on the day before, more than [`REST_MIN_M`] before
    /// it leaves the line, and no transfer lies between the two days, the day is the rest of that
    /// day and then this day. Otherwise it is this day's route as it is.
    pub fn load_day(&self, day: u16, progress: Option<&TripProgress>, join: Option<&DayJoin>) -> DayLoad {
        let from_m = self.position_m(progress);
        let join = join.and_then(|j| j.for_day(self.key, day));
        match (self.own(progress).and_then(|p| self.in_trip(p.day)), join) {
            (Some(at), Some(join))
                if at + 1 == day
                    && from_m > 0
                    && from_m + REST_MIN_M < join.leave_m
                    && join.gap_m <= TRANSFER_MIN_M =>
            {
                DayLoad::Rest { from_m, to_m: join.leave_m, join_m: join.join_m }
            }
            _ => DayLoad::AsIs,
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
            day_route: RouteVersion { id: [10, 20, 30][usize::from(day)], revision: 0 },
            metres: 54_000,
            last_finished,
            dates: all,
        }
    }

    #[test]
    fn days_keep_their_numbers_past_a_dangling_day() {
        let input = TripInput { id: 1, key: KEY, name: "Alps", start_date: 0, stage_ids: &[10, 20, 30] };
        let t = TripSummary::resolve(&input, &[], &[30, 10]);
        assert_eq!(t.days().collect::<std::vec::Vec<_>>(), [(0, 1), (2, 0)]);
    }

    #[test]
    fn the_active_trip_is_the_latest_record_while_it_has_a_day_left() {
        let catalog_ids = [10, 20, 30, 40];
        let alps = TripSummary::resolve(
            &TripInput { id: 1, key: KEY, name: "Alps", start_date: 0, stage_ids: &[10, 20, 30] },
            &[],
            &catalog_ids,
        );
        let jura = TripSummary::resolve(
            &TripInput { id: 2, key: 7, name: "Jura", start_date: 0, stage_ids: &[40] },
            &[],
            &catalog_ids,
        );
        let trips = [alps, jura];
        let alps_day2 = progress(1, Some(0), &[]);
        let jura_done = TripProgress { key: 7, ..progress(0, Some(0), &[]) };
        let next = |records: &[TripProgress]| next_trip_day(&trips, records).map(|(t, day, index)| (t.key, day, index));
        assert_eq!(next(core::slice::from_ref(&alps_day2)), Some((KEY, 1, 1)));
        assert_eq!(next(&[alps_day2.clone(), jura_done.clone()]), None, "the last ride finished its trip");
        assert_eq!(next(&[jura_done, alps_day2]), Some((KEY, 1, 1)));
        assert_eq!(next(&[]), None);
    }

    #[test]
    fn a_start_keeps_the_record_and_gives_a_trip_without_one_no_progress() {
        let t = trip(0);
        let early = progress(1, Some(0), &[MON]);
        assert_eq!(t.start(Some(&early)), early);
        let fresh = t.start(None);
        assert_eq!((fresh.key, fresh.metres, fresh.last_finished), (KEY, 0, None));
        assert_eq!(t.next_day(Some(&fresh)), Some(0));
        assert!(!t.is_ticked(0, Some(&fresh)));
    }

    #[test]
    fn later_days_count_after_the_day_and_skip_a_dangling_one() {
        let route = |distance_km| RouteSummary {
            name: Default::default(),
            distance_km,
            climb_m: 0,
            bbox: obc_map_scene::BBox { min_lon: 0, min_lat: 0, max_lon: 1, max_lat: 1 },
            start_lon: 0,
            start_lat: 0,
        };
        let catalog = [route(74), route(61), route(50)];
        let input = TripInput { id: 1, key: KEY, name: "Alps", start_date: 0, stage_ids: &[10, 20, 99, 30] };
        let t = TripSummary::resolve(&input, &catalog, &[10, 20, 30]);
        assert_eq!([0, 1, 3].map(|day| t.later_m(day, &catalog)), [111_000, 50_000, 0]);
    }

    #[test]
    fn a_day_after_an_early_stop_is_the_rest_of_the_day_before_and_the_day() {
        let t = trip(0);
        // Day 2 leaves the line at 74 km, and Day 3 joins it 3 km in from a camp.
        let join = DayJoin { key: KEY, day: 2, leave_m: 74_000, join_m: 3_000, gap_m: 0, after: None };
        assert_eq!(t.load_day(0, None, Some(&join)), DayLoad::AsIs, "no progress: the day as it is");
        // Stopped 20 km before the end of Day 2 and finished: Day 3 starts with the rest of Day 2.
        let early = progress(1, Some(1), &[]);
        let rest = t.load_day(2, Some(&early), Some(&join));
        assert_eq!(rest, DayLoad::Rest { from_m: 54_000, to_m: 74_000, join_m: 3_000 });
        assert_eq!(rest.distance_km(61), 78, "20 km of Day 2, then 58 km of Day 3");
        // Rode Day 2 to its end, or a few metres short of it: Day 3 as it is.
        let full = TripProgress { metres: 73_700, ..early.clone() };
        assert_eq!(t.load_day(2, Some(&full), Some(&join)), DayLoad::AsIs);
        // Rode 20 km into Day 3: Day 3 as it is, and the ride joins it where the rider is.
        assert_eq!(t.load_day(2, Some(&progress(2, Some(1), &[])), Some(&join)), DayLoad::AsIs);
        // A voided position, a day far ahead, or no line facts for the day: as it is.
        assert_eq!(t.load_day(2, Some(&TripProgress { metres: 0, ..early.clone() }), Some(&join)), DayLoad::AsIs);
        assert_eq!(t.load_day(0, Some(&early), Some(&join)), DayLoad::AsIs);
        assert_eq!(t.load_day(2, Some(&early), None), DayLoad::AsIs);
    }

    #[test]
    fn a_day_after_an_early_stop_before_a_transfer_loads_as_it_is() {
        let t = trip(0);
        let early = progress(1, Some(1), &[]);
        let at = |gap_m| {
            t.load_day(
                2,
                Some(&early),
                Some(&DayJoin { key: KEY, day: 2, leave_m: 74_000, join_m: 0, gap_m, after: None }),
            )
        };
        assert_eq!(at(TRANSFER_MIN_M), DayLoad::Rest { from_m: 54_000, to_m: 74_000, join_m: 0 });
        assert_eq!(at(TRANSFER_MIN_M + 1), DayLoad::AsIs, "a train from the end of Day 2 to the start of Day 3");
        // 1,801 µdeg of latitude is 200.5 m.
        let (end, start) = ((8_000_000, 46_000_000), (8_000_000, 46_001_801));
        assert!((200.4..200.6).contains(&obc_map_scene::ground_dist_m(end, start)));
        assert_eq!(at(gap_m(end, start)), DayLoad::AsIs, "200.5 m is a transfer");
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
        // A month-long trip: a Finish past the 32 stored dates stores none, and the day's date
        // follows the last stored one.
        let at = TripPosition { day: 40, route: 10, metres: 0 };
        let mut dates = [0; MAX_TRIP_DAYS];
        dates[31] = MON + 31;
        let late = trip(0).finish(Some(&TripProgress { dates, ..progress(0, None, &[]) }), 40, at, MON + 45);
        assert_eq!(late.dates, dates, "no date past the last slot");
        assert_eq!(trip(0).day_date(40, Some(&late)), Some(MON + 40));
    }

    #[test]
    fn a_changed_day_route_resets_the_position_to_the_day_start() {
        let p = progress(1, Some(0), &[]);
        assert_eq!(trip(0).position_m(Some(&p)), 54_000);
        let input = TripInput { id: 1, key: KEY, name: "Alps", start_date: 0, stage_ids: &[10, 21, 30] };
        let reuploaded = TripSummary::resolve(&input, &[], &[]);
        assert_eq!(reuploaded.position_m(Some(&p)), 0);
        assert_eq!(reuploaded.next_day(Some(&p)), Some(1), "the last finished day stays");
    }

    #[test]
    fn a_finish_moves_the_position_and_finishes_the_day_ridden() {
        let t = trip(0);
        let at = |day: u16, metres| TripPosition { day, route: [10, 20, 30][usize::from(day)], metres };
        let yesterday = progress(0, Some(0), &[MON]);
        // Early stop: 20 km short of the end of Day 2. Day 3 is next, and the rest of Day 2 leads to it.
        let early = t.finish(Some(&yesterday), 1, at(1, 54_000), MON + 1);
        assert_eq!((early.day, early.metres, early.last_finished), (1, 54_000, Some(1)));
        assert_eq!(early.dates[..2], [MON, MON + 1]);
        assert_eq!((t.next_day(Some(&early)), t.position_m(Some(&early))), (Some(2), 54_000));
        // Exact end: the position is the end of Day 2.
        let end = t.finish(Some(&yesterday), 1, at(1, 74_000), MON + 1);
        assert_eq!(t.next_day(Some(&end)), Some(2));
        // Past the end: the position moved 20 km into Day 3, which is next and starts there.
        let past = t.finish(Some(&yesterday), 1, at(2, 20_000), 0);
        assert_eq!((t.next_day(Some(&past)), t.position_m(Some(&past))), (Some(2), 20_000));
        assert_eq!(past.dates[..2], [MON, 0], "no trusted clock, no date");
        assert_eq!(t.finish(None, 0, at(0, 5), MON).dates[0], MON, "the first Finish starts the record");
        // A ride on the rest of Day 2 and Day 3 that stops 6 km into the rest: Day 2 is finished,
        // and Day 3 is still next.
        let short = t.finish(Some(&early), 2, at(1, 60_000), MON + 2);
        assert_eq!((short.day, short.metres, short.last_finished), (1, 60_000, Some(1)));
        assert_eq!(t.next_day(Some(&short)), Some(2));
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
