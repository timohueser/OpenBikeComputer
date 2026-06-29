//! The screen system — the modular core of the on-device UI.
//!
//! This is the architecture the brief calls for: `no_std`, zero-alloc, **no
//! retained widget tree**. Screens are an [`Screen`] enum dispatched by `match`
//! (static dispatch, no `dyn`), each variant a small module with typed state.
//! Navigation is a *return value* — a screen's [`handle`](Screen::handle) returns
//! a [`Transition`] that [`apply`] runs against a [`heapless::Vec`] stack, so
//! overlays Push and `back`/Resume Pop back to their caller automatically and the
//! `back` that pops the top is the guaranteed escape.
//!
//! **Adding a screen is a local edit** (the modularity test): add a module with a
//! state struct + `handle`/`draw`, add one [`Screen`] variant + its three match
//! arms, and `Push` it from wherever it's reached. No central dispatch table, no
//! trait objects, no allocation.
//!
//! The shared context is split by role: [`Ctx`] is the *logic* half handed to
//! `handle` (the mutable camera/mode + clock), [`Render`] is the *draw* half (the
//! read-only state plus the `Reader`, the reusable `MapRenderer`, and the
//! in-flight hold-progress for the confirm ring).

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
use crate::settings::{Settings, Units};

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
pub use settings::{DateTimeScreen, PowerScreen, ResetScreen, SettingsScreen, UnitsScreen};
pub use statistics::StatisticsScreen;

/// Maximum overlay depth (Home → Map → Ride control / Menu → …). Sized with
/// headroom; the real flow never nests more than a few deep. Growing it costs a
/// few enum-sized slots of static RAM, nothing per frame.
pub const MAX_DEPTH: usize = 8;

/// The screen stack: the bottom is the always-present root (Home), the top is the
/// screen currently receiving input.
pub type Stack = heapless::Vec<Screen, MAX_DEPTH>;

/// What a screen's [`handle`](Screen::handle) asks the navigation stack to do
/// next; [`apply`] runs it. This small closed set covers the whole spec's flow.
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
    /// Truncate to the Home root and push `screen`, landing on `[Home, screen]` from any
    /// depth — "load a route and go ride it", reachable through Home or the Menu, and from
    /// the route-swap prompt. Lands on a clean `[Home, Map]` instead of leaving stale Menu /
    /// Route-menu screens buried under the new Map.
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
            let _ = stack.push(s); // MAX_DEPTH has headroom; an overflow just no-ops
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

/// Logic context handed to [`Screen::handle`]: the mutable app state a screen
/// adjusts (camera + mode) plus the millis clock. The render half is [`Render`].
pub struct Ctx<'a> {
    /// The camera / orientation / last-fix state (a screen may zoom, pan, …).
    pub state: &'a mut AppState,
    /// The ride mode + tracking accumulators.
    pub activity: &'a mut Activity,
    /// The persisted device settings — the settings screens edit this in place; a change is
    /// detected by [`App::apply_gesture`](crate::App::apply_gesture) (a single `==`) and
    /// flagged for the host to save. Every other screen leaves it untouched.
    pub settings: &'a mut Settings,
    /// The route catalog (read-only) — the Route menu navigates it and centers the
    /// camera on the picked route's bbox from here, no I/O needed.
    pub routes: &'a [RouteSummary],
    /// Current millis clock.
    pub now_ms: u32,
}

/// Render context handed to [`Screen::draw`]: the read-only state plus the map
/// `Reader`, the reusable `MapRenderer`, and the in-flight encoder hold-progress
/// (0.0–1.0) the guarded-action confirm ring fills with.
pub struct Render<'a, 'd> {
    /// The streamed-map `Reader` — **`None` when the base screen doesn't draw the map** (a menu,
    /// the Statistics view, Home). Only the [`Map`](crate::screen::map) screen reads it, so a host
    /// can skip building the `Reader` (its SD style-table parse + stack spike) entirely on a non-map
    /// frame and pass `None`; every other screen ignores it. The single-target convenience entries
    /// [`render_map`](crate::App::render_map) / [`render_frame`](crate::App::render_frame) always
    /// pass `Some` (their callers are drawing the map).
    pub reader: Option<&'a Reader<'d>>,
    pub renderer: &'a mut MapRenderer,
    pub state: &'a AppState,
    pub activity: &'a Activity,
    /// The persisted device settings (read-only here) — the riding views read
    /// [`units`](Settings::units) to caption + scale their readouts, and the settings screens
    /// draw their current values from it.
    pub settings: &'a Settings,
    /// The route catalog, for the Route-menu list.
    pub routes: &'a [RouteSummary],
    /// The active route's geometry (the Map strokes it), or `None` when no route is
    /// loaded. Host-owned, streamed on demand; only the active route is open.
    pub route: Option<&'a RouteReader<'a>>,
    /// The active route's elevation profile (the Elevation screen draws it), rebuilt by
    /// the app on route load and cached — `None` when no route is loaded. Resident, so
    /// the screen never re-reads the route to draw.
    pub profile: Option<&'a Profile>,
    /// The travelled-path breadcrumb (bounded RAM); the Map strokes it under the route. Empty
    /// when nothing has been recorded yet, so the Map can skip it with [`Breadcrumb::is_empty`].
    pub breadcrumb: &'a Breadcrumb,
    pub w: f32,
    pub h: f32,
    pub now_ms: u32,
    pub hold_progress: f32,
    /// Microsecond clock for the map render's per-stage timing (collect / sort / draw), passed
    /// straight to [`MapRenderer::render_timed`] by the Map screen. Hosts that don't profile pass
    /// [`NoopClock`](obc_render::NoopClock) (via [`App::render_map`]); the device passes its
    /// `Instant`-based clock (via [`App::render_map_timed`]). Part of the strippable
    /// render-instrumentation seam.
    pub clock: &'a dyn Clock,
}

/// The on-device screens. Each variant owns its typed state and forwards the
/// contract to that screen's inherent `handle`/`draw`. Adding a screen = one
/// variant + three arms here + its module.
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

    /// Advance this screen's **time-driven** content one frame, returning whether the drawn
    /// output changed so the render-on-demand host marks the map dirty (issue #47). Most
    /// screens change only on input or a fresh fix and return `false`; the Statistics view's
    /// cursor springs back to the live position on an idle timer — a change driven by neither
    /// input nor a fix — so it reports that here. The host calls this each frame on every drawn
    /// screen; a future clock/battery readout would hook in the same way (a small region it
    /// owns, not the whole map, ticking on its own interval).
    pub fn animate(&mut self, now_ms: u32) -> bool {
        match self {
            Screen::Statistics(s) => s.animate(now_ms),
            _ => false,
        }
    }
}

/// Height of the wood title bar. Sized for the Body-tier title (28 px cell, ≈18 px caps)
/// with even ≈8 px padding above and below.
pub const TITLE_BAR_H: i32 = 34;

/// Top of the list area (just below the title bar) shared by list screens.
pub const LIST_TOP: i32 = TITLE_BAR_H + 8;

/// Draw the shared screen chrome: a full-screen near-white background (the housing
/// rounds the physical corners, so the panel goes edge to edge), a thin rounded
/// outline, and a rounded wood title bar with `title` **left-aligned** and `right`
/// (a counter, a grade readout, …) right-justified. The title is left-aligned rather
/// than centered so a long right-hand readout (e.g. the Statistics grade / off-route
/// distance) never collides with it at the bigger Terminus glyph sizes. Every framed
/// screen — the Menu, the Route menu, the Elevation profile — draws its header through
/// this, so they stay visually identical; the caller fills the body below [`LIST_TOP`].
pub fn title_frame<D, F>(cv: &mut Canvas<D, F>, w: i32, h: i32, title: &str, right: &str)
where
    D: DrawTarget,
    F: Fn(u16) -> D::Color,
{
    use palette::*;
    cv.clear(PARCHMENT);
    cv.round_outline(rect(4, 4, w - 8, h - 8), 8, WOOD_LIGHT);
    cv.round(rect(4, 4, w - 8, TITLE_BAR_H), 6, WOOD);
    // Both rows vertically centered in the bar (centre y ≈ 21): the Body title's caps sit
    // ≈4..22 px below its cell top, the Label readout's ≈4..19, so these y's align them.
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

/// First visible index of a scrolling list that keeps `selected` on screen within
/// `visible` rows of `total` items. Stateless — a pure function of the selection —
/// so list screens need no scroll state: the highlight moves down to the last
/// visible row, then the window follows it (and wrapping to either end lands on
/// the first/last page).
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

/// The gestures the two **riding views** (Map and Statistics) bind identically:
/// `press` pauses tracking and opens the Ride-control overlay, and `back-hold` opens
/// the Menu. Each riding screen calls this from its `Press | BackHold` arm, so the
/// shared navigation lives in one place (and a future riding view inherits it for
/// free) while the screen keeps its own `turn` / `back` / `hold`.
pub(crate) fn riding_common(g: Gesture, cx: &mut Ctx) -> Transition {
    match g {
        Gesture::Press => {
            // Pause: tracking stops and the Ride-control overlay opens over the view.
            cx.activity.mode = Mode::Paused;
            Transition::Push(Screen::RideControl(RideControl::new()))
        }
        Gesture::BackHold => Transition::Push(Screen::Menu(MenuScreen::new())),
        _ => Transition::None, // only ever called for the two arms above
    }
}

/// Advance a **wrapping** list selection by `n` detents over `len` items — the
/// `turn`-moves-the-highlight every list screen shares (Menu, Route menu, Ride
/// control). Wraps at both ends; a no-op on an empty list.
pub(crate) fn step_selection(selected: usize, n: i32, len: usize) -> usize {
    if len == 0 {
        return selected;
    }
    (selected as i32 + n).rem_euclid(len as i32) as usize
}

/// Draw a centered two-line **empty state** — a bold `title` over a muted `hint`,
/// vertically centered — the shared "nothing to show yet" body the Route menu (no
/// routes) and Statistics (no route loaded) both draw under their header.
pub(crate) fn empty_state<D, F>(cv: &mut Canvas<D, F>, w: i32, h: i32, title: &str, hint: &str)
where
    D: DrawTarget,
    F: Fn(u16) -> D::Color,
{
    cv.text(title, Point::new(w / 2, h / 2 - 28), Font::Body, TextAlign::Center, palette::INK);
    cv.text(hint, Point::new(w / 2, h / 2 + 8), Font::Label, TextAlign::Center, palette::SUBTEXT);
}

/// Append a cross-track distance after `prefix`, compacted to a whole large unit past the
/// cross-over so the readout stays within the panel width (a long "...14515m" would overrun).
/// In **metric**: `"<prefix>NNNm"` below 1 km, `"<prefix>NNkm"` above, rounded to the nearest
/// km. In **imperial**: `"<prefix>NNNft"` below a mile, `"<prefix>NNmi"` above. Shared by the
/// Statistics header readout and the Map's off-route pill so the two agree.
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

/// Draw a selected option row's **background** for the guarded-action menus (Ride control,
/// Route swap): a plain `AMBER` fill for an instant option, or — when `guard` is set — a
/// `PARCHMENT_SHADE` base that fills in `fill` (amber to confirm a save, warning-red for a
/// destructive action) tracking the encoder `hold_progress` (0.0–1.0). `radius` is the
/// corner radius; the caller draws the row's label text. A no-op for an unselected row.
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

/// The "explorer's field map" palette in RGB565 (the format/style color space),
/// so screen text and chrome quantize through the host `color_fn` exactly like
/// map styles.
///
/// **Tuned to the 64-color (RGB222) gamut.** The panel only has 4 levels per
/// channel (0/85/170/255), so each value below is chosen for the *quantized*
/// result, with the device-64 color noted. (Earlier, true-color-picked values
/// clipped: parchment → white, the tan accents → pink.) The trailing comments are
/// the device-64 RGB each value lands on; `tests/palette.rs` asserts every one
/// through `rgb565_to_device64`, so a retune that forgets to update a comment fails
/// the build rather than drifting silently.
pub mod palette {
    /// Pack 8-bit RGB into RGB565.
    pub const fn rgb565(r: u8, g: u8, b: u8) -> u16 {
        (((r as u16) >> 3) << 11) | (((g as u16) >> 2) << 5) | ((b as u16) >> 3)
    }

    // Device-64 has no warm off-white — any blue < 192 tints it yellow ("stained
    // paper"). So the panel is a clean near-white; the wood frame + ink + amber
    // carry the warmth instead.
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
    /// Green — the "on" state of a settings toggle pill (black/ink = off). The one green on
    /// the panel, kept for this single semantic so it always reads as "enabled".
    pub const ON: u16 = rgb565(0, 170, 0); // → (0,170,0) green
    /// Magenta — the **planned route** line on the Map, the boldest thing on screen.
    /// The classic GPS route-line hue (Garmin / RideWithGPS / Komoot): it lands on no
    /// base-map feature (greens, azure water, greys, warm roads), so it always reads as
    /// "the line to follow". Route ahead = magenta, trail behind = the navy breadcrumb.
    pub const ROUTE: u16 = rgb565(255, 0, 255); // → (255,0,255) magenta
    /// Navy — the recorded **breadcrumb** (travelled path), stroked over the route and under
    /// the marker. A cool, recessive line so the trail behind reads quieter than the magenta
    /// route ahead, while staying clearly darker than the lighter azure water it may cross.
    pub const BREADCRUMB: u16 = rgb565(0, 0, 170); // → (0,0,170) navy
}

#[cfg(test)]
mod tests {
    use super::step_selection;

    // `step_selection` wrapping (issue #93 item 4).
    //
    // Every list screen (Menu, Route menu, Ride control) shares this `rem_euclid`-based
    // wrap (screen/mod.rs ~315). The existing suite only ever turns *within* the list; these
    // pin the behaviour when a turn crosses either end — exactly where a `%` regression
    // (which is negative for a backward turn at the top) would hand back a garbage index and
    // either highlight nothing or panic on the row lookup.

    /// Backward off the top: `Turn(-1)` from index 0 must wrap to the *last* item, not produce a
    /// negative index. `%` would give `-1`; `rem_euclid` gives `len - 1`. This is the menu's
    /// "scroll up past the top to reach the bottom" — a daily interaction.
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

    /// A multi-detent turn larger than the list (a fast flick of the encoder) must wrap cleanly,
    /// not run off the end. `Turn(n)` with `n > len` and `Turn(-n)` both land via the same modulo.
    #[test]
    fn step_selection_wraps_multiple_turns() {
        // From 0, +5 over a 3-item list: 5 mod 3 = 2.
        assert_eq!(step_selection(0, 5, 3), 2, "a long forward flick wraps modulo the length");
        // From 0, -5 over a 3-item list: rem_euclid keeps it in 0..3 → 1.
        assert_eq!(step_selection(0, -5, 3), 1, "a long backward flick wraps without going negative");
        // A full lap forward returns to the start.
        assert_eq!(step_selection(2, 3, 3), 2, "exactly one lap is a no-op");
    }

    /// An empty list (no routes on the SD card, an empty menu) is a no-op for *any* turn — the
    /// `len == 0` guard must short-circuit before the `% 0` that would otherwise panic. Press is
    /// already covered elsewhere; this pins the Turn path the issue called out.
    #[test]
    fn step_selection_on_empty_list_is_a_noop() {
        assert_eq!(step_selection(0, 1, 0), 0, "a forward turn on an empty list stays at 0");
        assert_eq!(step_selection(0, -1, 0), 0, "a backward turn on an empty list stays at 0");
        assert_eq!(step_selection(7, 3, 0), 7, "the selection is returned unchanged, not modulo'd");
    }
}
