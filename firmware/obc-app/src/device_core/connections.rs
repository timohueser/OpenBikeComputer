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
//! | `UiRuntime` | `Recorder` | [`RecorderIntent`] | same pass |
//! | `RetentionMachine` | `CatalogMachine` | [`CatalogIntent`] (an expiry) | same pass |
//! | `CatalogMachine` | `Navigator` | [`ActiveRouteRemoved`] | same pass |
//! | `CatalogMachine` | `RetentionMachine` | [`CatalogIdentityChanged`] | next pass |
//! | `Recorder` | `RetentionMachine` | [`RideClosed`] | next pass |
//! | `Navigator` | `RetentionMachine` | [`RouteActivated`] | next pass |
//! | any domain | `FaultState` | [`FaultNotices`] | same pass, producer is earlier |
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
//! - **Intents and one-shots** ([`Slot`], [`Merge::KeepFirst`]): the first value stands and the
//!   second is handed back. The producer keeps it and offers it again next pass — backpressure,
//!   never a silent drop. This is why a producer checks [`Slot::is_empty`] *before* consuming its
//!   own one-shot: an intent it cannot deliver must stay where the rider left it.
//! - **Identity changes** ([`Merge::KeepLatest`]): a level, not an event. The newest revision
//!   replaces the older one, because acting on a superseded identity is worse than acting late.
//! - **Fault notices** ([`FaultNotices`]): accumulate. Two domains raising a warning in one pass
//!   both reach the rider; a bit set is the only shape that cannot lose one.
//!
//! A [`Deferred`] that still holds a value at the end of a pass makes the pass ask for **another
//! pass before sleep** ([`Connections::has_deferred`]) — the work is decided, it simply has not
//! reached its consumer yet, and sleeping on it would leave it sitting until the next input.

use crate::catalog_state::CatalogIntent;
use crate::recorder::RecorderIntent;
use crate::screen::WarningFlags;
use crate::CatalogObjectId;

use super::slots::{Slot, SlotFull};
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
    /// The store revision the catalog now reflects. [`Merge::KeepLatest`]: an older revision never
    /// displaces a newer one.
    pub revision: Revision,
}

/// `Recorder` → `RetentionMachine`, **next pass**: the open ride was closed.
///
/// The epic's table calls this row "ride finalized or synced". The *synced* half reaches retention
/// today as a fact about the ride inventory (it stamps `synced_at` eagerly from its own view), so
/// what the recorder itself has to say is the half only it knows: the ride is over, and the
/// inventory is about to change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RideClosed {
    /// Whether the ride was thrown away rather than kept.
    pub discarded: bool,
}

/// `Navigator` → `RetentionMachine`, **next pass**: a route became the active one.
///
/// An active route must not expire underneath the ride it is guiding, so retention stamps it once
/// per activation. [`Merge::KeepLatest`]: two activations in one pass leave the route that is
/// actually active.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RouteActivated {
    /// The durable identity of the newly active route.
    pub route: CatalogObjectId,
}

// ==================== the deferred slot ====================

/// What a second value in an occupied slot means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Merge {
    /// The first value stands; the second is handed back to its producer, which keeps it pending.
    KeepFirst,
    /// The newest value replaces the held one — for a *level*, where the older value is simply
    /// wrong once a newer one exists.
    KeepLatest,
}

/// A **later-to-earlier** connection: one value, deposited in one pass and consumed in the next.
///
/// Two halves, because "deposited" and "visible" are different states. [`defer`](Self::defer) fills
/// `pending`; [`promote`](Self::promote) — run once at the top of a pass, before any new input —
/// moves it to `ready`, where the consuming stage [`take`](Self::take)s it. A value deposited by a
/// later stage of *this* pass therefore cannot be seen by the earlier stage that already ran: the
/// backwards edge the pass order forbids is impossible rather than merely discouraged.
#[derive(Debug, PartialEq, Eq)]
pub struct Deferred<T> {
    merge: Merge,
    pending: Option<T>,
    ready: Option<T>,
}

impl<T> Deferred<T> {
    /// An empty slot with the given merge rule.
    pub const fn new(merge: Merge) -> Self {
        Deferred { merge, pending: None, ready: None }
    }

    /// Deposit a value for the next pass.
    ///
    /// Under [`Merge::KeepFirst`] a full slot refuses and hands `value` back — its producer keeps it
    /// and offers it again once the slot drains. Under [`Merge::KeepLatest`] it replaces, so this
    /// never fails.
    pub fn defer(&mut self, value: T) -> Result<(), SlotFull<T>> {
        match self.merge {
            Merge::KeepLatest => {
                self.pending = Some(value);
                Ok(())
            }
            Merge::KeepFirst if self.pending.is_some() => Err(SlotFull { rejected: value }),
            Merge::KeepFirst => {
                self.pending = Some(value);
                Ok(())
            }
        }
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
    /// `UiRuntime` → `Recorder`: save or discard the open ride.
    pub ui_recorder: Slot<RecorderIntent>,
    /// `RetentionMachine` → `CatalogMachine`: an expiry, as the same intent a rider's delete uses,
    /// so an auto-expired object leaves by exactly the path a deleted one does.
    pub expiry: Slot<CatalogIntent>,
    /// `CatalogMachine` → `Navigator`: the followed route is being removed.
    pub active_route_removed: Slot<ActiveRouteRemoved>,
    /// Any domain → `FaultState`.
    pub faults: FaultNotices,
    /// `CatalogMachine` → `RetentionMachine`, next pass.
    pub catalog_identity: Deferred<CatalogIdentityChanged>,
    /// `Recorder` → `RetentionMachine`, next pass.
    pub ride_closed: Deferred<RideClosed>,
    /// `Navigator` → `RetentionMachine`, next pass.
    pub route_activated: Deferred<RouteActivated>,
}

impl Connections {
    /// Nothing in flight — the boot state.
    pub const fn new() -> Self {
        Connections {
            ui_catalog: Slot::new(),
            ui_recorder: Slot::new(),
            expiry: Slot::new(),
            active_route_removed: Slot::new(),
            faults: FaultNotices::NONE,
            // An identity is a level: the newest one is the only one worth acting on.
            catalog_identity: Deferred::new(Merge::KeepLatest),
            // A closed ride is an event: the second one waits rather than erasing the first.
            ride_closed: Deferred::new(Merge::KeepFirst),
            // An activation is a level: what matters is which route is active now.
            route_activated: Deferred::new(Merge::KeepLatest),
        }
    }

    /// Make the previous pass's later-to-earlier deposits visible. Run once, at the top of a pass,
    /// before any new gesture, sensor reading or fact is applied.
    pub fn promote_deferred(&mut self) {
        self.catalog_identity.promote();
        self.ride_closed.promote();
        self.route_activated.promote();
    }

    /// Whether any deferred connection still holds a value — the pass's "run again before sleep"
    /// test.
    pub fn has_deferred(&self) -> bool {
        self.catalog_identity.is_pending() || self.ride_closed.is_pending() || self.route_activated.is_pending()
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
const _: () = assert!(core::mem::size_of::<RideClosed>() <= 1, "one flag");
const _: () = assert!(core::mem::size_of::<RouteActivated>() <= 8, "one durable identity");
const _: () = assert!(core::mem::size_of::<FaultNotices>() <= 4, "a bit set");
const _: () = assert!(core::mem::size_of::<Connections>() <= 192, "eight bounded slots");

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
        let mut slot: Deferred<RouteActivated> = Deferred::new(Merge::KeepLatest);
        assert!(!slot.is_pending());

        // Pass 1, a late stage deposits.
        slot.defer(RouteActivated { route: 7 }).unwrap();
        assert!(slot.take().is_none(), "an earlier stage of the same pass cannot see it");
        assert!(slot.is_pending(), "…and it is what asks for another pass");

        // Pass 2, before any input.
        slot.promote();
        assert_eq!(slot.take(), Some(RouteActivated { route: 7 }));
        assert!(!slot.is_pending(), "consumed");
        slot.promote();
        assert!(slot.take().is_none(), "promotion invents nothing");
    }

    /// `KeepFirst`: a full slot refuses and hands the value back, so its producer keeps it. Nothing
    /// is overwritten and nothing is lost.
    #[test]
    fn a_full_keep_first_slot_hands_the_second_value_back() {
        let mut slot: Deferred<RideClosed> = Deferred::new(Merge::KeepFirst);
        let first = RideClosed { discarded: false };
        let second = RideClosed { discarded: true };

        slot.defer(first).unwrap();
        let err = slot.defer(second).expect_err("a full slot refuses");
        assert_eq!(err.rejected, second, "the producer gets its value back unchanged");

        slot.promote();
        assert_eq!(slot.take(), Some(first), "the first value is what the consumer sees");

        // The producer offers the retained value again, and now it fits.
        slot.defer(second).unwrap();
        slot.promote();
        assert_eq!(slot.take(), Some(second));
    }

    /// `KeepLatest`: an identity is a level, so a newer revision replaces an older deposit rather
    /// than queueing behind it.
    #[test]
    fn a_keep_latest_slot_holds_the_newest_identity() {
        let mut slot: Deferred<CatalogIdentityChanged> = Deferred::new(Merge::KeepLatest);
        slot.defer(identity(4)).unwrap();
        slot.defer(identity(9)).unwrap();

        slot.promote();
        assert_eq!(slot.take(), Some(identity(9)), "acting on a superseded identity is the worse failure");
        assert!(!slot.is_pending());
    }

    /// An unconsumed promoted value is never overwritten by a later deposit: capacity one holds
    /// through promotion, so the deposit simply waits its turn.
    #[test]
    fn promotion_never_displaces_an_unconsumed_value() {
        let mut slot: Deferred<CatalogIdentityChanged> = Deferred::new(Merge::KeepLatest);
        slot.defer(identity(1)).unwrap();
        slot.promote();

        // The consumer skipped its stage this pass; a newer deposit arrives.
        slot.defer(identity(2)).unwrap();
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

        wires.route_activated.defer(RouteActivated { route: 1 }).unwrap();
        assert!(wires.has_deferred());
        wires.promote_deferred();
        assert!(wires.has_deferred(), "promoted but unconsumed still counts");
        assert!(wires.route_activated.take().is_some());
        assert!(!wires.has_deferred());
    }
}
