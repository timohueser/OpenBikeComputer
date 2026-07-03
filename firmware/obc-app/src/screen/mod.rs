//! The screen system — `no_std`, zero-alloc, no retained widget tree. Screens are a
//! [`Screen`] enum dispatched by `match` (static dispatch), each variant a small module
//! with typed state. Navigation is a return value: [`handle`](Screen::handle) returns a
//! [`Transition`] that [`apply`] runs against a [`heapless::Vec`] stack.
//!
//! The shared context is split by role: [`Ctx`] is the logic half handed to `handle`
//! (mutable camera/mode + clock), [`Render`] is the draw half (read-only state plus the
//! `Reader`, the reusable `MapRenderer`, and the in-flight hold-progress for the confirm ring).

use core::fmt::Write;

use embedded_graphics::{draw_target::DrawTarget, prelude::Point, primitives::Rectangle};
use obc_reader::Reader;
use obc_render::{
    rect,
    text::{Font, TextAlign},
    Canvas, Clock, MapRenderer, RenderStats,
};
use obc_route::{Profile, RouteReader};

use crate::activity::{Activity, Mode};
use crate::app::AppState;
use crate::breadcrumb::Breadcrumb;
use crate::input::Gesture;
use crate::route::RouteSummary;
use crate::settings::{DateTime, Settings, Units};

mod home;
mod map;
mod menu;
mod ride_control;
mod route_menu;
mod route_swap;
mod settings;
mod statistics;

pub use home::HomeScreen;
pub use map::MapScreen;
pub use menu::MenuScreen;
pub use ride_control::RideControl;
pub use route_menu::RouteMenuScreen;
pub use route_swap::RouteSwapScreen;
pub use settings::{
    AddFieldScreen, DateTimeScreen, PowerScreen, ResetScreen, SettingsScreen, StatFieldsScreen, StatsScreen,
    UnitsScreen,
};
pub use statistics::StatisticsScreen;

/// Maximum overlay depth. Sized with headroom; the real flow never nests more than a few deep.
pub const MAX_DEPTH: usize = 8;

/// The screen stack: the bottom is the always-present root (Home), the top is the
/// screen currently receiving input.
pub type Stack = heapless::Vec<Screen, MAX_DEPTH>;

/// What a screen's [`handle`](Screen::handle) asks the navigation stack to do next; [`apply`] runs it.
pub enum Transition {
    /// Stay on this screen (the gesture was handled in place, or is unbound).
    None,
    /// Open `screen` as the new top — a forward navigation or an overlay.
    Push(Screen),
    /// Return to the screen that opened this one — the `back` / Resume escape.
    Pop,
    /// Swap this screen for `screen` without growing the stack — sibling moves
    /// (Map ↔ Elevation) and "consume this screen" steps (Route menu → Map).
    Replace(Screen),
    /// Truncate to the Home root and push `screen`, landing on a clean `[Home, screen]` from any
    /// depth rather than leaving stale Menu / Route-menu screens buried under the new Map.
    Root(Screen),
    /// Clear every overlay back to the Home root — Finish / Discard / power-down.
    Home,
}

/// Apply a [`Transition`] to the stack. The root is never popped, so `back`
/// always has a defined target and the stack can never empty.
pub fn apply(stack: &mut Stack, t: Transition) {
    match t {
        Transition::None => {}
        Transition::Push(s) => {
            let _ = stack.push(s); // an overflow just no-ops
        }
        Transition::Pop => {
            if stack.len() > 1 {
                stack.pop();
            }
        }
        Transition::Replace(s) => {
            if let Some(top) = stack.last_mut() {
                *top = s;
            }
        }
        Transition::Root(s) => {
            stack.truncate(1); // keep the Home root
            let _ = stack.push(s);
        }
        Transition::Home => stack.truncate(1),
    }
}

/// Logic context handed to [`Screen::handle`]: the mutable app state a screen adjusts. The
/// render half is [`Render`].
pub struct Ctx<'a> {
    pub state: &'a mut AppState,
    pub activity: &'a mut Activity,
    /// The persisted device settings — the settings screens edit this in place; a change is
    /// detected by [`App::apply_gesture`](crate::App::apply_gesture) and flagged for the host
    /// to save. Every other screen leaves it untouched.
    pub settings: &'a mut Settings,
    pub routes: &'a [RouteSummary],
    pub now_ms: u32,
}

/// Render context handed to [`Screen::draw`]: the read-only state plus the map
/// `Reader`, the reusable `MapRenderer`, and the in-flight encoder hold-progress
/// (0.0–1.0) the guarded-action confirm ring fills with.
pub struct Render<'a, 'd> {
    /// The streamed-map `Reader` — `None` when the base screen doesn't draw the map (a menu, the
    /// Statistics view, Home). Only the [`Map`](crate::screen::map) screen reads it, so a host can
    /// skip building the `Reader` (its SD style-table parse + stack spike) on a non-map frame and
    /// pass `None`. [`render_map`](crate::App::render_map) / [`render_frame`](crate::App::render_frame)
    /// always pass `Some`.
    pub reader: Option<&'a Reader<'d>>,
    pub renderer: &'a mut MapRenderer,
    pub state: &'a AppState,
    pub activity: &'a Activity,
    /// The persisted device settings (read-only here) — the riding views read
    /// [`units`](Settings::units) to caption + scale their readouts.
    pub settings: &'a Settings,
    pub routes: &'a [RouteSummary],
    /// The active route's geometry (the Map strokes it), or `None` when no route is loaded.
    /// Host-owned, streamed on demand.
    pub route: Option<&'a RouteReader<'a>>,
    /// The active route's elevation profile (the Elevation screen draws it), rebuilt on route load
    /// and cached — `None` when no route is loaded. Resident, so the screen never re-reads to draw.
    pub profile: Option<&'a Profile>,
    /// The travelled-path breadcrumb (bounded RAM); the Map strokes it under the route. Empty when
    /// nothing has been recorded yet, so the Map can skip it with [`Breadcrumb::is_empty`].
    pub breadcrumb: &'a Breadcrumb,
    pub w: f32,
    pub h: f32,
    pub now_ms: u32,
    /// The live wall-clock time this frame (set-point advanced by elapsed millis — see
    /// [`WallClock`](crate::WallClock)). The Home screensaver draws it as `HH:MM`; for boot-relative
    /// millis a screen uses [`now_ms`](Render::now_ms) instead.
    pub now: DateTime,
    pub hold_progress: f32,
    /// No current GPS fix this frame: no fix yet (acquiring) or the last has gone stale (lost). The
    /// riding views draw the "No GPS Fix" banner when set, and the Map suppresses the off-route pill
    /// (the match is stale). Computed by [`App::has_live_fix`](crate::App::has_live_fix).
    pub no_fix: bool,
    /// Microsecond clock for the map render's per-stage timing, passed to
    /// [`MapRenderer::render_timed`]. Hosts that don't profile pass
    /// [`NoopClock`](obc_render::NoopClock); the device passes its `Instant`-based clock. Part of the
    /// strippable render-instrumentation seam.
    pub clock: &'a dyn Clock,
}

/// The on-device screens. Each variant owns its typed state and forwards to that screen's
/// inherent `handle`/`draw`.
pub enum Screen {
    Home(HomeScreen),
    Map(MapScreen),
    Statistics(StatisticsScreen),
    RideControl(RideControl),
    Menu(MenuScreen),
    RouteMenu(RouteMenuScreen),
    RouteSwap(RouteSwapScreen),
    Settings(SettingsScreen),
    DateTime(DateTimeScreen),
    Units(UnitsScreen),
    Stats(StatsScreen),
    StatFields(StatFieldsScreen),
    AddField(AddFieldScreen),
    Power(PowerScreen),
    Reset(ResetScreen),
}

impl Screen {
    /// Handle one gesture, returning the navigation [`Transition`] it triggers.
    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        match self {
            Screen::Home(s) => s.handle(g, cx),
            Screen::Map(s) => s.handle(g, cx),
            Screen::Statistics(s) => s.handle(g, cx),
            Screen::RideControl(s) => s.handle(g, cx),
            Screen::Menu(s) => s.handle(g, cx),
            Screen::RouteMenu(s) => s.handle(g, cx),
            Screen::RouteSwap(s) => s.handle(g, cx),
            Screen::Settings(s) => s.handle(g, cx),
            Screen::DateTime(s) => s.handle(g, cx),
            Screen::Units(s) => s.handle(g, cx),
            Screen::Stats(s) => s.handle(g, cx),
            Screen::StatFields(s) => s.handle(g, cx),
            Screen::AddField(s) => s.handle(g, cx),
            Screen::Power(s) => s.handle(g, cx),
            Screen::Reset(s) => s.handle(g, cx),
        }
    }

    /// Draw the screen. Returns the map [`RenderStats`] for the Map screen, and
    /// default stats for the others (so the host's stats panel keeps working).
    pub fn draw<D, F>(&self, target: &mut D, rx: &mut Render, color_fn: &F) -> RenderStats
    where
        D: DrawTarget,
        F: Fn(u16) -> D::Color,
    {
        match self {
            Screen::Home(s) => s.draw(target, rx, color_fn),
            Screen::Map(s) => s.draw(target, rx, color_fn),
            Screen::Statistics(s) => s.draw(target, rx, color_fn),
            Screen::RideControl(s) => s.draw(target, rx, color_fn),
            Screen::Menu(s) => s.draw(target, rx, color_fn),
            Screen::RouteMenu(s) => s.draw(target, rx, color_fn),
            Screen::RouteSwap(s) => s.draw(target, rx, color_fn),
            Screen::Settings(s) => s.draw(target, rx, color_fn),
            Screen::DateTime(s) => s.draw(target, rx, color_fn),
            Screen::Units(s) => s.draw(target, rx, color_fn),
            Screen::Stats(s) => s.draw(target, rx, color_fn),
            Screen::StatFields(s) => s.draw(target, rx, color_fn),
            Screen::AddField(s) => s.draw(target, rx, color_fn),
            Screen::Power(s) => s.draw(target, rx, color_fn),
            Screen::Reset(s) => s.draw(target, rx, color_fn),
        }
    }

    /// Whether this screen draws *over* the one below (the stack composites it on
    /// top) rather than replacing the view. Only Ride control is an overlay — it
    /// pauses on top of the still-visible map.
    pub fn is_overlay(&self) -> bool {
        matches!(self, Screen::RideControl(_))
    }

    /// Advance this screen's time-driven content one frame, returning whether the drawn output
    /// changed so the render-on-demand host marks the map dirty (issue #47). Most screens change
    /// only on input or a fresh fix and return `false`; the Statistics cursor springs back to live
    /// on an idle timer (off `now_ms`) and the Home clock ticks over each minute (off the wall-clock
    /// `now`), so those report it here.
    pub fn animate(&mut self, now_ms: u32, now: DateTime, settings: &Settings) -> bool {
        match self {
            Screen::Statistics(s) => s.animate(now_ms, settings),
            Screen::Home(s) => s.animate(now),
            _ => false,
        }
    }

    /// Milliseconds until this screen's next timed redraw, or `None` if it changes only on input /
    /// a fix. The event-driven host (issue #219) folds this across the visible stack into a single
    /// wake deadline so the M33 sleeps rather than free-running the loop. The mirror of
    /// [`animate`](Screen::animate). `ms_to_next_minute` is the wall-clock minute boundary the host
    /// pre-computes (it owns the clock); Home adopts it, Statistics reports its own input-clock deadlines.
    pub fn next_wake_in(&self, now_ms: u32, ms_to_next_minute: u32, settings: &Settings) -> Option<u32> {
        match self {
            Screen::Statistics(s) => s.next_wake_in(now_ms, settings),
            Screen::Home(_) => Some(ms_to_next_minute),
            _ => None,
        }
    }
}

/// Height of the wood title bar. Sized for the Body-tier title with even ≈8 px padding.
pub const TITLE_BAR_H: i32 = 34;

/// Top of the list area (just below the title bar) shared by list screens.
pub const LIST_TOP: i32 = TITLE_BAR_H + 8;

/// Draw the shared screen chrome: a near-white background, a thin rounded outline, and a rounded
/// wood title bar with `title` left-aligned and `right` (a counter, a grade readout, …) right-
/// justified. `title` is left-aligned so a long right-hand readout never collides with it. Every
/// framed screen draws its header through this; the caller fills the body below [`LIST_TOP`].
pub fn title_frame<D, F>(cv: &mut Canvas<D, F>, w: i32, h: i32, title: &str, right: &str)
where
    D: DrawTarget,
    F: Fn(u16) -> D::Color,
{
    use palette::*;
    cv.clear(PARCHMENT);
    cv.round_outline(rect(4, 4, w - 8, h - 8), 8, WOOD_LIGHT);
    cv.round(rect(4, 4, w - 8, TITLE_BAR_H), 6, WOOD);
    // Both rows vertically centered in the bar; the two y's account for the different glyph baselines.
    cv.text(title, Point::new(14, 8), Font::Body, TextAlign::Left, PARCHMENT);
    cv.text(right, Point::new(w - 14, 10), Font::Label, TextAlign::Right, PARCHMENT);
}

/// [`title_frame`] with a `pos / total` list counter on the right — the chrome the
/// Menu and Route menu share. The caller then draws its rows below [`LIST_TOP`].
pub fn list_frame<D, F>(cv: &mut Canvas<D, F>, w: i32, h: i32, title: &str, pos: usize, total: usize)
where
    D: DrawTarget,
    F: Fn(u16) -> D::Color,
{
    let mut counter: heapless::String<8> = heapless::String::new();
    let _ = write!(counter, "{pos} / {total}");
    title_frame(cv, w, h, title, &counter);
}

/// First visible index of a scrolling list that keeps `selected` on screen within `visible` rows
/// of `total` items. Stateless — a pure function of the selection — so list screens need no scroll
/// state: the highlight moves down to the last visible row, then the window follows it.
pub fn window_start(selected: usize, visible: usize, total: usize) -> usize {
    if total <= visible || selected < visible {
        0
    } else {
        (selected + 1 - visible).min(total - visible)
    }
}

/// Draw a list scrollbar — a faint track with a proportional thumb — at the right
/// edge, or nothing when everything fits. `top`/`height` is the windowed list
/// area; `first` is [`window_start`]'s result.
pub fn scrollbar<D, F>(cv: &mut Canvas<D, F>, x: i32, top: i32, height: i32, total: usize, first: usize, visible: usize)
where
    D: DrawTarget,
    F: Fn(u16) -> D::Color,
{
    if total <= visible || total == 0 {
        return;
    }
    cv.round(rect(x, top, 3, height), 1, palette::RULE);
    let thumb_h = (height * visible as i32 / total as i32).max(10);
    let thumb_y = top + height * first as i32 / total as i32;
    cv.round(rect(x, thumb_y, 3, thumb_h), 1, palette::WOOD);
}

/// The gestures the two riding views (Map and Statistics) bind identically: `press` pauses
/// tracking and opens the Ride-control overlay, `back-hold` opens the Menu. Each riding screen
/// calls this from its `Press | BackHold` arm.
pub(crate) fn riding_common(g: Gesture, cx: &mut Ctx) -> Transition {
    match g {
        Gesture::Press => {
            cx.activity.mode = Mode::Paused;
            Transition::Push(Screen::RideControl(RideControl::new()))
        }
        Gesture::BackHold => Transition::Push(Screen::Menu(MenuScreen::new())),
        _ => Transition::None,
    }
}

/// Advance a wrapping list selection by `n` detents over `len` items. Wraps at both ends; a no-op
/// on an empty list.
pub(crate) fn step_selection(selected: usize, n: i32, len: usize) -> usize {
    if len == 0 {
        return selected;
    }
    (selected as i32 + n).rem_euclid(len as i32) as usize
}

/// Draw a centered two-line empty state — a bold `title` over a muted `hint` — the shared
/// "nothing to show yet" body the Route menu and Statistics draw under their header.
pub(crate) fn empty_state<D, F>(cv: &mut Canvas<D, F>, w: i32, h: i32, title: &str, hint: &str)
where
    D: DrawTarget,
    F: Fn(u16) -> D::Color,
{
    cv.text(title, Point::new(w / 2, h / 2 - 28), Font::Body, TextAlign::Center, palette::INK);
    cv.text(hint, Point::new(w / 2, h / 2 + 8), Font::Label, TextAlign::Center, palette::SUBTEXT);
}

/// Append a cross-track distance after `prefix`, compacted to a whole large unit past the cross-
/// over so the readout stays within the panel width. Metric: `NNNm` below 1 km, `NNkm` above
/// (rounded). Imperial: `NNNft` below a mile, `NNmi` above. Shared by the Statistics header readout
/// and the Map's off-route pill.
pub(crate) fn write_off_route<const N: usize>(s: &mut heapless::String<N>, prefix: &str, d_m: u32, units: Units) {
    use crate::settings::{FT_PER_M, FT_PER_MI};
    if units.is_imperial() {
        let ft = (d_m as f32 * FT_PER_M) as u32;
        if ft >= FT_PER_MI {
            let _ = write!(s, "{prefix}{}mi", (ft + FT_PER_MI / 2) / FT_PER_MI);
        } else {
            let _ = write!(s, "{prefix}{ft}ft");
        }
    } else if d_m >= 1000 {
        let _ = write!(s, "{prefix}{}km", (d_m + 500) / 1000);
    } else {
        let _ = write!(s, "{prefix}{d_m}m");
    }
}

/// One option in a guarded-action menu (Ride control, Route swap): a static label and a
/// `guard` flag marking the irreversible options that need a hold-to-confirm instead of a
/// plain press.
pub(crate) struct MenuItem {
    pub label: &'static str,
    pub guard: bool,
}

/// Draw a selected option row's background for the guarded-action menus: a plain `AMBER` fill for
/// an instant option, or — when `guard` is set — a `PARCHMENT_SHADE` base that fills in `fill`
/// tracking `hold_progress` (0.0–1.0). The caller draws the label. A no-op for an unselected row.
pub(crate) fn confirm_row<D, F>(
    cv: &mut Canvas<D, F>,
    row: Rectangle,
    selected: bool,
    guard: bool,
    hold_progress: f32,
    fill: u16,
    radius: u32,
) where
    D: DrawTarget,
    F: Fn(u16) -> D::Color,
{
    if !selected {
        return;
    }
    if guard {
        cv.round(row, radius, palette::PARCHMENT_SHADE);
        let fill_w = (row.size.width as f32 * hold_progress.clamp(0.0, 1.0)) as i32;
        if fill_w > 0 {
            cv.round(rect(row.top_left.x, row.top_left.y, fill_w, row.size.height as i32), radius, fill);
        }
    } else {
        cv.round(row, radius, palette::AMBER);
    }
}

/// The "explorer's field map" palette in RGB565, so screen text and chrome quantize through the
/// host `color_fn` exactly like map styles.
///
/// Tuned to the 64-color (RGB222) gamut: the panel has 4 levels per channel (0/85/170/255), so each
/// value is chosen for the *quantized* result. The trailing comment on each is the device-64 RGB it
/// lands on; `tests/palette.rs` asserts every one through `rgb565_to_device64`, so a retune that
/// forgets to update a comment fails the build.
pub mod palette {
    /// Pack 8-bit RGB into RGB565.
    pub const fn rgb565(r: u8, g: u8, b: u8) -> u16 {
        (((r as u16) >> 3) << 11) | (((g as u16) >> 2) << 5) | ((b as u16) >> 3)
    }

    // Device-64 has no warm off-white — any blue < 192 tints it yellow — so this is a clean
    // near-white; the wood frame + ink + amber carry the warmth instead.
    pub const PARCHMENT: u16 = rgb565(245, 243, 238); // → (255,255,255) white
    pub const PARCHMENT_SHADE: u16 = rgb565(180, 170, 105); // → (170,170,85) tan
    pub const HUD: u16 = rgb565(46, 37, 26); // → (0,0,0) near-black frame
    pub const WOOD: u16 = rgb565(150, 100, 40); // → (170,85,0) wood brown
    /// Lighter wood for inset borders / frame lines.
    pub const WOOD_LIGHT: u16 = rgb565(180, 168, 100); // → (170,170,85) tan
    pub const INK: u16 = rgb565(44, 33, 20); // → (0,0,0) text black
    /// Muted ink for secondary / sub-label text.
    pub const SUBTEXT: u16 = rgb565(110, 90, 58); // → (85,85,0) olive
    /// Hairline rule between list rows.
    pub const RULE: u16 = rgb565(180, 170, 100); // → (170,170,85) tan
    pub const AMBER: u16 = rgb565(227, 165, 43); // → (255,170,0) accent
    pub const WARNING: u16 = rgb565(192, 73, 46); // → (255,85,0) warning
    /// Faint neutral grey — the Home screensaver's contour lines and empty battery cells: dim
    /// enough to sit behind the clock, bright enough to read as fine topo lines.
    pub const CONTOUR: u16 = rgb565(96, 96, 96); // → (85,85,85) grey
    /// Green — the "on" state of a settings toggle pill (ink = off). The only green on the panel.
    pub const ON: u16 = rgb565(0, 170, 0); // → (0,170,0) green
    /// Magenta — the planned route line on the Map. The classic GPS route hue: it lands on no
    /// base-map feature, so it always reads as "the line to follow".
    pub const ROUTE: u16 = rgb565(255, 0, 255); // → (255,0,255) magenta
    /// Navy — the recorded breadcrumb (travelled path), stroked over the route and under the marker.
    /// Recessive so the trail behind reads quieter than the magenta route ahead.
    pub const BREADCRUMB: u16 = rgb565(0, 0, 170); // → (0,0,170) navy
}

#[cfg(test)]
mod tests {
    use super::step_selection;

    // `step_selection` wrapping: a `%` regression is negative for a backward turn at the top, which
    // would hand back a garbage index and highlight nothing or panic on the row lookup.

    /// Backward off the top: `Turn(-1)` from index 0 wraps to the last item, not a negative index.
    #[test]
    fn step_selection_wraps_backward_past_the_top() {
        assert_eq!(step_selection(0, -1, 4), 3, "up from the first item lands on the last");
        assert_eq!(step_selection(0, -1, 1), 0, "a single-item list stays put");
    }

    /// Forward off the bottom: `Turn(1)` from the last item wraps to the first.
    #[test]
    fn step_selection_wraps_forward_past_the_bottom() {
        assert_eq!(step_selection(3, 1, 4), 0, "down from the last item lands on the first");
    }

    /// A multi-detent turn larger than the list wraps cleanly, not off the end.
    #[test]
    fn step_selection_wraps_multiple_turns() {
        assert_eq!(step_selection(0, 5, 3), 2, "a long forward flick wraps modulo the length");
        assert_eq!(step_selection(0, -5, 3), 1, "a long backward flick wraps without going negative");
        assert_eq!(step_selection(2, 3, 3), 2, "exactly one lap is a no-op");
    }

    /// An empty list is a no-op for any turn — the `len == 0` guard must short-circuit before the
    /// `% 0` that would panic.
    #[test]
    fn step_selection_on_empty_list_is_a_noop() {
        assert_eq!(step_selection(0, 1, 0), 0, "a forward turn on an empty list stays at 0");
        assert_eq!(step_selection(0, -1, 0), 0, "a backward turn on an empty list stays at 0");
        assert_eq!(step_selection(7, 3, 0), 7, "the selection is returned unchanged, not modulo'd");
    }
}
