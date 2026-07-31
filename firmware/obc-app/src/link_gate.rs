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

use core::sync::atomic::{AtomicU8, Ordering};

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

/// The gate: idle, or held by one wire.
///
/// Every plane runs as a cooperative future on one executor, so `Relaxed` is the honest ordering —
/// there is no second core to publish to and no data being handed over, only a flag two futures
/// read between their own await points. [`claim`](Self::claim) is nonetheless a compare-exchange
/// rather than a store, so the gate cannot be taken twice even if that ever changes.
pub struct TransferGate {
    owner: AtomicU8,
}

impl TransferGate {
    /// An idle gate.
    pub const fn new() -> TransferGate {
        TransferGate { owner: AtomicU8::new(0) }
    }

    /// Take the gate for `owner`. `false` = someone already holds it, and the caller must answer
    /// `busy` rather than arm.
    pub fn claim(&self, owner: GateOwner) -> bool {
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
}
