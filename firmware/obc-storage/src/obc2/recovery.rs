//! The recovery decision of `OBC2_Storage_Format.md` §6.3, as pure logic.
//!
//! Nothing here reads media. The caller hands in what it observed — each checkpoint's validity and
//! header facts, and each journal slot's validity and header facts — and gets back which checkpoint
//! to mount, how much of the journal to replay, or which fail-closed condition it hit. That split
//! is deliberate: the decision is the part with the subtle rules, and it is worth being able to
//! enumerate every one of them in a test without a filesystem.
//!
//! The two fail-closed rules are the reason recovery scans all 256 slots instead of stopping at the
//! first gap:
//!
//! - **A newer epoch.** Compaction advances the epoch monotonically, so a valid same-store record
//!   above the selected checkpoint's epoch proves a newer checkpoint existed and was lost. Mounting
//!   the older one would silently roll back everything it had absorbed.
//! - **A valid record beyond the replay stop.** Slots are written in sequence order and each
//!   occupies its own program page, so a valid later record proves an already-committed record was
//!   lost rather than never written.
//!
//! Ordinary end-of-journal — no valid record at or beyond the stop — is not a fault.

use obc_link::ids::StoreId;

use super::limits::JOURNAL_SLOTS;

/// What a mount observed about one checkpoint file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CheckpointObservation {
    /// The store the body names.
    pub store: StoreId,
    /// Its epoch.
    pub epoch: u64,
    /// Its through-sequence.
    pub through_sequence: u64,
    /// Its body CRC, which is what distinguishes "the same checkpoint" from "a different one".
    pub body_crc: u32,
}

/// What a mount observed about one journal slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SlotObservation {
    /// The store the body names.
    pub store: StoreId,
    /// Its epoch.
    pub epoch: u64,
    /// Its sequence.
    pub sequence: u64,
}

/// Why recovery refused to mount.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailClosed {
    /// Two valid checkpoints share a through-sequence and differ (§6.3).
    AmbiguousCheckpoint,
    /// A valid same-store record carries an epoch above the selected checkpoint's: a newer
    /// checkpoint existed and was lost.
    NewerEpochRecord {
        /// The physical slot that proved it.
        slot: u16,
    },
    /// A valid same-store, same-epoch record lies beyond the replay stop: a committed record was
    /// lost rather than never written.
    RecordBeyondStop {
        /// The physical slot that proved it.
        slot: u16,
    },
}

/// What recovery decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// No structurally valid checkpoint. §12 decides between initializing, resuming an `INIT`
    /// witness, removing a pre-birth prefix, and mounting recovery-failed; that classification is
    /// a later slice's, and this decision only reports the absence.
    NoCheckpoint,
    /// Mount this checkpoint and replay this many leading slots.
    Mount {
        /// Which checkpoint file, `0` or `1`.
        checkpoint: usize,
        /// How many leading journal slots form the contiguous valid suffix.
        replay: usize,
    },
    /// Mount recovery-failed and read-only, preserving all evidence.
    Fail(FailClosed),
}

/// Chooses per §6.3.
///
/// `checkpoints[i]` is `Some` only when checkpoint file `i` validated completely — body, gate, and
/// their agreement. `slots[i]` is `Some` only when journal slot `i` validated completely. An
/// invalid record contributes nothing at all: §6.3's fail-closed rules are about *valid* records,
/// because a torn one proves only that a write was interrupted.
pub fn choose(checkpoints: &[Option<CheckpointObservation>; 2], slots: &[Option<SlotObservation>]) -> Decision {
    debug_assert!(slots.len() <= JOURNAL_SLOTS);

    // "Recovery chooses the structurally valid checkpoint with the greatest `through_sequence`;
    // differing valid checkpoints at the same sequence are corruption."
    let selected = match (checkpoints[0], checkpoints[1]) {
        (None, None) => return Decision::NoCheckpoint,
        (Some(a), None) => (0usize, a),
        (None, Some(b)) => (1usize, b),
        (Some(a), Some(b)) => match a.through_sequence.cmp(&b.through_sequence) {
            core::cmp::Ordering::Greater => (0, a),
            core::cmp::Ordering::Less => (1, b),
            core::cmp::Ordering::Equal => {
                if a.body_crc != b.body_crc || a.epoch != b.epoch || a.store != b.store {
                    return Decision::Fail(FailClosed::AmbiguousCheckpoint);
                }
                (0, a)
            }
        },
    };
    let (index, checkpoint) = selected;

    // "It replays only journal records whose StoreId and epoch match and whose sequences begin
    // exactly at `through_sequence + 1` ... physical journal slot `i` must carry sequence
    // `checkpoint through_sequence + i + 1`; another mapping is invalid even when its CRCs pass."
    let mut replay = 0usize;
    while replay < slots.len() {
        match slots[replay] {
            Some(slot)
                if slot.store == checkpoint.store
                    && slot.epoch == checkpoint.epoch
                    && slot.sequence == checkpoint.through_sequence + replay as u64 + 1 =>
            {
                replay += 1;
            }
            _ => break,
        }
    }

    // "Recovery then scans all 256 slots before mounting, because stopping is not by itself
    // evidence that nothing later was committed."
    for (position, slot) in slots.iter().enumerate() {
        let Some(slot) = slot else { continue };
        if slot.store != checkpoint.store {
            continue;
        }
        if slot.epoch > checkpoint.epoch {
            return Decision::Fail(FailClosed::NewerEpochRecord { slot: position as u16 });
        }
        if slot.epoch == checkpoint.epoch && position >= replay {
            return Decision::Fail(FailClosed::RecordBeyondStop { slot: position as u16 });
        }
    }

    Decision::Mount { checkpoint: index, replay }
}

/// Whether a new mutation must wait for compaction (§6.3): "Before accepting a 193rd record in one
/// epoch, `CardStore` blocks new mutations and compacts."
pub fn compaction_required(valid_records: usize) -> bool {
    valid_records >= super::limits::JOURNAL_COMPACTION_TRIGGER
}

/// Whether recovery must compact before appending its bounded suffix (§6.3): "If fewer than 64
/// slots remain free in the selected epoch, recovery runs one compaction cycle ... before appending
/// its suffix and before accepting any new mutation."
///
/// The headroom is the same 64 slots the trigger leaves above it, which is what makes the 55-record
/// worst-case suffix of §6.3 fit.
pub fn recovery_must_compact(valid_records: usize) -> bool {
    JOURNAL_SLOTS - valid_records < JOURNAL_SLOTS - super::limits::JOURNAL_COMPACTION_TRIGGER
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::vec;
    use std::vec::Vec;

    const STORE: StoreId = StoreId::new([0x3c; 16]);
    const OTHER: StoreId = StoreId::new([0x11; 16]);

    fn checkpoint(epoch: u64, through: u64) -> CheckpointObservation {
        CheckpointObservation { store: STORE, epoch, through_sequence: through, body_crc: 0xABCD_1234 }
    }

    fn slot(epoch: u64, sequence: u64) -> Option<SlotObservation> {
        Some(SlotObservation { store: STORE, epoch, sequence })
    }

    #[test]
    fn the_greatest_through_sequence_wins() {
        let checkpoints = [Some(checkpoint(1, 10)), Some(checkpoint(2, 20))];
        assert_eq!(choose(&checkpoints, &[None; 4]), Decision::Mount { checkpoint: 1, replay: 0 });
        let checkpoints = [Some(checkpoint(3, 30)), Some(checkpoint(2, 20))];
        assert_eq!(choose(&checkpoints, &[None; 4]), Decision::Mount { checkpoint: 0, replay: 0 });
    }

    #[test]
    fn no_valid_checkpoint_is_reported_rather_than_guessed() {
        assert_eq!(choose(&[None, None], &[slot(1, 1)]), Decision::NoCheckpoint);
    }

    #[test]
    fn differing_valid_checkpoints_at_one_sequence_are_corruption() {
        let mut second = checkpoint(1, 10);
        second.body_crc ^= 1;
        assert_eq!(
            choose(&[Some(checkpoint(1, 10)), Some(second)], &[None; 4]),
            Decision::Fail(FailClosed::AmbiguousCheckpoint)
        );
    }

    #[test]
    fn replay_is_the_contiguous_prefix_and_ends_at_the_first_gap() {
        let checkpoints = [Some(checkpoint(1, 0)), None];
        let slots = vec![slot(1, 1), slot(1, 2), slot(1, 3), None, None];
        assert_eq!(choose(&checkpoints, &slots), Decision::Mount { checkpoint: 0, replay: 3 });
    }

    #[test]
    fn a_wrong_slot_to_sequence_mapping_stops_replay_even_with_valid_crcs() {
        let checkpoints = [Some(checkpoint(1, 0)), None];
        // Slot 1 carries sequence 3, which the mapping rule says belongs to slot 2.
        let slots = vec![slot(1, 1), slot(1, 3), None, None];
        assert_eq!(choose(&checkpoints, &slots), Decision::Fail(FailClosed::RecordBeyondStop { slot: 1 }));
    }

    #[test]
    fn a_newer_epoch_record_anywhere_fails_closed() {
        let checkpoints = [Some(checkpoint(1, 0)), None];
        let mut slots: Vec<Option<SlotObservation>> = vec![None; 256];
        slots[0] = slot(1, 1);
        slots[200] = slot(2, 900);
        assert_eq!(choose(&checkpoints, &slots), Decision::Fail(FailClosed::NewerEpochRecord { slot: 200 }));
    }

    #[test]
    fn a_same_epoch_record_beyond_the_stop_fails_closed() {
        let checkpoints = [Some(checkpoint(1, 0)), None];
        let mut slots: Vec<Option<SlotObservation>> = vec![None; 8];
        slots[0] = slot(1, 1);
        slots[1] = slot(1, 2);
        // Slot 2 was lost; slot 3 survives and proves it was committed.
        slots[3] = slot(1, 4);
        assert_eq!(choose(&checkpoints, &slots), Decision::Fail(FailClosed::RecordBeyondStop { slot: 3 }));
    }

    #[test]
    fn an_ordinary_end_of_journal_mounts_normally() {
        let checkpoints = [Some(checkpoint(1, 0)), None];
        let mut slots: Vec<Option<SlotObservation>> = vec![None; 256];
        for (index, entry) in slots.iter_mut().enumerate().take(5) {
            *entry = slot(1, index as u64 + 1);
        }
        assert_eq!(choose(&checkpoints, &slots), Decision::Mount { checkpoint: 0, replay: 5 });
    }

    /// A stale record of an *older* epoch is inert against the selected checkpoint: journal slots
    /// are reusable exactly because their epoch no longer matches (§6.3).
    #[test]
    fn old_epoch_slots_are_inert_rather_than_a_fault() {
        let checkpoints = [None, Some(checkpoint(2, 100))];
        let mut slots: Vec<Option<SlotObservation>> = vec![None; 256];
        slots[0] = slot(2, 101);
        for (index, entry) in slots.iter_mut().enumerate().take(20).skip(1) {
            *entry = slot(1, index as u64 + 1);
        }
        assert_eq!(choose(&checkpoints, &slots), Decision::Mount { checkpoint: 1, replay: 1 });
    }

    /// A record from another store is not this store's evidence at all.
    #[test]
    fn a_foreign_store_record_is_ignored() {
        let checkpoints = [Some(checkpoint(1, 0)), None];
        let mut slots: Vec<Option<SlotObservation>> = vec![None; 16];
        slots[0] = slot(1, 1);
        slots[5] = Some(SlotObservation { store: OTHER, epoch: 9, sequence: 900 });
        assert_eq!(choose(&checkpoints, &slots), Decision::Mount { checkpoint: 0, replay: 1 });
    }

    #[test]
    fn compaction_triggers_at_the_193rd_record() {
        assert!(!compaction_required(191));
        assert!(compaction_required(192));
    }

    /// Recovery's own trigger is the 64-slot headroom its bounded suffix needs.
    #[test]
    fn recovery_compacts_when_the_headroom_is_gone() {
        assert!(!recovery_must_compact(191));
        assert!(recovery_must_compact(193));
    }
}
