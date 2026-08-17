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
    CHECKPOINT_BODY_CRC_OFFSET, CHECKPOINT_BODY_LEN, GATE_LEN, MAX_ACTIVE_OPERATIONS, MAX_CATALOG_HEADS,
    MAX_DRAFT_PARENTS, MAX_DRAFT_PARTS, MAX_NORMAL_ACTIVE_OPERATIONS, MAX_REPOSITORY_STATES, MAX_RETAINED_PREVIOUS,
    MAX_TERMINAL_RESULTS, RESERVED_ACTIVE_OPERATIONS,
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

/// §5.1's regions in body order, so anything that needs to reason about *all* of them — the
/// tiling, the largest entry shape, a forward pass — derives it from one list rather than
/// restating it.
pub const REGIONS: [Region; 10] =
    [REPOSITORIES, HEADS, ACTIVE, DRAFT_PARENT, DRAFT_PARTS, RETAINED, RESULTS, HANDOFF, WEATHER, RIDE];

/// The largest entry any region holds — the update-handoff projection's 240 bytes, which is what
/// §6.3's forward pass must be able to stage.
pub const fn largest_entry() -> usize {
    let mut largest = 0;
    let mut index = 0;
    while index < REGIONS.len() {
        if REGIONS[index].entry > largest {
            largest = REGIONS[index].entry;
        }
        index += 1;
    }
    largest
}

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
        // §6.3 maps physical slot `i` onto `through_sequence + i + 1`, so a header whose sequence
        // cannot carry a full journal of successors is not a header this format can replay. §5.2:
        // "Sequences and generation IDs never wrap"; refusing here is what keeps every later
        // addition in this module total rather than merely unlikely to overflow.
        if header.through_sequence > u64::MAX - super::limits::JOURNAL_SLOTS as u64 {
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

/// One checkpoint file's bytes, read in bounded spans.
///
/// §13's mount budget is what this exists for: "the staging a commit needs is one journal-slot body,
/// and the staging compaction needs is … 752 bytes". Nothing there authorizes a 65,536-byte mount
/// image, and one was only ever needed because validation took the body as a single slice. A source
/// hands the scan whatever span it asks for, so the staging becomes the caller's scratch — a sector
/// on a device that has nothing else, a stride on one that already holds one.
pub trait FileSource {
    /// What a bounded read of the checkpoint file can fail with.
    type Error;

    /// Fills `into` from file offset `offset`. A short read is an error, never a success (§13.1).
    fn read_span(&mut self, offset: usize, into: &mut [u8]) -> core::result::Result<(), Self::Error>;
}

/// Why a streamed validation stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamError<E> {
    /// A bounded read of the checkpoint file failed.
    Media(E),
    /// The bytes are not a valid checkpoint.
    Invalid(DecodeError),
}

impl<E> From<DecodeError> for StreamError<E> {
    fn from(error: DecodeError) -> Self {
        StreamError::Invalid(error)
    }
}

/// What a streamed validation hands out as each entry passes under it.
///
/// Every method has a default, so a validation that only wants the verdict passes `&mut ()` and the
/// compiler removes them. The one real implementation builds §13's resident index without the body
/// ever existing in one place, which is the whole point of streaming it.
///
/// **A sink is only meaningful after the validation returns `Ok`.** Entries are handed over as they
/// are decoded, and the body CRC is not known until the last sector, so a failed scan leaves
/// whatever prefix it got to. Every caller here re-validates before it projects.
pub trait EntrySink {
    /// The 128-byte header, before any entry.
    fn header(&mut self, header: &CheckpointHeader) {
        let _ = header;
    }
    /// One occupied repository row, in key order.
    fn repository(&mut self, row: &RepositoryState) {
        let _ = row;
    }
    /// One occupied catalog head, in key order.
    fn head(&mut self, row: &CatalogHead) {
        let _ = row;
    }
    /// One occupied active row, in key order.
    fn active(&mut self, row: &ActiveOperation) {
        let _ = row;
    }
    /// The one draft parent.
    fn draft_parent(&mut self, row: &DraftParent) {
        let _ = row;
    }
    /// One occupied draft part, in key order.
    fn draft_part(&mut self, row: &DraftPart) {
        let _ = row;
    }
    /// One occupied retained-previous row, in key order.
    fn retained(&mut self, row: &RetainedPrevious) {
        let _ = row;
    }
    /// One occupied terminal result, at its **physical** ring index and in physical order.
    fn result(&mut self, physical: usize, row: &TerminalResult) {
        let _ = (physical, row);
    }
    /// The one update-handoff projection.
    fn handoff(&mut self, row: &super::handoff::HandoffRef) {
        let _ = row;
    }
    /// The one weather-request state.
    fn weather(&mut self, row: &WeatherState) {
        let _ = row;
    }
    /// The one active-ride state.
    fn ride(&mut self, row: &ActiveRide) {
        let _ = row;
    }
}

/// The sink a validation that wants only the verdict passes.
impl EntrySink for () {}

/// Structurally validates a complete 65,024-byte body and returns its header.
///
/// The checks are §5.1's, in the order §1 requires — counts before derived offsets:
///
/// 1. the body CRC covers the body with its own field zeroed;
/// 2. every occupied entry decodes;
/// 3. occupied entries are sorted by their stated key, with no duplicate;
/// 4. every entry past the occupied prefix is zero, and so is the region tail;
/// 5. the active region holds at most eight normal rows and at most one reserved row.
///
/// It is the same scan [`validate_streamed`] runs, over a source that is the slice itself: there is
/// one definition of what a valid checkpoint is, and a host that has the body in hand and a device
/// that has 512 bytes of scratch both reach it.
pub fn validate_body(body: &[u8]) -> Result<CheckpointHeader> {
    if body.len() != CHECKPOINT_BODY_LEN {
        return Err(DecodeError::new(Record::Checkpoint, Reason::Length));
    }
    let mut scan = Scan::new();
    scan.push(body, &mut ());
    scan.finish().map(|(header, _)| header)
}

/// The streamed form: the same scan, over spans of at most `scratch`.
///
/// `scratch` may be as small as one sector and as large as the caller likes; it bounds the read
/// size and nothing else. The sink sees every occupied entry as it is decoded.
pub fn validate_streamed<S: FileSource, K: EntrySink>(
    source: &mut S,
    scratch: &mut [u8],
    sink: &mut K,
) -> core::result::Result<CheckpointHeader, StreamError<S::Error>> {
    scan_streamed(source, scratch, sink).map(|(header, _)| header)
}

/// The same scan, also handing back the body CRC it accumulated, so a gate binding does not need a
/// second sweep of 65,024 bytes to recompute a value that has already been proved.
fn scan_streamed<S: FileSource, K: EntrySink>(
    source: &mut S,
    scratch: &mut [u8],
    sink: &mut K,
) -> core::result::Result<(CheckpointHeader, u32), StreamError<S::Error>> {
    if scratch.is_empty() {
        return Err(StreamError::Invalid(DecodeError::new(Record::Checkpoint, Reason::Length)));
    }
    let mut scan = Scan::new();
    let mut offset = 0usize;
    while offset < CHECKPOINT_BODY_LEN {
        let take = scratch.len().min(CHECKPOINT_BODY_LEN - offset);
        source.read_span(offset, &mut scratch[..take]).map_err(StreamError::Media)?;
        scan.push(&scratch[..take], sink);
        offset += take;
    }
    Ok(scan.finish()?)
}

/// Proves the whole checkpoint file: its body against its gate at file offset 65,024 (§5).
pub fn validate_file(file: &[u8], slot: u16) -> Result<CheckpointHeader> {
    const R: Record = Record::Checkpoint;
    if file.len() != super::limits::CHECKPOINT_FILE_LEN {
        return Err(DecodeError::new(R, Reason::Length));
    }
    let body = &file[..CHECKPOINT_BODY_LEN];
    let header = validate_body(body)?;
    bind_gate(&file[super::limits::CHECKPOINT_GATE_OFFSET..], slot, &header, body_crc(body))?;
    Ok(header)
}

/// The streamed form of [`validate_file`]: body then gate, staging only `scratch`.
///
/// `scratch` must hold one gate sector, which is 512 bytes — the same floor §6.3's compaction pass
/// already works to. The body CRC comes back with the header because the scan has already proved it
/// and §6.3's recovery decision needs it: re-deriving it would be a second sweep of 65,024 bytes.
pub fn validate_file_streamed<S: FileSource, K: EntrySink>(
    source: &mut S,
    slot: u16,
    scratch: &mut [u8],
    sink: &mut K,
) -> core::result::Result<(CheckpointHeader, u32), StreamError<S::Error>> {
    if scratch.len() < GATE_LEN {
        return Err(StreamError::Invalid(DecodeError::new(Record::Checkpoint, Reason::Length)));
    }
    // The scan already proved the stored CRC equals the fresh one, so the binding's two CRCs are
    // one value here. Recomputing it would mean a second sweep of 65,024 bytes for a comparison
    // that has already been made.
    let (header, crc) = scan_streamed(source, scratch, sink)?;
    source.read_span(super::limits::CHECKPOINT_GATE_OFFSET, &mut scratch[..GATE_LEN]).map_err(StreamError::Media)?;
    bind_gate(&scratch[..GATE_LEN], slot, &header, crc)?;
    Ok((header, crc))
}

fn bind_gate(gate_bytes: &[u8], slot: u16, header: &CheckpointHeader, crc: u32) -> Result<()> {
    let gate = Gate::decode(gate_bytes, MAGIC_CHECKPOINT, slot)?;
    gate.bind(&BodyBinding { stored_crc: crc, fresh_crc: crc, scope: header.epoch, sequence: header.through_sequence })
}

/// The forward scan §5.1's rules are stated once in.
///
/// It walks the body span by span — the header, then every entry of every region in body order, then
/// the zero tail — staging at most one entry (240 bytes) and accumulating the body CRC as it goes.
/// A structural failure is **remembered rather than returned**, so the scan still finishes and
/// [`finish`](Scan::finish) can report the CRC first: §1 puts the checksum ahead of every derived
/// judgement, and a torn body must not be reported as a mis-sorted one.
struct Scan {
    at: usize,
    crc: obc_crc::Crc32,
    stored_crc: u32,
    stage: [u8; MAX_STAGE],
    staged: usize,
    header: Option<CheckpointHeader>,
    error: Option<DecodeError>,
    previous_kind: Option<u16>,
    previous_head: Option<HeadKey>,
    previous_op: Option<[u8; 16]>,
    reserved_rows: usize,
    normal_rows: usize,
    parent_key: Option<[u8; 16]>,
    previous_part: Option<([u8; 16], u16, u64)>,
    previous_generation: Option<u64>,
}

/// The scan's whole entry stage: §5.1's largest entry shape, which is the handoff projection's 240.
const MAX_STAGE: usize = largest_entry();

impl Scan {
    fn new() -> Self {
        Scan {
            at: 0,
            crc: obc_crc::Crc32::new(),
            stored_crc: 0,
            stage: [0; MAX_STAGE],
            staged: 0,
            header: None,
            error: None,
            previous_kind: None,
            previous_head: None,
            previous_op: None,
            reserved_rows: 0,
            normal_rows: 0,
            parent_key: None,
            previous_part: None,
            previous_generation: None,
        }
    }

    fn fail(&mut self, reason: Reason) {
        if self.error.is_none() {
            self.error = Some(DecodeError::new(Record::Checkpoint, reason));
        }
    }

    fn note(&mut self, outcome: Result<()>) {
        if let Err(error) = outcome {
            if self.error.is_none() {
                self.error = Some(error);
            }
        }
    }

    /// Consumes the next contiguous run of body bytes.
    fn push<K: EntrySink>(&mut self, mut bytes: &[u8], sink: &mut K) {
        while !bytes.is_empty() {
            let end = span_end(self.at);
            let take = (end - self.at).min(bytes.len());
            let chunk = &bytes[..take];
            self.crc_push(chunk);
            if end <= REGIONS[REGIONS.len() - 1].end() {
                // The header and every entry are staged; both fit MAX_STAGE by construction.
                self.stage[self.staged..self.staged + take].copy_from_slice(chunk);
            } else if !chunk_tail_is_zero(self.at, chunk) {
                self.fail(Reason::Reserved);
            }
            self.staged += take;
            self.at += take;
            bytes = &bytes[take..];
            if self.at == end {
                self.complete(end, sink);
                self.staged = 0;
            }
        }
    }

    /// Accumulates the CRC with §1's hole: the body's own CRC field counts as four zeros, and its
    /// stored value is captured on the way past.
    fn crc_push(&mut self, chunk: &[u8]) {
        let start = self.at;
        let end = start + chunk.len();
        if end <= CHECKPOINT_BODY_CRC_OFFSET {
            self.crc.update(chunk);
            return;
        }
        let split = CHECKPOINT_BODY_CRC_OFFSET.saturating_sub(start).min(chunk.len());
        self.crc.update(&chunk[..split]);
        for (step, byte) in chunk[split..].iter().enumerate() {
            let position = start + split + step - CHECKPOINT_BODY_CRC_OFFSET;
            // §5's scalars are little-endian, so the field's first byte is its least significant.
            self.stored_crc |= u32::from(*byte) << (8 * position);
        }
        self.crc.update(&[0u8; 4][..chunk.len() - split]);
    }

    /// Judges the span that ends at `end`.
    fn complete<K: EntrySink>(&mut self, end: usize, sink: &mut K) {
        if end == HEADER_LEN {
            match CheckpointHeader::decode(&self.stage[..HEADER_LEN]) {
                Ok(header) => {
                    sink.header(&header);
                    self.header = Some(header);
                }
                Err(error) => self.note(Err(error)),
            }
            return;
        }
        // A header that did not decode leaves every count unknown, so no entry can be judged
        // occupied or absent and none is.
        let Some(header) = self.header else { return };
        for (which, region) in REGIONS.iter().enumerate() {
            if end <= region.offset {
                continue;
            }
            if end <= region.end() {
                let slot = (end - region.offset) / region.entry - 1;
                self.entry(which, slot, &header, sink);
                return;
            }
        }
    }

    fn entry<K: EntrySink>(&mut self, which: usize, slot: usize, header: &CheckpointHeader, sink: &mut K) {
        let occupied = match which {
            REGION_REPOSITORIES => slot < header.repository_count as usize,
            REGION_HEADS => slot < header.head_count as usize,
            REGION_ACTIVE => slot < header.active_count as usize,
            REGION_DRAFT_PARENT => slot < header.draft_parent_count as usize,
            REGION_DRAFT_PARTS => slot < header.draft_part_count as usize,
            REGION_RETAINED => slot < header.retained_count as usize,
            // §5.1's one exception: the ring's start and count select its occupied entries, and it
            // is walked physically here rather than in ring order.
            REGION_RESULTS => {
                (slot + MAX_TERMINAL_RESULTS - header.result_start as usize) % MAX_TERMINAL_RESULTS
                    < header.result_count as usize
            }
            REGION_HANDOFF => slot < header.handoff_count as usize,
            REGION_WEATHER => slot < header.weather_count as usize,
            _ => slot < header.ride_count as usize,
        };
        let len = REGIONS[which].entry;
        if !occupied {
            if !absent(&self.stage[..len]) {
                self.fail(Reason::Count);
            }
            return;
        }
        let bytes = &self.stage[..len];
        match which {
            REGION_REPOSITORIES => match RepositoryState::decode(bytes) {
                Ok(row) => {
                    if let Some(previous) = self.previous_kind {
                        if row.kind < previous {
                            self.fail(Reason::Order);
                        } else if row.kind == previous {
                            self.fail(Reason::Duplicate);
                        }
                    }
                    self.previous_kind = Some(row.kind);
                    sink.repository(&row);
                }
                Err(error) => self.note(Err(error)),
            },
            REGION_HEADS => match CatalogHead::decode(bytes) {
                Ok(row) => {
                    if let Some(previous) = self.previous_head {
                        if row.key < previous {
                            self.fail(Reason::Order);
                        } else if row.key == previous {
                            self.fail(Reason::Duplicate);
                        }
                    }
                    self.previous_head = Some(row.key);
                    sink.head(&row);
                }
                Err(error) => self.note(Err(error)),
            },
            REGION_ACTIVE => match ActiveOperation::decode(bytes) {
                Ok(row) => {
                    let key = row.operation.to_bytes();
                    if let Some(previous) = self.previous_op {
                        if key < previous {
                            self.fail(Reason::Order);
                        } else if key == previous {
                            self.fail(Reason::Duplicate);
                        }
                    }
                    self.previous_op = Some(key);
                    if row.flags & ActiveOperation::FLAG_RESERVED_SLOT != 0 {
                        self.reserved_rows += 1;
                    } else {
                        self.normal_rows += 1;
                    }
                    if self.reserved_rows > RESERVED_ACTIVE_OPERATIONS
                        || self.normal_rows > MAX_NORMAL_ACTIVE_OPERATIONS
                    {
                        self.fail(Reason::Count);
                    }
                    sink.active(&row);
                }
                Err(error) => self.note(Err(error)),
            },
            REGION_DRAFT_PARENT => match DraftParent::decode(bytes) {
                Ok(row) => {
                    self.parent_key = Some(row.parent.to_bytes());
                    sink.draft_parent(&row);
                }
                Err(error) => self.note(Err(error)),
            },
            REGION_DRAFT_PARTS => match DraftPart::decode(bytes) {
                Ok(row) => {
                    // Every part belongs to the one parent row — §2 admits a single parent and §6.1
                    // removes its parts in the same replay step — so a part naming another parent,
                    // or any part with no parent row at all, is not a membership fact this
                    // checkpoint can hold.
                    if self.parent_key != Some(row.key.parent.to_bytes()) {
                        self.fail(Reason::Combination);
                    }
                    let key = row.key.sort_key();
                    if let Some(previous) = self.previous_part {
                        if key < previous {
                            self.fail(Reason::Order);
                        } else if key == previous {
                            self.fail(Reason::Duplicate);
                        }
                    }
                    self.previous_part = Some(key);
                    sink.draft_part(&row);
                }
                Err(error) => self.note(Err(error)),
            },
            REGION_RETAINED => match RetainedPrevious::decode(bytes) {
                Ok(row) => {
                    if let Some(previous) = self.previous_generation {
                        if row.generation.get() < previous {
                            self.fail(Reason::Order);
                        } else if row.generation.get() == previous {
                            self.fail(Reason::Duplicate);
                        }
                    }
                    self.previous_generation = Some(row.generation.get());
                    sink.retained(&row);
                }
                Err(error) => self.note(Err(error)),
            },
            REGION_RESULTS => match TerminalResult::decode(bytes) {
                Ok(row) => sink.result(slot, &row),
                Err(error) => self.note(Err(error)),
            },
            REGION_HANDOFF => match super::handoff::HandoffRef::decode(bytes) {
                Ok(row) => sink.handoff(&row),
                Err(error) => self.note(Err(error)),
            },
            REGION_WEATHER => match WeatherState::decode(bytes) {
                Ok(row) => sink.weather(&row),
                Err(error) => self.note(Err(error)),
            },
            _ => match ActiveRide::decode(bytes) {
                Ok(row) => sink.ride(&row),
                Err(error) => self.note(Err(error)),
            },
        }
    }

    /// The verdict, with §1's precedence: the checksum first, then everything derived from it.
    fn finish(self) -> Result<(CheckpointHeader, u32)> {
        const R: Record = Record::Checkpoint;
        if self.at != CHECKPOINT_BODY_LEN {
            return Err(DecodeError::new(R, Reason::Length));
        }
        let crc = self.crc.finalize();
        if self.stored_crc != crc {
            return Err(DecodeError::new(R, Reason::BodyCrc));
        }
        if let Some(error) = self.error {
            return Err(error);
        }
        self.header.map(|header| (header, crc)).ok_or_else(|| DecodeError::new(R, Reason::Magic))
    }
}

// The region list's own indices, so the scan's match arms name §5.1's regions rather than numbers.
const REGION_REPOSITORIES: usize = 0;
const REGION_HEADS: usize = 1;
const REGION_ACTIVE: usize = 2;
const REGION_DRAFT_PARENT: usize = 3;
const REGION_DRAFT_PARTS: usize = 4;
const REGION_RETAINED: usize = 5;
const REGION_RESULTS: usize = 6;
const REGION_HANDOFF: usize = 7;
const REGION_WEATHER: usize = 8;

/// Where the span containing body offset `at` ends: the header, one entry, or the whole zero tail.
fn span_end(at: usize) -> usize {
    if at < HEADER_LEN {
        return HEADER_LEN;
    }
    let mut index = 0;
    while index < REGIONS.len() {
        let region = REGIONS[index];
        if at < region.end() {
            let slot = (at - region.offset) / region.entry;
            return region.offset + (slot + 1) * region.entry;
        }
        index += 1;
    }
    CHECKPOINT_BODY_LEN
}

/// Whether the tail bytes of `chunk`, which starts at body offset `at`, are the zeros §5.1 requires.
///
/// The body's own CRC field is inside this span and is exempt: it holds the stored checksum.
fn chunk_tail_is_zero(at: usize, chunk: &[u8]) -> bool {
    let end = (at + chunk.len()).min(CHECKPOINT_BODY_CRC_OFFSET);
    if end <= at {
        return true;
    }
    chunk[..end - at].iter().all(|byte| *byte == 0)
}

/// A [`FileSource`] over bytes the caller already holds, so the host path and the device path run
/// the identical scan.
pub struct SliceSource<'a>(pub &'a [u8]);

impl FileSource for SliceSource<'_> {
    type Error = DecodeError;

    fn read_span(&mut self, offset: usize, into: &mut [u8]) -> core::result::Result<(), DecodeError> {
        let end = offset.checked_add(into.len()).ok_or_else(|| DecodeError::new(Record::Checkpoint, Reason::Length))?;
        if end > self.0.len() {
            return Err(DecodeError::new(Record::Checkpoint, Reason::Length));
        }
        into.copy_from_slice(&self.0[offset..end]);
        Ok(())
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A [`FileSource`] that counts what it was asked for, so a test can prove the scan staged
    /// nothing bigger than the scratch it was handed.
    struct Counting<'a> {
        bytes: &'a [u8],
        largest: usize,
        reads: usize,
    }

    impl FileSource for Counting<'_> {
        type Error = DecodeError;

        fn read_span(&mut self, offset: usize, into: &mut [u8]) -> Result<()> {
            self.largest = self.largest.max(into.len());
            self.reads += 1;
            SliceSource(self.bytes).read_span(offset, into)
        }
    }

    fn populated_body() -> std::boxed::Box<[u8; CHECKPOINT_BODY_LEN]> {
        use super::super::{model::CatalogModel, samples};
        let mut model = CatalogModel::initial(samples::STORE, 4);
        // Enough of every region that the scan has entries to order, wrapped results to rotate, and
        // an occupied prefix followed by zeros.
        for step in 1..=5u64 {
            model.apply(&samples::claim(1, step * 2 - 1, 0, [step as u8; 16], step)).unwrap();
            model.apply(&samples::publish(1, step * 2, 0, [step as u8; 16], step, samples::head(1, step))).unwrap();
        }
        let mut body = std::boxed::Box::new([0u8; CHECKPOINT_BODY_LEN]);
        model.encode_body(body.as_mut_slice()).unwrap();
        body
    }

    /// The streamed scan and the slice scan are one implementation, so the streamed one must accept
    /// exactly what the slice one accepts — at **every** scratch size, because the entry stage and
    /// the CRC hole are the two places a chunk boundary could fall wrongly.
    #[test]
    fn the_streamed_scan_agrees_with_the_slice_scan_at_every_scratch_size() {
        let body = populated_body();
        let expected = validate_body(body.as_slice()).expect("the fixture is valid");
        // 512 is the floor a device works to; 3 is deliberately absurd, and lands mid-header,
        // mid-entry and mid-CRC-field; 65,024 is the whole body in one span.
        for size in [3usize, 7, 128, 160, 512, 513, 1_024, 16_384, CHECKPOINT_BODY_LEN] {
            let mut scratch = std::vec![0u8; size];
            let mut source = Counting { bytes: body.as_slice(), largest: 0, reads: 0 };
            let scanned = validate_streamed(&mut source, &mut scratch, &mut ()).expect("the streamed scan agrees");
            assert_eq!(scanned, expected, "scratch {size}");
            assert!(source.largest <= size, "the scan read more than its scratch at {size}");
        }
    }

    /// And it refuses exactly what the slice scan refuses, with the same reason — §1's precedence
    /// included, which is why the scan finishes before it reports rather than returning at the first
    /// structural fault it meets.
    #[test]
    fn the_streamed_scan_refuses_what_the_slice_scan_refuses() {
        // One corruption per region shape: a body CRC, a head that no longer decodes, a head order
        // inversion, an entry past the occupied prefix, and a nonzero tail.
        type Corruption = (&'static str, fn(&mut [u8]));
        let cases: [Corruption; 5] = [
            ("body crc", |body| body[CHECKPOINT_BODY_CRC_OFFSET] ^= 0xFF),
            ("head decode", |body| body[HEADS.slot(0).start + 40] = 0xFF),
            ("head order", |body| {
                let (first, second) = (HEADS.slot(0), HEADS.slot(1));
                let mut swap = [0u8; CatalogHead::LEN];
                swap.copy_from_slice(&body[first.clone()]);
                let mut other = [0u8; CatalogHead::LEN];
                other.copy_from_slice(&body[second.clone()]);
                body[first].copy_from_slice(&other);
                body[second].copy_from_slice(&swap);
            }),
            ("entry past the prefix", |body| body[HEADS.slot(200).start] = 1),
            ("nonzero tail", |body| body[TAIL.start + 17] = 1),
        ];
        for (name, corrupt) in cases {
            let mut body = populated_body();
            corrupt(body.as_mut_slice());
            let sliced = validate_body(body.as_slice());
            let mut scratch = [0u8; 512];
            let streamed = validate_streamed(&mut SliceSource(body.as_slice()), &mut scratch, &mut ());
            match (sliced, streamed) {
                (Err(expected), Err(StreamError::Invalid(got))) => assert_eq!(expected, got, "{name}"),
                (sliced, streamed) => panic!("{name}: sliced {sliced:?}, streamed {streamed:?}"),
            }
        }
    }

    /// The region list is the table in body order, and it tiles without a gap — which is what lets
    /// a forward pass emit region after region and land exactly on the zero tail.
    #[test]
    fn the_region_list_is_the_body_in_order() {
        assert_eq!(REGIONS[0].offset, HEADER_LEN);
        for pair in REGIONS.windows(2) {
            assert_eq!(pair[0].end(), pair[1].offset, "a gap between {:?} and {:?}", pair[0], pair[1]);
        }
        assert_eq!(REGIONS.last().unwrap().end(), TAIL.start);
        // §6.3's staging bound is this number, and §5.1's largest shape is the handoff projection.
        assert_eq!(largest_entry(), super::super::limits::HANDOFF_REF_LEN);
        assert_eq!(largest_entry(), 240);
    }

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

    /// §2 admits one draft parent and §6.1 removes its parts in the same replay step that removes
    /// it, so every part row belongs to that parent. A part with no parent row, or one naming
    /// another parent, is a membership fact the checkpoint cannot hold.
    #[test]
    fn draft_parts_must_belong_to_the_one_parent_row() {
        use super::super::samples;
        let mut body = std::boxed::Box::new([0u8; CHECKPOINT_BODY_LEN]);
        let mut model = super::super::model::CatalogModel::initial(samples::STORE, 4);

        // A part with no parent row at all.
        let _ = model.draft_parts.push(samples::part(1));
        model.encode_body(body.as_mut_slice()).unwrap();
        assert_eq!(validate_body(body.as_slice()).unwrap_err().reason, Reason::Combination);

        // With its parent present it decodes.
        model.draft_parent = Some(samples::parent());
        model.encode_body(body.as_mut_slice()).unwrap();
        validate_body(body.as_slice()).unwrap();

        // A part naming another parent does not.
        model.draft_parts[0].key.parent = obc_link::ids::OperationId::new(samples::OP_B);
        model.encode_body(body.as_mut_slice()).unwrap();
        assert_eq!(validate_body(body.as_slice()).unwrap_err().reason, Reason::Combination);
    }

    /// §5.2's "sequences never wrap", enforced where it can still be enforced: a header whose
    /// through-sequence cannot carry a full journal of successors is refused, so every later
    /// addition of a slot index to it is total.
    #[test]
    fn a_through_sequence_that_cannot_carry_a_journal_is_rejected() {
        let mut header = CheckpointHeader {
            store: StoreId::new([0x3C; 16]),
            epoch: 1,
            through_sequence: u64::MAX - super::super::limits::JOURNAL_SLOTS as u64,
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
        assert_eq!(CheckpointHeader::decode(&header.encode()).unwrap().through_sequence, header.through_sequence);
        header.through_sequence += 1;
        assert_eq!(CheckpointHeader::decode(&header.encode()).unwrap_err().reason, Reason::Overflow);
    }
}
