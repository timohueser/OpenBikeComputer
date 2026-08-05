//! The **scratch arena** (issue #1146, P2) — one block of RAM, three arms, one owner at a time.
//!
//! Three of the board's largest resident blocks are never live at the same moment, and each used to
//! own its bytes permanently: the per-frame render scratch (`obc_render::RenderScratch`, 92,320 B),
//! the nav block ([`NavArm`] — `NavScratch` + `NavTileCache` + the resumable `NavPlanner`,
//! ~59.9 KB), and the USB upload staging buffer ([`usb::STAGE_LEN`](crate::usb::STAGE_LEN),
//! 16 KiB). This module time-shares them through a `union`, so the board pays **max(arms)** instead
//! of their sum.
//!
//! **This is the only place in the feature `unsafe` lives.** Everything outside is ordinary
//! borrow-checked code reaching the arms through guards.
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
//! | render ⊥ usb | a cable transfer shows the transfer screen, not the map | [`TransferReady`] |
//! | nav ⊥ usb | no reroute while docked-transferring | [`TransferReady`] + `TransferGate`'s search arm |
//!
//! # The ride loop is the sole owner-switcher
//!
//! [`ArenaGate`] takes `&mut self`, so this module keeps it in a `static mut` reachable only from
//! here and every claim/release runs on the one thread-mode ride loop (the #677 async-frame
//! discipline: guards are `!Send` and are never held across an `.await` where another claimant
//! could run). The USB data plane never claims — it *asks*, and the loop grants between frames (see
//! [`with_usb_stage`]).
//!
//! # No two references at once
//!
//! Every reference into the arena — a guard's `Deref`, the chrome loan, the USB plane's scoped
//! access — is derived **freshly from the one raw pointer** ([`arena_ptr`]) and dies before the next
//! one is made. Nothing here stores a reference, so no two live borrows of the block can exist even
//! though three types name the same bytes.
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
//!   Today that is ~32 KB of free headroom for the nav arm and ~76 KB for the USB stage.
//! - Growing the **maximum** arm (today: render) costs the full delta, 1:1, exactly as before.
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

use obc_app::{ArenaError, ArenaGate, ArenaOwner, MapQuiesced, TransferReady};

/// The **nav arm**: everything one route search needs, as one struct so the union has one member
/// to name (epic #116 R4 + #499).
///
/// Deliberately **without** the map's terrain. `TERRAIN` is sampled at *fix* cadence during a ride
/// (`App::sample_terrain`, EL8) — that is, while the map plane is rendering and no search is
/// running — so it is genuinely always-live state and stays a resident static of its own. It is the
/// one nav-adjacent block that would have been wrong to fold in here, and the budget names it
/// separately for that reason.
#[cfg(has_nav)]
pub(crate) struct NavArm {
    /// The fixed A* table (~39.9 KB). Reset by the planner's first step of every request.
    pub(crate) scratch: obc_route::NavScratch,
    /// The graph-tile cache (~4.1 KB). Warmth across searches was never correctness — losing it
    /// costs cold 512 B tile reads on the next search, which is what its `misses` counter measures.
    pub(crate) tiles: obc_reader::NavTileCache,
    /// The resumable planner's slot (~15.8 KB, it owns the OBCR emitter across steps). Keeps its
    /// per-request `ptr::write` discipline (#499): the drain writes a fresh planner here and the
    /// step path reads it only while the loop's `NavRun` bookkeeping says a plan is active.
    pub(crate) planner: MaybeUninit<obc_route::NavPlanner>,
}

/// The arena itself: one block, three overlapping views.
///
/// `repr(C, align(8))` rather than a plain union so the block's address is stable and 8-aligned for
/// every arm regardless of which member the compiler would otherwise have led with — the nav arm's
/// `u64`-containing entries and the render scratch's `(i32, i32)` vectors both want it, and a
/// misaligned in-place `ptr::write` of the planner would fault on this strict-align target.
#[repr(C, align(8))]
union ScratchArena {
    /// The per-frame render scratch — the **largest** arm, so it sets the arena's size.
    render: ManuallyDrop<obc_render::RenderScratch>,
    #[cfg(has_nav)]
    nav: ManuallyDrop<NavArm>,
    /// The USB upload staging buffer. A plain byte buffer: it needs no initialization, the plane
    /// fills it before it reads it.
    usb: ManuallyDrop<[u8; crate::usb::STAGE_LEN]>,
}

/// The arena's resident size — `max(arms)`, the single term the board's RAM budget carries in place
/// of the three blocks it replaced.
pub(crate) const ARENA_BYTES: usize = core::mem::size_of::<ScratchArena>();
/// The render arm's size, for the resource report.
pub(crate) const RENDER_ARM_BYTES: usize = core::mem::size_of::<obc_render::RenderScratch>();
/// The nav arm's size, for the resource report. Reported as the *type's* size in every profile,
/// like the terrain entries, even where `has_nav` would keep it out of the image.
#[cfg(has_nav)]
pub(crate) const NAV_ARM_BYTES: usize = core::mem::size_of::<NavArm>();
#[cfg(not(has_nav))]
pub(crate) const NAV_ARM_BYTES: usize = core::mem::size_of::<obc_route::NavScratch>()
    + core::mem::size_of::<obc_reader::NavTileCache>()
    + core::mem::size_of::<obc_route::NavPlanner>();
/// The USB arm's size, for the resource report.
pub(crate) const USB_ARM_BYTES: usize = crate::usb::STAGE_LEN;

// **`max(arms)`, stated rather than assumed** — and the equality half is the one that bites. If a
// future edit grows the nav or USB arm past the render arm, the budget is still *correct* (it is a
// `size_of` either way), but the cliff has moved: every growth note in this module and at `main.rs`'s
// budget assert would then name the wrong arm, and the next reviewer would price a free growth as a
// 1:1 one (or the reverse). Fail the build rather than let the docs go quietly stale.
const _: () = assert!(
    NAV_ARM_BYTES <= ARENA_BYTES && USB_ARM_BYTES <= ARENA_BYTES && ARENA_BYTES == RENDER_ARM_BYTES,
    "the render arm is no longer the largest — the max-of-arms cliff moved; re-read the growth-asymmetry \
     notes in arena.rs and at main.rs's budget assert before re-pinning anything"
);

/// The arena's storage, in the **`.uninit`** section (cortex-m-rt's `link.x`: `NOLOAD`, placed
/// after `.bss`, never touched by the reset handler's zeroing loop).
///
/// `.bss` would be wrong twice over. It would cost ~92 KB of `memset` on **every** boot for bytes
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
/// The only precondition is that the arena is free — which *is* the render ⊥ nav / render ⊥ usb
/// enforcement, because a live search or a live transfer is literally the holder. A refusal is not
/// fatal: the caller skips this frame's map redraw and the reflective panel keeps the last one.
///
/// Re-initializes the scratch in place on every claim
/// ([`init_zeroed`](obc_render::RenderScratch::init_zeroed) — a `memset` of the all-zero empty
/// state), because the bytes may have been the nav arm's A* table a moment ago and a
/// `heapless::Vec` whose `len` is a stale node count would read past its own contents.
pub(crate) fn claim_render() -> Result<RenderGuard, ArenaError> {
    // SAFETY: see `gate`.
    unsafe { gate() }.claim_render().map_err(|e| refuse("render", e))?;
    // SAFETY: the claim succeeded, so nothing else references the block; `init_zeroed` fully
    // initializes the slot as an empty `RenderScratch` before the guard hands out a reference.
    unsafe { obc_render::RenderScratch::init_zeroed(arena_ptr() as *mut obc_render::RenderScratch) };
    Ok(RenderGuard { _not_send: PhantomData })
}

/// How a frame reaches the render scratch: an owned claim, or the chrome **loan**.
///
/// The loan exists because `App::render_map_timed` takes `&mut RenderScratch` on *every* frame,
/// including the ones that never touch it. Exactly one draw path in `obc-app` reads or writes the
/// scratch — `screen/map.rs`, the base-screen Map draw — so a frame whose base is chrome (a menu,
/// the nav-planning spinner, the map-transfer card) provably leaves the block alone. Those frames
/// must keep rendering while a search or a transfer holds the arena: the spinner is the only sign of
/// life during a plan, and the transfer card is the only explanation for a device whose SD bus is
/// saturated for minutes. Claiming for them would refuse both.
///
/// So the loan hands out a `&mut RenderScratch` over bytes another arm may own, and its safety rests
/// on three facts, all checked or checkable:
///
/// 1. **Aliasing.** The reference is derived fresh from [`arena_ptr`] and dies with the render call;
///    the nav guard and the USB plane materialize their own references only inside their own
///    synchronous calls, which never run inside a render.
/// 2. **Validity.** `RenderScratch` is `heapless::Vec`s over `MaybeUninit` backing arrays plus
///    `usize` lengths — every bit pattern is a *valid* value, so a `&mut` over foreign bytes is not
///    itself UB. (It would of course be nonsense to *use*; see 3.)
/// 3. **Nobody touches it.** Guarded by [`FrameScratch::chrome`]'s `debug_assert!` on
///    `!base_draws_map()`, and by the audit above.
///
/// The honest fix is upstream — a `render_*` entry point that does not demand a scratch it will not
/// use (or an `Option<&mut RenderScratch>`) — at which point the loan deletes itself. Until then
/// this is the whole of it, in one type, with the reasoning attached.
pub(crate) enum FrameScratch {
    /// A map-base frame: the arena is ours for the render span.
    Owned(RenderGuard),
    /// A chrome frame: see above.
    Loan,
}

impl FrameScratch {
    /// The chrome loan. `base_draws_map` is the caller's claim that this frame's base screen is not
    /// the map — the one predicate that decides whether the render will touch the scratch.
    pub(crate) fn chrome(base_draws_map: bool) -> FrameScratch {
        debug_assert!(!base_draws_map, "a map-base frame must claim the render arm, never borrow it");
        FrameScratch::Loan
    }

    /// The `&mut` the render call takes, borrowed for the length of that call only.
    pub(crate) fn get(&mut self) -> &mut obc_render::RenderScratch {
        match self {
            FrameScratch::Owned(guard) => &mut *guard,
            // SAFETY: see the type doc — derived fresh from `arena_ptr`, bounded by `&mut self`, and
            // never dereferenced by a chrome frame's draw.
            FrameScratch::Loan => unsafe { &mut *(arena_ptr() as *mut obc_render::RenderScratch) },
        }
    }
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
    _not_send: PhantomData<*const ()>,
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
    Ok(NavGuard { _not_send: PhantomData })
}

// ============================ The USB arm ============================

/// The USB staging arm, held for a **whole cable transfer** — and held by the *ride loop*, not by
/// the data plane. The plane never claims: it asks (`usb::request_stage`), the loop grants between
/// frames, and the plane reaches the bytes through [`with_usb_stage`].
///
/// That split is not ceremony. The plane's transfer future is suspended, not dropped, when the cable
/// goes (`usb::run` stops polling it until VBUS returns), so a `&'static mut` handed into that frame
/// could never be taken back — an unplug mid-upload would leave the arena owned by a task that may
/// never run again, and the map would never redraw. A scoped accessor makes revocation a
/// one-instruction fact instead: the loop drops this guard, and the plane's next append fails
/// politely and discards its partial upload, which is what an interrupted upload does anyway.
pub(crate) struct UsbGuard {
    _not_send: PhantomData<*const ()>,
}

impl Drop for UsbGuard {
    fn drop(&mut self) {
        release(ArenaOwner::Usb);
    }
}

/// Claim the arena for USB staging. Needs [`TransferReady`] — the transfer screen is up (so no map
/// render is coming) and no search is running (whose nav arm the staging bytes would overwrite).
///
/// No in-place initialization: the arm is a byte buffer the plane writes before it reads, and its
/// `Stage` tracks how much of it is live.
pub(crate) fn claim_usb(ready: TransferReady) -> Result<UsbGuard, ArenaError> {
    // SAFETY: see `gate`.
    unsafe { gate() }.claim_usb(ready).map_err(|e| refuse("usb", e))?;
    Ok(UsbGuard { _not_send: PhantomData })
}

/// Run `f` over the USB staging arm — the data plane's **only** access path, and it succeeds only
/// while the ride loop holds [`UsbGuard`].
///
/// `None` means the arm is not (or is no longer) the plane's: the loop never granted it, or it
/// revoked the grant on an unplug. The plane treats that exactly as a failed card append — discard
/// the partial and answer `error` — so a revocation can never turn into bytes written over another
/// arm.
///
/// The closure is **synchronous by type**, which is what keeps the reference off the plane's async
/// frame: nothing borrowed from the arena can survive an `.await` here.
pub(crate) fn with_usb_stage<R>(f: impl FnOnce(&mut [u8; crate::usb::STAGE_LEN]) -> R) -> Option<R> {
    if owner() != ArenaOwner::Usb {
        return None;
    }
    // SAFETY: the gate says `Usb` owns the block, so no other arm's reference is live; the
    // reference is derived fresh from `arena_ptr` and dies with `f`.
    Some(f(unsafe { &mut *(arena_ptr() as *mut [u8; crate::usb::STAGE_LEN]) }))
}
