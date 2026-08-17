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
//! ## There is still exactly one `apply`
//!
//! [`CatalogModel`](super::model::CatalogModel) and this index are two instantiations of one generic
//! [`Projection`](super::model::Projection): every region but the heads and the results is the same
//! type in both, and the two that differ are reached through [`HeadRow`](super::model::HeadRow) and
//! [`ResultRow`](super::model::ResultRow). So `apply` is written once and means one thing, which is
//! what the compaction proof — that a streamed body equals `CatalogModel::encode_body` — rests on.
//! [`RamIndex::project_into`] remains, as the host oracle's way of getting from one to the other.

use obc_link::ids::{GenerationId, LogicalObjectId, OperationId, Revision, StoreId};

use super::checkpoint;
use super::entries::{
    ActiveOperation, ActiveRide, CatalogHead, DraftParent, DraftPart, HeadKey, RepositoryState, RetainedPrevious,
    TerminalResult, WeatherState,
};
use super::handoff::HandoffRef;
use super::leases::LeaseTable;
use super::model::{HeadRow, Projection, ResultRow};

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

    /// The resident half of a decoded head entry, with the journal reference the caller resolved.
    pub fn carried(head: &CatalogHead, journal_slot: u16) -> Self {
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

impl HeadRow for HeadIndexEntry {
    fn head_key(&self) -> HeadKey {
        self.key()
    }

    /// A head put by a record starts with **no** journal reference. `apply` cannot install one: it
    /// is handed a decoded body and does not know which physical slot carried it. The caller that
    /// wrote or replayed that slot installs it afterwards through
    /// [`note_head_record`](RamIndex::note_head_record), which is also the only caller that knows.
    fn from_head(head: &CatalogHead) -> Self {
        HeadIndexEntry::carried(head, NO_JOURNAL_SLOT)
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

impl ResultRow for ResultIndexEntry {
    fn operation_id(&self) -> OperationId {
        self.operation
    }

    fn sequence(&self) -> u64 {
        self.commit_sequence
    }

    /// Same as a head's: the slot reference is the writer's to install, not `apply`'s.
    fn from_result(result: &TerminalResult) -> Self {
        ResultIndexEntry {
            operation: result.operation,
            commit_sequence: result.commit_sequence,
            journal_slot: NO_JOURNAL_SLOT,
        }
    }
}

/// §13's budget formula at the §2 capacities, before the lease table and the bounded staging:
/// `12,800 + 2,048 + 1,152 + 128 + 3,072 + 512`.
pub const SECTION_13_FORMULA: usize = 19_712;

/// The measured resident footprint §13 asks DOS2 to report and size its arena from.
///
/// Pinned exactly rather than bounded, so a field added to any resident table has to change this
/// line and be argued for. It is a 64-bit-host figure: this type's only width-dependent members are
/// the `usize` length each `heapless::Vec` carries and `result_start`, so a 32-bit target measures
/// less, never more — 19,840, which is pinned separately below.
///
/// ## What it does and does not count, against §13's own sentence
///
/// §13: "Add the four-entry lease table, the repository rows and the three singleton projections
/// above, **and the bounded staging**. The measured figure at these capacities is 19,848 bytes, 136
/// above the formula: the additions cost 872."
///
/// The enumeration is one item longer than the arithmetic. Measured here, the additions are the
/// three singleton projections plus the repository rows at **792 bytes** and the lease table at
/// **80** — 872 exactly, and the four small tables come in 736 under their on-card shapes, which is
/// the 136. **The bounded staging is not in the 872 and is not in the 19,848.** §13 sizes that
/// separately in the same section — "the staging compaction needs is one entry of at most 240 bytes
/// plus one 512-byte sector — 752 bytes" — and this crate holds it as
/// [`compaction::STAGING_BYTES`](super::compaction::STAGING_BYTES), on the frame of the pass that
/// needs it rather than in any resident value.
///
/// So the figure is the resident *index*, and a caller sizing an arena adds the 752 itself if it
/// wants the pass's stage in the same allocation. The spec sentence would read truer with the
/// staging struck from its list, which is a one-line amendment for whoever owns the freeze; the
/// number it records is right either way and is what this asserts against.
pub const MEASURED_RESIDENT: usize = 19_848;

/// The bounded resident index: §13's shape of [`Projection`].
///
/// It is the same generic projection [`CatalogModel`](super::model::CatalogModel) is, instantiated
/// over the two rows §13 shrinks — so there is one `apply`, one set of ordering rules and one
/// `header()`, and the only difference between the device's state and the host oracle's is what a
/// head and a result *are*.
///
/// §9's four download leases are resident too and §13 counts them, but they live beside this value
/// in the transaction that owns it rather than inside it: a lease is a RAM ownership fact that no
/// projection reconstructs, so a re-projection must not be able to touch one.
/// [`resident_bytes`] adds them back.
pub type RamIndex = Projection<HeadIndexEntry, ResultIndexEntry>;

impl RamIndex {
    /// An empty index. `const` so it can initialize a `static` rather than travel through a return
    /// slot, for the reason [`CatalogModel::empty`](super::model::CatalogModel::empty) gives.
    pub const fn new(store: StoreId) -> Self {
        Projection::empty(store)
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

    /// Records that journal slot `slot` carries the record which appended the result at
    /// `commit_sequence`.
    ///
    /// Keyed by the commit sequence rather than by "the newest": a replayed suffix installs a
    /// reference per record, and by the time the suffix is applied several of the ring's entries are
    /// journal-carried at once. Returns false when the ring no longer holds that sequence, which the
    /// 64-entry eviction makes an ordinary outcome rather than a caller mistake.
    pub fn note_result_record(&mut self, commit_sequence: u64, slot: u16) -> bool {
        match self.results.iter_mut().find(|entry| entry.commit_sequence == commit_sequence) {
            Some(entry) => {
                entry.journal_slot = slot;
                true
            }
            None => false,
        }
    }

    /// Applies one record **and** installs the journal-slot references it produced (§6.3).
    ///
    /// This is the only way a record should reach the index. `apply` is handed a decoded body and
    /// cannot install a reference — it does not know which physical slot carried it — so a caller
    /// that applied without noting would leave a head or a result whose card-resident half is in a
    /// journal record nothing points at, and the next read of it would go to the active checkpoint
    /// and find the *older* bytes. Writing it and replaying it are the same act here, which is why
    /// the commit path and both mount paths call this one function.
    pub fn absorb(
        &mut self,
        record: &super::journal::JournalBody,
    ) -> core::result::Result<(), super::error::ApplyError> {
        self.apply(record)?;
        if let Some(super::journal::Change::Put(head)) = &record.mutation.head {
            self.note_head_record(head.key, record.slot);
        }
        if let Some(result) = &record.mutation.result {
            self.note_result_record(result.commit_sequence, record.slot);
        }
        Ok(())
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

    /// Projects a reference model into this index, in place.
    ///
    /// Every journal-slot reference is left absent: the caller installs the ones its replay
    /// produced through [`note_head_record`](Self::note_head_record). No lease is touched, because
    /// none is held here — §9 makes leases RAM ownership facts that no projection reconstructs, and
    /// keeping them outside this value is what makes that structural rather than a rule to remember.
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
            let _ = self.heads.push(HeadIndexEntry::carried(head, NO_JOURNAL_SLOT));
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

/// Streams the selected checkpoint into `index`, staging nothing beyond `scratch`.
///
/// This is §13's mount, and the whole reason [`checkpoint::validate_streamed`] takes a sink: the
/// body is validated and projected in **one** forward pass, so no step of a mount ever holds the
/// 65,024 bytes. The index is left holding only what §13 keeps — the envelopes, the resolution
/// generations and the terminal-result bodies stay on the card and are re-read on demand.
///
/// Every journal-slot reference is absent afterwards, which is correct: nothing has been replayed
/// yet, so every card-resident field is in the checkpoint this just read. The caller installs the
/// references its suffix produces through [`RamIndex::absorb`].
///
/// A failed scan leaves the index holding whatever prefix it got to; the caller must discard it.
pub fn load_checkpoint<S: checkpoint::FileSource>(
    index: &mut RamIndex,
    source: &mut S,
    scratch: &mut [u8],
) -> core::result::Result<checkpoint::CheckpointHeader, checkpoint::StreamError<S::Error>> {
    let mut sink = IndexSink { index, wrapped: 0 };
    let header = checkpoint::validate_streamed(source, scratch, &mut sink)?;
    sink.finish();
    Ok(header)
}

/// The [`EntrySink`](checkpoint::EntrySink) that fills a [`RamIndex`] as the scan passes over it.
struct IndexSink<'i> {
    index: &'i mut RamIndex,
    /// How many occupied results were seen at a physical position below `result_start`.
    ///
    /// The scan walks the ring physically and the index holds it in ring order, so the two differ by
    /// exactly this rotation: the wrapped entries are visited first and belong last.
    wrapped: usize,
}

impl IndexSink<'_> {
    fn finish(self) {
        self.index.results.rotate_left(self.wrapped);
    }
}

impl checkpoint::EntrySink for IndexSink<'_> {
    fn header(&mut self, header: &checkpoint::CheckpointHeader) {
        // Every region is cleared here rather than by the caller: a decode that reused a populated
        // index would let a previous store's heads survive into the next one's catalog, and the
        // header is the first thing the scan produces.
        self.index.store = header.store;
        self.index.epoch = header.epoch;
        self.index.through_sequence = header.through_sequence;
        self.index.next_generation = header.next_generation;
        self.index.terminal_counter = header.terminal_counter;
        self.index.flags = header.flags;
        self.index.result_start = header.result_start as usize;
        self.index.repositories.clear();
        self.index.heads.clear();
        self.index.actives.clear();
        self.index.draft_parent = None;
        self.index.draft_parts.clear();
        self.index.retained.clear();
        self.index.results.clear();
        self.index.handoff = None;
        self.index.weather = None;
        self.index.ride = None;
        self.wrapped = 0;
    }

    fn repository(&mut self, row: &RepositoryState) {
        let _ = self.index.repositories.push(*row);
    }

    fn head(&mut self, row: &CatalogHead) {
        let _ = self.index.heads.push(HeadIndexEntry::carried(row, NO_JOURNAL_SLOT));
    }

    fn active(&mut self, row: &ActiveOperation) {
        let _ = self.index.actives.push(*row);
    }

    fn draft_parent(&mut self, row: &DraftParent) {
        self.index.draft_parent = Some(*row);
    }

    fn draft_part(&mut self, row: &DraftPart) {
        let _ = self.index.draft_parts.push(*row);
    }

    fn retained(&mut self, row: &RetainedPrevious) {
        let _ = self.index.retained.push(*row);
    }

    fn result(&mut self, physical: usize, row: &TerminalResult) {
        if physical < self.index.result_start {
            self.wrapped += 1;
        }
        let _ = self.index.results.push(ResultIndexEntry {
            operation: row.operation,
            commit_sequence: row.commit_sequence,
            journal_slot: NO_JOURNAL_SLOT,
        });
    }

    fn handoff(&mut self, row: &HandoffRef) {
        self.index.handoff = Some(*row);
    }

    fn weather(&mut self, row: &WeatherState) {
        self.index.weather = Some(*row);
    }

    fn ride(&mut self, row: &ActiveRide) {
        self.index.ride = Some(*row);
    }
}

/// The measured resident footprint of one index **plus §9's lease table**, in bytes.
///
/// §13 requires DOS2 to "measure the exact figure and size its arena from it", and the list it
/// measures includes "the live-lease table". The table is not a field of [`RamIndex`] — see that
/// type's note — so it is added here, which keeps the figure §13 records and the figure an arena is
/// sized from the same number whichever value happens to hold the leases.
///
/// It is the host build's; the board's differs only where a `usize` differs, and the two `usize`
/// members these types have — a `heapless::Vec` length and `result_start` — are the whole of that
/// difference.
pub const fn resident_bytes() -> usize {
    core::mem::size_of::<RamIndex>() + core::mem::size_of::<LeaseTable>()
}

/// Both measured figures, pinned so neither can drift unremarked.
///
/// Anonymous module-level consts for the reason
/// [`CatalogModel::init_empty`](super::model::CatalogModel::init_empty)'s size assert gives: an
/// associated const is evaluated lazily and would gate nothing. Two values because the board is
/// 32-bit thumbv8m and the host suite 64-bit, and the difference is exactly the seven `usize`
/// members these types carry — six `heapless::Vec` lengths and `result_start`.
#[cfg(target_pointer_width = "64")]
const _: () = assert!(resident_bytes() == MEASURED_RESIDENT);
#[cfg(target_pointer_width = "32")]
const _: () = assert!(resident_bytes() == 19_840);

#[cfg(test)]
mod tests {
    use super::super::limits::MAX_REPOSITORY_STATES;
    use super::super::samples;
    use super::*;
    use core::mem::size_of;
    use heapless::Vec;

    /// §13's mount: streaming the checkpoint into the index produces exactly the projection the
    /// whole-body decode would, **including the ring's rotation**.
    ///
    /// The scan walks the result region physically and the index holds it in ring order, so a start
    /// that is not zero is the case where the two disagree — and a `result_start` past the wrap is
    /// reached only after 64 commits, which is why the fixture forces one rather than hoping.
    #[test]
    fn streaming_a_checkpoint_into_the_index_equals_projecting_the_decoded_one() {
        use super::super::limits::CHECKPOINT_BODY_LEN;
        for commits in [2u64, 66, 70] {
            let mut model = super::super::model::CatalogModel::initial(samples::STORE, 4);
            for step in 1..=commits {
                model.apply(&samples::claim(1, step * 2 - 1, 0, [step as u8; 16], step)).unwrap();
                model
                    .apply(&samples::publish(1, step * 2, 0, [step as u8; 16], step, samples::head(1, step % 200)))
                    .unwrap();
            }
            let mut body = std::boxed::Box::new([0u8; CHECKPOINT_BODY_LEN]);
            model.encode_body(body.as_mut_slice()).unwrap();

            let mut streamed = std::boxed::Box::new(RamIndex::new(samples::STORE));
            let mut scratch = [0u8; 512];
            let header = load_checkpoint(&mut streamed, &mut checkpoint::SliceSource(body.as_slice()), &mut scratch)
                .expect("the streamed mount");
            assert_eq!(header, model.header());
            assert_eq!(*streamed, *RamIndex::project(&model), "commits {commits}");
            assert_eq!(streamed.result_start, model.result_start);
        }
    }

    /// A streamed mount reuses whatever index it is handed, so a previous store's rows must not
    /// survive into the next one's catalog.
    #[test]
    fn streaming_into_a_populated_index_leaves_nothing_of_the_old_one() {
        use super::super::limits::CHECKPOINT_BODY_LEN;
        let mut model = super::super::model::CatalogModel::initial(samples::STORE, 4);
        model.apply(&samples::claim(1, 1, 0, samples::OP_A, 1)).unwrap();
        model.apply(&samples::publish(1, 2, 1, samples::OP_A, 1, samples::head(1, 7))).unwrap();
        let mut populated = RamIndex::project(&model);
        assert!(!populated.heads.is_empty() && !populated.results.is_empty());

        let mut fresh = std::boxed::Box::new([0u8; CHECKPOINT_BODY_LEN]);
        super::super::model::CatalogModel::encode_initial_body(fresh.as_mut_slice(), samples::STORE, 4).unwrap();
        let mut scratch = [0u8; 512];
        load_checkpoint(&mut populated, &mut checkpoint::SliceSource(fresh.as_slice()), &mut scratch).unwrap();
        assert_eq!(*populated, *RamIndex::project(&super::super::model::CatalogModel::initial(samples::STORE, 4)));
    }

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

    /// The measured figure §13 asks for, **pinned**.
    ///
    /// The formula covers the head index, the result ring index and the four small tables. This
    /// type also holds what §6.3's pass needs and §13's enumeration does not name — the repository
    /// rows and the three singleton projections — plus the lease table, which §13 adds separately.
    ///
    /// Equality rather than an envelope: this is the number §13 records and the number an arena is
    /// sized from, so a field added to any resident table has to move this line deliberately rather
    /// than be absorbed by slack. The breakdown is printed so a change explains its own diff.
    #[test]
    fn the_resident_index_is_its_measured_footprint() {
        // The additions §13's formula does not enumerate, at their measured sizes.
        let singletons = size_of::<Option<HandoffRef>>()
            + size_of::<Option<WeatherState>>()
            + size_of::<Option<ActiveRide>>()
            + size_of::<Vec<RepositoryState, MAX_REPOSITORY_STATES>>();
        let leases = size_of::<LeaseTable>();
        std::println!(
            "OBC2 resident index: {} bytes (§13 formula {SECTION_13_FORMULA}, singletons {singletons}, leases \
             {leases}); head entry {}, result entry {}",
            resident_bytes(),
            size_of::<HeadIndexEntry>(),
            size_of::<ResultIndexEntry>(),
        );
        assert_eq!(
            resident_bytes(),
            MEASURED_RESIDENT,
            "the resident index moved: §13's formula is {SECTION_13_FORMULA}, the singletons {singletons}, the lease \
             table {leases}. Move MEASURED_RESIDENT and §13's recorded figure together.",
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
        let entry = HeadIndexEntry::carried(&manifest, NO_JOURNAL_SLOT);
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

    /// §9's leases are RAM ownership facts no projection reconstructs, so a re-projection must not
    /// be able to disturb one — a replay that cleared a live reader's pin would let GC collect what
    /// it is streaming. Keeping the table outside the index makes that structural, and this is the
    /// statement of that: a re-projection is total over the index and reaches no lease at all.
    #[test]
    fn a_re_projection_cannot_reach_a_lease() {
        let model = super::super::model::CatalogModel::initial(samples::STORE, 4);
        let mut index = RamIndex::project(&model);
        let mut leases = LeaseTable::new();
        let session = obc_link::ids::SessionId::new(1).unwrap();
        let _lease = leases.pin(1, session, GenerationId::new(42)).unwrap();
        index.project_into(&model);
        assert_eq!(leases.live(), 1);
        assert!(leases.holds(GenerationId::new(42)));
    }
}
