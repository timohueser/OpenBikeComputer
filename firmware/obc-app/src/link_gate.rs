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
//! while docked-transferring). A route search and a cable transfer want the same RAM, and neither
//! belongs to a wire, so the gate carries a **search flag** beside the transfer owner —
//! [`begin_search`](TransferGate::begin_search) / [`end_search`](TransferGate::end_search) — and the
//! two exclude each other: a live search refuses [`claim`](TransferGate::claim), a held transfer
//! refuses `begin_search`.
//!
//! Deliberately a second flag rather than a third [`GateOwner`]: `GateOwner` answers *which wire*,
//! and a search is on no wire. Folding it in would have made [`holder`](TransferGate::holder) —
//! which routes an `Abort` to the data plane actually transferring — answer with something no
//! transport can equal, quietly turning aborts during a search into `busy`. So the split predicates
//! stay honest: [`in_flight`](TransferGate::in_flight) means *a transfer is streaming* (unchanged),
//! and [`busy`](TransferGate::busy) is the "may a new transfer start?" test a control plane answers
//! `busy` from. A control plane that still tests `in_flight` is not *wrong* — [`claim`](TransferGate::claim)
//! is the hard gate and refuses regardless — it is merely late and impolite, arming a transfer that
//! then cannot take the gate.

use core::sync::atomic::{AtomicBool, AtomicU8, Ordering};

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
            GateOwner::Ble => 1,
            GateOwner::Usb => 2,
        }
    }

    const fn from_tag(tag: u8) -> Option<GateOwner> {
        match tag {
            1 => Some(GateOwner::Ble),
            2 => Some(GateOwner::Usb),
            _ => None,
        }
    }
}

/// Opaque capability for one open upload sink.
///
/// The token deliberately carries only the sink's raw key. Ownership and destination remain with
/// the live board slot; terminal operations must match both this key and their expected destination.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SinkSession<K>(K);

impl<K: Copy + Eq> SinkSession<K> {
    /// Bind a token to the newly opened sink key.
    pub const fn new(key: K) -> Self {
        Self(key)
    }

    /// Whether this token still names the live sink.
    pub fn matches_key(self, key: K) -> bool {
        self.0 == key
    }

    /// Match the live key and require that the repository taking it owns its destination.
    pub fn matches<D: Eq>(self, key: K, actual: D, expected: D) -> bool {
        self.matches_key(key) && actual == expected
    }
}

/// The gate: idle, or held by one wire.
///
/// Every plane runs as a cooperative future on one executor, so `Relaxed` is the honest ordering —
/// there is no second core to publish to and no data being handed over, only a flag two futures
/// read between their own await points. [`claim`](Self::claim) is nonetheless a compare-exchange
/// rather than a store, so the gate cannot be taken twice even if that ever changes.
pub struct TransferGate {
    owner: AtomicU8,
    searching: AtomicBool,
}

impl TransferGate {
    /// An idle gate.
    pub const fn new() -> TransferGate {
        TransferGate { owner: AtomicU8::new(0), searching: AtomicBool::new(false) }
    }

    /// Take the gate for `owner`. `false` = someone already holds it **or a route search is
    /// running**, and the caller must answer `busy` rather than arm.
    pub fn claim(&self, owner: GateOwner) -> bool {
        if self.search_live() {
            return false;
        }
        self.owner.compare_exchange(0, owner.tag(), Ordering::Relaxed, Ordering::Relaxed).is_ok()
    }

    /// Release the gate — **only if `owner` is the one holding it**. A teardown on the wire that
    /// is not transferring is a no-op, which is the whole point: it must not open the door on a
    /// transfer the other wire is still running.
    pub fn release(&self, owner: GateOwner) {
        let _ = self.owner.compare_exchange(owner.tag(), 0, Ordering::Relaxed, Ordering::Relaxed);
    }

    /// Whether any transfer is in flight — the `busy` test, and it is deliberately wire-blind:
    /// a second transfer is refused whichever wire offers it.
    pub fn in_flight(&self) -> bool {
        self.owner.load(Ordering::Relaxed) != 0
    }

    /// Who holds it, if anyone. Lets a control plane tell "my own transfer" from "the other wire's"
    /// — an abort aimed at a transfer this wire is not running cannot be forwarded to a data plane
    /// that is not listening.
    pub fn holder(&self) -> Option<GateOwner> {
        GateOwner::from_tag(self.owner.load(Ordering::Relaxed))
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
        if self.in_flight() {
            return false;
        }
        self.searching.compare_exchange(false, true, Ordering::Relaxed, Ordering::Relaxed).is_ok()
    }

    /// End the search (the plan answered, failed, or was cancelled). Idempotent, and it releases
    /// **only** the search — a transfer that started after it is untouched, exactly as
    /// [`release`](TransferGate::release) leaves the other wire's claim alone.
    pub fn end_search(&self) {
        self.searching.store(false, Ordering::Relaxed);
    }

    /// Whether a route search holds the gate's search side. The
    /// [`TransferReady`](crate::arena_gate::TransferReady) precondition reads this.
    pub fn search_live(&self) -> bool {
        self.searching.load(Ordering::Relaxed)
    }

    /// Whether a **new transfer** would be refused — a transfer already streaming *or* a live
    /// search. The `busy` test a `transferControl` open should answer from; [`in_flight`](TransferGate::in_flight)
    /// stays the narrower "is a transfer streaming" fact that abort routing and the data planes use.
    pub fn busy(&self) -> bool {
        self.in_flight() || self.search_live()
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

    #[test]
    fn sink_session_rejects_stale_keys_and_wrong_destinations() {
        #[derive(Clone, Copy, PartialEq, Eq)]
        enum Destination {
            Route,
            Trip,
        }

        let session = SinkSession::new(7u32);
        assert!(session.matches(7, Destination::Route, Destination::Route));
        assert!(!session.matches(8, Destination::Route, Destination::Route));
        assert!(!session.matches(7, Destination::Route, Destination::Trip));
        assert_eq!(core::mem::size_of_val(&session), core::mem::size_of::<u32>());
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
