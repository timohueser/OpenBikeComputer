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
//!   (3 rows × 2 cols), keeping a `2`-span tile row-aligned so it never straddles a row or page.

use core::fmt::Write;

use crate::screen::Render;

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
}

impl StatField {
    /// Every field, in catalogue order — drives the "Add field" picker and decode validation.
    pub const ALL: [StatField; 10] = [
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
    ];

    /// Decode a persisted discriminant, or `None` for an unknown byte (a newer writer, a bit-flip
    /// the CRC missed) — the codec drops it rather than trusting a garbage field.
    pub fn from_u8(b: u8) -> Option<StatField> {
        Self::ALL.into_iter().find(|f| *f as u8 == b)
    }

    /// Column span: `2` for the full-width [`Clock`](StatField::Clock), else `1`.
    pub const fn span(self) -> u8 {
        match self {
            StatField::Clock => 2,
            _ => 1,
        }
    }

    /// The field's name for the settings list / picker (the on-grid caption is in [`cell`](StatField::cell)).
    pub const fn name(self) -> &'static str {
        match self {
            StatField::Speed => "Speed",
            StatField::AvgSpeed => "Avg speed",
            StatField::DistDone => "Dist. done",
            StatField::DistToGo => "Dist. to go",
            StatField::Climbed => "Climbed",
            StatField::ToClimb => "To climb",
            StatField::Grade => "Grade",
            StatField::Elevation => "Elevation",
            StatField::RideTime => "Ride time",
            StatField::Clock => "Clock",
        }
    }

    /// The rendered tile content: a unit-bearing caption, the number-only value, and whether to
    /// prefix an up-triangle (the climb fields). Route-relative fields fall back to `--` with no
    /// route loaded. The unit lives in the caption so the big [`Display`](obc_render::text::Font)
    /// digits fit the half-width tile.
    pub fn cell(self, rx: &Render) -> StatCell {
        let units = rx.settings.units;
        let live = live_frac(rx);
        match self {
            StatField::Speed => {
                let v = rx.state.user_fix.and_then(|f| f.speed_mps).map(|mps| units.speed(mps * 3.6));
                StatCell::new(cap(units.speed_label(), ""), fmt_speed(v), false)
            }
            StatField::AvgSpeed => {
                let v = rx.activity.avg_kmh().map(|kmh| units.speed(kmh));
                StatCell::new(cap("AVG ", units.speed_label()), fmt_speed(v), false)
            }
            StatField::DistDone => StatCell::new(
                cap(units.dist_label(), " DONE"),
                fmt_km(units.dist(rx.activity.ridden_m / 1000.0)),
                false,
            ),
            StatField::DistToGo => {
                let to_go_m = rx.route.map_or(0, |r| r.total_distance_m).saturating_sub(rx.activity.progress_m);
                StatCell::new(cap(units.dist_label(), " TO GO"), fmt_km(units.dist(to_go_m as f32 / 1000.0)), false)
            }
            StatField::Climbed => {
                StatCell::new(cap("CLIMBED", ""), fmt_int(units.elev(rx.activity.climb_m()) as u32), true)
            }
            StatField::ToClimb => {
                let to_climb = match (rx.route, rx.profile) {
                    (Some(r), Some(p)) => r.total_ascent_m.saturating_sub(p.ascent_to(live)),
                    _ => 0,
                };
                StatCell::new(cap("TO CLIMB", ""), fmt_int(units.elev(to_climb as f32) as u32), true)
            }
            StatField::Grade => {
                let g = match (rx.route, rx.profile) {
                    (Some(r), Some(p)) => grade_at(p, r.total_distance_m, live),
                    _ => 0,
                };
                let mut value: heapless::String<8> = heapless::String::new();
                let _ = write!(value, "{g}%");
                StatCell::new(cap("GRADE", ""), value, false)
            }
            StatField::Elevation => {
                // The live barometric altitude, not the route profile — so it reads the current
                // height with no route loaded, and `--` until the first sample.
                let v = rx.activity.current_elevation_m().map(|m| units.elev(m));
                StatCell::new(cap("ELEV ", units.elev_label()), fmt_elev(v), false)
            }
            StatField::RideTime => StatCell::new(cap("RIDE", ""), fmt_hms(rx.activity.moving_s), false),
            StatField::Clock => {
                let mut value: heapless::String<8> = heapless::String::new();
                let _ = write!(value, "{:02}:{:02}", rx.now.hour, rx.now.minute);
                StatCell::new(cap("TIME", ""), value, false)
            }
        }
    }
}

/// The rendered content of one tile — caption (unit-bearing), number-only value, and the climb
/// up-triangle flag. Drawn by the Statistics screen's `tile`.
pub struct StatCell {
    pub caption: heapless::String<12>,
    pub value: heapless::String<8>,
    pub arrow: bool,
}

impl StatCell {
    fn new(caption: heapless::String<12>, value: heapless::String<8>, arrow: bool) -> Self {
        StatCell { caption, value, arrow }
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
    /// **two-span** field only lands where an *even number of single panels* precede it — so a wide
    /// tile always begins a row, hopping over a pair of singles (or one wide tile) per step.
    pub fn move_item(&mut self, i: usize, dir: i32) -> usize {
        let len = self.len as usize;
        if len == 0 || dir == 0 {
            return i.min(len.saturating_sub(1));
        }
        let i = i.min(len - 1);
        let f = self.ids[i];
        let step = dir.signum();
        // Candidate insertion indices in `dir`; a two-span field skips past any index whose
        // preceding single-panel count is odd (it would land mid-row).
        let mut p = i as i32;
        loop {
            let cand = p + step;
            if cand < 0 || cand as usize >= len {
                return i; // hit an end without a valid landing → no move
            }
            if f.span() == 1 || self.even_singles_before(cand as usize, i) {
                self.shift(i, cand as usize);
                return cand as usize;
            }
            p = cand;
        }
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

    /// Whether inserting the item currently at `from` at index `to` leaves an **even** number of
    /// single-span fields before it — i.e. a two-span field would begin a row there. Counts the
    /// singles among the other fields that would precede the insertion point.
    fn even_singles_before(&self, to: usize, from: usize) -> bool {
        let mut singles: usize = 0;
        let mut seen = 0; // positions filled by the *other* fields, in order
        for (k, &g) in self.as_slice().iter().enumerate() {
            if k == from {
                continue;
            }
            if seen == to {
                break;
            }
            if g.span() == 1 {
                singles += 1;
            }
            seen += 1;
        }
        singles.is_multiple_of(2)
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

/// Walk the selection into global slots, calling `visit(field, slot)` for each. A two-span field
/// that would start in the right column is bumped to the next row (leaving a one-slot gap), so it
/// never straddles a row — and, since rows align to the 6-slot page, never a page either. Returns
/// the total slots consumed (gaps included). Pure spine shared by [`page_count`] / [`page_fields`].
fn walk(list: &StatFieldList, mut visit: impl FnMut(StatField, usize)) -> usize {
    let mut slot = 0usize;
    for &f in list.as_slice() {
        if f.span() == 2 && !slot.is_multiple_of(COLS) {
            slot += 1; // defensive: a malformed list can't mis-render — the wide tile starts a row
        }
        visit(f, slot);
        slot += f.span() as usize;
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

// Value formatters + the grade helper — the field catalogue owns its own rendering. `grade_at` is
// shared with the Statistics header readout.

/// A km figure for a tile: one decimal up to 100 km, none past it, so the value stays ≤ 3 digits
/// and fits the half-width tile.
fn fmt_km(km: f32) -> heapless::String<8> {
    let mut s = heapless::String::new();
    let _ = if km >= 100.0 { write!(s, "{km:.0}") } else { write!(s, "{km:.1}") };
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
fn fmt_hms(secs: f32) -> heapless::String<8> {
    let total_min = (secs as u32) / 60;
    let mut s = heapless::String::new();
    let _ = write!(s, "{}:{:02}", total_min / 60, total_min % 60);
    s
}

/// Glue two caption fragments into a tile caption (e.g. `"AVG "` + `Units::speed_label()`),
/// keeping the unit label as the single source of truth.
fn cap(a: &str, b: &str) -> heapless::String<12> {
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
fn live_frac(rx: &Render) -> f32 {
    let a = rx.activity;
    if a.route_total_m == 0 {
        0.0
    } else {
        (a.progress_m as f32 / a.route_total_m as f32).clamp(0.0, 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    /// The Elevation tile reads the live barometric altitude, not the route profile: it shows the
    /// current height with no route loaded, converts to the active unit, and reads `--` before the
    /// first altimeter sample.
    #[test]
    fn elevation_tile_reads_live_barometric_altitude() {
        use crate::activity::{Activity, Mode};
        use crate::breadcrumb::Breadcrumb;
        use crate::settings::{DateTime, Settings, Units};
        use crate::AppState;
        use obc_render::{MapRenderer, NoopClock};

        let state = AppState::new(0, 0, 1.0);
        let breadcrumb = Breadcrumb::new();
        let mut renderer = MapRenderer::new();
        let now = DateTime::default();
        // A minimal `Render` for one `Elevation` cell — no route/profile, reading the live altitude
        // off `activity`.
        let value = |settings: &Settings, activity: &Activity, renderer: &mut MapRenderer| {
            let rx = Render {
                reader: None,
                renderer,
                state: &state,
                activity,
                settings,
                routes: &[],
                route: None,
                profile: None,
                breadcrumb: &breadcrumb,
                w: 240.0,
                h: 320.0,
                now_ms: 0,
                now,
                hold_progress: 0.0,
                no_fix: false,
                clock: &NoopClock,
            };
            StatField::Elevation.cell(&rx).value
        };

        let metric = Settings::default();
        let mut activity = Activity::new(Mode::Riding);
        assert_eq!(value(&metric, &activity, &mut renderer).as_str(), "--", "no altimeter sample yet");

        activity.record_altitude(144.0);
        assert_eq!(value(&metric, &activity, &mut renderer).as_str(), "144", "metric shows whole metres");

        let imperial = Settings { units: Units::Imperial, ..Settings::default() };
        // 144 m × 3.28084 ≈ 472.4 ft → rounds to 472.
        assert_eq!(value(&imperial, &activity, &mut renderer).as_str(), "472", "imperial converts to feet");
    }

    /// Discriminants round-trip through `from_u8`, and an unknown byte is dropped.
    #[test]
    fn discriminant_round_trips() {
        for f in StatField::ALL {
            assert_eq!(StatField::from_u8(f as u8), Some(f));
        }
        assert_eq!(StatField::from_u8(200), None, "an unknown discriminant is rejected");
    }

    /// An empty selection is still one page (drawing nothing), never zero — the `.max(1)` guard.
    #[test]
    fn empty_selection_is_one_page() {
        let l = list(&[]);
        assert_eq!(page_count(&l), 1);
        assert!(page_fields(&l, 0).is_empty());
    }
}
