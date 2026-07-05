//! The POI **list** screen — the distance-sorted nearest-16 of one category, reached from the
//! [category screen](super::PoiMenuScreen). Each row shows the POI name (or its subtype fallback
//! label when unnamed), a live **bearing arrow** relative to the rider's heading, and the
//! distance. `back` returns to the category list; selecting a POI is a no-op (open/navigate is a
//! follow-up epic — no dead hint row advertises it, per the copy-tone rule).
//!
//! # The static snapshot
//!
//! The list is a **static snapshot** taken once on entry (locked on epic #115): membership, order
//! and distances are frozen so rows never jump under the cursor and the SD isn't re-scanned per
//! frame. The catch is that the [`Reader`](obc_reader::Reader) the query needs lives only in the
//! **draw** context ([`Render::reader`]) — so the snapshot is taken **lazily on the first draw**
//! that has both a `Reader` and a fix, into a single [`PoiScratch`] the [`App`](crate::App) owns
//! (see the storage note on [`PoiScratch`]). Re-entering the screen re-queries: opening a POI list
//! [invalidates](PoiScratch::invalidate) the scratch, so the next draw re-runs the query.
//!
//! The one live element is the bearing arrow: recomputed every draw/animate tick from the stored
//! coordinates and the rider's current heading — pure trig, **zero SD**.

use embedded_graphics::prelude::Point;
use obc_reader::{cos_lat, label_of, Poi, PoiCategory, MAX_POI_RESULTS};
use obc_render::{
    text::{Font, TextAlign},
    Surface,
};

use crate::hal::Fix;
use crate::input::Gesture;
use crate::settings::Units;

use super::list::{self, ListGeometry, Separators};
use super::{palette, Ctx, PoiDetailScreen, Render, Screen, Transition};

/// Per-POI row height — two lines (name above, bearing arrow + distance below) with margin to keep
/// the distance clear of the row separator / selected-row fill; the nearest-16 still page through
/// the list widget.
const ROW_H: i32 = 64;

/// The [`App`](crate::App)-owned snapshot of one category's nearest-16. **One** buffer, shared by
/// whatever POI-list screen is on top — never owned by the screen variant.
///
/// # Storage decision (issue #425)
///
/// A `heapless::Vec<Poi, 16>` is ~776 B. The [`Screen`](super::Screen) enum is a union sized to its
/// largest variant (measured 40 B without this) held in a `Vec<Screen, MAX_DEPTH=8>` in `.bss`, so
/// an inline snapshot would inflate **every** stack slot: 8 × ~784 B ≈ 6.3 KB resident. Held once in
/// `App` it costs the buffer **once** (~800 B). Only one POI list is ever visible, and the
/// static-snapshot contract already forbids two live snapshots, so the single buffer loses nothing.
pub struct PoiScratch {
    /// The category the current snapshot is for once a query has run — `Some` even when the result
    /// is empty (so the screen can tell "queried, empty category" from "not queried yet"). `None`
    /// on a fresh/invalidated scratch.
    taken_for: Option<PoiCategory>,
    /// The nearest-16 for [`taken_for`](PoiScratch::taken_for), ascending by distance. Frozen once
    /// filled; the query owns the ordering.
    pois: heapless::Vec<Poi, MAX_POI_RESULTS>,
}

impl PoiScratch {
    /// An empty scratch — no snapshot taken yet.
    pub const fn new() -> Self {
        PoiScratch { taken_for: None, pois: heapless::Vec::new() }
    }

    /// Drop any snapshot so the next POI-list draw re-queries. Called when a POI list screen opens,
    /// so re-entering a category always takes a fresh snapshot at the current fix.
    pub fn invalidate(&mut self) {
        self.taken_for = None;
        self.pois.clear();
    }

    /// Whether a query for `category` has already run (a snapshot is present — possibly empty).
    /// Read by the host reader-build seam ([`App::base_needs_reader`](crate::App::base_needs_reader))
    /// as well as the screen's own draw.
    pub(crate) fn holds(&self, category: PoiCategory) -> bool {
        self.taken_for == Some(category)
    }

    /// Number of POIs currently snapshotted (0 when none taken). Introspection for
    /// [`App::poi_snapshot_len`](crate::App::poi_snapshot_len).
    pub(crate) fn len(&self) -> usize {
        self.pois.len()
    }

    /// The snapshotted POI at `index` (ascending by distance), or `None` past the end. The POI
    /// list's `Gesture::Press` reads it through [`Ctx`](super::Ctx) to hand the selected `Poi` to the
    /// detail screen — the one place `handle` reaches the draw-taken snapshot (the query itself still
    /// only runs at draw).
    pub(crate) fn get(&self, index: usize) -> Option<&Poi> {
        self.pois.get(index)
    }
}

impl Default for PoiScratch {
    fn default() -> Self {
        PoiScratch::new()
    }
}

/// The POI list. State is the browsed category and the highlighted row; the snapshot itself lives
/// in the [`App`](crate::App)-owned [`PoiScratch`], keyed by category and taken lazily on draw.
#[derive(Debug)]
pub struct PoiListScreen {
    category: PoiCategory,
    selected: usize,
}

impl PoiListScreen {
    /// Open the list for `category`. The caller ([`PoiMenuScreen`](super::PoiMenuScreen)) also
    /// [invalidates](PoiScratch::invalidate) the App scratch on this transition, so the first draw
    /// re-queries even when re-entering the same category.
    pub fn new(category: PoiCategory) -> Self {
        PoiListScreen { category, selected: 0 }
    }

    /// The category this list browses — read by the host reader-build seam
    /// ([`App::base_needs_reader`](crate::App::base_needs_reader)) to check the scratch.
    pub(crate) fn category(&self) -> PoiCategory {
        self.category
    }

    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        match g {
            // The row count isn't known here (the scratch is in the draw ctx), so wrap over the full
            // 16 cap; `draw` clamps the selection to the real length, so a turn past the end is
            // harmless. A short list still highlights a valid row.
            Gesture::Turn(n) => list::on_turn(&mut self.selected, n, MAX_POI_RESULTS),
            // Open the detail screen for the highlighted POI (epic #439 P4 #444). The snapshot is
            // taken at draw, so it lives in the App-owned scratch `cx` carries read-only; clamp the
            // selection to the real length (a turn can wrap past a short list). An empty scratch
            // (never drawn / no fix) ⇒ `get` is `None` ⇒ nothing to open, stay put.
            Gesture::Press => match cx.poi_scratch.get(self.selected.min(cx.poi_scratch.len().saturating_sub(1))) {
                Some(poi) => Transition::Push(Screen::PoiDetail(PoiDetailScreen::new(poi.clone()))),
                None => Transition::None,
            },
            Gesture::Back => Transition::Pop, // return to the category list
            _ => Transition::None,
        }
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        // Lazily take the snapshot on the first draw that has a Reader **and** a fix, into the
        // App-owned scratch. A no-op once the scratch already holds this category.
        self.ensure_snapshot(rx);

        let (w, h) = (rx.w, rx.h);
        let queried = rx.poi_scratch.holds(self.category);
        let pois: &[Poi] = if queried { &rx.poi_scratch.pois } else { &[] };
        let total = pois.len();

        let geo = ListGeometry::below_title(w, h, ROW_H, 6, 14, Separators::All);
        let pos = if total == 0 { 0 } else { self.selected.min(total - 1) + 1 };
        list::list_frame(cv, w, h, self.category.name(), pos, total, geo.visible);

        if total == 0 {
            // "No position" when a snapshot could never be taken (no fix ever); once a query has run
            // against a fix, "No POIs in this map" for a genuinely empty category. Before the first
            // query (no fix yet, not queried) draw nothing — a transient one-frame state.
            if rx.state.user_fix.is_none() {
                super::empty_state(cv, w, h, "No position", "No GPS fix yet");
            } else if queried {
                // Body title fits ~16 chars on the 240 px panel — keep it short; the hint carries
                // the "in this map" scope the epic's wording wants.
                super::empty_state(cv, w, h, "No POIs nearby", "None in this map");
            }
            return;
        }

        // The heading reference for the bearing arrows: GPS course while moving, compass while
        // stopped (the #231 seam). `None` ⇒ hide the arrows rather than point wrong.
        let heading = rx.state.effective_heading_deg();
        let fix = rx.state.user_fix; // present here (a snapshot exists ⇒ there was a fix)
        let units = rx.settings.units;

        let sel = self.selected.min(total - 1);
        let first = list::window_start(sel, geo.visible, total) as i32;
        list::draw_rows(cv, geo, total, sel, first, |cv, row| {
            draw_poi_row(cv, &pois[row.index], row.area, w, fix, heading, units);
        });
    }

    /// Fill the [`App`](crate::App)-owned scratch with this category's nearest-16 on the first draw
    /// that has both a `Reader` and a fix — then never again (the scratch already `holds` the
    /// category). A re-entry invalidated the scratch in [`PoiMenuScreen`](super::PoiMenuScreen), so
    /// it re-queries.
    ///
    /// Runs in the **draw** path — the only place [`Render::reader`] exists — writing solely to the
    /// `&mut` [`Render::poi_scratch`] (no screen mutation, so `draw` stays `&self`). On a host that
    /// skips building the `Reader` on a non-map frame,
    /// [`base_needs_reader`](crate::App::base_needs_reader) keeps `rx.reader` `Some` here until the
    /// snapshot lands.
    fn ensure_snapshot(&self, rx: &mut Render) {
        if rx.poi_scratch.holds(self.category) {
            return; // already queried this category
        }
        let (Some(reader), Some(fix)) = (rx.reader, rx.state.user_fix) else {
            return; // no map or no fix yet — retry next draw (the empty state covers "no fix ever")
        };
        // `nearest_pois` takes `pos` as (lon, lat) µdeg — pass the fix in that order.
        let _ = reader.nearest_pois(self.category, (fix.lon, fix.lat), &mut rx.poi_scratch.pois);
        rx.poi_scratch.taken_for = Some(self.category);
    }
}

/// Draw one POI row on two lines: the name (or its subtype fallback label) on top, in the row's
/// prominent Body type across the full width; the live bearing arrow + the distance below it, in
/// muted Label type. Giving the name its own line is what lets a real POI name fit instead of a
/// truncated stub. `fix`/`heading` drive the live arrow; `units` scales the distance.
fn draw_poi_row(
    cv: &mut impl Surface,
    poi: &Poi,
    area: embedded_graphics::primitives::Rectangle,
    w: i32,
    fix: Option<Fix>,
    heading: Option<f32>,
    units: Units,
) {
    use palette::*;
    let x = area.top_left.x + 8;
    let top = area.top_left.y;

    // Line 1 — the name, the row's primary element, now on its own full-width line so most names
    // fit whole (only a genuinely long one still gets the ".." from `fit`).
    let name = if poi.name.is_empty() { label_of(poi.subtype).unwrap_or("POI") } else { poi.name.as_str() };
    let name_top = top + 6;
    let name_max = ((w - x - 12) / Font::Body.char_width() as i32).max(6) as usize;
    cv.text(&fit(name, name_max), Point::new(x, name_top), Font::Body, TextAlign::Left, INK);

    // Line 2 — bearing arrow + distance, secondary (smaller, muted), stacked under the name.
    let line2_top = name_top + Font::Body.cap_height() as i32 + 4;
    let mut dist: heapless::String<12> = heapless::String::new();
    super::write_off_route(&mut dist, "", poi.distance_m, units);
    let mut text_x = x;
    // Arrow at the left of line 2 (only when a heading reference exists — else hidden), distance
    // just to its right.
    if let (Some(fix), Some(heading)) = (fix, heading) {
        let arrow_mid = line2_top + Font::Label.cap_height() as i32 / 2;
        draw_bearing_arrow(cv, Point::new(x + ARROW_R, arrow_mid), (fix.lon, fix.lat), (poi.lon, poi.lat), heading);
        text_x = x + 2 * ARROW_R + 8;
    }
    cv.text(&dist, Point::new(text_x, line2_top), Font::Label, TextAlign::Left, SUBTEXT);
}

/// Half-size of the bearing-arrow glyph (px) — a chevron ~`2 * ARROW_R` across, ~2 chars wide.
/// Shared with the [detail screen](super::PoiDetailScreen), which draws the same live arrow.
pub(super) const ARROW_R: i32 = 7;

/// Draw a 16-direction bearing chevron centred at `c`, pointing from the rider toward the POI
/// **relative to the rider's heading** — a filled triangular arrowhead like the user-position
/// marker, not a font glyph. `pos`/`poi` are `(lon, lat)` µdeg; `heading_deg` is CW from north.
///
/// Geometry: the true bearing to the POI is `atan2(east, north)` (CW from north) with the east
/// component scaled by `cos_lat` (local-equirectangular, matching the reader's distance metric);
/// subtracting the heading gives the on-screen angle, where 0° points up and clockwise is positive,
/// so the unit direction is `(sin θ, -cos θ)`.
///
/// Shared with the [detail screen](super::PoiDetailScreen) so the one arrow trig lives in one place.
pub(super) fn draw_bearing_arrow(cv: &mut impl Surface, c: Point, pos: (i32, i32), poi: (i32, i32), heading_deg: f32) {
    let dlat = (poi.1 - pos.1) as f32;
    let dlon = (poi.0 - pos.0) as f32;
    let east = dlon * cos_lat(pos.1);
    // atan2(east, north): 0 = due north, +east (clockwise) positive — same convention as `heading`.
    let bearing = libm::atan2f(east, dlat);
    let theta = bearing - heading_deg.to_radians();
    let (ux, uy) = (libm::sinf(theta), -libm::cosf(theta)); // screen: 0° up, CW positive
    let (px, py) = (-uy, ux); // perpendicular = arrowhead base spread

    let r = ARROW_R as f32;
    let tip = Point::new(c.x + (ux * r) as i32, c.y + (uy * r) as i32);
    // Base corners behind the centre, spread to ~0.75 r — a compact solid arrowhead.
    let back = (-r * 0.6, r * 0.75);
    let bl = Point::new(c.x + (ux * back.0 - px * back.1) as i32, c.y + (uy * back.0 - py * back.1) as i32);
    let br = Point::new(c.x + (ux * back.0 + px * back.1) as i32, c.y + (uy * back.0 + py * back.1) as i32);
    cv.triangle(tip, bl, br, palette::WOOD);
}

/// Fit `s` into `max` chars, appending ".." when truncated (no ellipsis glyph). Truncates on a char
/// boundary. A local twin of the Route menu's `fit_name`, capped for a POI name (≤ 20 bytes).
fn fit(s: &str, max: usize) -> heapless::String<24> {
    let mut out = heapless::String::new();
    if s.chars().count() <= max {
        let _ = out.push_str(s);
    } else {
        for ch in s.chars().take(max.saturating_sub(2)) {
            let _ = out.push(ch);
        }
        let _ = out.push_str("..");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Screen-space unit direction of the bearing arrow for a POI at `(poi_lon, poi_lat)` seen from
    /// `pos` with `heading_deg` — the same math `draw_bearing_arrow` renders, exposed for the
    /// geometry tests. `(0, -1)` is up, `(1, 0)` is right.
    fn arrow_dir(pos: (i32, i32), poi: (i32, i32), heading_deg: f32) -> (f32, f32) {
        let dlat = (poi.1 - pos.1) as f32;
        let dlon = (poi.0 - pos.0) as f32;
        let east = dlon * cos_lat(pos.1);
        let bearing = libm::atan2f(east, dlat);
        let theta = bearing - heading_deg.to_radians();
        (libm::sinf(theta), -libm::cosf(theta))
    }

    fn approx(a: (f32, f32), b: (f32, f32)) {
        assert!((a.0 - b.0).abs() < 0.02 && (a.1 - b.1).abs() < 0.02, "dir {a:?} != {b:?}");
    }

    /// A POI due north with a north-facing heading points straight up.
    #[test]
    fn due_north_north_course_points_up() {
        let pos = (7_000_000, 43_000_000);
        let north = (7_000_000, 43_010_000);
        approx(arrow_dir(pos, north, 0.0), (0.0, -1.0));
    }

    /// The arrow rotates with the heading: same POI due north, but facing east ⇒ it points to the
    /// rider's left (west on screen), and facing south ⇒ it points down.
    #[test]
    fn arrow_rotates_with_heading() {
        let pos = (7_000_000, 43_000_000);
        let north = (7_000_000, 43_010_000);
        approx(arrow_dir(pos, north, 90.0), (-1.0, 0.0)); // heading east ⇒ north is on the left
        approx(arrow_dir(pos, north, 180.0), (0.0, 1.0)); // heading south ⇒ north is behind (down)
        approx(arrow_dir(pos, north, 270.0), (1.0, 0.0)); // heading west ⇒ north is on the right
    }

    /// A POI due east with a north heading points right; the cos_lat scaling doesn't rotate a
    /// pure-east or pure-north bearing (only mixes the diagonal).
    #[test]
    fn due_east_north_course_points_right() {
        let pos = (7_000_000, 43_000_000);
        let east = (7_010_000, 43_000_000);
        approx(arrow_dir(pos, east, 0.0), (1.0, 0.0));
    }
}
