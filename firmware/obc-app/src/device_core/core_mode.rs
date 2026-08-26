//! **`CoreMode`** — the one owner of *what heavy work may run now, and what the rider is looking
//! at* (#1397 S5).
//!
//! Four levels and nothing else. Two of them say a planner run holds the scratch arena's nav arm
//! (one per [`PlanFamily`]), one says a bulk transfer holds the store, and the fourth is the
//! level→edge converter the Recalculating banner's single repaint comes from.
//!
//! Everything that used to answer "is a search live?" reads these levels now: the freeze, the two
//! arena proof tokens, and the pass's admission stage. Before S5 that fact existed four times over
//! — a freeze module, a link gate, the arena's owner and the board's own planner handle — and the
//! four could disagree.
//!
//! # The Recalculating freeze (#1146, P2)
//!
//! A route search and a map render want the same RAM (the arena's `render ⊥ nav` rule), and the
//! product rule that makes them disjoint is the one every commercial bike computer already ships:
//! while it recalculates, the map stops. So a live planner run engages a freeze in which
//!
//! - the host skips map redraws ([`App::reroute_freeze_active`](crate::App::reroute_freeze_active)),
//!   leaving the last frame on glass — a reflective panel keeps showing it for free;
//! - [`App::tick`](crate::App::tick) stops advancing route-match progress, so the guidance the
//!   frozen frame shows cannot drift away from it (fixes still record — breadcrumb, ride totals,
//!   altimeter, sensors — a freeze pauses the *map*, never the ride);
//! - a banner says so. A screen that stops responding without saying why reads as a crash, and the
//!   freeze lasts as long as the search does.
//!
//! ## Why the base screen matters
//!
//! The freeze is engaged only when the base screen would actually draw a map. Planning from the
//! menus already renders no map — `NavPlanning` is an opaque chrome screen, so it *is* the base
//! while it is up — and freezing there would put a banner over a spinner that is already saying
//! the same thing in its own words ("Finding a route..." for a route plan, "Planning detour..." for
//! a detour).
//!
//! The window that needs this is the **detour** path (#882), where the planning screen is *pushed
//! over a map base*: Back pops it while the planner is still running, and the next frame would
//! render the map straight into the arena the search still owns. One predicate covers both: a live
//! search plus [`base_draws_map`](crate::App::base_draws_map).
//!
//! # Two search levels, never a family tag
//!
//! The nav arm is **one block**, so it stays out until every family that took it is done. A single
//! tag would have to pick a winner, and every terminal edge — a drained cancel, an answer, a
//! failure tier — fires unconditionally on whatever is live: a detour's edge would release a
//! freeze a *route* search is still holding the arm behind, the map plane would resume, and the
//! next frame's render claim would be refused for the rest of the ride (#1146).
//!
//! # No atomics, no task, no latch
//!
//! `CoreMode` is plain data inside [`App`](crate::App), taken by `&mut` for the same reason
//! [`ArenaGate`](crate::ArenaGate) is: the ride loop is the sole switcher. Every level is exactly
//! that — a level, recomputed from what is true now and never latched.

use crate::arena_gate::{MapQuiesced, TransferReady};
use crate::navigator::PlanFamily;

/// What the device is busy with, as the rider would name it — the ranked, payload-free answer.
///
/// The ranking decides only what the rider is **told**. It never decides admission: that reads the
/// levels, because a search and a transfer exclude different things.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModeState {
    /// Nothing heavy is holding the device.
    Free,
    /// A planner run holds the nav arm.
    Searching,
    /// A bulk transfer holds the store.
    Transferring,
}

/// The three levels: two searches and one transfer.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct CoreMode {
    /// A live [`Route`](PlanFamily::Route) planner run.
    route_search: bool,
    /// A live [`Detour`](PlanFamily::Detour) planner run.
    detour_search: bool,
    /// A bulk transfer is streaming into the store — the latest level reported, never a count.
    transferring: bool,
}

impl CoreMode {
    /// The boot state: nothing searching, nothing streaming.
    pub(crate) const fn new() -> CoreMode {
        CoreMode { route_search: false, detour_search: false, transferring: false }
    }

    /// The level `family` owns.
    fn slot(&mut self, family: PlanFamily) -> &mut bool {
        match family {
            PlanFamily::Route => &mut self.route_search,
            PlanFamily::Detour => &mut self.detour_search,
        }
    }

    /// An executor took `family`'s planner operation: the search holds the nav arm from now on.
    /// Returns whether this *changed* whether any search is live at all.
    ///
    /// Written only from [`NavigatorMachine::next_plan_effect`](crate::navigator::NavigatorMachine).
    pub(crate) fn search_started(&mut self, family: PlanFamily) -> bool {
        let was = self.searching();
        *self.slot(family) = true;
        !was
    }

    /// A `family` planner run is over — answered, failed, or its cancellation reached the executor.
    /// Returns whether that ended the *last* live search. Idempotent (several of those edges
    /// legitimately land for one run: a cancel is delivered and the late answer arrives behind it),
    /// and it never touches the other family — see the module docs for the regression that is.
    ///
    /// Written only from `NavigatorMachine`'s `note_answer` / `note_cancel_delivered`.
    pub(crate) fn search_ended(&mut self, family: PlanFamily) -> bool {
        let was = self.searching();
        *self.slot(family) = false;
        was && !self.searching()
    }

    /// Whether a planner run holds the nav arm at all — true through a menu plan too, where no
    /// freeze is engaged. Deliberately the **union** of the two families.
    pub(crate) fn searching(&self) -> bool {
        self.route_search || self.detour_search
    }

    /// A bulk transfer started or ended. Written only from
    /// [`App::set_map_transfer`](crate::App::set_map_transfer) and from
    /// [`ExternalFacts::transfer`](crate::device_core::ExternalFacts) at the pass's fact stage —
    /// two reports of the same fact, and the newest one is the truth.
    pub(crate) fn note_transfer(&mut self, streaming: bool) {
        self.transferring = streaming;
    }

    /// What the rider is looking at. A search outranks a transfer: it is the shorter, more
    /// urgent-looking wait, and it is the one with a banner.
    pub(crate) fn state(&self) -> ModeState {
        if self.searching() {
            ModeState::Searching
        } else if self.transferring {
            ModeState::Transferring
        } else {
            ModeState::Free
        }
    }

    /// Whether a **new** heavy operation may start — the verdict
    /// [`Capabilities::calculate`](crate::device_core::Capabilities::calculate) withdraws
    /// `plan_route`, `plan_detour` and `dfu.install` on.
    ///
    /// Reads the levels, not [`state`](CoreMode::state): a search and a transfer each exclude
    /// heavy work on their own, so the ranking must not be able to hide one behind the other.
    pub(crate) fn admits_heavy(&self) -> bool {
        !self.searching() && !self.transferring
    }

    /// Whether the freeze is **engaged**: a live search *and* a base screen that would draw a map.
    pub(crate) fn frozen(&self, base_draws_map: bool) -> bool {
        self.searching() && base_draws_map
    }

    /// Mint the proof that the **map plane will not draw this pass**, or `None` when it still
    /// would — the precondition on [`ArenaGate::claim_nav`](crate::ArenaGate::claim_nav).
    ///
    /// Two ways to be quiesced: menu planning happens on a chrome base (there is no map underneath
    /// to freeze), and a mid-ride detour plan happens over a **map** base, where the freeze is what
    /// makes the second case as safe as the first.
    pub(crate) fn nav_precondition(&self, base_draws_map: bool) -> Option<MapQuiesced> {
        (self.frozen(base_draws_map) || !base_draws_map).then(MapQuiesced::mint)
    }

    /// Mint the proof that a cable upload may take the arena — its transfer screen is up (`render ⊥
    /// usb`) and no search holds the nav arm (`nav ⊥ usb`), or `None`.
    pub(crate) fn usb_precondition(&self, transfer_card_up: bool) -> Option<TransferReady> {
        (transfer_card_up && !self.searching()).then(TransferReady::mint)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The lifecycle in one test: nothing frozen at rest, engaged only where a map would be drawn,
    /// and released by whichever edge lands first.
    #[test]
    fn the_freeze_follows_the_search_and_the_base_screen() {
        let mut m = CoreMode::new();
        assert!(!m.searching());
        assert!(!m.frozen(true), "no search, no freeze — the map renders normally");

        assert!(m.search_started(PlanFamily::Route), "taking the operation is the engaging edge");
        assert!(!m.search_started(PlanFamily::Route), "…and re-engaging is not an edge");
        assert!(m.searching());
        assert!(!m.frozen(false), "menu planning draws no map: nothing to freeze, no banner");
        assert!(m.frozen(true), "a search over a map base is the freeze");

        assert!(m.search_ended(PlanFamily::Route), "the answer releases it");
        assert!(!m.frozen(true));
    }

    /// **The regression** a stuck freeze would be: the map never redraws again for the rest of the
    /// ride. Every release edge is idempotent, so the delivered cancel and the late answer behind
    /// it can both fire, in either order, without leaving the level inconsistent.
    #[test]
    fn releasing_twice_is_harmless_and_a_new_search_re_engages() {
        let mut m = CoreMode::new();
        assert!(!m.search_ended(PlanFamily::Detour), "releasing what was never engaged is not an edge");
        assert!(m.search_started(PlanFamily::Detour));
        assert!(m.search_ended(PlanFamily::Detour));
        assert!(!m.search_ended(PlanFamily::Detour), "the late answer behind the cancel is a no-op");
        assert!(!m.frozen(true));
        assert!(m.search_started(PlanFamily::Detour), "and the next reroute freezes again");
        assert!(m.frozen(true));
    }

    /// **The regression** the families exist for, in both directions: every terminal edge fires
    /// unconditionally on whatever is live, so one shared level would let a detour's cancel (or the
    /// board's immediate `NoPath` answer for the detour half it has not built) release a freeze a
    /// *route* search is still holding the nav arm behind — and the very next frame would claim the
    /// render arm the search is mid-way through.
    #[test]
    fn a_search_is_released_only_by_its_own_familys_terminal_edge() {
        let mut m = CoreMode::new();
        m.search_started(PlanFamily::Route);
        assert!(!m.search_ended(PlanFamily::Detour), "a detour terminal edge is not this run's");
        assert!(m.searching(), "the route search still holds the arm");
        assert!(m.frozen(true), "…so the map stays frozen");
        assert!(m.search_ended(PlanFamily::Route), "only its own answer releases it");
        assert!(!m.searching());

        // And the mirror image.
        m.search_started(PlanFamily::Detour);
        assert!(!m.search_ended(PlanFamily::Route));
        assert!(m.frozen(true));
        assert!(m.search_ended(PlanFamily::Detour));
        assert!(!m.frozen(true));
    }

    /// Two runs live at once is reachable through the legacy drain's cancel window, and the arm is
    /// **one block** — so the freeze must hold until the last of them is done, not the first. Two
    /// levels give that for free; a single family *tag* would not.
    #[test]
    fn the_freeze_outlives_the_first_of_two_live_runs() {
        let mut m = CoreMode::new();
        assert!(m.search_started(PlanFamily::Route));
        assert!(!m.search_started(PlanFamily::Detour), "already live: not a fresh engaging edge");
        assert!(!m.search_ended(PlanFamily::Route), "one down, one to go — not the releasing edge");
        assert!(m.searching());
        assert!(m.search_ended(PlanFamily::Detour), "the last one out releases the freeze");
        assert!(!m.searching());
    }

    /// The axis stage 12 did not have before S5: a live search withdraws heavy work exactly as a
    /// streaming transfer does, so a second plan is never *started* and then failed.
    #[test]
    fn a_live_search_withdraws_heavy_work() {
        let mut m = CoreMode::new();
        assert!(m.admits_heavy(), "at rest the device may start anything");
        m.search_started(PlanFamily::Detour);
        assert!(!m.admits_heavy(), "no second plan, and no install, while the planner has the arm");
        m.search_ended(PlanFamily::Detour);
        assert!(m.admits_heavy(), "and it comes straight back when the answer lands");
    }

    /// The other half of the same verdict, and the two levels are independent: a transfer teardown
    /// never releases a search, and an answer never ends a transfer. (`link_gate`'s
    /// `tearing_down_one_side_never_releases_the_other`, on the levels that replaced it.)
    #[test]
    fn a_streaming_transfer_withdraws_heavy_work_and_the_two_levels_are_independent() {
        let mut m = CoreMode::new();
        m.note_transfer(true);
        assert!(!m.admits_heavy(), "no reroute while docked-transferring");
        assert!(!m.frozen(true), "…but a transfer is not a freeze: the map plane is the card's");

        m.search_started(PlanFamily::Route);
        m.note_transfer(false); // the transfer concludes mid-search
        assert!(m.searching(), "the search is untouched by a transfer ending");
        assert!(!m.admits_heavy());

        m.note_transfer(true);
        assert!(m.search_ended(PlanFamily::Route), "and the answer releases only the search");
        assert!(!m.admits_heavy(), "the transfer still holds the store");
        m.note_transfer(false);
        assert!(m.admits_heavy());
    }

    /// The ranking says what the rider is told and nothing else — with both levels set, admission
    /// answers the same as it would for either one alone.
    #[test]
    fn searching_outranks_transferring_and_the_ranking_never_decides_admission() {
        let mut m = CoreMode::new();
        assert_eq!(m.state(), ModeState::Free);
        m.note_transfer(true);
        assert_eq!(m.state(), ModeState::Transferring);
        m.search_started(PlanFamily::Route);
        assert_eq!(m.state(), ModeState::Searching, "the search is what the rider is waiting on");
        assert!(!m.admits_heavy(), "and both levels still refuse heavy work");

        m.search_ended(PlanFamily::Route);
        assert_eq!(m.state(), ModeState::Transferring, "the transfer underneath is still there");
        assert!(!m.admits_heavy(), "so the ranking hid nothing");
    }

    /// The proof tokens are the gate, and `CoreMode` is now their only mint. Pin exactly which
    /// levels mint one. (`arena_gate`'s two precondition tests, re-pointed at the new mint.)
    #[test]
    fn the_arena_proofs_can_only_be_minted_from_the_levels() {
        let mut m = CoreMode::new();
        assert!(m.nav_precondition(false).is_some(), "menu planning: chrome base, no map to freeze");
        assert!(
            m.nav_precondition(true).is_none(),
            "a map base with no search is the regression: the search would eat the scratch the next frame renders from"
        );
        assert!(m.usb_precondition(true).is_some(), "a visible transfer, nothing searching");
        assert!(m.usb_precondition(false).is_none(), "no transfer screen up: the map plane still owns the glass");

        m.search_started(PlanFamily::Detour);
        assert!(m.nav_precondition(true).is_some(), "mid-ride detour: map base, freeze engaged");
        assert!(m.usb_precondition(true).is_none(), "and the cable waits for the search — `nav ⊥ usb`");
    }
}
