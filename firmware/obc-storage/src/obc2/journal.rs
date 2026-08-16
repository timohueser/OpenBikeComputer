//! The commit journal: one slot's 96-byte header, its fixed 1,272-byte mutation, and the record
//! kinds that constrain them (`OBC2_Storage_Format.md` §6).
//!
//! `COMMIT.JNL` is 256 slots of one stride. A slot holds a 1,536-byte body at its base, an `O2JG`
//! gate at base `+ 1,536`, and a zero pad to the next stride, so writing one slot can never damage
//! another. A slot is written once in an epoch, and §6.3's mapping rule ties the two together:
//! within an epoch, physical slot `i` carries sequence `checkpoint through_sequence + i + 1`.
//!
//! The mutation is "a compact projection delta, not a union of domain payloads": nine fixed entry
//! slots, each either absent (all zero), put (a complete entry) or removed (key bytes only), plus
//! the repository and generation cursors. Which combinations are legal is not a policy the engine
//! layers on top — §6.1 fixes them per record kind, and a record outside those combinations is
//! structurally invalid, so [`Mutation::decode`] refuses it here rather than letting replay meet it.

use obc_link::ids::{GenerationId, OperationId, StoreId};

use super::entries::{
    absent, ActiveOperation, ActiveRide, CatalogHead, DraftParent, DraftPart, HeadKey, PartKey, RetainedPrevious,
    TerminalResult, WeatherState,
};
use super::error::{DecodeError, Reason, Record, Result};
use super::gate::{BodyBinding, Gate, MAGIC_JOURNAL};
use super::handoff::HandoffRef;
use super::limits::{JOURNAL_BODY_CRC_OFFSET, JOURNAL_BODY_LEN, JOURNAL_GATE_OFFSET, MUTATION_LEN, SLOT_STRIDE};
use super::raw::{
    bytes16_at, bytes32_at, crc32_with_hole, put_bytes, put_u16, put_u32, put_u64, require_zero, u16_at, u32_at, u64_at,
};

/// Body magic.
pub const MAGIC: [u8; 4] = *b"O2JR";
/// The header length the body declares.
pub const HEADER_LEN: usize = 96;
/// The mutation version.
pub const MUTATION_VERSION: u16 = 1;

/// What a journal record is for (§6.1). The kind is stored twice — in the header and in the
/// mutation — and both copies must agree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordKind {
    /// Claims an `OperationId` and atomically reserves everything preflight accounted for.
    Claim = 1,
    /// Refreshes a claim's durable-progress facts.
    Work = 2,
    /// Removes the active row, appends the result, and may publish.
    Terminal = 3,
    /// Changes exactly one retained-previous entry, under no operation identity.
    Retention = 4,
    /// Changes the one handoff projection.
    Handoff = 5,
    /// Changes the single active-ride recovery state before its publication claim.
    Domain = 6,
}

impl RecordKind {
    fn from_u16(value: u16) -> Option<Self> {
        Some(match value {
            1 => RecordKind::Claim,
            2 => RecordKind::Work,
            3 => RecordKind::Terminal,
            4 => RecordKind::Retention,
            5 => RecordKind::Handoff,
            6 => RecordKind::Domain,
            _ => return None,
        })
    }
}

/// One entry slot's change: a complete entry, or the key bytes of a removal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change<P, K> {
    /// Insert or replace the entry.
    Put(P),
    /// Remove the entry this key names.
    Remove(K),
}

/// The repository cursors a record may advance (§6.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RepositoryChange {
    /// The repository's `ObjectKind`.
    pub kind: u16,
    /// The new revision, when presence bit 13 is set.
    pub revision: Option<u64>,
    /// The new logical-ID candidate and flags, when presence bit 14 is set.
    pub next_logical_id: Option<u64>,
    /// Repository flags; bit 0 is "logical-ID space exhausted".
    pub flags: u16,
}

/// Presence bits, in the order §6.1 lists them.
pub mod presence {
    /// Put the active-operation row.
    pub const ACTIVE_PUT: u32 = 1 << 0;
    /// Remove the active-operation row.
    pub const ACTIVE_REMOVE: u32 = 1 << 1;
    /// Put the catalog head.
    pub const HEAD_PUT: u32 = 1 << 2;
    /// Remove the catalog head.
    pub const HEAD_REMOVE: u32 = 1 << 3;
    /// Put the draft parent.
    pub const PARENT_PUT: u32 = 1 << 4;
    /// Remove the draft parent, and implicitly its parts.
    pub const PARENT_REMOVE: u32 = 1 << 5;
    /// Put the draft part.
    pub const PART_PUT: u32 = 1 << 6;
    /// Remove the draft part.
    pub const PART_REMOVE: u32 = 1 << 7;
    /// Put the retained-previous entry.
    pub const PREVIOUS_PUT: u32 = 1 << 8;
    /// Remove the retained-previous entry.
    pub const PREVIOUS_REMOVE: u32 = 1 << 9;
    /// Append the terminal result.
    pub const RESULT_APPEND: u32 = 1 << 10;
    /// Put the handoff projection.
    pub const HANDOFF_PUT: u32 = 1 << 11;
    /// Remove the handoff projection.
    pub const HANDOFF_REMOVE: u32 = 1 << 12;
    /// Set the repository revision.
    pub const REPOSITORY_REVISION: u32 = 1 << 13;
    /// Set the repository's logical-ID cursor.
    pub const REPOSITORY_CURSOR: u32 = 1 << 14;
    /// Put the weather-request state.
    pub const WEATHER_PUT: u32 = 1 << 15;
    /// Put the active-ride state.
    pub const RIDE_PUT: u32 = 1 << 16;
    /// Remove the active-ride state.
    pub const RIDE_REMOVE: u32 = 1 << 17;
    /// Reserve the next `GenerationId`.
    pub const GENERATION_CURSOR: u32 = 1 << 18;
    /// Every defined bit; 19..31 are zero.
    pub const DEFINED: u32 = (1 << 19) - 1;
}

/// Offsets of the fixed entry slots inside the mutation (§6.1).
mod at {
    pub const ACTIVE: usize = 40;
    pub const HEAD: usize = 168;
    pub const PARENT: usize = 328;
    pub const PART: usize = 456;
    pub const PREVIOUS: usize = 552;
    pub const RESULT: usize = 616;
    pub const HANDOFF: usize = 824;
    pub const WEATHER: usize = 1_064;
    pub const RIDE: usize = 1_144;
}

/// The fixed 1,272-byte projection delta (§6.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Mutation {
    /// The repository cursors, when either repository bit is set.
    pub repository: Option<RepositoryChange>,
    /// The reserved `GenerationId` cursor: the encoded value is the current cursor plus one, and
    /// the record reserves the *former* value.
    pub generation_cursor: Option<u64>,
    /// The active-operation row.
    pub active: Option<Change<ActiveOperation, OperationId>>,
    /// The catalog head.
    pub head: Option<Change<CatalogHead, HeadKey>>,
    /// The draft parent.
    pub draft_parent: Option<Change<DraftParent, OperationId>>,
    /// The draft part.
    pub draft_part: Option<Change<DraftPart, PartKey>>,
    /// The retained-previous entry.
    pub retained: Option<Change<RetainedPrevious, GenerationId>>,
    /// The appended terminal result. Append-only: there is no result removal.
    pub result: Option<TerminalResult>,
    /// The handoff projection; its removal is the whole 240 bytes zero.
    pub handoff: Option<Change<HandoffRef, ()>>,
    /// The weather-request state. Put-only: deleting the head is a put with head-present clear.
    pub weather: Option<WeatherState>,
    /// The active-ride state; its removal carries only the occupied byte.
    pub ride: Option<Change<ActiveRide, ()>>,
}

impl Mutation {
    /// The presence flags this mutation encodes to.
    pub fn presence(&self) -> u32 {
        let mut flags = 0;
        if let Some(repository) = &self.repository {
            if repository.revision.is_some() {
                flags |= presence::REPOSITORY_REVISION;
            }
            if repository.next_logical_id.is_some() {
                flags |= presence::REPOSITORY_CURSOR;
            }
        }
        if self.generation_cursor.is_some() {
            flags |= presence::GENERATION_CURSOR;
        }
        flags |= pair(&self.active, presence::ACTIVE_PUT, presence::ACTIVE_REMOVE);
        flags |= pair(&self.head, presence::HEAD_PUT, presence::HEAD_REMOVE);
        flags |= pair(&self.draft_parent, presence::PARENT_PUT, presence::PARENT_REMOVE);
        flags |= pair(&self.draft_part, presence::PART_PUT, presence::PART_REMOVE);
        flags |= pair(&self.retained, presence::PREVIOUS_PUT, presence::PREVIOUS_REMOVE);
        flags |= pair(&self.handoff, presence::HANDOFF_PUT, presence::HANDOFF_REMOVE);
        flags |= pair(&self.ride, presence::RIDE_PUT, presence::RIDE_REMOVE);
        if self.result.is_some() {
            flags |= presence::RESULT_APPEND;
        }
        if self.weather.is_some() {
            flags |= presence::WEATHER_PUT;
        }
        flags
    }

    /// Encodes the exact 1,272 bytes for a record of `kind`.
    pub fn encode(&self, kind: RecordKind) -> [u8; MUTATION_LEN] {
        let mut out = [0u8; MUTATION_LEN];
        put_u16(&mut out, 0, MUTATION_VERSION);
        put_u32(&mut out, 4, self.presence());
        if let Some(repository) = &self.repository {
            put_u16(&mut out, 8, repository.kind);
            if let Some(revision) = repository.revision {
                put_u64(&mut out, 12, revision);
            }
            if let Some(next) = repository.next_logical_id {
                put_u64(&mut out, 20, next);
                put_u16(&mut out, 28, repository.flags);
            }
        }
        put_u16(&mut out, 10, kind as u16);
        if let Some(cursor) = self.generation_cursor {
            put_u64(&mut out, 32, cursor);
        }
        match &self.active {
            Some(Change::Put(row)) => put_bytes(&mut out, at::ACTIVE, &row.encode()),
            Some(Change::Remove(key)) => put_bytes(&mut out, at::ACTIVE, &ActiveOperation::encode_removal(*key)),
            None => {}
        }
        match &self.head {
            Some(Change::Put(row)) => put_bytes(&mut out, at::HEAD, &row.encode()),
            Some(Change::Remove(key)) => put_bytes(&mut out, at::HEAD, &CatalogHead::encode_removal(*key)),
            None => {}
        }
        match &self.draft_parent {
            Some(Change::Put(row)) => put_bytes(&mut out, at::PARENT, &row.encode()),
            Some(Change::Remove(key)) => put_bytes(&mut out, at::PARENT, &DraftParent::encode_removal(*key)),
            None => {}
        }
        match &self.draft_part {
            Some(Change::Put(row)) => put_bytes(&mut out, at::PART, &row.encode()),
            Some(Change::Remove(key)) => put_bytes(&mut out, at::PART, &DraftPart::encode_removal(*key)),
            None => {}
        }
        match &self.retained {
            Some(Change::Put(row)) => put_bytes(&mut out, at::PREVIOUS, &row.encode()),
            Some(Change::Remove(key)) => put_bytes(&mut out, at::PREVIOUS, &RetainedPrevious::encode_removal(*key)),
            None => {}
        }
        if let Some(result) = &self.result {
            put_bytes(&mut out, at::RESULT, &result.encode());
        }
        match &self.handoff {
            // §6.1: "the singleton removal is all 240 bytes zero".
            Some(Change::Put(row)) => put_bytes(&mut out, at::HANDOFF, &row.encode()),
            Some(Change::Remove(())) | None => {}
        }
        if let Some(weather) = &self.weather {
            put_bytes(&mut out, at::WEATHER, &weather.encode());
        }
        match &self.ride {
            Some(Change::Put(row)) => put_bytes(&mut out, at::RIDE, &row.encode()),
            Some(Change::Remove(())) => put_bytes(&mut out, at::RIDE, &ActiveRide::encode_removal()),
            None => {}
        }
        out
    }

    /// Decodes the 1,272 bytes of a record of `kind`, proving §6.1's combination rules.
    pub fn decode(bytes: &[u8], kind: RecordKind, identified: bool) -> Result<Self> {
        const R: Record = Record::Mutation;
        let err = |reason| DecodeError::new(R, reason);
        if bytes.len() != MUTATION_LEN {
            return Err(err(Reason::Length));
        }
        if u16_at(bytes, 0) != MUTATION_VERSION {
            return Err(err(Reason::Version));
        }
        require_zero(R, bytes, 2, 2)?;
        require_zero(R, bytes, 30, 2)?;
        let flags = u32_at(bytes, 4);
        if flags & !presence::DEFINED != 0 {
            return Err(err(Reason::Reserved));
        }
        if u16_at(bytes, 10) != kind as u16 {
            return Err(err(Reason::Combination));
        }
        // Put and remove for the same entry are mutually exclusive (§6.1).
        for (put, remove) in [
            (presence::ACTIVE_PUT, presence::ACTIVE_REMOVE),
            (presence::HEAD_PUT, presence::HEAD_REMOVE),
            (presence::PARENT_PUT, presence::PARENT_REMOVE),
            (presence::PART_PUT, presence::PART_REMOVE),
            (presence::PREVIOUS_PUT, presence::PREVIOUS_REMOVE),
            (presence::HANDOFF_PUT, presence::HANDOFF_REMOVE),
            (presence::RIDE_PUT, presence::RIDE_REMOVE),
        ] {
            if flags & put != 0 && flags & remove != 0 {
                return Err(err(Reason::Combination));
            }
        }

        let repository = if flags & (presence::REPOSITORY_REVISION | presence::REPOSITORY_CURSOR) != 0 {
            let revision = if flags & presence::REPOSITORY_REVISION != 0 {
                Some(u64_at(bytes, 12))
            } else {
                require_zero(R, bytes, 12, 8)?;
                None
            };
            let next_logical_id = if flags & presence::REPOSITORY_CURSOR != 0 {
                Some(u64_at(bytes, 20))
            } else {
                require_zero(R, bytes, 20, 8)?;
                require_zero(R, bytes, 28, 2)?;
                None
            };
            let flags_field = u16_at(bytes, 28);
            if flags_field & !super::entries::RepositoryState::FLAG_ID_EXHAUSTED != 0 {
                return Err(err(Reason::Reserved));
            }
            Some(RepositoryChange { kind: u16_at(bytes, 8), revision, next_logical_id, flags: flags_field })
        } else {
            require_zero(R, bytes, 8, 2)?;
            require_zero(R, bytes, 12, 18)?;
            None
        };

        let generation_cursor = if flags & presence::GENERATION_CURSOR != 0 {
            Some(u64_at(bytes, 32))
        } else {
            require_zero(R, bytes, 32, 8)?;
            None
        };

        let active = decode_change(
            bytes,
            at::ACTIVE,
            ActiveOperation::LEN,
            flags,
            presence::ACTIVE_PUT,
            presence::ACTIVE_REMOVE,
            ActiveOperation::decode,
            ActiveOperation::decode_removal,
            R,
        )?;
        let head = decode_change(
            bytes,
            at::HEAD,
            CatalogHead::LEN,
            flags,
            presence::HEAD_PUT,
            presence::HEAD_REMOVE,
            CatalogHead::decode,
            CatalogHead::decode_removal,
            R,
        )?;
        let draft_parent = decode_change(
            bytes,
            at::PARENT,
            DraftParent::LEN,
            flags,
            presence::PARENT_PUT,
            presence::PARENT_REMOVE,
            DraftParent::decode,
            DraftParent::decode_removal,
            R,
        )?;
        let draft_part = decode_change(
            bytes,
            at::PART,
            DraftPart::LEN,
            flags,
            presence::PART_PUT,
            presence::PART_REMOVE,
            DraftPart::decode,
            DraftPart::decode_removal,
            R,
        )?;
        let retained = decode_change(
            bytes,
            at::PREVIOUS,
            RetainedPrevious::LEN,
            flags,
            presence::PREVIOUS_PUT,
            presence::PREVIOUS_REMOVE,
            RetainedPrevious::decode,
            RetainedPrevious::decode_removal,
            R,
        )?;

        let result = if flags & presence::RESULT_APPEND != 0 {
            Some(TerminalResult::decode(&bytes[at::RESULT..at::RESULT + TerminalResult::LEN])?)
        } else {
            require_absent(R, bytes, at::RESULT, TerminalResult::LEN)?;
            None
        };

        let handoff = if flags & presence::HANDOFF_PUT != 0 {
            Some(Change::Put(HandoffRef::decode(&bytes[at::HANDOFF..at::HANDOFF + HandoffRef::LEN])?))
        } else {
            // Both the removal and absence are 240 zero bytes; the presence bit is the difference.
            require_absent(R, bytes, at::HANDOFF, HandoffRef::LEN)?;
            if flags & presence::HANDOFF_REMOVE != 0 {
                Some(Change::Remove(()))
            } else {
                None
            }
        };

        let weather = if flags & presence::WEATHER_PUT != 0 {
            Some(WeatherState::decode(&bytes[at::WEATHER..at::WEATHER + WeatherState::LEN])?)
        } else {
            require_absent(R, bytes, at::WEATHER, WeatherState::LEN)?;
            None
        };

        let ride = if flags & presence::RIDE_PUT != 0 {
            Some(Change::Put(ActiveRide::decode(&bytes[at::RIDE..at::RIDE + ActiveRide::LEN])?))
        } else if flags & presence::RIDE_REMOVE != 0 {
            ActiveRide::decode_removal(&bytes[at::RIDE..at::RIDE + ActiveRide::LEN])?;
            Some(Change::Remove(()))
        } else {
            require_absent(R, bytes, at::RIDE, ActiveRide::LEN)?;
            None
        };

        let mutation = Mutation {
            repository,
            generation_cursor,
            active,
            head,
            draft_parent,
            draft_part,
            retained,
            result,
            handoff,
            weather,
            ride,
        };
        mutation.check_kind_rules(kind, flags, identified)?;
        Ok(mutation)
    }

    /// §6.1's per-kind combination rules, in the order that section states them.
    fn check_kind_rules(&self, kind: RecordKind, flags: u32, identified: bool) -> Result<()> {
        let err = || DecodeError::new(Record::Mutation, Reason::Combination);
        let head_touched = flags & (presence::HEAD_PUT | presence::HEAD_REMOVE) != 0;
        let previous_touched = flags & (presence::PREVIOUS_PUT | presence::PREVIOUS_REMOVE) != 0;
        let handoff_touched = flags & (presence::HANDOFF_PUT | presence::HANDOFF_REMOVE) != 0;
        let ride_touched = flags & (presence::RIDE_PUT | presence::RIDE_REMOVE) != 0;
        match kind {
            RecordKind::Claim => {
                // "A claim record requires active put and forbids active remove, result append, and
                // head mutation; it may atomically put the newly reserved draft row."
                if flags & presence::ACTIVE_PUT == 0 {
                    return Err(err());
                }
                if flags & presence::RESULT_APPEND != 0 || head_touched {
                    return Err(err());
                }
                if handoff_touched || ride_touched || self.weather.is_some() {
                    return Err(err());
                }
            }
            RecordKind::Work => {
                // "A work record requires active put for an existing claim and may update its
                // matching draft row, but forbids result and head mutation."
                if flags & presence::ACTIVE_PUT == 0 {
                    return Err(err());
                }
                if flags & presence::RESULT_APPEND != 0 || head_touched {
                    return Err(err());
                }
                if handoff_touched || ride_touched || self.weather.is_some() {
                    return Err(err());
                }
            }
            RecordKind::Terminal => {
                // "A terminal record requires active remove and result append and may contain the
                // publication fields."
                if flags & presence::ACTIVE_REMOVE == 0 || flags & presence::RESULT_APPEND == 0 {
                    return Err(err());
                }
            }
            RecordKind::Retention => {
                // "A retention record has zero OperationId/digest and changes exactly one previous
                // entry."
                if identified || !previous_touched {
                    return Err(err());
                }
                if flags & !(presence::PREVIOUS_PUT | presence::PREVIOUS_REMOVE) != 0 {
                    return Err(err());
                }
            }
            RecordKind::Handoff => {
                // "A handoff record changes the one handoff entry and may update the already-active
                // install operation." A zero-identity record is valid only for the removal suffix.
                if !handoff_touched {
                    return Err(err());
                }
                if flags & presence::HANDOFF_PUT != 0 && !identified {
                    return Err(err());
                }
                let allowed = presence::HANDOFF_PUT | presence::HANDOFF_REMOVE | presence::ACTIVE_PUT;
                if flags & !allowed != 0 {
                    return Err(err());
                }
            }
            RecordKind::Domain => {
                // "A domain record has zero OperationId/digest and changes only the single
                // active-ride recovery state before its publication claim, setting the
                // next-GenerationId cursor only on initial reservation."
                if identified || !ride_touched {
                    return Err(err());
                }
                let allowed = presence::RIDE_PUT | presence::RIDE_REMOVE | presence::GENERATION_CURSOR;
                if flags & !allowed != 0 {
                    return Err(err());
                }
            }
        }
        // "Weather state changes only in the terminal record of its claimed local operation or
        // weather-object publication."
        if self.weather.is_some() && kind != RecordKind::Terminal {
            return Err(err());
        }
        // §6.1 bit 18: the reservation carriers are a claim's active entry, a pre-claim ride domain
        // record, an update rollback snapshot on the already-active install entry, and a
        // parent-manifest work record's reserved resolution field. "No other record may set bit 18."
        if self.generation_cursor.is_some() {
            let carried = match kind {
                RecordKind::Claim | RecordKind::Work => {
                    matches!(self.active, Some(Change::Put(_))) || matches!(self.draft_parent, Some(Change::Put(_)))
                }
                RecordKind::Domain => matches!(self.ride, Some(Change::Put(_))),
                _ => false,
            };
            if !carried {
                return Err(err());
            }
        }
        Ok(())
    }
}

fn pair<P, K>(change: &Option<Change<P, K>>, put: u32, remove: u32) -> u32 {
    match change {
        Some(Change::Put(_)) => put,
        Some(Change::Remove(_)) => remove,
        None => 0,
    }
}

fn require_absent(record: Record, bytes: &[u8], offset: usize, len: usize) -> Result<()> {
    if absent(&bytes[offset..offset + len]) {
        Ok(())
    } else {
        Err(DecodeError::new(record, Reason::Combination))
    }
}

#[allow(clippy::too_many_arguments)]
fn decode_change<P, K>(
    bytes: &[u8],
    offset: usize,
    len: usize,
    flags: u32,
    put_bit: u32,
    remove_bit: u32,
    decode_put: fn(&[u8]) -> Result<P>,
    decode_remove: fn(&[u8]) -> Result<K>,
    record: Record,
) -> Result<Option<Change<P, K>>> {
    let slot = &bytes[offset..offset + len];
    if flags & put_bit != 0 {
        Ok(Some(Change::Put(decode_put(slot)?)))
    } else if flags & remove_bit != 0 {
        Ok(Some(Change::Remove(decode_remove(slot)?)))
    } else {
        require_absent(record, bytes, offset, len)?;
        Ok(None)
    }
}

/// One journal slot body (§6.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JournalBody {
    /// The store this record belongs to.
    pub store: StoreId,
    /// The compaction epoch, and the gate's scope.
    pub epoch: u64,
    /// The globally contiguous sequence, and the gate's logical sequence.
    pub sequence: u64,
    /// The physical slot index this body must be read from.
    pub slot: u16,
    /// What the record is for.
    pub kind: RecordKind,
    /// The operation identity; zero for retention, pre-claim ride recovery, or handoff cleanup.
    pub operation: OperationId,
    /// The canonical-intent digest, zero exactly where the operation identity is.
    pub intent: [u8; 32],
    /// The projection delta.
    pub mutation: Mutation,
}

impl JournalBody {
    /// True when this record carries an operation identity.
    pub fn is_identified(&self) -> bool {
        !self.operation.is_zero() || self.intent != [0u8; 32]
    }

    /// Encodes the 1,536-byte body with its CRC stamped.
    pub fn encode_body(&self) -> [u8; JOURNAL_BODY_LEN] {
        let mut out = [0u8; JOURNAL_BODY_LEN];
        put_bytes(&mut out, 0, &MAGIC);
        put_u16(&mut out, 4, super::gate::FORMAT_VERSION);
        put_u16(&mut out, 6, HEADER_LEN as u16);
        put_bytes(&mut out, 8, self.store.as_bytes());
        put_u64(&mut out, 24, self.epoch);
        put_u64(&mut out, 32, self.sequence);
        put_u16(&mut out, 40, self.slot);
        put_u16(&mut out, 42, self.kind as u16);
        put_bytes(&mut out, 44, self.operation.as_bytes());
        put_bytes(&mut out, 60, &self.intent);
        put_u16(&mut out, 92, MUTATION_LEN as u16);
        put_bytes(&mut out, 96, &self.mutation.encode(self.kind));
        let crc = crc32_with_hole(&out, JOURNAL_BODY_CRC_OFFSET);
        put_u32(&mut out, JOURNAL_BODY_CRC_OFFSET, crc);
        out
    }

    /// Encodes the complete 16,384-byte slot: body, gate, and the pad to the next stride.
    pub fn encode_slot(&self) -> [u8; SLOT_STRIDE] {
        let mut out = [0u8; SLOT_STRIDE];
        let body = self.encode_body();
        put_bytes(&mut out, 0, &body);
        put_bytes(&mut out, JOURNAL_GATE_OFFSET, &self.gate().encode());
        out
    }

    /// The gate that publishes this body.
    pub fn gate(&self) -> Gate {
        Gate {
            magic: MAGIC_JOURNAL,
            slot: self.slot,
            scope: self.epoch,
            sequence: self.sequence,
            body_crc: u32_at(&self.encode_body(), JOURNAL_BODY_CRC_OFFSET),
        }
    }

    /// Decodes the 1,536-byte body.
    pub fn decode_body(bytes: &[u8]) -> Result<Self> {
        const R: Record = Record::JournalSlot;
        let err = |reason| DecodeError::new(R, reason);
        if bytes.len() != JOURNAL_BODY_LEN {
            return Err(err(Reason::Length));
        }
        if bytes[0..4] != MAGIC {
            return Err(err(Reason::Magic));
        }
        if u16_at(bytes, 4) != super::gate::FORMAT_VERSION {
            return Err(err(Reason::Version));
        }
        if u16_at(bytes, 6) as usize != HEADER_LEN {
            return Err(err(Reason::HeaderLength));
        }
        if u16_at(bytes, 92) as usize != MUTATION_LEN {
            return Err(err(Reason::Overflow));
        }
        require_zero(R, bytes, 94, 2)?;
        require_zero(R, bytes, 96 + MUTATION_LEN, JOURNAL_BODY_CRC_OFFSET - (96 + MUTATION_LEN))?;
        let epoch = u64_at(bytes, 24);
        if epoch == 0 {
            return Err(err(Reason::Sequence));
        }
        let slot = u16_at(bytes, 40);
        if slot as usize >= super::limits::JOURNAL_SLOTS {
            return Err(err(Reason::SlotIndex));
        }
        let kind = RecordKind::from_u16(u16_at(bytes, 42)).ok_or(err(Reason::UnknownEnum))?;
        let operation = OperationId::new(bytes16_at(bytes, 44));
        let intent = bytes32_at(bytes, 60);
        let identified = !operation.is_zero() || intent != [0u8; 32];
        // §6.1: the identity is "zero only for retention, pre-claim ride recovery, or
        // completed-handoff cleanup", and where it is present both fields are.
        if operation.is_zero() != (intent == [0u8; 32]) {
            return Err(err(Reason::Combination));
        }
        match kind {
            RecordKind::Claim | RecordKind::Work | RecordKind::Terminal if !identified => {
                return Err(err(Reason::Combination))
            }
            RecordKind::Retention | RecordKind::Domain if identified => return Err(err(Reason::Combination)),
            _ => {}
        }
        let mutation = Mutation::decode(&bytes[96..96 + MUTATION_LEN], kind, identified)?;
        Ok(JournalBody {
            store: StoreId::new(bytes16_at(bytes, 8)),
            epoch,
            sequence: u64_at(bytes, 32),
            slot,
            kind,
            operation,
            intent,
            mutation,
        })
    }

    /// Validates a complete slot: body, the physical slot index, the gate binding, and the zero pad.
    pub fn validate_slot(slot_bytes: &[u8], slot: u16) -> Result<Self> {
        const R: Record = Record::JournalSlot;
        if slot_bytes.len() != SLOT_STRIDE {
            return Err(DecodeError::new(R, Reason::Length));
        }
        let body = Self::decode_body(&slot_bytes[..JOURNAL_BODY_LEN])?;
        if body.slot != slot {
            return Err(DecodeError::new(R, Reason::SlotIndex));
        }
        let gate = Gate::decode(&slot_bytes[JOURNAL_GATE_OFFSET..JOURNAL_GATE_OFFSET + 512], MAGIC_JOURNAL, slot)?;
        gate.bind(&BodyBinding {
            stored_crc: u32_at(slot_bytes, JOURNAL_BODY_CRC_OFFSET),
            fresh_crc: crc32_with_hole(&slot_bytes[..JOURNAL_BODY_LEN], JOURNAL_BODY_CRC_OFFSET),
            scope: body.epoch,
            sequence: body.sequence,
        })?;
        require_zero(R, slot_bytes, 2_048, SLOT_STRIDE - 2_048)?;
        Ok(body)
    }
}

#[cfg(test)]
mod tests {
    use super::super::samples as sample;
    use super::*;

    fn claim(sequence: u64, slot: u16, operation: [u8; 16]) -> JournalBody {
        sample::claim(1, sequence, slot, operation, 43)
    }

    fn terminal(sequence: u64, slot: u16, operation: [u8; 16], commit: u64) -> JournalBody {
        sample::publish(1, sequence, slot, operation, commit, sample::head(1, 7))
    }

    #[test]
    fn a_claim_round_trips_through_a_whole_slot() {
        let body = claim(1, 0, sample::OP_A);
        let slot = body.encode_slot();
        assert_eq!(JournalBody::validate_slot(&slot, 0).unwrap(), body);
        assert!(JournalBody::validate_slot(&slot, 1).is_err());
    }

    #[test]
    fn a_terminal_round_trips_with_its_publication_fields() {
        let body = terminal(2, 1, sample::OP_A, 1);
        assert_eq!(JournalBody::validate_slot(&body.encode_slot(), 1).unwrap(), body);
    }

    #[test]
    fn every_record_kind_round_trips() {
        let store = StoreId::new([0x3C; 16]);
        let base = JournalBody {
            store,
            epoch: 1,
            sequence: 1,
            slot: 0,
            kind: RecordKind::Claim,
            operation: OperationId::ZERO,
            intent: [0u8; 32],
            mutation: Mutation::default(),
        };

        let work = JournalBody {
            kind: RecordKind::Work,
            operation: OperationId::new(sample::OP_A),
            intent: [0x11; 32],
            mutation: Mutation { active: Some(Change::Put(sample::active(sample::OP_A))), ..Mutation::default() },
            ..base
        };
        assert_eq!(JournalBody::decode_body(&work.encode_body()).unwrap(), work);

        let retention = JournalBody {
            kind: RecordKind::Retention,
            mutation: Mutation { retained: Some(Change::Remove(GenerationId::new(9))), ..Mutation::default() },
            ..base
        };
        assert_eq!(JournalBody::decode_body(&retention.encode_body()).unwrap(), retention);

        let handoff = JournalBody {
            kind: RecordKind::Handoff,
            operation: OperationId::new(sample::OP_INSTALL),
            intent: [0x55; 32],
            mutation: Mutation {
                handoff: Some(Change::Put(sample::handoff_ref(4, super::super::handoff::HandoffPhase::Armed))),
                ..Mutation::default()
            },
            ..base
        };
        assert_eq!(JournalBody::decode_body(&handoff.encode_body()).unwrap(), handoff);

        let domain = JournalBody {
            kind: RecordKind::Domain,
            mutation: Mutation {
                ride: Some(Change::Put(sample::ride())),
                generation_cursor: Some(78),
                ..Mutation::default()
            },
            ..base
        };
        assert_eq!(JournalBody::decode_body(&domain.encode_body()).unwrap(), domain);
    }

    #[test]
    fn a_claim_that_touches_a_head_or_a_result_is_invalid() {
        let mut body = claim(1, 0, sample::OP_A);
        body.mutation.head = Some(Change::Put(sample::head(1, 7)));
        assert_eq!(JournalBody::decode_body(&body.encode_body()).unwrap_err().reason, Reason::Combination);

        let mut body = claim(1, 0, sample::OP_A);
        body.mutation.result = Some(sample::result(1, sample::OP_A));
        assert_eq!(JournalBody::decode_body(&body.encode_body()).unwrap_err().reason, Reason::Combination);
    }

    #[test]
    fn a_terminal_without_its_active_remove_or_result_is_invalid() {
        let mut body = terminal(2, 1, sample::OP_A, 1);
        body.mutation.active = None;
        assert_eq!(JournalBody::decode_body(&body.encode_body()).unwrap_err().reason, Reason::Combination);

        let mut body = terminal(2, 1, sample::OP_A, 1);
        body.mutation.result = None;
        assert_eq!(JournalBody::decode_body(&body.encode_body()).unwrap_err().reason, Reason::Combination);
    }

    #[test]
    fn a_retention_record_is_zero_identity_and_touches_exactly_one_previous_entry() {
        let mut body = JournalBody {
            store: StoreId::new([0x3C; 16]),
            epoch: 1,
            sequence: 1,
            slot: 0,
            kind: RecordKind::Retention,
            operation: OperationId::ZERO,
            intent: [0u8; 32],
            mutation: Mutation { retained: Some(Change::Put(sample::retained(9))), ..Mutation::default() },
        };
        assert!(JournalBody::decode_body(&body.encode_body()).is_ok());

        body.operation = OperationId::new(sample::OP_A);
        body.intent = [0x11; 32];
        assert_eq!(JournalBody::decode_body(&body.encode_body()).unwrap_err().reason, Reason::Combination);

        body.operation = OperationId::ZERO;
        body.intent = [0u8; 32];
        body.mutation.retained = None;
        assert_eq!(JournalBody::decode_body(&body.encode_body()).unwrap_err().reason, Reason::Combination);
    }

    #[test]
    fn weather_state_changes_only_in_a_terminal_record() {
        let mut body = terminal(2, 1, sample::OP_A, 1);
        body.mutation.weather = Some(sample::weather());
        assert!(JournalBody::decode_body(&body.encode_body()).is_ok());

        let mut body = claim(1, 0, sample::OP_A);
        body.mutation.weather = Some(sample::weather());
        assert_eq!(JournalBody::decode_body(&body.encode_body()).unwrap_err().reason, Reason::Combination);
    }

    #[test]
    fn only_the_named_records_may_reserve_a_generation() {
        let mut body = terminal(2, 1, sample::OP_A, 1);
        body.mutation.generation_cursor = Some(50);
        assert_eq!(JournalBody::decode_body(&body.encode_body()).unwrap_err().reason, Reason::Combination);
    }

    #[test]
    fn an_undefined_presence_bit_is_rejected() {
        let mut body = claim(1, 0, sample::OP_A).encode_body();
        let flags = u32_at(&body, 96 + 4) | (1 << 19);
        put_u32(&mut body, 96 + 4, flags);
        let crc = crc32_with_hole(&body, JOURNAL_BODY_CRC_OFFSET);
        put_u32(&mut body, JOURNAL_BODY_CRC_OFFSET, crc);
        assert_eq!(JournalBody::decode_body(&body).unwrap_err().reason, Reason::Reserved);
    }

    #[test]
    fn an_absent_entry_with_a_nonzero_byte_is_rejected() {
        let mut body = claim(1, 0, sample::OP_A).encode_body();
        body[96 + super::at::HEAD] = 1;
        let crc = crc32_with_hole(&body, JOURNAL_BODY_CRC_OFFSET);
        put_u32(&mut body, JOURNAL_BODY_CRC_OFFSET, crc);
        assert_eq!(JournalBody::decode_body(&body).unwrap_err().reason, Reason::Combination);
    }

    #[test]
    fn a_nonzero_pad_invalidates_the_slot() {
        let mut slot = claim(1, 0, sample::OP_A).encode_slot();
        slot[3_000] = 1;
        assert_eq!(JournalBody::validate_slot(&slot, 0).unwrap_err().reason, Reason::Reserved);
    }

    /// The mutation's entry slots tile it exactly as §6.1's table says.
    #[test]
    fn mutation_entry_offsets_match_the_table() {
        assert_eq!(super::at::ACTIVE + ActiveOperation::LEN, super::at::HEAD);
        assert_eq!(super::at::HEAD + CatalogHead::LEN, super::at::PARENT);
        assert_eq!(super::at::PARENT + DraftParent::LEN, super::at::PART);
        assert_eq!(super::at::PART + DraftPart::LEN, super::at::PREVIOUS);
        assert_eq!(super::at::PREVIOUS + RetainedPrevious::LEN, super::at::RESULT);
        assert_eq!(super::at::RESULT + TerminalResult::LEN, super::at::HANDOFF);
        assert_eq!(super::at::HANDOFF + HandoffRef::LEN, super::at::WEATHER);
        assert_eq!(super::at::WEATHER + WeatherState::LEN, super::at::RIDE);
        assert_eq!(super::at::RIDE + ActiveRide::LEN, MUTATION_LEN);
    }
}
