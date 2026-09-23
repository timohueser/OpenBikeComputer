//! An App-owned, bounded page of route-corridor places from the shared query engine.
//! Entry, filter and route changes define a new generation. Progress and clock ticks do not
//! rerank that generation; only current opening status changes. Errors settle as failures.

use obc_reader::reader::places::{PlaceKey, PlaceQuery, PlaceWindow, QueryProgress, PLACE_PAGE_SIZE};
use obc_reader::{CorridorPoi, PoiCategorySet, Reader, RoutePath, CORRIDOR_HALF_WIDTH_M};
use obc_route::RouteReader;

/// What a corridor snapshot is *for*: the category filter and the along-route progress it was
/// anchored at. Two snapshots with the same key are the same list, so re-arming with an unchanged
/// key is a no-op — which is what keeps the query off the per-frame path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CorridorKey {
    /// The categories the list shows ("Everything" is [`PoiCategorySet::ALL`]).
    pub filter: PoiCategorySet,
    pub hours_filter: obc_reader::reader::places::HoursFilter,
    /// Live route progress (m) at the moment the screen armed the request. Distances in the
    /// snapshot are relative to this, not to progress as it advances.
    pub anchor_m: u32,
}

/// What the Up-ahead timeline is currently scoped to: the rider's live category filter (app state,
/// reset on entry) and their persisted source preference (a settings row). Together they decide
/// which tables the list may walk and whether a corridor snapshot is wanted at all, so they travel
/// as one value that cannot be passed apart and cannot drift.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UpAheadScope {
    pub filter: PoiCategorySet,
    /// Which of the two source tables may feed the list — and, under
    /// [`WaypointsOnly`](crate::settings::UpAheadSource::WaypointsOnly), whether any snapshot is
    /// armed at all.
    pub source: crate::settings::UpAheadSource,
}

/// The [`App`](crate::App)-owned corridor snapshot. One buffer, shared by whatever screen is
/// showing the Up-ahead list, and never owned by a [`Screen`](crate::screen::Screen) variant.
pub struct CorridorScratch {
    query: Option<PlaceQuery>,
    generation: u32,
    status: QueryProgress,
    clock_key: Option<(bool, i16)>,
    local: Option<(u8, u16)>,
    recheck: bool,
    /// The page boundary the next query starts past, and whether it pages backwards.
    start: Option<(PlaceKey, bool)>,
    /// The key a snapshot is *wanted* for — `None` when nothing is asking (the normal state: no
    /// Up-ahead screen is up, so the query never runs and the host never builds a `Reader` for it).
    want: Option<CorridorKey>,
    /// The key the held snapshot was taken for; `Some` even when the result is empty, so "queried,
    /// nothing ahead" is distinguishable from "not queried yet". `None` on a fresh/invalidated
    /// scratch.
    taken_for: Option<CorridorKey>,
    /// The corridor POIs for [`taken_for`](CorridorScratch::taken_for), ascending by along-route
    /// distance. Frozen once filled; the query owns the ordering.
    pois: heapless::Vec<CorridorPoi, PLACE_PAGE_SIZE>,
}

impl CorridorScratch {
    pub const fn new() -> Self {
        CorridorScratch {
            want: None,
            taken_for: None,
            pois: heapless::Vec::new(),
            query: None,
            generation: 0,
            status: QueryProgress::Unavailable,
            clock_key: None,
            local: None,
            recheck: false,
            start: None,
        }
    }

    /// Ask for a snapshot of `key`. Idempotent: re-arming the key already held changes nothing (so a
    /// screen may call this every frame without re-querying), while a different key drops the
    /// stale rows immediately so no screen can draw a list that no longer matches its filter.
    pub fn arm(&mut self, key: CorridorKey) {
        if self.want != Some(key) {
            self.invalidate();
        }
        self.want = Some(key);
    }

    /// Drop the held snapshot so the next `prepare` re-runs the query for the armed key — the
    /// "re-enter to refresh" half of the contract. Also used when active route geometry changes.
    pub fn invalidate(&mut self) {
        self.taken_for = None;
        self.recheck = false;
        self.start = None;
        self.pois.clear();
        self.query = None;
        self.generation = self.generation.wrapping_add(1);
        self.status = QueryProgress::Unavailable;
    }

    /// Start the next query past `boundary`, so a later page does not first walk the earlier ones.
    pub(crate) fn start_after(&mut self, boundary: PlaceKey, backwards: bool) {
        self.start = Some((boundary, backwards));
    }

    pub(crate) fn cancel(&mut self) {
        if let Some(query) = &mut self.query {
            query.cancel();
        }
        self.status = QueryProgress::Unavailable;
        self.taken_for = self.want;
        self.pois.clear();
        self.recheck = false;
    }

    pub fn next_page(&mut self, key: PlaceKey) {
        if let Some(query) = &mut self.query {
            query.next_page(key);
            self.pois.clear();
            self.taken_for = None;
            self.status = QueryProgress::Pending;
        }
    }

    /// Stop wanting a snapshot at all (the screen closed): drops the rows *and* the request, so the
    /// reader seam goes quiet.
    pub fn disarm(&mut self) {
        self.want = None;
        self.invalidate();
    }

    #[inline]
    pub fn armed(&self) -> Option<CorridorKey> {
        self.want
    }

    /// Whether a snapshot for `key` is held (possibly empty).
    #[inline]
    pub fn holds(&self, key: CorridorKey) -> bool {
        self.taken_for == Some(key)
    }

    /// Whether a query is armed but not yet satisfied — the fact the host reader-build seam reads.
    #[inline]
    pub fn pending(&self) -> bool {
        match self.want {
            Some(key) => !self.holds(key) || self.recheck,
            None => false,
        }
    }

    /// The frozen snapshot, ascending by along-route distance. Empty before the first successful
    /// take (and for a genuinely empty corridor — [`holds`](Self::holds) tells the two apart).
    #[inline]
    pub fn entries(&self) -> &[CorridorPoi] {
        &self.pois
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.pois.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.pois.is_empty()
    }

    /// The explicit result state; failure is never an empty successful query.
    pub fn status(&self) -> QueryProgress {
        self.status
    }

    /// Opening hours change only on quarter hours, so a new minute inside one changes nothing.
    pub(crate) fn clock_changed(&mut self, local: Option<(u8, u16)>, offset: i16) -> bool {
        let authority = (local.is_some(), offset);
        let changed = quarter(self.local) != quarter(local) || self.clock_key.is_some_and(|key| key != authority);
        if self.query.is_some() && self.clock_key.is_some_and(|key| key != authority) {
            self.cancel();
        } else if self.query.is_some() {
            self.recheck |= changed;
        }
        self.clock_key = Some(authority);
        self.local = local;
        changed
    }

    pub(crate) fn prepare(&mut self, reader: Option<&Reader>, route: Option<&RouteReader>, local: Option<(u8, u16)>) {
        self.prepare_to(reader, route, local, u32::MAX);
    }

    pub(crate) fn prepare_to(
        &mut self,
        reader: Option<&Reader>,
        route: Option<&RouteReader>,
        local: Option<(u8, u16)>,
        to_m: u32,
    ) {
        let Some(key) = self.want else { return };
        if self.holds(key) && !self.recheck {
            return;
        }
        let (Some(reader), Some(route)) = (reader, route) else { return };
        if self.recheck {
            if let Err(error) = reader.refresh_place_hours(&mut self.pois, local) {
                self.status = QueryProgress::Failed(error);
                self.pois.clear();
            }
            self.recheck = false;
            if self.holds(key) {
                return;
            }
        }
        let path: &dyn RoutePath = route;
        let query = self.query.get_or_insert_with(|| {
            let query = PlaceQuery::new(
                self.generation,
                key.filter,
                PlaceWindow::Corridor { from_m: key.anchor_m, to_m, half_width_m: CORRIDOR_HALF_WIDTH_M },
                local,
            )
            .with_hours_filter(key.hours_filter);
            match self.start {
                Some((boundary, backwards)) => query.starting_after(boundary, backwards),
                None => query,
            }
        });
        for _ in 0..64 {
            self.status = query.step(reader, Some(path), self.generation, &mut self.pois);
            if self.status != QueryProgress::Pending {
                if let Err(error) = reader.refresh_place_hours(&mut self.pois, local) {
                    self.status = QueryProgress::Failed(error);
                    self.pois.clear();
                }
                self.taken_for = Some(key);
                break;
            }
        }
    }
}

/// The weekday and quarter hour that opening status depends on.
pub(crate) fn quarter(local: Option<(u8, u16)>) -> Option<(u8, u16)> {
    local.map(|(weekday, minute)| (weekday, minute / 15))
}

impl Default for CorridorScratch {
    fn default() -> Self {
        CorridorScratch::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use obc_reader::PoiCategory;

    fn key(filter: PoiCategorySet, anchor_m: u32) -> CorridorKey {
        CorridorKey { hours_filter: obc_reader::reader::places::HoursFilter::HideClosed, filter, anchor_m }
    }

    /// A fresh scratch wants nothing, so the reader seam stays quiet. That is the normal case: no
    /// Up-ahead screen is up and the corridor query costs nothing.
    #[test]
    fn disarmed_scratch_is_never_pending() {
        let s = CorridorScratch::new();
        assert!(!s.pending(), "nothing armed ⇒ the host is never asked for a Reader");
        assert!(s.armed().is_none());
        assert!(s.is_empty());
    }

    /// Arming makes the scratch pending; a take satisfies it, and re-arming the same key does not
    /// re-query (the frozen contract — progress advancing does not move the anchor).
    #[test]
    fn arm_then_take_settles_and_stays_settled() {
        let mut s = CorridorScratch::new();
        let k = key(PoiCategorySet::ALL, 1_000);
        s.arm(k);
        assert!(s.pending(), "armed but not taken");
        s.taken_for = Some(k); // stand in for a successful query (no Reader in a unit test)
        assert!(!s.pending());
        s.arm(k);
        assert!(!s.pending(), "re-arming the held key is a no-op");
    }

    /// A different filter re-arms, and so does a different anchor: the key is the pair, and either
    /// change drops the stale rows immediately.
    #[test]
    fn a_changed_key_invalidates_both_ways() {
        let mut s = CorridorScratch::new();
        let k = key(PoiCategorySet::ALL, 1_000);
        s.arm(k);
        s.taken_for = Some(k);

        s.arm(key(PoiCategorySet::only(PoiCategory::Water), 1_000));
        assert!(s.pending(), "a filter change re-queries");
        assert!(s.is_empty(), "and drops the stale rows at once");

        let k2 = key(PoiCategorySet::ALL, 1_000);
        s.arm(k2);
        s.taken_for = Some(k2);
        s.arm(key(PoiCategorySet::ALL, 4_000));
        assert!(s.pending(), "a new progress anchor re-queries");
    }

    /// `invalidate` forces a re-take of the *same* key (screen re-entry); `disarm` also stops the
    /// request, so the reader seam goes quiet.
    #[test]
    fn invalidate_retakes_and_disarm_goes_quiet() {
        let mut s = CorridorScratch::new();
        let k = key(PoiCategorySet::ALL, 0);
        s.arm(k);
        s.taken_for = Some(k);
        assert!(!s.pending());

        s.invalidate();
        assert!(s.pending(), "re-entry re-queries the identical key");
        assert_eq!(s.armed(), Some(k), "the request survives an invalidate");

        s.disarm();
        assert!(!s.pending(), "a closed screen stops asking");
        assert!(s.armed().is_none());
    }
}
