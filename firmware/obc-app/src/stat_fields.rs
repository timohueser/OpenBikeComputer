//! The Statistics grid's data fields: the catalogue the rider picks from, the ordered
//! selection they build, and the layout maths that places it.
//!
//! [`StatField`] is the catalogue and [`StatFieldList`] the selection persisted in [`Settings`].
//! [`page_count`] / [`page_fields`] walk the selection into 6-slot pages (3 rows x 2 cols). A
//! 2-span tile stays row-aligned and the waypoint panel page-aligned, so neither straddles a row.

include!("stat_fields/selection.rs");

use obc_reader::PoiCategory;
use obc_render::text::TextAlign;
use obc_route::{Profile, RouteReader, Waypoints};

use crate::i18n::{t, Msg};
use crate::navigator::RouteState;
use crate::screen::vocab::fmt;
use crate::settings::{DateTime, Language, Units};
use obc_ports::Fix;

/// The narrow live-data view a stat field formats from. It is decoupled from the full draw
/// context, so a cell is pure data-to-string and a test can build a bare `Readout`.
pub struct Readout<'a> {
    pub fix: Option<Fix>,
    pub navigation: &'a RouteState,
    pub recorder: &'a crate::recorder::RecorderMachine,
    pub units: Units,
    pub route: Option<&'a RouteReader<'a>>,
    pub profile: Option<&'a Profile>,
    /// The climb the rider is on now, or `None` between climbs.
    pub climb: Option<crate::screen::ActiveClimb<'a>>,
    /// The active route's named-waypoint table, in route order. Empty when no route is loaded.
    pub waypoints: &'a Waypoints,
    /// Index into [`waypoints`](Self::waypoints) of the next waypoint ahead, or `None`.
    pub next_waypoint: Option<usize>,
    pub now: DateTime,
    /// Boot-relative millis for this frame. The live sensor tiles use it as a staleness clock.
    pub now_ms: u32,
    /// The current bike type, which keys the time estimate.
    pub bike_type: crate::settings::BikeType,
    pub language: Language,
    /// The per-category "next ahead" cache the six `Next: <category>` tiles read. It is empty on
    /// a host that never refreshes it, which makes those tiles waypoint-only.
    pub next_ahead: &'a crate::next_ahead::NextAhead,
    /// The length of the loaded trip day's later days, or `None` off a trip.
    pub trip_later_m: Option<u32>,
}

/// Grid geometry: a page is `ROWS_PER_PAGE` x `COLS` tiles. A single-span field fills one slot, a
/// two-span field a whole row.
pub const COLS: usize = 2;
pub const ROWS_PER_PAGE: usize = 3;
pub const SLOTS_PER_PAGE: usize = ROWS_PER_PAGE * COLS;

/// Max fields the rider can pin to the grid: two full pages.
pub const MAX_STAT_FIELDS: usize = 2 * SLOTS_PER_PAGE;

// Rows are in picker order; explicit byte IDs remain independent of that order.
macro_rules! stat_field_table {
    ($( $(#[$doc:meta])* $field:ident = $id:literal, $name:ident, $span:literal, $rows:literal, $category:expr; )+) => {
        /// One predefined data field. The explicit byte IDs are the persisted settings contract.
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        #[repr(u8)]
        pub enum StatField {
            $( $(#[$doc])* $field = $id, )+
        }

        impl StatField {
            /// Every field in picker order.
            pub const ALL: [Self; [$(stringify!($field)),+].len()] = [$(Self::$field),+];

            /// The localized picker name. POI fields use their category's catalog string.
            pub const fn name(self, lang: Language) -> &'static str {
                match self { $(Self::$field => t(Msg::$name, lang),)+ }
            }

            pub const fn category(self) -> Option<PoiCategory> {
                match self { $(Self::$field => $category,)+ }
            }

            /// The field's width in grid columns.
            pub const fn span(self) -> u8 {
                match self { $(Self::$field => $span,)+ }
            }

            pub const fn rows(self) -> u8 {
                match self { $(Self::$field => $rows,)+ }
            }
        }
    };
}

stat_field_table! {
    Speed = 0, StatfieldSpeed, 1, 1, None;
    AvgSpeed = 1, StatfieldAvgSpeed, 1, 1, None;
    DistDone = 2, StatfieldDistDone, 1, 1, None;
    DistToGo = 3, StatfieldDistToGo, 1, 1, None;
    Climbed = 4, StatfieldClimbed, 1, 1, None;
    ToClimb = 5, StatfieldToClimb, 1, 1, None;
    Grade = 6, StatfieldGrade, 1, 1, None;
    /// Live altitude: map-referenced once the offset estimator settles, raw barometric until then.
    Elevation = 7, StatfieldElevation, 1, 1, None;
    RideTime = 8, StatfieldRideTime, 1, 1, None;
    TimeToGo = 21, StatfieldTimeToGo, 1, 1, None;
    Eta = 22, StatfieldEta, 1, 1, None;
    TripToGo = 23, StatfieldTripToGo, 1, 1, None;
    Clock = 9, StatfieldClock, 2, 1, None;
    NextWaypoint = 10, StatfieldNextWaypoint, 2, 1, None;
    NextWater = 15, PoiCatWater, 2, 1, Some(PoiCategory::Water);
    NextCampsite = 16, PoiCatCampsite, 2, 1, Some(PoiCategory::Campsite);
    NextLodging = 17, PoiCatAccommodation, 2, 1, Some(PoiCategory::Accommodation);
    NextResupply = 18, PoiCatResupply, 2, 1, Some(PoiCategory::Resupply);
    NextPharmacy = 19, PoiCatPharmacy, 2, 1, Some(PoiCategory::Pharmacy);
    NextBikeShop = 20, PoiCatBikeShop, 2, 1, Some(PoiCategory::BikeShop);
    WaypointList = 11, StatfieldWaypointList, 2, 3, None;
    HeartRate = 12, StatfieldHeartRate, 1, 1, None;
    Power = 13, StatfieldPower, 1, 1, None;
    Cadence = 14, StatfieldCadence, 1, 1, None;
}

impl StatField {
    /// Decode a persisted discriminant. Unknown bytes are dropped by the settings codec.
    pub fn from_u8(b: u8) -> Option<StatField> {
        Self::ALL.into_iter().find(|f| *f as u8 == b)
    }

    pub const fn slots(self) -> usize {
        self.span() as usize * self.rows() as usize
    }

    /// The rendered tile content: a caption, the number-only value, and whether to prefix an
    /// up-triangle. The unit sits in the caption so the big digits fit the half-width tile.
    pub fn cell(&self, cx: &Readout) -> StatCell {
        let units = cx.units;
        let lang = cx.language;
        let live = live_frac(cx.navigation);
        match self {
            StatField::Speed => {
                let v = cx.fix.and_then(|f| f.speed_mps).map(|mps| units.speed(mps * 3.6));
                StatCell::new(cap(units.speed_label(), ""), fmt::speed_figure(v), false)
            }
            StatField::AvgSpeed => {
                let v = cx.recorder.avg_kmh().map(|kmh| units.speed(kmh));
                StatCell::new(cap(t(Msg::TileAvg, lang), units.speed_label()), fmt::speed_figure(v), false)
            }
            StatField::DistDone => StatCell::new(
                cap(units.dist_label(), t(Msg::TileDone, lang)),
                fmt::distance_figure(units.dist(cx.recorder.ridden_m() / 1000.0)),
                false,
            ),
            StatField::DistToGo => {
                // With no route loaded there is nothing to go, so the tile reads `--` and not a
                // misleading 0.0.
                let value = match cx.route {
                    Some(r) => fmt::distance_figure(
                        units.dist(r.total_distance_m.saturating_sub(cx.navigation.progress_m) as f32 / 1000.0),
                    ),
                    None => fmt::dashes(),
                };
                StatCell::new(cap(units.dist_label(), t(Msg::TileToGo, lang)), value, false)
            }
            StatField::Climbed => StatCell::new(
                cap(t(Msg::TileClimbed, lang), ""),
                fmt::integer(units.elev(cx.recorder.climb_m()) as u32),
                true,
            ),
            StatField::ToClimb => {
                // The remaining ascent comes from the profile lookup the Up-ahead rows and the
                // time model also read, so TO CLIMB and TIME TO GO can never disagree.
                let value = match (cx.route, cx.profile) {
                    (Some(r), Some(p)) => fmt::integer(units.elev(ascent_to_go_m(r, p, cx.navigation) as f32) as u32),
                    _ => fmt::dashes(),
                };
                StatCell::new(cap(t(Msg::TileToClimb, lang), ""), value, true)
            }
            StatField::Grade => {
                let value = match (cx.route, cx.profile) {
                    (Some(r), Some(p)) => grade_at(p, r.total_distance_m, live).map_or_else(fmt::dashes, fmt::percent),
                    _ => fmt::dashes(),
                };
                StatCell::new(cap(t(Msg::TileGrade, lang), ""), value, false)
            }
            StatField::Elevation => {
                // The live altitude, not the route profile: it reads the current height with no
                // route loaded. Map-referenced once the estimator settles, raw barometric before.
                let v = cx.recorder.current_elevation_m().map(|m| units.elev(m));
                StatCell::new(cap(t(Msg::TileElev, lang), units.elev_label()), fmt::elevation_rounded(v), false)
            }
            StatField::RideTime => {
                StatCell::new(cap(t(Msg::TileRide, lang), ""), fmt::duration_hms(cx.recorder.moving_s()), false)
            }
            // Both time tiles read one number, the gradient-aware seconds still to ride, and
            // differ only in how they present it.
            StatField::TimeToGo => {
                let value = match time_to_go_s(cx) {
                    Some(s) => fmt::duration_hms(s as f32),
                    None => fmt::dashes(),
                };
                // The unit rides in the caption: a single-column tile fits 8 Label glyphs, so a
                // spelled-out "TIME TO GO" would only be ellipsised back to this.
                StatCell::new(cap("h", t(Msg::TileToGo, lang)), value, false)
            }
            StatField::Eta => {
                // Rounded to the nearest minute, because an arrival time is read to the minute.
                let value = match time_to_go_s(cx) {
                    Some(s) => {
                        let at = cx.now.add_minutes((s + 30) / 60);
                        fmt::clock_hm(at.hour, at.minute)
                    }
                    None => fmt::dashes(),
                };
                StatCell::new(cap(t(Msg::TileEta, lang), ""), value, false)
            }
            StatField::TripToGo => {
                let value = match (cx.trip_later_m, cx.route) {
                    (Some(later_m), Some(r)) => {
                        let to_go_m = r.total_distance_m.saturating_sub(cx.navigation.progress_m) + later_m;
                        // Whole units: each later day is a whole-kilometre catalog figure.
                        fmt::integer((units.dist(to_go_m as f32 / 1000.0) + 0.5) as u32)
                    }
                    _ => fmt::dashes(),
                };
                StatCell::new(cap(t(Msg::TileTrip, lang), units.dist_label()), value, false)
            }
            StatField::Clock => {
                let value = fmt::clock_hm(cx.now.hour, cx.now.minute);
                StatCell::new(cap(t(Msg::TileTime, lang), ""), value, false)
            }
            StatField::NextWaypoint => {
                // The caption is the waypoint's name, the value its along-route distance to go.
                // It clamps to `0m` through the 100 m pass-linger.
                let mut cell = match cx.next_waypoint.and_then(|k| cx.waypoints.as_slice().get(k)) {
                    Some(wp) => {
                        let mut caption: heapless::String<24> = heapless::String::new();
                        let _ = caption.push_str(wp.name.as_str());
                        let value =
                            fmt::distance_short(wp.dist_along_m.saturating_sub(cx.navigation.progress_m), units);
                        StatCell::new(caption, value, false)
                    }
                    None => StatCell::new(cap(t(Msg::TileNextWpt, lang), ""), fmt::dashes(), false),
                };
                cell.value_align = TextAlign::Right;
                cell
            }
            StatField::WaypointList => {
                // The panel is drawn by `waypoint_panel`, because its 2x3 list does not fit the
                // caption + value shape of a `StatCell`. This arm only keeps `cell` total.
                StatCell::new(cap(t(Msg::TileWaypoints, lang), ""), fmt::dashes(), false)
            }
            StatField::HeartRate => {
                // `_display` judges freshness on the ride clock the sample recorded on, not on
                // this render's clock. The two differ in the sim during a GPX replay.
                let v = cx.recorder.live_hr_display().map(|bpm| bpm as u32);
                StatCell::new(cap(t(Msg::TileHr, lang), ""), fmt::integer_opt(v), false)
            }
            StatField::Power => {
                let v = cx.recorder.live_power_display().map(|w| w as u32);
                StatCell::new(cap(t(Msg::TilePwr, lang), ""), fmt::integer_opt(v), false)
            }
            StatField::Cadence => {
                // A fresh `0` (coasting) is a real reading and shows `0`; only an absent or
                // stale value reads `--`.
                let v = cx.recorder.live_cadence_display().map(|rpm| rpm as u32);
                StatCell::new(cap(t(Msg::TileRpm, lang), ""), fmt::integer_opt(v), false)
            }
            // One arm for the six `Next: <category>` tiles, because only `category()` differs.
            // It is spelled out so a new field fails to compile until it has a cell.
            StatField::NextWater
            | StatField::NextCampsite
            | StatField::NextLodging
            | StatField::NextResupply
            | StatField::NextPharmacy
            | StatField::NextBikeShop => {
                let cat = self.category().expect("the six Next: variants all carry a category");
                let mut cell = match next_of_category(cat, cx) {
                    Some((dist_along_m, name)) => {
                        let mut caption: heapless::String<24> = heapless::String::new();
                        for ch in name.chars() {
                            if caption.push(ch).is_err() {
                                break;
                            }
                        }
                        let value = fmt::distance_short(dist_along_m - cx.navigation.progress_m, units);
                        StatCell::new(caption, value, false)
                    }
                    // Nothing of this kind ahead. The icon still says what, so the caption falls
                    // back to the category's name.
                    None => StatCell::new(cap(self.name(lang), ""), fmt::dashes(), false),
                };
                cell.value_align = TextAlign::Right;
                cell
            }
        }
    }
}

/// The nearest thing of `cat` ahead on the route, from the resident waypoint table or the cached
/// corridor POI, with its along-route position and its name. "Ahead" means
/// `dist_along_m >= progress`, and a tie goes to the rider's own waypoint. Both rules come from the
/// Up-ahead timeline, so one entry cannot read differently in the list and in a tile.
fn next_of_category<'a>(cat: PoiCategory, cx: &'a Readout<'a>) -> Option<(u32, &'a str)> {
    // The guard is the active route, not the frame's `route` reader: a route-less ride must never
    // leak the previous route's resident table or a cache line taken against it.
    cx.navigation.active_route?;
    let progress = cx.navigation.progress_m;
    let wpt = cx
        .waypoints
        .as_slice()
        .iter()
        .find(|w| w.category == Some(cat) && w.dist_along_m >= progress)
        .map(|w| (w.dist_along_m, w.name.as_str()));
    let poi = cx.next_ahead.poi(cat).filter(|p| p.dist_along_m >= progress).map(|p| (p.dist_along_m, p.name.as_str()));
    match (wpt, poi) {
        (Some(w), Some(p)) if p.0 < w.0 => Some(p),
        (Some(w), _) => Some(w),
        (None, p) => p,
    }
}

/// The rendered content of one tile. The caption is `String<24>` so a waypoint name fits; the tile
/// drawer ellipsis-truncates one that overflows the tile width.
pub struct StatCell {
    pub caption: heapless::String<24>,
    pub value: heapless::String<10>,
    pub arrow: bool,
    pub value_align: TextAlign,
}

impl StatCell {
    fn new(caption: heapless::String<24>, value: impl AsRef<str>, arrow: bool) -> Self {
        let value = heapless::String::try_from(value.as_ref()).expect("stat values fit the value buffer");
        StatCell { caption, value, arrow, value_align: TextAlign::Left }
    }
}

/// Glue two caption fragments into a tile caption, such as `"AVG "` + `Units::speed_label()`.
fn cap(a: &str, b: &str) -> heapless::String<24> {
    let mut s = heapless::String::new();
    let _ = s.push_str(a);
    let _ = s.push_str(b);
    s
}

/// Cached signed endpoint grade at the requested route position, absent over missing spans.
pub(crate) fn grade_at(profile: &obc_route::Profile, _total_distance_m: u32, frac: f32) -> Option<i32> {
    profile.grade_at(frac)
}

/// The ascent (m) still to climb between the rider's matched progress and the end of the route.
/// The length axis is the route reader's total, so this cannot disagree with `DIST TO GO` about
/// where the end is.
fn ascent_to_go_m(r: &RouteReader, p: &Profile, navigation: &RouteState) -> u32 {
    p.ascent_between_m(navigation.progress_m, r.total_distance_m, r.total_distance_m)
}

/// Seconds still to ride to the end of the route, or `None` with no route or profile. It is the
/// shared source for the [`TimeToGo`](StatField::TimeToGo) and [`Eta`](StatField::Eta) tiles, so
/// the duration and the arrival stamp are always the same estimate rendered two ways.
fn time_to_go_s(cx: &Readout) -> Option<u32> {
    let (r, p) = (cx.route?, cx.profile?);
    Some(obc_route::time_to_go_s(p, r.total_distance_m, cx.navigation.progress_m, cx.bike_type))
}

/// The fractional live position (`0.0`-`1.0`) along the route; `0.0` when no length is known.
pub(crate) fn live_frac(navigation: &RouteState) -> f32 {
    if navigation.route_total_m == 0 {
        0.0
    } else {
        (navigation.progress_m as f32 / navigation.route_total_m as f32).clamp(0.0, 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::support::wpts;
    use crate::recorder::RecorderMachine;
    use core::fmt::Write;

    #[test]
    fn const_default_equals_the_push_built_grid() {
        let mut pushed = StatFieldList { ids: [StatField::Speed; MAX_STAT_FIELDS], len: 0 };
        for f in [
            StatField::Speed,
            StatField::AvgSpeed,
            StatField::DistDone,
            StatField::DistToGo,
            StatField::Climbed,
            StatField::ToClimb,
        ] {
            assert!(pushed.push(f));
        }
        assert_eq!(StatFieldList::DEFAULT, pushed);
        assert_eq!(StatFieldList::default(), pushed);
    }

    fn list(fields: &[StatField]) -> StatFieldList {
        let mut l = StatFieldList { ids: [StatField::Speed; MAX_STAT_FIELDS], len: 0 };
        for &f in fields {
            assert!(l.push(f), "test list overflowed or duplicated");
        }
        l
    }

    #[test]
    fn default_is_the_classic_six() {
        let l = StatFieldList::default();
        assert_eq!(l.len(), 6);
        assert_eq!(page_count(&l), 1);
        assert_eq!(page_fields(&l, 0).len(), 6);
    }

    #[test]
    fn singles_pack_into_pages_of_six() {
        let l = list(&[
            StatField::Speed,
            StatField::AvgSpeed,
            StatField::DistDone,
            StatField::DistToGo,
            StatField::Climbed,
            StatField::ToClimb,
            StatField::Grade,
        ]);
        assert_eq!(page_count(&l), 2);
        let p0 = page_fields(&l, 0);
        assert_eq!(p0.len(), 6);
        assert_eq!((p0[0].col, p0[0].row), (0, 0));
        assert_eq!((p0[1].col, p0[1].row), (1, 0));
        assert_eq!((p0[5].col, p0[5].row), (1, 2));
        let p1 = page_fields(&l, 1);
        assert_eq!(p1.len(), 1);
        assert_eq!((p1[0].field, p1[0].col, p1[0].row), (StatField::Grade, 0, 0));
    }

    #[test]
    fn two_span_fills_a_row() {
        let l = list(&[StatField::Clock, StatField::Speed]);
        let p = page_fields(&l, 0);
        assert_eq!((p[0].field, p[0].col, p[0].row), (StatField::Clock, 0, 0));
        assert_eq!((p[1].field, p[1].col, p[1].row), (StatField::Speed, 0, 1));
    }

    #[test]
    fn two_span_after_one_single_starts_a_new_row() {
        let l = list(&[StatField::Speed, StatField::Clock]);
        let p = page_fields(&l, 0);
        assert_eq!((p[0].col, p[0].row), (0, 0), "the single sits top-left");
        assert_eq!((p[1].field, p[1].col, p[1].row), (StatField::Clock, 0, 1), "the wide tile bumps to row 1");
    }

    #[test]
    fn move_single_steps_by_one() {
        let mut l = list(&[StatField::Speed, StatField::AvgSpeed, StatField::DistDone]);
        let ni = l.move_item(0, 1);
        assert_eq!(ni, 1);
        assert_eq!(l.as_slice(), &[StatField::AvgSpeed, StatField::Speed, StatField::DistDone]);
    }

    #[test]
    fn move_two_span_hops_a_row() {
        let mut l = list(&[StatField::Clock, StatField::Speed, StatField::AvgSpeed, StatField::DistDone]);
        let ni = l.move_item(0, 1);
        assert_eq!(ni, 2, "the wide tile lands after the pair, not between them");
        assert_eq!(l.as_slice(), &[StatField::Speed, StatField::AvgSpeed, StatField::Clock, StatField::DistDone]);
        for placed in page_fields(&l, 0) {
            if placed.field == StatField::Clock {
                assert_eq!(placed.col, 0, "the wide tile always begins a row");
            }
        }
    }

    #[test]
    fn move_off_the_end_is_a_noop() {
        let mut l = list(&[StatField::Speed, StatField::AvgSpeed]);
        assert_eq!(l.move_item(1, 1), 1, "the last item can't move further down");
        assert_eq!(l.as_slice(), &[StatField::Speed, StatField::AvgSpeed]);
        assert_eq!(l.move_item(0, -1), 0, "the first item can't move further up");
    }

    #[test]
    fn add_and_remove() {
        let mut l = list(&[StatField::Speed]);
        assert!(l.push(StatField::Clock));
        assert!(!l.push(StatField::Speed), "a duplicate is refused");
        assert_eq!(l.as_slice(), &[StatField::Speed, StatField::Clock]);
        l.remove(0);
        assert_eq!(l.as_slice(), &[StatField::Clock]);
    }

    /// An empty cache. A `static`, so it outlives the borrowed `Readout`.
    static EMPTY_CACHE: &crate::next_ahead::NextAhead = &crate::next_ahead::NextAhead::EMPTY;

    /// A ride that has recorded nothing. A `static`, so it outlives the borrowed `Readout`.
    fn idle_recorder() -> &'static RecorderMachine {
        static IDLE: std::sync::LazyLock<RecorderMachine> = std::sync::LazyLock::new(RecorderMachine::new);
        &IDLE
    }

    fn readout<'a>(
        navigation: &'a RouteState,
        recorder: &'a RecorderMachine,
        units: Units,
        waypoints: &'a Waypoints,
    ) -> Readout<'a> {
        Readout {
            fix: None,
            navigation,
            recorder,
            units,
            route: None,
            profile: None,
            climb: None,
            waypoints,
            next_waypoint: None,
            now: DateTime::default(),
            now_ms: 0,
            bike_type: crate::settings::BikeType::Road,
            language: Language::En,
            next_ahead: EMPTY_CACHE,
            trip_later_m: None,
        }
    }

    /// A ~9 km pass: 500 m up to 800 m and back down, zigzagged so no corner decimates away.
    const PASS_GPX: &str = r#"<gpx><trk><trkseg>
    <trkpt lat="47.0000" lon="8.0000"><ele>500</ele></trkpt>
    <trkpt lat="47.0020" lon="8.0200"><ele>600</ele></trkpt>
    <trkpt lat="47.0000" lon="8.0400"><ele>700</ele></trkpt>
    <trkpt lat="47.0020" lon="8.0600"><ele>800</ele></trkpt>
    <trkpt lat="47.0000" lon="8.0800"><ele>700</ele></trkpt>
    <trkpt lat="47.0020" lon="8.1000"><ele>600</ele></trkpt>
    <trkpt lat="47.0000" lon="8.1200"><ele>500</ele></trkpt>
  </trkseg></trk></gpx>"#;

    /// Runs `f` with the converted route and its profile. A `RouteReader` borrows its source, so
    /// this takes a closure instead of returning the pair.
    fn with_pass_route<R>(f: impl FnOnce(&RouteReader, &Profile) -> R) -> R {
        use crate::harness::support::VecSink;
        use obc_formats::io::SliceSource;

        let mut sink = VecSink::default();
        obc_route::gpx_to_obcr(&SliceSource(PASS_GPX.as_bytes()), "Pass", &mut sink).unwrap();
        let src = SliceSource(&sink.0);
        let idx = obc_route::RouteIndex::read(&src).unwrap();
        let route = RouteReader::new(&idx, &src);
        let profile = route.elevation_profile();
        f(&route, &profile)
    }

    /// Pins the wiring (route + profile + bike type in, the right two strings out), not the
    /// physics of the model itself.
    #[test]
    fn time_tiles_render_the_gradient_aware_estimate() {
        let rec = idle_recorder();
        with_pass_route(|route, profile| {
            let mut navigation = RouteState::new();
            navigation.route_total_m = route.total_distance_m;
            let empty = Waypoints::new();
            let cx = Readout {
                route: Some(route),
                profile: Some(profile),
                now: DateTime { hour: 14, minute: 0, ..DateTime::default() },
                ..readout(&navigation, rec, Units::Metric, &empty)
            };
            assert_eq!(route.total_ascent_m, 300, "the fixture climbs 300 m");

            let secs = crate::settings::BikeType::Road.ride_time_s(route.total_distance_m, route.total_ascent_m);
            assert_eq!(StatField::TimeToGo.cell(&cx).value.as_str(), fmt::duration_hms(secs as f32).as_str());
            let mins = (secs + 30) / 60;
            let mut want: heapless::String<8> = heapless::String::new();
            let at = DateTime { hour: 14, minute: 0, ..DateTime::default() }.add_minutes(mins);
            write!(want, "{:02}:{:02}", at.hour, at.minute).unwrap();
            assert_eq!(StatField::Eta.cell(&cx).value.as_str(), want.as_str());

            // The climb term on the Road profile is 300 m x 1.6 s/m = 480 s.
            let flat = crate::settings::BikeType::Road.ride_time_s(route.total_distance_m, 0);
            assert!((secs - flat).abs_diff(480) <= 2, "the climb term is {} s", secs - flat);

            let imperial = Readout {
                route: Some(route),
                profile: Some(profile),
                ..readout(&navigation, rec, Units::Imperial, &empty)
            };
            assert_eq!(StatField::TimeToGo.cell(&imperial).value, StatField::TimeToGo.cell(&cx).value);
        });
    }

    #[test]
    fn time_tiles_follow_the_bike_profile() {
        let rec = idle_recorder();
        with_pass_route(|route, profile| {
            let mut navigation = RouteState::new();
            navigation.route_total_m = route.total_distance_m;
            let empty = Waypoints::new();
            let secs = |bike| {
                let cx = Readout {
                    route: Some(route),
                    profile: Some(profile),
                    bike_type: bike,
                    ..readout(&navigation, rec, Units::Metric, &empty)
                };
                time_to_go_s(&cx).unwrap()
            };
            use crate::settings::BikeType;
            assert!(secs(BikeType::Mtb) > secs(BikeType::Road), "an MTB is slower than a road bike over the same pass");
        });
    }

    #[test]
    fn time_tiles_count_down_as_the_ride_advances() {
        let rec = idle_recorder();
        with_pass_route(|route, profile| {
            let total = route.total_distance_m;
            let empty = Waypoints::new();
            let mut prev = u32::MAX;
            let mut prev_eta: std::string::String = std::string::String::new();
            for step in 0..=40u32 {
                let mut navigation = RouteState::new();
                navigation.route_total_m = total;
                navigation.progress_m = total * step / 40;
                let cx = Readout {
                    route: Some(route),
                    profile: Some(profile),
                    now: DateTime { hour: 14, minute: 0, ..DateTime::default() },
                    ..readout(&navigation, rec, Units::Metric, &empty)
                };
                let secs = time_to_go_s(&cx).unwrap();
                assert!(secs <= prev, "time-to-go rose from {prev} to {secs} s at {} m", navigation.progress_m);
                prev = secs;
                let eta = StatField::Eta.cell(&cx).value;
                if !prev_eta.is_empty() {
                    assert!(eta.as_str() <= prev_eta.as_str(), "ETA slipped from {prev_eta} to {eta}");
                }
                prev_eta = eta.as_str().into();
            }
            assert_eq!(prev, 0, "nothing left at the finish");
            assert_eq!(prev_eta, "14:00", "arriving now — the ETA is the wall clock");
        });
    }

    #[test]
    fn trip_to_go_adds_the_later_days_to_the_rest_of_the_loaded_day() {
        let rec = idle_recorder();
        with_pass_route(|route, _| {
            let mut navigation = RouteState::new();
            navigation.route_total_m = route.total_distance_m;
            navigation.progress_m = route.total_distance_m - 1_000;
            let empty = Waypoints::new();
            let cell = |units| {
                let cx = Readout {
                    route: Some(route),
                    trip_later_m: Some(61_000),
                    ..readout(&navigation, rec, units, &empty)
                };
                let cell = StatField::TripToGo.cell(&cx);
                (cell.caption.as_str().to_owned(), cell.value.as_str().to_owned())
            };
            assert_eq!(cell(Units::Metric), ("TRIP KM".into(), "62".into()), "the last km of today, 61 km later");
            assert_eq!(cell(Units::Imperial), ("TRIP MI".into(), "39".into()));
        });
    }

    #[test]
    fn elevation_tile_reads_live_barometric_altitude() {
        let navigation = RouteState::new();
        let empty = Waypoints::new();
        let mut rec = RecorderMachine::new();
        let value =
            |rec: &RecorderMachine, units| StatField::Elevation.cell(&readout(&navigation, rec, units, &empty)).value;

        assert_eq!(value(&rec, Units::Metric).as_str(), "--", "no altimeter sample yet");

        rec.record_altitude(144.0, true);
        assert_eq!(value(&rec, Units::Metric).as_str(), "144", "metric shows whole metres");
        // 144 m × 3.28084 ≈ 472.4 ft → rounds to 472.
        assert_eq!(value(&rec, Units::Imperial).as_str(), "472", "imperial converts to feet");
    }

    #[test]
    fn elevation_tile_switches_to_the_fused_height_once_settled() {
        let navigation = RouteState::new();
        let empty = Waypoints::new();
        let mut rec = RecorderMachine::new();
        let value =
            |rec: &RecorderMachine| StatField::Elevation.cell(&readout(&navigation, rec, Units::Metric, &empty)).value;

        // The barometer reads 62 m high all ride; the terrain under the fix says 1800 m.
        rec.record_altitude(1862.0, true);
        rec.record_map_elevation(1800);
        assert_eq!(value(&rec).as_str(), "1862", "one residual is not settled → the raw reading");

        for _ in 1..crate::altitude::SETTLE_SAMPLES {
            rec.record_altitude(1862.0, true);
            rec.record_map_elevation(1800);
        }
        assert_eq!(value(&rec).as_str(), "1800", "settled → the map-referenced height");
        rec.record_altitude(1902.0, true);
        assert_eq!(value(&rec).as_str(), "1840", "baro supplies the dynamics, the map the frame");
    }

    #[test]
    fn fields_fall_back_without_data() {
        let rec = idle_recorder();
        let navigation = RouteState::new();
        let empty = Waypoints::new();
        let cx = readout(&navigation, rec, Units::Metric, &empty);
        let val = |f: StatField| f.cell(&cx).value;
        assert_eq!(val(StatField::Speed).as_str(), "--", "no fix → no live speed");
        assert_eq!(val(StatField::AvgSpeed).as_str(), "--", "no moving time → no average");
        assert_eq!(val(StatField::Elevation).as_str(), "--", "no altimeter sample yet");
        assert_eq!(val(StatField::DistDone).as_str(), "0.0");
        assert_eq!(val(StatField::DistToGo).as_str(), "--", "no route → the route-relative tile reads --");
        assert_eq!(val(StatField::Climbed).as_str(), "0");
        assert_eq!(val(StatField::ToClimb).as_str(), "--", "no route → nothing to climb, reads --");
        assert_eq!(val(StatField::Grade).as_str(), "--", "no route → grade reads --");
        assert_eq!(val(StatField::RideTime).as_str(), "0:00");
        assert_eq!(val(StatField::TimeToGo).as_str(), "--", "no route → no end to ride to, reads --");
        assert_eq!(val(StatField::Eta).as_str(), "--", "no route → no arrival to estimate, reads --");
        assert_eq!(val(StatField::TripToGo).as_str(), "--", "no trip → nothing left of one, reads --");
        assert_eq!(StatField::TripToGo.cell(&cx).caption.as_str(), "TRIP KM");
        assert_eq!(val(StatField::Clock).as_str(), "12:00", "the neutral default DateTime");
        assert_eq!(val(StatField::NextWaypoint).as_str(), "--", "no route → the waypoint tile reads --");
    }

    #[test]
    fn route_less_ride_shows_dashes_for_route_fields_but_real_data_otherwise() {
        let navigation = RouteState::new();
        let mut rec = RecorderMachine::new();
        rec.record_fix(Fix::at(52_520_000, 13_405_000), 0, true);
        rec.record_fix(Fix::at(52_520_100, 13_405_000), 2000, true);
        rec.record_altitude(200.0, true);
        rec.record_altitude(230.0, true); // +30 m climbed
        let empty = Waypoints::new();
        let cx = readout(&navigation, &rec, Units::Metric, &empty);
        let val = |f: StatField| f.cell(&cx).value;
        assert_eq!(val(StatField::DistToGo).as_str(), "--", "no route → to-go reads --");
        assert_eq!(val(StatField::ToClimb).as_str(), "--", "no route → to-climb reads --");
        assert_eq!(val(StatField::Grade).as_str(), "--", "no route → grade reads --");
        assert_eq!(val(StatField::TimeToGo).as_str(), "--", "no route → time-to-go reads --");
        assert_eq!(val(StatField::Eta).as_str(), "--", "no route → ETA reads --");
        assert_ne!(val(StatField::DistDone).as_str(), "--", "distance done is real, not --");
        assert_eq!(val(StatField::Climbed).as_str(), "30", "climbed is barometric, route-independent");
        assert_eq!(val(StatField::Elevation).as_str(), "230", "elevation is the live altitude");
    }

    #[test]
    fn speed_tile_reads_the_fix() {
        let rec = idle_recorder();
        let navigation = RouteState::new();
        let empty = Waypoints::new();
        let fix = Fix { speed_mps: Some(10.0), ..Fix::at(0, 0) };
        let value = |units: Units| {
            StatField::Speed.cell(&Readout { fix: Some(fix), ..readout(&navigation, rec, units, &empty) }).value
        };
        assert_eq!(value(Units::Metric).as_str(), "36.0", "10 m/s reads 36 km/h");
        assert_eq!(value(Units::Imperial).as_str(), "22.4", "…and 22.4 mph");
    }

    /// Literal expectations pin persisted IDs and picker metadata independently of the declaration.
    #[test]
    fn field_metadata_contract() {
        use StatField::*;
        let expected = [
            (Speed, 0, Msg::StatfieldSpeed, 1, 1, None),
            (AvgSpeed, 1, Msg::StatfieldAvgSpeed, 1, 1, None),
            (DistDone, 2, Msg::StatfieldDistDone, 1, 1, None),
            (DistToGo, 3, Msg::StatfieldDistToGo, 1, 1, None),
            (Climbed, 4, Msg::StatfieldClimbed, 1, 1, None),
            (ToClimb, 5, Msg::StatfieldToClimb, 1, 1, None),
            (Grade, 6, Msg::StatfieldGrade, 1, 1, None),
            (Elevation, 7, Msg::StatfieldElevation, 1, 1, None),
            (RideTime, 8, Msg::StatfieldRideTime, 1, 1, None),
            (TimeToGo, 21, Msg::StatfieldTimeToGo, 1, 1, None),
            (Eta, 22, Msg::StatfieldEta, 1, 1, None),
            (TripToGo, 23, Msg::StatfieldTripToGo, 1, 1, None),
            (Clock, 9, Msg::StatfieldClock, 2, 1, None),
            (NextWaypoint, 10, Msg::StatfieldNextWaypoint, 2, 1, None),
            (NextWater, 15, Msg::PoiCatWater, 2, 1, Some(PoiCategory::Water)),
            (NextCampsite, 16, Msg::PoiCatCampsite, 2, 1, Some(PoiCategory::Campsite)),
            (NextLodging, 17, Msg::PoiCatAccommodation, 2, 1, Some(PoiCategory::Accommodation)),
            (NextResupply, 18, Msg::PoiCatResupply, 2, 1, Some(PoiCategory::Resupply)),
            (NextPharmacy, 19, Msg::PoiCatPharmacy, 2, 1, Some(PoiCategory::Pharmacy)),
            (NextBikeShop, 20, Msg::PoiCatBikeShop, 2, 1, Some(PoiCategory::BikeShop)),
            (WaypointList, 11, Msg::StatfieldWaypointList, 2, 3, None),
            (HeartRate, 12, Msg::StatfieldHeartRate, 1, 1, None),
            (Power, 13, Msg::StatfieldPower, 1, 1, None),
            (Cadence, 14, Msg::StatfieldCadence, 1, 1, None),
        ];
        assert_eq!(StatField::ALL, expected.map(|(field, ..)| field), "picker order");
        for (field, id, name, span, rows, category) in expected {
            assert_eq!(field as u8, id, "persisted ID for {field:?}");
            assert_eq!(StatField::from_u8(id), Some(field));
            assert_eq!((field.span(), field.rows(), field.category()), (span, rows, category), "{field:?}");
            for lang in [Language::En, Language::De, Language::Fr, Language::Es] {
                assert_eq!(field.name(lang), t(name, lang), "{field:?} in {lang:?}");
            }
        }
        for byte in 24..=u8::MAX {
            assert_eq!(StatField::from_u8(byte), None, "unknown persisted ID {byte}");
        }
        let list = StatFieldList::decode(1, &[10]);
        assert_eq!(list.as_slice(), &[NextWaypoint], "a decoded byte-10 selection keeps the field");
    }

    #[test]
    fn empty_selection_is_one_page() {
        let l = list(&[]);
        assert_eq!(page_count(&l), 1);
        assert!(page_fields(&l, 0).is_empty());
    }

    #[test]
    fn next_waypoint_tile_names_and_counts_down() {
        let rec = idle_recorder();
        let w = wpts(&[(1_000, "Brunnen"), (5_000, "Pass Summit")]);
        let mut navigation = RouteState::new();
        navigation.progress_m = 1_200; // past Brunnen, 3.8 km before Pass Summit
        let cell = |units| {
            StatField::NextWaypoint.cell(&Readout { next_waypoint: Some(1), ..readout(&navigation, rec, units, &w) })
        };

        let m = cell(Units::Metric);
        assert_eq!(m.caption.as_str(), "Pass Summit", "caption is the waypoint's name");
        assert_eq!(m.value.as_str(), "3.8km", "metric distance-to-go = 5000 − 1200 m");
        assert_eq!(m.value_align, TextAlign::Right, "the wide-tile value hugs the far edge");
        assert!(!m.arrow, "no climb triangle on the waypoint tile");

        let i = cell(Units::Imperial);
        assert_eq!(i.caption.as_str(), "Pass Summit");
        assert_eq!(i.value.as_str(), "2.4mi", "imperial distance-to-go (3800 m ≈ 2.36 mi)");
    }

    #[test]
    fn next_waypoint_tile_clamps_to_zero_in_the_linger() {
        let rec = idle_recorder();
        let w = wpts(&[(1_000, "Brunnen")]);
        let mut navigation = RouteState::new();
        navigation.progress_m = 1_050; // 50 m past Brunnen, still inside its 100 m linger band
        let cell = StatField::NextWaypoint
            .cell(&Readout { next_waypoint: Some(0), ..readout(&navigation, rec, Units::Metric, &w) });
        assert_eq!(cell.caption.as_str(), "Brunnen");
        assert_eq!(cell.value.as_str(), "0m", "saturating_sub clamps the passed distance to zero");
    }

    #[test]
    fn next_waypoint_tile_empty_state() {
        let rec = idle_recorder();
        let navigation = RouteState::new();
        let w = wpts(&[(1_000, "Brunnen")]);
        let empty = Waypoints::new();
        let check = |cell: StatCell| {
            assert_eq!(cell.caption.as_str(), "NEXT WPT");
            assert_eq!(cell.value.as_str(), "--");
            assert_eq!(cell.value_align, TextAlign::Right, "the fallback stays right-aligned too");
        };
        // next_waypoint = None (the readout default): no route, or nothing ahead.
        check(StatField::NextWaypoint.cell(&readout(&navigation, rec, Units::Metric, &w)));
        // A stale index past the table's end.
        check(
            StatField::NextWaypoint
                .cell(&Readout { next_waypoint: Some(9), ..readout(&navigation, rec, Units::Metric, &w) }),
        );
        // An index against an empty table (no route loaded).
        check(
            StatField::NextWaypoint
                .cell(&Readout { next_waypoint: Some(0), ..readout(&navigation, rec, Units::Metric, &empty) }),
        );
    }

    #[test]
    fn next_waypoint_two_span_fills_a_row() {
        assert_eq!(StatField::NextWaypoint.span(), 2, "the waypoint tile is full-width");
        let l = list(&[StatField::NextWaypoint, StatField::Speed]);
        let p = page_fields(&l, 0);
        assert_eq!((p[0].field, p[0].col, p[0].row), (StatField::NextWaypoint, 0, 0));
        assert_eq!((p[1].field, p[1].col, p[1].row), (StatField::Speed, 0, 1));
    }

    fn cat_wpts(items: &[(u32, &str, Option<PoiCategory>)]) -> Waypoints {
        let mut w = Waypoints::new();
        for &(dist_along_m, name, category) in items {
            let mut n = heapless::String::new();
            n.push_str(name).unwrap();
            w.entries
                .push(obc_route::WptEntry { dist_along_m, lon: 0, lat: 0, category, lateral_offset_m: 0, name: n })
                .unwrap();
        }
        w
    }

    /// A cache filled through the real arm-then-harvest path, so the test cannot cache something
    /// the scheduler would not.
    fn cache(items: &[(PoiCategory, u32, &str)]) -> crate::next_ahead::NextAhead {
        use obc_reader::{Poi, PoiCategorySet};
        let mut c = crate::next_ahead::NextAhead::new();
        for &(cat, dist_along_m, name) in items {
            c.reconcile(PoiCategorySet::only(cat), true, Some(0), 0);
            let key = c.request().expect("a placed, never-taken category is always wanted");
            let mut n = heapless::String::new();
            n.push_str(name).unwrap();
            let subtype = obc_formats::obcm::POI_SUBTYPES
                .iter()
                .position(|s| s.category == cat)
                .map(|i| i as u8 + 1)
                .expect("every category has a subtype");
            c.harvest(
                key,
                &[obc_reader::CorridorPoi {
                    poi: Poi {
                        opening: Default::default(),
                        metadata: Default::default(),
                        lat: 0,
                        lon: 0,
                        subtype,
                        name: n,
                        hours_ref: 0xFFFF,
                        distance_m: dist_along_m,
                    },
                    dist_along_m,
                    offset_m: 0,
                }],
            );
        }
        c
    }

    /// A route-loaded readout: `active_route` is what makes a route-relative tile answer at all.
    fn riding<'a>(
        navigation: &'a mut RouteState,
        recorder: &'a RecorderMachine,
        waypoints: &'a Waypoints,
        next_ahead: &'a crate::next_ahead::NextAhead,
        units: Units,
        progress_m: u32,
    ) -> Readout<'a> {
        navigation.active_route = Some(0);
        navigation.progress_m = progress_m;
        Readout { next_ahead, ..readout(navigation, recorder, units, waypoints) }
    }

    /// Catalogue order and the on-disk discriminants are independent, and both are contracts.
    #[test]
    fn next_category_fields_group_after_the_next_waypoint_field() {
        let at = |f: StatField| StatField::ALL.iter().position(|g| *g == f).expect("in the catalogue");
        let six = [
            StatField::NextWater,
            StatField::NextCampsite,
            StatField::NextLodging,
            StatField::NextResupply,
            StatField::NextPharmacy,
            StatField::NextBikeShop,
        ];
        for (i, f) in six.iter().enumerate() {
            assert_eq!(at(*f), at(StatField::NextWaypoint) + 1 + i, "{f:?} follows the next-waypoint field");
            assert_eq!(f.span(), 2, "{f:?} is a full-width tile");
            assert_eq!(f.rows(), 1, "{f:?} is one row tall");
        }
        assert_eq!(six.map(|f| f.category().unwrap()).as_slice(), &PoiCategory::ALL[..6]);
        // Every other field carries no category, so nothing else can pick up the icon anatomy.
        for f in StatField::ALL {
            assert_eq!(f.category().is_some(), six.contains(&f), "{f:?} category-ness");
        }
    }

    #[test]
    fn next_category_discriminants_round_trip() {
        let expected = [
            (StatField::NextWater, 15u8),
            (StatField::NextCampsite, 16),
            (StatField::NextLodging, 17),
            (StatField::NextResupply, 18),
            (StatField::NextPharmacy, 19),
            (StatField::NextBikeShop, 20),
        ];
        for (f, b) in expected {
            assert_eq!(f as u8, b, "append-only: {f:?} is byte {b}");
            assert_eq!(StatField::from_u8(b), Some(f));
        }
        let bytes: [u8; 6] = expected.map(|(_, b)| b);
        let list = StatFieldList::decode(6, &bytes);
        assert_eq!(list.as_slice(), expected.map(|(f, _)| f), "a decoded selection keeps all six, in order");
        let (len, ids) = list.encode();
        assert_eq!(StatFieldList::decode(len, &ids).as_slice(), list.as_slice());
    }

    #[test]
    fn next_category_tiles_fill_rows_like_any_wide_tile() {
        let l = list(&[StatField::NextWater, StatField::Speed, StatField::NextPharmacy]);
        let p = page_fields(&l, 0);
        assert_eq!((p[0].field, p[0].col, p[0].row), (StatField::NextWater, 0, 0));
        assert_eq!((p[1].field, p[1].col, p[1].row), (StatField::Speed, 0, 1));
        assert_eq!(
            (p[2].field, p[2].col, p[2].row),
            (StatField::NextPharmacy, 0, 2),
            "the second wide tile bumps past the half-filled row rather than straddling it"
        );
    }

    #[test]
    fn next_category_tile_takes_the_nearest_of_either_source() {
        let w = cat_wpts(&[(2_000, "Brunnen", Some(PoiCategory::Water)), (9_000, "Camp", Some(PoiCategory::Campsite))]);
        let c = cache(&[(PoiCategory::Water, 5_000, "Fontaine"), (PoiCategory::Campsite, 4_000, "Camping Est")]);
        let mut a = RouteState::new();

        let cx = riding(&mut a, idle_recorder(), &w, &c, Units::Metric, 0);
        let cell = StatField::NextWater.cell(&cx);
        assert_eq!(cell.caption.as_str(), "Brunnen", "the rider's own waypoint is nearest");
        assert_eq!(cell.value.as_str(), "2.0km");
        assert_eq!(cell.value_align, TextAlign::Right, "the wide-tile distance hugs the far edge");
        let cell = StatField::NextCampsite.cell(&cx);
        assert_eq!(cell.caption.as_str(), "Camping Est", "the corridor POI is nearest");
        assert_eq!(cell.value.as_str(), "4.0km");

        // Ride past the water waypoint: the tile hands over to the cached POI and counts down to it.
        let cx = riding(&mut a, idle_recorder(), &w, &c, Units::Metric, 2_100);
        let cell = StatField::NextWater.cell(&cx);
        assert_eq!((cell.caption.as_str(), cell.value.as_str()), ("Fontaine", "2.9km"));

        let tie = cat_wpts(&[(5_000, "Brunnen", Some(PoiCategory::Water))]);
        let cx = riding(&mut a, idle_recorder(), &tie, &c, Units::Metric, 0);
        assert_eq!(StatField::NextWater.cell(&cx).caption.as_str(), "Brunnen", "a tie goes to the plan entry");
    }

    #[test]
    fn next_category_tile_empty_states() {
        let rec = idle_recorder();
        let w = cat_wpts(&[(2_000, "Turn left", None), (3_000, "Camp", Some(PoiCategory::Campsite))]);
        let c = cache(&[(PoiCategory::Water, 1_000, "Fontaine")]);
        let mut a = RouteState::new();

        let empty = crate::next_ahead::NextAhead::new();
        let cx = Readout { next_ahead: &c, ..readout(&a, rec, Units::Metric, &w) };
        let cell = StatField::NextWater.cell(&cx);
        assert_eq!((cell.caption.as_str(), cell.value.as_str()), ("Water", "--"), "no route ⇒ the category name + --");
        assert_eq!(cell.value_align, TextAlign::Right, "the fallback stays right-aligned too");

        // Nothing of this category anywhere (the generic waypoint doesn't answer a category question).
        let cx = riding(&mut a, idle_recorder(), &w, &empty, Units::Metric, 0);
        assert_eq!(StatField::NextPharmacy.cell(&cx).value.as_str(), "--");
        assert_eq!(StatField::NextWater.cell(&cx).value.as_str(), "--", "an empty cache is just no answer yet");
        // …while a categorized waypoint of another kind still answers its own tile.
        assert_eq!(StatField::NextCampsite.cell(&cx).caption.as_str(), "Camp");

        // A cached entry the rider has passed is dropped, not clamped to `0m` (the scheduler re-arms).
        let cx = riding(&mut a, idle_recorder(), &w, &c, Units::Metric, 1_500);
        let cell = StatField::NextWater.cell(&cx);
        assert_eq!((cell.caption.as_str(), cell.value.as_str()), ("Water", "--"), "a passed cache line reads --");
    }

    #[test]
    fn next_category_tile_follows_the_unit_system() {
        let w = Waypoints::new();
        let c = cache(&[(PoiCategory::BikeShop, 12_400, "Cycles Monaco")]);
        let mut a = RouteState::new();
        assert_eq!(
            StatField::NextBikeShop.cell(&riding(&mut a, idle_recorder(), &w, &c, Units::Metric, 0)).value.as_str(),
            "12.4km"
        );
        assert_eq!(
            StatField::NextBikeShop.cell(&riding(&mut a, idle_recorder(), &w, &c, Units::Imperial, 0)).value.as_str(),
            "7.7mi"
        );
    }

    #[test]
    fn next_category_field_names_are_the_localized_category_words() {
        use crate::screen::poi_menu::category_msg;
        for lang in [Language::En, Language::De, Language::Fr, Language::Es] {
            for f in StatField::ALL {
                let Some(cat) = f.category() else { continue };
                assert_eq!(f.name(lang), t(category_msg(cat), lang), "{f:?} in {lang:?} is the category's own word");
                assert!(!f.name(lang).is_empty());
            }
            // The words are distinct within a language, so six picker rows can't read alike.
            let names: heapless::Vec<&str, 7> = PoiCategory::ALL.iter().map(|c| t(category_msg(*c), lang)).collect();
            for (i, n) in names.iter().enumerate() {
                assert!(!names[i + 1..].contains(n), "{n:?} appears twice in {lang:?}");
            }
        }
        assert_eq!(StatField::NextLodging.name(Language::De), "Unterkunft");
        assert_eq!(StatField::NextBikeShop.name(Language::Fr), "Vélociste");
    }

    #[test]
    fn waypoint_list_is_a_page_sized_field() {
        assert_eq!(StatField::WaypointList.span(), 2, "the panel is full-width");
        assert_eq!(StatField::WaypointList.rows(), 3, "and three rows tall — the only multi-row field");
        assert_eq!(StatField::WaypointList.slots(), SLOTS_PER_PAGE, "span × rows = a whole page");
        for f in StatField::ALL {
            if f != StatField::WaypointList {
                assert_eq!(f.rows(), 1, "{f:?} is one row tall");
                assert_eq!(f.slots(), f.span() as usize, "{f:?} slots() == span()");
            }
        }
    }

    #[test]
    fn panel_always_starts_a_page() {
        // Panel first: it owns page 0; the trailing single starts page 1.
        let l = list(&[StatField::WaypointList, StatField::Speed]);
        assert_eq!(slot_of(&l, 0), Some(0), "the panel sits at slot 0");
        assert_eq!(slot_of(&l, 1), Some(SLOTS_PER_PAGE), "the single after it lands on page 1");
        assert_eq!(page_count(&l), 2);

        // Mid-list after an odd single: the single fills slot 0, the panel bumps a whole page.
        let l = list(&[StatField::Speed, StatField::WaypointList, StatField::AvgSpeed]);
        assert_eq!(slot_of(&l, 0), Some(0));
        assert_eq!(slot_of(&l, 1), Some(SLOTS_PER_PAGE), "the panel bumps off the half-filled page 0");
        assert_eq!(slot_of(&l, 2), Some(2 * SLOTS_PER_PAGE), "and the trailing single lands on page 2");
        assert_eq!(page_count(&l), 3);

        // After a wide tile, the panel still bumps to the next page boundary.
        let l = list(&[StatField::Clock, StatField::WaypointList, StatField::Speed]);
        assert_eq!(slot_of(&l, 0), Some(0), "the wide clock fills row 0 of page 0");
        assert_eq!(slot_of(&l, 1), Some(SLOTS_PER_PAGE), "the panel bumps to page 1");
        assert_eq!(slot_of(&l, 2), Some(2 * SLOTS_PER_PAGE), "the single after the panel lands on page 2");
        assert_eq!(page_count(&l), 3);
    }

    #[test]
    fn page_count_treats_the_panel_as_a_page() {
        let six = [StatField::Speed, StatField::AvgSpeed, StatField::DistDone, StatField::DistToGo, StatField::Climbed];
        let mut l = list(&[StatField::WaypointList]);
        for f in six {
            assert!(l.push(f));
        }
        assert!(l.push(StatField::ToClimb)); // panel + 6 singles
        assert_eq!(page_count(&l), 2, "the panel's page + one full page of six singles");
        assert!(l.push(StatField::Grade)); // panel + 7 singles
        assert_eq!(page_count(&l), 3, "the seventh single spills to a third page");

        // A maxed selection that carries the panel spills past two pages. The exact count follows
        // catalogue order, because that decides which fields make the cut.
        let full = {
            let mut l = StatFieldList { ids: [StatField::Speed; MAX_STAT_FIELDS], len: 0 };
            l.push(StatField::WaypointList);
            for f in StatField::ALL {
                l.push(f); // silently refused once the grid is full
            }
            l
        };
        assert_eq!(full.len(), MAX_STAT_FIELDS, "the grid fills to its MAX_STAT_FIELDS cap");
        assert!(full.contains(StatField::WaypointList), "the maxed selection includes the panel");
        assert_eq!(page_count(&full), 3, "a full selection with the panel spans three pages, not two");
    }

    #[test]
    fn move_item_panel_hops_whole_pages() {
        let mut l = list(&[
            StatField::WaypointList,
            StatField::Speed,
            StatField::AvgSpeed,
            StatField::DistDone,
            StatField::DistToGo,
            StatField::Climbed,
            StatField::ToClimb,
        ]);
        let ni = l.move_item(0, 1);
        assert_eq!(ni, 6, "the panel lands after the whole page of six singles");
        assert_eq!(l.as_slice()[6], StatField::WaypointList);
        assert_eq!(slot_of(&l, 6), Some(SLOTS_PER_PAGE), "and its slot is a page boundary");
        let ni = l.move_item(6, -1);
        assert_eq!(ni, 0, "and back up a whole page in one step");
        assert_eq!(l.as_slice()[0], StatField::WaypointList);
    }

    #[test]
    fn move_item_panel_is_a_noop_at_the_ends() {
        let mut l = list(&[StatField::WaypointList, StatField::Speed, StatField::AvgSpeed]);
        assert_eq!(l.move_item(0, -1), 0, "a leading panel can't move up");
        assert_eq!(l.as_slice()[0], StatField::WaypointList);
        let mut l = list(&[StatField::Speed, StatField::AvgSpeed, StatField::WaypointList]);
        assert_eq!(l.move_item(2, 1), 2, "a trailing panel can't move down");
        assert_eq!(l.as_slice()[2], StatField::WaypointList);
    }

    #[test]
    fn move_item_single_hops_the_whole_panel() {
        let mut l = list(&[StatField::Speed, StatField::WaypointList, StatField::AvgSpeed]);
        let ni = l.move_item(0, 1);
        assert_eq!(ni, 1, "Speed lands right after the panel");
        assert_eq!(l.as_slice(), &[StatField::WaypointList, StatField::Speed, StatField::AvgSpeed]);
        assert_eq!(slot_of(&l, 1), Some(SLOTS_PER_PAGE));
    }

    #[test]
    fn move_item_wide_stays_row_aligned_around_the_panel() {
        let mut l = list(&[StatField::Speed, StatField::WaypointList, StatField::Clock]);
        let ni = l.move_item(2, -1);
        assert_eq!(ni, 0, "the wide tile skips the odd slot-1 landing and lands row-aligned at slot 0");
        assert_eq!(l.as_slice(), &[StatField::Clock, StatField::Speed, StatField::WaypointList]);
        for placed in
            (0..page_count(&l)).flat_map(|p| page_fields(&l, p).into_iter()).filter(|p| p.field == StatField::Clock)
        {
            assert_eq!(placed.col, 0, "the wide tile always begins a row, even around the panel");
        }
    }

    #[test]
    fn slot_queries_agree_with_page_fields_around_the_panel() {
        let l = list(&[StatField::Speed, StatField::AvgSpeed, StatField::WaypointList]);
        assert_eq!(slot_of(&l, 0), Some(0));
        assert_eq!(slot_of(&l, 1), Some(1));
        assert_eq!(slot_of(&l, 2), Some(SLOTS_PER_PAGE), "the panel starts page 1");
        assert_eq!(slot_of(&l, 3), None, "past the selection there is no slot");
        assert_eq!(next_free_slot(&l), 2 * SLOTS_PER_PAGE);
        assert_eq!(next_free_slot(&l) / SLOTS_PER_PAGE, 2, "the ghost Add lands on the page after the panel");
        let p1 = page_fields(&l, 1);
        assert_eq!(p1.len(), 1, "the panel owns its page");
        assert_eq!((p1[0].field, p1[0].col, p1[0].row), (StatField::WaypointList, 0, 0));
        for (i, &f) in l.as_slice().iter().enumerate() {
            let slot = slot_of(&l, i).unwrap();
            let placed = page_fields(&l, slot / SLOTS_PER_PAGE).into_iter().find(|p| p.field == f).unwrap();
            let s = slot % SLOTS_PER_PAGE;
            assert_eq!((placed.col as usize, placed.row as usize), (s % COLS, s / COLS), "{f:?} slot vs placement");
        }
    }

    #[test]
    fn waypoint_list_discriminant_round_trips() {
        assert_eq!(StatField::WaypointList as u8, 11, "append-only: the panel is byte 11");
        assert_eq!(StatField::from_u8(11), Some(StatField::WaypointList));
        let list = StatFieldList::decode(1, &[11]);
        assert_eq!(list.as_slice(), &[StatField::WaypointList], "a decoded byte-11 selection keeps the panel");
    }

    #[test]
    fn sensor_tiles_are_single_column_captioned() {
        let rec = idle_recorder();
        let navigation = RouteState::new();
        let empty = Waypoints::new();
        let cx = readout(&navigation, rec, Units::Metric, &empty);
        for f in [StatField::HeartRate, StatField::Power, StatField::Cadence] {
            assert_eq!(f.span(), 1, "{f:?} is a single column");
            assert_eq!(f.rows(), 1, "{f:?} is one row tall");
            assert_eq!(f.slots(), 1, "{f:?} fills one slot");
            assert!(!f.cell(&cx).arrow, "{f:?} has no climb triangle");
        }
        assert_eq!(StatField::HeartRate.cell(&cx).caption.as_str(), "HR");
        assert_eq!(StatField::Power.cell(&cx).caption.as_str(), "PWR");
        assert_eq!(StatField::Cadence.cell(&cx).caption.as_str(), "RPM");
    }

    #[test]
    fn sensor_tiles_format_raw_ints_and_dash_when_stale() {
        let mut rec = RecorderMachine::new();
        let navigation = RouteState::new();
        let empty = Waypoints::new();
        // The render clock below is deliberately unrelated to the sensor clock: the tiles must
        // ignore it and gate on `note_sensor_clock` instead.
        let val = |rec: &RecorderMachine, f: StatField| {
            f.cell(&Readout { now_ms: 999_999, ..readout(&navigation, rec, Units::Metric, &empty) }).value
        };

        rec.note_sensor_clock(0);
        assert_eq!(val(&rec, StatField::HeartRate).as_str(), "--", "no HR sensor → --");
        assert_eq!(val(&rec, StatField::Power).as_str(), "--", "no power meter → --");
        assert_eq!(val(&rec, StatField::Cadence).as_str(), "--", "no cadence sensor → --");

        // Fresh samples → the raw numbers, no glued unit.
        rec.record_hr(152, 1_000);
        rec.record_power(210, 1_000);
        rec.record_cadence(88, 1_000);
        rec.note_sensor_clock(1_000);
        assert_eq!(val(&rec, StatField::HeartRate).as_str(), "152");
        assert_eq!(val(&rec, StatField::Power).as_str(), "210");
        assert_eq!(val(&rec, StatField::Cadence).as_str(), "88");

        rec.note_sensor_clock(6_001);
        assert_eq!(val(&rec, StatField::HeartRate).as_str(), "--", "HR older than 5 s reads --");
        assert_eq!(val(&rec, StatField::Power).as_str(), "--", "power older than 5 s reads --");
        assert_eq!(val(&rec, StatField::Cadence).as_str(), "--", "cadence older than 5 s reads --");

        rec.record_cadence(0, 7_000);
        rec.note_sensor_clock(7_000);
        assert_eq!(val(&rec, StatField::Cadence).as_str(), "0", "a fresh coasting 0 shows 0, not --");
    }

    #[test]
    fn sensor_tile_discriminants_round_trip() {
        assert_eq!(StatField::HeartRate as u8, 12, "append-only: HR is byte 12");
        assert_eq!(StatField::Power as u8, 13, "append-only: power is byte 13");
        assert_eq!(StatField::Cadence as u8, 14, "append-only: cadence is byte 14");
        assert_eq!(StatField::from_u8(12), Some(StatField::HeartRate));
        assert_eq!(StatField::from_u8(13), Some(StatField::Power));
        assert_eq!(StatField::from_u8(14), Some(StatField::Cadence));
        let list = StatFieldList::decode(3, &[12, 13, 14]);
        assert_eq!(list.as_slice(), &[StatField::HeartRate, StatField::Power, StatField::Cadence]);
    }
}
