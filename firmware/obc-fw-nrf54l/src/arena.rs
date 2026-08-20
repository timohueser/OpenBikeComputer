//! The **scratch arena** (issue #1146, P2) — one block of RAM, three arms, one owner at a time.
//!
//! Three of the board's largest resident blocks are never live at the same moment, and each used to
//! own its bytes permanently: the per-frame render scratch (`obc_render::RenderScratch`) and the nav
//! block ([`NavArm`] — `NavScratch` + `NavTileCache` + the resumable `NavPlanner`, ~80.6 KB). This
//! module time-shares them through a `union`, so the board pays **max(arms)** instead of their sum.
//!
//! The third is the protocol-v4 USB write-combining stage. Sixteen 4 KiB records share a 64 KiB arm
//! so the flat store issues one 128-block card command instead of sixteen short program cycles.
//! It remains below the 128 KiB render arm, so restoring the throughput arm costs no resident RAM.
//!
//! **This is the only place in the feature that names the block's bytes, and the only place that
//! reads or writes them through a raw pointer.** Everything outside reaches an arm through a guard:
//! a `Deref` for the render and nav arms and — for the nav arm's
//! `MaybeUninit` planner slot, the one field a guard cannot hand out unconditionally — the checked
//! [`NavGuard::plan_parts`] / [`NavGuard::planner_ref`] accessors, which answer `None` until
//! [`NavGuard::begin_plan`] has written it. That last one is why the claim is about *this* module
//! rather than about the `unsafe` keyword: a `MaybeUninit` field reachable from outside would let
//! any caller `assume_init` a slot nothing had written, with only loop-local bookkeeping standing
//! between it and a garbage planner.
//!
//! # What makes the arms disjoint
//!
//! Nothing about the memory — three *product rules* do, and each is a gate on the claim that needs
//! it. The rules, the tokens that prove them, and the regressions they exist for are all in
//! [`obc_app::arena_gate`], where they are host-tested; the board does not re-derive them. The gate
//! object here is that same [`ArenaGate`], paired with the union:
//!
//! | Pair | Rule | Enforced by |
//! |---|---|---|
//! | render ⊥ nav | the map does not redraw while a planner run is live | the Recalculating freeze → [`MapQuiesced`] |
//! | render ⊥ USB | the transfer card displaces map rendering | [`obc_app::TransferReady`] → [`UsbGuard`] |
//! | nav ⊥ USB | an upload grant is refused while route search owns the arena | [`ArenaGate`] |
//!
//! # One thread-mode owner-switcher
//!
//! [`ArenaGate`] takes `&mut self`, so this module keeps it in a `static mut` reachable only from
//! here and every claim/release runs in thread mode. During a normal ride the ride loop is the
//! owner-switcher (the #677 async-frame discipline: guards are `!Send` and are never held across an
//! `.await` where another claimant could run).
//!
//! # No two references at once
//!
//! Every ordinary reference into the arena — each guard's `Deref` — is derived
//! **freshly from the one raw pointer** ([`arena_ptr`]) and dies before the next one is made.
//! The USB exception is narrower: FLPR retains one bank's numeric address during deferred DMA, so
//! [`with_usb_stage_bank`] forms `&mut` for only the other, disjoint bank and never for the whole
//! arm. Nothing here stores a Rust reference. A frame that draws no map takes no reference at all: the app's render
//! entry point wants an `Option<&mut RenderScratch>` and chrome passes `None` (#1146 P2).
//!
//! # The bug class this creates: sticky state
//!
//! An arm holds **scratch**, never state. Nothing written into an arm may be read after its window
//! closes, because the next claimant re-initializes those bytes in place. The precedent is the
//! renderer's `suppress_terrain` (#1096): a single `bool` that had to survive between frames, and
//! that P1 moved out into a per-call `RenderConfig` precisely so the render arm could join this
//! union. A field added to an arm that must outlive its window is silently corrupted the first time
//! another arm runs — the compiler cannot see it, so it is a review rule: **a value that must
//! survive belongs in a `Config` beside the arena, not in it.**
//!
//! # The growth asymmetry, and the ≥10 KB bar
//!
//! The budget is `max(arms)`, so growth is **not** linear:
//!
//! - An arm **below** the maximum grows at **zero** resident cost until it reaches the maximum arm.
//!   The nav arm is that case today, with ~36 KB of headroom under the render one.
//! - Growing either **maximum** arm — today render and USB are tied at the ceiling — costs the full
//!   delta, 1:1. The report and the assertion below keep that accounting literal.
//!
//! Both halves are traps in opposite directions: nobody should "optimize" a growth that is free,
//! and nobody should wave through one that is not. The compile-time assert in `main.rs` says the
//! same thing at the place the number is enforced.
//!
//! And a bar for any future arm: **≥10 KB, or it does not belong here.** Every arm adds an
//! exclusion rule that has to hold forever, and an arena that keeps acquiring arms is accumulating
//! invariants faster than it is saving RAM. Map/route caches are permanently disqualified for a
//! different reason — the render reads *through* them *into* the render scratch, so they are
//! concurrent by construction.

use core::marker::PhantomData;
use core::mem::{ManuallyDrop, MaybeUninit};
use core::ops::{Deref, DerefMut};
use core::ptr::addr_of_mut;

use obc_app::{ArenaError, ArenaGate, ArenaInit, ArenaOwner, MapQuiesced};

/// One planner step's staged OBCR output. The largest terminal step is the full 256-entry chunk
/// index (11,264 B), one final 1,530-byte chunk body, and the 128-byte header backfill.
pub(crate) const NAV_OUTPUT_STAGE_BYTES: usize = 16 * 1_024;

/// The **nav arm**: everything one route search needs, as one struct so the union has one member
/// to name (epic #116 R4 + #499).
///
/// Deliberately **without** the map's terrain. `TERRAIN` is sampled at *fix* cadence during a ride
/// (`App::sample_terrain`, EL8) — that is, while the map plane is rendering and no search is
/// running — so it is genuinely always-live state and stays a resident static of its own. It is the
/// one nav-adjacent block that would have been wrong to fold in here, and the budget names it
/// separately for that reason.
///
/// Defined in **every** profile, `has_nav` or not, so [`NAV_ARM_BYTES`] can be the type's own size
/// everywhere: the report's arm figures are sizes of types, not sums of parts, and a hand-summed
/// fallback silently loses the struct's tail padding (finding #1150-9 — it read 59,868 against the
/// pinned 80,584).
///
/// **`has_nav` is currently always on** — `build.rs` emits it unconditionally (the router rides
/// every LM20 build), so the `not(has_nav)` arms in this module are dormant, not dead-by-mistake.
/// They are kept because the cfg is the one seam that would carve the router back out, and a
/// `#[cfg]` site is cheaper to keep honest than to re-derive.
#[cfg_attr(not(has_nav), allow(dead_code))]
pub(crate) struct NavArm {
    /// The fixed A* table (~39.9 KB). Reset by the planner's first step of every request.
    pub(crate) scratch: obc_route::NavScratch,
    /// The route-private graph/index cache (~24.9 KB). Warmth across searches was never correctness
    /// — losing it costs cold 512 B source reads on the next search, which its counters measure.
    pub(crate) tiles: obc_reader::NavTileCache,
    /// The resumable planner's slot (~15.8 KB, it owns the OBCR emitter across steps). Keeps its
    /// per-request `ptr::write` discipline (#499): the drain writes a fresh planner here and the
    /// step path reads it only while the loop's `NavRun` bookkeeping says a plan is active.
    ///
    /// **Module-private on purpose.** It is the one field of any arm that a claim does not leave
    /// initialized, so the type system cannot let a caller reach it: everything outside goes through
    /// [`NavGuard::begin_plan`] / [`NavGuard::plan_parts`] / [`NavGuard::planner_ref`], which carry
    /// the "was it written?" fact with the guard instead of leaving it in the ride loop's `NavRun`
    /// bookkeeping.
    planner: MaybeUninit<obc_route::NavPlanner>,
    /// Append bytes plus the final header backfill, flushed through the flat-store writer after
    /// each bounded synchronous planner step. This fits in the nav arm's existing headroom under
    /// the larger render arm, so it adds no resident RAM.
    pub(crate) output: [u8; NAV_OUTPUT_STAGE_BYTES],
}

/// The arena itself: one block, three overlapping views.
///
/// `repr(C, align(8))` rather than a plain union so the block's address is stable and 8-aligned for
/// every arm regardless of which member the compiler would otherwise have led with — the nav arm's
/// `u64`-containing entries and the render scratch's `(i32, i32)` vectors both want it, and a
/// misaligned in-place `ptr::write` of the planner would fault on this strict-align target.
#[repr(C, align(8))]
union ScratchArena {
    /// The per-frame render scratch.
    render: ManuallyDrop<obc_render::RenderScratch>,
    #[cfg(has_nav)]
    nav: ManuallyDrop<NavArm>,
    /// USB upload bytes; written before read during one transfer grant.
    usb: ManuallyDrop<[u8; crate::usb::STAGE_LEN]>,
}

/// The arena's resident size — `max(arms)`, the single term the board's RAM budget carries in place
/// of the three blocks it replaced.
pub(crate) const ARENA_BYTES: usize = core::mem::size_of::<ScratchArena>();
/// The render arm's size, for the resource report.
pub(crate) const RENDER_ARM_BYTES: usize = core::mem::size_of::<obc_render::RenderScratch>();
/// The nav arm's size, for the resource report. The *type's* size in every profile, like the
/// terrain entries, even where `has_nav` would keep it out of the image — see [`NavArm`].
pub(crate) const NAV_ARM_BYTES: usize = core::mem::size_of::<NavArm>();
/// The cable upload arm's size, reported beside the arena maximum.
pub(crate) const USB_ARM_BYTES: usize = crate::usb::STAGE_LEN;
// **`max(arms)`, stated rather than assumed.** Render and USB are tied at the ceiling, so every
// growth note in this module and at `main.rs`'s budget assert prices growth in either 1:1 and nav
// growth at nothing until it passes them. Fail the build rather than let those notes go stale.
const _: () = assert!(
    NAV_ARM_BYTES <= ARENA_BYTES && USB_ARM_BYTES <= ARENA_BYTES && RENDER_ARM_BYTES == ARENA_BYTES,
    "the render arm is no longer the arena ceiling — re-read the growth-asymmetry notes in arena.rs \
     and at main.rs's budget assert before re-pinning anything"
);

/// The arena's storage, in the **`.uninit`** section (cortex-m-rt's `link.x`: `NOLOAD`, placed
/// after `.bss`, never touched by the reset handler's zeroing loop).
///
/// `.bss` would be wrong twice over. It would cost ~117 KB of `memset` on **every** boot for bytes
/// that are meaningless until an arm claims them and initializes itself in place — and it would say
/// something false: `.bss` means "zero at boot", and the one thing that is never true of this block
/// is that its contents mean anything before a claim.
///
/// The mechanism is the one `defmt_rtt::BUFFER` already uses — it is the section's only other tenant
/// and it is exactly where the resource guard's 1,024 B `uninit_max` came from. So the guard's two
/// RAM figures both move here and both need re-pinning: `.bss + .data` drops by the three donor
/// statics, `.uninit` rises by the arena. The *sum* is what fell (~76 KB), and the residual main
/// stack — `_stack_start − __euninit`, so it is charged for `.uninit` exactly as it was for `.bss` —
/// rises by the same amount.
///
/// `static mut` + [`addr_of_mut`] rather than a `SyncUnsafeCell` wrapper: it is the idiom the rest
/// of this crate's resident statics already use (`FB`, `APP`, the nav statics this replaces), and
/// no reference to it is ever formed outside [`arena_ptr`].
#[link_section = ".uninit.OBC_SCRATCH_ARENA"]
static mut ARENA: MaybeUninit<ScratchArena> = MaybeUninit::uninit();

/// Who owns the arena right now — the host-tested state machine from `obc-app`, not a second copy
/// of its rules. Tiny (`one u8`), so it stays in `.bss`.
static mut GATE: ArenaGate = ArenaGate::new();

/// The one raw pointer every reference into the block is derived from — see the module doc's
/// "no two references at once".
#[inline(always)]
fn arena_ptr() -> *mut ScratchArena {
    // Taking the address of a `static mut` forms no reference, so this needs no `unsafe`; every
    // caller below is where the reference — and the obligation — actually begins.
    addr_of_mut!(ARENA) as *mut ScratchArena
}

/// The gate, as the `&mut` its API takes.
///
/// # Safety
/// The caller must not hold another `&mut` to the gate. Every caller is a claim/release in this
/// module, each of which takes and drops it within one synchronous statement, on the one thread-mode
/// ride loop — there is no second owner-switcher (see the module doc).
#[inline(always)]
unsafe fn gate() -> &'static mut ArenaGate {
    &mut *addr_of_mut!(GATE)
}

/// Who holds the arena — for the ride loop's diagnostics and for the "should this frame even try?"
/// question.
pub(crate) fn owner() -> ArenaOwner {
    // SAFETY: see `gate`; the borrow ends with this expression.
    unsafe { gate().owner() }
}

/// A refused claim or release, reported the way the issue's checklist demands: **loud, never
/// silent**. `debug_assert!` turns it into a panic on a debug build (where it is a programming error
/// worth stopping for) and an `Err` the caller must handle on the shipping build (where a degraded
/// device beats a wedged one).
///
/// Every refusal is a bug *on this board*, including a render one — which
/// [`ArenaGate::claim_render`](obc_app::ArenaGate::claim_render) documents as ordinary, because a
/// generic host may simply try each frame and skip. The board doesn't: the Recalculating freeze
/// stops map frames for the whole of a search and the transfer card covers the map for the whole of
/// a transfer, so by the time a claim is attempted the arena is free by construction. If that stops
/// being true, this is the line that says so.
#[inline(never)]
#[cold]
fn refuse(what: &'static str, error: ArenaError) -> ArenaError {
    match error {
        ArenaError::Busy(holder) => {
            defmt::error!("arena: {=str} claim refused — held by {}", what, defmt::Debug2Format(&holder))
        }
        ArenaError::NotHeld(holder) => {
            defmt::error!("arena: {=str} release refused — the holder is {}", what, defmt::Debug2Format(&holder))
        }
    }
    debug_assert!(false, "arena: a claim or release was refused — the gating upstream of it is wrong");
    error
}

/// Release `owner`, which must be the holder. Called only from a guard's `Drop`.
fn release(owner: ArenaOwner) {
    // SAFETY: see `gate`; the borrow ends with this statement.
    if let Err(e) = unsafe { gate().release(owner) } {
        refuse("guard drop", e);
    }
}

// ============================ The render arm ============================

/// The render arm, held for **one render span** — claim, render, drop. Never across an `.await`:
/// the board's render is the synchronous half of the #809 render/present split, and the present that
/// awaits runs after this guard is gone.
pub(crate) struct RenderGuard {
    /// `*const ()` makes the guard `!Send` and `!Sync`: ownership is the ride loop's, and a guard
    /// that could cross tasks would be a second owner-switcher the `&mut` gate cannot see.
    _not_send: PhantomData<*const ()>,
}

impl Deref for RenderGuard {
    type Target = obc_render::RenderScratch;
    fn deref(&self) -> &obc_render::RenderScratch {
        // SAFETY: the guard exists ⇒ the gate says `Render` owns the block, and `claim_render`
        // initialized it in place as a `RenderScratch`. Derived fresh from `arena_ptr`; no other
        // reference into the block is live (module doc).
        unsafe { &*(arena_ptr() as *const obc_render::RenderScratch) }
    }
}

impl DerefMut for RenderGuard {
    fn deref_mut(&mut self) -> &mut obc_render::RenderScratch {
        // SAFETY: as `deref`, and `&mut self` makes this the only live borrow through the guard.
        unsafe { &mut *(arena_ptr() as *mut obc_render::RenderScratch) }
    }
}

impl Drop for RenderGuard {
    fn drop(&mut self) {
        release(ArenaOwner::Render);
    }
}

/// Claim the arena for a map render.
///
/// The only precondition is that the arena is free — which *is* the render ⊥ nav enforcement,
/// because a live search is literally the holder. A refusal is not
/// fatal: the caller skips this frame's map redraw and the reflective panel keeps the last one.
///
/// Re-initializes the scratch in place ([`init_zeroed`](obc_render::RenderScratch::init_zeroed) — a
/// `memset` of the all-zero empty state) **when the gate says the block was last another arm's**,
/// because a `heapless::Vec` whose `len` is a stale A* node count would read past its own contents.
///
/// Not on every claim, though, and the difference is ~117 KB of `memset` per map frame. A
/// `RenderScratch` that a render span just gave back is still a valid, fully initialized
/// `RenderScratch`: every buffer in it is written before it is read within a frame (`collect` clears
/// the frame buffers it fills; the fill/stroke scratch clears its own runs), which is exactly the
/// discipline that made the pre-#1146 resident static — zeroed **once**, at boot, then reused for
/// every frame of the device's life — correct. So a Render → Render sequence answers
/// [`ArenaInit::Skippable`] and restores that, while every foreign-arm transition (and the
/// first-ever claim, over `.uninit`'s boot garbage) still pays. The rule is host-tested in
/// [`obc_app::arena_gate`], with the rest of them.
pub(crate) fn claim_render() -> Result<RenderGuard, ArenaError> {
    // SAFETY: see `gate`.
    let init = unsafe { gate() }.claim_render().map_err(|e| refuse("render", e))?;
    if init == ArenaInit::Required {
        // SAFETY: the claim succeeded, so nothing else references the block; `init_zeroed` fully
        // initializes the slot as an empty `RenderScratch` before the guard hands out a reference.
        unsafe { obc_render::RenderScratch::init_zeroed(arena_ptr() as *mut obc_render::RenderScratch) };
    }
    Ok(RenderGuard { _not_send: PhantomData })
}

// ============================ The nav arm ============================

/// The nav arm, held for a **whole search** — many ride-loop passes, by design: the A* table, the
/// tile cache and the planner all have to survive from one bounded step to the next (#499). The ride
/// loop keeps this guard in its own state and the Recalculating freeze is what keeps render claims
/// away meanwhile.
///
/// `!Send` for the same reason [`RenderGuard`] is.
#[cfg(has_nav)]
pub(crate) struct NavGuard {
    /// Whether [`begin_plan`](NavGuard::begin_plan) has written the arm's planner slot **for this
    /// claim**. It rides on the guard rather than in the arm because that is what makes it
    /// unforgeable: the guard is minted by [`claim_nav`] with the slot uninitialized, so the only
    /// way to reach a planner is to have written one, and dropping the guard takes the fact with it.
    planner_ready: bool,
    _not_send: PhantomData<*const ()>,
}

#[cfg(has_nav)]
impl NavGuard {
    /// Write a fresh planner into the arm's slot — the **only** way it is ever initialized (#499's
    /// per-request `ptr::write` discipline, now with the fact recorded).
    ///
    /// Called from an `#[inline(never)]` frame at the request drain: `NavPlanner::new` materializes
    /// a ~9 kB temporary, and inlined into the ride loop that temporary becomes a permanent slot in
    /// the main task's poll frame (the #501 HardFault's true cause).
    pub(crate) fn begin_plan(&mut self, planner: obc_route::NavPlanner) {
        // SAFETY: the guard exists ⇒ `Nav` owns the block; the write initializes the slot in place
        // and no other reference into the arena is live (module doc).
        unsafe { (*(arena_ptr() as *mut NavArm)).planner.write(planner) };
        self.planner_ready = true;
    }

    /// The three things one planner step touches, borrowed together — or `None` when no plan has
    /// been written into this claim's slot yet.
    ///
    /// One call rather than three accessors because the step needs all three at once and they are
    /// fields of one struct: splitting the borrow is the arena's job, not the caller's.
    pub(crate) fn plan_parts(
        &mut self,
    ) -> Option<(
        &mut obc_route::NavPlanner,
        &mut obc_route::NavScratch,
        &mut obc_reader::NavTileCache,
        &mut [u8; NAV_OUTPUT_STAGE_BYTES],
    )> {
        if !self.planner_ready {
            return None;
        }
        // SAFETY: the guard exists ⇒ `Nav` owns the block and `claim_nav` initialized the arm;
        // `planner_ready` says the slot was written by `begin_plan`. Derived fresh from `arena_ptr`,
        // and `&mut self` bounds all three borrows to this guard, so no second reference can exist.
        let arm = unsafe { &mut *(arena_ptr() as *mut NavArm) };
        let planner = unsafe { arm.planner.assume_init_mut() };
        Some((planner, &mut arm.scratch, &mut arm.tiles, &mut arm.output))
    }

    /// The written planner, for the read-only diagnostics the finish path reports (`settles`, the ε
    /// rung, the phase attribution) — `None` before [`begin_plan`](NavGuard::begin_plan).
    pub(crate) fn planner_ref(&self) -> Option<&obc_route::NavPlanner> {
        // SAFETY: as `plan_parts`, and the returned borrow is shared and bounded by `&self`.
        self.planner_ready.then(|| unsafe { (*(arena_ptr() as *const NavArm)).planner.assume_init_ref() })
    }

    /// The addresses the one plan-start diagnostic line reports (#501 fault dossiers): planner slot,
    /// A\* table, tile cache — offsets inside the arena since #1146 P2.
    pub(crate) fn arm_addrs(&self) -> (usize, usize, usize) {
        let arm = arena_ptr() as *const NavArm;
        // SAFETY: address arithmetic only — `addr_of!` forms no reference into the block.
        unsafe {
            (
                core::ptr::addr_of!((*arm).planner) as usize,
                core::ptr::addr_of!((*arm).scratch) as usize,
                core::ptr::addr_of!((*arm).tiles) as usize,
            )
        }
    }
}

#[cfg(has_nav)]
impl Deref for NavGuard {
    type Target = NavArm;
    fn deref(&self) -> &NavArm {
        // SAFETY: the guard exists ⇒ `Nav` owns the block and `claim_nav` initialized it in place.
        unsafe { &*(arena_ptr() as *const NavArm) }
    }
}

#[cfg(has_nav)]
impl DerefMut for NavGuard {
    fn deref_mut(&mut self) -> &mut NavArm {
        // SAFETY: as `deref`; `&mut self` makes this the only live borrow.
        unsafe { &mut *(arena_ptr() as *mut NavArm) }
    }
}

#[cfg(has_nav)]
impl Drop for NavGuard {
    fn drop(&mut self) {
        release(ArenaOwner::Nav);
    }
}

/// Claim the arena for a route search. Needs [`MapQuiesced`] — mint it off the app with
/// [`App::nav_arena_precondition`](obc_app::App::nav_arena_precondition), which is the only way to
/// get one, so this cannot be *called* without the evidence that no map render is coming.
///
/// Initializes the whole arm in place: a zero fill (which is exactly `NavScratch::new()` — the
/// format crate owns that invariant, and a zeroed planner slot is a `MaybeUninit` nobody reads
/// before the drain writes it), then the one field a zeroed image would get wrong — the tile
/// cache's `EMPTY` tags are `u32::MAX`, so [`NavTileCache::reset`](obc_reader::NavTileCache::reset)
/// stamps them. A cold cache on every search is the accepted cost (see [`NavArm::tiles`]).
#[cfg(has_nav)]
pub(crate) fn claim_nav(quiesced: MapQuiesced) -> Result<NavGuard, ArenaError> {
    // SAFETY: see `gate`.
    unsafe { gate() }.claim_nav(quiesced).map_err(|e| refuse("nav", e))?;
    // SAFETY: the claim succeeded, so nothing else references the block. The zero fill leaves a
    // valid `NavScratch` and a valid (uninitialized) planner slot; `reset` fixes the tile tags.
    unsafe {
        let arm = arena_ptr() as *mut NavArm;
        core::ptr::write_bytes(arm as *mut u8, 0, core::mem::size_of::<NavArm>());
        (*arm).tiles.reset();
    }
    // `planner_ready: false` — the zero fill above left a *slot*, not a planner. `begin_plan` is the
    // only thing that changes that, and it is what the accessors key on.
    Ok(NavGuard { planner_ready: false, _not_send: PhantomData })
}

// ============================ The USB arm ============================

/// Guard retained by the ride loop for one staged cable upload.
pub(crate) struct UsbGuard {
    _not_send: PhantomData<*const ()>,
}

impl Drop for UsbGuard {
    fn drop(&mut self) {
        release(ArenaOwner::Usb);
    }
}

/// Claim the USB byte stage after the transfer card has displaced map rendering and no route search
/// owns the nav arm. The bytes need no initialization: the engine tracks the initialized prefix.
pub(crate) fn claim_usb(ready: obc_app::TransferReady) -> Result<UsbGuard, ArenaError> {
    // SAFETY: see `gate`.
    unsafe { gate() }.claim_usb(ready).map_err(|e| refuse("usb", e))?;
    Ok(UsbGuard { _not_send: PhantomData })
}

/// Lend one stage bank synchronously. `None` means the grant was revoked or never arrived.
///
/// This deliberately never forms a reference over the complete USB arm. Once FLPR has started a
/// deferred write it retains the other bank as its DMA source; even a caller that touched only this
/// bank would make a whole-arm `&mut` alias that retained access.
pub(crate) fn with_usb_stage_bank<R>(
    bank: usize,
    f: impl FnOnce(&mut [u8; crate::usb::STAGE_HALF_LEN]) -> R,
) -> Option<R> {
    if owner() != ArenaOwner::Usb {
        return None;
    }
    if bank >= 2 {
        return None;
    }
    // SAFETY: the gate says USB owns the union. `bank` selects exactly one half, so this reference
    // excludes the opposite half that FLPR may still retain as a DMA source. The closure cannot
    // retain the borrow.
    let address = unsafe { (arena_ptr() as *mut u8).add(bank * crate::usb::STAGE_HALF_LEN) };
    Some(f(unsafe { &mut *(address as *mut [u8; crate::usb::STAGE_HALF_LEN]) }))
}

/// Whether a DMA source lies wholly inside the currently-owned USB arm. Numeric only: the sEMMC
/// driver retains the address until its explicit join and no Rust reference escapes this call.
pub(crate) fn usb_stage_contains(address: usize, len: usize) -> bool {
    if owner() != ArenaOwner::Usb {
        return false;
    }
    let start = arena_ptr() as usize;
    address >= start
        && address.checked_add(len).is_some_and(|end| end <= start + crate::usb::STAGE_LEN)
}
