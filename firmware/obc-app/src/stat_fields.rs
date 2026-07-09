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

use core::fmt::Write;

use obc_render::text::TextAlign;
use obc_route::{Profile, RouteReader, Waypoints};

use crate::activity::Activity;
use crate::hal::Fix;
use crate::i18n::{t, Msg};
use crate::settings::{DateTime, Language, Units};

/// The narrow live-data view a stat field formats from — exactly what [`StatField::cell`] reads,
/// nothing more. Deliberately decoupled from the full draw context
/// ([`Render`](crate::screen::Render), which drags in the `MapRenderer`): a cell is pure
/// data-to-string, so a test — or a future non-draw host readout — builds a bare `Readout` instead
/// of faking a renderer. Constructed from a frame by [`Render::readout`](crate::screen::Render).
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
    /// The UI language (epic #602) — the word-bearing tile captions (`AVG`, `CLIMBED`, `TO GO`…)
    /// route through the catalog; the unit symbols glued to the value stay language-independent.
    pub language: Language,
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
    /// Current elevation — the live barometric altitude.
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
}

impl StatField {
    /// Every field, in catalogue order — drives the "Add field" picker and decode validation.
    pub const ALL: [StatField; 12] = [
        StatField::Speed,
        StatField::AvgSpeed,
        StatField::DistDone,
        StatField::DistToGo,
        StatField::Climbed,
        StatField::ToClimb,
        StatField::Grade,
        StatField::Elevation,
        StatField::RideTime,
        StatField::Clock,
        StatField::NextWaypoint,
        StatField::WaypointList,
    ];

    /// Decode a persisted discriminant, or `None` for an unknown byte (a newer writer, a bit-flip
    /// the CRC missed) — the codec drops it rather than trusting a garbage field.
    pub fn from_u8(b: u8) -> Option<StatField> {
        Self::ALL.into_iter().find(|f| *f as u8 == b)
    }

    /// Column span: `2` for the full-width [`Clock`](StatField::Clock),
    /// [`NextWaypoint`](StatField::NextWaypoint), and the [`WaypointList`](StatField::WaypointList)
    /// panel, else `1`.
    pub const fn span(self) -> u8 {
        match self {
            StatField::Clock | StatField::NextWaypoint | StatField::WaypointList => 2,
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
            StatField::Clock => t(Msg::StatfieldClock, lang),
            StatField::NextWaypoint => t(Msg::StatfieldNextWaypoint, lang),
            StatField::WaypointList => t(Msg::StatfieldWaypointList, lang),
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
                let value = match (cx.route, cx.profile) {
                    (Some(r), Some(p)) => {
                        fmt_int(units.elev(r.total_ascent_m.saturating_sub(p.ascent_to(live)) as f32) as u32)
                    }
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
                // The live barometric altitude, not the route profile — so it reads the current
                // height with no route loaded, and `--` until the first sample.
                let v = cx.activity.current_elevation_m().map(|m| units.elev(m));
                StatCell::new(cap(t(Msg::TileElev, lang), units.elev_label()), fmt_elev(v), false)
            }
            StatField::RideTime => StatCell::new(cap(t(Msg::TileRide, lang), ""), fmt_hms(cx.activity.moving_s), false),
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
        }
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

/// The rider's ordered field selection — a fixed-capacity list (no alloc) that is the POD persisted
/// in [`Settings`](crate::Settings). `Copy + Eq` so a settings edit is caught by one `==` (the same
/// trick the rest of [`Settings`](crate::Settings) uses). Slots past `len` are unused padding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StatFieldList {
    ids: [StatField; MAX_STAT_FIELDS],
    len: u8,
}

impl Default for StatFieldList {
    /// The classic six single-column tiles, in their original order — so an un-customized device
    /// (and a settings reset) shows exactly today's grid.
    fn default() -> Self {
        let mut list = StatFieldList { ids: [StatField::Speed; MAX_STAT_FIELDS], len: 0 };
        for f in [
            StatField::Speed,
            StatField::AvgSpeed,
            StatField::DistDone,
            StatField::DistToGo,
            StatField::Climbed,
            StatField::ToClimb,
        ] {
            let _ = list.push(f);
        }
        list
    }
}

impl StatFieldList {
    /// The selected fields, in display order.
    pub fn as_slice(&self) -> &[StatField] {
        &self.ids[..self.len as usize]
    }

    pub fn len(&self) -> usize {
        self.len as usize
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Whether `f` is already shown (so the picker can offer only the rest).
    pub fn contains(&self, f: StatField) -> bool {
        self.as_slice().contains(&f)
    }

    /// Append `f` to the end of the selection, or do nothing when full / already present. Returns
    /// whether it was added.
    pub fn push(&mut self, f: StatField) -> bool {
        if self.len as usize >= MAX_STAT_FIELDS || self.contains(f) {
            return false;
        }
        self.ids[self.len as usize] = f;
        self.len += 1;
        true
    }

    /// Remove the field at `i`, shifting the rest down.
    pub fn remove(&mut self, i: usize) {
        if i >= self.len as usize {
            return;
        }
        for k in i..self.len as usize - 1 {
            self.ids[k] = self.ids[k + 1];
        }
        self.len -= 1;
    }

    /// Pack into a length byte + [`MAX_STAT_FIELDS`] discriminant bytes (unused slots filled with
    /// the padding discriminant) — the fixed-width form the settings codec embeds.
    pub fn encode(&self) -> (u8, [u8; MAX_STAT_FIELDS]) {
        let mut ids = [0u8; MAX_STAT_FIELDS];
        for (b, f) in ids.iter_mut().zip(self.ids.iter()) {
            *b = *f as u8;
        }
        (self.len, ids)
    }

    /// Rebuild from a length byte + discriminant bytes, **sanitising** as it goes: the length is
    /// clamped, unknown discriminants are dropped, and duplicates are coalesced (via [`push`]) — so a
    /// valid-CRC-but-stale blob can never load a garbage or contradictory selection.
    pub fn decode(len: u8, ids: &[u8]) -> StatFieldList {
        let mut list = StatFieldList { ids: [StatField::Speed; MAX_STAT_FIELDS], len: 0 };
        let n = (len as usize).min(MAX_STAT_FIELDS).min(ids.len());
        for &b in &ids[..n] {
            if let Some(f) = StatField::from_u8(b) {
                let _ = list.push(f);
            }
        }
        list
    }

    /// Move the field at `i` one valid step in `dir` (`+1` down / `-1` up), returning its new index
    /// (unchanged if it can't move further). A single-span field moves one slot at a time; a
    /// **two-span** field only lands where it begins a row, and the page-sized **panel** only where
    /// it begins a page — so a wide tile hops over a pair of singles (or one wide tile) per step, and
    /// the panel hops a whole page. The rule is one slot-simulation: for each candidate insertion
    /// index, [`landing_slot`](Self::landing_slot) walks the *other* fields (with their own bumps) to
    /// the slot the moved field would start at, and the step is valid iff the field needs no bump of
    /// its own there ([`placed_slot`] is the identity) — subsuming the old even-singles-before rule.
    pub fn move_item(&mut self, i: usize, dir: i32) -> usize {
        let len = self.len as usize;
        if len == 0 || dir == 0 {
            return i.min(len.saturating_sub(1));
        }
        let i = i.min(len - 1);
        let f = self.ids[i];
        let step = dir.signum();
        // Candidate insertion indices in `dir`; skip past any index where the moved field would need
        // its own alignment bump (a wide tile landing mid-row, the panel landing mid-page).
        let mut p = i as i32;
        loop {
            let cand = p + step;
            if cand < 0 || cand as usize >= len {
                return i; // hit an end without a valid landing → no move
            }
            let slot = self.landing_slot(i, cand as usize);
            if placed_slot(slot, f) == slot {
                self.shift(i, cand as usize);
                return cand as usize;
            }
            p = cand;
        }
    }

    /// The slot the field currently at `from` would start at if reordered to insertion index `to`:
    /// walk the *other* fields in order (each bumped to its own alignment by [`placed_slot`]) and
    /// stop once `to` of them are placed — the accumulated slot is where the moved field then lands,
    /// before any bump of its own. The reorder-time mirror of [`walk`], sharing its `placed_slot`
    /// spine so a proposed landing can never disagree with where the grid would actually draw it.
    fn landing_slot(&self, from: usize, to: usize) -> usize {
        let mut slot = 0usize;
        let mut placed = 0usize;
        for (k, &g) in self.as_slice().iter().enumerate() {
            if k == from {
                continue; // the moved field isn't part of what precedes it
            }
            if placed == to {
                break; // `to` other fields now sit before the insertion point
            }
            slot = placed_slot(slot, g) + g.slots();
            placed += 1;
        }
        slot
    }

    /// Move the item from index `from` to index `to` by rotating the span between them — an
    /// order-preserving shift, not a swap, so the passed-over fields keep their relative order.
    fn shift(&mut self, from: usize, to: usize) {
        if from == to {
            return;
        }
        let f = self.ids[from];
        if to > from {
            for k in from..to {
                self.ids[k] = self.ids[k + 1];
            }
        } else {
            for k in (to + 1..=from).rev() {
                self.ids[k] = self.ids[k - 1];
            }
        }
        self.ids[to] = f;
    }
}

/// A field placed in the grid: which field, and its top-left cell (`col` ∈ `0..COLS`,
/// `row` ∈ `0..ROWS_PER_PAGE`) on its page. The Statistics screen turns this into pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Placed {
    pub field: StatField,
    pub col: u8,
    pub row: u8,
}

/// The slot `f` actually starts at when a left-to-right walk reaches it at `slot`, bumped forward so
/// it never straddles a row or page: a **single** fills any slot (no bump); a **two-span** tile that
/// would start in the right column is bumped to the next row; the page-sized **panel** is bumped to
/// the next page ([`slot.next_multiple_of(SLOTS_PER_PAGE)`](usize::next_multiple_of)). The single
/// alignment rule shared by the layout [`walk`] and [`StatFieldList::move_item`]'s landing check, so
/// a reorder can never propose a slot the walk would then shift out from under it.
fn placed_slot(slot: usize, f: StatField) -> usize {
    if f.slots() == SLOTS_PER_PAGE {
        slot.next_multiple_of(SLOTS_PER_PAGE) // the panel begins a page
    } else if f.span() == 2 && !slot.is_multiple_of(COLS) {
        slot + 1 // defensive: a malformed list can't mis-render — the wide tile begins a row
    } else {
        slot
    }
}

/// Walk the selection into global slots, calling `visit(field, slot)` for each. Every field is
/// placed at its [`placed_slot`] (bumped so a wide tile begins a row and the panel begins a page,
/// leaving a defensive gap) and then advances the cursor by its [`slots`](StatField::slots)
/// footprint. Because rows align to the [`SLOTS_PER_PAGE`] page, a bumped wide tile never straddles a
/// page either. Returns the total slots consumed (gaps included). Pure spine shared by
/// [`page_count`] / [`page_fields`] / [`slot_of`] / [`next_free_slot`].
fn walk(list: &StatFieldList, mut visit: impl FnMut(StatField, usize)) -> usize {
    let mut slot = 0usize;
    for &f in list.as_slice() {
        slot = placed_slot(slot, f);
        visit(f, slot);
        slot += f.slots();
    }
    slot
}

/// Number of pages the selection fills (at least `1`, even when empty — the grid draws nothing but
/// the page is still "page 0").
pub fn page_count(list: &StatFieldList) -> usize {
    let slots = walk(list, |_, _| {});
    slots.div_ceil(SLOTS_PER_PAGE).max(1)
}

/// The fields placed on `page` (clamped to the last page), with their on-page cells. At most
/// [`SLOTS_PER_PAGE`] entries.
pub fn page_fields(list: &StatFieldList, page: usize) -> heapless::Vec<Placed, SLOTS_PER_PAGE> {
    let page = page.min(page_count(list) - 1);
    let mut out = heapless::Vec::new();
    walk(list, |f, slot| {
        if slot / SLOTS_PER_PAGE == page {
            let s = slot % SLOTS_PER_PAGE;
            let _ = out.push(Placed { field: f, col: (s % COLS) as u8, row: (s / COLS) as u8 });
        }
    });
    out
}

/// The global slot the `index`-th selected field starts at (`None` past the selection) — the same
/// walk [`page_fields`] places with, so a cursor mapped through this always agrees with the drawn
/// grid. `slot / SLOTS_PER_PAGE` is the page, the remainder the on-page cell.
pub fn slot_of(list: &StatFieldList, index: usize) -> Option<usize> {
    let mut found = None;
    let mut i = 0usize;
    walk(list, |_, slot| {
        if i == index {
            found = Some(slot);
        }
        i += 1;
    });
    found
}

/// The first slot past the selection (gaps included) — where the Fields editor's ghost "add"
/// tile lands.
pub fn next_free_slot(list: &StatFieldList) -> usize {
    walk(list, |_, _| {})
}

// Value formatters + the grade helper — the field catalogue owns its own rendering. `grade_at` is
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
    /// next-waypoint. The point of [`Readout`]: formatting a cell needs no `MapRenderer`, no
    /// `Render`. Tests that exercise the next-waypoint tile set `waypoints` / `next_waypoint` on the
    /// returned value.
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
            language: Language::En,
        }
    }

    /// A `Waypoints` table from `(dist_along_m, name)` pairs, in route order — the stat-field
    /// mirror of `app.rs`'s `wpts` helper, for the next-waypoint tile tests.
    fn wpts(items: &[(u32, &str)]) -> Waypoints {
        let mut w = Waypoints::new();
        for &(dist_along_m, name) in items {
            let mut n = heapless::String::new();
            n.push_str(name).unwrap();
            w.entries.push(obc_route::WptEntry { dist_along_m, lon: 0, lat: 0, name: n }).unwrap();
        }
        w
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

        // The full 12-field catalogue (nine singles, two wide tiles, the panel) — a legitimate
        // MAX_STAT_FIELDS selection — reaches four pages. Nothing may cap at two.
        let full = {
            let mut l = StatFieldList { ids: [StatField::Speed; MAX_STAT_FIELDS], len: 0 };
            for f in StatField::ALL {
                assert!(l.push(f), "the whole catalogue fits MAX_STAT_FIELDS");
            }
            l
        };
        assert_eq!(full.len(), MAX_STAT_FIELDS, "all twelve fields selected");
        assert_eq!(page_count(&full), 4, "a full selection with the panel spans four pages, not two");
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

    /// A single stepping past the panel hops the whole panel in one detent, keeping its order
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
}
