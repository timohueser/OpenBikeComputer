//! Reader leases, and the durable retention they cause (`OBC2_Storage_Format.md` §9).
//!
//! A lease is a **RAM-only capability** containing `(connection generation, SessionId,
//! GenerationId)`. Four may coexist, reset closes every one of them, and no lease record is ever
//! replayed from media. What that leaves is a small table with three jobs:
//!
//! - **Pin at resolve.** A download resolves the current head and pins the generation it resolved
//!   to *before* the request is accepted. §9: "Acquiring a lease writes nothing" — a head is not a
//!   collection candidate while it is the head, so nothing durable has to name the pin.
//! - **Displacement is where it becomes durable.** The publication that replaces or deletes a leased
//!   head retains the displaced generation in the same terminal record, with the live-lease reason
//!   bit set and `lease count` equal to the leases live at that moment.
//! - **Release decrements exactly once.** Releasing a lease named by such an entry appends one
//!   retention record; releasing a lease on a generation no entry names appends nothing.
//!
//! ## Why a slot carries a generation of its own
//!
//! §9 says only exact capability equality may advance or release a lease, "a stale disconnect or
//! reused numeric SessionId from another connection is a no-op". The capability triple alone does
//! not close that: a connection that disconnects and reconnects with the same numeric session and
//! resolves the same head produces a *byte-identical* triple, so a late close from the first
//! connection would release — and durably decrement — the second one's lease. The connection
//! generation is what §9 offers against exactly that, and it works only if the store never reissues
//! one; nothing in this kernel can enforce that about a number a session layer hands it.
//!
//! So a handle also carries the physical slot and a **slot generation** that increments every time
//! the slot is taken. A handle from a previous tenancy of the slot fails that comparison whatever
//! its triple says, which makes the no-op structural rather than a property of the caller's
//! numbering. It is a RAM detail and appears in no record.

use obc_link::ids::{GenerationId, LogicalObjectId, Revision, SessionId};

use super::entries::RetainedPrevious;
use super::journal::Change;
use super::limits::{MAX_LEASES, MAX_RETAINED_PREVIOUS};

/// The capability a reader holds. Opaque: its fields are private, so only this module can compare
/// one against the table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LeaseHandle {
    slot: u8,
    slot_generation: u32,
    connection: u32,
    session: SessionId,
    generation: GenerationId,
}

impl LeaseHandle {
    /// The generation these bytes are pinned to. The one field a caller legitimately reads: it is
    /// what a reader streams from and what a displacement counts.
    pub fn generation(&self) -> GenerationId {
        self.generation
    }
}

/// Why a lease could not be acquired.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeaseError {
    /// All four slots are held (§2). §11 proves this before a download is admitted; the table
    /// refuses rather than growing, and a failed open consumes no slot.
    Exhausted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Held {
    connection: u32,
    session: SessionId,
    generation: GenerationId,
}

/// The bounded lease table (§2: four live download leases).
#[derive(Debug, Clone)]
pub struct LeaseTable {
    slots: [Option<Held>; MAX_LEASES],
    slot_generations: [u32; MAX_LEASES],
}

impl Default for LeaseTable {
    fn default() -> Self {
        LeaseTable::new()
    }
}

impl LeaseTable {
    /// An empty table. `const` so it can initialize a resident structure rather than be returned by
    /// value.
    pub const fn new() -> Self {
        LeaseTable { slots: [None; MAX_LEASES], slot_generations: [0; MAX_LEASES] }
    }

    /// Pins the generation a resolve landed on (§9).
    ///
    /// This writes nothing and is the only way a lease is created. Taking the resolved generation
    /// rather than a logical key is deliberate: §9's continuity guarantee is that "catalog
    /// replacement never changes an existing lease", which is only true if the pin is fixed at the
    /// generation resolve returned and never re-resolved afterwards.
    pub fn pin(
        &mut self,
        connection: u32,
        session: SessionId,
        generation: GenerationId,
    ) -> Result<LeaseHandle, LeaseError> {
        let slot = self.slots.iter().position(Option::is_none).ok_or(LeaseError::Exhausted)?;
        self.slots[slot] = Some(Held { connection, session, generation });
        // Wrapping is fine and is not a hole: the counter only has to distinguish this tenancy from
        // the handles of the previous one, and a handle does not survive 2^32 reuses of one slot in
        // a table that is cleared at every reboot.
        self.slot_generations[slot] = self.slot_generations[slot].wrapping_add(1);
        Ok(LeaseHandle {
            slot: slot as u8,
            slot_generation: self.slot_generations[slot],
            connection,
            session,
            generation,
        })
    }

    /// How many leases are live.
    pub fn live(&self) -> usize {
        self.slots.iter().flatten().count()
    }

    /// How many live leases pin `generation`.
    ///
    /// §9: the displacing publication records this as the entry's `lease count`, and "it never
    /// exceeds the four-lease capacity of section 2, which admission proves before publication".
    pub fn count_for(&self, generation: GenerationId) -> u16 {
        self.slots.iter().flatten().filter(|held| held.generation == generation).count() as u16
    }

    /// Whether any live lease pins `generation`. Garbage collection asks this before it treats a
    /// generation as unreachable.
    pub fn holds(&self, generation: GenerationId) -> bool {
        self.count_for(generation) > 0
    }

    /// Releases a lease, and says what the catalog must durably do about it.
    ///
    /// `retained` is the retained-previous table as it stands. The three outcomes are §9's, and the
    /// first is the one that makes a stale disconnect harmless.
    pub fn release(&mut self, handle: LeaseHandle, retained: &[RetainedPrevious]) -> ReleaseEffect {
        let slot = handle.slot as usize;
        if slot >= MAX_LEASES || self.slot_generations[slot] != handle.slot_generation {
            return ReleaseEffect::Stale;
        }
        let matches = matches!(
            self.slots[slot],
            Some(held)
                if held.connection == handle.connection
                    && held.session == handle.session
                    && held.generation == handle.generation
        );
        if !matches {
            return ReleaseEffect::Stale;
        }
        self.slots[slot] = None;

        // "releasing a lease on a generation no entry names appends nothing" — which is the ordinary
        // case, because a head that was never displaced was never retained.
        let Some(entry) = retained.iter().find(|entry| entry.generation == handle.generation) else {
            return ReleaseEffect::NoRecord;
        };
        if entry.reasons & RetainedPrevious::REASON_LIVE_LEASE == 0 {
            return ReleaseEffect::NoRecord;
        }
        ReleaseEffect::Retention(decrement(entry))
    }

    /// Drops every lease of one connection, as a disconnect does.
    ///
    /// Each released lease still owes its retention record, so the caller gets them back rather
    /// than the table quietly forgetting: §9 makes the decrement durable, and a connection that
    /// died is not an exemption from it.
    ///
    /// **Each release sees the table as the previous one left it.** Two leases of the same
    /// connection on the same generation — the ordinary shape, since one BLE connection may hold all
    /// four — must fold to a decrement and then a removal, not to two identical decrements. Folding
    /// them against one unchanged snapshot would emit `Put(count - 1)` twice and leave a
    /// `REASON_LIVE_LEASE` entry carrying a count no live lease justifies, which blocks collection
    /// of those bytes until the next reboot clears it.
    pub fn close_connection(
        &mut self,
        connection: u32,
        retained: &[RetainedPrevious],
        out: &mut heapless::Vec<Change<RetainedPrevious, GenerationId>, MAX_LEASES>,
    ) -> usize {
        debug_assert!(retained.len() <= MAX_RETAINED_PREVIOUS, "§2 bounds the retention table at eight entries");
        let mut table: heapless::Vec<RetainedPrevious, MAX_RETAINED_PREVIOUS> = heapless::Vec::new();
        for entry in retained.iter().take(MAX_RETAINED_PREVIOUS) {
            let _ = table.push(*entry);
        }

        let mut closed = 0;
        for slot in 0..MAX_LEASES {
            let Some(held) = self.slots[slot] else { continue };
            if held.connection != connection {
                continue;
            }
            let handle = LeaseHandle {
                slot: slot as u8,
                slot_generation: self.slot_generations[slot],
                connection: held.connection,
                session: held.session,
                generation: held.generation,
            };
            if let ReleaseEffect::Retention(change) = self.release(handle, &table) {
                apply(&mut table, &change);
                let _ = out.push(change);
            }
            closed += 1;
        }
        closed
    }

    /// Clears the whole table, as a reset or a reboot does. §9: "Reset closes every connection and
    /// lease, so no lease record is replayed from media."
    pub fn clear(&mut self) {
        for slot in 0..MAX_LEASES {
            self.slots[slot] = None;
            self.slot_generations[slot] = self.slot_generations[slot].wrapping_add(1);
        }
    }
}

/// What releasing a lease obliges the catalog to write (§9).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReleaseEffect {
    /// The handle named no live lease of this tenancy: "a stale disconnect or reused numeric
    /// SessionId from another connection is a no-op". Nothing is released and nothing is written.
    Stale,
    /// The lease was released and no retained entry names its generation, so no record is appended.
    NoRecord,
    /// The lease was released and a retained entry names its generation: one retention journal
    /// record carries this change.
    Retention(Change<RetainedPrevious, GenerationId>),
}

/// The head a publication is about to displace, in the fields a retained-previous entry records.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Displaced {
    /// The displaced head's object kind.
    pub kind: u16,
    /// Its logical object ID.
    pub logical_id: LogicalObjectId,
    /// The generation being displaced.
    pub generation: GenerationId,
    /// Its payload length.
    pub length: u64,
    /// Its payload CRC-32.
    pub crc: u32,
    /// The repository Revision whose head this generation was.
    pub object_revision: Revision,
}

/// The retained-previous entry a displacing publication must carry, or `None` when no reason
/// applies (§9).
///
/// `other_reasons` are the reasons the caller already knows apply — update rollback, or the
/// displacing repository's own domain retention. §9: "A displaced generation with no reason is not
/// retained at all: it becomes collectable at that gate, which is the ordinary case for every
/// replace and delete this store admits."
pub fn retention_for(displaced: &Displaced, leases: &LeaseTable, other_reasons: u8) -> Option<RetainedPrevious> {
    let lease_count = leases.count_for(displaced.generation);
    let mut reasons = other_reasons & !RetainedPrevious::REASON_LIVE_LEASE;
    if lease_count > 0 {
        reasons |= RetainedPrevious::REASON_LIVE_LEASE;
    }
    if reasons == 0 {
        return None;
    }
    Some(RetainedPrevious {
        reasons,
        lease_count,
        kind: displaced.kind,
        logical_id: displaced.logical_id,
        generation: displaced.generation,
        length: displaced.length,
        crc: displaced.crc,
        retain_through: 0,
        object_revision: displaced.object_revision,
    })
}

/// The retention change recovery owes one retained entry (§9).
///
/// "The lease *reason bit* in a retained-previous entry is durable even though the lease is not, so
/// reboot must clear it durably, never only in memory." Returns `None` for an entry that carries no
/// lease reason, so the caller appends nothing for it.
pub fn recovery_clear(entry: &RetainedPrevious) -> Option<Change<RetainedPrevious, GenerationId>> {
    if entry.reasons & RetainedPrevious::REASON_LIVE_LEASE == 0 {
        return None;
    }
    let mut cleared = *entry;
    cleared.lease_count = 0;
    Some(clear_lease_reason(&cleared))
}

/// Every retention record recovery must append before garbage collection may run, in table order.
///
/// §9 bounds this at the retention capacity — eight records — and §6.3 counts those eight into the
/// bounded recovery suffix.
///
/// The output vector is sized at that capacity, so a full table always fits. A push that failed
/// would silently drop a record recovery still owes — the exact fault §9 calls out, since a lease
/// bit cleared nowhere lets GC treat a generation the durable catalog still names as unreachable —
/// so it is asserted rather than ignored. `out` is expected empty; appending to a non-empty vector
/// is what would make a full table overflow it.
pub fn recovery_suffix(
    retained: &[RetainedPrevious],
    out: &mut heapless::Vec<Change<RetainedPrevious, GenerationId>, { super::limits::MAX_RETAINED_PREVIOUS }>,
) {
    debug_assert!(retained.len() <= MAX_RETAINED_PREVIOUS, "§2 bounds the retention table at eight entries");
    debug_assert!(out.is_empty(), "the recovery suffix is collected into a fresh vector");
    for entry in retained {
        if let Some(change) = recovery_clear(entry) {
            let pushed = out.push(change).is_ok();
            debug_assert!(pushed, "a retention record recovery owes was dropped for want of capacity");
        }
    }
}

/// Applies one emitted retention change to a working copy of the table, so the next release of the
/// same batch decides against the state its predecessor produced rather than against the snapshot
/// they all started from.
fn apply(
    table: &mut heapless::Vec<RetainedPrevious, MAX_RETAINED_PREVIOUS>,
    change: &Change<RetainedPrevious, GenerationId>,
) {
    match change {
        Change::Put(row) => {
            if let Some(held) = table.iter_mut().find(|held| held.generation == row.generation) {
                *held = *row;
            }
        }
        Change::Remove(generation) => {
            if let Some(index) = table.iter().position(|held| held.generation == *generation) {
                // The table has no ordering rule among entries of one kind (§5.3), and this copy is
                // a lookup structure rather than a body being encoded, so a swap is enough.
                table.swap_remove(index);
            }
        }
    }
}

/// One release's effect on an entry: decrement the count, clear the bit at zero, remove the entry
/// when no reason remains.
fn decrement(entry: &RetainedPrevious) -> Change<RetainedPrevious, GenerationId> {
    let mut next = *entry;
    next.lease_count = next.lease_count.saturating_sub(1);
    if next.lease_count > 0 {
        return Change::Put(next);
    }
    clear_lease_reason(&next)
}

fn clear_lease_reason(entry: &RetainedPrevious) -> Change<RetainedPrevious, GenerationId> {
    let mut next = *entry;
    next.lease_count = 0;
    next.reasons &= !RetainedPrevious::REASON_LIVE_LEASE;
    if next.reasons == 0 {
        // "An entry whose reasons have all been cleared is removed and its generation becomes
        // collectable."
        Change::Remove(next.generation)
    } else {
        Change::Put(next)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::vec::Vec;

    fn session(value: u32) -> SessionId {
        SessionId::new(value).expect("a nonzero session")
    }

    fn generation(value: u64) -> GenerationId {
        GenerationId::new(value)
    }

    fn displaced(value: u64) -> Displaced {
        Displaced {
            kind: 1,
            logical_id: LogicalObjectId::new(7),
            generation: generation(value),
            length: 3_000,
            crc: 0x1234_5678,
            object_revision: Revision::new(8),
        }
    }

    #[test]
    fn four_leases_coexist_and_the_fifth_is_refused() {
        let mut table = LeaseTable::new();
        let handles: Vec<LeaseHandle> =
            (0..MAX_LEASES).map(|index| table.pin(1, session(index as u32 + 1), generation(9)).unwrap()).collect();
        assert_eq!(table.live(), MAX_LEASES);
        assert_eq!(table.count_for(generation(9)), MAX_LEASES as u16);
        assert_eq!(table.pin(1, session(99), generation(9)), Err(LeaseError::Exhausted));

        // A refused pin consumed nothing, so releasing one makes room again.
        assert_eq!(table.release(handles[0], &[]), ReleaseEffect::NoRecord);
        assert_eq!(table.live(), MAX_LEASES - 1);
        assert!(table.pin(1, session(99), generation(9)).is_ok());
    }

    /// §9: "Acquiring a lease writes nothing." The only durable act is the displacement, and it
    /// carries the count the table held at that moment.
    #[test]
    fn displacement_records_the_live_lease_reason_and_its_count() {
        let mut table = LeaseTable::new();
        // Nothing pins generation 42, so displacing it retains nothing at all.
        assert_eq!(retention_for(&displaced(42), &table, 0), None);

        let _first = table.pin(1, session(1), generation(42)).unwrap();
        let _second = table.pin(2, session(1), generation(42)).unwrap();
        let entry = retention_for(&displaced(42), &table, 0).expect("a leased head is retained");
        assert_eq!(entry.reasons, RetainedPrevious::REASON_LIVE_LEASE);
        assert_eq!(entry.lease_count, 2);
        assert_eq!(entry.generation, generation(42));
        assert_eq!(entry.retain_through, 0, "the lease reason controls the entry, not a counter");
    }

    /// A displaced generation with an update-rollback reason is retained whether or not it is
    /// leased, and the two reasons coexist in one entry.
    #[test]
    fn other_reasons_retain_a_generation_no_lease_pins() {
        let mut table = LeaseTable::new();
        let entry = retention_for(&displaced(42), &table, RetainedPrevious::REASON_UPDATE_ROLLBACK).unwrap();
        assert_eq!(entry.reasons, RetainedPrevious::REASON_UPDATE_ROLLBACK);
        assert_eq!(entry.lease_count, 0);

        let _lease = table.pin(1, session(1), generation(42)).unwrap();
        let entry = retention_for(&displaced(42), &table, RetainedPrevious::REASON_UPDATE_ROLLBACK).unwrap();
        assert_eq!(
            entry.reasons,
            RetainedPrevious::REASON_LIVE_LEASE | RetainedPrevious::REASON_UPDATE_ROLLBACK,
            "both reasons hold the same entry",
        );
        assert_eq!(entry.lease_count, 1);
    }

    /// §9's release rule, all three steps: decrement, clear the bit at zero, remove when no reason
    /// remains.
    #[test]
    fn release_decrements_then_clears_then_removes() {
        let mut table = LeaseTable::new();
        let first = table.pin(1, session(1), generation(42)).unwrap();
        let second = table.pin(2, session(1), generation(42)).unwrap();
        let entry = retention_for(&displaced(42), &table, 0).unwrap();

        // Two leases, so the first release only decrements.
        let ReleaseEffect::Retention(Change::Put(after_first)) = table.release(first, &[entry]) else {
            panic!("the first release must decrement rather than remove");
        };
        assert_eq!(after_first.lease_count, 1);
        assert_eq!(after_first.reasons, RetainedPrevious::REASON_LIVE_LEASE);

        // The last release clears the bit, and with no reason left the entry goes.
        assert_eq!(table.release(second, &[after_first]), ReleaseEffect::Retention(Change::Remove(generation(42))));
    }

    /// With another reason still holding the entry, the last release clears only the lease bit.
    #[test]
    fn the_last_release_leaves_an_entry_another_reason_still_holds() {
        let mut table = LeaseTable::new();
        let handle = table.pin(1, session(1), generation(42)).unwrap();
        let entry = retention_for(&displaced(42), &table, RetainedPrevious::REASON_DOMAIN_RETENTION).unwrap();

        let ReleaseEffect::Retention(Change::Put(after)) = table.release(handle, &[entry]) else {
            panic!("the entry is still held by its domain-retention reason");
        };
        assert_eq!(after.reasons, RetainedPrevious::REASON_DOMAIN_RETENTION);
        assert_eq!(after.lease_count, 0);
    }

    /// The finding this module's slot generation exists for: a reconnect that reproduces the whole
    /// capability triple must not let the first connection's late close decrement the second's
    /// lease.
    #[test]
    fn a_stale_close_cannot_decrement_a_reused_slot() {
        let mut table = LeaseTable::new();
        let stale = table.pin(7, session(3), generation(42)).unwrap();
        // The reader disconnects. Nothing is retained yet, so nothing is written.
        assert_eq!(table.release(stale, &[]), ReleaseEffect::NoRecord);

        // The same numeric connection and session resolve the same head: a byte-identical triple.
        let fresh = table.pin(7, session(3), generation(42)).unwrap();
        assert_ne!(stale, fresh, "the handles differ only in the slot generation");
        let entry = retention_for(&displaced(42), &table, 0).unwrap();
        assert_eq!(entry.lease_count, 1);

        // The first connection's close arrives late. It must do nothing at all.
        assert_eq!(table.release(stale, &[entry]), ReleaseEffect::Stale);
        assert_eq!(table.live(), 1, "a stale close released the second connection's lease");
        assert!(table.holds(generation(42)));

        // And the real close still works.
        assert_eq!(table.release(fresh, &[entry]), ReleaseEffect::Retention(Change::Remove(generation(42))));
        assert_eq!(table.live(), 0);
    }

    /// Double release is the same no-op, for the same reason.
    #[test]
    fn releasing_twice_decrements_once() {
        let mut table = LeaseTable::new();
        let handle = table.pin(1, session(1), generation(42)).unwrap();
        let entry = retention_for(&displaced(42), &table, 0).unwrap();
        assert_eq!(table.release(handle, &[entry]), ReleaseEffect::Retention(Change::Remove(generation(42))));
        assert_eq!(table.release(handle, &[entry]), ReleaseEffect::Stale);
    }

    /// §9: "Catalog replacement never changes an existing lease." The pin is fixed at the generation
    /// resolve returned, so a later head is simply a different generation.
    #[test]
    fn a_replacement_never_moves_an_existing_lease() {
        let mut table = LeaseTable::new();
        let handle = table.pin(1, session(1), generation(42)).unwrap();
        assert_eq!(handle.generation(), generation(42));
        // The head is replaced by generation 43. The lease still pins 42 and still counts for it.
        assert_eq!(table.count_for(generation(42)), 1);
        assert_eq!(table.count_for(generation(43)), 0);
        assert!(!table.holds(generation(43)));
    }

    /// A disconnect drops every lease of the connection and still owes their retention records.
    #[test]
    fn closing_a_connection_releases_its_leases_and_reports_their_records() {
        let mut table = LeaseTable::new();
        let _a = table.pin(1, session(1), generation(42)).unwrap();
        let _b = table.pin(1, session(2), generation(43)).unwrap();
        let _c = table.pin(2, session(1), generation(42)).unwrap();
        let retained =
            [retention_for(&displaced(42), &table, 0).unwrap(), retention_for(&displaced(43), &table, 0).unwrap()];
        assert_eq!(retained[0].lease_count, 2);

        let mut records = heapless::Vec::new();
        assert_eq!(table.close_connection(1, &retained, &mut records), 2);
        assert_eq!(table.live(), 1, "connection 2's lease survives");
        assert_eq!(records.len(), 2);
        assert_eq!(records[0], Change::Put(RetainedPrevious { lease_count: 1, ..retained[0] }));
        assert_eq!(records[1], Change::Remove(generation(43)));
    }

    /// M1's regression, and the ordinary device configuration rather than an exotic one: **one**
    /// connection holding several leases on the same generation.
    ///
    /// Each release must decide against the table its predecessor produced. Folding all of them
    /// against the snapshot they started from emits the same decrement twice and leaves an entry
    /// carrying a live-lease count no live lease justifies — which keeps those bytes uncollectable
    /// until a reboot clears the bit.
    #[test]
    fn several_leases_of_one_connection_on_one_generation_fold_to_a_removal() {
        let mut table = LeaseTable::new();
        for index in 0..MAX_LEASES {
            table.pin(1, session(index as u32 + 1), generation(42)).unwrap();
        }
        let retained = [retention_for(&displaced(42), &table, 0).unwrap()];
        assert_eq!(retained[0].lease_count, MAX_LEASES as u16);

        let mut records = heapless::Vec::new();
        assert_eq!(table.close_connection(1, &retained, &mut records), MAX_LEASES);
        assert_eq!(table.live(), 0);
        assert_eq!(records.len(), MAX_LEASES);
        // Three decrements walking the count down, then the removal that frees the generation.
        for (index, record) in records.iter().enumerate().take(MAX_LEASES - 1) {
            assert_eq!(
                *record,
                Change::Put(RetainedPrevious { lease_count: (MAX_LEASES - 1 - index) as u16, ..retained[0] }),
                "release {index} did not decide against its predecessor's table",
            );
        }
        assert_eq!(
            records[MAX_LEASES - 1],
            Change::Remove(generation(42)),
            "the last release of the last lease must free the entry",
        );
    }

    /// The same fold when another reason holds the entry: the count still walks to zero and the
    /// last record clears only the lease bit.
    #[test]
    fn several_leases_of_one_connection_clear_the_bit_and_leave_another_reason() {
        let mut table = LeaseTable::new();
        table.pin(5, session(1), generation(42)).unwrap();
        table.pin(5, session(2), generation(42)).unwrap();
        let retained = [retention_for(&displaced(42), &table, RetainedPrevious::REASON_UPDATE_ROLLBACK).unwrap()];

        let mut records = heapless::Vec::new();
        table.close_connection(5, &retained, &mut records);
        assert_eq!(records.len(), 2);
        assert_eq!(records[0], Change::Put(RetainedPrevious { lease_count: 1, ..retained[0] }),);
        assert_eq!(
            records[1],
            Change::Put(RetainedPrevious {
                lease_count: 0,
                reasons: RetainedPrevious::REASON_UPDATE_ROLLBACK,
                ..retained[0]
            }),
            "the rollback reason still holds the entry after the last lease goes",
        );
    }

    /// Mixed generations under one connection stay independent: each generation's own count is what
    /// walks down, and one reaching zero does not disturb another.
    #[test]
    fn closing_a_connection_folds_each_generation_independently() {
        let mut table = LeaseTable::new();
        table.pin(1, session(1), generation(42)).unwrap();
        table.pin(1, session(2), generation(42)).unwrap();
        table.pin(1, session(3), generation(43)).unwrap();
        let retained =
            [retention_for(&displaced(42), &table, 0).unwrap(), retention_for(&displaced(43), &table, 0).unwrap()];

        let mut records = heapless::Vec::new();
        assert_eq!(table.close_connection(1, &retained, &mut records), 3);
        assert_eq!(records.len(), 3);
        assert_eq!(records[0], Change::Put(RetainedPrevious { lease_count: 1, ..retained[0] }));
        assert_eq!(records[1], Change::Remove(generation(42)));
        assert_eq!(records[2], Change::Remove(generation(43)));
    }

    /// §9's reboot rule: every entry carrying the live-lease bit gets one retention record, and
    /// nothing else does. At most eight of them, which is the retention capacity.
    #[test]
    fn recovery_clears_every_lease_reason_durably_and_nothing_else() {
        let mut table = LeaseTable::new();
        let _lease = table.pin(1, session(1), generation(42)).unwrap();
        let leased = retention_for(&displaced(42), &table, 0).unwrap();
        let mut both = retention_for(&displaced(43), &table, RetainedPrevious::REASON_UPDATE_ROLLBACK).unwrap();
        both.reasons |= RetainedPrevious::REASON_LIVE_LEASE;
        both.lease_count = 3;
        let rollback_only = retention_for(&displaced(44), &table, RetainedPrevious::REASON_UPDATE_ROLLBACK).unwrap();

        let mut records = heapless::Vec::new();
        recovery_suffix(&[leased, both, rollback_only], &mut records);
        assert_eq!(records.len(), 2, "only the entries carrying the lease bit produce a record");
        assert_eq!(records[0], Change::Remove(generation(42)), "no reason remains, so the entry goes");
        assert_eq!(
            records[1],
            Change::Put(RetainedPrevious { reasons: RetainedPrevious::REASON_UPDATE_ROLLBACK, lease_count: 0, ..both }),
            "the rollback reason still holds the entry",
        );
        assert!(recovery_clear(&rollback_only).is_none());

        // The bound §6.3's suffix budget counts on.
        let saturated: Vec<RetainedPrevious> = (0..super::super::limits::MAX_RETAINED_PREVIOUS as u64)
            .map(|index| retention_for(&displaced(100 + index), &table, 0).unwrap_or(leased))
            .collect();
        let mut records = heapless::Vec::new();
        recovery_suffix(&saturated, &mut records);
        assert!(records.len() <= super::super::limits::MAX_RETAINED_PREVIOUS);
    }

    /// Reset closes every lease, and the handles it invalidated stay invalid.
    #[test]
    fn clearing_the_table_invalidates_every_outstanding_handle() {
        let mut table = LeaseTable::new();
        let handle = table.pin(1, session(1), generation(42)).unwrap();
        table.clear();
        assert_eq!(table.live(), 0);
        assert_eq!(table.release(handle, &[]), ReleaseEffect::Stale);
    }
}
