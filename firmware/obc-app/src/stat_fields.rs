//! The Statistics grid's **data fields** — the predefined catalogue the rider picks from, the
//! ordered selection they build, and the grid-layout maths that places it.
//!
//! Below the riding [`Statistics`](crate::screen) view's elevation chart is a customizable grid of
//! tiles. Each shows one [`StatField`] — a value with a caption, single column or a `2`-span
//! full-width tile. The rider chooses which fields and in what order; the choice lives in
//! [`Settings::stat_fields`](crate::Settings) and persists.
//!
//! Three concerns, all `no_std` / zero-alloc / pure:
//! - **the catalogue** — [`StatField`], one variant per field, owning its span, name and value
//!   formatter. Adding a field is one variant + one match arm.
//! - **the selection** — [`StatFieldList`], a fixed-capacity ordered list persisted in [`Settings`].
//! - **the layout** — [`page_count`] / [`page_fields`], walking the selection into 6-slot pages
//!   (3 rows × 2 cols) by each field's [`slots`](StatField::slots) footprint, keeping a `2`-span tile
//!   row-aligned and the page-sized waypoint panel page-aligned so neither straddles a row or page.

include!("stat_fields/selection.rs");

use core::fmt::Write;

use obc_reader::PoiCategory;
use obc_render::text::TextAlign;
use obc_route::{Profile, RouteReader, Waypoints};

use crate::activity::Activity;
use crate::i18n::{t, Msg};
use crate::settings::{DateTime, Language, Units};
use obc_ports::Fix;

/// The narrow live-data view a stat field formats from — exactly what [`StatField::cell`] reads,
/// nothing more. Deliberately decoupled from the full draw context
/// ([`Render`](crate::screen::Render), which drags in the borrowed `RenderScratch`): a cell is pure
/// data-to-string, so a test — or a future non-draw host readout — builds a bare `Readout` instead
/// of faking a render context. Constructed from a frame by [`Render::readout`](crate::screen::Render).
pub struct Readout<'a> {
    /// The current GPS fix, `None` when there isn't one (acquiring / lost).
    pub fix: Option<Fix>,
    /// The ride accumulators (distance, climb, moving time, live altitude…).
    pub activity: &'a Activity,
    /// The active unit system — captions and scales every readout.
    pub units: Units,
    /// The active route's geometry totals, or `None` when no route is loaded.
    pub route: Option<&'a RouteReader<'a>>,
    /// The active route's elevation profile, or `None` when no route is loaded.
    pub profile: Option<&'a Profile>,
    /// The climb the rider is currently on (C3), or `None` between climbs — the source for the
    /// climb-scoped tiles (to-top / to-climb / grade) the Climb screen adds in C4. `Some` exactly
    /// when [`Activity::active_climb`](crate::activity::Activity) is `Some`.
    pub climb: Option<crate::screen::ActiveClimb<'a>>,
    /// The active route's resident named-waypoint table (App-owned), in route order — the
    /// [`NextWaypoint`](StatField::NextWaypoint) tile reads its name + along-route position. Empty
    /// when no route is loaded, so the tile falls back to its `NEXT WPT` / `--` empty state.
    pub waypoints: &'a Waypoints,
    /// The resolved index into [`waypoints`](Self::waypoints) of the next waypoint ahead, or `None`
    /// when there's no route / nothing ahead. Mirrors
    /// [`Activity::next_waypoint`](crate::activity::Activity) but is kept explicit so a test — or a
    /// future non-`App` host — can build a `Readout` without the ride loop that resolves it.
    pub next_waypoint: Option<usize>,
    /// The live wall-clock time (the [`Clock`](StatField::Clock) tile).
    pub now: DateTime,
    /// The boot-relative millis this frame (the [`RideClock`](obc_ports::RideClock) already threaded
    /// through the ride loop) — the staleness clock the live sensor tiles
    /// ([`HeartRate`](StatField::HeartRate) / [`Power`](StatField::Power) /
    /// [`Cadence`](StatField::Cadence)) pass to [`Activity`](crate::activity::Activity)'s 5 s-gated
    /// `live_*` accessors, so a dropped sensor reads `--` rather than its frozen last value.
    pub now_ms: u32,
    /// The rider's selected bike profile ([`Settings::bike_profile_idx`](crate::Settings)) — the row
    /// the EL9 time model (#1077) reads its `v_flat` / `k_climb` from, so the ETA tiles answer for
    /// the bike the router planned under. Out-of-range indices fall back to profile 0 inside
    /// [`obc_route::eta`], the same rule the router and the Bike-type label use.
    pub bike_profile_idx: u8,
    /// The UI language (epic #602) — the word-bearing tile captions (`AVG`, `CLIMBED`, `TO GO`…)
    /// route through the catalog; the unit symbols glued to the value stay language-independent.
    pub language: Language,
    /// The App-owned per-category "next ahead" cache (epic #946, U5) — the map-POI half of the six
    /// `Next: <category>` tiles. Read-only here, and *only* read by those tiles: everything else in
    /// this catalogue is resident data. Empty on a host that never refreshes it, which simply makes
    /// the tiles waypoint-only.
    pub next_ahead: &'a crate::next_ahead::NextAhead,
}

/// Grid geometry: a page is `ROWS_PER_PAGE × COLS` tiles. A single-span field fills one slot, a
/// two-span field a whole row (both columns). When the selection needs more than [`SLOTS_PER_PAGE`]
/// slots the grid paginates and auto-cycles (see the Statistics screen).
pub const COLS: usize = 2;
pub const ROWS_PER_PAGE: usize = 3;
pub const SLOTS_PER_PAGE: usize = ROWS_PER_PAGE * COLS;

/// Max fields the rider can pin to the grid — two full pages. Sizes the [`StatFieldList`] array
/// and bounds the persisted blob.
pub const MAX_STAT_FIELDS: usize = 2 * SLOTS_PER_PAGE;

/// One predefined data field. `#[repr(u8)]` + `Copy + Eq` so the selection is a trivially packed,
/// comparable POD (the settings codec writes the discriminants; [`Settings`](crate::Settings) stays
/// `Copy + Eq` for the one-`==` save check). The discriminants are a **stable on-disk contract** —
/// only ever append, never renumber.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum StatField {
    /// Live GPS speed.
    Speed = 0,
    /// Moving-average speed.
    AvgSpeed = 1,
    /// Distance ridden so far.
    DistDone = 2,
    /// Distance remaining along the route.
    DistToGo = 3,
    /// Ascent climbed so far.
    Climbed = 4,
    /// Ascent remaining along the route.
    ToClimb = 5,
    /// Grade (%) at the live position.
    Grade = 6,
    /// Current elevation — the live altitude: map-referenced (EL8) once the offset estimator has
    /// settled, raw barometric until then.
    Elevation = 7,
    /// Moving time this ride.
    RideTime = 8,
    /// Wall-clock time of day — a **two-column** tile.
    Clock = 9,
    /// Next route waypoint ahead — name + distance-to-go, a **two-column** tile.
    NextWaypoint = 10,
    /// The next ~4 route waypoints ahead — a **2-column × 3-row** list panel (name + along-route
    /// distance-to-go per row, the first emphasized). The one multi-row field: page-sized
    /// ([`SLOTS_PER_PAGE`] slots), so it always begins a page — mirroring how a two-span tile always
    /// begins a row — which keeps the layout + reorder machinery tractable.
    WaypointList = 11,
    /// Live heart rate (bpm) from a paired BLE sensor — a single-column raw-int tile (epic #707).
    /// `--` with no sensor / no data / a reading older than the 5 s staleness gate.
    HeartRate = 12,
    /// Live power (W) from a paired BLE power meter — a single-column raw-int tile (epic #707).
    /// `--` with no sensor / no data / stale.
    Power = 13,
    /// Live cadence (rpm) from a paired BLE sensor (or a power meter's crank data) — a
    /// single-column raw-int tile (epic #707). `--` with no sensor / no data / stale; a fresh `0`
    /// is a coasting rider and shows `0`, not `--`.
    Cadence = 14,
    /// Next **water** on the route ahead — a **two-column** tile (epic #946, U5). See
    /// [`category`](StatField::category) for the shared anatomy of the six.
    NextWater = 15,
    /// Next **campsite** on the route ahead — a two-column tile.
    NextCampsite = 16,
    /// Next **lodging** on the route ahead — a two-column tile.
    NextLodging = 17,
    /// Next **resupply** on the route ahead — a two-column tile.
    NextResupply = 18,
    /// Next **pharmacy** on the route ahead — a two-column tile.
    NextPharmacy = 19,
    /// Next **bike shop** on the route ahead — a two-column tile.
    NextBikeShop = 20,
    /// Estimated **time still to ride** to the end of the route — the gradient-aware model
    /// (elevation epic #1068, EL9), not distance ÷ average speed. `--` on a route-less ride.
    TimeToGo = 21,
    /// Estimated **arrival clock time** at the end of the route — [`TimeToGo`](StatField::TimeToGo)
    /// added to the wall clock. `--` on a route-less ride.
    Eta = 22,
}

impl StatField {
    /// Every field, in catalogue order — drives the "Add field" picker and decode validation.
    ///
    /// Catalogue order is a **UI** decision and deliberately independent of the on-disk
    /// discriminants (which are append-only): the six `Next: <category>` tiles are numbered last but
    /// listed **directly after** [`NextWaypoint`](StatField::NextWaypoint), because that is where a
    /// rider looking for "what's coming up" will look for them (epic #946, U5 — grouping in the
    /// picker is the *only* curation knob the epic allows). The same reasoning puts
    /// [`TimeToGo`](StatField::TimeToGo) and [`Eta`](StatField::Eta) (EL9, #1077) between
    /// [`RideTime`](StatField::RideTime) and [`Clock`](StatField::Clock) — the clock family, read
    /// together — rather than at the end where their discriminants sit.
    pub const ALL: [StatField; 23] = [
        StatField::Speed,
        StatField::AvgSpeed,
        StatField::DistDone,
        StatField::DistToGo,
        StatField::Climbed,
        StatField::ToClimb,
        StatField::Grade,
        StatField::Elevation,
        StatField::RideTime,
        StatField::TimeToGo,
        StatField::Eta,
        StatField::Clock,
        StatField::NextWaypoint,
        StatField::NextWater,
        StatField::NextCampsite,
        StatField::NextLodging,
        StatField::NextResupply,
        StatField::NextPharmacy,
        StatField::NextBikeShop,
        StatField::WaypointList,
        StatField::HeartRate,
        StatField::Power,
        StatField::Cadence,
    ];

    /// The POI category a `Next: <category>` tile tracks, or `None` for every other field. The one
    /// switch the whole feature hangs off: it selects the tile drawer (icon + name | distance), the
    /// picker row's icon, the field's name, and which categories the
    /// [`NextAhead`](crate::next_ahead::NextAhead) cache keeps warm.
    pub const fn category(self) -> Option<PoiCategory> {
        Some(match self {
            StatField::NextWater => PoiCategory::Water,
            StatField::NextCampsite => PoiCategory::Campsite,
            StatField::NextLodging => PoiCategory::Accommodation,
            StatField::NextResupply => PoiCategory::Resupply,
            StatField::NextPharmacy => PoiCategory::Pharmacy,
            StatField::NextBikeShop => PoiCategory::BikeShop,
            _ => return None,
        })
    }

    /// Decode a persisted discriminant, or `None` for an unknown byte (a newer writer, a bit-flip
    /// the CRC missed) — the codec drops it rather than trusting a garbage field.
    pub fn from_u8(b: u8) -> Option<StatField> {
        Self::ALL.into_iter().find(|f| *f as u8 == b)
    }

    /// Column span: `2` for the full-width [`Clock`](StatField::Clock),
    /// [`NextWaypoint`](StatField::NextWaypoint), the six `Next: <category>` tiles (same
    /// icon + name | distance anatomy), and the [`WaypointList`](StatField::WaypointList) panel,
    /// else `1`.
    pub const fn span(self) -> u8 {
        match self {
            StatField::Clock
            | StatField::NextWaypoint
            | StatField::WaypointList
            | StatField::NextWater
            | StatField::NextCampsite
            | StatField::NextLodging
            | StatField::NextResupply
            | StatField::NextPharmacy
            | StatField::NextBikeShop => 2,
            _ => 1,
        }
    }

    /// Row span: `3` for the multi-row [`WaypointList`](StatField::WaypointList) panel, `1` for every
    /// other field (all today's tiles are one row tall). With [`span`](Self::span) it derives the
    /// field's slot footprint, [`slots`](Self::slots).
    pub const fn rows(self) -> u8 {
        match self {
            StatField::WaypointList => 3,
            _ => 1,
        }
    }

    /// The field's grid footprint in slots — [`span`](Self::span) × [`rows`](Self::rows): `1` for a
    /// single tile, `2` for a full-width tile, [`SLOTS_PER_PAGE`] (`6`) for the page-sized waypoint
    /// panel. The one measure the layout [`walk`] advances by, so the three tile shapes flow through
    /// it uniformly.
    pub const fn slots(self) -> usize {
        self.span() as usize * self.rows() as usize
    }

    /// The field's name for the settings list / picker, in the UI `lang` (epic #602). The on-grid
    /// caption is in [`cell`](StatField::cell).
    ///
    /// A `Next: <category>` field names itself with the **category's** own catalog string — the very
    /// one the Up-ahead picker and the POI menu use (epic #946 reuses one icon *and* one word per
    /// category; a parallel `statfield.next_water` set would be the same six words drifting in four
    /// languages). What makes the row read as a field rather than a place is the category icon the
    /// picker draws beside it, and the tile preview in the editor.
    pub const fn name(self, lang: Language) -> &'static str {
        match self {
            StatField::Speed => t(Msg::StatfieldSpeed, lang),
            StatField::AvgSpeed => t(Msg::StatfieldAvgSpeed, lang),
            StatField::DistDone => t(Msg::StatfieldDistDone, lang),
            StatField::DistToGo => t(Msg::StatfieldDistToGo, lang),
            StatField::Climbed => t(Msg::StatfieldClimbed, lang),
            StatField::ToClimb => t(Msg::StatfieldToClimb, lang),
            StatField::Grade => t(Msg::StatfieldGrade, lang),
            StatField::Elevation => t(Msg::StatfieldElevation, lang),
            StatField::RideTime => t(Msg::StatfieldRideTime, lang),
            StatField::TimeToGo => t(Msg::StatfieldTimeToGo, lang),
            StatField::Eta => t(Msg::StatfieldEta, lang),
            StatField::Clock => t(Msg::StatfieldClock, lang),
            StatField::NextWaypoint => t(Msg::StatfieldNextWaypoint, lang),
            StatField::WaypointList => t(Msg::StatfieldWaypointList, lang),
            StatField::HeartRate => t(Msg::StatfieldHeartRate, lang),
            StatField::Power => t(Msg::StatfieldPower, lang),
            StatField::Cadence => t(Msg::StatfieldCadence, lang),
            StatField::NextWater => t(Msg::PoiCatWater, lang),
            StatField::NextCampsite => t(Msg::PoiCatCampsite, lang),
            StatField::NextLodging => t(Msg::PoiCatAccommodation, lang),
            StatField::NextResupply => t(Msg::PoiCatResupply, lang),
            StatField::NextPharmacy => t(Msg::PoiCatPharmacy, lang),
            StatField::NextBikeShop => t(Msg::PoiCatBikeShop, lang),
        }
    }

    /// The rendered tile content: a unit-bearing caption, the number-only value, and whether to
    /// prefix an up-triangle (the climb fields). Route-relative fields fall back to `--` with no
    /// route loaded. The unit lives in the caption so the big [`Display`](obc_render::text::Font)
    /// digits fit the half-width tile.
    pub fn cell(&self, cx: &Readout) -> StatCell {
        let units = cx.units;
        let lang = cx.language;
        let live = live_frac(cx.activity);
        match self {
            StatField::Speed => {
                let v = cx.fix.and_then(|f| f.speed_mps).map(|mps| units.speed(mps * 3.6));
                StatCell::new(cap(units.speed_label(), ""), fmt_speed(v), false)
            }
            StatField::AvgSpeed => {
                let v = cx.activity.avg_kmh().map(|kmh| units.speed(kmh));
                StatCell::new(cap(t(Msg::TileAvg, lang), units.speed_label()), fmt_speed(v), false)
            }
            StatField::DistDone => StatCell::new(
                cap(units.dist_label(), t(Msg::TileDone, lang)),
                fmt_km(units.dist(cx.activity.ridden_m / 1000.0)),
                false,
            ),
            StatField::DistToGo => {
                // Route-relative: with no route loaded (a route-less ride) there's nothing "to go",
                // so the tile reads `--` rather than a misleading 0.0.
                let value = match cx.route {
                    Some(r) => {
                        fmt_km(units.dist(r.total_distance_m.saturating_sub(cx.activity.progress_m) as f32 / 1000.0))
                    }
                    None => dashes(),
                };
                StatCell::new(cap(units.dist_label(), t(Msg::TileToGo, lang)), value, false)
            }
            StatField::Climbed => StatCell::new(
                cap(t(Msg::TileClimbed, lang), ""),
                fmt_int(units.elev(cx.activity.climb_m()) as u32),
                true,
            ),
            StatField::ToClimb => {
                // Route-relative — `--` on a route-less ride (no cumulative ascent to subtract from).
                // The remaining ascent is the profile's own climb-between-two-points lookup over
                // [progress, end] — the one the Up-ahead rows and the EL9 time model also read, so
                // TO CLIMB and TIME TO GO can never disagree about the climbing that's left.
                let value = match (cx.route, cx.profile) {
                    (Some(r), Some(p)) => fmt_int(units.elev(ascent_to_go_m(r, p, cx.activity) as f32) as u32),
                    _ => dashes(),
                };
                StatCell::new(cap(t(Msg::TileToClimb, lang), ""), value, true)
            }
            StatField::Grade => {
                // Route-relative — `--` on a route-less ride (grade comes from the route profile).
                let value = match (cx.route, cx.profile) {
                    (Some(r), Some(p)) => {
                        let mut s: heapless::String<8> = heapless::String::new();
                        let _ = write!(s, "{}%", grade_at(p, r.total_distance_m, live));
                        s
                    }
                    _ => dashes(),
                };
                StatCell::new(cap(t(Msg::TileGrade, lang), ""), value, false)
            }
            StatField::Elevation => {
                // The live altitude, not the route profile — so it reads the current height with no
                // route loaded, and `--` until the first sample. Map-referenced once the EL8
                // estimator has settled (`Activity::current_elevation_m`), raw barometric before
                // that and on a terrain-less map: same tile, same presentation, no fake precision.
                let v = cx.activity.current_elevation_m().map(|m| units.elev(m));
                StatCell::new(cap(t(Msg::TileElev, lang), units.elev_label()), fmt_elev(v), false)
            }
            StatField::RideTime => StatCell::new(cap(t(Msg::TileRide, lang), ""), fmt_hms(cx.activity.moving_s), false),
            // The two EL9 time tiles (#1077). Both read one number — the gradient-aware seconds
            // still to ride — and differ only in how they present it: TIME TO GO as a duration,
            // ETA as the clock time it lands on. Route-relative, so `--` on a route-less ride
            // (like DistToGo / ToClimb): with no route there is no end to arrive at, and a
            // distance-only guess is exactly the wrong answer this field exists to replace.
            StatField::TimeToGo => {
                let value = match time_to_go_s(cx) {
                    Some(s) => fmt_hms(s as f32),
                    None => dashes(),
                };
                // The unit rides in the caption like every other tile ("h TO GO", the twin of
                // DistToGo's "km TO GO") — a single-column tile fits 8 Label glyphs, so a
                // spelled-out "TIME TO GO" would only be ellipsised back to this.
                StatCell::new(cap("h", t(Msg::TileToGo, lang)), value, false)
            }
            StatField::Eta => {
                // Wall clock + the estimate, rounded to the nearest minute — an arrival time is
                // read to the minute, and truncating would make a 59-second remainder vanish.
                let value = match time_to_go_s(cx) {
                    Some(s) => {
                        let at = cx.now.add_minutes((s + 30) / 60);
                        let mut v: heapless::String<8> = heapless::String::new();
                        let _ = write!(v, "{:02}:{:02}", at.hour, at.minute);
                        v
                    }
                    None => dashes(),
                };
                StatCell::new(cap(t(Msg::TileEta, lang), ""), value, false)
            }
            StatField::Clock => {
                let mut value: heapless::String<8> = heapless::String::new();
                let _ = write!(value, "{:02}:{:02}", cx.now.hour, cx.now.minute);
                StatCell::new(cap(t(Msg::TileTime, lang), ""), value, false)
            }
            StatField::NextWaypoint => {
                // The next named waypoint ahead (App-resolved index into the resident table): its
                // name is the caption, its along-route distance-to-go the value — `dist_along_m −
                // progress`, the same arithmetic as the Map chip, clamping to `0m` through the 100 m
                // pass-linger. With no route / nothing ahead / a stale index the tile reads
                // `NEXT WPT` / `--`, the route-relative fallback (like `DistToGo`). The value is
                // right-aligned to sit at the wide tile's far edge, per the field's mockup; the
                // caption (a name up to `WAYPOINT_NAME_CAP`) is ellipsis-truncated by the tile drawer.
                let mut cell = match cx.next_waypoint.and_then(|k| cx.waypoints.as_slice().get(k)) {
                    Some(wp) => {
                        let mut caption: heapless::String<24> = heapless::String::new();
                        let _ = caption.push_str(wp.name.as_str());
                        let value = fmt_dist_short(wp.dist_along_m.saturating_sub(cx.activity.progress_m), units);
                        StatCell::new(caption, value, false)
                    }
                    None => StatCell::new(cap(t(Msg::TileNextWpt, lang), ""), dashes(), false),
                };
                cell.value_align = TextAlign::Right;
                cell
            }
            StatField::WaypointList => {
                // The panel is drawn by the dedicated `waypoint_panel` (its 2×3 list doesn't fit the
                // caption+value shape a `StatCell` carries — the Statistics grid + Fields editor
                // special-case `rows() > 1` and call that drawer instead). This arm exists only so
                // `cell` stays total and no path can panic: a caption + `--`, echoing the empty state.
                StatCell::new(cap(t(Msg::TileWaypoints, lang), ""), dashes(), false)
            }
            StatField::HeartRate => {
                // Live bpm from the paired HR sensor, staleness-gated by `Activity` (SE2): `--` with
                // no sensor, no reading, or a sample older than the 5 s gate. bpm is the implied
                // unit (like `CLIMBED`'s metres) — no unit glued to the value. `_display` judges
                // freshness on the ride clock the sample recorded on, not this render's clock (they
                // differ in the sim during a GPX replay), so the tile doesn't spuriously blank.
                let v = cx.activity.live_hr_display().map(|bpm| bpm as u32);
                StatCell::new(cap(t(Msg::TileHr, lang), ""), fmt_int_opt(v), false)
            }
            StatField::Power => {
                // Live watts from the paired power meter, same 5 s staleness gate → `--`.
                let v = cx.activity.live_power_display().map(|w| w as u32);
                StatCell::new(cap(t(Msg::TilePwr, lang), ""), fmt_int_opt(v), false)
            }
            StatField::Cadence => {
                // Live rpm, same gate. A fresh `0` (coasting) is a real reading and shows `0`; only
                // an absent/stale value reads `--`.
                let v = cx.activity.live_cadence_display().map(|rpm| rpm as u32);
                StatCell::new(cap(t(Msg::TileRpm, lang), ""), fmt_int_opt(v), false)
            }
            // The six `Next: <category>` tiles (epic #946, U5) share one arm — the field's own
            // `category()` is the only thing that differs. Anatomy = the next-waypoint tile's, plus
            // the category icon the drawer puts left of the caption. Spelled out rather than a
            // wildcard so the match stays exhaustive and a future field still fails to compile
            // until it has a cell.
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
                        let value = fmt_dist_short(dist_along_m - cx.activity.progress_m, units);
                        StatCell::new(caption, value, false)
                    }
                    // Nothing of this kind ahead (no route, nothing cached yet, or a genuinely
                    // empty corridor): the icon still says *what*, so the caption falls back to the
                    // category's name and the value to the house `--`.
                    None => StatCell::new(cap(self.name(lang), ""), dashes(), false),
                };
                cell.value_align = TextAlign::Right;
                cell
            }
        }
    }
}

/// The nearest thing of `cat` **ahead** on the route, across both of the epic's sources: the
/// resident categorized waypoint table (U1) and the cached corridor-POI answer (U2 via the U5
/// [`NextAhead`](crate::next_ahead::NextAhead) cache). Returns its along-route position and its
/// name, or `None` when nothing of that kind is ahead.
///
/// Two rules, both inherited from the Up-ahead timeline so one entry can never read differently in
/// the list and in a tile:
///
/// * **ahead** means `dist_along_m >= progress` (the same boundary [`figures`] calls "passed") —
///   applied to the *cached* POI too, so a cache line the rider has ridden past is dropped here and
///   re-armed by the scheduler rather than shown as `0m`;
/// * a **tie** goes to the rider's own waypoint, exactly like [`Merge`]'s tie-break.
///
/// The waypoint half is a walk over resident RAM (the table is route-ordered, so the first match is
/// the nearest) — no cache, no I/O, correct on the very first frame. Only the map-POI half is
/// cached, because only it costs a card read.
///
/// [`figures`]: crate::screen::up_ahead
/// [`Merge`]: crate::screen::up_ahead
fn next_of_category<'a>(cat: PoiCategory, cx: &'a Readout<'a>) -> Option<(u32, &'a str)> {
    // Route-relative, like every other route field: with no route loaded there is no "ahead". The
    // guard is the *active route*, not the frame's `route` reader — the same fact the Up-ahead list
    // gates on (a route-less ride must never leak the previous route's resident table or a cache
    // line taken against it).
    cx.activity.active_route?;
    let progress = cx.activity.progress_m;
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

/// The rendered content of one tile — caption (unit-bearing, or a waypoint name), number-only
/// value, the climb up-triangle flag, and the value's horizontal alignment. Drawn by the Statistics
/// screen's `tile`. The caption is `String<24>` (not the built-in fields' short unit captions) so a
/// waypoint name fits; the tile drawer ellipsis-truncates one that overflows the tile width.
pub struct StatCell {
    pub caption: heapless::String<24>,
    pub value: heapless::String<8>,
    pub arrow: bool,
    /// Where the value sits in the tile: [`Left`](TextAlign::Left) for the number-only built-in
    /// fields, [`Right`](TextAlign::Right) for the wide [`NextWaypoint`](StatField::NextWaypoint)
    /// distance (hugging the far edge, clear of the name caption).
    pub value_align: TextAlign,
}

impl StatCell {
    fn new(caption: heapless::String<24>, value: heapless::String<8>, arrow: bool) -> Self {
        StatCell { caption, value, arrow, value_align: TextAlign::Left }
    }
}

// shared with the Statistics header readout.

/// A km figure for a tile: one decimal up to 100 km, none past it, so the value stays ≤ 3 digits
/// and fits the half-width tile.
pub(crate) fn fmt_km(km: f32) -> heapless::String<8> {
    let mut s = heapless::String::new();
    let _ = if km >= 100.0 { write!(s, "{km:.0}") } else { write!(s, "{km:.1}") };
    s
}

/// A compact **whole-distance** readout in the units system — the shared string for the Map
/// waypoint chip's distance-to-go and (later in epic #523) the waypoint stat fields. Metric: `NNNm`
/// below 1 km, `N.Nkm` to one decimal below 100 km, whole `NNNkm` above. Imperial: `NNNft` below
/// 1000 ft, `N.Nmi` to one decimal below 100 mi, whole `NNNmi` above. Rounds to the readout's own
/// grain (nearest tenth / whole), the same integer style as [`write_off_route`](crate::screen)'s
/// warning-chip readout — which stays as-is, being a different (feet-to-a-full-mile) format.
pub(crate) fn fmt_dist_short(d_m: u32, units: Units) -> heapless::String<8> {
    use crate::settings::{FT_PER_M, FT_PER_MI};
    let mut s = heapless::String::new();
    if units.is_imperial() {
        let ft = (d_m as f32 * FT_PER_M) as u32;
        if ft < 1000 {
            let _ = write!(s, "{ft}ft");
        } else if ft < 100 * FT_PER_MI {
            // One decimal mile, rounded to the nearest tenth.
            let tenths = (ft * 10 + FT_PER_MI / 2) / FT_PER_MI;
            let _ = write!(s, "{}.{}mi", tenths / 10, tenths % 10);
        } else {
            let _ = write!(s, "{}mi", (ft + FT_PER_MI / 2) / FT_PER_MI);
        }
    } else if d_m < 1000 {
        let _ = write!(s, "{d_m}m");
    } else if d_m < 100_000 {
        // One decimal km, rounded to the nearest tenth (100 m).
        let tenths = (d_m + 50) / 100;
        let _ = write!(s, "{}.{}km", tenths / 10, tenths % 10);
    } else {
        let _ = write!(s, "{}km", (d_m + 500) / 1000);
    }
    s
}

/// A speed to one decimal, or `--` when unknown (no fix / no moving time yet).
fn fmt_speed(v: Option<f32>) -> heapless::String<8> {
    let mut s = heapless::String::new();
    match v {
        Some(v) => {
            let _ = write!(s, "{v:.1}");
        }
        None => {
            let _ = s.push_str("--");
        }
    }
    s
}

/// The `--` placeholder a route-relative tile shows when no route is loaded (a route-less ride) —
/// the same "no data" glyph the live speed/elevation tiles use for an absent reading.
fn dashes() -> heapless::String<8> {
    let mut s = heapless::String::new();
    let _ = s.push_str("--");
    s
}

/// An integer figure (climb) as plain digits.
fn fmt_int(m: u32) -> heapless::String<8> {
    let mut s = heapless::String::new();
    let _ = write!(s, "{m}");
    s
}

/// A raw-integer live-sensor figure (bpm / watts / rpm) as plain digits, or `--` when the reading
/// is absent or stale — the "no data" glyph the live speed/elevation tiles share.
fn fmt_int_opt(v: Option<u32>) -> heapless::String<8> {
    match v {
        Some(v) => fmt_int(v),
        None => dashes(),
    }
}

/// A live-elevation figure: rounded to a whole unit (signed, so a sub-sea-level reading shows a
/// `-` rather than wrapping), or `--` when there's no altimeter sample yet. Rounds
/// half away from zero without `libm` (the codebase keeps elevation maths off the math lib).
fn fmt_elev(v: Option<f32>) -> heapless::String<8> {
    let mut s = heapless::String::new();
    match v {
        Some(v) => {
            let rounded = (v + if v >= 0.0 { 0.5 } else { -0.5 }) as i32;
            let _ = write!(s, "{rounded}");
        }
        None => {
            let _ = s.push_str("--");
        }
    }
    s
}

/// A duration in seconds as `H:MM` (moving time) — hours uncapped, minutes zero-padded.
pub(crate) fn fmt_hms(secs: f32) -> heapless::String<8> {
    let total_min = (secs as u32) / 60;
    let mut s = heapless::String::new();
    let _ = write!(s, "{}:{:02}", total_min / 60, total_min % 60);
    s
}

/// Glue two caption fragments into a tile caption (e.g. `"AVG "` + `Units::speed_label()`),
/// keeping the unit label as the single source of truth. `String<24>` to share the
/// [`StatCell::caption`] type (a waypoint name's width); the built-in fragments are far shorter.
fn cap(a: &str, b: &str) -> heapless::String<24> {
    let mut s = heapless::String::new();
    let _ = s.push_str(a);
    let _ = s.push_str(b);
    s
}

/// The grade (%) at fractional position `frac`: rise over run across a small fixed window of the
/// route around it, using each end's mid-band elevation (base level). Zero when the run is
/// degenerate. Shared by the [`Grade`](StatField::Grade) field and the Statistics header readout.
pub(crate) fn grade_at(profile: &obc_route::Profile, total_distance_m: u32, frac: f32) -> i32 {
    // ±1.5 % of the route — a touch of smoothing.
    const HALF: f32 = 0.015;
    let lo = (frac - HALF).max(0.0);
    let hi = (frac + HALF).min(1.0);
    let mid = |t: f32| {
        let (a, b) = profile.at(t);
        (a as i32 + b as i32) / 2
    };
    let run_m = (hi - lo) * total_distance_m as f32;
    if run_m < 1.0 {
        return 0;
    }
    ((mid(hi) - mid(lo)) as f32 / run_m * 100.0) as i32
}

/// The ascent (m) still to climb between the rider's matched progress and the end of the route —
/// the profile's own [`ascent_between_m`](Profile::ascent_between_m) over `[progress, total]`.
///
/// One lookup, three readers: the `TO CLIMB` tile, the EL9 time model below, and (over a different
/// pair of distances) the Up-ahead rows' climb-to-go. The length axis is the **route reader's**
/// total, which is exactly what [`Activity::route_total_m`](crate::Activity) mirrors, so this can't
/// disagree with `DIST TO GO` about where the end is.
fn ascent_to_go_m(r: &RouteReader, p: &Profile, a: &Activity) -> u32 {
    p.ascent_between_m(a.progress_m, r.total_distance_m, r.total_distance_m)
}

/// Seconds still to ride to the end of the route under the EL9 gradient-aware model (#1077), or
/// `None` on a route-less ride / before the profile has streamed in — the shared source for both the
/// [`TimeToGo`](StatField::TimeToGo) and [`Eta`](StatField::Eta) tiles, so the duration and the
/// arrival stamp are always the same estimate rendered two ways.
///
/// A route with no elevation (a device-planned one, until EL7 fills it from terrain) has zero
/// ascent-to-go and so degrades to `dist / v_flat` — the model's own answer for a flat input, not a
/// branch here.
fn time_to_go_s(cx: &Readout) -> Option<u32> {
    let (r, p) = (cx.route?, cx.profile?);
    Some(obc_route::time_to_go_s(p, r.total_distance_m, cx.activity.progress_m, cx.bike_profile_idx))
}

/// The fractional live position (`0.0`–`1.0`) along the route; `0.0` when no length is known.
/// Shared by the route-relative fields here and the Statistics screen's cursor logic.
pub(crate) fn live_frac(a: &Activity) -> f32 {
    if a.route_total_m == 0 {
        0.0
    } else {
        (a.progress_m as f32 / a.route_total_m as f32).clamp(0.0, 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activity::Mode;
    use crate::harness::support::wpts;

    /// The const default grid is byte-identical to the push-built list it replaced — the pin that
    /// keeps `StatFieldList::DEFAULT` honest against `push`'s semantics (#1197's const chain).
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

    /// A list built from a slice of fields, for the layout/reorder tests.
    fn list(fields: &[StatField]) -> StatFieldList {
        let mut l = StatFieldList { ids: [StatField::Speed; MAX_STAT_FIELDS], len: 0 };
        for &f in fields {
            assert!(l.push(f), "test list overflowed or duplicated");
        }
        l
    }

    /// The default selection is the six classic single-column tiles, one page, no gaps.
    #[test]
    fn default_is_the_classic_six() {
        let l = StatFieldList::default();
        assert_eq!(l.len(), 6);
        assert_eq!(page_count(&l), 1);
        assert_eq!(page_fields(&l, 0).len(), 6);
    }

    /// Single-span fields fill left-to-right, top-to-bottom; the 7th spills to page 2.
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

    /// A two-span tile at an even slot fills a whole row; the next single starts the row below it.
    #[test]
    fn two_span_fills_a_row() {
        let l = list(&[StatField::Clock, StatField::Speed]);
        let p = page_fields(&l, 0);
        assert_eq!((p[0].field, p[0].col, p[0].row), (StatField::Clock, 0, 0));
        assert_eq!((p[1].field, p[1].col, p[1].row), (StatField::Speed, 0, 1));
    }

    /// A two-span tile after an *odd* number of singles is bumped to the next row (a defensive gap),
    /// so it never straddles a row.
    #[test]
    fn two_span_after_one_single_starts_a_new_row() {
        let l = list(&[StatField::Speed, StatField::Clock]);
        let p = page_fields(&l, 0);
        assert_eq!((p[0].col, p[0].row), (0, 0), "the single sits top-left");
        assert_eq!((p[1].field, p[1].col, p[1].row), (StatField::Clock, 0, 1), "the wide tile bumps to row 1");
    }

    /// Moving a single-span field steps one slot at a time and swaps order with its neighbour.
    #[test]
    fn move_single_steps_by_one() {
        let mut l = list(&[StatField::Speed, StatField::AvgSpeed, StatField::DistDone]);
        let ni = l.move_item(0, 1);
        assert_eq!(ni, 1);
        assert_eq!(l.as_slice(), &[StatField::AvgSpeed, StatField::Speed, StatField::DistDone]);
    }

    /// Moving a two-span field down hops a *pair* of singles in one step (landing on the next
    /// even-slot position) so it stays row-aligned — the rider-facing reorder rule.
    #[test]
    fn move_two_span_hops_a_row() {
        // [Clock(wide), a, b, c] — moving the wide tile down skips past the pair (a, b).
        let mut l = list(&[StatField::Clock, StatField::Speed, StatField::AvgSpeed, StatField::DistDone]);
        let ni = l.move_item(0, 1);
        assert_eq!(ni, 2, "the wide tile lands after the pair, not between them");
        assert_eq!(l.as_slice(), &[StatField::Speed, StatField::AvgSpeed, StatField::Clock, StatField::DistDone]);
        // And every placement keeps the wide tile in the left column (row-aligned).
        for placed in page_fields(&l, 0) {
            if placed.field == StatField::Clock {
                assert_eq!(placed.col, 0, "the wide tile always begins a row");
            }
        }
    }

    /// A move that can't find a valid landing (off the end) leaves the list untouched.
    #[test]
    fn move_off_the_end_is_a_noop() {
        let mut l = list(&[StatField::Speed, StatField::AvgSpeed]);
        assert_eq!(l.move_item(1, 1), 1, "the last item can't move further down");
        assert_eq!(l.as_slice(), &[StatField::Speed, StatField::AvgSpeed]);
        assert_eq!(l.move_item(0, -1), 0, "the first item can't move further up");
    }

    /// Add appends and refuses duplicates / overflow; remove shifts the tail down.
    #[test]
    fn add_and_remove() {
        let mut l = list(&[StatField::Speed]);
        assert!(l.push(StatField::Clock));
        assert!(!l.push(StatField::Speed), "a duplicate is refused");
        assert_eq!(l.as_slice(), &[StatField::Speed, StatField::Clock]);
        l.remove(0);
        assert_eq!(l.as_slice(), &[StatField::Clock]);
    }

    /// A bare readout over `activity` + a waypoint table — no fix, no route, no profile, no
    /// next-waypoint. The point of [`Readout`]: formatting a cell needs no `RenderScratch`, no
    /// `Render`. Tests that exercise the next-waypoint tile set `waypoints` / `next_waypoint` on the
    /// returned value.
    /// An empty per-category cache: the tiles then answer from the resident waypoint table alone.
    /// A `static` (not a `&` temporary) so it outlives the borrowed `Readout`.
    static EMPTY_CACHE: &crate::next_ahead::NextAhead = &crate::next_ahead::NextAhead::EMPTY;

    fn readout<'a>(activity: &'a Activity, units: Units, waypoints: &'a Waypoints) -> Readout<'a> {
        Readout {
            fix: None,
            activity,
            units,
            route: None,
            profile: None,
            climb: None,
            waypoints,
            next_waypoint: None,
            now: DateTime::default(),
            now_ms: 0,
            bike_profile_idx: 0,
            language: Language::En,
            next_ahead: EMPTY_CACHE,
        }
    }

    // ---------------------------------------------------------------------------------------
    // The EL9 time tiles (#1077) — TIME TO GO / ETA against a real converted route.
    // ---------------------------------------------------------------------------------------

    /// A ~9 km pass: 500 m up to 800 m and back down, zigzagged so no corner decimates away. All
    /// 300 m of ascent sit in the first half, which is where the two tiles have to move fastest.
    const PASS_GPX: &str = r#"<gpx><trk><trkseg>
    <trkpt lat="47.0000" lon="8.0000"><ele>500</ele></trkpt>
    <trkpt lat="47.0020" lon="8.0200"><ele>600</ele></trkpt>
    <trkpt lat="47.0000" lon="8.0400"><ele>700</ele></trkpt>
    <trkpt lat="47.0020" lon="8.0600"><ele>800</ele></trkpt>
    <trkpt lat="47.0000" lon="8.0800"><ele>700</ele></trkpt>
    <trkpt lat="47.0020" lon="8.1000"><ele>600</ele></trkpt>
    <trkpt lat="47.0000" lon="8.1200"><ele>500</ele></trkpt>
  </trkseg></trk></gpx>"#;

    /// Convert [`PASS_GPX`] and run `f` with the route reader + its profile, exactly as the App
    /// holds them (a `RouteReader` borrows its source, so this has to be a closure, not a return).
    fn with_pass_route<R>(f: impl FnOnce(&RouteReader, &Profile) -> R) -> R {
        use obc_formats::io::{ByteSink, Error, SliceSource};
        #[derive(Default)]
        struct VecSink(std::vec::Vec<u8>);
        impl ByteSink for VecSink {
            fn write(&mut self, b: &[u8]) -> Result<(), Error> {
                self.0.extend_from_slice(b);
                Ok(())
            }
            fn patch_at(&mut self, off: u32, b: &[u8]) -> Result<(), Error> {
                let o = off as usize;
                self.0[o..o + b.len()].copy_from_slice(b);
                Ok(())
            }
        }
        let mut sink = VecSink::default();
        obc_route::gpx_to_obcr(&SliceSource(PASS_GPX.as_bytes()), "Pass", &mut sink).unwrap();
        let src = SliceSource(&sink.0);
        let idx = obc_route::RouteIndex::read(&src).unwrap();
        let route = RouteReader::new(&idx, &src);
        let profile = route.elevation_profile();
        f(&route, &profile)
    }

    /// Both tiles render **one** estimate: TIME TO GO as `H:MM`, ETA as that many minutes added to
    /// the wall clock. The number itself is `obc-route`'s gradient-aware model, so this pins the
    /// wiring (route + profile + bike profile in, the right two strings out), not the physics.
    #[test]
    fn time_tiles_render_the_gradient_aware_estimate() {
        with_pass_route(|route, profile| {
            let mut activity = Activity::new(Mode::Riding);
            activity.route_total_m = route.total_distance_m;
            let empty = Waypoints::new();
            let cx = Readout {
                route: Some(route),
                profile: Some(profile),
                // 14:00 on the wall clock, so the ETA arithmetic is easy to read.
                now: DateTime { hour: 14, minute: 0, ..DateTime::default() },
                ..readout(&activity, Units::Metric, &empty)
            };
            assert_eq!(route.total_ascent_m, 300, "the fixture climbs 300 m");

            let secs = obc_route::route_time_s(route.total_distance_m, route.total_ascent_m, 0);
            assert_eq!(StatField::TimeToGo.cell(&cx).value.as_str(), fmt_hms(secs as f32).as_str());
            // ETA = 14:00 + the estimate rounded to the nearest minute.
            let mins = (secs + 30) / 60;
            let mut want: heapless::String<8> = heapless::String::new();
            let at = DateTime { hour: 14, minute: 0, ..DateTime::default() }.add_minutes(mins);
            write!(want, "{:02}:{:02}", at.hour, at.minute).unwrap();
            assert_eq!(StatField::Eta.cell(&cx).value.as_str(), want.as_str());

            // The climb is really in there: the same route ridden as if it were flat is quicker by
            // the climb term (300 m × 1.6 s/m = 480 s on the Road profile).
            let flat = obc_route::ride_time_s(route.total_distance_m, 0, 0);
            assert!((secs - flat).abs_diff(480) <= 2, "the climb term is {} s", secs - flat);

            // Unit-system independent: hours and minutes are the same in both systems.
            let imperial =
                Readout { route: Some(route), profile: Some(profile), ..readout(&activity, Units::Imperial, &empty) };
            assert_eq!(StatField::TimeToGo.cell(&imperial).value, StatField::TimeToGo.cell(&cx).value);
        });
    }

    /// The bike profile is what the model is keyed by: an MTB estimate is longer than a road one on
    /// the identical route, and a stale out-of-range index falls back to profile 0 (the router's own
    /// rule) rather than reading `--` or panicking.
    #[test]
    fn time_tiles_follow_the_bike_profile() {
        with_pass_route(|route, profile| {
            let mut activity = Activity::new(Mode::Riding);
            activity.route_total_m = route.total_distance_m;
            let empty = Waypoints::new();
            let secs = |idx: u8| {
                let cx = Readout {
                    route: Some(route),
                    profile: Some(profile),
                    bike_profile_idx: idx,
                    ..readout(&activity, Units::Metric, &empty)
                };
                time_to_go_s(&cx).unwrap()
            };
            assert!(secs(2) > secs(0), "an MTB is slower than a road bike over the same pass");
            assert_eq!(secs(99), secs(0), "a stale index falls back to profile 0");
        });
    }

    /// TIME TO GO counts down and ETA never slips later as the rider advances — the monotonicity the
    /// model guarantees, checked through the rendered tiles (so a formatting bug can't hide it).
    /// At the finish both read the arrival instant: `0:00` and the current clock.
    #[test]
    fn time_tiles_count_down_as_the_ride_advances() {
        with_pass_route(|route, profile| {
            let total = route.total_distance_m;
            let empty = Waypoints::new();
            let mut prev = u32::MAX;
            let mut prev_eta: std::string::String = std::string::String::new();
            for step in 0..=40u32 {
                let mut activity = Activity::new(Mode::Riding);
                activity.route_total_m = total;
                activity.progress_m = total * step / 40;
                let cx = Readout {
                    route: Some(route),
                    profile: Some(profile),
                    now: DateTime { hour: 14, minute: 0, ..DateTime::default() },
                    ..readout(&activity, Units::Metric, &empty)
                };
                let secs = time_to_go_s(&cx).unwrap();
                assert!(secs <= prev, "time-to-go rose from {prev} to {secs} s at {} m", activity.progress_m);
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

    /// The Elevation tile reads the live barometric altitude, not the route profile: it shows the
    /// current height with no route loaded, converts to the active unit, and reads `--` before the
    /// first altimeter sample.
    #[test]
    fn elevation_tile_reads_live_barometric_altitude() {
        let mut activity = Activity::new(Mode::Riding);
        let empty = Waypoints::new();
        let value = |a: &Activity, units: Units| StatField::Elevation.cell(&readout(a, units, &empty)).value;

        assert_eq!(value(&activity, Units::Metric).as_str(), "--", "no altimeter sample yet");

        activity.record_altitude(144.0);
        assert_eq!(value(&activity, Units::Metric).as_str(), "144", "metric shows whole metres");
        // 144 m × 3.28084 ≈ 472.4 ft → rounds to 472.
        assert_eq!(value(&activity, Units::Imperial).as_str(), "472", "imperial converts to feet");
    }

    /// …and switches to the **map-referenced** height once the EL8 estimator settles (#1076): same
    /// tile, same presentation, a trustworthy absolute number. Unsettled it is still the raw
    /// reading — the tile never shows a half-converged one.
    #[test]
    fn elevation_tile_switches_to_the_fused_height_once_settled() {
        let mut activity = Activity::new(Mode::Riding);
        let empty = Waypoints::new();
        let value = |a: &Activity| StatField::Elevation.cell(&readout(a, Units::Metric, &empty)).value;

        // The barometer reads 62 m high all ride; the terrain under the fix says 1800 m.
        activity.record_altitude(1862.0);
        activity.record_map_elevation(1800);
        assert_eq!(value(&activity).as_str(), "1862", "one residual is not settled → the raw reading");

        for _ in 1..crate::altitude::SETTLE_SAMPLES {
            activity.record_altitude(1862.0);
            activity.record_map_elevation(1800);
        }
        assert_eq!(value(&activity).as_str(), "1800", "settled → the map-referenced height");
        // The barometer then climbs a real 40 m; the fused tile follows it metre for metre.
        activity.record_altitude(1902.0);
        assert_eq!(value(&activity).as_str(), "1840", "baro supplies the dynamics, the map the frame");
    }

    /// With no fix, no route and a fresh ride, every field falls back to its documented idle
    /// reading — `--` for the live/averaged values, zeros for the accumulators.
    #[test]
    fn fields_fall_back_without_data() {
        let activity = Activity::new(Mode::Riding);
        let empty = Waypoints::new();
        let cx = readout(&activity, Units::Metric, &empty);
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
        assert_eq!(val(StatField::Clock).as_str(), "12:00", "the neutral default DateTime");
        assert_eq!(val(StatField::NextWaypoint).as_str(), "--", "no route → the waypoint tile reads --");
    }

    /// A **route-less ride** (a live session, distance/climb accumulated, but no route loaded): the
    /// route-relative tiles read `--`, while the route-independent ones (distance done, climbed,
    /// elevation) show their real values. This is the mid-route-less-ride grid.
    #[test]
    fn route_less_ride_shows_dashes_for_route_fields_but_real_data_otherwise() {
        let mut activity = Activity::new(Mode::Riding);
        // Accumulate some ridden distance, climb, and a live altitude — no route involved.
        activity.record_motion(Fix::at(52_520_000, 13_405_000), 0);
        activity.record_motion(Fix::at(52_520_100, 13_405_000), 2000);
        activity.record_altitude(200.0);
        activity.record_altitude(230.0); // +30 m climbed
        let empty = Waypoints::new();
        let cx = readout(&activity, Units::Metric, &empty); // route: None, profile: None
        let val = |f: StatField| f.cell(&cx).value;
        // Route-relative → dashes.
        assert_eq!(val(StatField::DistToGo).as_str(), "--", "no route → to-go reads --");
        assert_eq!(val(StatField::ToClimb).as_str(), "--", "no route → to-climb reads --");
        assert_eq!(val(StatField::Grade).as_str(), "--", "no route → grade reads --");
        assert_eq!(val(StatField::TimeToGo).as_str(), "--", "no route → time-to-go reads --");
        assert_eq!(val(StatField::Eta).as_str(), "--", "no route → ETA reads --");
        // Route-independent → real data.
        assert_ne!(val(StatField::DistDone).as_str(), "--", "distance done is real, not --");
        assert_eq!(val(StatField::Climbed).as_str(), "30", "climbed is barometric, route-independent");
        assert_eq!(val(StatField::Elevation).as_str(), "230", "elevation is the live altitude");
    }

    /// The Speed tile reads the fix's ground speed and rescales per unit system.
    #[test]
    fn speed_tile_reads_the_fix() {
        let activity = Activity::new(Mode::Riding);
        let empty = Waypoints::new();
        let fix = Fix { speed_mps: Some(10.0), ..Fix::at(0, 0) };
        let value = |units: Units| {
            StatField::Speed.cell(&Readout { fix: Some(fix), ..readout(&activity, units, &empty) }).value
        };
        assert_eq!(value(Units::Metric).as_str(), "36.0", "10 m/s reads 36 km/h");
        assert_eq!(value(Units::Imperial).as_str(), "22.4", "…and 22.4 mph");
    }

    /// Discriminants round-trip through `from_u8`, and an unknown byte is dropped. Byte `10`
    /// (`NextWaypoint`, the on-disk contract this sub-issue appends) resolves and survives a
    /// `StatFieldList` decode — so a persisted grid carrying the field reloads it.
    #[test]
    fn discriminant_round_trips() {
        for f in StatField::ALL {
            assert_eq!(StatField::from_u8(f as u8), Some(f));
        }
        assert_eq!(StatField::from_u8(200), None, "an unknown discriminant is rejected");
        assert_eq!(StatField::from_u8(10), Some(StatField::NextWaypoint), "byte 10 is Next waypoint");
        let list = StatFieldList::decode(1, &[10]);
        assert_eq!(list.as_slice(), &[StatField::NextWaypoint], "a decoded byte-10 selection keeps the field");
    }

    /// An empty selection is still one page (drawing nothing), never zero — the `.max(1)` guard.
    #[test]
    fn empty_selection_is_one_page() {
        let l = list(&[]);
        assert_eq!(page_count(&l), 1);
        assert!(page_fields(&l, 0).is_empty());
    }

    /// `fmt_dist_short` metric: metres below 1 km, one decimal km up to 100 km, whole km above —
    /// pinned across the 1 km and 100 km crossovers.
    #[test]
    fn fmt_dist_short_metric_crossovers() {
        assert_eq!(fmt_dist_short(0, Units::Metric).as_str(), "0m");
        assert_eq!(fmt_dist_short(487, Units::Metric).as_str(), "487m");
        assert_eq!(fmt_dist_short(999, Units::Metric).as_str(), "999m", "just under 1 km stays metres");
        assert_eq!(fmt_dist_short(1000, Units::Metric).as_str(), "1.0km", "1 km crosses to one-decimal km");
        assert_eq!(fmt_dist_short(12_400, Units::Metric).as_str(), "12.4km");
        assert_eq!(fmt_dist_short(99_900, Units::Metric).as_str(), "99.9km", "just under 100 km keeps a decimal");
        assert_eq!(fmt_dist_short(100_000, Units::Metric).as_str(), "100km", "100 km crosses to whole km");
        assert_eq!(fmt_dist_short(153_000, Units::Metric).as_str(), "153km");
    }

    /// `fmt_dist_short` imperial: feet below 1000 ft, one decimal miles up to 100 mi, whole miles
    /// above — pinned across the ft→mi and 100 mi crossovers.
    #[test]
    fn fmt_dist_short_imperial_crossovers() {
        assert_eq!(fmt_dist_short(0, Units::Imperial).as_str(), "0ft");
        assert_eq!(fmt_dist_short(300, Units::Imperial).as_str(), "984ft", "300 m ≈ 984 ft stays feet");
        // 1000 ft ≈ 304.8 m — the feet→miles crossover; 305 m ≈ 1000 ft reads a fractional mile.
        assert_eq!(fmt_dist_short(305, Units::Imperial).as_str(), "0.2mi", "past 1000 ft crosses to decimal miles");
        assert_eq!(fmt_dist_short(15_933, Units::Imperial).as_str(), "9.9mi");
        // 100 mi = 528000 ft ≈ 160934 m — the decimal→whole-miles crossover.
        assert_eq!(fmt_dist_short(160_000, Units::Imperial).as_str(), "99.4mi", "just under 100 mi keeps a decimal");
        assert_eq!(fmt_dist_short(200_000, Units::Imperial).as_str(), "124mi", "well past 100 mi is whole miles");
    }

    /// The next-waypoint tile ahead of the rider: caption = the waypoint's name, value = its
    /// along-route distance-to-go (`dist_along_m − progress`) in the readout's units, right-aligned
    /// so it sits at the wide tile's far edge.
    #[test]
    fn next_waypoint_tile_names_and_counts_down() {
        let w = wpts(&[(1_000, "Brunnen"), (5_000, "Pass Summit")]);
        let mut activity = Activity::new(Mode::Riding);
        activity.progress_m = 1_200; // past Brunnen, 3.8 km before Pass Summit
        let cell =
            |units| StatField::NextWaypoint.cell(&Readout { next_waypoint: Some(1), ..readout(&activity, units, &w) });

        let m = cell(Units::Metric);
        assert_eq!(m.caption.as_str(), "Pass Summit", "caption is the waypoint's name");
        assert_eq!(m.value.as_str(), "3.8km", "metric distance-to-go = 5000 − 1200 m");
        assert_eq!(m.value_align, TextAlign::Right, "the wide-tile value hugs the far edge");
        assert!(!m.arrow, "no climb triangle on the waypoint tile");

        let i = cell(Units::Imperial);
        assert_eq!(i.caption.as_str(), "Pass Summit");
        assert_eq!(i.value.as_str(), "2.4mi", "imperial distance-to-go (3800 m ≈ 2.36 mi)");
    }

    /// Inside the 100 m pass-linger (progress ≥ the waypoint's `dist`, its index still current) the
    /// shown distance clamps to `0m` via `saturating_sub` — the "you are here" readout the chip also
    /// pins until the index advances.
    #[test]
    fn next_waypoint_tile_clamps_to_zero_in_the_linger() {
        let w = wpts(&[(1_000, "Brunnen")]);
        let mut activity = Activity::new(Mode::Riding);
        activity.progress_m = 1_050; // 50 m past Brunnen, still inside its 100 m linger band
        let cell =
            StatField::NextWaypoint.cell(&Readout { next_waypoint: Some(0), ..readout(&activity, Units::Metric, &w) });
        assert_eq!(cell.caption.as_str(), "Brunnen");
        assert_eq!(cell.value.as_str(), "0m", "saturating_sub clamps the passed distance to zero");
    }

    /// Empty state — caption `NEXT WPT`, value `--`, still right-aligned — for every way there's no
    /// waypoint ahead: no index resolved (`None`, i.e. no route / nothing ahead), a stale out-of-
    /// range index, and an empty table.
    #[test]
    fn next_waypoint_tile_empty_state() {
        let activity = Activity::new(Mode::Riding);
        let w = wpts(&[(1_000, "Brunnen")]);
        let empty = Waypoints::new();
        let check = |cell: StatCell| {
            assert_eq!(cell.caption.as_str(), "NEXT WPT");
            assert_eq!(cell.value.as_str(), "--");
            assert_eq!(cell.value_align, TextAlign::Right, "the fallback stays right-aligned too");
        };
        // next_waypoint = None (the readout default): no route, or nothing ahead.
        check(StatField::NextWaypoint.cell(&readout(&activity, Units::Metric, &w)));
        // A stale index past the table's end (defensive against a lagging resolver).
        check(
            StatField::NextWaypoint.cell(&Readout { next_waypoint: Some(9), ..readout(&activity, Units::Metric, &w) }),
        );
        // An index against an empty table (no route loaded).
        check(
            StatField::NextWaypoint
                .cell(&Readout { next_waypoint: Some(0), ..readout(&activity, Units::Metric, &empty) }),
        );
    }

    /// As a two-span field the tile fills a whole row and the next single starts the row below —
    /// exactly the wide-`Clock` layout it mirrors.
    #[test]
    fn next_waypoint_two_span_fills_a_row() {
        assert_eq!(StatField::NextWaypoint.span(), 2, "the waypoint tile is full-width");
        let l = list(&[StatField::NextWaypoint, StatField::Speed]);
        let p = page_fields(&l, 0);
        assert_eq!((p[0].field, p[0].col, p[0].row), (StatField::NextWaypoint, 0, 0));
        assert_eq!((p[1].field, p[1].col, p[1].row), (StatField::Speed, 0, 1));
    }

    // ── `Next: <category>` tiles (epic #946, U5) ───────────────────────────────────────────────

    /// A `Waypoints` table from `(dist_along_m, name, category)` — the categorized (U1) source half
    /// of a `Next: <category>` tile.
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

    /// A cache holding one harvested corridor answer per `(category, dist_along_m, name)` — the
    /// map-POI (U2) source half, filled through the real arm→harvest path so the test can't cache
    /// something the scheduler wouldn't.
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
                    poi: Poi { lat: 0, lon: 0, subtype, name: n, hours_ref: 0xFFFF, distance_m: dist_along_m },
                    dist_along_m,
                    offset_m: 0,
                }],
            );
        }
        c
    }

    /// A **route-loaded** readout over both sources at `progress_m` — `active_route` is what makes a
    /// route-relative tile answer at all (the Up-ahead list's own guard).
    fn riding<'a>(
        activity: &'a mut Activity,
        waypoints: &'a Waypoints,
        next_ahead: &'a crate::next_ahead::NextAhead,
        units: Units,
        progress_m: u32,
    ) -> Readout<'a> {
        activity.active_route = Some(0);
        activity.progress_m = progress_m;
        Readout { next_ahead, ..readout(activity, units, waypoints) }
    }

    /// The six tiles sit **directly after** `Next waypoint` in catalogue order (the picker's order),
    /// while their on-disk discriminants are appended at the end — the two orders are independent,
    /// and both are contracts.
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
        // The categories, in canonical id order — the picker's block mirrors the POI menu's.
        assert_eq!(six.map(|f| f.category().unwrap()), PoiCategory::ALL);
        // Every other field carries no category, so nothing else can pick up the icon anatomy.
        for f in StatField::ALL {
            assert_eq!(f.category().is_some(), six.contains(&f), "{f:?} category-ness");
        }
    }

    /// Append-only discriminants 15..=20, decoding through `from_u8` and surviving a
    /// `StatFieldList` decode — a persisted grid carrying them reloads.
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
        // And they round-trip back out through the fixed-width settings blob.
        let (len, ids) = list.encode();
        assert_eq!(StatFieldList::decode(len, &ids).as_slice(), list.as_slice());
    }

    /// Three tiles' worth of layout: each is full-width, so three of them fill a page and the fourth
    /// starts the next — the wide-tile rule, unchanged.
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

    /// The tile answers from **both** sources: the nearest entry ahead wins whether it is a map POI
    /// (the cache) or the rider's own categorized waypoint, and a tie goes to the waypoint — the
    /// Up-ahead merge's rule, so one entry can't read differently in the list and in a tile.
    #[test]
    fn next_category_tile_takes_the_nearest_of_either_source() {
        let w = cat_wpts(&[(2_000, "Brunnen", Some(PoiCategory::Water)), (9_000, "Camp", Some(PoiCategory::Campsite))]);
        let c = cache(&[(PoiCategory::Water, 5_000, "Fontaine"), (PoiCategory::Campsite, 4_000, "Camping Est")]);
        let mut a = Activity::new(Mode::Riding);

        // Water: the waypoint at 2 km is nearer than the cached POI at 5 km.
        let cx = riding(&mut a, &w, &c, Units::Metric, 0);
        let cell = StatField::NextWater.cell(&cx);
        assert_eq!(cell.caption.as_str(), "Brunnen", "the rider's own waypoint is nearest");
        assert_eq!(cell.value.as_str(), "2.0km");
        assert_eq!(cell.value_align, TextAlign::Right, "the wide-tile distance hugs the far edge");
        // Campsite: the cached map POI at 4 km beats the waypoint at 9 km.
        let cell = StatField::NextCampsite.cell(&cx);
        assert_eq!(cell.caption.as_str(), "Camping Est", "the corridor POI is nearest");
        assert_eq!(cell.value.as_str(), "4.0km");

        // Ride past the water waypoint: the tile hands over to the cached POI and counts down to it.
        let cx = riding(&mut a, &w, &c, Units::Metric, 2_100);
        let cell = StatField::NextWater.cell(&cx);
        assert_eq!((cell.caption.as_str(), cell.value.as_str()), ("Fontaine", "2.9km"));

        // A tie at the same metre goes to the waypoint (`Merge`'s tie-break).
        let tie = cat_wpts(&[(5_000, "Brunnen", Some(PoiCategory::Water))]);
        let cx = riding(&mut a, &tie, &c, Units::Metric, 0);
        assert_eq!(StatField::NextWater.cell(&cx).caption.as_str(), "Brunnen", "a tie goes to the plan entry");
    }

    /// The empty state is the icon's own caption + `--`: no route at all, nothing of that category
    /// ahead, and — the case the refresh policy creates — a cached entry the rider has ridden past,
    /// which must never render as a phantom `0m`.
    #[test]
    fn next_category_tile_empty_states() {
        let w = cat_wpts(&[(2_000, "Turn left", None), (3_000, "Camp", Some(PoiCategory::Campsite))]);
        let c = cache(&[(PoiCategory::Water, 1_000, "Fontaine")]);
        let mut a = Activity::new(Mode::Riding);

        // No route loaded: route-relative, so `--` — and no leak from the resident tables.
        let empty = crate::next_ahead::NextAhead::new();
        let cx = Readout { next_ahead: &c, ..readout(&a, Units::Metric, &w) };
        let cell = StatField::NextWater.cell(&cx);
        assert_eq!((cell.caption.as_str(), cell.value.as_str()), ("Water", "--"), "no route ⇒ the category name + --");
        assert_eq!(cell.value_align, TextAlign::Right, "the fallback stays right-aligned too");

        // Nothing of this category anywhere (the generic waypoint doesn't answer a category question).
        let cx = riding(&mut a, &w, &empty, Units::Metric, 0);
        assert_eq!(StatField::NextPharmacy.cell(&cx).value.as_str(), "--");
        assert_eq!(StatField::NextWater.cell(&cx).value.as_str(), "--", "an empty cache is just no answer yet");
        // …while a categorized waypoint of another kind still answers its own tile.
        assert_eq!(StatField::NextCampsite.cell(&cx).caption.as_str(), "Camp");

        // A cached entry the rider has passed is dropped, not clamped to `0m` (the scheduler re-arms).
        let cx = riding(&mut a, &w, &c, Units::Metric, 1_500);
        let cell = StatField::NextWater.cell(&cx);
        assert_eq!((cell.caption.as_str(), cell.value.as_str()), ("Water", "--"), "a passed cache line reads --");
    }

    /// The distance is the shared `fmt_dist_short` readout, so the tile re-scales with the unit
    /// system exactly like the next-waypoint tile beside it.
    #[test]
    fn next_category_tile_follows_the_unit_system() {
        let w = Waypoints::new();
        let c = cache(&[(PoiCategory::BikeShop, 12_400, "Cycles Monaco")]);
        let mut a = Activity::new(Mode::Riding);
        assert_eq!(StatField::NextBikeShop.cell(&riding(&mut a, &w, &c, Units::Metric, 0)).value.as_str(), "12.4km");
        assert_eq!(StatField::NextBikeShop.cell(&riding(&mut a, &w, &c, Units::Imperial, 0)).value.as_str(), "7.7mi");
    }

    /// The field names are the **category** catalog strings (epic #602 + the epic's one-word-per-
    /// category rule) — in all four languages, and never an English fallback.
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
            let names: heapless::Vec<&str, 6> = PoiCategory::ALL.iter().map(|c| t(category_msg(*c), lang)).collect();
            for (i, n) in names.iter().enumerate() {
                assert!(!names[i + 1..].contains(n), "{n:?} appears twice in {lang:?}");
            }
        }
        assert_eq!(StatField::NextLodging.name(Language::De), "Unterkunft");
        assert_eq!(StatField::NextBikeShop.name(Language::Fr), "Vélociste");
    }

    // ── Multi-row panel machinery (issue #574) ─────────────────────────────────────────────────

    /// The panel's shape: span 2, rows 3, so a `SLOTS_PER_PAGE`-slot footprint — exactly one page.
    #[test]
    fn waypoint_list_is_a_page_sized_field() {
        assert_eq!(StatField::WaypointList.span(), 2, "the panel is full-width");
        assert_eq!(StatField::WaypointList.rows(), 3, "and three rows tall — the only multi-row field");
        assert_eq!(StatField::WaypointList.slots(), SLOTS_PER_PAGE, "span × rows = a whole page");
        // Every other field is a single row; slots() = span().
        for f in StatField::ALL {
            if f != StatField::WaypointList {
                assert_eq!(f.rows(), 1, "{f:?} is one row tall");
                assert_eq!(f.slots(), f.span() as usize, "{f:?} slots() == span()");
            }
        }
    }

    /// The panel always begins a page — first, mid-list, after an odd single, or after a wide tile —
    /// consuming all six slots so the following field lands on the next page. The row-align bump the
    /// wide tile does, scaled to a whole page.
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

        // After a wide tile (slots 0..2): the panel still bumps to the next page boundary, and a
        // following single lands on the page after it.
        let l = list(&[StatField::Clock, StatField::WaypointList, StatField::Speed]);
        assert_eq!(slot_of(&l, 0), Some(0), "the wide clock fills row 0 of page 0");
        assert_eq!(slot_of(&l, 1), Some(SLOTS_PER_PAGE), "the panel bumps to page 1");
        assert_eq!(slot_of(&l, 2), Some(2 * SLOTS_PER_PAGE), "the single after the panel lands on page 2");
        assert_eq!(page_count(&l), 3);
    }

    /// `page_count` counts the panel as a full page. Panel + six singles = two pages; panel + seven
    /// singles spills a third; and a maxed selection that includes the panel reaches beyond two —
    /// there is no ≤2-page assumption to trip over.
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

        // A maxed selection carrying the panel (it leads, then the catalogue fills the rest until
        // `push` refuses) spills well past two pages. Nothing may cap at two. (The catalogue carries
        // far more than MAX_STAT_FIELDS fields, so a full grid is always a MAX_STAT_FIELDS-sized
        // *subset* — and the panel is only in it if it was picked, hence the explicit lead.) The
        // exact count follows catalogue *order*, since that decides which fields make the cut: with
        // the EL9 pair (#1077) inserted before `Clock`, the subset is the panel + 11 single-column
        // fields = 17 slots = 3 pages (it was 4 while two wide tiles fell inside the cut).
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

    /// The panel hops a whole page of singles per step and can never land mid-page — the page-level
    /// analogue of the wide tile's row hop.
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
        // Down: past all six singles in one step, landing on the next page boundary — not between.
        let ni = l.move_item(0, 1);
        assert_eq!(ni, 6, "the panel lands after the whole page of six singles");
        assert_eq!(l.as_slice()[6], StatField::WaypointList);
        assert_eq!(slot_of(&l, 6), Some(SLOTS_PER_PAGE), "and its slot is a page boundary");
        // Up: hops the whole page back.
        let ni = l.move_item(6, -1);
        assert_eq!(ni, 0, "and back up a whole page in one step");
        assert_eq!(l.as_slice()[0], StatField::WaypointList);
    }

    /// A panel at either end can't move further (no valid aligned landing past the end) — a no-op.
    #[test]
    fn move_item_panel_is_a_noop_at_the_ends() {
        let mut l = list(&[StatField::WaypointList, StatField::Speed, StatField::AvgSpeed]);
        assert_eq!(l.move_item(0, -1), 0, "a leading panel can't move up");
        assert_eq!(l.as_slice()[0], StatField::WaypointList);
        let mut l = list(&[StatField::Speed, StatField::AvgSpeed, StatField::WaypointList]);
        assert_eq!(l.move_item(2, 1), 2, "a trailing panel can't move down");
        assert_eq!(l.as_slice()[2], StatField::WaypointList);
    }

    /// A single stepping past the panel hops the whole panel in one step, keeping its order
    /// relative to the other singles (the panel moves as one page-sized unit, not something to land
    /// inside).
    #[test]
    fn move_item_single_hops_the_whole_panel() {
        let mut l = list(&[StatField::Speed, StatField::WaypointList, StatField::AvgSpeed]);
        let ni = l.move_item(0, 1); // step Speed down, past the panel
        assert_eq!(ni, 1, "Speed lands right after the panel");
        assert_eq!(l.as_slice(), &[StatField::WaypointList, StatField::Speed, StatField::AvgSpeed]);
        // Speed hopped onto the page after the panel; its order before AvgSpeed is preserved.
        assert_eq!(slot_of(&l, 1), Some(SLOTS_PER_PAGE));
    }

    /// A wide tile navigating around the panel still lands only on an even (row-aligned) slot — the
    /// old even-singles-before rule, now falling out of the shared slot simulation.
    #[test]
    fn move_item_wide_stays_row_aligned_around_the_panel() {
        // [Speed, WaypointList, Clock]: the clock is on page 2 at an even slot. Moving it up, the
        // only valid landing is slot 0 (past the single *and* the panel) — never the odd slot 1.
        let mut l = list(&[StatField::Speed, StatField::WaypointList, StatField::Clock]);
        let ni = l.move_item(2, -1);
        assert_eq!(ni, 0, "the wide tile skips the odd slot-1 landing and lands row-aligned at slot 0");
        assert_eq!(l.as_slice(), &[StatField::Clock, StatField::Speed, StatField::WaypointList]);
        // Every placement keeps the wide tile in the left column.
        for placed in
            (0..page_count(&l)).flat_map(|p| page_fields(&l, p).into_iter()).filter(|p| p.field == StatField::Clock)
        {
            assert_eq!(placed.col, 0, "the wide tile always begins a row, even around the panel");
        }
    }

    /// `slot_of` / `next_free_slot` agree with `page_fields` around a trailing panel: the panel sits
    /// alone on its page at col 0 / row 0, and the ghost Add slot lands on the page after it.
    #[test]
    fn slot_queries_agree_with_page_fields_around_the_panel() {
        let l = list(&[StatField::Speed, StatField::AvgSpeed, StatField::WaypointList]);
        assert_eq!(slot_of(&l, 0), Some(0));
        assert_eq!(slot_of(&l, 1), Some(1));
        assert_eq!(slot_of(&l, 2), Some(SLOTS_PER_PAGE), "the panel starts page 1");
        assert_eq!(slot_of(&l, 3), None, "past the selection there is no slot");
        // The Add ghost sits past the panel, on page 2.
        assert_eq!(next_free_slot(&l), 2 * SLOTS_PER_PAGE);
        assert_eq!(next_free_slot(&l) / SLOTS_PER_PAGE, 2, "the ghost Add lands on the page after the panel");
        // page_fields draws the panel alone on its page, col 0 / row 0.
        let p1 = page_fields(&l, 1);
        assert_eq!(p1.len(), 1, "the panel owns its page");
        assert_eq!((p1[0].field, p1[0].col, p1[0].row), (StatField::WaypointList, 0, 0));
        // And each field's reported slot matches where page_fields places it.
        for (i, &f) in l.as_slice().iter().enumerate() {
            let slot = slot_of(&l, i).unwrap();
            let placed = page_fields(&l, slot / SLOTS_PER_PAGE).into_iter().find(|p| p.field == f).unwrap();
            let s = slot % SLOTS_PER_PAGE;
            assert_eq!((placed.col as usize, placed.row as usize), (s % COLS, s / COLS), "{f:?} slot vs placement");
        }
    }

    /// The panel's on-disk discriminant is `11` (append-only), decodes through `from_u8`, and
    /// survives a `StatFieldList` decode — a persisted grid carrying the panel reloads it.
    #[test]
    fn waypoint_list_discriminant_round_trips() {
        assert_eq!(StatField::WaypointList as u8, 11, "append-only: the panel is byte 11");
        assert_eq!(StatField::from_u8(11), Some(StatField::WaypointList));
        let list = StatFieldList::decode(1, &[11]);
        assert_eq!(list.as_slice(), &[StatField::WaypointList], "a decoded byte-11 selection keeps the panel");
    }

    // ── Live sensor tiles: HR / power / cadence (epic #707, SE5) ───────────────────────────────

    /// The three sensor tiles are single-column, arrow-free, and caption HR / PWR / RPM — the
    /// house all-caps register the neighbouring built-in captions use.
    #[test]
    fn sensor_tiles_are_single_column_captioned() {
        let activity = Activity::new(Mode::Riding);
        let empty = Waypoints::new();
        let cx = readout(&activity, Units::Metric, &empty);
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

    /// The sensor tiles read raw ints through `Activity`'s `live_*_display` accessors, which gate
    /// staleness on the last `tick`'s ride clock (`note_sensor_clock`) — not the `Readout`'s render
    /// clock — so the tile stays correct when a host's render clock differs from its ride clock (the
    /// sim, mid GPX replay). A fresh sample shows the number, an absent or 5 s-stale one reads `--`,
    /// and a fresh coasting `0` cadence shows `0` (distinct from the `--` no-sensor state).
    #[test]
    fn sensor_tiles_format_raw_ints_and_dash_when_stale() {
        let mut activity = Activity::new(Mode::Riding);
        let empty = Waypoints::new();
        // The render clock passed to `Readout` is deliberately fixed and unrelated to the sensor
        // clock below — the tiles must ignore it and gate on `note_sensor_clock` instead.
        let val = |a: &Activity, f: StatField| {
            f.cell(&Readout { now_ms: 999_999, ..readout(a, Units::Metric, &empty) }).value
        };

        // No sensor yet → all three read `--`.
        activity.note_sensor_clock(0);
        assert_eq!(val(&activity, StatField::HeartRate).as_str(), "--", "no HR sensor → --");
        assert_eq!(val(&activity, StatField::Power).as_str(), "--", "no power meter → --");
        assert_eq!(val(&activity, StatField::Cadence).as_str(), "--", "no cadence sensor → --");

        // Fresh samples → the raw numbers, no glued unit.
        activity.record_hr(152, 1_000);
        activity.record_power(210, 1_000);
        activity.record_cadence(88, 1_000);
        activity.note_sensor_clock(1_000);
        assert_eq!(val(&activity, StatField::HeartRate).as_str(), "152");
        assert_eq!(val(&activity, StatField::Power).as_str(), "210");
        assert_eq!(val(&activity, StatField::Cadence).as_str(), "88");

        // Ride clock just past the 5 s gate → stale → `--` (a dropped sensor never freezes its value).
        activity.note_sensor_clock(6_001);
        assert_eq!(val(&activity, StatField::HeartRate).as_str(), "--", "HR older than 5 s reads --");
        assert_eq!(val(&activity, StatField::Power).as_str(), "--", "power older than 5 s reads --");
        assert_eq!(val(&activity, StatField::Cadence).as_str(), "--", "cadence older than 5 s reads --");

        // A fresh coasting `0` cadence is a real reading — shows `0`, not `--`.
        activity.record_cadence(0, 7_000);
        activity.note_sensor_clock(7_000);
        assert_eq!(val(&activity, StatField::Cadence).as_str(), "0", "a fresh coasting 0 shows 0, not --");
    }

    /// The sensor tiles' on-disk discriminants are 12 / 13 / 14 (append-only), decode through
    /// `from_u8`, and survive a `StatFieldList` decode — a persisted grid carrying them reloads.
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
