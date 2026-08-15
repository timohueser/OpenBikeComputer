//! The one-transfer-at-a-time gate, and **which wire holds it** (issue #1039).
//!
//! The companion link arbitrates one transfer across every transport, because the resource being
//! arbitrated is the store rather than the wire: there is one upload handle and one open download
//! source, so two simultaneous uploads would interleave into the same file. A single flag says that
//! much, and for a long time a single flag was enough.
//!
//! It stopped being enough when the transfers grew: **one transfer at a time is not one transport
//! at a time**. Both links can be *connected* while only one is transferring, and each clears its
//! own state when it drops — so a phone walking out of range released the gate the cable was
//! holding, and the next descriptor on the radio was answered `busy`-free while a multi-gigabyte
//! volume set was still streaming. A flag cannot tell whose it is; an owner can.
//!
//! The rule lives here rather than on the board for the reason [`crate::set_upload`] does: the
//! board crate has no `test` harness in CI, and "a teardown releases only its own claim" is exactly
//! the kind of statement that should be asserted rather than reviewed.
//!
//! # The search arm (issue #1146, P2)
//!
//! The gate arbitrates a second resource now: the scratch arena's `nav ⊥ usb` rule (no reroute
//! while docked-transferring). A route search and a cable transfer want the same RAM. They are four
//! mutually-exclusive states in one tagged atomic: idle, BLE transfer, USB transfer, or search.
//! [`begin_search`](TransferGate::begin_search) and [`claim`](TransferGate::claim) therefore compete
//! in one compare-exchange instead of each checking one atomic before claiming another.
//!
//! Search deliberately remains outside [`GateOwner`]: `GateOwner` answers *which wire*, while a
//! search is on no wire. [`holder`](TransferGate::holder) therefore still returns `None` for search,
//! [`in_flight`](TransferGate::in_flight) still means *a transfer is streaming*, and
//! [`busy`](TransferGate::busy) remains the wider "may a new transfer start?" predicate.

use core::sync::atomic::{AtomicU8, Ordering};

const IDLE: u8 = 0;
const BLE: u8 = 1;
const USB: u8 = 2;
const SEARCH: u8 = 3;

/// Which wire holds the transfer gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateOwner {
    /// The BLE control plane armed it.
    Ble,
    /// The USB control plane armed it.
    Usb,
}

impl GateOwner {
    const fn tag(self) -> u8 {
        match self {
            GateOwner::Ble => BLE,
            GateOwner::Usb => USB,
        }
    }

    const fn from_tag(tag: u8) -> Option<GateOwner> {
        match tag {
            BLE => Some(GateOwner::Ble),
            USB => Some(GateOwner::Usb),
            _ => None,
        }
    }
}

/// The gate: idle, or held by one wire.
///
/// Every plane runs as a cooperative future on one executor, so `Relaxed` is the honest ordering —
/// there is no second core to publish to and no data being handed over, only a flag two futures
/// read between their own await points. [`claim`](Self::claim) is nonetheless a compare-exchange
/// rather than a store, so the gate cannot be taken twice even if that ever changes.
pub struct TransferGate {
    state: AtomicU8,
}

impl TransferGate {
    /// An idle gate.
    pub const fn new() -> TransferGate {
        TransferGate { state: AtomicU8::new(IDLE) }
    }

    /// Take the gate for `owner`. `false` = someone already holds it **or a route search is
    /// running**, and the caller must answer `busy` rather than arm.
    pub fn claim(&self, owner: GateOwner) -> bool {
        self.state.compare_exchange(IDLE, owner.tag(), Ordering::Relaxed, Ordering::Relaxed).is_ok()
    }

    /// Release the gate — **only if `owner` is the one holding it**. A teardown on the wire that
    /// is not transferring is a no-op, which is the whole point: it must not open the door on a
    /// transfer the other wire is still running.
    #[inline(never)]
    pub fn release(&self, owner: GateOwner) {
        let _ = self.state.compare_exchange(owner.tag(), IDLE, Ordering::Relaxed, Ordering::Relaxed);
    }

    /// Whether any transfer is in flight — the `busy` test, and it is deliberately wire-blind:
    /// a second transfer is refused whichever wire offers it.
    pub fn in_flight(&self) -> bool {
        matches!(self.state.load(Ordering::Relaxed), BLE | USB)
    }

    /// Who holds it, if anyone. Lets a control plane tell "my own transfer" from "the other wire's"
    /// — an abort aimed at a transfer this wire is not running cannot be forwarded to a data plane
    /// that is not listening.
    pub fn holder(&self) -> Option<GateOwner> {
        GateOwner::from_tag(self.state.load(Ordering::Relaxed))
    }

    /// Take the **search** side of the gate (issue #1146: the nav arm of the scratch arena).
    /// `false` = a transfer is streaming, so the search must not start — the rider's reroute waits
    /// for the cable, because the alternative is a planner writing its A* table over the bytes the
    /// USB data plane is staging into.
    ///
    /// Not owner-tracked: there is exactly one searcher (the ride loop), whereas transfers arrive on
    /// two independent wires.
    #[must_use = "a refused search must not start planning — the arena belongs to the transfer"]
    pub fn begin_search(&self) -> bool {
        self.state.compare_exchange(IDLE, SEARCH, Ordering::Relaxed, Ordering::Relaxed).is_ok()
    }

    /// End the search (the plan answered, failed, or was cancelled). Idempotent, and it releases
    /// **only** the search — a transfer that started after it is untouched, exactly as
    /// [`release`](TransferGate::release) leaves the other wire's claim alone.
    ///
    /// Keep this transition out of line: ARM's byte compare-exchange expands to an interrupt-safe
    /// retry loop, and duplicating that loop inside the ride task costs flash for a path that runs
    /// only when a route search ends.
    #[inline(never)]
    pub fn end_search(&self) {
        let _ = self.state.compare_exchange(SEARCH, IDLE, Ordering::Relaxed, Ordering::Relaxed);
    }

    /// Whether a route search holds the gate's search side. The
    /// [`TransferReady`](crate::arena_gate::TransferReady) precondition reads this.
    pub fn search_live(&self) -> bool {
        self.state.load(Ordering::Relaxed) == SEARCH
    }

    /// Whether a **new transfer** would be refused — a transfer already streaming *or* a live
    /// search. The `busy` test a `transferControl` open should answer from; [`in_flight`](TransferGate::in_flight)
    /// stays the narrower "is a transfer streaming" fact that abort routing and the data planes use.
    pub fn busy(&self) -> bool {
        self.state.load(Ordering::Relaxed) != IDLE
    }
}

impl Default for TransferGate {
    fn default() -> Self {
        TransferGate::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum ModelState {
        Idle,
        Ble,
        Usb,
        Search,
    }

    impl ModelState {
        const ALL: [ModelState; 4] = [ModelState::Idle, ModelState::Ble, ModelState::Usb, ModelState::Search];

        const fn tag(self) -> u8 {
            match self {
                ModelState::Idle => IDLE,
                ModelState::Ble => BLE,
                ModelState::Usb => USB,
                ModelState::Search => SEARCH,
            }
        }

        const fn holder(self) -> Option<GateOwner> {
            match self {
                ModelState::Ble => Some(GateOwner::Ble),
                ModelState::Usb => Some(GateOwner::Usb),
                ModelState::Idle | ModelState::Search => None,
            }
        }
    }

    #[derive(Debug, Clone, Copy)]
    enum Action {
        Claim(GateOwner),
        Release(GateOwner),
        BeginSearch,
        EndSearch,
    }

    impl Action {
        const ALL: [Action; 6] = [
            Action::Claim(GateOwner::Ble),
            Action::Claim(GateOwner::Usb),
            Action::Release(GateOwner::Ble),
            Action::Release(GateOwner::Usb),
            Action::BeginSearch,
            Action::EndSearch,
        ];
    }

    fn model_step(state: ModelState, action: Action) -> (ModelState, Option<bool>) {
        match action {
            Action::Claim(owner) => match state {
                ModelState::Idle => (
                    match owner {
                        GateOwner::Ble => ModelState::Ble,
                        GateOwner::Usb => ModelState::Usb,
                    },
                    Some(true),
                ),
                _ => (state, Some(false)),
            },
            Action::Release(owner) if state.holder() == Some(owner) => (ModelState::Idle, None),
            Action::Release(_) => (state, None),
            Action::BeginSearch if state == ModelState::Idle => (ModelState::Search, Some(true)),
            Action::BeginSearch => (state, Some(false)),
            Action::EndSearch if state == ModelState::Search => (ModelState::Idle, None),
            Action::EndSearch => (state, None),
        }
    }

    fn apply(gate: &TransferGate, action: Action) -> Option<bool> {
        match action {
            Action::Claim(owner) => Some(gate.claim(owner)),
            Action::Release(owner) => {
                gate.release(owner);
                None
            }
            Action::BeginSearch => Some(gate.begin_search()),
            Action::EndSearch => {
                gate.end_search();
                None
            }
        }
    }

    /// Exhaust the complete four-state transition table. This is stronger than sampling traces:
    /// every action's successor is itself one of these four rows, so arbitrary-length traces are
    /// closed under the transitions checked here.
    #[test]
    fn tagged_gate_matches_the_reference_model_for_every_transition() {
        for before in ModelState::ALL {
            for action in Action::ALL {
                let gate = TransferGate { state: AtomicU8::new(before.tag()) };
                let (after, expected_result) = model_step(before, action);

                assert_eq!(apply(&gate, action), expected_result, "{before:?} -> {action:?}");
                assert_eq!(gate.holder(), after.holder(), "holder after {before:?} -> {action:?}");
                assert_eq!(gate.in_flight(), matches!(after, ModelState::Ble | ModelState::Usb));
                assert_eq!(gate.search_live(), after == ModelState::Search);
                assert_eq!(gate.busy(), after != ModelState::Idle);
            }
        }
    }

    #[test]
    fn gate_is_exactly_one_atomic_byte() {
        assert_eq!(core::mem::size_of::<TransferGate>(), core::mem::size_of::<AtomicU8>());
        assert_eq!(core::mem::align_of::<TransferGate>(), core::mem::align_of::<AtomicU8>());
    }

    #[test]
    fn a_claimed_gate_refuses_a_second_claim_from_either_wire() {
        let gate = TransferGate::new();
        assert!(!gate.in_flight());
        assert_eq!(gate.holder(), None);

        assert!(gate.claim(GateOwner::Usb));
        assert!(gate.in_flight(), "the busy test is wire-blind");
        assert_eq!(gate.holder(), Some(GateOwner::Usb));
        assert!(!gate.claim(GateOwner::Ble), "the radio cannot start a second transfer");
        assert!(!gate.claim(GateOwner::Usb), "and neither can the cable");
    }

    /// **The regression.** A BLE teardown mid-USB-transfer used to clear the shared flag, which
    /// re-opened the gate on a transfer that was still streaming — so the next `transferControl`
    /// on the radio armed a second one into the same store.
    #[test]
    fn a_teardown_on_the_idle_wire_does_not_release_the_other_wires_transfer() {
        let gate = TransferGate::new();
        assert!(gate.claim(GateOwner::Usb), "the cable is uploading a volume set");

        gate.release(GateOwner::Ble); // the phone walked out of range
        assert!(gate.in_flight(), "the cable still holds it");
        assert_eq!(gate.holder(), Some(GateOwner::Usb));
        assert!(!gate.claim(GateOwner::Ble), "so the radio is still answered busy");

        gate.release(GateOwner::Usb); // the transfer concludes
        assert!(!gate.in_flight());
        assert!(gate.claim(GateOwner::Ble), "and now the radio may have it");
    }

    /// Releasing an idle gate, or releasing twice, is a no-op rather than an underflow — both
    /// happen for real: a transfer that answered clears the gate, and the link teardown right
    /// behind it clears it again.
    #[test]
    fn releasing_what_you_do_not_hold_is_a_no_op() {
        let gate = TransferGate::new();
        gate.release(GateOwner::Usb);
        assert!(!gate.in_flight());

        assert!(gate.claim(GateOwner::Ble));
        gate.release(GateOwner::Ble);
        gate.release(GateOwner::Ble);
        assert!(!gate.in_flight());
        assert!(gate.claim(GateOwner::Usb), "and the gate is usable afterwards");
    }

    // --- The `search ⊕ transfer` arm (issue #1146, P2) ---

    /// **The regression** the arm prevents: the rider reroutes while docked and the phone (or the
    /// cable) opens a transfer a moment later. Both want the scratch arena, and the transfer's
    /// staging buffer would land on the planner's live A* table.
    #[test]
    fn a_live_search_refuses_transfers_on_both_wires() {
        let gate = TransferGate::new();
        assert!(gate.begin_search(), "nothing streaming — the reroute may plan");
        assert!(gate.search_live());
        assert!(gate.busy(), "…and the gate answers busy to a new transfer");
        assert!(!gate.in_flight(), "though no transfer is streaming — the two facts stay distinct");

        assert!(!gate.claim(GateOwner::Usb), "the cable waits for the search");
        assert!(!gate.claim(GateOwner::Ble), "and so does the radio");
        assert_eq!(gate.holder(), None, "a refused claim leaves the gate unowned, so an abort still routes");

        gate.end_search();
        assert!(!gate.busy());
        assert!(gate.claim(GateOwner::Usb), "the transfer arms the moment the plan answers");
    }

    /// The mirror: a multi-gigabyte volume set is streaming and the rider asks for a detour. The
    /// search is refused (the UI answers "not while transferring"), never started half-owned.
    #[test]
    fn a_held_transfer_refuses_a_search() {
        let gate = TransferGate::new();
        assert!(gate.claim(GateOwner::Usb));
        assert!(!gate.begin_search(), "no reroute while docked-transferring");
        assert!(!gate.search_live(), "and the refusal left no half-set flag behind");

        gate.release(GateOwner::Usb);
        assert!(gate.begin_search(), "the transfer concluded — now it may plan");
    }

    /// Each side releases only itself. The two teardowns run on different clocks (a plan answers
    /// while a transfer streams, a link drops while a plan runs), so a release that cleared "the
    /// gate" wholesale would re-open the door on work still in flight — the #1039 lesson, applied
    /// to the second resource.
    #[test]
    fn tearing_down_one_side_never_releases_the_other() {
        let gate = TransferGate::new();
        assert!(gate.begin_search());
        gate.release(GateOwner::Usb); // a cable teardown arriving mid-search
        gate.release(GateOwner::Ble);
        assert!(gate.search_live(), "the search is untouched by a transfer teardown");

        gate.end_search();
        assert!(gate.claim(GateOwner::Ble), "the radio takes the gate once the search ends");
        gate.end_search(); // a stray second end_search behind the answer
        assert_eq!(gate.holder(), Some(GateOwner::Ble), "…and it does not release the radio's transfer");
        assert!(!gate.begin_search(), "which still refuses a new search");
    }

    /// `begin_search` twice is a bug, not a nesting: there is one searcher, and the second call
    /// would pair with an `end_search` that releases the first search's arena.
    #[test]
    fn a_second_search_claim_is_refused() {
        let gate = TransferGate::new();
        assert!(gate.begin_search());
        assert!(!gate.begin_search(), "one searcher, one claim");
        gate.end_search();
        assert!(gate.begin_search(), "and a fresh search after it ends is fine");
    }
}
