//! The resident RAM index (`OBC2_Storage_Format.md` §13), and the source the compaction pass of
//! §6.3 reads its fixed fields from.
//!
//! §13 is explicit that the checkpoint projection is **card-resident**: "RAM holds a bounded index,
//! not the projection". Two things are deliberately left on the card and re-read on demand — a
//! head's catalog-projection envelope and its resolution `GenerationId` with the flag that travels
//! with it — and one more is left there by the same sentence that sizes the result ring: the
//! terminal-result *body*, of which RAM keeps only `(OperationId, commit sequence)`.
//!
//! Everything else this type holds, and §6.3 calls it "authoritative for everything it holds".
//!
//! ## The per-head journal-slot reference
//!
//! Each head entry carries a `u16` physical journal slot. §6.3 introduces it for one purpose: when a
//! head-putting record has been replayed since the active checkpoint, the newest bytes of those two
//! card-resident fields are in *that record*, not in the checkpoint — and the reference is how
//! compaction, `QueryCatalog` and garbage collection find it without scanning 256 slots. It is
//! meaningful only within the selected epoch and is reset by compaction, which is why
//! [`RamIndex::clear_journal_slots`] exists and is called at the end of the pass.
//!
//! [`NO_JOURNAL_SLOT`] is the absent value rather than an `Option<u16>`, because §13 counts this
//! field as a `u16` in a budget that has no room for the discriminant an `Option` would add.
//!
//! ## What this is not
//!
//! It is not a second `apply`. [`CatalogModel`](super::model::CatalogModel) is the reference model
//! and the meaning of a journal record; this is the resident *shape* of the same facts, projected
//! from it by [`RamIndex::project_into`]. Keeping one `apply` is deliberate: two would be two
//! definitions of what a record means, and the compaction proof — that a streamed body equals
//! `CatalogModel::encode_body` — only means something while the index is derived from the model
//! rather than maintained beside it.

use heapless::Vec;
use obc_link::ids::{GenerationId, LogicalObjectId, OperationId, Revision, StoreId};

use super::checkpoint::CheckpointHeader;
use super::entries::{
    ActiveOperation, ActiveRide, CatalogHead, DraftParent, DraftPart, HeadKey, RepositoryState, RetainedPrevious,
    WeatherState,
};
use super::handoff::HandoffRef;
use super::leases::LeaseTable;
use super::limits::{
    MAX_ACTIVE_OPERATIONS, MAX_CATALOG_HEADS, MAX_DRAFT_PARTS, MAX_REPOSITORY_STATES, MAX_RETAINED_PREVIOUS,
    MAX_TERMINAL_RESULTS,
};

/// The absent journal-slot reference: this head's newest bytes are in the active checkpoint.
///
/// 65,535 is not a slot — `COMMIT.JNL` has 256 — so the sentinel costs no representable value.
pub const NO_JOURNAL_SLOT: u16 = u16::MAX;

/// One resident catalog head (§13).
///
/// The envelope and the resolution `GenerationId` are **not** here, and neither is the
/// resolution-present bit: §6.3 makes that bit travel with the field it describes, so a head whose
/// resolution is card-resident cannot advertise it from RAM.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HeadIndexEntry {
    /// The logical object this head names.
    pub id: LogicalObjectId,
    /// The repository Revision of this head.
    pub revision: Revision,
    /// The generation its bytes are.
    pub generation: GenerationId,
    /// Its payload length.
    pub length: u64,
    /// Its payload CRC-32.
    pub crc: u32,
    /// Its object kind.
    pub kind: u16,
    /// The physical journal slot carrying a newer head entry, or [`NO_JOURNAL_SLOT`].
    pub journal_slot: u16,
    /// The head-entry flags **other than** resolution-present.
    pub flags: u8,
}

impl HeadIndexEntry {
    /// §13's per-entry target: "targeting at most 50 bytes per entry".
    pub const RESIDENT_TARGET: usize = 50;

    /// The `(kind, logical id)` this entry is keyed by.
    pub fn key(&self) -> HeadKey {
        HeadKey { kind: self.kind, id: self.id }
    }

    /// The resident half of a decoded head entry.
    pub fn from_head(head: &CatalogHead, journal_slot: u16) -> Self {
        HeadIndexEntry {
            id: head.key.id,
            revision: head.revision,
            generation: head.generation,
            length: head.length,
            crc: head.crc,
            kind: head.key.kind,
            journal_slot,
            flags: head.flags & !CatalogHead::FLAG_RESOLUTION_PRESENT,
        }
    }
}

/// One resident result-ring entry (§13): "OperationId and commit sequence with the result body
/// re-read from card".
///
/// The journal-slot reference is the same device §6.3 gives a catalog head, for the same reason and
/// with the same lifetime. §6.3 states it only for heads, but a terminal result appended by a record
/// replayed since the active checkpoint is in no checkpoint at all, so "re-read from card" can only
/// mean re-read from that record — and finding it needs the reference. It fits §13's budget without
/// moving it: the ring is budgeted at 32 bytes an entry and the two fields §13 names are 24.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResultIndexEntry {
    /// The operation whose result this is — the key `QueryOperation` looks up.
    pub operation: OperationId,
    /// Its terminal commit sequence, which is also the ring's ordering.
    pub commit_sequence: u64,
    /// The physical journal slot carrying the record that appended it, or [`NO_JOURNAL_SLOT`] when
    /// the active checkpoint's ring already holds it.
    pub journal_slot: u16,
}

impl ResultIndexEntry {
    /// §13's per-entry budget figure: `32 × 64`.
    pub const RESIDENT_TARGET: usize = 32;
}

/// §13's budget formula at the §2 capacities, before the lease table and the bounded staging:
/// `12,800 + 2,048 + 1,152 + 128 + 3,072 + 512`.
pub const SECTION_13_FORMULA: usize = 19_712;

/// The bounded resident index.
#[derive(Debug, Clone)]
pub struct RamIndex {
    /// The store this index belongs to.
    pub store: StoreId,
    /// The selected checkpoint's epoch.
    pub epoch: u64,
    /// The last sequence absorbed, checkpoint plus replayed suffix.
    pub through_sequence: u64,
    /// The next `GenerationId` cursor.
    pub next_generation: u64,
    /// The terminal-commit counter.
    pub terminal_counter: u64,
    /// Header flags; bit 0 is the durable store-wide degraded record.
    pub flags: u8,
    /// Repository rows, sorted by kind.
    pub repositories: Vec<RepositoryState, MAX_REPOSITORY_STATES>,
    /// Head index entries, sorted by `(kind, logical id)`.
    pub heads: Vec<HeadIndexEntry, MAX_CATALOG_HEADS>,
    /// Active rows, sorted by `OperationId` wire bytes.
    pub actives: Vec<ActiveOperation, MAX_ACTIVE_OPERATIONS>,
    /// The one draft parent.
    pub draft_parent: Option<DraftParent>,
    /// Its parts, sorted by `(parent, kind, part key)`.
    pub draft_parts: Vec<DraftPart, MAX_DRAFT_PARTS>,
    /// Retained generations, sorted by `GenerationId`.
    pub retained: Vec<RetainedPrevious, MAX_RETAINED_PREVIOUS>,
    /// The result ring's start index.
    pub result_start: usize,
    /// The result ring's keys, in ring order from `result_start`.
    pub results: Vec<ResultIndexEntry, MAX_TERMINAL_RESULTS>,
    /// The one update-handoff projection.
    pub handoff: Option<HandoffRef>,
    /// The one weather-request state.
    pub weather: Option<WeatherState>,
    /// The one active-ride state.
    pub ride: Option<ActiveRide>,
    /// The four RAM-only download leases (§9).
    pub leases: LeaseTable,
}

impl RamIndex {
    /// An empty index. `const` so it can initialize a `static` rather than travel through a return
    /// slot, for the reason [`CatalogModel::empty`](super::model::CatalogModel::empty) gives.
    pub const fn new(store: StoreId) -> Self {
        RamIndex {
            store,
            epoch: 1,
            through_sequence: 0,
            next_generation: 0,
            terminal_counter: 0,
            flags: 0,
            repositories: Vec::new(),
            heads: Vec::new(),
            actives: Vec::new(),
            draft_parent: None,
            draft_parts: Vec::new(),
            retained: Vec::new(),
            result_start: 0,
            results: Vec::new(),
            handoff: None,
            weather: None,
            ride: None,
            leases: LeaseTable::new(),
        }
    }

    /// The head a `(kind, logical id)` names.
    pub fn head(&self, key: HeadKey) -> Option<&HeadIndexEntry> {
        self.heads.iter().find(|entry| entry.key() == key)
    }

    /// Records that journal slot `slot` carries this head's newest entry (§6.3).
    ///
    /// Returns false when the index holds no such head, which is a caller that recorded a slot for
    /// a head it never applied.
    pub fn note_head_record(&mut self, key: HeadKey, slot: u16) -> bool {
        match self.heads.iter_mut().find(|entry| entry.key() == key) {
            Some(entry) => {
                entry.journal_slot = slot;
                true
            }
            None => false,
        }
    }

    /// Records that journal slot `slot` carries the record that appended the newest result.
    ///
    /// Results are append-only, so this always names the last entry of the ring.
    pub fn note_result_record(&mut self, slot: u16) -> bool {
        match self.results.last_mut() {
            Some(entry) => {
                entry.journal_slot = slot;
                true
            }
            None => false,
        }
    }

    /// Forgets every journal-slot reference, of a head and of a result alike.
    ///
    /// §6.3: the reference is "meaningful only within the selected epoch and reset by compaction".
    /// After the pass the new checkpoint holds every card-resident field, so a stale slot index
    /// would point a later read at a record of an epoch that no longer replays.
    pub fn clear_journal_slots(&mut self) {
        for entry in self.heads.iter_mut() {
            entry.journal_slot = NO_JOURNAL_SLOT;
        }
        for entry in self.results.iter_mut() {
            entry.journal_slot = NO_JOURNAL_SLOT;
        }
    }

    /// The header a compaction pass writes for this index.
    pub fn header(&self) -> CheckpointHeader {
        CheckpointHeader {
            store: self.store,
            epoch: self.epoch,
            through_sequence: self.through_sequence,
            next_generation: self.next_generation,
            repository_count: self.repositories.len() as u16,
            head_count: self.heads.len() as u16,
            active_count: self.actives.len() as u8,
            draft_parent_count: u8::from(self.draft_parent.is_some()),
            draft_part_count: self.draft_parts.len() as u8,
            retained_count: self.retained.len() as u8,
            result_start: self.result_start as u8,
            result_count: self.results.len() as u8,
            handoff_count: u8::from(self.handoff.is_some()),
            flags: self.flags,
            terminal_counter: self.terminal_counter,
            weather_count: u8::from(self.weather.is_some()),
            ride_count: u8::from(self.ride.is_some()),
        }
    }

    /// Projects a reference model into this index, in place.
    ///
    /// Every journal-slot reference is left absent: the caller installs the ones its replay
    /// produced through [`note_head_record`](Self::note_head_record). The lease table is *not*
    /// touched — §9 makes leases RAM ownership facts that no projection can reconstruct, and
    /// silently clearing them here would release a live reader's pin.
    pub fn project_into(&mut self, model: &super::model::CatalogModel) {
        self.store = model.store;
        self.epoch = model.epoch;
        self.through_sequence = model.through_sequence;
        self.next_generation = model.next_generation;
        self.terminal_counter = model.terminal_counter;
        self.flags = model.flags;
        self.result_start = model.result_start;

        self.repositories.clear();
        for row in &model.repositories {
            let _ = self.repositories.push(*row);
        }
        self.heads.clear();
        for head in &model.heads {
            let _ = self.heads.push(HeadIndexEntry::from_head(head, NO_JOURNAL_SLOT));
        }
        self.actives.clear();
        for row in &model.actives {
            let _ = self.actives.push(*row);
        }
        self.draft_parent = model.draft_parent;
        self.draft_parts.clear();
        for row in &model.draft_parts {
            let _ = self.draft_parts.push(*row);
        }
        self.retained.clear();
        for row in &model.retained {
            let _ = self.retained.push(*row);
        }
        self.results.clear();
        for result in &model.results {
            let _ = self.results.push(ResultIndexEntry {
                operation: result.operation,
                commit_sequence: result.commit_sequence,
                journal_slot: NO_JOURNAL_SLOT,
            });
        }
        self.handoff = model.handoff;
        self.weather = model.weather;
        self.ride = model.ride;
    }

    /// The same projection, boxed. Host-only: this value is around 19 KiB and nothing on the device
    /// hands one back through a return slot.
    #[cfg(any(test, feature = "std"))]
    pub fn project(model: &super::model::CatalogModel) -> std::boxed::Box<Self> {
        let mut index = std::boxed::Box::new(RamIndex::new(model.store));
        index.project_into(model);
        index
    }
}

/// The measured resident footprint of one index, in bytes.
///
/// §13 requires DOS2 to "measure the exact figure and size its arena from it". This is that figure
/// for the host build; the board's differs only where a `usize` differs, and the two `usize` fields
/// this type has — a `heapless::Vec` length and `result_start` — are the whole of that difference.
pub const fn resident_bytes() -> usize {
    core::mem::size_of::<RamIndex>()
}

#[cfg(test)]
mod tests {
    use super::super::samples;
    use super::*;
    use core::mem::size_of;

    /// §13's per-entry targets, which the budget formula is built out of.
    #[test]
    fn the_two_index_entries_meet_their_per_entry_budgets() {
        assert!(
            size_of::<HeadIndexEntry>() <= HeadIndexEntry::RESIDENT_TARGET,
            "a head index entry is {} bytes, above §13's {}",
            size_of::<HeadIndexEntry>(),
            HeadIndexEntry::RESIDENT_TARGET,
        );
        assert!(
            size_of::<ResultIndexEntry>() <= ResultIndexEntry::RESIDENT_TARGET,
            "a result index entry is {} bytes, above §13's {}",
            size_of::<ResultIndexEntry>(),
            ResultIndexEntry::RESIDENT_TARGET,
        );
        // The four small tables are held "in their on-card shapes", so each Rust row must fit the
        // on-card entry §13 budgets it at.
        assert!(size_of::<ActiveOperation>() <= ActiveOperation::LEN);
        assert!(size_of::<DraftParent>() <= DraftParent::LEN);
        assert!(size_of::<DraftPart>() <= DraftPart::LEN);
        assert!(size_of::<RetainedPrevious>() <= RetainedPrevious::LEN);
    }

    /// The measured figure §13 asks for, held against its own formula.
    ///
    /// The formula covers the head index, the result ring index and the four small tables. This
    /// type also holds what §6.3's pass needs and §13's enumeration does not name — the repository
    /// rows and the three singleton projections — plus the lease table, which §13 adds separately.
    /// The assertion is an envelope rather than an equality because a struct's exact size is the
    /// compiler's business; what must not drift is the total.
    #[test]
    fn the_resident_index_fits_its_measured_envelope() {
        // The additions §13's formula does not enumerate, at their measured sizes.
        let singletons = size_of::<Option<HandoffRef>>()
            + size_of::<Option<WeatherState>>()
            + size_of::<Option<ActiveRide>>()
            + size_of::<Vec<RepositoryState, MAX_REPOSITORY_STATES>>();
        let leases = size_of::<LeaseTable>();
        let envelope = SECTION_13_FORMULA + singletons + leases + 128;
        std::println!(
            "OBC2 resident index: {} bytes (§13 formula {SECTION_13_FORMULA}, singletons {singletons}, leases \
             {leases}, envelope {envelope}); head entry {}, result entry {}",
            resident_bytes(),
            size_of::<HeadIndexEntry>(),
            size_of::<ResultIndexEntry>(),
        );
        assert!(
            resident_bytes() <= envelope,
            "the resident index is {} bytes, above the {envelope}-byte envelope (§13 formula {SECTION_13_FORMULA} + \
             {singletons} of singletons + {leases} of leases + 128 of scalars and lengths)",
            resident_bytes(),
        );
        // And it is genuinely an index rather than a projection: a whole checkpoint body is 65,024
        // bytes and the reference model that holds one is far larger than this.
        assert!(resident_bytes() < super::super::limits::CHECKPOINT_BODY_LEN / 3);
    }

    /// §13: the resolution-present bit is not resident, because §6.3 makes it travel with the field
    /// it describes.
    #[test]
    fn the_resolution_present_bit_is_not_resident() {
        let manifest = samples::manifest_head(3, 92);
        assert_ne!(manifest.flags & CatalogHead::FLAG_RESOLUTION_PRESENT, 0);
        let entry = HeadIndexEntry::from_head(&manifest, NO_JOURNAL_SLOT);
        assert_eq!(entry.flags & CatalogHead::FLAG_RESOLUTION_PRESENT, 0);
        assert_eq!(entry.generation, manifest.generation);
        assert_eq!(entry.key(), manifest.key);
    }

    #[test]
    fn a_projection_reproduces_the_models_header_and_ordering() {
        let mut model = super::super::model::CatalogModel::initial(samples::STORE, 4);
        model.apply(&samples::claim(1, 1, 0, samples::OP_A, 1)).unwrap();
        model.apply(&samples::publish(1, 2, 1, samples::OP_A, 1, samples::head(1, 7))).unwrap();

        let index = RamIndex::project(&model);
        assert_eq!(index.header(), model.header());
        assert_eq!(index.heads.len(), 1);
        assert_eq!(index.heads[0].key(), model.heads[0].key);
        assert_eq!(index.results.len(), 1);
        assert_eq!(index.results[0].commit_sequence, model.results[0].commit_sequence);
        assert_eq!(index.results[0].operation, model.results[0].operation);
    }

    /// The journal-slot reference is installed by replay and cleared by compaction.
    #[test]
    fn journal_slot_references_are_installed_and_reset() {
        let mut model = super::super::model::CatalogModel::initial(samples::STORE, 4);
        model.apply(&samples::claim(1, 1, 0, samples::OP_A, 1)).unwrap();
        model.apply(&samples::publish(1, 2, 1, samples::OP_A, 1, samples::head(1, 7))).unwrap();
        let mut index = RamIndex::project(&model);
        assert_eq!(index.heads[0].journal_slot, NO_JOURNAL_SLOT);

        let key = model.heads[0].key;
        assert!(index.note_head_record(key, 1));
        assert_eq!(index.heads[0].journal_slot, 1);
        // A head the index does not hold is a caller mistake, not a silently ignored write.
        assert!(!index.note_head_record(HeadKey { kind: 9, id: LogicalObjectId::new(99) }, 2));

        index.clear_journal_slots();
        assert_eq!(index.heads[0].journal_slot, NO_JOURNAL_SLOT);
    }

    /// §9's leases are RAM ownership facts, so a re-projection of the catalog must not disturb
    /// them: a replay that cleared a live reader's pin would let GC collect what it is streaming.
    #[test]
    fn projecting_a_model_leaves_the_lease_table_alone() {
        let model = super::super::model::CatalogModel::initial(samples::STORE, 4);
        let mut index = RamIndex::project(&model);
        let session = obc_link::ids::SessionId::new(1).unwrap();
        let _lease = index.leases.pin(1, session, GenerationId::new(42)).unwrap();
        index.project_into(&model);
        assert_eq!(index.leases.live(), 1);
        assert!(index.leases.holds(GenerationId::new(42)));
    }
}
