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
//! | render ⊥ usb | a map upload shows the transfer screen, not the map | [`TransferReady`] |
//! | nav ⊥ usb | no route search while the cable owns upload scratch | [`TransferReady`] |
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
    /// The USB write-combining buffer, held for one map upload.
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

/// Proof that a cable upload may take the arena: its transfer screen is up and no route search is
/// live. Both facts come from the ride loop, the arena's sole owner-switcher.
#[derive(Debug, Clone, Copy)]
pub struct TransferReady(());

impl TransferReady {
    pub fn prove(transfer_screen_up: bool, search_live: bool) -> Option<Self> {
        (transfer_screen_up && !search_live).then_some(Self(()))
    }
}

/// Whether a successful claim must **initialize the block in place** before anything reads it.
///
/// The arms are not types of the same shape: each claim writes the block's bytes into its own arm's
/// valid state first (a `RenderScratch`'s all-zero empty vectors, the nav arm's zero fill plus the
/// tile cache's `u32::MAX` tags), because the bytes it inherits are the *previous* arm's — and a
/// `heapless::Vec` whose `len` is a stale A\* node count would read past its own contents.
///
/// The exception is worth a type rather than a comment: when the previous claimant was the **same
/// arm**, the bytes are already that arm's valid state, and re-initializing is a ~117 KB `memset`
/// per map frame for nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArenaInit {
    /// The block is another arm's (or boot garbage, on the first-ever claim): initialize it.
    Required,
    /// The block already holds this arm's initialized state — the last claimant was this same arm
    /// and gave it back intact. Skippable **only** for an arm whose own use is write-before-read
    /// within one span; see [`claim_render`](ArenaGate::claim_render), the one caller that acts on
    /// it.
    Skippable,
}

/// The arena's owner state machine: idle, or held by exactly one arm — plus which arm the block's
/// bytes were last *initialized* as.
///
/// Pure bookkeeping — it holds no memory and hands out no references. The board pairs it with the
/// union and turns each successful claim into a guard whose `Drop` calls [`release`](Self::release).
#[derive(Debug, Default)]
pub struct ArenaGate {
    owner: ArenaOwner,
    /// Which arm the block's bytes are currently set up as — **not** who holds it (`owner` is
    /// `None` between every two claims). This is what makes [`ArenaInit::Skippable`] decidable, and
    /// it lives here rather than beside the union so the rule is host-tested with the rest of them.
    initialized_as: ArenaOwner,
}

impl ArenaGate {
    /// An idle gate — the boot state, and the state between every two claims.
    pub const fn new() -> ArenaGate {
        ArenaGate { owner: ArenaOwner::None, initialized_as: ArenaOwner::None }
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
    /// duration (not per step), and a render claims for its
    /// duration — so a frame that would draw a map while either runs is refused here rather than
    /// silently reading a half-written A* table as span records.
    ///
    /// A refusal is not a crash: the host skips the map redraw for that pass (the frozen map stays
    /// on glass) and tries again next frame.
    ///
    /// Returns whether the scratch must be re-initialized ([`ArenaInit`]). This is the one arm that
    /// can skip it: a render span leaves behind a fully valid, empty-on-next-use `RenderScratch`
    /// (every buffer is written before it is read within a frame), which is exactly why the
    /// pre-arena resident static could be zeroed once at boot and reused for every frame of the
    /// device's life. Frames follow frames, so on the dominant path this is `Skippable` and the map
    /// pays no `memset` at all.
    pub fn claim_render(&mut self) -> Result<ArenaInit, ArenaError> {
        self.take(ArenaOwner::Render)
    }

    /// Claim the arena for a **route search**, for the whole search.
    ///
    /// Requires [`MapQuiesced`]: the map plane must already have stopped drawing *before* the nav
    /// arm overwrites the render scratch. The freeze is engaged by the app when the plan starts, so
    /// the token is minted from state that is already true — never as a side effect of claiming.
    ///
    /// No [`ArenaInit`]: the nav arm re-initializes unconditionally. Two searches back to back would
    /// report `Skippable`, and it would be wrong — the block would hold the *previous* search's A\*
    /// table and its finished planner, which is state, not an empty arm.
    pub fn claim_nav(&mut self, _map_quiesced: MapQuiesced) -> Result<(), ArenaError> {
        self.take(ArenaOwner::Nav).map(|_| ())
    }

    /// Claim the write-combining arm for a whole USB map upload.
    pub fn claim_usb(&mut self, _ready: TransferReady) -> Result<(), ArenaError> {
        self.take(ArenaOwner::Usb).map(|_| ())
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
    /// already holds is refused too — an arm may re-initialize in place on claim, so a double-claim
    /// would reset a buffer someone upstack is mid-way through.
    fn take(&mut self, owner: ArenaOwner) -> Result<ArenaInit, ArenaError> {
        if self.owner != ArenaOwner::None {
            return Err(ArenaError::Busy(self.owner));
        }
        self.owner = owner;
        let init = if core::mem::replace(&mut self.initialized_as, owner) == owner {
            ArenaInit::Skippable
        } else {
            ArenaInit::Required
        };
        Ok(init)
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

    #[test]
    fn a_fresh_gate_is_idle_and_a_render_may_take_it() {
        let mut gate = ArenaGate::new();
        assert!(gate.is_idle());
        assert_eq!(gate.owner(), ArenaOwner::None);

        assert_eq!(gate.claim_render(), Ok(ArenaInit::Required));
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
        assert_eq!(gate.claim_render(), Ok(ArenaInit::Required), "the frame after the answer renders normally");
    }

    #[test]
    fn usb_stage_requires_a_visible_transfer_and_excludes_render_and_nav() {
        assert!(TransferReady::prove(false, false).is_none());
        assert!(TransferReady::prove(true, true).is_none());
        let ready = TransferReady::prove(true, false).expect("visible transfer, no search");
        let mut gate = ArenaGate::new();
        assert_eq!(gate.claim_usb(ready), Ok(()));
        assert_eq!(gate.claim_render(), Err(ArenaError::Busy(ArenaOwner::Usb)));
        let quiet = MapQuiesced::prove(true, true).unwrap();
        assert_eq!(gate.claim_nav(quiet), Err(ArenaError::Busy(ArenaOwner::Usb)));
        assert_eq!(gate.release(ArenaOwner::Usb), Ok(()));
    }

    /// A render span is short but it is still a span: a search that started inside one (the answer
    /// to a plan the rider queued a frame earlier) must wait for the frame to finish.
    #[test]
    fn a_render_in_progress_refuses_a_search_and_a_transfer() {
        let mut gate = ArenaGate::new();
        assert_eq!(gate.claim_render(), Ok(ArenaInit::Required));

        let quiesced = MapQuiesced::prove(true, true).unwrap();
        assert_eq!(gate.claim_nav(quiesced), Err(ArenaError::Busy(ArenaOwner::Render)));
    }

    /// Every arm re-initializes its buffers in place on claim, so a second claim by the *same* arm
    /// would zero a table its own caller is still filling. Refuse it like any other double-claim.
    #[test]
    fn re_claiming_what_you_already_hold_is_refused_not_a_no_op() {
        let mut gate = ArenaGate::new();
        assert_eq!(gate.claim_render(), Ok(ArenaInit::Required));
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
        assert_eq!(gate.owner(), ArenaOwner::Nav, "the search keeps the arena through both");
        assert_eq!(gate.release(ArenaOwner::Nav), Ok(()));
    }

    #[test]
    fn releasing_an_idle_gate_or_releasing_twice_is_an_error() {
        let mut gate = ArenaGate::new();
        assert_eq!(gate.release(ArenaOwner::Render), Err(ArenaError::NotHeld(ArenaOwner::None)));
        assert_eq!(gate.release(ArenaOwner::None), Err(ArenaError::NotHeld(ArenaOwner::None)), "None is not an arm");

        assert_eq!(gate.claim_render(), Ok(ArenaInit::Required));
        assert_eq!(gate.release(ArenaOwner::Render), Ok(()));
        assert_eq!(gate.release(ArenaOwner::Render), Err(ArenaError::NotHeld(ArenaOwner::None)));
        assert!(gate.is_idle(), "and the gate is usable afterwards");
        assert_eq!(gate.claim_render(), Ok(ArenaInit::Skippable), "…still holding the last render's scratch");
    }

    /// The **~117 KB `memset` per map frame** this exists to avoid, and the two halves of when it is
    /// safe. A render span hands the block back as a valid `RenderScratch`, so frame after frame
    /// skips the re-init; anything that hands it to another arm — a search, a cable transfer — makes
    /// the next render pay for it again. The first-ever claim pays too, which is what keeps
    /// `.uninit`'s boot garbage from ever being read as a scratch.
    #[test]
    fn only_a_render_following_a_render_may_skip_the_re_init() {
        let mut gate = ArenaGate::new();
        assert_eq!(gate.claim_render(), Ok(ArenaInit::Required), "the first claim meets .uninit's boot garbage");
        assert_eq!(gate.release(ArenaOwner::Render), Ok(()));
        for _ in 0..5 {
            assert_eq!(gate.claim_render(), Ok(ArenaInit::Skippable), "frame after frame: no memset");
            assert_eq!(gate.release(ArenaOwner::Render), Ok(()));
        }

        // **The nav arm, and there is only one other arm to check.** This was a loop over every
        // foreign owner while the USB staging arm existed; with two arms the loop is the nav case
        // written awkwardly, so it is written plainly. If a third arm is ever added, this becomes a
        // loop again — and the const assert in `arena.rs` is what will say so first.
        assert_eq!(gate.claim_nav(MapQuiesced::prove(false, false).unwrap()), Ok(()));
        assert_eq!(gate.release(ArenaOwner::Nav), Ok(()));
        assert_eq!(gate.claim_render(), Ok(ArenaInit::Required), "the nav arm left its own bytes behind");
        assert_eq!(gate.release(ArenaOwner::Render), Ok(()));
    }

    /// The full ride-loop cycle the on-glass soak walks: frames render, a reroute takes over, the
    /// answer lands, frames render again, the cable takes over, and back.
    #[test]
    fn the_ride_loop_hands_the_arena_around_one_arm_at_a_time() {
        let mut gate = ArenaGate::new();
        for i in 0..2 {
            let init = if i == 0 { ArenaInit::Required } else { ArenaInit::Skippable };
            assert_eq!(gate.claim_render(), Ok(init));
            assert_eq!(gate.release(ArenaOwner::Render), Ok(()));
        }
        assert_eq!(gate.claim_nav(MapQuiesced::prove(true, true).unwrap()), Ok(()));
        assert_eq!(gate.release(ArenaOwner::Nav), Ok(()));
        assert_eq!(gate.claim_render(), Ok(ArenaInit::Required), "the search left its A* table in the block");
        assert_eq!(gate.release(ArenaOwner::Render), Ok(()));
        assert!(gate.is_idle(), "every arm gave the arena back");
    }
}
