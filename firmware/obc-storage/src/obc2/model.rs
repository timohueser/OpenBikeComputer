//! The in-memory catalog projection, and the apply-mutation semantics that move it
//! (`OBC2_Storage_Format.md` §5, §6).
//!
//! This is the **reference model**: one bounded value holding every region a checkpoint body holds,
//! with a total `apply` that is the meaning of a journal record. Two things are built on it. Replay
//! is literally `for record in suffix { model.apply(record) }`, and compaction is
//! [`CatalogModel::encode_body`], so a checkpoint is by construction the projection its records
//! produce. The crash harness then uses it as the oracle: recovery must land on exactly the model's
//! before-state or its after-state, never anything else.
//!
//! It is deliberately **not** the device's resident state. §13 fixes that as a bounded *index* —
//! about 19.25 KiB, with envelopes and resolution generations left on the card and re-read on
//! demand — while this value holds whole entries because a host oracle has no reason not to. The
//! device's index and the compaction pass that materializes a body without holding one are a later
//! slice; nothing here is instantiated by the device image.
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
use super::journal::{Change, JournalBody};
use super::limits::{
    CHECKPOINT_BODY_LEN, MAX_ACTIVE_OPERATIONS, MAX_CATALOG_HEADS, MAX_DRAFT_PARTS, MAX_REPOSITORY_STATES,
    MAX_RETAINED_PREVIOUS, MAX_TERMINAL_RESULTS,
};
use super::raw::put_bytes;

/// The bounded catalog projection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogModel {
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
    pub heads: Vec<CatalogHead, MAX_CATALOG_HEADS>,
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
    pub results: Vec<TerminalResult, MAX_TERMINAL_RESULTS>,
    /// The one update-handoff projection.
    pub handoff: Option<HandoffRef>,
    /// The one weather-request state.
    pub weather: Option<WeatherState>,
    /// The one active-ride state.
    pub ride: Option<ActiveRide>,
}

impl CatalogModel {
    /// The first checkpoint of a freshly initialized store (§12): epoch 1, through-sequence 0, next
    /// `GenerationId` 0, terminal counter 0, and weather logical ID zero reserved by setting the
    /// weather repository's next candidate to one while leaving the weather state absent.
    pub fn initial(store: StoreId, weather_kind: u16) -> Self {
        let mut model = CatalogModel {
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
        };
        let _ = model.repositories.push(RepositoryState {
            kind: weather_kind,
            flags: 0,
            revision: obc_link::ids::Revision::ZERO,
            next_logical_id: obc_link::ids::LogicalObjectId::new(1),
        });
        model
    }

    /// The head a `(kind, logical id)` names.
    pub fn head(&self, key: HeadKey) -> Option<&CatalogHead> {
        self.heads.iter().find(|head| head.key == key)
    }

    /// The retained entry a generation names.
    pub fn retained_entry(&self, generation: GenerationId) -> Option<&RetainedPrevious> {
        self.retained.iter().find(|entry| entry.generation == generation)
    }

    /// The retained terminal result for an `OperationId`, if it is still inside the 64-entry window.
    ///
    /// §2: after eviction `QueryOperation` returns `Unknown`, "which is an indeterminate old
    /// outcome, not permission to retry that identity".
    pub fn result_for(&self, operation: OperationId) -> Option<&TerminalResult> {
        self.results.iter().find(|result| result.operation == operation)
    }

    /// Applies one decoded journal record.
    ///
    /// The record must be the contiguous successor of what this projection has absorbed: same
    /// store, same epoch, sequence exactly `through_sequence + 1` (§6.3). Everything past those
    /// three checks is the mutation's own semantics.
    pub fn apply(&mut self, record: &JournalBody) -> core::result::Result<(), ApplyError> {
        if record.store != self.store {
            return Err(ApplyError::StoreId);
        }
        if record.epoch != self.epoch {
            return Err(ApplyError::Epoch);
        }
        if record.sequence != self.through_sequence + 1 {
            return Err(ApplyError::Sequence);
        }
        let mutation = &record.mutation;

        // §6.1 bit 18: "the encoded cursor must be the current cursor plus one without wrap. The
        // record reserves the former cursor value as its GenerationId."
        if let Some(cursor) = mutation.generation_cursor {
            if cursor != self.next_generation.checked_add(1).ok_or(ApplyError::GenerationCursor)? {
                return Err(ApplyError::GenerationCursor);
            }
            self.next_generation = cursor;
        }

        if let Some(repository) = &mutation.repository {
            let row = match self.repositories.iter_mut().find(|row| row.kind == repository.kind) {
                Some(row) => row,
                None => {
                    let fresh = RepositoryState {
                        kind: repository.kind,
                        flags: 0,
                        revision: obc_link::ids::Revision::ZERO,
                        next_logical_id: obc_link::ids::LogicalObjectId::ZERO,
                    };
                    let position = self.repositories.iter().position(|row| row.kind > repository.kind);
                    insert(&mut self.repositories, position, fresh, Record::RepositoryState)?;
                    self.repositories.iter_mut().find(|row| row.kind == repository.kind).expect("just inserted")
                }
            };
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
                        insert(&mut self.actives, position, *row, Record::ActiveOperation)?;
                    }
                }
            }
            Some(Change::Remove(key)) => {
                let index = self
                    .actives
                    .iter()
                    .position(|held| held.operation == *key)
                    .ok_or(ApplyError::MissingKey(Record::ActiveOperation))?;
                remove(&mut self.actives, index);
            }
            None => {}
        }

        match &mutation.head {
            Some(Change::Put(row)) => match self.heads.iter().position(|held| held.key == row.key) {
                Some(index) => self.heads[index] = *row,
                None => {
                    let position = self.heads.iter().position(|held| held.key > row.key);
                    insert(&mut self.heads, position, *row, Record::CatalogHead)?;
                }
            },
            Some(Change::Remove(key)) => {
                let index = self
                    .heads
                    .iter()
                    .position(|held| held.key == *key)
                    .ok_or(ApplyError::MissingKey(Record::CatalogHead))?;
                remove(&mut self.heads, index);
            }
            None => {}
        }

        match &mutation.draft_parent {
            Some(Change::Put(row)) => self.draft_parent = Some(*row),
            Some(Change::Remove(key)) => {
                match self.draft_parent {
                    Some(parent) if parent.parent == *key => self.draft_parent = None,
                    _ => return Err(ApplyError::MissingKey(Record::DraftParent)),
                }
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
                    insert(&mut self.draft_parts, position, *row, Record::DraftPart)?;
                }
            },
            Some(Change::Remove(key)) => {
                let index = self
                    .draft_parts
                    .iter()
                    .position(|held| held.key == *key)
                    .ok_or(ApplyError::MissingKey(Record::DraftPart))?;
                remove(&mut self.draft_parts, index);
            }
            None => {}
        }

        match &mutation.retained {
            Some(Change::Put(row)) => match self.retained.iter().position(|held| held.generation == row.generation) {
                Some(index) => self.retained[index] = *row,
                None => {
                    let position = self.retained.iter().position(|held| held.generation > row.generation);
                    insert(&mut self.retained, position, *row, Record::RetainedPrevious)?;
                }
            },
            Some(Change::Remove(key)) => {
                let index = self
                    .retained
                    .iter()
                    .position(|held| held.generation == *key)
                    .ok_or(ApplyError::MissingKey(Record::RetainedPrevious))?;
                remove(&mut self.retained, index);
            }
            None => {}
        }

        if let Some(result) = &mutation.result {
            // §5.3: "`terminal commit sequence` is the checkpoint's terminal-commit counter after
            // increment", so the counter and the appended entry move together or not at all.
            self.terminal_counter += 1;
            if result.commit_sequence != self.terminal_counter {
                return Err(ApplyError::TerminalCounter);
            }
            if self.results.len() == MAX_TERMINAL_RESULTS {
                // "Ring append writes `(result_start + result_count) mod 64`; when already full it
                // overwrites `result_start` and advances that index by one. This is the only
                // eviction path."
                self.results[0] = *result;
                self.results.rotate_left(1);
                self.result_start = (self.result_start + 1) % MAX_TERMINAL_RESULTS;
            } else {
                let _ = self.results.push(*result);
            }
        }

        match &mutation.handoff {
            Some(Change::Put(row)) => self.handoff = Some(*row),
            Some(Change::Remove(())) if self.handoff.take().is_none() => {
                return Err(ApplyError::MissingKey(Record::HandoffRef))
            }
            Some(Change::Remove(())) => {}
            None => {}
        }

        if let Some(weather) = &mutation.weather {
            self.weather = Some(*weather);
        }

        match &mutation.ride {
            Some(Change::Put(row)) => self.ride = Some(*row),
            Some(Change::Remove(())) if self.ride.take().is_none() => {
                return Err(ApplyError::MissingKey(Record::ActiveRide))
            }
            Some(Change::Remove(())) => {}
            None => {}
        }

        self.through_sequence = record.sequence;
        Ok(())
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

    /// Reconstructs a projection from a validated checkpoint body.
    pub fn decode_body(body: &[u8]) -> Result<Self> {
        let header = checkpoint::validate_body(body)?;
        let mut model = CatalogModel {
            store: header.store,
            epoch: header.epoch,
            through_sequence: header.through_sequence,
            next_generation: header.next_generation,
            terminal_counter: header.terminal_counter,
            flags: header.flags,
            repositories: Vec::new(),
            heads: Vec::new(),
            actives: Vec::new(),
            draft_parent: None,
            draft_parts: Vec::new(),
            retained: Vec::new(),
            result_start: header.result_start as usize,
            results: Vec::new(),
            handoff: None,
            weather: None,
            ride: None,
        };
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
        Ok(model)
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

fn insert<T, const N: usize>(
    vec: &mut Vec<T, N>,
    position: Option<usize>,
    value: T,
    record: Record,
) -> core::result::Result<(), ApplyError> {
    if vec.len() == N {
        return Err(ApplyError::ResourceLimit(record));
    }
    let index = position.unwrap_or(vec.len());
    let _ = vec.push(value);
    let last = vec.len() - 1;
    // `heapless::Vec` has no `insert`; rotating the freshly pushed element into place is the same
    // thing and stays allocation-free.
    vec[index..=last].rotate_right(1);
    Ok(())
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
pub fn replay<'a>(
    model: &mut CatalogModel,
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
    use super::super::journal::RecordKind;
    use super::super::samples;
    use super::*;
    use std::boxed::Box;

    fn model() -> CatalogModel {
        CatalogModel::initial(samples::STORE, 4)
    }

    fn body_buffer() -> Box<[u8; CHECKPOINT_BODY_LEN]> {
        Box::new([0u8; CHECKPOINT_BODY_LEN])
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
            insert(&mut model.draft_parts, position, row, Record::DraftPart).unwrap();
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

    #[test]
    fn a_full_region_reports_a_resource_limit_rather_than_growing() {
        let mut model = model();
        for index in 0..MAX_CATALOG_HEADS {
            let row = samples::head(1, index as u64);
            let position = model.heads.iter().position(|held| held.key > row.key);
            insert(&mut model.heads, position, row, Record::CatalogHead).unwrap();
        }
        let row = samples::head(1, MAX_CATALOG_HEADS as u64);
        assert_eq!(
            insert(&mut model.heads, None, row, Record::CatalogHead),
            Err(ApplyError::ResourceLimit(Record::CatalogHead))
        );
    }
}
