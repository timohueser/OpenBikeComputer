//! The catalog checkpoint: its 128-byte header and its eleven fixed regions
//! (`OBC2_Storage_Format.md` §5).
//!
//! A checkpoint file is exactly 65,536 bytes — four slot strides — holding 65,024 body bytes
//! followed by one `O2CG` gate. This module owns the body: where each region starts, which count
//! selects its occupied prefix, and the structural rules §5.1 states about the prefix (sorted by
//! the entry's stated key, everything after it zero, the result region circular instead).
//!
//! It deliberately stops at structure. Turning a body into a projection — and a projection back
//! into a body — is [`super::model`]'s job, because that is where capacities, replay and the
//! compaction pass live. What is here is what a decoder must prove before any of that runs, and
//! [`CheckpointHeader::decode`] plus [`validate_body`] are the two halves of it: the header alone
//! is a bounded 128-byte read that mount can do before it commits to anything larger.

use super::error::{DecodeError, Reason, Record, Result};
use super::gate::{BodyBinding, Gate, MAGIC_CHECKPOINT};
use super::limits::{
    CHECKPOINT_BODY_CRC_OFFSET, CHECKPOINT_BODY_LEN, MAX_ACTIVE_OPERATIONS, MAX_CATALOG_HEADS, MAX_DRAFT_PARENTS,
    MAX_DRAFT_PARTS, MAX_NORMAL_ACTIVE_OPERATIONS, MAX_REPOSITORY_STATES, MAX_RETAINED_PREVIOUS, MAX_TERMINAL_RESULTS,
    RESERVED_ACTIVE_OPERATIONS,
};
use super::raw::{crc32_with_hole, is_zero, put_bytes, put_u16, put_u32, put_u64, u16_at, u32_at, u64_at};
use obc_link::ids::StoreId;

use super::entries::{
    absent, ActiveOperation, ActiveRide, CatalogHead, DraftParent, DraftPart, HeadKey, RepositoryState,
    RetainedPrevious, TerminalResult, WeatherState,
};

/// Body magic.
pub const MAGIC: [u8; 4] = *b"O2CK";
/// The header length the body declares.
pub const HEADER_LEN: usize = 128;

/// One fixed region of the body: where it starts, how big an entry is, and how many fit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Region {
    /// Byte offset of the region's first entry.
    pub offset: usize,
    /// Bytes per entry.
    pub entry: usize,
    /// Entries the region holds.
    pub capacity: usize,
}

impl Region {
    const fn new(offset: usize, entry: usize, capacity: usize) -> Self {
        Region { offset, entry, capacity }
    }

    /// The region's end offset.
    pub const fn end(&self) -> usize {
        self.offset + self.entry * self.capacity
    }

    /// The byte range of entry `index`.
    pub fn slot(&self, index: usize) -> core::ops::Range<usize> {
        let start = self.offset + index * self.entry;
        start..start + self.entry
    }
}

/// Repository states, keyed by `ObjectKind`.
pub const REPOSITORIES: Region = Region::new(128, RepositoryState::LEN, MAX_REPOSITORY_STATES);
/// Catalog heads, keyed by `(ObjectKind, LogicalObjectId)`.
pub const HEADS: Region = Region::new(512, CatalogHead::LEN, MAX_CATALOG_HEADS);
/// Active operations, keyed by `OperationId`.
pub const ACTIVE: Region = Region::new(41_472, ActiveOperation::LEN, MAX_ACTIVE_OPERATIONS);
/// The one draft parent.
pub const DRAFT_PARENT: Region = Region::new(42_624, DraftParent::LEN, MAX_DRAFT_PARENTS);
/// Draft parts of that parent.
pub const DRAFT_PARTS: Region = Region::new(42_752, DraftPart::LEN, MAX_DRAFT_PARTS);
/// Retained previous generations, keyed by `GenerationId`.
pub const RETAINED: Region = Region::new(45_824, RetainedPrevious::LEN, MAX_RETAINED_PREVIOUS);
/// The terminal-result ring.
pub const RESULTS: Region = Region::new(46_336, TerminalResult::LEN, MAX_TERMINAL_RESULTS);
/// The update-handoff projection.
pub const HANDOFF: Region = Region::new(59_648, super::limits::HANDOFF_REF_LEN, 1);
/// The weather-request state.
pub const WEATHER: Region = Region::new(59_888, WeatherState::LEN, 1);
/// The active-ride state.
pub const RIDE: Region = Region::new(59_968, ActiveRide::LEN, 1);
/// The zero tail between the last region and the body CRC.
pub const TAIL: core::ops::Range<usize> = 60_096..CHECKPOINT_BODY_CRC_OFFSET;

/// The decoded 128-byte header (§5.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CheckpointHeader {
    /// The store this checkpoint belongs to.
    pub store: StoreId,
    /// The compaction epoch, nonzero and monotonic.
    pub epoch: u64,
    /// The last journal sequence this body absorbed.
    pub through_sequence: u64,
    /// The next `GenerationId` cursor; greater than every reserved generation.
    pub next_generation: u64,
    /// Occupied repository rows.
    pub repository_count: u16,
    /// Occupied catalog heads.
    pub head_count: u16,
    /// Occupied active rows, `0..=9`.
    pub active_count: u8,
    /// Occupied draft-parent rows, `0..=1`.
    pub draft_parent_count: u8,
    /// Occupied draft-part rows, `0..=32`.
    pub draft_part_count: u8,
    /// Occupied retained-previous rows, `0..=8`.
    pub retained_count: u8,
    /// The result ring's start index, `0..=63`.
    pub result_start: u8,
    /// The result ring's occupancy, `0..=64`.
    pub result_count: u8,
    /// Occupied handoff projections, `0..=1`.
    pub handoff_count: u8,
    /// Flags; bit 0 is the durable record of a store-wide degraded mount.
    pub flags: u8,
    /// The terminal-commit counter, which work expiry is evaluated against.
    pub terminal_counter: u64,
    /// Occupied weather states, `0..=1`.
    pub weather_count: u8,
    /// Occupied active-ride states, `0..=1`.
    pub ride_count: u8,
}

impl CheckpointHeader {
    /// Bit 0 of `flags`: the store mounted store-wide degraded and needs explicit recovery (§12).
    pub const FLAG_RECOVERY_DEGRADED: u8 = 1 << 0;

    /// Encodes the exact 128 header bytes.
    pub fn encode(&self) -> [u8; HEADER_LEN] {
        let mut out = [0u8; HEADER_LEN];
        put_bytes(&mut out, 0, &MAGIC);
        put_u16(&mut out, 4, super::gate::FORMAT_VERSION);
        put_u16(&mut out, 6, HEADER_LEN as u16);
        put_bytes(&mut out, 8, self.store.as_bytes());
        put_u64(&mut out, 24, self.epoch);
        put_u64(&mut out, 32, self.through_sequence);
        put_u64(&mut out, 40, self.next_generation);
        put_u16(&mut out, 48, self.repository_count);
        put_u16(&mut out, 50, self.head_count);
        out[52] = self.active_count;
        out[53] = self.draft_parent_count;
        out[54] = self.draft_part_count;
        out[55] = self.retained_count;
        out[56] = self.result_start;
        out[57] = self.result_count;
        out[58] = self.handoff_count;
        out[59] = self.flags;
        put_u64(&mut out, 60, self.terminal_counter);
        put_u32(&mut out, 68, CHECKPOINT_BODY_LEN as u32);
        out[104] = self.weather_count;
        out[105] = self.ride_count;
        out
    }

    /// Decodes and range-checks the header. Every count is proved against its region capacity here,
    /// before any derived offset is used (§1).
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        const R: Record = Record::Checkpoint;
        let err = |reason| DecodeError::new(R, reason);
        if bytes.len() < HEADER_LEN {
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
        let epoch = u64_at(bytes, 24);
        if epoch == 0 {
            return Err(err(Reason::Sequence));
        }
        let header = CheckpointHeader {
            store: StoreId::new(super::raw::bytes16_at(bytes, 8)),
            epoch,
            through_sequence: u64_at(bytes, 32),
            next_generation: u64_at(bytes, 40),
            repository_count: u16_at(bytes, 48),
            head_count: u16_at(bytes, 50),
            active_count: bytes[52],
            draft_parent_count: bytes[53],
            draft_part_count: bytes[54],
            retained_count: bytes[55],
            result_start: bytes[56],
            result_count: bytes[57],
            handoff_count: bytes[58],
            flags: bytes[59],
            terminal_counter: u64_at(bytes, 60),
            weather_count: bytes[104],
            ride_count: bytes[105],
        };
        if header.flags & !Self::FLAG_RECOVERY_DEGRADED != 0 {
            return Err(err(Reason::Reserved));
        }
        if u32_at(bytes, 68) as usize != CHECKPOINT_BODY_LEN {
            return Err(err(Reason::Overflow));
        }
        if !is_zero(bytes, 72, 32) || !is_zero(bytes, 106, 22) {
            return Err(err(Reason::Reserved));
        }
        let over = header.repository_count as usize > REPOSITORIES.capacity
            || header.head_count as usize > HEADS.capacity
            || header.active_count as usize > ACTIVE.capacity
            || header.draft_parent_count as usize > DRAFT_PARENT.capacity
            || header.draft_part_count as usize > DRAFT_PARTS.capacity
            || header.retained_count as usize > RETAINED.capacity
            || header.result_count as usize > RESULTS.capacity
            || header.result_start as usize >= RESULTS.capacity
            || header.handoff_count as usize > HANDOFF.capacity
            || header.weather_count as usize > 1
            || header.ride_count as usize > 1;
        if over {
            return Err(err(Reason::Count));
        }
        Ok(header)
    }
}

/// Structurally validates a complete 65,024-byte body and returns its header.
///
/// The checks are §5.1's, in the order §1 requires — counts before derived offsets:
///
/// 1. the body CRC covers the body with its own field zeroed;
/// 2. every occupied entry decodes;
/// 3. occupied entries are sorted by their stated key, with no duplicate;
/// 4. every entry past the occupied prefix is zero, and so is the region tail;
/// 5. the active region holds at most eight normal rows and at most one reserved row.
pub fn validate_body(body: &[u8]) -> Result<CheckpointHeader> {
    const R: Record = Record::Checkpoint;
    let err = |reason| DecodeError::new(R, reason);
    if body.len() != CHECKPOINT_BODY_LEN {
        return Err(err(Reason::Length));
    }
    if u32_at(body, CHECKPOINT_BODY_CRC_OFFSET) != body_crc(body) {
        return Err(err(Reason::BodyCrc));
    }
    let header = CheckpointHeader::decode(body)?;

    // Repository states: keyed by kind, strictly ascending.
    let mut previous_kind: Option<u16> = None;
    for index in 0..header.repository_count as usize {
        let row = RepositoryState::decode(&body[REPOSITORIES.slot(index)])?;
        if let Some(previous) = previous_kind {
            if row.kind < previous {
                return Err(err(Reason::Order));
            }
            if row.kind == previous {
                return Err(err(Reason::Duplicate));
            }
        }
        previous_kind = Some(row.kind);
    }
    zero_after(body, REPOSITORIES, header.repository_count as usize)?;

    // Catalog heads: keyed by (kind, logical id), strictly ascending.
    let mut previous_head: Option<HeadKey> = None;
    for index in 0..header.head_count as usize {
        let row = CatalogHead::decode(&body[HEADS.slot(index)])?;
        if let Some(previous) = previous_head {
            if row.key < previous {
                return Err(err(Reason::Order));
            }
            if row.key == previous {
                return Err(err(Reason::Duplicate));
            }
        }
        previous_head = Some(row.key);
    }
    zero_after(body, HEADS, header.head_count as usize)?;

    // Active operations: keyed by OperationId, compared lexicographically over wire bytes.
    let mut previous_op: Option<[u8; 16]> = None;
    let mut reserved_rows = 0usize;
    let mut normal_rows = 0usize;
    for index in 0..header.active_count as usize {
        let row = ActiveOperation::decode(&body[ACTIVE.slot(index)])?;
        let key = row.operation.to_bytes();
        if let Some(previous) = previous_op {
            if key < previous {
                return Err(err(Reason::Order));
            }
            if key == previous {
                return Err(err(Reason::Duplicate));
            }
        }
        previous_op = Some(key);
        if row.flags & ActiveOperation::FLAG_RESERVED_SLOT != 0 {
            reserved_rows += 1;
        } else {
            normal_rows += 1;
        }
    }
    if reserved_rows > RESERVED_ACTIVE_OPERATIONS || normal_rows > MAX_NORMAL_ACTIVE_OPERATIONS {
        return Err(err(Reason::Count));
    }
    zero_after(body, ACTIVE, header.active_count as usize)?;

    for index in 0..header.draft_parent_count as usize {
        DraftParent::decode(&body[DRAFT_PARENT.slot(index)])?;
    }
    zero_after(body, DRAFT_PARENT, header.draft_parent_count as usize)?;

    // Draft parts: keyed by (parent, kind, part key).
    let mut previous_part: Option<([u8; 16], u16, u64)> = None;
    for index in 0..header.draft_part_count as usize {
        let row = DraftPart::decode(&body[DRAFT_PARTS.slot(index)])?;
        let key = row.key.sort_key();
        if let Some(previous) = previous_part {
            if key < previous {
                return Err(err(Reason::Order));
            }
            if key == previous {
                return Err(err(Reason::Duplicate));
            }
        }
        previous_part = Some(key);
    }
    zero_after(body, DRAFT_PARTS, header.draft_part_count as usize)?;

    // Retained previous: keyed by GenerationId.
    let mut previous_generation: Option<u64> = None;
    for index in 0..header.retained_count as usize {
        let row = RetainedPrevious::decode(&body[RETAINED.slot(index)])?;
        if let Some(previous) = previous_generation {
            if row.generation.get() < previous {
                return Err(err(Reason::Order));
            }
            if row.generation.get() == previous {
                return Err(err(Reason::Duplicate));
            }
        }
        previous_generation = Some(row.generation.get());
    }
    zero_after(body, RETAINED, header.retained_count as usize)?;

    // The result ring is the one region that is not a sorted prefix: `result_start` and
    // `result_count` select its occupied entries, and everything else in it is zero.
    let mut occupied = [false; MAX_TERMINAL_RESULTS];
    for step in 0..header.result_count as usize {
        let index = (header.result_start as usize + step) % RESULTS.capacity;
        occupied[index] = true;
        TerminalResult::decode(&body[RESULTS.slot(index)])?;
    }
    for (index, taken) in occupied.iter().enumerate() {
        if !taken && !absent(&body[RESULTS.slot(index)]) {
            return Err(err(Reason::Count));
        }
    }

    for index in 0..header.handoff_count as usize {
        super::handoff::HandoffRef::decode(&body[HANDOFF.slot(index)])?;
    }
    zero_after(body, HANDOFF, header.handoff_count as usize)?;

    for index in 0..header.weather_count as usize {
        WeatherState::decode(&body[WEATHER.slot(index)])?;
    }
    zero_after(body, WEATHER, header.weather_count as usize)?;

    for index in 0..header.ride_count as usize {
        ActiveRide::decode(&body[RIDE.slot(index)])?;
    }
    zero_after(body, RIDE, header.ride_count as usize)?;

    if !is_zero(body, TAIL.start, TAIL.end - TAIL.start) {
        return Err(err(Reason::Reserved));
    }
    Ok(header)
}

/// Proves the whole checkpoint file: its body against its gate at file offset 65,024 (§5).
pub fn validate_file(file: &[u8], slot: u16) -> Result<CheckpointHeader> {
    const R: Record = Record::Checkpoint;
    if file.len() != super::limits::CHECKPOINT_FILE_LEN {
        return Err(DecodeError::new(R, Reason::Length));
    }
    let body = &file[..CHECKPOINT_BODY_LEN];
    let header = validate_body(body)?;
    let gate = Gate::decode(&file[super::limits::CHECKPOINT_GATE_OFFSET..], MAGIC_CHECKPOINT, slot)?;
    gate.bind(&BodyBinding {
        stored_crc: u32_at(body, CHECKPOINT_BODY_CRC_OFFSET),
        fresh_crc: body_crc(body),
        scope: header.epoch,
        sequence: header.through_sequence,
    })?;
    Ok(header)
}

/// The body CRC of a complete body, with the CRC field itself treated as zero (§1).
pub fn body_crc(body: &[u8]) -> u32 {
    crc32_with_hole(body, CHECKPOINT_BODY_CRC_OFFSET)
}

/// Stamps the body CRC into a fully written body.
pub fn seal_body(body: &mut [u8]) {
    let crc = body_crc(body);
    put_u32(body, CHECKPOINT_BODY_CRC_OFFSET, crc);
}

/// The gate that publishes a sealed body.
pub fn gate_for(body: &[u8], slot: u16) -> Gate {
    Gate {
        magic: MAGIC_CHECKPOINT,
        slot,
        scope: u64_at(body, 24),
        sequence: u64_at(body, 32),
        body_crc: u32_at(body, CHECKPOINT_BODY_CRC_OFFSET),
    }
}

fn zero_after(body: &[u8], region: Region, count: usize) -> Result<()> {
    for index in count..region.capacity {
        if !absent(&body[region.slot(index)]) {
            return Err(DecodeError::new(Record::Checkpoint, Reason::Count));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// §5.1's table, region by region: each starts where the previous ended and the last one ends
    /// at the zero tail.
    #[test]
    fn regions_tile_the_body_exactly() {
        assert_eq!(REPOSITORIES.offset, HEADER_LEN);
        assert_eq!(REPOSITORIES.end(), 512);
        assert_eq!(HEADS.offset, 512);
        assert_eq!(HEADS.end(), 41_472);
        assert_eq!(ACTIVE.end(), 42_624);
        assert_eq!(DRAFT_PARENT.end(), 42_752);
        assert_eq!(DRAFT_PARTS.end(), 45_824);
        assert_eq!(RETAINED.end(), 46_336);
        assert_eq!(RESULTS.end(), 59_648);
        assert_eq!(HANDOFF.end(), 59_888);
        assert_eq!(WEATHER.end(), 59_968);
        assert_eq!(RIDE.end(), 60_096);
        assert_eq!(TAIL.start, RIDE.end());
        assert_eq!(TAIL.end, CHECKPOINT_BODY_CRC_OFFSET);
    }

    #[test]
    fn header_round_trips() {
        let header = CheckpointHeader {
            store: StoreId::new([0x3C; 16]),
            epoch: 1,
            through_sequence: 0,
            next_generation: 0,
            repository_count: 0,
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
        };
        assert_eq!(CheckpointHeader::decode(&header.encode()).unwrap(), header);
    }

    #[test]
    fn a_zero_epoch_is_not_a_checkpoint() {
        let mut bytes = [0u8; HEADER_LEN];
        bytes[0..4].copy_from_slice(&MAGIC);
        bytes[4..6].copy_from_slice(&1u16.to_le_bytes());
        bytes[6..8].copy_from_slice(&(HEADER_LEN as u16).to_le_bytes());
        bytes[68..72].copy_from_slice(&(CHECKPOINT_BODY_LEN as u32).to_le_bytes());
        assert_eq!(CheckpointHeader::decode(&bytes).unwrap_err().reason, Reason::Sequence);
    }

    #[test]
    fn a_count_above_its_region_capacity_is_rejected_before_any_offset_is_derived() {
        let mut header = CheckpointHeader::decode(&{
            let mut bytes = [0u8; HEADER_LEN];
            bytes[0..4].copy_from_slice(&MAGIC);
            bytes[4..6].copy_from_slice(&1u16.to_le_bytes());
            bytes[6..8].copy_from_slice(&(HEADER_LEN as u16).to_le_bytes());
            bytes[24..32].copy_from_slice(&1u64.to_le_bytes());
            bytes[68..72].copy_from_slice(&(CHECKPOINT_BODY_LEN as u32).to_le_bytes());
            bytes
        })
        .unwrap();
        header.head_count = 257;
        assert_eq!(CheckpointHeader::decode(&header.encode()).unwrap_err().reason, Reason::Count);
        header.head_count = 0;
        header.active_count = 10;
        assert_eq!(CheckpointHeader::decode(&header.encode()).unwrap_err().reason, Reason::Count);
        header.active_count = 0;
        header.result_start = 64;
        assert_eq!(CheckpointHeader::decode(&header.encode()).unwrap_err().reason, Reason::Count);
    }
}
