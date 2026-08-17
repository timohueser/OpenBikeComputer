//! The in-memory catalog projection, and the apply-mutation semantics that move it
//! (`OBC2_Storage_Format.md` §5, §6).
//!
//! This is the **reference model**: one bounded value holding every region a checkpoint body holds,
//! with a total `apply` that is the meaning of a journal record. Replay is literally
//! `for record in suffix { model.apply(record) }`, and the crash harness uses the result as its
//! oracle: recovery must land on exactly the model's before-state or its after-state, never
//! anything else.
//!
//! [`CatalogModel::encode_body`] materializes a body from a projection, which is what compaction
//! *produces* — but it is not compaction. §6.3 specifies a bounded forward pass that never holds a
//! projection at all: region by region, entry by entry, staging one 208-byte entry plus a 512-byte
//! sector, taking each card-resident field from the RAM index, from a journal-carried head entry,
//! or from the active checkpoint's stored bytes. That pass, and the RAM index it reads, are a later
//! slice. What this function gives that slice is the answer to check itself against.
//!
//! It is deliberately **not** the device's resident state. §13 fixes that as a bounded *index* —
//! about 19.25 KiB, with envelopes and resolution generations left on the card and re-read on
//! demand — while this value holds whole entries because a host oracle has no reason not to.
//! Nothing here is instantiated by the device image.
//!
//! Every region is a `heapless::Vec` at its §2 capacity, so a mutation that would exceed one
//! returns [`ApplyError::ResourceLimit`] rather than growing: there is no allocator anywhere in
//! this crate.

use heapless::Vec;
use obc_link::ids::{GenerationId, OperationId, StoreId};

use super::checkpoint::{self, CheckpointHeader, Region};
use super::entries::{
    ActiveOperation, ActiveRide, CatalogHead, DraftParent, DraftPart, HeadKey, RepositoryState, RetainedPrevious,
    TerminalResult, WeatherState,
};
use super::error::{ApplyError, Record, Result};
use super::handoff::HandoffRef;
use super::journal::{Change, JournalBody, RecordKind};
use super::limits::{
    CHECKPOINT_BODY_CRC_OFFSET, CHECKPOINT_BODY_LEN, MAX_ACTIVE_OPERATIONS, MAX_CATALOG_HEADS, MAX_DRAFT_PARTS,
    MAX_REPOSITORY_STATES, MAX_RETAINED_PREVIOUS, MAX_TERMINAL_RESULTS,
};
use super::raw::put_bytes;

/// One row of the head region, in whichever shape its holder keeps.
///
/// [`CatalogHead`] is the whole on-card entry, which is what a host oracle holds; the device holds
/// [`HeadIndexEntry`](super::index::HeadIndexEntry), which drops the envelope and the resolution
/// generation because §13 leaves those on the card. `apply` is written once against this trait so
/// the two shapes cannot acquire two meanings of a record.
pub trait HeadRow: Copy {
    /// The `(kind, logical id)` this row is keyed by.
    fn head_key(&self) -> HeadKey;

    /// The row a `Change::Put` of this decoded head produces.
    fn from_head(head: &CatalogHead) -> Self;
}

impl HeadRow for CatalogHead {
    fn head_key(&self) -> HeadKey {
        self.key
    }

    fn from_head(head: &CatalogHead) -> Self {
        *head
    }
}

/// One row of the terminal-result ring, in whichever shape its holder keeps.
///
/// The device keeps §13's `(OperationId, commit sequence)` pair and re-reads the 208-byte body from
/// the card; a host oracle keeps the whole [`TerminalResult`].
pub trait ResultRow: Copy {
    /// The operation this result belongs to — the key `QueryOperation` looks up.
    fn operation_id(&self) -> OperationId;

    /// Its terminal commit sequence, which is also the ring's ordering.
    fn sequence(&self) -> u64;

    /// The row a `Mutation::result` of this decoded entry produces.
    fn from_result(result: &TerminalResult) -> Self;
}

impl ResultRow for TerminalResult {
    fn operation_id(&self) -> OperationId {
        self.operation
    }

    fn sequence(&self) -> u64 {
        self.commit_sequence
    }

    fn from_result(result: &TerminalResult) -> Self {
        *result
    }
}

/// The bounded catalog projection, over whichever head and result shapes its holder keeps.
///
/// [`CatalogModel`] is the host oracle's instantiation and [`RamIndex`](super::index::RamIndex) the
/// device's. Everything that is not a head or a result — the repository rows, the active table, the
/// draft tables, the retained table, the three singletons and the six scalars — is identical in
/// both, which is why they are one type rather than two.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Projection<H: HeadRow, R: ResultRow> {
    /// The store this projection belongs to.
    pub store: StoreId,
    /// The compaction epoch.
    pub epoch: u64,
    /// The last journal sequence absorbed.
    pub through_sequence: u64,
    /// The next `GenerationId` cursor.
    pub next_generation: u64,
    /// The terminal-commit counter.
    pub terminal_counter: u64,
    /// Header flags; bit 0 is the durable store-wide degraded record.
    pub flags: u8,
    /// Repository rows, sorted by kind.
    pub repositories: Vec<RepositoryState, MAX_REPOSITORY_STATES>,
    /// Catalog heads, sorted by `(kind, logical id)`.
    pub heads: Vec<H, MAX_CATALOG_HEADS>,
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
    /// The result ring, in ring order from `result_start`.
    pub results: Vec<R, MAX_TERMINAL_RESULTS>,
    /// The one update-handoff projection.
    pub handoff: Option<HandoffRef>,
    /// The one weather-request state.
    pub weather: Option<WeatherState>,
    /// The one active-ride state.
    pub ride: Option<ActiveRide>,
}

/// The host oracle's projection: whole heads and whole terminal results.
///
/// It is deliberately **not** the device's resident state — §13 fixes that as
/// [`RamIndex`](super::index::RamIndex) — and nothing in the device image instantiates one.
pub type CatalogModel = Projection<CatalogHead, TerminalResult>;

/// The size [`CatalogModel::init_empty`]'s field list was written against.
///
/// `init_empty` writes the struct field by field through a raw pointer, so its soundness is the
/// claim that the list is complete; a field added without a line there is an uninitialized hole that
/// reads as initialized. This is one of the two things that make that a build error — the other is
/// `every_field_is_named_by_the_in_place_constructor`, which destructures the struct by name.
///
/// It is an **anonymous module-level** const on purpose: an associated `const _SIZE` inside the impl
/// is evaluated lazily and never checked unless something reads it, so it would look like a guard
/// and gate nothing. (Verified by setting it wrong: the impl-scoped form built clean.) Two values
/// because the two targets have different pointer widths — the board is 32-bit thumbv8m, the host
/// suite 64-bit — and both are measured rather than derived.
#[cfg(target_pointer_width = "32")]
const _: () = assert!(core::mem::size_of::<CatalogModel>() == 56_112);
#[cfg(target_pointer_width = "64")]
const _: () = assert!(core::mem::size_of::<CatalogModel>() == 56_120);

impl<H: HeadRow, R: ResultRow> Projection<H, R> {
    /// An empty projection, `const` so it can initialize a `static` rather than be returned by
    /// value.
    ///
    /// This value is around 56 KiB. Nothing in this crate hands one back through a return slot for
    /// that reason — every constructor here either fills a `&mut` the caller placed or, on a host,
    /// hands back a [`Box`]. A board crate that copied one through a stack temporary would blow the
    /// ~36 KiB task stack the nRF54L notes measure.
    pub const fn empty(store: StoreId) -> Self {
        Projection {
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
        }
    }

    /// Resets this projection to the first checkpoint of a freshly initialized store (§12): epoch 1,
    /// through-sequence 0, next `GenerationId` 0, terminal counter 0, and weather logical ID zero
    /// reserved by setting the weather repository's next candidate to one while leaving the
    /// weather state absent.
    ///
    /// It clears field by field rather than assigning [`empty`](Self::empty). `*self = empty(store)`
    /// reads as the same thing and is not: it builds a 56 KiB value in a stack temporary and copies
    /// it in. On the nRF54L that made **57,344 bytes** of every `KernelTransaction::execute` frame —
    /// paid by every command, because a function's frame is the maximum over all its arms and this
    /// one reached `execute` through §16's `ResetStore`. Measured, then removed.
    pub fn reset_to_initial(&mut self, store: StoreId, weather_kind: u16) {
        self.store = store;
        self.epoch = 1;
        self.through_sequence = 0;
        self.next_generation = 0;
        self.terminal_counter = 0;
        self.flags = 0;
        self.repositories.clear();
        self.heads.clear();
        self.actives.clear();
        self.draft_parent = None;
        self.draft_parts.clear();
        self.retained.clear();
        self.result_start = 0;
        self.results.clear();
        self.handoff = None;
        self.weather = None;
        self.ride = None;
        let _ = self.repositories.push(RepositoryState {
            kind: weather_kind,
            flags: 0,
            revision: obc_link::ids::Revision::ZERO,
            next_logical_id: obc_link::ids::LogicalObjectId::new(1),
        });
    }

    /// Initializes an empty projection into storage the caller already owns.
    ///
    /// [`empty`](Self::empty) is `const` and returns by value, which at a board call site is a
    /// 56 KiB stack temporary — 58,112 bytes of the bench's own frame, measured. This writes each
    /// field into the uninitialized slot and materializes nothing: the bounded vectors are written
    /// as `Vec::new()`, whose buffer is uninitialized by construction, so only their lengths are
    /// stored.
    pub fn init_empty(slot: &mut core::mem::MaybeUninit<Self>, store: StoreId) -> &mut Self {
        let at = slot.as_mut_ptr();
        // SAFETY: every field of `CatalogModel` is written exactly once below, through a raw
        // pointer into the caller's uninitialized slot, and none is read before it is written.
        //
        // "Exhaustive" is the whole safety argument, and prose does not enforce it — a field added
        // to the struct without a line here is uninitialized memory that reads as initialized. Two
        // things make that a build error rather than a silent hole: the size assert below, which any
        // layout change trips, and `every_field_is_named_by_the_in_place_constructor` in this
        // module's tests, which destructures the struct by name so a new field cannot compile until
        // it is acknowledged.
        unsafe {
            core::ptr::addr_of_mut!((*at).store).write(store);
            core::ptr::addr_of_mut!((*at).epoch).write(1);
            core::ptr::addr_of_mut!((*at).through_sequence).write(0);
            core::ptr::addr_of_mut!((*at).next_generation).write(0);
            core::ptr::addr_of_mut!((*at).terminal_counter).write(0);
            core::ptr::addr_of_mut!((*at).flags).write(0);
            core::ptr::addr_of_mut!((*at).repositories).write(Vec::new());
            core::ptr::addr_of_mut!((*at).heads).write(Vec::new());
            core::ptr::addr_of_mut!((*at).actives).write(Vec::new());
            core::ptr::addr_of_mut!((*at).draft_parent).write(None);
            core::ptr::addr_of_mut!((*at).draft_parts).write(Vec::new());
            core::ptr::addr_of_mut!((*at).retained).write(Vec::new());
            core::ptr::addr_of_mut!((*at).result_start).write(0);
            core::ptr::addr_of_mut!((*at).results).write(Vec::new());
            core::ptr::addr_of_mut!((*at).handoff).write(None);
            core::ptr::addr_of_mut!((*at).weather).write(None);
            core::ptr::addr_of_mut!((*at).ride).write(None);
            slot.assume_init_mut()
        }
    }

    /// The same first checkpoint, boxed. Host-only for the reason [`empty`](Self::empty) gives.
    #[cfg(any(test, feature = "std"))]
    pub fn initial(store: StoreId, weather_kind: u16) -> std::boxed::Box<Self> {
        let mut model = std::boxed::Box::new(Self::empty(store));
        model.reset_to_initial(store, weather_kind);
        model
    }

    /// The head a `(kind, logical id)` names.
    pub fn head(&self, key: HeadKey) -> Option<&H> {
        self.heads.iter().find(|head| head.head_key() == key)
    }

    /// The retained entry a generation names.
    pub fn retained_entry(&self, generation: GenerationId) -> Option<&RetainedPrevious> {
        self.retained.iter().find(|entry| entry.generation == generation)
    }

    /// The retained terminal result for an `OperationId`, if it is still inside the 64-entry window.
    ///
    /// §2: after eviction `QueryOperation` returns `Unknown`, "which is an indeterminate old
    /// outcome, not permission to retry that identity".
    pub fn result_for(&self, operation: OperationId) -> Option<&R> {
        self.results.iter().find(|result| result.operation_id() == operation)
    }

    /// The ring position and the row an `OperationId` names.
    ///
    /// The position is the *physical* index the entry occupies in both checkpoints, which is what a
    /// card re-read of the 208-byte body addresses (§6.3): the ring's start and count are resident,
    /// so a result does not move when the checkpoint is rewritten.
    pub fn result_position(&self, operation: OperationId) -> Option<(usize, R)> {
        let step = self.results.iter().position(|result| result.operation_id() == operation)?;
        Some(((self.result_start + step) % MAX_TERMINAL_RESULTS, self.results[step]))
    }

    /// Applies one decoded journal record.
    ///
    /// The record must be the contiguous successor of what this projection has absorbed: same
    /// store, same epoch, sequence exactly `through_sequence + 1` (§6.3). Everything past those
    /// three checks is the mutation's own semantics.
    ///
    /// **`apply` is all-or-nothing.** Every rule is proved against `&self` first and only then is
    /// anything written, so a refused record leaves the projection byte-identical to what it was.
    /// That is not a nicety: replay stops at the first record that does not apply, and a partially
    /// applied one would make the mounted state a thing no sequence of records can produce.
    pub fn apply(&mut self, record: &JournalBody) -> core::result::Result<(), ApplyError> {
        self.check(record)?;
        self.commit(record);
        Ok(())
    }

    /// Proves every rule `commit` then relies on. Reads only.
    fn check(&self, record: &JournalBody) -> core::result::Result<(), ApplyError> {
        if record.store != self.store {
            return Err(ApplyError::StoreId);
        }
        if record.epoch != self.epoch {
            return Err(ApplyError::Epoch);
        }
        let expected = self.through_sequence.checked_add(1).ok_or(ApplyError::Sequence)?;
        if record.sequence != expected {
            return Err(ApplyError::Sequence);
        }
        let mutation = &record.mutation;

        // §6.1 bit 18: "the encoded cursor must be the current cursor plus one without wrap. The
        // record reserves the former cursor value as its GenerationId." The reserved value is the
        // *former* cursor, and §6.1 names exactly which entry of which record kind carries it, so
        // the carrier is checked to hold that value rather than merely to be present.
        if let Some(cursor) = mutation.generation_cursor {
            if cursor != self.next_generation.checked_add(1).ok_or(ApplyError::GenerationCursor)? {
                return Err(ApplyError::GenerationCursor);
            }
            let reserved = self.next_generation;
            let carried = match (record.kind, &mutation.active, &mutation.draft_parent, &mutation.ride) {
                // "A normal claim carries that value in an active entry with flag bit 4."
                (RecordKind::Claim, Some(Change::Put(row)), _, _) => {
                    row.flags & ActiveOperation::FLAG_GENERATION_RESERVED != 0 && row.generation.get() == reserved
                }
                // "an update rollback-snapshot reservation carries it in the already-active install
                // entry; and a parent-manifest work record ... in the draft-parent row's reserved
                // resolution field."
                (RecordKind::Work, Some(Change::Put(row)), _, _) => {
                    row.flags & ActiveOperation::FLAG_GENERATION_RESERVED != 0 && row.generation.get() == reserved
                }
                (RecordKind::Work, _, Some(Change::Put(parent)), _) => parent.resolution.get() == reserved,
                // "a pre-claim ride domain record carries it in ActiveRideState".
                (RecordKind::Domain, _, _, Some(Change::Put(ride))) => ride.generation.get() == reserved,
                _ => false,
            };
            if !carried {
                return Err(ApplyError::GenerationCursor);
            }
        }

        if let Some(repository) = &mutation.repository {
            if !self.repositories.iter().any(|row| row.kind == repository.kind)
                && self.repositories.len() == MAX_REPOSITORY_STATES
            {
                return Err(ApplyError::ResourceLimit(Record::RepositoryState));
            }
        }

        match &mutation.active {
            Some(Change::Put(row)) => {
                let key = row.operation.to_bytes();
                if !self.actives.iter().any(|held| held.operation.to_bytes() == key)
                    && self.actives.len() == MAX_ACTIVE_OPERATIONS
                {
                    return Err(ApplyError::ResourceLimit(Record::ActiveOperation));
                }
            }
            Some(Change::Remove(key)) if !self.actives.iter().any(|held| held.operation == *key) => {
                return Err(ApplyError::MissingKey(Record::ActiveOperation))
            }
            Some(Change::Remove(_)) => {}
            None => {}
        }

        match &mutation.head {
            Some(Change::Put(row)) => {
                if !self.heads.iter().any(|held| held.head_key() == row.key) && self.heads.len() == MAX_CATALOG_HEADS {
                    return Err(ApplyError::ResourceLimit(Record::CatalogHead));
                }
            }
            Some(Change::Remove(key)) if !self.heads.iter().any(|held| held.head_key() == *key) => {
                return Err(ApplyError::MissingKey(Record::CatalogHead))
            }
            Some(Change::Remove(_)) => {}
            None => {}
        }

        // §2 bounds draft parents at one, and §5.1 gives the region one row. A put that named a
        // different parent while one was open would silently replace it and strand its parts, so it
        // is a resource limit, not an update.
        match &mutation.draft_parent {
            Some(Change::Put(row)) => match self.draft_parent {
                Some(open) if open.parent != row.parent => return Err(ApplyError::ResourceLimit(Record::DraftParent)),
                _ => {}
            },
            Some(Change::Remove(key)) => match self.draft_parent {
                Some(open) if open.parent == *key => {}
                _ => return Err(ApplyError::MissingKey(Record::DraftParent)),
            },
            None => {}
        }

        // A part belongs to the parent this record leaves open — the one it puts, or the one
        // already open. §5.3 keys a part by its parent, so a row naming any other parent is not a
        // membership fact this projection can hold.
        let effective_parent = match &mutation.draft_parent {
            Some(Change::Put(row)) => Some(row.parent),
            Some(Change::Remove(_)) => None,
            None => self.draft_parent.map(|parent| parent.parent),
        };
        match &mutation.draft_part {
            Some(Change::Put(row)) => {
                if effective_parent != Some(row.key.parent) {
                    return Err(ApplyError::MissingKey(Record::DraftParent));
                }
                if !self.draft_parts.iter().any(|held| held.key == row.key) && self.draft_parts.len() == MAX_DRAFT_PARTS
                {
                    return Err(ApplyError::ResourceLimit(Record::DraftPart));
                }
            }
            Some(Change::Remove(key)) if !self.draft_parts.iter().any(|held| held.key == *key) => {
                return Err(ApplyError::MissingKey(Record::DraftPart))
            }
            Some(Change::Remove(_)) => {}
            None => {}
        }

        match &mutation.retained {
            Some(Change::Put(row)) => {
                if !self.retained.iter().any(|held| held.generation == row.generation)
                    && self.retained.len() == MAX_RETAINED_PREVIOUS
                {
                    return Err(ApplyError::ResourceLimit(Record::RetainedPrevious));
                }
            }
            Some(Change::Remove(key)) if !self.retained.iter().any(|held| held.generation == *key) => {
                return Err(ApplyError::MissingKey(Record::RetainedPrevious))
            }
            Some(Change::Remove(_)) => {}
            None => {}
        }

        if let Some(result) = &mutation.result {
            // §5.3: "`terminal commit sequence` is the checkpoint's terminal-commit counter after
            // increment", so the counter and the appended entry move together or not at all.
            let next = self.terminal_counter.checked_add(1).ok_or(ApplyError::TerminalCounter)?;
            if result.commit_sequence != next {
                return Err(ApplyError::TerminalCounter);
            }
            // §8.1 answers `QueryOperation` by `OperationId`, so two ring entries under one identity
            // make the answer ambiguous — `result_for` would return whichever came first and the
            // client would be told about an outcome that is not the newest. §11 already makes this
            // unreachable from the wire, because an identifier is spent once terminal and a
            // readmission is served the retained result rather than re-claiming; the identities a
            // device-local producer mints are unique by construction for the same reason. So this
            // refuses a state no record can legitimately reach rather than one that merely should
            // not happen — and it is what makes "unique by construction" checkable instead of
            // argued. An identity the 64-entry window has already evicted is not in the ring and is
            // free to be claimed again.
            if self.results.iter().any(|held| held.operation_id() == result.operation) {
                return Err(ApplyError::MissingKey(Record::TerminalResult));
            }
        }

        if matches!(mutation.handoff, Some(Change::Remove(()))) && self.handoff.is_none() {
            return Err(ApplyError::MissingKey(Record::HandoffRef));
        }
        if matches!(mutation.ride, Some(Change::Remove(()))) && self.ride.is_none() {
            return Err(ApplyError::MissingKey(Record::ActiveRide));
        }
        Ok(())
    }

    /// Writes what [`check`](Self::check) proved. Infallible by construction.
    fn commit(&mut self, record: &JournalBody) {
        let mutation = &record.mutation;

        if let Some(cursor) = mutation.generation_cursor {
            self.next_generation = cursor;
        }

        if let Some(repository) = &mutation.repository {
            let index = match self.repositories.iter().position(|row| row.kind == repository.kind) {
                Some(index) => index,
                None => {
                    let fresh = RepositoryState {
                        kind: repository.kind,
                        flags: 0,
                        revision: obc_link::ids::Revision::ZERO,
                        next_logical_id: obc_link::ids::LogicalObjectId::ZERO,
                    };
                    let position = self.repositories.iter().position(|row| row.kind > repository.kind);
                    insert(&mut self.repositories, position, fresh);
                    position.unwrap_or(self.repositories.len() - 1)
                }
            };
            let row = &mut self.repositories[index];
            if let Some(revision) = repository.revision {
                row.revision = obc_link::ids::Revision::new(revision);
            }
            if let Some(next) = repository.next_logical_id {
                row.next_logical_id = obc_link::ids::LogicalObjectId::new(next);
                row.flags = repository.flags;
            }
        }

        match &mutation.active {
            Some(Change::Put(row)) => {
                let key = row.operation.to_bytes();
                match self.actives.iter().position(|held| held.operation.to_bytes() == key) {
                    Some(index) => self.actives[index] = *row,
                    None => {
                        let position = self.actives.iter().position(|held| held.operation.to_bytes() > key);
                        insert(&mut self.actives, position, *row);
                    }
                }
            }
            Some(Change::Remove(key)) => {
                if let Some(index) = self.actives.iter().position(|held| held.operation == *key) {
                    remove(&mut self.actives, index);
                }
            }
            None => {}
        }

        match &mutation.head {
            Some(Change::Put(row)) => match self.heads.iter().position(|held| held.head_key() == row.key) {
                Some(index) => self.heads[index] = H::from_head(row),
                None => {
                    let position = self.heads.iter().position(|held| held.head_key() > row.key);
                    insert(&mut self.heads, position, H::from_head(row));
                }
            },
            Some(Change::Remove(key)) => {
                if let Some(index) = self.heads.iter().position(|held| held.head_key() == *key) {
                    remove(&mut self.heads, index);
                }
            }
            None => {}
        }

        match &mutation.draft_parent {
            Some(Change::Put(row)) => self.draft_parent = Some(*row),
            Some(Change::Remove(key)) => {
                self.draft_parent = None;
                // §6.1: "Removing a terminal draft parent also removes every draft-part row with
                // that parent in the same replay step."
                retain_parts(&mut self.draft_parts, *key);
            }
            None => {}
        }

        match &mutation.draft_part {
            Some(Change::Put(row)) => match self.draft_parts.iter().position(|held| held.key == row.key) {
                Some(index) => self.draft_parts[index] = *row,
                None => {
                    let position = self.draft_parts.iter().position(|held| held.key.sort_key() > row.key.sort_key());
                    insert(&mut self.draft_parts, position, *row);
                }
            },
            Some(Change::Remove(key)) => {
                if let Some(index) = self.draft_parts.iter().position(|held| held.key == *key) {
                    remove(&mut self.draft_parts, index);
                }
            }
            None => {}
        }

        match &mutation.retained {
            Some(Change::Put(row)) => match self.retained.iter().position(|held| held.generation == row.generation) {
                Some(index) => self.retained[index] = *row,
                None => {
                    let position = self.retained.iter().position(|held| held.generation > row.generation);
                    insert(&mut self.retained, position, *row);
                }
            },
            Some(Change::Remove(key)) => {
                if let Some(index) = self.retained.iter().position(|held| held.generation == *key) {
                    remove(&mut self.retained, index);
                }
            }
            None => {}
        }

        if let Some(result) = &mutation.result {
            self.terminal_counter += 1;
            if self.results.len() == MAX_TERMINAL_RESULTS {
                // "Ring append writes `(result_start + result_count) mod 64`; when already full it
                // overwrites `result_start` and advances that index by one. This is the only
                // eviction path."
                self.results[0] = R::from_result(result);
                self.results.rotate_left(1);
                self.result_start = (self.result_start + 1) % MAX_TERMINAL_RESULTS;
            } else {
                let _ = self.results.push(R::from_result(result));
            }
        }

        match &mutation.handoff {
            Some(Change::Put(row)) => self.handoff = Some(*row),
            Some(Change::Remove(())) => self.handoff = None,
            None => {}
        }

        if let Some(weather) = &mutation.weather {
            self.weather = Some(*weather);
        }

        match &mutation.ride {
            Some(Change::Put(row)) => self.ride = Some(*row),
            Some(Change::Remove(())) => self.ride = None,
            None => {}
        }

        self.through_sequence = record.sequence;
    }

    /// The header this projection encodes to.
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
}

impl CatalogModel {
    /// Materializes the complete 65,024-byte checkpoint body, CRC included.
    ///
    /// The caller owns the buffer, which is what lets the device write it a sector at a time
    /// through the bounded forward pass of §6.3 instead of holding one; the host oracle simply
    /// hands in 65,024 bytes.
    pub fn encode_body(&self, out: &mut [u8]) -> Result<()> {
        use super::error::{DecodeError, Reason};
        if out.len() != CHECKPOINT_BODY_LEN {
            return Err(DecodeError::new(Record::Checkpoint, Reason::Length));
        }
        out.fill(0);
        put_bytes(out, 0, &self.header().encode());
        write_region(out, checkpoint::REPOSITORIES, &self.repositories, |row| row.encode());
        write_region(out, checkpoint::HEADS, &self.heads, |row| row.encode());
        write_region(out, checkpoint::ACTIVE, &self.actives, |row| row.encode());
        if let Some(parent) = &self.draft_parent {
            put_bytes(out, checkpoint::DRAFT_PARENT.offset, &parent.encode());
        }
        write_region(out, checkpoint::DRAFT_PARTS, &self.draft_parts, |row| row.encode());
        write_region(out, checkpoint::RETAINED, &self.retained, |row| row.encode());
        for (step, result) in self.results.iter().enumerate() {
            let index = (self.result_start + step) % MAX_TERMINAL_RESULTS;
            put_bytes(out, checkpoint::RESULTS.slot(index).start, &result.encode());
        }
        if let Some(handoff) = &self.handoff {
            put_bytes(out, checkpoint::HANDOFF.offset, &handoff.encode());
        }
        if let Some(weather) = &self.weather {
            put_bytes(out, checkpoint::WEATHER.offset, &weather.encode());
        }
        if let Some(ride) = &self.ride {
            put_bytes(out, checkpoint::RIDE.offset, &ride.encode());
        }
        checkpoint::seal_body(out);
        Ok(())
    }

    /// Encodes §12's **initial** checkpoint body — the one a freshly initialized store is born
    /// with — without building a projection at all.
    ///
    /// A store's initialization and its §16 reset both need these 65,024 bytes and nothing else, and
    /// the obvious route to them (`CatalogModel::empty`, `reset_to_initial`, `encode_body`) puts a
    /// 56 KiB projection on the caller's stack for the sake of one header and one repository row.
    /// The board cannot afford that on its boot path, so this writes the two records directly.
    ///
    /// The duplication that buys is pinned by a test: the bytes here are asserted equal to what an
    /// empty projection encodes, so the two definitions cannot drift.
    pub fn encode_initial_body(out: &mut [u8], store: StoreId, weather_kind: u16) -> Result<()> {
        use super::error::{DecodeError, Reason};
        if out.len() != CHECKPOINT_BODY_LEN {
            return Err(DecodeError::new(Record::Checkpoint, Reason::Length));
        }
        out.fill(0);
        put_bytes(out, 0, &initial_header(store).encode());
        put_bytes(out, checkpoint::REPOSITORIES.slot(0).start, &initial_repository(weather_kind).encode());
        checkpoint::seal_body(out);
        Ok(())
    }

    /// The same initial body, emitted in bounded chunks, returning the body CRC its gate carries.
    ///
    /// §12's birth point needs these 65,024 bytes on the card and nothing needs them in RAM: the
    /// only non-zero bytes are the header and one repository row, both inside the first chunk. So a
    /// device stages `chunk` — one sector is enough, one stride is faster — instead of the 65,536-byte
    /// image a whole-body encode would ask for, which is the same reason §13 gives for streaming the
    /// mount.
    ///
    /// `write` is handed `(offset, bytes)` spans in ascending order and must write all of them.
    pub fn stream_initial_body<E>(
        store: StoreId,
        weather_kind: u16,
        chunk: &mut [u8],
        mut write: impl FnMut(usize, &[u8]) -> core::result::Result<(), E>,
    ) -> core::result::Result<u32, E> {
        debug_assert!(chunk.len() >= checkpoint::HEADER_LEN + RepositoryState::LEN, "the first chunk holds both rows");
        let mut crc = obc_crc::Crc32::new();
        let mut at = 0usize;
        let mut sealed = 0u32;
        while at < CHECKPOINT_BODY_LEN {
            let take = chunk.len().min(CHECKPOINT_BODY_LEN - at);
            let span = &mut chunk[..take];
            span.fill(0);
            if at == 0 {
                put_bytes(span, 0, &initial_header(store).encode());
                put_bytes(span, checkpoint::REPOSITORIES.slot(0).start, &initial_repository(weather_kind).encode());
            }
            // §1 treats a CRC field as zero while its record is checksummed, and it is still zero
            // here — so the accumulator is already the value by the time the last span is reached.
            crc.update(span);
            if at + take == CHECKPOINT_BODY_LEN {
                sealed = crc.finalize();
                super::raw::put_u32(span, CHECKPOINT_BODY_CRC_OFFSET - at, sealed);
                write(at, span)?;
                break;
            }
            write(at, span)?;
            at += take;
        }
        Ok(sealed)
    }

    /// Reconstructs a projection from a validated checkpoint body, into a buffer the caller owns.
    ///
    /// The in-place form is the one the device can use: see [`empty`](Self::empty) for why nothing
    /// here returns this value through a return slot.
    /// Every field is assigned rather than the whole value replaced. `*self = CatalogModel { .. }`
    /// reads as an in-place write and is not: it builds the 56 KiB value in a temporary first, which
    /// on the board put **58,112 bytes** on the frame of the mount that called it. Measured, then
    /// removed — which is the whole point of this function existing beside
    /// [`decode_body`](Self::decode_body).
    pub fn decode_body_into(&mut self, body: &[u8]) -> Result<()> {
        let header = checkpoint::validate_body(body)?;
        let model = self;
        model.store = header.store;
        model.epoch = header.epoch;
        model.through_sequence = header.through_sequence;
        model.next_generation = header.next_generation;
        model.terminal_counter = header.terminal_counter;
        model.flags = header.flags;
        model.result_start = header.result_start as usize;
        model.repositories.clear();
        model.heads.clear();
        model.actives.clear();
        model.draft_parent = None;
        model.draft_parts.clear();
        model.retained.clear();
        model.results.clear();
        model.handoff = None;
        model.weather = None;
        model.ride = None;
        for index in 0..header.repository_count as usize {
            let _ = model.repositories.push(RepositoryState::decode(&body[checkpoint::REPOSITORIES.slot(index)])?);
        }
        for index in 0..header.head_count as usize {
            let _ = model.heads.push(CatalogHead::decode(&body[checkpoint::HEADS.slot(index)])?);
        }
        for index in 0..header.active_count as usize {
            let _ = model.actives.push(ActiveOperation::decode(&body[checkpoint::ACTIVE.slot(index)])?);
        }
        if header.draft_parent_count == 1 {
            model.draft_parent = Some(DraftParent::decode(&body[checkpoint::DRAFT_PARENT.slot(0)])?);
        }
        for index in 0..header.draft_part_count as usize {
            let _ = model.draft_parts.push(DraftPart::decode(&body[checkpoint::DRAFT_PARTS.slot(index)])?);
        }
        for index in 0..header.retained_count as usize {
            let _ = model.retained.push(RetainedPrevious::decode(&body[checkpoint::RETAINED.slot(index)])?);
        }
        for step in 0..header.result_count as usize {
            let index = (header.result_start as usize + step) % MAX_TERMINAL_RESULTS;
            let _ = model.results.push(TerminalResult::decode(&body[checkpoint::RESULTS.slot(index)])?);
        }
        if header.handoff_count == 1 {
            model.handoff = Some(HandoffRef::decode(&body[checkpoint::HANDOFF.slot(0)])?);
        }
        if header.weather_count == 1 {
            model.weather = Some(WeatherState::decode(&body[checkpoint::WEATHER.slot(0)])?);
        }
        if header.ride_count == 1 {
            model.ride = Some(ActiveRide::decode(&body[checkpoint::RIDE.slot(0)])?);
        }
        Ok(())
    }

    /// The same reconstruction, boxed. Host-only for the reason [`empty`](Self::empty) gives.
    #[cfg(any(test, feature = "std"))]
    pub fn decode_body(body: &[u8]) -> Result<std::boxed::Box<Self>> {
        let mut model = std::boxed::Box::new(CatalogModel::empty(obc_link::ids::StoreId::ZERO));
        model.decode_body_into(body)?;
        Ok(model)
    }
}

/// §12's first header: epoch one, nothing absorbed, nothing claimed.
fn initial_header(store: StoreId) -> CheckpointHeader {
    CheckpointHeader {
        store,
        epoch: 1,
        through_sequence: 0,
        next_generation: 0,
        repository_count: 1,
        head_count: 0,
        active_count: 0,
        draft_parent_count: 0,
        draft_part_count: 0,
        retained_count: 0,
        result_start: 0,
        result_count: 0,
        handoff_count: 0,
        flags: 0,
        terminal_counter: 0,
        weather_count: 0,
        ride_count: 0,
    }
}

/// The one row §12's first checkpoint carries: the weather repository, with logical ID zero reserved
/// by starting its candidate at one.
fn initial_repository(weather_kind: u16) -> RepositoryState {
    RepositoryState {
        kind: weather_kind,
        flags: 0,
        revision: obc_link::ids::Revision::ZERO,
        next_logical_id: obc_link::ids::LogicalObjectId::new(1),
    }
}

fn write_region<T, const N: usize, const L: usize>(
    out: &mut [u8],
    region: Region,
    rows: &Vec<T, N>,
    encode: impl Fn(&T) -> [u8; L],
) {
    for (index, row) in rows.iter().enumerate() {
        put_bytes(out, region.slot(index).start, &encode(row));
    }
}

/// Inserts at `position`, or appends when it is `None`.
///
/// Infallible: `check` proved the room before `commit` ran. A full vector here would be a bug in
/// that proof rather than a state a record can reach, so the push simply cannot fail — and if it
/// somehow did, dropping the row silently is still better than a panic on the replay path.
fn insert<T, const N: usize>(vec: &mut Vec<T, N>, position: Option<usize>, value: T) {
    debug_assert!(vec.len() < N, "capacity is proved by CatalogModel::check");
    if vec.push(value).is_err() {
        return;
    }
    let last = vec.len() - 1;
    let index = position.unwrap_or(last);
    // `heapless::Vec` has no `insert`; rotating the freshly pushed element into place is the same
    // thing and stays allocation-free.
    vec[index..=last].rotate_right(1);
}

fn remove<T: Copy, const N: usize>(vec: &mut Vec<T, N>, index: usize) {
    let last = vec.len() - 1;
    vec[index..=last].rotate_left(1);
    let _ = vec.pop();
}

fn retain_parts<const N: usize>(parts: &mut Vec<DraftPart, N>, parent: OperationId) {
    let mut index = 0;
    while index < parts.len() {
        if parts[index].key.parent == parent {
            remove(parts, index);
        } else {
            index += 1;
        }
    }
}

/// Replays a contiguous suffix onto a projection, stopping at the first record that does not apply.
///
/// Returns how many records were absorbed. §6.3's *selection* of that suffix — which records are
/// valid, and whether stopping early is a fault — belongs to [`super::recovery`]; this is only the
/// application of an already-chosen sequence.
pub fn replay<'a, H: HeadRow, R: ResultRow>(
    model: &mut Projection<H, R>,
    records: impl IntoIterator<Item = &'a JournalBody>,
) -> core::result::Result<usize, ApplyError> {
    let mut applied = 0;
    for record in records {
        model.apply(record)?;
        applied += 1;
    }
    Ok(applied)
}

#[cfg(test)]
mod tests {
    use super::super::samples;
    use super::*;
    use std::boxed::Box;

    fn model() -> Box<CatalogModel> {
        CatalogModel::initial(samples::STORE, 4)
    }

    fn body_buffer() -> Box<[u8; CHECKPOINT_BODY_LEN]> {
        Box::new([0u8; CHECKPOINT_BODY_LEN])
    }

    /// **The compile-time half of `init_empty`'s safety argument.**
    ///
    /// `init_empty` writes a `MaybeUninit<CatalogModel>` field by field, so its soundness is exactly
    /// the claim that the list is complete. This destructures the struct with no `..`, which means a
    /// field added to `CatalogModel` stops this file compiling until someone looks at it — and the
    /// only reason to look is the raw-pointer list above. The body then asserts the values that list
    /// writes, so the check is not merely structural.
    #[test]
    fn every_field_is_named_by_the_in_place_constructor() {
        let mut slot = Box::new(core::mem::MaybeUninit::<CatalogModel>::uninit());
        let placed = CatalogModel::init_empty(&mut slot, samples::STORE);
        let CatalogModel {
            store,
            epoch,
            through_sequence,
            next_generation,
            terminal_counter,
            flags,
            repositories,
            heads,
            actives,
            draft_parent,
            draft_parts,
            retained,
            result_start,
            results,
            handoff,
            weather,
            ride,
        } = placed;
        assert_eq!(*store, samples::STORE);
        assert_eq!((*epoch, *through_sequence, *next_generation, *terminal_counter, *flags), (1, 0, 0, 0, 0));
        assert!(repositories.is_empty() && heads.is_empty() && actives.is_empty());
        assert!(draft_parts.is_empty() && retained.is_empty() && results.is_empty());
        assert_eq!(*result_start, 0);
        assert!(draft_parent.is_none() && handoff.is_none() && weather.is_none() && ride.is_none());
    }

    /// `init_empty` and `empty` are two spellings of one value, and the board runs the first while
    /// every host test runs the second.
    #[test]
    fn the_in_place_constructor_agrees_with_the_by_value_one() {
        let mut slot = Box::new(core::mem::MaybeUninit::<CatalogModel>::uninit());
        let placed = CatalogModel::init_empty(&mut slot, samples::STORE);
        assert_eq!(*placed, CatalogModel::empty(samples::STORE));
    }

    /// [`CatalogModel::encode_initial_body`] writes §12's first checkpoint without building a
    /// projection, which is a second definition of the same bytes. This is what stops the two from
    /// drifting: the shortcut and the long way round must produce the identical 65,024 bytes, CRC
    /// included, for every kind the weather repository could be registered under.
    #[test]
    fn the_initial_body_shortcut_encodes_exactly_what_an_initial_projection_does() {
        for kind in [0u16, 4, 1_000, u16::MAX] {
            let mut long_way = body_buffer();
            CatalogModel::initial(samples::STORE, kind).encode_body(long_way.as_mut_slice()).unwrap();
            let mut shortcut = body_buffer();
            CatalogModel::encode_initial_body(shortcut.as_mut_slice(), samples::STORE, kind).unwrap();
            assert_eq!(long_way.as_slice(), shortcut.as_slice(), "kind {kind}");
        }
        // And what it wrote is a valid checkpoint body that decodes back to that same projection.
        let mut bytes = body_buffer();
        CatalogModel::encode_initial_body(bytes.as_mut_slice(), samples::STORE, 4).unwrap();
        assert_eq!(CatalogModel::decode_body(bytes.as_slice()).unwrap(), CatalogModel::initial(samples::STORE, 4));
    }

    /// A decode reuses the projection it is given rather than replacing it, so anything the previous
    /// tenant left behind has to be gone — a stale head or a stale draft parent would otherwise
    /// survive a remount into the next store's catalog.
    #[test]
    fn decoding_into_a_populated_projection_leaves_nothing_of_the_old_one() {
        // **Every** region, not a representative few: a decode clears field by field now, so a
        // region left out of that list is a region whose stale contents survive into the next
        // store's catalog — and a test that populated only some of them would not see it.
        let mut populated = model();
        populated.apply(&samples::claim(1, 1, 0, samples::OP_A, 1)).unwrap();
        populated.apply(&samples::publish(1, 2, 1, samples::OP_A, 1, samples::head(1, 7))).unwrap();
        populated.epoch = 9;
        populated.through_sequence = 41;
        populated.next_generation = 12;
        populated.terminal_counter = 5;
        populated.flags = 1;
        populated.result_start = 3;
        let _ = populated.repositories.push(samples::repository(11, 4));
        let _ = populated.heads.push(samples::head(2, 8));
        let _ = populated.actives.push(samples::active(samples::OP_PARENT));
        populated.draft_parent = Some(samples::parent());
        let _ = populated.draft_parts.push(samples::part(1));
        let _ = populated.retained.push(samples::retained(3));
        let _ = populated.results.push(samples::result(2, samples::OP_A));
        populated.handoff = Some(samples::handoff_ref(1, super::super::handoff::HandoffPhase::Armed));
        populated.weather = Some(samples::weather());
        populated.ride = Some(samples::ride());
        // Bind the check to the struct: a region added to `CatalogModel` without a line above
        // leaves this assertion passing vacuously, so the count is asserted rather than assumed.
        assert!(
            !populated.repositories.is_empty()
                && !populated.heads.is_empty()
                && !populated.actives.is_empty()
                && populated.draft_parent.is_some()
                && !populated.draft_parts.is_empty()
                && !populated.retained.is_empty()
                && !populated.results.is_empty()
                && populated.handoff.is_some()
                && populated.weather.is_some()
                && populated.ride.is_some()
        );

        let mut fresh = body_buffer();
        CatalogModel::encode_initial_body(fresh.as_mut_slice(), samples::STORE, 4).unwrap();
        populated.decode_body_into(fresh.as_slice()).unwrap();
        assert_eq!(*populated, *CatalogModel::initial(samples::STORE, 4));
    }

    #[test]
    fn a_claim_then_a_publication_moves_the_projection() {
        let mut model = model();
        model.apply(&samples::claim(1, 1, 0, samples::OP_A, 1)).unwrap();
        assert_eq!(model.actives.len(), 1);
        assert_eq!(model.next_generation, 1);

        model.apply(&samples::publish(1, 2, 1, samples::OP_A, 1, samples::head(1, 7))).unwrap();
        assert!(model.actives.is_empty());
        assert_eq!(model.heads.len(), 1);
        assert_eq!(model.results.len(), 1);
        assert_eq!(model.terminal_counter, 1);
        assert_eq!(model.through_sequence, 2);
    }

    #[test]
    fn a_body_round_trips_through_the_model() {
        let mut model = model();
        model.apply(&samples::claim(1, 1, 0, samples::OP_A, 1)).unwrap();
        model.apply(&samples::publish(1, 2, 1, samples::OP_A, 1, samples::head(1, 7))).unwrap();

        let mut body = body_buffer();
        model.encode_body(body.as_mut_slice()).unwrap();
        checkpoint::validate_body(body.as_slice()).unwrap();
        assert_eq!(CatalogModel::decode_body(body.as_slice()).unwrap(), model);
    }

    /// Heads arrive in whatever order operations commit, and the checkpoint region requires them
    /// sorted by `(kind, logical id)`. Applying four out-of-order publications must therefore
    /// produce a body that `validate_body` accepts.
    #[test]
    fn heads_stay_sorted_by_kind_then_logical_id() {
        let mut model = model();
        let mut sequence = 0;
        for (index, (kind, id)) in [(2u16, 5u64), (1, 9), (1, 3), (2, 1)].into_iter().enumerate() {
            let mut operation = samples::OP_A;
            operation[0] = index as u8;
            sequence += 1;
            model.apply(&samples::claim(1, sequence, 0, operation, model.next_generation + 1)).unwrap();
            sequence += 1;
            model
                .apply(&samples::publish(1, sequence, 1, operation, index as u64 + 1, samples::head(kind, id)))
                .unwrap();
        }
        let keys: std::vec::Vec<_> = model.heads.iter().map(|head| (head.key.kind, head.key.id.get())).collect();
        assert_eq!(keys, std::vec![(1, 3), (1, 9), (2, 1), (2, 5)]);

        let mut body = body_buffer();
        model.encode_body(body.as_mut_slice()).unwrap();
        checkpoint::validate_body(body.as_slice()).unwrap();
    }

    #[test]
    fn the_result_ring_evicts_the_oldest_at_sixty_five() {
        let mut model = model();
        for index in 1..=MAX_TERMINAL_RESULTS as u64 + 3 {
            let mut operation = samples::OP_A;
            operation[0] = index as u8;
            model.apply(&samples::claim(1, index * 2 - 1, 0, operation, model.next_generation + 1)).unwrap();
            model.apply(&samples::publish(1, index * 2, 1, operation, index, samples::head(1, index))).unwrap();
        }
        assert_eq!(model.results.len(), MAX_TERMINAL_RESULTS);
        // The oldest three are gone and the newest is the last commit.
        assert_eq!(model.results[0].commit_sequence, 4);
        assert_eq!(model.results[MAX_TERMINAL_RESULTS - 1].commit_sequence, MAX_TERMINAL_RESULTS as u64 + 3);
        assert_eq!(model.result_start, 3);

        let mut body = body_buffer();
        model.encode_body(body.as_mut_slice()).unwrap();
        assert_eq!(CatalogModel::decode_body(body.as_slice()).unwrap(), model);
    }

    #[test]
    fn a_noncontiguous_or_foreign_record_does_not_apply() {
        let mut model = model();
        let mut record = samples::claim(1, 2, 1, samples::OP_A, 1);
        assert_eq!(model.apply(&record), Err(ApplyError::Sequence));
        record.sequence = 1;
        record.epoch = 2;
        assert_eq!(model.apply(&record), Err(ApplyError::Epoch));
        record.epoch = 1;
        record.store = obc_link::ids::StoreId::new([0x11; 16]);
        assert_eq!(model.apply(&record), Err(ApplyError::StoreId));
    }

    #[test]
    fn the_generation_cursor_advances_by_exactly_one() {
        let mut model = model();
        let record = samples::claim(1, 1, 0, samples::OP_A, 2);
        assert_eq!(model.apply(&record), Err(ApplyError::GenerationCursor));
        model.apply(&samples::claim(1, 1, 0, samples::OP_A, 1)).unwrap();
        assert_eq!(model.next_generation, 1);
    }

    #[test]
    fn removing_a_draft_parent_removes_its_parts_in_the_same_step() {
        let mut model = model();
        model.draft_parent = Some(samples::parent());
        for key in 1..=3u64 {
            let row = samples::part(key);
            let position = model.draft_parts.iter().position(|held| held.key.sort_key() > row.key.sort_key());
            insert(&mut model.draft_parts, position, row);
        }
        let record = super::super::journal::JournalBody {
            store: samples::STORE,
            epoch: 1,
            sequence: 1,
            slot: 0,
            kind: RecordKind::Terminal,
            operation: obc_link::ids::OperationId::new(samples::OP_PARENT),
            intent: samples::INTENT,
            mutation: super::super::journal::Mutation {
                active: Some(Change::Remove(obc_link::ids::OperationId::new(samples::OP_PARENT))),
                draft_parent: Some(Change::Remove(obc_link::ids::OperationId::new(samples::OP_PARENT))),
                result: Some(samples::result(1, samples::OP_PARENT)),
                ..Default::default()
            },
        };
        // The parent's own active row must exist for the removal to apply.
        model
            .apply(&super::super::journal::JournalBody {
                sequence: 1,
                kind: RecordKind::Claim,
                mutation: super::super::journal::Mutation {
                    active: Some(Change::Put(samples::active(samples::OP_PARENT))),
                    ..Default::default()
                },
                ..record
            })
            .unwrap();
        let mut record = record;
        record.sequence = 2;
        model.apply(&record).unwrap();
        assert!(model.draft_parent.is_none());
        assert!(model.draft_parts.is_empty());
    }

    /// A full region is a refusal, not a truncation — and the refusal has to leave the projection
    /// exactly as it was, which is the property `check`-then-`commit` exists for.
    #[test]
    fn a_full_region_reports_a_resource_limit_rather_than_growing() {
        let mut model = model();
        for index in 0..MAX_CATALOG_HEADS {
            let row = samples::head(1, index as u64);
            let position = model.heads.iter().position(|held| held.key > row.key);
            insert(&mut model.heads, position, row);
        }
        let before = model.clone();
        let record = samples::publish(1, 1, 0, samples::OP_A, 1, samples::head(1, MAX_CATALOG_HEADS as u64));
        // The active row the terminal record removes has to exist, or that check fires first.
        let mut model = model;
        model.actives.push(samples::active(samples::OP_A)).unwrap();
        let mut before = before;
        before.actives.push(samples::active(samples::OP_A)).unwrap();

        assert_eq!(model.apply(&record), Err(ApplyError::ResourceLimit(Record::CatalogHead)));
        assert_eq!(model.as_ref(), before.as_ref(), "a refused record moved the projection");
    }

    /// MAJ-2's regression: every refusal path must leave the projection untouched, including the
    /// ones that used to run after the terminal counter had already been incremented.
    #[test]
    fn every_refusal_leaves_the_projection_byte_identical() {
        let mut model = model();
        model.apply(&samples::claim(1, 1, 0, samples::OP_A, 1)).unwrap();
        let before = model.clone();

        // A result whose commit sequence is not the incremented counter.
        let mut record = samples::publish(1, 2, 1, samples::OP_A, 9, samples::head(1, 7));
        assert_eq!(model.apply(&record), Err(ApplyError::TerminalCounter));
        assert_eq!(model, before);

        // A head removal naming a head that does not exist.
        record = samples::publish(1, 2, 1, samples::OP_A, 2, samples::head(1, 7));
        record.mutation.head = Some(Change::Remove(HeadKey { kind: 1, id: obc_link::ids::LogicalObjectId::new(99) }));
        assert_eq!(model.apply(&record), Err(ApplyError::MissingKey(Record::CatalogHead)));
        assert_eq!(model, before);

        // A retention removal naming a generation no entry holds.
        let retention = samples::retention_remove(1, 2, 1, 40);
        assert_eq!(model.apply(&retention), Err(ApplyError::MissingKey(Record::RetainedPrevious)));
        assert_eq!(model, before);

        // A ride removal with no ride state.
        let mut ride_removal = samples::retention_remove(1, 2, 1, 40);
        ride_removal.kind = RecordKind::Domain;
        ride_removal.mutation =
            super::super::journal::Mutation { ride: Some(Change::Remove(())), ..Default::default() };
        assert_eq!(model.apply(&ride_removal), Err(ApplyError::MissingKey(Record::ActiveRide)));
        assert_eq!(model, before);
    }

    /// MAJ-3: the one draft-parent row is a capacity, not a slot to overwrite. A second parent
    /// would strand the first one's parts, which is exactly what §5.1's single row forbids.
    #[test]
    fn a_second_draft_parent_is_refused_rather_than_replacing_the_open_one() {
        let mut model = model();
        model.draft_parent = Some(samples::parent());
        let before = model.clone();

        let mut other = samples::parent();
        other.parent = obc_link::ids::OperationId::new(samples::OP_B);
        let mut record = samples::claim(1, 1, 0, samples::OP_B, 1);
        record.mutation.draft_parent = Some(Change::Put(other));
        assert_eq!(model.apply(&record), Err(ApplyError::ResourceLimit(Record::DraftParent)));
        assert_eq!(model, before);

        // The same parent is an ordinary state transition and applies.
        let mut same = samples::parent();
        same.state = super::super::entries::DraftParentState::ManifestStreaming;
        let mut record = samples::claim(1, 1, 0, samples::OP_PARENT, 1);
        record.mutation.draft_parent = Some(Change::Put(same));
        model.apply(&record).unwrap();
        assert_eq!(model.draft_parent.unwrap().state, super::super::entries::DraftParentState::ManifestStreaming);
    }

    /// MAJ-3: a part row names its parent, so one naming a parent this projection does not hold is
    /// not a membership fact it can absorb.
    #[test]
    fn a_part_naming_a_foreign_parent_is_refused() {
        let mut model = model();
        model.draft_parent = Some(samples::parent());
        let before = model.clone();

        let mut foreign = samples::part(1);
        foreign.key.parent = obc_link::ids::OperationId::new(samples::OP_B);
        let mut record = samples::claim(1, 1, 0, samples::OP_PARENT, 1);
        record.mutation.draft_part = Some(Change::Put(foreign));
        assert_eq!(model.apply(&record), Err(ApplyError::MissingKey(Record::DraftParent)));
        assert_eq!(model, before);
    }

    /// §6.1 bit 18 names the entry that carries the reserved generation, and the value it carries
    /// is the *former* cursor. A carrier holding anything else is not a reservation.
    #[test]
    fn the_reserved_generation_must_appear_in_the_named_carrier() {
        let mut model = model();
        let mut record = samples::claim(1, 1, 0, samples::OP_A, 1);
        // The claim's active row carries generation 0, the former cursor. Move it and the record
        // stops being a reservation.
        if let Some(Change::Put(row)) = &mut record.mutation.active {
            row.generation = GenerationId::new(7);
        }
        assert_eq!(model.apply(&record), Err(ApplyError::GenerationCursor));

        // Clearing flag bit 4 has the same effect: the row no longer claims to hold one.
        let mut record = samples::claim(1, 1, 0, samples::OP_A, 1);
        if let Some(Change::Put(row)) = &mut record.mutation.active {
            row.flags &= !ActiveOperation::FLAG_GENERATION_RESERVED;
        }
        assert_eq!(model.apply(&record), Err(ApplyError::GenerationCursor));
    }

    /// The three saturating points in `check`, each pinned at its own boundary.
    ///
    /// All three are `checked_add`s that can only fire at `u64::MAX`, which no test reaches by
    /// counting — so each is driven by placing the projection one short of the ceiling. Without
    /// these the checks are unexecuted lines that a refactor could delete unnoticed.
    #[test]
    fn every_checked_add_refuses_rather_than_wrapping() {
        // 1. The sequence successor. A projection at `u64::MAX` has no next sequence at all, so no
        //    record can be its successor — not even one claiming `u64::MAX`.
        let mut projection = model();
        projection.through_sequence = u64::MAX;
        let mut record = samples::claim(1, u64::MAX, 0, samples::OP_A, 1);
        assert_eq!(projection.apply(&record), Err(ApplyError::Sequence));
        record.sequence = 0;
        assert_eq!(projection.apply(&record), Err(ApplyError::Sequence));

        // 2. The generation cursor. At `u64::MAX` the reservation cannot advance, and the record
        //    that tries is refused rather than wrapping to zero.
        let mut projection = model();
        projection.next_generation = u64::MAX;
        let mut record = samples::claim(1, 1, 0, samples::OP_A, 0);
        if let Some(Change::Put(row)) = &mut record.mutation.active {
            row.generation = GenerationId::new(u64::MAX);
        }
        assert_eq!(projection.apply(&record), Err(ApplyError::GenerationCursor));

        // 3. The terminal-commit counter. §5.3 makes a result's sequence the counter *after*
        //    increment, so a counter at `u64::MAX` can produce no further result.
        let mut projection = model();
        projection.terminal_counter = u64::MAX;
        projection.actives.push(samples::active(samples::OP_A)).unwrap();
        let record = samples::publish(1, 1, 0, samples::OP_A, 0, samples::head(1, 7));
        assert_eq!(projection.apply(&record), Err(ApplyError::TerminalCounter));
    }

    /// The replay helper is the loop recovery runs; it stops at the first record that does not
    /// apply and reports how many it absorbed.
    #[test]
    fn replay_absorbs_a_contiguous_prefix_and_stops_at_the_first_refusal() {
        let mut model = model();
        let records = [
            samples::claim(1, 1, 0, samples::OP_A, 1),
            samples::publish(1, 2, 1, samples::OP_A, 1, samples::head(1, 7)),
            // Sequence 4 is not the successor of 2.
            samples::claim(1, 4, 3, samples::OP_B, 2),
        ];
        assert_eq!(replay(&mut model, records.iter()), Err(ApplyError::Sequence));
        assert_eq!(model.through_sequence, 2);
        assert_eq!(replay(&mut model, records[..0].iter()), Ok(0));
    }
}
