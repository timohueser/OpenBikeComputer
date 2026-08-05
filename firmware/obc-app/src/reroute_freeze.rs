//! The **Recalculating freeze** (issue #1146, P2): the map plane holds still while the host plans.
//!
//! A route search and a map render want the same RAM (the scratch arena's `render ⊥ nav` rule), and
//! the product rule that makes them disjoint is the one every commercial bike computer already
//! ships: while it recalculates, the map stops. So a live planner run engages a freeze in which
//!
//! - the host skips map redraws ([`App::reroute_freeze_active`](crate::App::reroute_freeze_active)),
//!   leaving the last frame on glass — a reflective panel keeps showing it for free;
//! - [`App::tick`](crate::App::tick) stops advancing route-match progress, so the guidance the
//!   frozen frame shows cannot drift away from it (fixes still record — breadcrumb, ride totals,
//!   altimeter, sensors — a freeze pauses the *map*, never the ride);
//! - a banner says so. A screen that stops responding without saying why reads as a crash, and the
//!   freeze lasts as long as the search does.
//!
//! # Why the base screen matters
//!
//! The freeze is engaged only when the base screen would actually draw a map. Planning from the
//! menus already renders no map — `NavPlanning` is an opaque chrome screen, so it *is* the base
//! while it is up — and freezing there would show a banner over the spinner that already says
//! "Planning...".
//!
//! The window that needs this is the **detour** path (#882), where the planning screen is *pushed
//! over a map base*: Back pops it while the host's planner is still running, and the next frame
//! would render the map straight into the arena the search still owns. One predicate covers both:
//! a live plan plus [`base_draws_map`](crate::App::base_draws_map).
//!
//! # The banner lives on the overlay plane
//!
//! Drawing it on the map plane would mean rendering the map — the exact thing the freeze forbids.
//! It is painted by [`App::render_overlay`](crate::App::render_overlay) instead, the cheap half that
//! composites over the still-visible frame, beside the long-press bulge.

use embedded_graphics::{draw_target::DrawTarget, prelude::Point};
use obc_render::{
    rect,
    text::{text_width, Font, TextAlign},
    Canvas, Surface,
};

use crate::screen::palette;

/// Banner height (px) — the map's status-chip height, so the two chrome pills read as one family.
const BANNER_H: i32 = 36;
/// Horizontal padding (px) around the copy, split either side — tighter than the status chip's 28,
/// because the copy is one long word: at 240 px the longest catalogued string ("Neuberechnung...")
/// would otherwise leave under 10 px of frame either side and read as a full-width bar.
const BANNER_PAD_X: i32 = 20;
/// Corner radius (px) — the shared pill radius.
const BANNER_RADIUS: u32 = 9;
/// Where the banner's top sits, as a fraction of frame height. A third of the way down: clear of
/// the top-centre clock, and well above the centred rider marker the rider is looking at (the map
/// under the banner is frozen, not gone — covering the marker would read as "lost").
const BANNER_Y_FRAC: f32 = 0.3;

/// Whether a host planner run is live — the freeze's whole state — plus the one bit that turns that
/// *level* into the repaint *edge* a render-on-demand host can act on.
///
/// The interesting part is where the plan flag is set and cleared: the app engages it when a plan
/// command is actually drained (the host will begin planning this pass) and releases it on the
/// answer, on the failure, and on a cancel drain. Anything else — a plan the rider cancelled before
/// the host ever saw it, a late answer whose screen is gone — must not leave it stuck, since a stuck
/// freeze is a map that never redraws again.
#[derive(Debug, Default)]
pub(crate) struct RerouteFreeze {
    plan_live: bool,
    /// Whether the *engaged* freeze — plan **and** map base — was already reported to the host by a
    /// [`take_engaged_edge`](RerouteFreeze::take_engaged_edge) drain. See that method: this is the
    /// difference between a banner that appears and one that is silently swallowed.
    engaged_shown: bool,
}

impl RerouteFreeze {
    pub(crate) const fn new() -> RerouteFreeze {
        RerouteFreeze { plan_live: false, engaged_shown: false }
    }

    /// A plan command was drained: the host begins a planner run this pass. Returns whether this
    /// *changed* the state, so the caller can repaint the overlay exactly on the edge.
    pub(crate) fn plan_started(&mut self) -> bool {
        !core::mem::replace(&mut self.plan_live, true)
    }

    /// The planner run is over — answered, failed, or cancelled. Idempotent (several of those edges
    /// legitimately land for one run: a cancel drains and the late answer arrives behind it).
    pub(crate) fn plan_ended(&mut self) -> bool {
        core::mem::replace(&mut self.plan_live, false)
    }

    /// Whether a planner run is live at all — true through a menu plan too, where no freeze is
    /// engaged. This is the "is the arena's nav arm claimed?" fact.
    pub(crate) fn plan_live(&self) -> bool {
        self.plan_live
    }

    /// Whether the freeze is **engaged**: a live plan *and* a base screen that would draw the map.
    pub(crate) fn active(&self, base_draws_map: bool) -> bool {
        self.plan_live && base_draws_map
    }

    /// The level→edge converter the host's once-per-frame dirty drain runs: `true` on the pass the
    /// *engaged* state flips, either way.
    ///
    /// **This is a level, and the plan flag alone is not it.** The plan's own start edge is useless
    /// to the banner, because the two facts the freeze is made of move independently: a plan drained
    /// under the opaque planning spinner engages nothing (chrome base), and the pass that puts a map
    /// base back under that still-running search raises no plan edge at all — it is a screen change.
    /// A host that keyed its overlay repaint on the plan edge would spend it on a chrome frame and
    /// then draw *nothing* for the rest of the search: the map plane is frozen, the overlay plane was
    /// never asked, and the last frame on glass belongs to a screen that is gone. Deriving the edge
    /// from the engaged level here means the banner lands whenever a frozen map is what the rider is
    /// actually looking at, however it got that way — and lands exactly **once**, so a freeze that
    /// spans hundreds of ride-loop passes costs one overlay repaint, not one per pass.
    pub(crate) fn take_engaged_edge(&mut self, base_draws_map: bool) -> bool {
        let now = self.active(base_draws_map);
        now != core::mem::replace(&mut self.engaged_shown, now)
    }
}

/// The banner's bounding rows `[y0, y0 + rows)` in a `h`-high frame — what a partial-overlay host
/// re-presents (the board pushes overlay rows, not whole frames).
pub(crate) fn banner_rows(h: f32) -> (u16, u16) {
    let y0 = (h * BANNER_Y_FRAC) as i32;
    let y0 = y0.clamp(0, (h as i32 - BANNER_H).max(0));
    (y0 as u16, BANNER_H.min(h as i32).max(0) as u16)
}

/// Draw the "Recalculating..." banner: a centred parchment pill with an ink outline and the copy in
/// ink — the calm chip idiom (the alert orange stays reserved for the No-GPS / off-route chip, which
/// is *below* on the frozen map plane and never collides with this band).
pub(crate) fn draw_banner<D, F>(target: &mut D, color_fn: &F, w: f32, h: f32, text: &str)
where
    D: DrawTarget,
    F: Fn(u16) -> D::Color,
{
    let (w, h) = (w as i32, h as i32);
    // [`Font::Label`], not the status chip's Body: the copy is a long word in every language
    // ("Neuberechnung...", "Recalculando..."), and the pill must keep a margin at 240 px.
    let font = Font::Label;
    let (y0, _) = banner_rows(h as f32);
    let pw = (text_width(text, font) as i32 + BANNER_PAD_X).min(w - 8);
    let px = (w - pw) / 2;
    let py = y0 as i32;
    let mut cv = Canvas::new(target, color_fn);
    cv.round(rect(px, py, pw, BANNER_H), BANNER_RADIUS, palette::PARCHMENT);
    cv.round_outline(rect(px, py, pw, BANNER_H), BANNER_RADIUS, palette::INK);
    cv.text(text, Point::new(w / 2, py + 5), font, TextAlign::Center, palette::INK);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The lifecycle in one test: nothing frozen at rest, engaged only where a map would be drawn,
    /// and released by whichever edge lands first.
    #[test]
    fn the_freeze_follows_the_plan_and_the_base_screen() {
        let mut f = RerouteFreeze::new();
        assert!(!f.plan_live());
        assert!(!f.active(true), "no plan, no freeze — the map renders normally");

        assert!(f.plan_started(), "the drain is the engaging edge");
        assert!(!f.plan_started(), "…and re-engaging is not an edge");
        assert!(f.plan_live());
        assert!(!f.active(false), "menu planning draws no map: nothing to freeze, no banner");
        assert!(f.active(true), "a plan over a map base is the freeze");

        assert!(f.plan_ended(), "the answer releases it");
        assert!(!f.active(true));
    }

    /// **The regression** a stuck freeze would be: the map never redraws again for the rest of the
    /// ride. Every release edge is idempotent, so the cancel drain and the late answer behind it can
    /// both fire, in either order, without leaving the flag inconsistent.
    #[test]
    fn releasing_twice_is_harmless_and_a_new_plan_re_engages() {
        let mut f = RerouteFreeze::new();
        assert!(!f.plan_ended(), "releasing what was never engaged is not an edge");
        assert!(f.plan_started());
        assert!(f.plan_ended());
        assert!(!f.plan_ended(), "the late answer behind the cancel is a no-op");
        assert!(!f.active(true));
        assert!(f.plan_started(), "and the next reroute freezes again");
        assert!(f.active(true));
    }

    /// **The regression** the engaged edge exists for: the plan's own start edge fires under the
    /// planning spinner, where nothing freezes — and the pass that puts a map base back under the
    /// still-live search raises no plan edge at all. A host keyed on the start edge would never be
    /// told to paint the banner for the whole of that search.
    #[test]
    fn the_repaint_edge_follows_the_engaged_level_not_the_plan() {
        let mut f = RerouteFreeze::new();
        assert!(!f.take_engaged_edge(true), "at rest there is nothing to repaint");

        f.plan_started(); // …under the opaque spinner: a chrome base
        assert!(!f.take_engaged_edge(false), "a plan with no map under it engages nothing");
        assert!(!f.take_engaged_edge(false), "…and keeps engaging nothing");

        // The spinner goes and a map base is back — no plan edge, but *this* is the freeze.
        assert!(f.take_engaged_edge(true), "THE edge: a frozen map the rider is actually looking at");
        assert!(!f.take_engaged_edge(true), "a level, so one repaint — not one per ride-loop pass");

        f.plan_ended();
        assert!(f.take_engaged_edge(true), "and one more to take the banner off");
        assert!(!f.take_engaged_edge(true));
    }

    /// The banner sits in its own band: below the top-centre clock, above the centred rider marker,
    /// and always fully on-panel.
    #[test]
    fn the_banner_band_stays_on_panel_and_clear_of_the_marker() {
        let (y0, rows) = banner_rows(320.0);
        assert_eq!((y0, rows), (96, 36));
        assert!(y0 as i32 + rows as i32 <= 320);
        assert!((y0 + rows) < 160, "clear of the centred user marker");

        let (y0, rows) = banner_rows(20.0); // a frame shorter than the banner (the test harnesses')
        assert_eq!(y0, 0, "clamped to the top rather than drawn off-panel");
        assert_eq!(rows, 20);
    }
}
