//! The **named cross-domain connections** — how one component reaches another inside a pass
//! (#1438, epic #1433 §6).
//!
//! There is no event bus here and no general event enum. Every way one domain can reach another is
//! a *named slot with a named type*, listed in one place, so the answer to "what can move between
//! these two components, and when?" is a field rather than a search through a dispatcher.
//!
//! | Producer | Consumer | Type | Delivery |
//! |---|---|---|---|
//! | `UiRuntime` | `CatalogMachine` | [`CatalogIntent`] | same pass |
//! | `RetentionMachine` | `CatalogMachine` | [`CatalogIntent`] (an expiry) | same pass |
//! | `CatalogMachine` | `Navigator` | [`ActiveRouteRemoved`] | same pass |
//! | `CatalogMachine` | `RetentionMachine` | [`CatalogIdentityChanged`] | next pass |
//! | `Navigator` | `RetentionMachine` | [`RouteActivated`] | next pass |
//! | any domain | `FaultState` | [`FaultNotices`] | same pass, producer is earlier |
//!
//! There is deliberately **no** `UiRuntime` → `Recorder` row and no ride-closed row. Recorder has no
//! machine in Phase 1, so a connection into it could only take the rider's finish one-shot away from
//! the legacy drain that still performs it — provisioning for a lifecycle nobody owns, at the cost of
//! destroying a rider request. #1397 S6 brings the connection back with the domain that needs it.
//!
//! There is deliberately **no** `UiRuntime` → `Navigator`, `DfuState` or `StorageInfo` row either,
//! and for the opposite reason: those domains exist, so a screen names its request straight to the
//! owner as the gesture happens (`Ctx::navigator`, `Ctx::dfu`, `Ctx::storage`). That is stronger
//! than a same-pass slot — the request is with its owner before stage 1 rather than at stage 4 —
//! and it is the only shape that also works for the hosts still driving `drain_host_commands`,
//! which run no pass at all until #1397 S6. A slot beside it would be a second place a rider's plan
//! lives, which is the defect #1397 S2 exists to remove.
//!
//! ## Which direction decides the timing
//!
//! The pass order (see [`pass`](super::pass)) is fixed, so a connection's timing is not a policy
//! choice — it follows from where the two components sit:
//!
//! - **Earlier → later** rides a [`Slot`]: the producer fills it, the consumer's stage takes it a
//!   few stages later, *in the same pass*. Nothing waits a frame for something already decided.
//! - **Later → earlier** rides a [`Deferred`]: it cannot reach backwards, so it waits.
//!   [`Connections::promote_deferred`] runs at the top of the next pass — **before any new user
//!   input** — and the earlier component consumes it in its own stage.
//!
//! ## Capacity and merge rules
//!
//! Every slot holds **one** value. What a *second* value of the same kind means is different per
//! connection, so each one states its rule rather than sharing a default:
//!
//! - **Intents and one-shots** ([`Slot`]): the first value stands and the second is handed back. The
//!   producer keeps it and offers it again next pass — backpressure, never a silent drop. This is
//!   why a producer checks [`Slot::is_empty`] *before* consuming its own one-shot: an intent it
//!   cannot deliver must stay where the rider left it.
//! - **Later-to-earlier deposits** ([`Deferred`]): the newest value replaces the older one. Both of
//!   them are levels — which identity the catalog holds, which route is active — and acting on a
//!   superseded level is worse than acting late. A deposit that must *queue* rather than replace
//!   arrives with the first connection that needs one.
//! - **Fault notices** ([`FaultNotices`]): accumulate. Two domains raising a warning in one pass
//!   both reach the rider; a bit set is the only shape that cannot lose one.
//!
//! A [`Deferred`] that still holds a value at the end of a pass makes the pass ask for **another
//! pass before sleep** ([`Connections::has_deferred`]) — the work is decided, it simply has not
//! reached its consumer yet, and sleeping on it would leave it sitting until the next input.

// Reached from the pass alone, which has no production caller yet — see [`pass`](super::pass).
#![allow(dead_code)]

use crate::catalog_state::CatalogIntent;
use crate::screen::WarningFlags;
use crate::CatalogObjectId;

use super::slots::Slot;
use super::Revision;

// ==================== the connection payloads ====================

/// `CatalogMachine` → `Navigator`, **same pass**: the route being followed is being removed.
///
/// The catalog emits this when it admits the deletion, not when the store confirms it: a route the
/// device has decided to delete is not a route the rider should still be guided along, and
/// Navigator advances after the catalog precisely so it can drop it in the same pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActiveRouteRemoved {
    /// The durable identity of the route leaving the catalog.
    pub route: CatalogObjectId,
}

/// `CatalogMachine` → `RetentionMachine`, **next pass**: the catalog's identity set moved.
///
/// Retention's sweep queue and its hourly gate were derived from an inventory that has since
/// changed, so it re-discovers rather than draining candidates against a picture that is gone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CatalogIdentityChanged {
    /// The store revision the catalog now reflects. A newer revision replaces an older deposit —
    /// an older one never displaces it.
    pub revision: Revision,
}

/// `Navigator` → `RetentionMachine`, **next pass**: a route became the active one.
///
/// An active route must not expire underneath the ride it is guiding, so retention stamps it once
/// per activation. Two activations before the next pass leave the route that is actually active.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RouteActivated {
    /// The durable identity of the newly active route.
    pub route: CatalogObjectId,
}

// ==================== the deferred slot ====================

/// A **later-to-earlier** connection: one value, deposited in one pass and consumed in the next.
///
/// Two halves, because "deposited" and "visible" are different states. [`defer`](Self::defer) fills
/// `pending`; [`promote`](Self::promote) — run once at the top of a pass, before any new input —
/// moves it to `ready`, where the consuming stage [`take`](Self::take)s it. A value deposited by a
/// later stage of *this* pass therefore cannot be seen by the earlier stage that already ran: the
/// backwards edge the pass order forbids is impossible rather than merely discouraged.
#[derive(Debug, PartialEq, Eq)]
pub struct Deferred<T> {
    pending: Option<T>,
    ready: Option<T>,
}

impl<T> Deferred<T> {
    /// An empty slot.
    pub const fn new() -> Self {
        Deferred { pending: None, ready: None }
    }

    /// Deposit a value for the next pass, replacing any deposit not yet promoted.
    ///
    /// Both connections that use this carry a *level* — which identity the catalog holds, which
    /// route is active — so a newer deposit makes an older one wrong rather than second in line.
    /// A connection whose deposits must queue instead arrives with the domain that needs one.
    pub fn defer(&mut self, value: T) {
        self.pending = Some(value);
    }

    /// Make a value deposited on an earlier pass visible to its consumer. Called once per pass,
    /// before anything else runs.
    ///
    /// A `ready` value the consumer did not take stays put and the deposit waits behind it: the
    /// slot is capacity one all the way through, so promotion can never overwrite an unconsumed
    /// value.
    pub fn promote(&mut self) {
        if self.ready.is_none() {
            self.ready = self.pending.take();
        }
    }

    /// Take the promoted value, if the previous pass left one.
    pub fn take(&mut self) -> Option<T> {
        self.ready.take()
    }

    /// Whether the slot holds anything at all — deposited, promoted, or both. A `true` here at the
    /// end of a pass is what makes the runtime run another pass before it sleeps.
    pub fn is_pending(&self) -> bool {
        self.pending.is_some() || self.ready.is_some()
    }
}

// ==================== the fault connection ====================

/// Any domain → `FaultState`, **same pass** (every producer runs before the fault stage).
///
/// The one connection that accumulates: warnings are a bit set, so two domains raising one in the
/// same pass both reach the rider. Nothing clears until the fault stage takes them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FaultNotices(WarningFlags);

impl FaultNotices {
    /// Nothing raised.
    pub const NONE: FaultNotices = FaultNotices(WarningFlags::NONE);

    /// Raise `flags`. Never displaces what another domain already raised.
    pub fn raise(&mut self, flags: WarningFlags) {
        self.0 |= flags;
    }

    /// Take everything raised this pass, clearing the set.
    pub fn take(&mut self) -> WarningFlags {
        core::mem::replace(&mut self.0, WarningFlags::NONE)
    }

    /// Whether anything is raised.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

// ==================== the connection set ====================

/// Every cross-domain connection DeviceCore has, in one value.
///
/// Resident, because the deferred half of it must survive between passes. The same-pass slots are
/// empty at every pass boundary — their consumer's stage always runs after their producer's.
#[derive(Debug, PartialEq, Eq)]
pub struct Connections {
    /// `UiRuntime` → `CatalogMachine`: the rider's delete, resolved to a durable identity.
    pub ui_catalog: Slot<CatalogIntent>,
    /// `RetentionMachine` → `CatalogMachine`: an expiry, as the same intent a rider's delete uses,
    /// so an auto-expired object leaves by exactly the path a deleted one does.
    pub expiry: Slot<CatalogIntent>,
    /// `CatalogMachine` → `Navigator`: the followed route is being removed.
    pub active_route_removed: Slot<ActiveRouteRemoved>,
    /// Any domain → `FaultState`.
    pub faults: FaultNotices,
    /// `CatalogMachine` → `RetentionMachine`, next pass.
    pub catalog_identity: Deferred<CatalogIdentityChanged>,
    /// `Navigator` → `RetentionMachine`, next pass.
    pub route_activated: Deferred<RouteActivated>,
}

impl Connections {
    /// Nothing in flight — the boot state.
    pub const fn new() -> Self {
        Connections {
            ui_catalog: Slot::new(),
            expiry: Slot::new(),
            active_route_removed: Slot::new(),
            faults: FaultNotices::NONE,
            catalog_identity: Deferred::new(),
            route_activated: Deferred::new(),
        }
    }

    /// Make the previous pass's later-to-earlier deposits visible. Run once, at the top of a pass,
    /// before any new gesture, sensor reading or fact is applied.
    pub fn promote_deferred(&mut self) {
        self.catalog_identity.promote();
        self.route_activated.promote();
    }

    /// Whether any deferred connection still holds a value — the pass's "run again before sleep"
    /// test.
    pub fn has_deferred(&self) -> bool {
        self.catalog_identity.is_pending() || self.route_activated.is_pending()
    }
}

impl Default for Connections {
    fn default() -> Self {
        Connections::new()
    }
}

// Layout tripwires. Connections are resident state on the device, and every payload here is an
// identity, a revision or a flag — a growth means bulk crept into a message.
const _: () = assert!(core::mem::size_of::<ActiveRouteRemoved>() <= 8, "one durable identity");
const _: () = assert!(core::mem::size_of::<CatalogIdentityChanged>() <= 8, "one revision");
const _: () = assert!(core::mem::size_of::<RouteActivated>() <= 8, "one durable identity");
const _: () = assert!(core::mem::size_of::<FaultNotices>() <= 4, "a bit set");
const _: () = assert!(core::mem::size_of::<Connections>() <= 160, "six bounded slots");

#[cfg(test)]
mod tests {
    use super::*;

    fn identity(revision: u64) -> CatalogIdentityChanged {
        CatalogIdentityChanged { revision: Revision::new(revision) }
    }

    /// The deferred contract in the shape the pass uses it: a value deposited during a pass is
    /// invisible until the *next* pass promotes it. This is what makes a later-to-earlier edge
    /// impossible rather than merely discouraged.
    #[test]
    fn a_deferred_value_is_invisible_until_the_next_pass_promotes_it() {
        let mut slot: Deferred<RouteActivated> = Deferred::new();
        assert!(!slot.is_pending());

        // Pass 1, a late stage deposits.
        slot.defer(RouteActivated { route: 7 });
        assert!(slot.take().is_none(), "an earlier stage of the same pass cannot see it");
        assert!(slot.is_pending(), "…and it is what asks for another pass");

        // Pass 2, before any input.
        slot.promote();
        assert_eq!(slot.take(), Some(RouteActivated { route: 7 }));
        assert!(!slot.is_pending(), "consumed");
        slot.promote();
        assert!(slot.take().is_none(), "promotion invents nothing");
    }

    /// An identity is a level, so a newer revision replaces an older deposit rather than queueing
    /// behind it.
    #[test]
    fn a_deferred_slot_holds_the_newest_identity() {
        let mut slot: Deferred<CatalogIdentityChanged> = Deferred::new();
        slot.defer(identity(4));
        slot.defer(identity(9));

        slot.promote();
        assert_eq!(slot.take(), Some(identity(9)), "acting on a superseded identity is the worse failure");
        assert!(!slot.is_pending());
    }

    /// An unconsumed promoted value is never overwritten by a later deposit: capacity one holds
    /// through promotion, so the deposit simply waits its turn.
    #[test]
    fn promotion_never_displaces_an_unconsumed_value() {
        let mut slot: Deferred<CatalogIdentityChanged> = Deferred::new();
        slot.defer(identity(1));
        slot.promote();

        // The consumer skipped its stage this pass; a newer deposit arrives.
        slot.defer(identity(2));
        slot.promote();
        assert_eq!(slot.take(), Some(identity(1)), "the promoted value stands");
        slot.promote();
        assert_eq!(slot.take(), Some(identity(2)), "and the deposit follows it");
    }

    /// Faults accumulate: two domains raising in one pass both reach the rider, and taking clears.
    #[test]
    fn fault_notices_accumulate_from_every_producer() {
        let mut faults = FaultNotices::NONE;
        assert!(faults.is_empty());

        faults.raise(WarningFlags::REC_ERROR);
        faults.raise(WarningFlags::SETTINGS_ERROR);
        assert!(!faults.is_empty());

        let taken = faults.take();
        assert!(taken.contains(WarningFlags::REC_ERROR) && taken.contains(WarningFlags::SETTINGS_ERROR));
        assert!(faults.take().is_empty(), "taking clears the set");
    }

    /// The same-pass intent slots are capacity one with the first value winning — the rule a
    /// producer honours by checking the slot *before* it consumes its own one-shot.
    #[test]
    fn a_same_pass_intent_slot_keeps_the_first_intent() {
        let mut wires = Connections::new();
        assert!(wires.ui_catalog.is_empty() && !wires.has_deferred());

        wires.ui_catalog.try_put(CatalogIntent::DeleteRoute { id: 3 }).unwrap();
        let err = wires.ui_catalog.try_put(CatalogIntent::DeleteRide { id: 4 }).expect_err("occupied");
        assert_eq!(err.rejected, CatalogIntent::DeleteRide { id: 4 });
        assert_eq!(wires.ui_catalog.take(), Some(CatalogIntent::DeleteRoute { id: 3 }));
    }

    /// Any deferred connection keeps the runtime awake; the same-pass ones never do, because their
    /// consumer always runs later in the very pass that filled them.
    #[test]
    fn only_deferred_connections_ask_for_another_pass() {
        let mut wires = Connections::new();
        wires.ui_catalog.try_put(CatalogIntent::Refresh).unwrap();
        wires.active_route_removed.try_put(ActiveRouteRemoved { route: 1 }).unwrap();
        wires.faults.raise(WarningFlags::NO_GPS);
        assert!(!wires.has_deferred(), "a same-pass slot is drained by the pass that filled it");

        wires.route_activated.defer(RouteActivated { route: 1 });
        assert!(wires.has_deferred());
        wires.promote_deferred();
        assert!(wires.has_deferred(), "promoted but unconsumed still counts");
        assert!(wires.route_activated.take().is_some());
        assert!(!wires.has_deferred());
    }
}
