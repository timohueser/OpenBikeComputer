//! Who owns the scratch arena, and what each claimant must prove first (issue #1146, P2).
//!
//! The board time-shares **one** RAM block between three arms that are never live together — the
//! per-frame render scratch, the nav block (`NavScratch` + `NavTileCache` + `NavPlanner`), and the
//! USB staging buffer. The union itself is device-only (`obc-fw-nrf54l`'s `arena.rs`, the one place
//! `unsafe` lives); *who may hold it, and when* is plain bookkeeping, and it lives here for the same
//! reason [`crate::link_gate`] does: the board crate has no `test` harness in CI, and "a claim while
//! a search is running must be refused" is exactly the kind of statement that should be asserted
//! rather than reviewed.
//!
//! # The arms are disjoint because the product says so
//!
//! Nothing about the memory makes these three mutually exclusive — three **product rules** do, and
//! each one is a gate on the claim that needs it:
//!
//! | Pair | Rule | Encoded as |
//! |---|---|---|
//! | render ⊥ nav | the map does not redraw while a planner run is live | [`MapQuiesced`] |
//! | render ⊥ usb | a cable transfer shows the transfer screen, not the map | [`TransferReady`] |
//! | nav ⊥ usb | no reroute while docked-transferring | [`TransferReady`] + the search arm on [`TransferGate`](crate::TransferGate) |
//!
//! A gate that is merely *documented* is a gate that gets skipped, so each precondition is a token
//! only its `prove` constructor can mint: [`claim_nav`](ArenaGate::claim_nav) cannot even be
//! *called* without evidence that the map plane is quiesced.
//!
//! # No atomics
//!
//! [`ArenaGate`] takes `&mut self` because the **ride loop is the sole owner-switcher** — the USB
//! plane never claims directly, it requests through the [`TransferGate`](crate::TransferGate) and
//! the loop grants between frames (the #677 async-frame discipline: guards are `!Send` and never
//! held across an `.await` where another claimant could run). If a second switcher ever appears,
//! this becomes an `AtomicU8` compare-exchange like the transfer gate — and the `&mut` is what
//! makes that a *compile* error to skip rather than a race to debug.
//!
//! # The bug class this creates
//!
//! An arm holds **scratch**, never state: nothing written into an arm may be read after its window
//! closes, because the next claimant re-initializes the same bytes in place. A field that must
//! survive its window (the renderer's `suppress_terrain` precedent) belongs in a `Config` beside
//! the arena, not in it. Name it in review; the compiler cannot.

/// Which arm currently owns the arena.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ArenaOwner {
    /// Nobody — the arena's bytes are dead and any claimant may take it.
    #[default]
    None,
    /// The per-frame render scratch, held for the span of one map render.
    Render,
    /// The nav block, held for a whole search (many frames).
    Nav,
    /// The USB staging buffer, held for a whole cable transfer.
    Usb,
}

/// Why a claim (or a release) was refused. The board maps this to a debug `panic!` and a release
/// `Err` — loud, never silent: a wrong-owner access reads another arm's bytes as its own type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArenaError {
    /// Someone else holds the arena. Carries the holder so the caller can log *who* (a render frame
    /// refused by a live search is normal and expected; one refused by a transfer is a UI bug).
    Busy(ArenaOwner),
    /// A release naming an arm that does not hold the arena. Carries the actual owner. Never a
    /// silent no-op — unlike [`TransferGate::release`](crate::TransferGate::release), where two
    /// wires legitimately tear down independently, there is exactly one owner-switcher here, so a
    /// mismatched release means the caller lost track of its own guard.
    NotHeld(ArenaOwner),
}

/// Proof that the **map plane will not draw this pass** — the precondition on
/// [`claim_nav`](ArenaGate::claim_nav), since the nav arm overwrites the render scratch's bytes.
///
/// Two ways to be quiesced, and the second is why this is a token rather than a bool: menu planning
/// happens on a chrome base (`NavPlanning` is opaque, so there is no map underneath to freeze), and
/// a mid-ride detour plan happens over a **map** base — the Recalculating freeze is what makes the
/// second case as safe as the first. See [`App::reroute_freeze_active`](crate::App::reroute_freeze_active).
#[derive(Debug, Clone, Copy)]
pub struct MapQuiesced(());

impl MapQuiesced {
    /// Mint the proof from the two facts the app reports each pass, or `None` when the map plane
    /// would still draw. Compose it straight off the app with
    /// [`App::nav_arena_precondition`](crate::App::nav_arena_precondition).
    pub fn prove(freeze_active: bool, base_draws_map: bool) -> Option<MapQuiesced> {
        (freeze_active || !base_draws_map).then_some(MapQuiesced(()))
    }
}

/// Proof that a **cable transfer may take the arena**: the transfer screen is up (so no map render
/// is coming) *and* no route search is running (whose nav arm the staging buffer would overwrite).
/// The precondition on [`claim_usb`](ArenaGate::claim_usb).
///
/// The search half is the same fact the [`TransferGate`](crate::TransferGate) search arm arbitrates
/// — read it from there ([`search_live`](crate::TransferGate::search_live)) rather than
/// re-deriving it, so the control plane's `busy` answer and this claim can never disagree.
#[derive(Debug, Clone, Copy)]
pub struct TransferReady(());

impl TransferReady {
    /// Mint the proof, or `None` when the UI is not on the transfer screen or a search is live.
    pub fn prove(transfer_screen_up: bool, search_live: bool) -> Option<TransferReady> {
        (transfer_screen_up && !search_live).then_some(TransferReady(()))
    }
}

/// The arena's owner state machine: idle, or held by exactly one arm.
///
/// Pure bookkeeping — it holds no memory and hands out no references. The board pairs it with the
/// union and turns each successful claim into a guard whose `Drop` calls [`release`](Self::release).
#[derive(Debug, Default)]
pub struct ArenaGate {
    owner: ArenaOwner,
}

impl ArenaGate {
    /// An idle gate — the boot state, and the state between every two claims.
    pub const fn new() -> ArenaGate {
        ArenaGate { owner: ArenaOwner::None }
    }

    /// Who holds the arena right now.
    pub fn owner(&self) -> ArenaOwner {
        self.owner
    }

    /// Whether nobody holds it (any claim would pass the ownership half of its gate).
    pub fn is_idle(&self) -> bool {
        self.owner == ArenaOwner::None
    }

    /// Claim the arena for a **map render**, for the span of that render only.
    ///
    /// The only precondition is that the arena is free — which is the whole render ⊥ nav / render ⊥
    /// usb enforcement, and it needs no token because a live search or a live transfer *is* the
    /// holder. Two invariants ride on that: a search claims [`Nav`](ArenaOwner::Nav) for its whole
    /// duration (not per step), and a transfer claims [`Usb`](ArenaOwner::Usb) for its whole
    /// duration — so a frame that would draw a map while either runs is refused here rather than
    /// silently reading a half-written A* table as span records.
    ///
    /// A refusal is not a crash: the host skips the map redraw for that pass (the frozen map stays
    /// on glass) and tries again next frame.
    pub fn claim_render(&mut self) -> Result<(), ArenaError> {
        self.take(ArenaOwner::Render)
    }

    /// Claim the arena for a **route search**, for the whole search.
    ///
    /// Requires [`MapQuiesced`]: the map plane must already have stopped drawing *before* the nav
    /// arm overwrites the render scratch. The freeze is engaged by the app when the plan starts, so
    /// the token is minted from state that is already true — never as a side effect of claiming.
    pub fn claim_nav(&mut self, _map_quiesced: MapQuiesced) -> Result<(), ArenaError> {
        self.take(ArenaOwner::Nav)
    }

    /// Claim the arena for **USB staging**, for the whole transfer.
    ///
    /// Requires [`TransferReady`]: the transfer screen is up and no search is running.
    pub fn claim_usb(&mut self, _transfer_ready: TransferReady) -> Result<(), ArenaError> {
        self.take(ArenaOwner::Usb)
    }

    /// Release the arena — **only** the arm that holds it. Releasing anything else is an
    /// [`ArenaError::NotHeld`], because with one owner-switcher there is no benign reason for it.
    pub fn release(&mut self, owner: ArenaOwner) -> Result<(), ArenaError> {
        if owner == ArenaOwner::None || self.owner != owner {
            return Err(ArenaError::NotHeld(self.owner));
        }
        self.owner = ArenaOwner::None;
        Ok(())
    }

    /// The one transition: idle → `owner`, or `Busy(holder)`. A claimant re-claiming what it
    /// already holds is refused too — the arms re-initialize in place on every claim, so a
    /// double-claim would reset a buffer someone upstack is mid-way through.
    fn take(&mut self, owner: ArenaOwner) -> Result<(), ArenaError> {
        if self.owner != ArenaOwner::None {
            return Err(ArenaError::Busy(self.owner));
        }
        self.owner = owner;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The proof tokens are the gate. Minting them is the *only* way to call the guarded claims, so
    /// pin exactly which facts mint one.
    #[test]
    fn the_nav_precondition_is_a_quiet_map_plane_however_it_got_quiet() {
        assert!(MapQuiesced::prove(false, false).is_some(), "menu planning: chrome base, no map to freeze");
        assert!(MapQuiesced::prove(true, true).is_some(), "mid-ride detour: map base, freeze engaged");
        assert!(MapQuiesced::prove(true, false).is_some(), "belt and braces");
        assert!(
            MapQuiesced::prove(false, true).is_none(),
            "a map base with no freeze is the regression: the search would eat the scratch the next frame renders from"
        );
    }

    /// **The regression** the nav ⊥ usb arm exists for: a reroute started while the cable is
    /// streaming (or a transfer armed mid-search) would hand two owners the same bytes.
    #[test]
    fn the_usb_precondition_needs_the_transfer_screen_and_no_search() {
        assert!(TransferReady::prove(true, false).is_some(), "docked, transfer screen up, nothing planning");
        assert!(TransferReady::prove(false, false).is_none(), "no transfer screen means a map render may still come");
        assert!(TransferReady::prove(true, true).is_none(), "a live search owns the arena — the cable waits");
    }

    #[test]
    fn a_fresh_gate_is_idle_and_a_render_may_take_it() {
        let mut gate = ArenaGate::new();
        assert!(gate.is_idle());
        assert_eq!(gate.owner(), ArenaOwner::None);

        assert_eq!(gate.claim_render(), Ok(()));
        assert_eq!(gate.owner(), ArenaOwner::Render);
        assert!(!gate.is_idle());

        assert_eq!(gate.release(ArenaOwner::Render), Ok(()));
        assert!(gate.is_idle(), "the render span ends and the arena is dead again");
    }

    /// **The regression.** A search holds the nav arm across *many* frames, and the ride loop keeps
    /// producing frames. Without this refusal the next map frame would collect spans into the
    /// planner's live A* table.
    #[test]
    fn a_live_search_refuses_every_render_claim_until_it_finishes() {
        let mut gate = ArenaGate::new();
        let proof = MapQuiesced::prove(true, true).expect("the freeze is engaged");
        assert_eq!(gate.claim_nav(proof), Ok(()));

        for _ in 0..3 {
            assert_eq!(gate.claim_render(), Err(ArenaError::Busy(ArenaOwner::Nav)), "the frame skips its map redraw");
        }
        assert_eq!(gate.owner(), ArenaOwner::Nav, "and the refusal left the search untouched");

        assert_eq!(gate.release(ArenaOwner::Nav), Ok(()));
        assert_eq!(gate.claim_render(), Ok(()), "the frame after the answer renders normally");
    }

    /// The render ⊥ usb half: browsing the map during a cable transfer is refused in the UI (the
    /// transfer screen is up), and refused here too if the UI ever lets one through.
    #[test]
    fn a_live_transfer_refuses_render_and_nav_claims() {
        let mut gate = ArenaGate::new();
        let ready = TransferReady::prove(true, false).expect("docked with the transfer screen up");
        assert_eq!(gate.claim_usb(ready), Ok(()));

        assert_eq!(gate.claim_render(), Err(ArenaError::Busy(ArenaOwner::Usb)));
        let quiesced = MapQuiesced::prove(true, true).expect("even a properly frozen map");
        assert_eq!(gate.claim_nav(quiesced), Err(ArenaError::Busy(ArenaOwner::Usb)), "no reroute while docked");
    }

    /// A render span is short but it is still a span: a search that started inside one (the answer
    /// to a plan the rider queued a frame earlier) must wait for the frame to finish.
    #[test]
    fn a_render_in_progress_refuses_a_search_and_a_transfer() {
        let mut gate = ArenaGate::new();
        assert_eq!(gate.claim_render(), Ok(()));

        let quiesced = MapQuiesced::prove(true, true).unwrap();
        assert_eq!(gate.claim_nav(quiesced), Err(ArenaError::Busy(ArenaOwner::Render)));
        let ready = TransferReady::prove(true, false).unwrap();
        assert_eq!(gate.claim_usb(ready), Err(ArenaError::Busy(ArenaOwner::Render)));
    }

    /// Every arm re-initializes its buffers in place on claim, so a second claim by the *same* arm
    /// would zero a table its own caller is still filling. Refuse it like any other double-claim.
    #[test]
    fn re_claiming_what_you_already_hold_is_refused_not_a_no_op() {
        let mut gate = ArenaGate::new();
        assert_eq!(gate.claim_render(), Ok(()));
        assert_eq!(gate.claim_render(), Err(ArenaError::Busy(ArenaOwner::Render)));
        assert_eq!(gate.owner(), ArenaOwner::Render, "and the first claim still holds");
    }

    /// **The regression** a wrong release would cause: the render frame ends, drops its guard, and
    /// (with a mismatched arm) hands the arena to nobody while the search still reads it. Loud
    /// rather than silent — this is the mirror of `link_gate`'s no-op release, and it differs on
    /// purpose: there, two wires tear down independently; here there is one owner-switcher.
    #[test]
    fn releasing_an_arm_that_does_not_hold_the_arena_is_an_error() {
        let mut gate = ArenaGate::new();
        let proof = MapQuiesced::prove(false, false).unwrap();
        assert_eq!(gate.claim_nav(proof), Ok(()));

        assert_eq!(gate.release(ArenaOwner::Render), Err(ArenaError::NotHeld(ArenaOwner::Nav)));
        assert_eq!(gate.release(ArenaOwner::Usb), Err(ArenaError::NotHeld(ArenaOwner::Nav)));
        assert_eq!(gate.owner(), ArenaOwner::Nav, "the search keeps the arena through both");
        assert_eq!(gate.release(ArenaOwner::Nav), Ok(()));
    }

    #[test]
    fn releasing_an_idle_gate_or_releasing_twice_is_an_error() {
        let mut gate = ArenaGate::new();
        assert_eq!(gate.release(ArenaOwner::Render), Err(ArenaError::NotHeld(ArenaOwner::None)));
        assert_eq!(gate.release(ArenaOwner::None), Err(ArenaError::NotHeld(ArenaOwner::None)), "None is not an arm");

        assert_eq!(gate.claim_render(), Ok(()));
        assert_eq!(gate.release(ArenaOwner::Render), Ok(()));
        assert_eq!(gate.release(ArenaOwner::Render), Err(ArenaError::NotHeld(ArenaOwner::None)));
        assert!(gate.is_idle(), "and the gate is usable afterwards");
        assert_eq!(gate.claim_render(), Ok(()));
    }

    /// The full ride-loop cycle the on-glass soak walks: frames render, a reroute takes over, the
    /// answer lands, frames render again, the cable takes over, and back.
    #[test]
    fn the_ride_loop_hands_the_arena_around_one_arm_at_a_time() {
        let mut gate = ArenaGate::new();
        for _ in 0..2 {
            assert_eq!(gate.claim_render(), Ok(()));
            assert_eq!(gate.release(ArenaOwner::Render), Ok(()));
        }
        assert_eq!(gate.claim_nav(MapQuiesced::prove(true, true).unwrap()), Ok(()));
        assert_eq!(gate.release(ArenaOwner::Nav), Ok(()));
        assert_eq!(gate.claim_render(), Ok(()));
        assert_eq!(gate.release(ArenaOwner::Render), Ok(()));
        assert_eq!(gate.claim_usb(TransferReady::prove(true, false).unwrap()), Ok(()));
        assert_eq!(gate.release(ArenaOwner::Usb), Ok(()));
        assert!(gate.is_idle(), "every arm gave the arena back");
    }
}
