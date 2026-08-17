//! The kernel-backed transaction: the wire engine's effect seam, executed against the real store.
//!
//! `obc-link`'s [`Engine`] does no storage I/O. It hands out a [`Command`] and takes back an
//! [`Outcome`], and everything between those two is somebody else's job. In the harness that job is
//! done by an in-memory `FakeTransaction`; here it is done by the OBC2 kernel — the journal, the
//! catalog projection, the generation writer, the lease table, and the media under all three.
//!
//! [`Engine`]: obc_link::engine::Engine
//!
//! ## Why the bridge lives here
//!
//! `obc-link` is a transport-free, storage-free codec and must stay one: it declares the
//! [`Transaction`] seam and never learns what a card is. `obc-storage` already depends on it for
//! the identity types, so the implementation belongs on this side of that edge — the same direction
//! `obc-storage -> obc-formats` runs, and the same shape the harness's fake takes.
//!
//! ## What one command costs
//!
//! - **Lookup** reads the projection. It writes nothing, which is what §11 requires of the
//!   idempotency lookup that precedes preflight.
//! - **Claim** runs §11's admission — capacity, space, and the lazy shard creation §12 puts on the
//!   path that is about to create a leaf — and then appends one `Claim` journal record. That record
//!   is the first mutation and precedes every side effect.
//! - **Append** goes straight to the payload file through [`GenerationWriter`]. Nothing is
//!   synchronized: the restart-only profile of §6.1 acknowledges no offset, so there is no durable
//!   point to owe.
//! - **Checkpoint** synchronizes the payload prefix and reports it. It is a progress fact only —
//!   §13's teardown still durably aborts the work — and it is what makes `CheckpointUpload`'s
//!   answer true rather than merely well-formed.
//! - **Seal** proves the declared length and CRC and writes the sealed `WORK` slot.
//! - **Validate** is the typed-validator seam. Domain validators arrive with DOS5; what is here is
//!   the hook and the refusal shape, in the kind's own detail namespace.
//! - **Publish** rechecks the compare-and-swap under the commit lock and appends **one** `Terminal`
//!   record carrying the head, the repository revision, the active-row removal and the retained
//!   result together. §11's "logical publication and OperationResult in one commit" is that record.
//! - **Abort** appends the terminal `Aborted` record — and is a **no-op against a claim that is
//!   already terminal**, because a spent identifier must not be spent twice.
//! - **Resolve / ReadSource / ReleaseLease** are §7's download: one lease pinned at the generation
//!   the resolve landed on, reads from that generation however often the head is replaced
//!   underneath it, and exactly one release.
//!
//! ## What is deliberately not here
//!
//! Compaction and the incremental garbage collector. Both are store-level background passes with
//! their own cursors and their own budgets; a transaction that ran them inside a command would make
//! one client's `FinishUpload` pay for another's garbage. They stay with the store #1359 builds.
//! What *is* here is the eager collection of a generation this transaction itself abandoned, which
//! no collector needs to discover.

use obc_link::control::{ClockStatus, ConfigBlock, DeviceStatus};
use obc_link::engine::{
    AbortCause, ClaimIntent, ClaimOutcome, Command, DeviceControlAnswer, DeviceControlRequest, FailureCause,
    IntentMetadata, OperationReport, Outcome, PinnedSource, PrincipalScope, TerminalError, Transaction,
};
use obc_link::error::detail;
use obc_link::frame::Opcode;
use obc_link::ids::{DraftPartRef, GenerationId, LogicalObjectId, OperationId, Revision, SessionId, StoreId};
use obc_link::query::{progress_flags, OperationProgress};
use obc_link::registry::{AbortReason, ObjectKind, ObjectOutcome, Phase, SubjectNamespace};
use obc_link::result::{AbortDisposition, AbortResult, ObjectResult, ResultEnvelope};
use obc_link::upload::Target;
use obc_link::ErrorBody;

use super::checkpoint;
use super::commit::{ChangeKind, CommitEvent};
use super::compaction::{self, CardHeadFields, CheckpointPass, CompactionError};
use super::entries::{
    ActiveOperation, CatalogHead, HeadKey, OperationPhase, ResultType, RetainedPrevious, TerminalResult,
};
use super::generation::{Capability, GenerationMedia, GenerationWriter, Intent, WriteError};
use super::index::{HeadIndexEntry, RamIndex, ResultIndexEntry, NO_JOURNAL_SLOT};
use super::journal::{Change, JournalBody, Mutation, RecordKind, RepositoryChange};
use super::leases::{LeaseHandle, LeaseTable, ReleaseEffect};
use super::limits::{
    GATE_LEN, JOURNAL_BODY_LEN, JOURNAL_COMPACTION_TRIGGER, MAX_ACTIVE_OPERATIONS, MAX_NORMAL_ACTIVE_OPERATIONS,
    MAX_TERMINAL_RESULTS, SECTOR, SLOT_STRIDE,
};
use super::work::Subject;

/// The §5.1 ceiling on normal claimed operations, re-exported so a caller can size against it.
pub const ACTIVE_CLAIMS: usize = MAX_NORMAL_ACTIVE_OPERATIONS;

/// The §8.1 retained-result window.
pub const RESULT_RING: usize = MAX_TERMINAL_RESULTS;

/// The eight bytes a head reserves when **no repository derived a projection for it**.
///
/// §5.3 gives a head an eight-to-ninety-six byte canonical envelope and refuses a shorter one, so
/// there has to be something. What there is not, any more, is a kernel that *invents* the head's
/// metadata: the envelope a head carries is now whatever the [`Validator`] returned, and this is
/// only what a store running [`AcceptEverything`] — a device with no domain repositories at all —
/// leaves behind. A device-local publication that names no operation is the other user of it.
const PLACEHOLDER_ENVELOPE: [u8; 8] = [0; 8];

// ---------------------------------------------------------------------------------------------
// The seams
// ---------------------------------------------------------------------------------------------

/// The media a store transaction addresses, over and above one generation's files.
///
/// [`GenerationMedia`] already names what §7's writer needs of the *current* generation. This adds
/// what a whole store needs: the journal it commits through, the ability to address a generation
/// other than the one being written, and the two facts admission and collection ask for.
///
/// It is a seam rather than a concrete type for the same reason `GenerationMedia` is: the crash
/// matrix drives it against the faulting harness, and the board drives the same transaction against
/// FAT through the §13.1 adapter.
pub trait KernelMedia: GenerationMedia {
    /// Appends one journal record into `slot`.
    ///
    /// The implementation owns the ordering §1 fixes and the §1 exemption that goes with it: the
    /// whole 16,384-byte stride is written **with its gate sector zeroed**, synchronized, and only
    /// then is the gate written and synchronized. Writing the stride rather than the body is not
    /// padding for its own sake — a slot that was torn once holds garbage across its whole program
    /// page, and a reader rejects a nonzero pad.
    fn append_journal(
        &mut self,
        slot: u16,
        body: &[u8; JOURNAL_BODY_LEN],
        gate: &[u8; GATE_LEN],
    ) -> Result<(), Self::Error>;

    /// Makes `generation` the payload and `WORK` pair every [`GenerationMedia`] method addresses.
    fn open_generation(&mut self, generation: GenerationId) -> Result<(), Self::Error>;

    /// Reads from any generation's payload, including one a later publication has displaced.
    ///
    /// This is what makes a lease worth holding (§9): the reader keeps reading the bytes it pinned
    /// however often the head above them is replaced.
    fn read_generation(&mut self, generation: GenerationId, offset: u64, into: &mut [u8])
        -> Result<usize, Self::Error>;

    /// Deletes a generation's `GEN`/`WORK` pair. Either ordering recovers as orphan cleanup (§9).
    fn collect_generation(&mut self, generation: GenerationId) -> Result<(), Self::Error>;

    /// The free bytes §11's admission reserves against.
    fn free_bytes(&mut self) -> u64;

    /// Destroys the store and recreates its fixed files under `store` (§16's ResetStore).
    fn reset_store(&mut self, store: StoreId) -> Result<(), Self::Error>;

    // -- §13's on-demand re-reads ---------------------------------------------------------------
    //
    // §13 leaves three things on the card — a head's catalog-projection envelope, its resolution
    // `GenerationId` with the flag that travels with it, and a terminal result's 208-byte body —
    // and says they "are re-read on demand through the mounted-file budget". These two primitives
    // are that budget; everything above them is a provided method, so a media supplies two bounded
    // reads and gets §6.3's newest-source rule for free rather than reimplementing it.

    /// Reads `into` from the file §6.3 selected as the **active** checkpoint.
    fn read_checkpoint(&mut self, offset: usize, into: &mut [u8]) -> Result<(), Self::Error>;

    /// Reads and validates one journal slot, `None` when it holds no valid record.
    ///
    /// The whole 16,384-byte stride is read, not the 2,048 bytes the body occupies: §6 makes the
    /// pad part of what a valid record proves, and reading less and zero-filling the rest would
    /// falsify that check rather than defer it.
    fn read_record(&mut self, slot: u16) -> Result<Option<JournalBody>, Self::Error>;

    /// The two card-resident fields of one head, from §6.3's newest source.
    ///
    /// "The catalog-projection envelope and the resolution `GenerationId` with its flag come from
    /// the journal's carried head entry when a head-putting record has been replayed since the
    /// active checkpoint … otherwise all of them are copied across from the active checkpoint's
    /// stored bytes by one bounded read."
    fn head_fields(&mut self, entry: &HeadIndexEntry) -> Result<CardHeadFields, RereadError<Self::Error>> {
        if entry.journal_slot != NO_JOURNAL_SLOT {
            let record = self.read_record(entry.journal_slot).map_err(RereadError::Media)?;
            let Some(Change::Put(head)) = record.and_then(|body| body.mutation.head) else {
                return Err(RereadError::Missing);
            };
            if head.key != entry.key() {
                return Err(RereadError::Missing);
            }
            return Ok(CardHeadFields::of(&head));
        }
        // The heads region is sorted by `(kind, logical id)` and its occupancy is in the header, so
        // the stored entry is found by bisection: the header read plus at most eight 160-byte
        // probes, rather than a scan of 256.
        let mut header = [0u8; checkpoint::HEADER_LEN];
        self.read_checkpoint(0, &mut header).map_err(RereadError::Media)?;
        let header = checkpoint::CheckpointHeader::decode(&header).map_err(|_| RereadError::Missing)?;
        let mut stage = [0u8; CatalogHead::LEN];
        let (mut low, mut high) = (0usize, header.head_count as usize);
        while low < high {
            let middle = low + (high - low) / 2;
            self.read_checkpoint(checkpoint::HEADS.slot(middle).start, &mut stage).map_err(RereadError::Media)?;
            let stored = CatalogHead::decode(&stage).map_err(|_| RereadError::Missing)?;
            match stored.key.cmp(&entry.key()) {
                core::cmp::Ordering::Less => low = middle + 1,
                core::cmp::Ordering::Greater => high = middle,
                core::cmp::Ordering::Equal => return Ok(CardHeadFields::of(&stored)),
            }
        }
        Err(RereadError::Missing)
    }

    /// The 208 stored bytes of one terminal result, from the same newest source.
    ///
    /// `physical` is the ring index the entry occupies in the checkpoint — the ring's start and
    /// count are resident, so a result does not move when the checkpoint is rewritten.
    fn result_entry(
        &mut self,
        physical: usize,
        key: &ResultIndexEntry,
    ) -> Result<[u8; TerminalResult::LEN], RereadError<Self::Error>> {
        let mut stage = [0u8; TerminalResult::LEN];
        if key.journal_slot != NO_JOURNAL_SLOT {
            // A result appended since the active checkpoint exists in no checkpoint at all, so
            // "re-read from card" can only mean re-read from the record that appended it.
            let record = self.read_record(key.journal_slot).map_err(RereadError::Media)?;
            let Some(result) = record.and_then(|body| body.mutation.result) else {
                return Err(RereadError::Missing);
            };
            if result.operation != key.operation || result.commit_sequence != key.commit_sequence {
                return Err(RereadError::Missing);
            }
            stage.copy_from_slice(&result.encode());
            return Ok(stage);
        }
        self.read_checkpoint(checkpoint::RESULTS.slot(physical).start, &mut stage).map_err(RereadError::Media)?;
        Ok(stage)
    }

    // -- §6.3's compaction, at the media seam ---------------------------------------------------

    /// §6.3 step 2: invalidate the **inactive** checkpoint's gate and synchronize.
    fn begin_checkpoint(&mut self) -> Result<(), Self::Error>;

    /// §6.3 step 3: write one 512-byte sector of that checkpoint's body.
    fn write_checkpoint_sector(&mut self, offset: usize, sector: &[u8; SECTOR]) -> Result<(), Self::Error>;

    /// §6.3 step 4: synchronize the body, write and synchronize the gate over the epoch, sequence
    /// and body CRC the pass produced, and make the file just written the active checkpoint.
    ///
    /// The gate's own `slot` field is the checkpoint file's index, which is a media fact — this side
    /// of the seam is the only one that knows which of the pair it just wrote.
    fn finish_checkpoint(&mut self, epoch: u64, through_sequence: u64, body_crc: u32) -> Result<(), Self::Error>;
}

/// §6.3's step 3 driven against a [`KernelMedia`]: the index's fixed fields, the card's everything
/// else, and the inactive checkpoint's sectors as the destination.
///
/// It exists so [`compaction::materialize`] — which is written against nothing but its own three
/// sources — is reached end to end from the real store rather than only from a host fixture.
struct Pass<'m, M: KernelMedia> {
    media: &'m mut M,
}

impl<M: KernelMedia> CheckpointPass for Pass<'_, M> {
    type Error = RereadError<M::Error>;

    fn head_fields(&mut self, entry: &HeadIndexEntry) -> Result<CardHeadFields, Self::Error> {
        self.media.head_fields(entry)
    }

    fn result_entry(
        &mut self,
        physical: usize,
        key: &ResultIndexEntry,
    ) -> Result<[u8; TerminalResult::LEN], Self::Error> {
        self.media.result_entry(physical, key)
    }

    fn write_body_sector(&mut self, offset: usize, sector: &[u8; SECTOR]) -> Result<(), Self::Error> {
        self.media.write_checkpoint_sector(offset, sector).map_err(RereadError::Media)
    }
}

/// Why an on-demand re-read of a card-resident field did not produce one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RereadError<E> {
    /// A bounded read of the active checkpoint or of a journal slot failed.
    Media(E),
    /// The card does not hold what the index says it does — an invalid slot where a reference
    /// points, a head the checkpoint's region has no entry for, a result whose stored identity is
    /// not the one the ring names. It is a kernel invariant break rather than a client error: RAM
    /// and card must never be able to disagree about a fact one of them derived from the other.
    Missing,
}

/// A bounded reader over the bytes a validator is judging.
///
/// It is what the sealed generation looks like to a repository, and it is deliberately *only* this:
/// §2 of the system contract keeps the storage-private `GenerationId` and every path on the far side
/// of the repository seam, so a validator is handed the ability to read the bytes and never the name
/// of the thing holding them. `None` is a medium that refused; a short read is the end of the
/// payload.
pub trait SealedBytes {
    /// Reads into `into` at `offset`, returning how many bytes the payload had there.
    fn read_at(&mut self, offset: u64, into: &mut [u8]) -> Option<usize>;
}

/// The bytes a validator is judging, and the metadata that came with them.
#[derive(Debug, Clone, Copy)]
pub struct Validation<'a> {
    /// The kind whose repository owns the rules.
    pub kind: ObjectKind,
    /// Which mutation is being validated: `StartUpload`, `SetMetadata`, or one of the commands that
    /// changes no head at all.
    pub opcode: Opcode,
    /// The canonical envelope the request declared — a Put envelope, or a `SetMetadata` patch — and
    /// empty for an opcode that declares neither.
    pub metadata: &'a [u8],
    /// The catalog projection the head currently carries, for a mutation that has one to patch.
    pub current: Option<&'a [u8]>,
    /// The sealed payload length; zero for a direct mutation.
    pub length: u64,
    /// The sealed payload CRC; zero for a direct mutation.
    pub crc: u32,
}

/// The canonical catalog-projection envelope a repository derived for a head (§5.3, registries §4.3).
///
/// It is a *result*, not an input: the registries make every catalog field validator-derived, so the
/// only code allowed to say what a head's metadata is, is the repository that just validated the
/// payload it describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CatalogProjection {
    bytes: [u8; CatalogHead::ENVELOPE_CAPACITY],
    len: u16,
}

impl CatalogProjection {
    /// The minimum well-formed reservation, for a store with no repository for this kind.
    pub const RESERVATION: Self = {
        let mut bytes = [0u8; CatalogHead::ENVELOPE_CAPACITY];
        let mut at = 0;
        while at < PLACEHOLDER_ENVELOPE.len() {
            bytes[at] = PLACEHOLDER_ENVELOPE[at];
            at += 1;
        }
        CatalogProjection { bytes, len: PLACEHOLDER_ENVELOPE.len() as u16 }
    };

    /// Takes `bytes` as the projection, or `None` when §5.3 would refuse the length.
    pub fn of(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < CatalogHead::MIN_ENVELOPE as usize || bytes.len() > CatalogHead::ENVELOPE_CAPACITY {
            return None;
        }
        let mut this = CatalogProjection { bytes: [0; CatalogHead::ENVELOPE_CAPACITY], len: bytes.len() as u16 };
        this.bytes[..bytes.len()].copy_from_slice(bytes);
        Some(this)
    }

    /// The canonical bytes.
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.len as usize]
    }

    /// The declared length §5.3 stores beside the reservation.
    pub const fn len(&self) -> u16 {
        self.len
    }

    /// Never: §5.3's minimum is eight bytes.
    pub const fn is_empty(&self) -> bool {
        false
    }
}

/// The typed validator §6.3 runs over sealed bytes, before publication and never before the seal.
///
/// It is the one seam a concrete repository reaches the commit path through, and it does two jobs
/// that §2 gives the same owner: it decides whether the bytes are admissible at all — `Err(detail)`
/// is the kind-scoped semantic detail, which is what makes a rejection legible to a client that has
/// never heard of this device's repositories — and it derives the catalog projection the published
/// head will carry. Neither job is the kernel's: the kernel owns byte counts, CRCs, ordering,
/// publication and recovery, and never parses a domain payload.
pub trait Validator {
    /// Validates one mutation and derives the projection its head should carry.
    fn validate(&mut self, subject: &Validation<'_>, bytes: &mut dyn SealedBytes) -> Result<CatalogProjection, u16>;
}

/// The validator a device without domain rules runs: every sealed generation is admissible, and
/// every head it publishes carries §5.3's bare reservation because nothing here can derive more.
#[derive(Debug, Default, Clone, Copy)]
pub struct AcceptEverything;

impl Validator for AcceptEverything {
    fn validate(&mut self, _subject: &Validation<'_>, _bytes: &mut dyn SealedBytes) -> Result<CatalogProjection, u16> {
        Ok(CatalogProjection::RESERVATION)
    }
}

/// The sealed generation, as the [`SealedBytes`] a validator reads.
struct SealedGeneration<'m, M: KernelMedia> {
    media: &'m mut M,
    generation: GenerationId,
    length: u64,
}

impl<M: KernelMedia> SealedBytes for SealedGeneration<'_, M> {
    fn read_at(&mut self, offset: u64, into: &mut [u8]) -> Option<usize> {
        let Some(remaining) = self.length.checked_sub(offset) else { return Some(0) };
        let wanted = into.len().min(remaining.min(into.len() as u64) as usize);
        if wanted == 0 {
            return Some(0);
        }
        self.media.read_generation(self.generation, offset, &mut into[..wanted]).ok().map(|read| read.min(wanted))
    }
}

/// The bytes a direct mutation has: none. It sealed no generation of its own.
struct NoBytes;

impl SealedBytes for NoBytes {
    fn read_at(&mut self, _offset: u64, _into: &mut [u8]) -> Option<usize> {
        Some(0)
    }
}

/// The policy points a transaction consults that are neither media nor domain validation.
///
/// Every method has a default, so production wiring passes [`NoHooks`] and the compiler removes
/// them. They exist because the engine's unwind paths must be exercisable against the *real*
/// transaction — a media failure at exactly one step, a compare-and-swap that loses a race — and
/// injecting that through a fault plan several layers down would prove less than it appears to.
pub trait Hooks {
    /// Refuses admission before any state exists (§11's "fail without creating state").
    fn admit_claim(&mut self) -> Option<FailureCause> {
        None
    }

    /// Fails the append that would end at `would_reach`.
    fn appending(&mut self, would_reach: u64) -> Option<FailureCause> {
        let _ = would_reach;
        None
    }

    /// Fails the seal.
    fn sealing(&mut self) -> Option<FailureCause> {
        None
    }

    /// Fails the publication.
    fn publishing(&mut self) -> Option<FailureCause> {
        None
    }

    /// Fails the terminal record an abort writes.
    fn aborting(&mut self) -> Option<FailureCause> {
        None
    }

    /// True when a device-local producer commits a competing mutation just before the commit lock.
    fn races_publication(&mut self) -> bool {
        false
    }

    /// One durable catalog commit happened (§4's `CommitEvent`).
    ///
    /// It is called from the commit path **after** the record's validity gate is synchronized and
    /// never before — §4 fixes that ordering and nothing else about delivery — and it is called for
    /// a device-local producer's publication exactly as for a client's, because §4 gives both the
    /// same path. A `Hooks` that does nothing with it is a device with no consumer; the store's own
    /// [`CommitLog`](super::commit::CommitLog) is the one that keeps the latest revision per
    /// repository.
    fn committed(&mut self, event: super::commit::CommitEvent) {
        let _ = event;
    }

    /// The identity a `ResetStore` mints (§16). The default derives it from the store it replaces,
    /// so a device that never overrides this still never reuses a StoreId.
    fn mint_store_id(&mut self, previous: StoreId) -> StoreId {
        let mut bytes = *previous.as_bytes();
        for byte in bytes.iter_mut().rev() {
            let (next, carried) = byte.overflowing_add(1);
            *byte = next;
            if !carried {
                break;
            }
        }
        StoreId::new(bytes)
    }
}

/// The hooks a device runs: none.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoHooks;

impl Hooks for NoHooks {}

// ---------------------------------------------------------------------------------------------
// Resident state
// ---------------------------------------------------------------------------------------------

/// The RAM half of one live claim: what §8.1 projects and what no record makes durable.
///
/// Under the restart-only profile the durable claim carries the phase it was made in and nothing
/// else moves it: there is no `Work` record per append, because there is no offset a client may
/// resume from. So the phase a query reports is resident, exactly as the offset is.
#[derive(Debug, Clone, Copy)]
struct Live {
    operation: OperationId,
    phase: Phase,
    /// Whether a stream session is attached to this claim (§8.1's progress bit 1).
    ///
    /// It is a fact of its own, not a function of the phase, and it is **resident**: §8.1 sets the
    /// bit "only while that session exists", and a session exists inside one connection. So a claim
    /// this store finds at mount has no attachment however far its durable phase had got, and an
    /// abort the medium refused has none either even though the claim it left is still live.
    attached: bool,
    durable_offset: u64,
    checkpoint_sequence: u32,
    /// The operation an `AbortOperation` command names (§6.4).
    ///
    /// It is resident rather than durable because §5.3's active row has no field for it and because
    /// nothing needs it after the command's own terminal record: a mount that finds an unfinished
    /// abort command abandons it, and abandoning needs no target.
    target: Option<OperationId>,
    /// What an `AbortOperation` command found when it reached its target (§6.4).
    disposition: Option<AbortDisposition>,
    /// The metadata envelope the claim declared, kept for the validator that will read it.
    ///
    /// It is **resident**, and that is sound for exactly the reason the phase above is: the envelope
    /// is part of §11's canonical intent, so a claim this store finds at mount can only be driven
    /// forward by a request carrying a byte-identical envelope, which re-supplies it. There is no
    /// state a cut can lose here that the intent does not carry back.
    metadata: IntentMetadata,
    /// The catalog projection the repository derived at `Validate`, for `Publish` to store.
    ///
    /// Kept per claim rather than in one slot because §5.1 admits nine live claims and any of them
    /// may be between its validation and its publication.
    projection: Option<CatalogProjection>,
}

/// Whether a terminal commit moves the repository revision.
///
/// §10 reports a revision on every `ObjectResult`, but only a command that changed a head has moved
/// one. `InstallUpdate` and `AcknowledgeRideImported` change no head: they hand work to the boot
/// path and acknowledge an import, and a revision that advanced for either would tell every other
/// client that something it can see has changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Bump {
    /// A head changed; the repository revision advances with it.
    Yes,
    /// Nothing a client can see changed.
    No,
}

/// The generation one upload is writing.
struct Writing {
    operation: OperationId,
    writer: GenerationWriter,
    capability: Capability,
    generation: GenerationId,
    sealed: bool,
}

/// A pinned download source and the lease that holds it.
#[derive(Debug, Clone, Copy)]
struct Pinned {
    generation: GenerationId,
    handle: LeaseHandle,
}

/// The kernel-backed transaction.
///
/// It is big — the catalog projection alone is around 56 KiB — and deliberately owns its state
/// inline rather than behind a pointer, because the board places it once in static storage and
/// never moves it. A host test boxes it.
pub struct KernelTransaction<M: KernelMedia, V: Validator = AcceptEverything, H: Hooks = NoHooks> {
    media: M,
    validator: V,
    hooks: H,
    index: RamIndex,
    /// The journal cursor: the next sequence and the slot it lands in.
    sequence: u64,
    /// The `through_sequence` of the checkpoint this epoch was opened by (§6.3).
    ///
    /// Physical journal slot `i` carries sequence `epoch_base + i + 1`, so the slot a commit lands
    /// in is a *relative* index — and it is relative to the checkpoint, not to the store. Compaction
    /// moves it to the sequence it materialized, which is what makes the next record land at slot
    /// zero of the new epoch instead of running off the end of the ring.
    epoch_base: u64,
    /// The store-wide monotone revision every publication stamps.
    revision: u64,
    /// The store-wide logical-id cursor.
    next_logical_id: u64,
    leases: LeaseTable,
    pinned: Option<Pinned>,
    /// The synthetic connection identity leases are taken under. The engine owns the real one; the
    /// store only needs the two to be distinguishable, and this transaction serves one at a time.
    lease_connection: u32,
    live: [Option<Live>; MAX_ACTIVE_OPERATIONS],
    writing: Option<Writing>,
    /// The 16 KiB stride buffer a seal assembles its `WORK` slot in. The board hands this in from
    /// the scratch arena; here it is owned so the transaction is one value.
    stride: [u8; SLOT_STRIDE],
    /// The device-control plane's config block (§16).
    pub config: ConfigBlock,
    /// The device-control plane's status.
    pub status: DeviceStatus,
    /// The device-control plane's clock.
    pub clock: ClockStatus,
}

impl<M: KernelMedia, V: Validator, H: Hooks> KernelTransaction<M, V, H> {
    /// Opens a transaction over a store whose resident index is `index`.
    ///
    /// `epoch_base` is the `through_sequence` of the checkpoint that index was mounted from — the
    /// origin §6.3 maps physical journal slots against. A store that has never compacted has zero.
    pub fn mount(media: M, validator: V, hooks: H, index: RamIndex, epoch_base: u64) -> Self {
        let store = index.store;
        let mut this = KernelTransaction {
            media,
            validator,
            hooks,
            index,
            sequence: 1,
            epoch_base: 0,
            revision: 0,
            next_logical_id: 1,
            leases: LeaseTable::new(),
            pinned: None,
            lease_connection: 0,
            live: [None; MAX_ACTIVE_OPERATIONS],
            writing: None,
            stride: [0; SLOT_STRIDE],
            config: initial_config(),
            status: initial_status(store),
            clock: initial_clock(),
        };
        this.rebind(epoch_base);
        this
    }

    /// Mounts into storage the caller already owns, over a projection that is not yet loaded.
    ///
    /// ## Why this exists, measured
    ///
    /// [`mount`](Self::mount) takes the projection by value and returns this whole value by value.
    /// Both are large — the projection alone is around 56 KiB and the transaction around 73 KiB —
    /// so placing one in a board's `.bss` through it costs **206,080 bytes of transient stack**,
    /// measured on the nRF54L with a painted stack. The shipping image's residual main stack is
    /// **51,576 bytes**. A device that mounted a store that way would not fault at some future
    /// depth; it would fault during the mount.
    ///
    /// So this writes each field into the caller's slot directly and never materializes a
    /// `KernelTransaction` anywhere. The projection starts empty: a mount decodes the selected
    /// checkpoint straight into [`model_mut`](Self::model_mut) — through
    /// [`media_and_model_mut`](Self::media_and_model_mut), which hands out the media that reads it
    /// and the projection it is read into together — and then calls [`rebind`](Self::rebind) to
    /// derive the cursors from what landed.
    ///
    /// The host path keeps [`mount`](Self::mount): a test boxes the value and the copies cost
    /// nothing that matters.
    pub fn mount_in_place(
        slot: &mut core::mem::MaybeUninit<Self>,
        media: M,
        validator: V,
        hooks: H,
        store: StoreId,
    ) -> &mut Self {
        let at = slot.as_mut_ptr();
        // SAFETY: every field of `Self` is written exactly once below, through a raw pointer into
        // the caller's uninitialized slot, and none of them is read before it is written.
        //
        // "Exhaustive" is the whole safety argument, and prose does not enforce it. Two things do:
        // `SIZE_32`/`SIZE_64` below, which any layout change trips, and
        // `every_field_is_named_by_the_in_place_constructor` in this module's tests, which
        // destructures the struct by name so a field added to it cannot compile until someone has
        // looked at this list.
        unsafe {
            core::ptr::addr_of_mut!((*at).media).write(media);
            core::ptr::addr_of_mut!((*at).validator).write(validator);
            core::ptr::addr_of_mut!((*at).hooks).write(hooks);
            // Not `.write(RamIndex::new(store))`: that is the return-slot temporary this
            // constructor exists to avoid, and it would put the whole of it back.
            RamIndex::init_empty(&mut *core::ptr::addr_of_mut!((*at).index).cast(), store);
            core::ptr::addr_of_mut!((*at).sequence).write(1);
            core::ptr::addr_of_mut!((*at).epoch_base).write(0);
            core::ptr::addr_of_mut!((*at).revision).write(0);
            core::ptr::addr_of_mut!((*at).next_logical_id).write(1);
            core::ptr::addr_of_mut!((*at).leases).write(LeaseTable::new());
            core::ptr::addr_of_mut!((*at).pinned).write(None);
            core::ptr::addr_of_mut!((*at).lease_connection).write(0);
            core::ptr::addr_of_mut!((*at).live).write([None; MAX_ACTIVE_OPERATIONS]);
            core::ptr::addr_of_mut!((*at).writing).write(None);
            core::ptr::addr_of_mut!((*at).stride).write([0; SLOT_STRIDE]);
            core::ptr::addr_of_mut!((*at).config).write(initial_config());
            core::ptr::addr_of_mut!((*at).status).write(initial_status(store));
            core::ptr::addr_of_mut!((*at).clock).write(initial_clock());
            slot.assume_init_mut()
        }
    }

    /// The media and the projection together, so a mount can read one into the other.
    ///
    /// They are handed out as a pair because that is the only way to have both: a store's mount
    /// reads a checkpoint *through its own media* into *its own projection*, and two separate
    /// accessors could not be held at once.
    pub fn media_and_index_mut(&mut self) -> (&mut M, &mut RamIndex) {
        (&mut self.media, &mut self.index)
    }

    /// Derives the journal cursor and the two identity cursors from the projection now in place.
    ///
    /// **Call this after the projection is loaded and before the first command.** A transaction
    /// built by [`mount_in_place`](Self::mount_in_place) starts on an empty projection, so its
    /// cursors are the empty store's; a mount that decoded a real checkpoint through
    /// [`media_and_model_mut`](Self::media_and_model_mut) and skipped this would append its next
    /// journal record at sequence one, over a slot the store is still replaying, and hand out a
    /// `LogicalObjectId` some existing head already owns. [`mount`](Self::mount) calls it for the
    /// caller, which is why the by-value path needs no such note.
    ///
    /// §6.3: the next record's sequence is the projection's `through_sequence` plus one. The
    /// revision and logical-id cursors are the maxima the repository rows carry, which is what makes
    /// a remount continue the store rather than restart it.
    pub fn rebind(&mut self, epoch_base: u64) {
        self.epoch_base = epoch_base;
        self.sequence = self.index.through_sequence.saturating_add(1);
        self.revision = self.index.repositories.iter().map(|row| row.revision.get()).max().unwrap_or(0);
        self.next_logical_id =
            self.index.repositories.iter().map(|row| row.next_logical_id.get()).max().unwrap_or(0).max(1);
        self.status.store_id = self.index.store;
    }

    /// True once the journal has reached §6.3's compaction trigger: "before accepting a 193rd
    /// record in one epoch, `CardStore` blocks new mutations and compacts".
    ///
    /// [`commit`](Self::commit) consults it and runs [`compact`](Self::compact) itself, so this is a
    /// predicate a caller may observe rather than a duty a caller has.
    pub fn compaction_required(&self) -> bool {
        self.next_slot() >= JOURNAL_COMPACTION_TRIGGER as u64
    }

    /// The physical journal slot the next record lands in (§6.3: slot `i` carries sequence
    /// `epoch_base + i + 1`).
    fn next_slot(&self) -> u64 {
        self.sequence.saturating_sub(self.epoch_base + 1)
    }

    /// The store's identity.
    pub fn store_id(&self) -> StoreId {
        self.index.store
    }

    /// Runs §6.3's compaction cycle: steps 2 through 4, over the index and the card.
    ///
    /// Step 1 — "apply all valid records through sequence `S` in memory" — has already happened:
    /// every record this store wrote or replayed is in the index, and `through_sequence` is that
    /// `S`. What is left is the ordering, and it is the whole of this function:
    ///
    /// 2. invalidate and synchronize the inactive checkpoint's gate;
    /// 3. write its complete body at epoch `E + 1` and through-sequence `S`, in one bounded forward
    ///    pass that stages [`compaction::STAGING_BYTES`] and never the projection;
    /// 4. write and synchronize its `O2CG` gate, which is the moment the new epoch exists.
    ///
    /// Step 5 is the next commit: `epoch_base` moves to `S`, so the record after this one lands at
    /// physical slot zero, and every old-epoch slot is inert because its epoch no longer matches.
    ///
    /// The journal-slot references are cleared **after** step 4 and not before. §6.3 makes them
    /// "meaningful only within the selected epoch", and the epoch does not change until that gate is
    /// durable — so a cut between steps 3 and 4 must still find the old checkpoint with an index that
    /// remembers where its newest head entries were.
    pub fn compact(&mut self) -> Result<(), FailureCause> {
        let write = |_| FailureCause::MediaIo { detail: detail::media_io::WRITE };
        self.media.begin_checkpoint().map_err(write)?;
        let epoch = self.index.epoch;
        let through = self.index.through_sequence;
        // The body carries the *new* epoch, so the header the pass emits must already hold it.
        self.index.epoch = epoch + 1;
        let crc = {
            let mut pass = Pass { media: &mut self.media };
            match compaction::materialize(&self.index, &mut pass) {
                Ok(crc) => crc,
                Err(error) => {
                    // The old checkpoint is still the active, valid one and its journal still
                    // replays, so the honest state to leave behind is the one this pass started in.
                    self.index.epoch = epoch;
                    return Err(match error {
                        CompactionError::Media(RereadError::Media(_)) => {
                            FailureCause::MediaIo { detail: detail::media_io::WRITE }
                        }
                        _ => FailureCause::Internal { detail: detail::internal::INVARIANT },
                    });
                }
            }
        };
        if let Err(error) = self.media.finish_checkpoint(epoch + 1, through, crc) {
            self.index.epoch = epoch;
            return Err(write(error));
        }
        self.index.clear_journal_slots();
        self.epoch_base = through;
        Ok(())
    }

    /// The resident index, for a caller that wants to compare a mount against what was committed.
    pub fn index(&self) -> &RamIndex {
        &self.index
    }

    /// The §9 lease table. §13 counts it resident beside the index; it lives here because a lease is
    /// a RAM ownership fact no projection reconstructs.
    pub fn leases(&self) -> &LeaseTable {
        &self.leases
    }

    /// The media, for a harness that reboots it under the transaction.
    pub fn media_mut(&mut self) -> &mut M {
        &mut self.media
    }

    /// Gives the media back, ending this transaction's tenancy over it.
    ///
    /// A power cut is exactly this followed by a fresh [`mount`](Self::mount): every resident fact —
    /// the live claims, the writer, the lease table — is gone, and what the store is afterwards is
    /// whatever the card can prove.
    pub fn into_media(self) -> M {
        self.media
    }

    /// The hooks, for a caller that reads what they have collected.
    pub fn hooks(&self) -> &H {
        &self.hooks
    }

    /// The hooks, so a test can arm the next one.
    pub fn hooks_mut(&mut self) -> &mut H {
        &mut self.hooks
    }

    /// The typed validator, so a domain can be installed or a refusal armed.
    pub fn validator_mut(&mut self) -> &mut V {
        &mut self.validator
    }

    /// The catalog projection a head carries, copied into `into` and re-read from §13's newest
    /// source. `Ok(None)` is a key no head occupies.
    ///
    /// It is a **card read**: §13 leaves the envelope on the card precisely so RAM does not hold 256
    /// of them, and a caller that wants a page of metadata pays one bounded read per entry.
    pub fn head_projection(&mut self, key: HeadKey, into: &mut [u8]) -> Result<Option<usize>, FailureCause> {
        let Some(head) = self.stored_head(key)? else { return Ok(None) };
        let len = usize::from(head.envelope_len).min(into.len());
        into[..len].copy_from_slice(&head.envelope[..len]);
        Ok(Some(len))
    }

    /// The current head of one logical object, as `(revision, length, crc)`.
    pub fn head(&self, kind: ObjectKind, logical_object_id: LogicalObjectId) -> Option<(Revision, u64, u32)> {
        self.index
            .head(HeadKey { kind: kind.to_u16(), id: logical_object_id })
            .map(|head| (head.revision, head.length, head.crc))
    }

    /// How many terminal results are retained. The ring never grows past [`RESULT_RING`].
    pub fn retained_results(&self) -> usize {
        self.index.results.len()
    }

    /// True when the operation's result is still inside the retained window.
    pub fn retains(&self, operation_id: OperationId) -> bool {
        self.index.result_for(operation_id).is_some()
    }

    /// True while a reader lease is held.
    pub fn has_lease(&self) -> bool {
        self.pinned.is_some()
    }

    /// Reads a published head's bytes into `into`, reporting how many it holds.
    pub fn read_head(
        &mut self,
        kind: ObjectKind,
        logical_object_id: LogicalObjectId,
        into: &mut [u8],
    ) -> Option<usize> {
        let head = *self.index.head(HeadKey { kind: kind.to_u16(), id: logical_object_id })?;
        let len = (head.length as usize).min(into.len());
        self.media.read_generation(head.generation, 0, &mut into[..len]).ok()
    }

    /// Publishes a head directly, as a device-local producer does. Returns its new Revision.
    ///
    /// This is the store's own path, not the wire's: no claim, no session, one terminal record.
    pub fn publish_local(&mut self, kind: ObjectKind, bytes: &[u8]) -> (LogicalObjectId, Revision) {
        let logical_object_id = self.allocate_logical_id();
        let revision = self.commit_head(kind, logical_object_id, bytes).expect("a local publication");
        (logical_object_id, revision)
    }

    /// Claims an `InstallUpdate` directly, so a caller can name a target §9 makes non-cancellable.
    pub fn claim_install_update(&mut self, operation_id: OperationId, principal: PrincipalScope) {
        let generation = self.reserve_generation();
        let row = ActiveOperation {
            operation: operation_id,
            intent: [0x11; 32],
            principal: principal_bytes(principal),
            opcode: Opcode::InstallUpdate.to_u16(),
            subject_kind: ObjectKind::UpdatePackage.to_u16(),
            phase: OperationPhase::ExternalHandoff,
            flags: ActiveOperation::FLAG_GENERATION_RESERVED,
            logical_id: 0,
            expected_revision: 0,
            generation,
            progress_counter: 0,
            work_sequence: 0,
            abort_reason: 0,
        };
        let mutation = Mutation {
            active: Some(Change::Put(row)),
            generation_cursor: Some(generation.get() + 1),
            ..Mutation::default()
        };
        let _ = self.commit(RecordKind::Claim, operation_id, [0x11; 32], mutation);
    }

    /// Retains one terminal result under a synthetic identity, as any device-local producer's
    /// terminal commit does. §8.1: the window "is store-global in the strict sense".
    ///
    /// It claims first. §6.1 fixes a terminal record as "requires active remove **and** result
    /// append", so a result cannot be retired from a claim that was never made — and a record
    /// carrying the result alone is one this store can apply and cannot read back. That went
    /// unnoticed while the whole projection was resident, because nothing ever asked the card what
    /// the record said; §13's on-demand re-read asks on every `QueryOperation`.
    pub fn retain_local_result(&mut self, operation_id: OperationId) {
        if self.commit(RecordKind::Claim, operation_id, LOCAL_INTENT, local_claim(operation_id, None)).is_err() {
            return;
        }
        let envelope = ResultEnvelope::Object(ObjectResult {
            operation_id,
            store_id: self.index.store,
            kind: ObjectKind::Ride,
            outcome: ObjectOutcome::Committed,
            logical_object_id: LogicalObjectId::ZERO,
            revision: Revision::ZERO,
            length: 0,
            crc32: 0,
        });
        let result = self.new_result(operation_id, LOCAL_INTENT, LOCAL_PRINCIPAL, Ok(envelope));
        let mutation =
            Mutation { active: Some(Change::Remove(operation_id)), result: Some(result), ..Mutation::default() };
        // §6.1 makes the record's identity and the result's the same pair, so both carry the local
        // producer's intent rather than one of them carrying zero.
        let _ = self.commit(RecordKind::Terminal, operation_id, LOCAL_INTENT, mutation);
    }

    // -- the claim lock -------------------------------------------------------------------------

    /// §11's lookup: it answers every action but the first and **creates no state**.
    ///
    /// The one thing it does write is §6.1's restart. That is not an exception to "creates no
    /// state": the state already exists, and the row that says so is unchanged. What the restart
    /// makes durable is the payload rewind, and §6.1 makes the ordering normative — an acceptance
    /// carrying restart-at-zero "is emitted **only after** the durable restart record ... is
    /// synchronized" — so it happens here, before the outcome that carries the flag is returned.
    fn lookup(&mut self, intent: ClaimIntent) -> ClaimOutcome {
        if let Some(row) = self.active(intent.operation_id) {
            let (principal, digest, logical_id, generation) =
                (row.principal, row.intent, row.logical_id, row.generation);
            if principal != principal_bytes(intent.principal) {
                return ClaimOutcome::ForeignPrincipal;
            }
            if digest != intent.digest {
                return ClaimOutcome::Conflict;
            }
            if let Err(cause) = self.restart_work(intent, generation) {
                return ClaimOutcome::Refused(cause);
            }
            return ClaimOutcome::Restarted {
                logical_object_id: LogicalObjectId::new(logical_id),
                repository_revision: Revision::new(self.revision),
            };
        }
        let result = match self.retained_result(intent.operation_id) {
            Ok(Some(result)) => result,
            Ok(None) => return ClaimOutcome::Unclaimed,
            Err(cause) => return ClaimOutcome::Refused(cause),
        };
        if result.principal != principal_bytes(intent.principal) {
            return ClaimOutcome::ForeignPrincipal;
        }
        if result.intent != intent.digest {
            return ClaimOutcome::Conflict;
        }
        match decode_result(&result) {
            Ok(envelope) => ClaimOutcome::Committed(envelope),
            Err(terminal) => ClaimOutcome::Aborted(terminal),
        }
    }

    /// §11's durable claim, which is the first mutation and precedes every side effect.
    fn claim(&mut self, intent: ClaimIntent) -> ClaimOutcome {
        match self.lookup(intent) {
            ClaimOutcome::Unclaimed => {}
            resolved => return resolved,
        }
        // Target admissibility is preflight: §11 lets it "fail without creating state", and §9
        // requires exactly that for an AbortOperation naming a non-cancellable target.
        if intent.opcode == Opcode::AbortOperation {
            if let Some(target) = intent.target_operation_id {
                if let Some(cause) = self.target_admissibility(target, intent.principal) {
                    return ClaimOutcome::Refused(cause);
                }
            }
        }
        if let Some(cause) = self.hooks.admit_claim() {
            return ClaimOutcome::Refused(cause);
        }
        if self.index.actives.len() >= ACTIVE_CLAIMS {
            return ClaimOutcome::Refused(FailureCause::ResourceLimit {
                detail: detail::resource::NORMAL_OPERATION_CLAIMS,
            });
        }
        // §11's size and space admission, ahead of the record that reserves them.
        let available = self.media.free_bytes();
        if intent.declared_length > available {
            return ClaimOutcome::Refused(FailureCause::InsufficientSpace {
                required: intent.declared_length,
                available,
            });
        }

        // §6 admits one heavy transfer at a time and the engine's coordinator is what enforces it.
        // A second upload reaching this store would replace the generation the first is writing
        // into, and the client would never learn: its declared CRC is computed over the bytes it
        // offered, not the bytes that were stored. That is an invariant break, not a client error.
        if intent.opcode == Opcode::StartUpload && self.writing.is_some() {
            return ClaimOutcome::Refused(FailureCause::Internal { detail: detail::internal::INVARIANT });
        }

        let generation = self.reserve_generation();
        let logical_object_id = match intent.target {
            Target::Create => self.allocate_logical_id(),
            Target::Replace { logical_object_id, .. } => logical_object_id,
        };
        let expected_revision = match intent.target {
            Target::Create => 0,
            Target::Replace { expected_revision, .. } => expected_revision.get(),
        };
        let phase =
            if intent.opcode == Opcode::StartUpload { OperationPhase::Prepared } else { OperationPhase::Validating };
        let row = ActiveOperation {
            operation: intent.operation_id,
            intent: intent.digest,
            principal: principal_bytes(intent.principal),
            opcode: intent.opcode.to_u16(),
            subject_kind: intent.kind.to_u16(),
            phase,
            flags: ActiveOperation::FLAG_GENERATION_RESERVED,
            logical_id: logical_object_id.get(),
            expected_revision,
            generation,
            progress_counter: 0,
            work_sequence: 0,
            abort_reason: 0,
        };
        let mut mutation = Mutation {
            active: Some(Change::Put(row)),
            generation_cursor: Some(generation.get() + 1),
            ..Mutation::default()
        };
        if matches!(intent.target, Target::Create) {
            mutation.repository = Some(RepositoryChange {
                kind: intent.kind.to_u16(),
                revision: None,
                next_logical_id: Some(self.next_logical_id),
                flags: 0,
            });
        }
        // §11: "Failure returns without claiming." Everything the media half of an upload needs —
        // §12's lazy shard directories, the payload file itself, and the writer's own bound on the
        // declared length — happens **here**, ahead of the record that burns the OperationId. A
        // card that cannot make room for this generation must refuse an identifier the client can
        // still reuse, not one it can never use again.
        let opened = if intent.opcode == Opcode::StartUpload {
            match self.open_writer(intent, generation) {
                Ok(writing) => Some(writing),
                Err(cause) => {
                    // Nothing durable names this generation — the claim that would have reserved it
                    // was never written — so whatever the attempt created is an orphan §9 collects.
                    // Taking it back now saves the collector the walk.
                    let _ = self.media.collect_generation(generation);
                    return ClaimOutcome::Refused(cause);
                }
            }
        } else {
            None
        };

        if let Err(cause) = self.commit(RecordKind::Claim, intent.operation_id, intent.digest, mutation) {
            let _ = self.media.collect_generation(generation);
            return ClaimOutcome::Refused(cause);
        }
        // Only an upload opened one, and only an upload may replace one: assigning `opened`
        // unconditionally would let a direct mutation's claim — which opens nothing — clear the
        // writer of the upload it is running beside.
        if let Some(writing) = opened {
            self.writing = Some(writing);
        }
        let live = Live {
            operation: intent.operation_id,
            phase: wire_phase(phase),
            attached: intent.opcode == Opcode::StartUpload,
            durable_offset: 0,
            checkpoint_sequence: 0,
            target: intent.target_operation_id,
            disposition: None,
            metadata: intent.metadata,
            projection: None,
        };
        match self.live.iter_mut().find(|slot| slot.is_none()) {
            Some(slot) => *slot = Some(live),
            // The durable claim is bounded by the same §5.1 capacity this array is, and admission
            // refused above once the projection was full, so there is always a slot.
            None => return ClaimOutcome::Refused(FailureCause::Internal { detail: detail::internal::INVARIANT }),
        }
        ClaimOutcome::Claimed { logical_object_id, repository_revision: Revision::new(self.revision) }
    }

    /// The media half of admitting an upload: the shard directories, an empty payload file, and the
    /// writer that owns them.
    ///
    /// It creates no catalog state, which is what lets §11's preflight call it; and it truncates
    /// **and synchronizes** before it returns, which is what lets §6.1's restart call it. Both need
    /// the same thing — a generation that is known to be at offset zero on the card — and having
    /// one function say so is the only way the two paths cannot drift apart.
    fn open_writer(&mut self, intent: ClaimIntent, generation: GenerationId) -> Result<Writing, FailureCause> {
        let write = |_| FailureCause::MediaIo { detail: detail::media_io::WRITE };
        // §12's lazy shards: the leaf's directory has to exist before the leaf does.
        self.media.ensure_shards(generation).map_err(write)?;
        self.media.open_generation(generation).map_err(write)?;
        // §7: a generation is what it declared or it is nothing, so a readmission starts from an
        // empty payload rather than from whatever a previous tenancy left behind.
        self.media.truncate_payload().map_err(write)?;
        self.media.sync_payload().map_err(|_| FailureCause::MediaIo { detail: detail::media_io::SYNCHRONIZE })?;
        let generation_intent = Intent {
            store: self.index.store,
            operation: intent.operation_id,
            intent: intent.digest,
            parent: OperationId::ZERO,
            generation,
            declared_length: intent.declared_length,
            declared_crc: intent.expected_crc,
            subject_kind: intent.kind.to_u16(),
            subject: Subject::LogicalObject,
            part_key: 0,
        };
        let (writer, capability) = GenerationWriter::begin::<M::Error>(generation_intent)
            .map_err(|_| FailureCause::ResourceLimit { detail: detail::resource::OBJECT_LENGTH })?;
        Ok(Writing { operation: intent.operation_id, writer, capability, generation, sealed: false })
    }

    /// Points the media at this transaction's generation before a payload or `WORK` write.
    ///
    /// The media seam addresses "the generation this transaction opened", and a device-local
    /// publication opens one of its own. Nothing in the wire contract forbids one landing between
    /// two payload frames, so the cursor is re-pointed at every write rather than assumed. Without
    /// this the second half of an upload lands in somebody else's file and the declared CRC does not
    /// catch it: that CRC is computed over the bytes that were offered, not the bytes stored.
    fn writable(&mut self, operation_id: OperationId) -> Result<GenerationId, FailureCause> {
        let Some(generation) =
            self.writing.as_ref().filter(|writing| writing.operation == operation_id).map(|writing| writing.generation)
        else {
            return Err(FailureCause::Internal { detail: detail::internal::INVARIANT });
        };
        self.media
            .open_generation(generation)
            .map_err(|_| FailureCause::MediaIo { detail: detail::media_io::WRITE })?;
        Ok(generation)
    }

    /// Whether an `AbortOperation` may name this target at all (§3's ownership, §9's
    /// non-cancellable `InstallUpdate`). `None` means it may.
    fn target_admissibility(&mut self, target: OperationId, principal: PrincipalScope) -> Option<FailureCause> {
        let owner = match self.active(target).map(|row| row.principal) {
            Some(owner) => Some(owner),
            // §3 puts authorization ahead of every existence fact, so the owning principal has to be
            // known — and for a spent identifier it is in the result body, which §13 leaves on card.
            None => match self.retained_result(target) {
                Ok(result) => result.map(|result| result.principal),
                Err(cause) => return Some(cause),
            },
        };
        let active = self.active(target);
        if let Some(owner) = owner {
            if owner != principal_bytes(principal) {
                // §6.4 "requires the target's owning principal", and §3 puts authorization ahead of
                // every existence fact.
                return Some(FailureCause::Authorization { detail: detail::authorization::OPERATION_OWNER });
            }
        }
        if active.is_some_and(|row| row.opcode == Opcode::InstallUpdate.to_u16()) {
            return Some(FailureCause::UnsupportedCapability { detail: detail::capability::NON_CANCELLABLE_OPERATION });
        }
        None
    }

    /// §6.1's restart row: the work is discarded and re-synchronized at offset zero.
    ///
    /// [`GenerationWriter::restart`] truncates the payload **and syncs it** before the fresh
    /// capability exists, which is the durable restart §6.1 requires to precede the acceptance. No
    /// `WORK` slot is written because the restart-only profile records no streaming offset for one
    /// to contradict.
    fn restart_work(&mut self, intent: ClaimIntent, generation: GenerationId) -> Result<(), FailureCause> {
        let operation_id = intent.operation_id;
        if intent.opcode == Opcode::StartUpload {
            let resident = self.writing.as_ref().is_some_and(|writing| writing.operation == operation_id);
            if resident {
                self.writable(operation_id)?;
                let capability = self.writing.as_ref().expect("resident").capability;
                match self.writing.as_mut().expect("resident").writer.restart(capability, &mut self.media) {
                    Ok(fresh) => {
                        let writing = self.writing.as_mut().expect("resident");
                        writing.capability = fresh;
                        writing.sealed = false;
                    }
                    Err(error) => return Err(write_failure(&error)),
                }
            } else if self.writing.is_some() {
                // Another operation owns the one heavy transfer §6 allows, so this claim has no
                // work to restart and cannot be given any.
                return Err(FailureCause::Internal { detail: detail::internal::INVARIANT });
            } else {
                // No resident writer: a mount found this claim, or a readmission arrived after the
                // work was released. The restart-only profile has one answer for both — this
                // generation starts again from zero — and `open_writer` is what makes that durable
                // before the acceptance that carries the flag goes out.
                self.writing = Some(self.open_writer(intent, generation)?);
            }
        }
        if let Some(live) = self.live_mut(operation_id) {
            live.phase = Phase::Prepared;
            live.durable_offset = 0;
            live.checkpoint_sequence = 0;
            // A readmission issues a fresh session, so the claim is attached again.
            live.attached = intent.opcode == Opcode::StartUpload;
            // The digest matched, so these bytes are the ones already held — but the work restarted
            // and any projection derived from the discarded bytes is now about nothing.
            live.metadata = intent.metadata;
            live.projection = None;
        }
        Ok(())
    }

    // -- the work record ------------------------------------------------------------------------

    fn append<'s>(&mut self, operation_id: OperationId, offset: u64, bytes: &[u8]) -> Outcome<'s> {
        let Some(writing) = self.writing.as_mut() else {
            return Outcome::Failed(FailureCause::Internal { detail: detail::internal::INVARIANT });
        };
        if writing.operation != operation_id {
            return Outcome::Failed(FailureCause::Internal { detail: detail::internal::INVARIANT });
        }
        if writing.writer.written() != offset {
            return Outcome::Failed(FailureCause::Internal { detail: detail::internal::INVARIANT });
        }
        if let Some(cause) = self.hooks.appending(offset.saturating_add(bytes.len() as u64)) {
            return Outcome::Failed(cause);
        }
        if let Err(cause) = self.writable(operation_id) {
            return Outcome::Failed(cause);
        }
        let Some(writing) = self.writing.as_mut() else {
            return Outcome::Failed(FailureCause::Internal { detail: detail::internal::INVARIANT });
        };
        let capability = writing.capability;
        match writing.writer.append(capability, &mut self.media, bytes) {
            Ok(_) => {
                self.set_phase(operation_id, Phase::Streaming);
                Outcome::Appended
            }
            Err(error) => Outcome::Failed(write_failure(&error)),
        }
    }

    fn checkpoint<'s>(&mut self, operation_id: OperationId, offset: u64) -> Outcome<'s> {
        let Some(writing) = self.writing.as_mut() else {
            return Outcome::Failed(FailureCause::Internal { detail: detail::internal::INVARIANT });
        };
        if writing.operation != operation_id || writing.writer.written() < offset {
            return Outcome::Failed(FailureCause::Internal { detail: detail::internal::INVARIANT });
        }
        if let Err(cause) = self.writable(operation_id) {
            return Outcome::Failed(cause);
        }
        let writing = self.writing.as_mut().expect("the writer this transaction just re-opened");
        let capability = writing.capability;
        let prefix_crc = match writing.writer.synchronize(capability, &mut self.media) {
            Ok(crc) => crc,
            Err(error) => return Outcome::Failed(write_failure(&error)),
        };
        let sequence = match self.live_mut(operation_id) {
            Some(live) => {
                live.durable_offset = offset;
                live.checkpoint_sequence += 1;
                live.checkpoint_sequence
            }
            None => return Outcome::Failed(FailureCause::Internal { detail: detail::internal::INVARIANT }),
        };
        Outcome::Checkpointed { durable_offset: offset, prefix_crc, sequence }
    }

    fn seal<'s>(&mut self, operation_id: OperationId, declared_length: u64, expected_crc: u32) -> Outcome<'s> {
        if let Some(cause) = self.hooks.sealing() {
            return Outcome::Failed(cause);
        }
        let Some(writing) = self.writing.as_mut() else {
            return Outcome::Failed(FailureCause::Internal { detail: detail::internal::INVARIANT });
        };
        if writing.operation != operation_id {
            return Outcome::Failed(FailureCause::Internal { detail: detail::internal::INVARIANT });
        }
        // §7 fixes the declared length and CRC at `begin`, from the same claim the engine is now
        // sealing, so the two the command restates cannot legitimately differ — and a seal that
        // silently took the command's word for it would be sealing something other than what was
        // claimed. Disagreement is a contract break between the engine and this store, not a
        // client error, so it is refused as one.
        if writing.writer.declared_length() != declared_length || writing.writer.declared_crc() != expected_crc {
            return Outcome::Failed(FailureCause::Internal { detail: detail::internal::INVARIANT });
        }
        if let Err(cause) = self.writable(operation_id) {
            return Outcome::Failed(cause);
        }
        let writing = self.writing.as_mut().expect("the writer this transaction just re-opened");
        let capability = writing.capability;
        match writing.writer.seal(capability, &mut self.media, &mut self.stride, DraftPartRef::ZERO) {
            Ok(_) => {
                writing.sealed = true;
                self.set_phase(operation_id, Phase::Sealed);
                Outcome::Sealed
            }
            Err(error) => Outcome::Failed(write_failure(&error)),
        }
    }

    fn validate<'s>(&mut self, operation_id: OperationId) -> Outcome<'s> {
        let Some(row) = self.active(operation_id).copied() else {
            return Outcome::Failed(FailureCause::Internal { detail: detail::internal::INVARIANT });
        };
        let Some(kind) = ObjectKind::from_u16(row.subject_kind) else {
            return Outcome::Failed(FailureCause::Internal { detail: detail::internal::INVARIANT });
        };
        let Some(opcode) = Opcode::from_u16(row.opcode) else {
            return Outcome::Failed(FailureCause::Internal { detail: detail::internal::INVARIANT });
        };
        // §7 runs domain validation only after the seal, over bytes the card is known to hold, so
        // what the validator is handed is the sealed generation's own length and CRC. A direct
        // mutation has no generation of its own and is validated on its request alone.
        // The writer has to be *this* operation's. It never mattered while the validator was handed
        // only a generation it ignored; it matters now, because a direct mutation validated beside a
        // sealed upload would be handed that upload's bytes and judge the wrong payload.
        let sealed = self.writing.as_ref().filter(|writing| writing.sealed && writing.operation == operation_id);
        let (generation, length, crc) = match sealed {
            Some(writing) => (writing.generation, writing.writer.written(), writing.writer.declared_crc()),
            None => (row.generation, 0, 0),
        };
        let streaming = sealed.is_some();
        let Some(metadata) = self.live_for(operation_id).map(|live| live.metadata) else {
            return Outcome::Failed(FailureCause::Internal { detail: detail::internal::INVARIANT });
        };
        // A patch is applied *to* something, so the repository needs the projection the head carries
        // — which §13 leaves on the card. Only the one opcode that patches pays for the re-read.
        let existing = if opcode == Opcode::SetMetadata {
            match self.stored_head(HeadKey { kind: kind.to_u16(), id: LogicalObjectId::new(row.logical_id) }) {
                Ok(head) => head,
                Err(cause) => return Outcome::Failed(cause),
            }
        } else {
            None
        };
        let subject = Validation {
            kind,
            opcode,
            metadata: metadata.as_bytes(),
            current: existing.as_ref().map(|head| &head.envelope[..head.envelope_len as usize]),
            length,
            crc,
        };
        // The two are borrowed apart rather than through `self` because a validator reads the
        // payload *while* it runs: the repository is the domain half and the media the byte half of
        // one step, and neither is reachable from the other.
        let this = &mut *self;
        let validator = &mut this.validator;
        let outcome = if streaming {
            let mut bytes = SealedGeneration { media: &mut this.media, generation, length };
            validator.validate(&subject, &mut bytes)
        } else {
            validator.validate(&subject, &mut NoBytes)
        };
        match outcome {
            Ok(projection) => {
                if let Some(live) = self.live_mut(operation_id) {
                    live.projection = Some(projection);
                }
                self.set_phase(operation_id, Phase::Validating);
                Outcome::Validated
            }
            Err(detail) => Outcome::Failed(FailureCause::SemanticValidation { kind, detail }),
        }
    }

    /// §6.4's second durable step: the target is marked terminal `Aborted` before the abort
    /// command's own result is committed, and the disposition it produced is kept for that result.
    fn cancel_target<'s>(
        &mut self,
        operation_id: OperationId,
        target: OperationId,
        reason: AbortReason,
    ) -> Outcome<'s> {
        let disposition = if self.active(target).is_some() {
            if let Some(cause) = self.hooks.aborting() {
                // §6.4's step 2 is a terminal record like any other, and a medium that cannot write
                // one cannot write this one either.
                return Outcome::Failed(cause);
            }
            let terminal = AbortCause::Cancelled { reason }.terminal();
            if self.finish_claim(target, Err(terminal)).is_err() {
                return Outcome::Failed(FailureCause::MediaIo { detail: detail::media_io::WRITE });
            }
            AbortDisposition::Cancelled
        } else if self.retains(target) {
            // "If the target was already terminal, it is unchanged and the abort result says
            // `already terminal`."
            AbortDisposition::AlreadyTerminal
        } else {
            AbortDisposition::AlreadyAbsent
        };
        if let Some(live) = self.live_mut(operation_id) {
            live.disposition = Some(disposition);
            live.phase = Phase::Aborting;
        }
        Outcome::TargetCancelled(disposition)
    }

    fn publish<'s>(&mut self, operation_id: OperationId) -> Outcome<'s> {
        if let Some(cause) = self.hooks.publishing() {
            return Outcome::Failed(cause);
        }
        let Some(row) = self.active(operation_id).copied() else {
            return Outcome::Failed(FailureCause::Internal { detail: detail::internal::INVARIANT });
        };
        let Some(kind) = ObjectKind::from_u16(row.subject_kind) else {
            return Outcome::Failed(FailureCause::Internal { detail: detail::internal::INVARIANT });
        };
        let Some(opcode) = Opcode::from_u16(row.opcode) else {
            return Outcome::Failed(FailureCause::Internal { detail: detail::internal::INVARIANT });
        };
        let logical_object_id = LogicalObjectId::new(row.logical_id);
        let key = HeadKey { kind: kind.to_u16(), id: logical_object_id };

        if self.hooks.races_publication() {
            // A device-local producer commits a competing mutation just before the commit lock. It
            // restamps the head it found, so it needs the whole of it — envelope and resolution
            // included, which §13 leaves on the card.
            let existing = match self.stored_head(key) {
                Ok(Some(head)) => head,
                Ok(None) => return Outcome::Failed(FailureCause::Internal { detail: detail::internal::INVARIANT }),
                Err(cause) => return Outcome::Failed(cause),
            };
            let head = CatalogHead { revision: Revision::new(self.revision + 1), ..existing };
            if self.commit_local_publication(head, None).is_err() {
                return Outcome::Failed(FailureCause::MediaIo { detail: detail::media_io::WRITE });
            }
        }

        // The compare-and-swap is rechecked here, under the commit lock, exactly as §6.3 requires.
        if let Some(expected) = replace_expectation(&row, opcode) {
            let current = self.index.head(key).map_or(Revision::ZERO, |head| head.revision);
            if current.get() != expected {
                return Outcome::Failed(FailureCause::RevisionConflict { detail: detail::revision::OBJECT, current });
            }
        }

        let principal = row.principal;
        let digest = row.intent;
        // What the repository derived at `Validate`. A store with no repository for this kind
        // derived §5.3's bare reservation, which is what the fallback names — never a head whose
        // metadata the kernel made up.
        let projection = self.live_for(operation_id).and_then(|live| live.projection);
        let change = change_kind(opcode, &row);
        let envelope = match opcode {
            Opcode::StartUpload => {
                let Some(writing) = self.writing.take() else {
                    return Outcome::Failed(FailureCause::Internal { detail: detail::internal::INVARIANT });
                };
                let length = writing.writer.written();
                let crc = writing.writer.declared_crc();
                let revision = Revision::new(self.revision + 1);
                let projection = projection.unwrap_or(CatalogProjection::RESERVATION);
                let head = CatalogHead {
                    key,
                    flags: 0,
                    revision,
                    generation: writing.generation,
                    length,
                    crc,
                    envelope_len: projection.len(),
                    envelope: reserve(&projection),
                    resolution: GenerationId::ZERO,
                };
                let result = ObjectResult {
                    operation_id,
                    store_id: self.index.store,
                    kind,
                    outcome: ObjectOutcome::Committed,
                    logical_object_id,
                    revision,
                    length,
                    crc32: crc,
                };
                let envelope = ResultEnvelope::Object(result);
                if self.commit_publication(operation_id, digest, principal, Some(head), envelope, Bump::Yes).is_err() {
                    return Outcome::Failed(FailureCause::MediaIo { detail: detail::media_io::WRITE });
                }
                envelope
            }
            Opcode::DeleteObject => {
                let previous = self.index.head(key).map(|head| (head.length, head.crc)).unwrap_or((0, 0));
                let revision = Revision::new(self.revision + 1);
                let envelope = ResultEnvelope::Object(ObjectResult {
                    operation_id,
                    store_id: self.index.store,
                    kind,
                    outcome: ObjectOutcome::Deleted,
                    logical_object_id,
                    revision,
                    length: previous.0,
                    crc32: previous.1,
                });
                if self.commit_removal(operation_id, digest, principal, key, envelope).is_err() {
                    return Outcome::Failed(FailureCause::MediaIo { detail: detail::media_io::WRITE });
                }
                envelope
            }
            Opcode::SetMetadata => {
                // The new head keeps everything but the revision, so this is the one publishing
                // path that has to re-read the envelope and resolution §13 leaves on the card.
                let existing = match self.stored_head(key) {
                    Ok(Some(head)) => head,
                    Ok(None) => {
                        return Outcome::Failed(FailureCause::ObjectNotFound {
                            detail: detail::not_found::LOGICAL_OBJECT,
                        })
                    }
                    Err(cause) => return Outcome::Failed(cause),
                };
                let revision = Revision::new(self.revision + 1);
                // The patched projection is the repository's, not the kernel's: it was derived at
                // `Validate` from this patch *and* the envelope the head already carried, which is
                // why a patch adds a field instead of replacing the whole projection with itself.
                let head = match projection {
                    Some(projection) => CatalogHead {
                        revision,
                        envelope_len: projection.len(),
                        envelope: reserve(&projection),
                        ..existing
                    },
                    None => CatalogHead { revision, ..existing },
                };
                let envelope = ResultEnvelope::Object(ObjectResult {
                    operation_id,
                    store_id: self.index.store,
                    kind,
                    outcome: ObjectOutcome::MetadataChanged,
                    logical_object_id,
                    revision,
                    length: existing.length,
                    crc32: existing.crc,
                });
                if self.commit_publication(operation_id, digest, principal, Some(head), envelope, Bump::Yes).is_err() {
                    return Outcome::Failed(FailureCause::MediaIo { detail: detail::media_io::WRITE });
                }
                envelope
            }
            Opcode::InstallUpdate | Opcode::AcknowledgeRideImported => {
                let outcome = if opcode == Opcode::InstallUpdate {
                    ObjectOutcome::UpdateInstallRequested
                } else {
                    ObjectOutcome::RideImported
                };
                // Neither command changes a head, so neither moves a revision: a repository whose
                // revision advanced would tell every other client that something it can see has
                // changed, and nothing has. §10's ObjectResult reports the revision the target
                // still holds.
                let revision = self.index.head(key).map_or(Revision::ZERO, |head| head.revision);
                let envelope = ResultEnvelope::Object(ObjectResult {
                    operation_id,
                    store_id: self.index.store,
                    kind,
                    outcome,
                    logical_object_id,
                    revision,
                    length: 0,
                    crc32: 0,
                });
                if self.commit_publication(operation_id, digest, principal, None, envelope, Bump::No).is_err() {
                    return Outcome::Failed(FailureCause::MediaIo { detail: detail::media_io::WRITE });
                }
                envelope
            }
            Opcode::AbortOperation => {
                let live = self.live_for(operation_id);
                let disposition = live.and_then(|live| live.disposition).unwrap_or(AbortDisposition::AlreadyAbsent);
                let envelope = ResultEnvelope::Abort(AbortResult {
                    operation_id,
                    store_id: self.index.store,
                    // §6.4 gives the abort command's own result the target it named.
                    target_operation_id: live.and_then(|live| live.target).unwrap_or(OperationId::ZERO),
                    disposition,
                });
                if self.commit_publication(operation_id, digest, principal, None, envelope, Bump::No).is_err() {
                    return Outcome::Failed(FailureCause::MediaIo { detail: detail::media_io::WRITE });
                }
                envelope
            }
            _ => return Outcome::Failed(FailureCause::Internal { detail: detail::internal::INVARIANT }),
        };
        // §4: the event fires "after a catalog commit's validity gate is durable", and never
        // before. Every arm above either committed its record — gate included — or returned, so
        // reaching this line *is* that gate.
        //
        // An `AbortOperation` is the one publication that emits nothing, and deliberately: §4 gives
        // the event to a *repository*, and an abort command belongs to none. It names no head, moves
        // no revision, and the kind its claim carries is a placeholder the engine had to write
        // somewhere — attributing an abort to the route repository would be a lie a consumer acts on.
        if let ResultEnvelope::Object(result) = envelope {
            self.hooks.committed(CommitEvent {
                store: result.store_id,
                kind: result.kind,
                logical_object_id: Some(result.logical_object_id),
                revision: result.revision,
                change,
                operation: operation_id,
            });
        }
        self.clear_live(operation_id);
        Outcome::Published(envelope)
    }

    /// Durably abandons a claim — and does **nothing at all** when it is already terminal.
    ///
    /// §11 makes an identifier spent once it reaches a terminal state, and a second abort would
    /// append a second result for the same operation: two rows in a 64-entry ring, one of which the
    /// client never asked for. The engine's unwind paths can reach this twice — an outcome that
    /// lands after its connection died, an orphaned claim walked to terminal by teardown — so the
    /// no-op is a property of the store, not a discipline expected of the caller.
    fn abort<'s>(&mut self, operation_id: OperationId, cause: AbortCause) -> Outcome<'s> {
        let terminal = cause.terminal();
        if self.active(operation_id).is_none() {
            return Outcome::Aborted(terminal);
        }
        // The session goes whether or not the record lands: the engine has already released it, and
        // §8.1's attachment bit is about the session, not about the claim.
        if let Some(live) = self.live_mut(operation_id) {
            live.attached = false;
            live.phase = Phase::Aborting;
        }
        if let Some(cause) = self.hooks.aborting() {
            return Outcome::Failed(cause);
        }
        if self.finish_claim(operation_id, Err(terminal)).is_err() {
            return Outcome::Failed(FailureCause::MediaIo { detail: detail::media_io::WRITE });
        }
        Outcome::Aborted(terminal)
    }

    // -- downloads ------------------------------------------------------------------------------

    fn resolve<'s>(&mut self, kind: ObjectKind, logical_object_id: LogicalObjectId) -> Outcome<'s> {
        let key = HeadKey { kind: kind.to_u16(), id: logical_object_id };
        let Some(head) = self.index.head(key).copied() else {
            return Outcome::Failed(FailureCause::ObjectNotFound { detail: detail::not_found::LOGICAL_OBJECT });
        };
        // §9: the pin is fixed at the generation the resolve returned and never re-resolved, which
        // is what makes "catalog replacement never changes an existing lease" true.
        self.lease_connection = self.lease_connection.wrapping_add(1);
        // The engine owns the real SessionId; the table only needs the two identity fields to
        // distinguish one tenancy of a slot from the next, and this transaction serves one heavy
        // transfer at a time. The low bit keeps the value out of the inactive zero a SessionId
        // refuses.
        let Some(session) = SessionId::new(self.lease_connection | 1) else {
            return Outcome::Failed(FailureCause::Internal { detail: detail::internal::INVARIANT });
        };
        match self.leases.pin(self.lease_connection, session, head.generation) {
            Ok(handle) => {
                self.pinned = Some(Pinned { generation: head.generation, handle });
                Outcome::Resolved(PinnedSource {
                    logical_object_id,
                    revision: head.revision,
                    total_length: head.length,
                    crc32: head.crc,
                })
            }
            Err(_) => Outcome::Failed(FailureCause::ResourceLimit { detail: detail::resource::READER_LEASES }),
        }
    }

    fn read_source<'s>(&mut self, offset: u64, length: u16, scratch: &'s mut [u8]) -> Outcome<'s> {
        let Some(pinned) = self.pinned else {
            return Outcome::Failed(FailureCause::Internal { detail: detail::internal::INVARIANT });
        };
        let wanted = usize::from(length).min(scratch.len());
        match self.media.read_generation(pinned.generation, offset, &mut scratch[..wanted]) {
            Ok(read) => Outcome::SourceBytes { offset, bytes: &scratch[..read.min(wanted)] },
            Err(_) => Outcome::Failed(FailureCause::MediaIo { detail: detail::media_io::READ }),
        }
    }

    /// Releases the reader lease exactly once, and writes the retention decrement §9 owes.
    fn release_lease<'s>(&mut self) -> Outcome<'s> {
        let Some(pinned) = self.pinned.take() else {
            // The engine releases on teardown and on finish; only one of them can be first, and a
            // second release naming no live lease is §9's explicit no-op.
            return Outcome::LeaseReleased;
        };
        if let ReleaseEffect::Retention(change) = self.leases.release(pinned.handle, &self.index.retained) {
            let mutation = Mutation { retained: Some(change), ..Mutation::default() };
            let _ = self.commit(RecordKind::Retention, OperationId::ZERO, [0; 32], mutation);
            // §9: the entry's last reason has been cleared, so the bytes are unreachable and this
            // transaction can collect them without waiting for a pass to find them.
            if let Change::Remove(generation) = change {
                let _ = self.media.collect_generation(generation);
            }
        }
        Outcome::LeaseReleased
    }

    // -- device control and queries ---------------------------------------------------------------

    fn device_control<'s>(
        &mut self,
        request: DeviceControlRequest<'_>,
        scratch: &'s mut [u8],
    ) -> DeviceControlAnswer<'s> {
        match request {
            DeviceControlRequest::GetDeviceStatus => DeviceControlAnswer::DeviceStatus(self.status),
            DeviceControlRequest::GetConfig => DeviceControlAnswer::Config(self.config),
            DeviceControlRequest::SetConfig(block) => {
                self.config = block;
                DeviceControlAnswer::Config(self.config)
            }
            DeviceControlRequest::SetClock(_) => DeviceControlAnswer::ClockStatus(self.clock),
            DeviceControlRequest::ForgetBond(_) => DeviceControlAnswer::BondForgotten,
            DeviceControlRequest::Echo(payload) => {
                let len = payload.len().min(scratch.len());
                scratch[..len].copy_from_slice(&payload[..len]);
                DeviceControlAnswer::Echo(&scratch[..len])
            }
            DeviceControlRequest::ResetStore(echoed) => {
                if echoed != self.index.store {
                    return DeviceControlAnswer::Refused(FailureCause::MediaUnavailable {
                        detail: detail::media::UNMOUNTED,
                    });
                }
                let store = self.hooks.mint_store_id(self.index.store);
                if self.media.reset_store(store).is_err() {
                    return DeviceControlAnswer::Refused(FailureCause::MediaIo { detail: detail::media_io::WRITE });
                }
                // §16: reset destroys "every object, operation result, and lease". The projection
                // is rebuilt as §12's first checkpoint, and nothing of the old store survives it.
                self.index.reset_to_initial(store, ObjectKind::Weather.to_u16());
                // Every cursor comes back through `rebind`, not by hand. §12's reset writes a first
                // checkpoint with `through_sequence` zero, so §6.3's slot origin for the new store
                // is zero too — and a reset that restored the three obvious cursors and left
                // `epoch_base` at a compacted store's `S` would compute slot zero for the whole
                // range `1..=S`. Every commit would overwrite the same slot, replay would stop at
                // nothing, and §6.3's fail-closed rule would turn the next mount into a
                // recovery-failed one. Deriving all four from the index that was just reset is the
                // only form of this that cannot go stale when a cursor is added.
                self.rebind(0);
                self.leases.clear();
                self.pinned = None;
                self.live = [None; MAX_ACTIVE_OPERATIONS];
                self.writing = None;
                DeviceControlAnswer::ResetStore(store)
            }
        }
    }

    /// The report an operation gets when `principal` asks about it (§3: authorization precedes
    /// status, so a foreign principal never learns whether the ID exists).
    /// It re-reads the card, because §13 leaves both the owning principal and the typed outcome in
    /// the retained result's body. A re-read the medium refuses is reported as the media failure it
    /// is rather than as §8.1's `Unknown`, which would say "neither active nor retained" about an
    /// identity this store is still holding.
    pub fn report_for(
        &mut self,
        operation_id: OperationId,
        principal: PrincipalScope,
    ) -> Result<OperationReport, FailureCause> {
        if let Some(owner) = self.active(operation_id).map(|row| row.principal) {
            if owner != principal_bytes(principal) {
                return Ok(OperationReport::NotAuthorized);
            }
            return self.report(operation_id, None);
        }
        match self.retained_result(operation_id)? {
            Some(result) if result.principal != principal_bytes(principal) => Ok(OperationReport::NotAuthorized),
            retained => self.report(operation_id, retained),
        }
    }

    /// `retained` is the result body the caller already re-read, so one query costs one card read.
    fn report(
        &mut self,
        operation_id: OperationId,
        retained: Option<TerminalResult>,
    ) -> Result<OperationReport, FailureCause> {
        if let Some(row) = self.active(operation_id) {
            // §8.1's matrix fixes every field from the originating claim, and a row outside it "is
            // an internal state/codec error and MUST NOT be emitted".
            if row.opcode == Opcode::AbortOperation.to_u16() {
                return Ok(OperationReport::InProgress(OperationProgress {
                    namespace: SubjectNamespace::None,
                    subject_kind: 0,
                    phase: Phase::Aborting,
                    flags: 0,
                    logical_object_id: LogicalObjectId::ZERO,
                    durable_offset: 0,
                }));
            }
            let live = self.live_for(operation_id);
            let phase = live.map_or(wire_phase(row.phase), |live| live.phase);
            let is_upload = row.opcode == Opcode::StartUpload.to_u16();
            let mut flags = progress_flags::LOGICAL_ID_PRESENT;
            if live.is_some_and(|live| live.attached) {
                // "attached only while that session exists" — a fact about the session, which is
                // why a claim a mount recovered reports none however far its phase had got.
                flags |= progress_flags::SESSION_ATTACHED;
            }
            let durable_offset = match (is_upload, phase) {
                // "offset is durable payload prefix, declared length in phases 2..4".
                (true, Phase::Sealed | Phase::Validating | Phase::Publishing) => {
                    self.writing.as_ref().map_or(0, |writing| writing.writer.declared_length())
                }
                (true, _) => live.map_or(0, |live| live.durable_offset),
                (false, _) => 0,
            };
            return Ok(OperationReport::InProgress(OperationProgress {
                namespace: SubjectNamespace::Logical,
                subject_kind: row.subject_kind,
                phase,
                flags,
                logical_object_id: LogicalObjectId::new(row.logical_id),
                durable_offset,
            }));
        }
        let retained = match retained {
            Some(result) => Some(result),
            None => self.retained_result(operation_id)?,
        };
        Ok(match retained {
            Some(result) => match decode_result(&result) {
                Ok(envelope) => OperationReport::Committed(envelope),
                Err(terminal) => OperationReport::Aborted(terminal),
            },
            // §8.1: "Unknown means only that the ID is neither active nor retained. It cannot
            // distinguish never claimed from evicted."
            None => OperationReport::Unknown,
        })
    }

    // -- §13's on-demand re-reads, in the shapes this transaction needs them ---------------------

    /// The retained terminal result an `OperationId` names, its 208 bytes re-read from the card.
    ///
    /// `Ok(None)` is §8.1's honest "neither active nor retained"; a medium that refused the read is
    /// a `MediaIo` failure, and a card holding something other than what the ring names is a kernel
    /// invariant break, because RAM and card derive that identity from one another.
    fn retained_result(&mut self, operation: OperationId) -> Result<Option<TerminalResult>, FailureCause> {
        let Some((physical, key)) = self.index.result_position(operation) else { return Ok(None) };
        let bytes = self.media.result_entry(physical, &key).map_err(reread_failure)?;
        match TerminalResult::decode(&bytes) {
            Ok(stored) if stored.operation == key.operation && stored.commit_sequence == key.commit_sequence => {
                Ok(Some(stored))
            }
            _ => Err(FailureCause::Internal { detail: detail::internal::INVARIANT }),
        }
    }

    /// The whole catalog head a key names: the index's fixed fields, with the envelope and the
    /// resolution generation re-read from §6.3's newest source.
    fn stored_head(&mut self, key: HeadKey) -> Result<Option<CatalogHead>, FailureCause> {
        let Some(entry) = self.index.head(key).copied() else { return Ok(None) };
        let fields = self.media.head_fields(&entry).map_err(reread_failure)?;
        compaction::compose_head::<()>(&entry, &fields)
            .map(Some)
            .map_err(|_| FailureCause::Internal { detail: detail::internal::INVARIANT })
    }

    // -- committing -------------------------------------------------------------------------------

    /// Appends one journal record and applies it to the projection.
    ///
    /// The order is the whole point: the record is durable before the projection moves, so a cut
    /// between them recovers to a store that either has the record or does not, and never to one
    /// whose RAM says something the card cannot.
    fn commit(
        &mut self,
        kind: RecordKind,
        operation: OperationId,
        intent: [u8; 32],
        mutation: Mutation,
    ) -> Result<(), FailureCause> {
        if self.sequence == 0 {
            return Err(FailureCause::Internal { detail: detail::internal::INVARIANT });
        }
        // §6.3: "before accepting a 193rd record in one epoch, `CardStore` blocks new mutations and
        // compacts". Committing past the trigger would wrap a slot — making recovery choose a suffix
        // that spans two epochs — or overwrite a record the selected checkpoint still needs, so the
        // pass runs here rather than being owed to a caller who might not know it is due.
        //
        // **It is measured and it is expensive.** On the shipped media (#1359) the pass took
        // **715 ms** over 30 card-sourced entries, of which ~441 ms was the 30 whole-stride journal
        // reads §6.3's newest-source rule makes and ~274 ms the 127 sector writes, the two syncs and
        // the gate. An epoch that reaches the trigger carries up to 192 journal-carried entries, so
        // the same rate projects **3-5 s** — blocked inside one client's `FinishUpload`, which is
        // exactly the objection the previous slice's comment raised against running store-level
        // passes inside a command. Correctness first: a store that reaches its trigger and does not
        // compact cannot commit at all. Moving this to a background pass with its own budget is a
        // cutover-slice decision with a measured figure behind it now, and is deliberately not
        // taken here.
        if self.compaction_required() {
            self.compact()?;
        }
        let slot = self.next_slot() as u16;
        let record = JournalBody {
            store: self.index.store,
            epoch: self.index.epoch,
            sequence: self.sequence,
            slot,
            kind,
            operation,
            intent,
            mutation,
        };
        let body = record.encode_body();
        // The card and RAM must never be able to disagree, and a record this store can apply but
        // cannot read back is exactly that disagreement — invisible while the projection was
        // resident, and a `QueryOperation` failure once §13's re-reads look at the bytes. Debug-only
        // because it is a second decode of every commit, and release builds have the crash matrix.
        debug_assert!(
            JournalBody::decode_body(&body).is_ok(),
            "a {kind:?} record for {:?} does not decode: {:?}",
            record.operation,
            JournalBody::decode_body(&body).err(),
        );
        let gate = record.gate_for(&body).encode();
        if self.media.append_journal(slot, &body, &gate).is_err() {
            return Err(FailureCause::MediaIo { detail: detail::media_io::WRITE });
        }
        // `absorb`, not `apply`: the record is now on the card at `slot`, and the head or result it
        // carries has its card-resident half *there* rather than in the active checkpoint until the
        // next compaction. A plain `apply` would leave the index pointing every later re-read at the
        // checkpoint, which holds the older bytes or none at all.
        if self.index.absorb(&record).is_err() {
            // The record is on the card and the projection refused it. That is a kernel invariant
            // break, not a client error: the two must never be able to disagree.
            return Err(FailureCause::Internal { detail: detail::internal::INVARIANT });
        }
        self.sequence += 1;
        Ok(())
    }

    /// The one terminal record §11 requires: catalog publication and the retained result together.
    fn commit_publication(
        &mut self,
        operation: OperationId,
        intent: [u8; 32],
        principal: [u8; 32],
        head: Option<CatalogHead>,
        envelope: ResultEnvelope,
        bump: Bump,
    ) -> Result<(), FailureCause> {
        let revision = self.revision + 1;
        let kind = head.map_or_else(|| self.active(operation).map_or(0, |row| row.subject_kind), |head| head.key.kind);
        let mut mutation = Mutation {
            active: Some(Change::Remove(operation)),
            repository: matches!(bump, Bump::Yes).then_some(RepositoryChange {
                kind,
                revision: Some(revision),
                next_logical_id: None,
                flags: 0,
            }),
            result: Some(self.new_result(operation, intent, principal, Ok(envelope))),
            ..Mutation::default()
        };
        if let Some(head) = head {
            if let Some(retention) = self.retention_for_displaced(head.key, head.generation) {
                mutation.retained = Some(Change::Put(retention));
            }
            mutation.head = Some(Change::Put(head));
        }
        let displaced = head.and_then(|head| self.displaced_generation(head.key, head.generation));
        self.commit(RecordKind::Terminal, operation, intent, mutation)?;
        if matches!(bump, Bump::Yes) {
            self.revision = revision;
        }
        if let Some(generation) = displaced {
            if !self.leases.holds(generation) {
                let _ = self.media.collect_generation(generation);
            }
        }
        Ok(())
    }

    fn commit_removal(
        &mut self,
        operation: OperationId,
        intent: [u8; 32],
        principal: [u8; 32],
        key: HeadKey,
        envelope: ResultEnvelope,
    ) -> Result<(), FailureCause> {
        let revision = self.revision + 1;
        let mutation = Mutation {
            active: Some(Change::Remove(operation)),
            head: self.index.head(key).map(|_| Change::Remove(key)),
            repository: Some(RepositoryChange {
                kind: key.kind,
                revision: Some(revision),
                next_logical_id: None,
                flags: 0,
            }),
            result: Some(self.new_result(operation, intent, principal, Ok(envelope))),
            ..Mutation::default()
        };
        let displaced = self.index.head(key).map(|head| head.generation);
        self.commit(RecordKind::Terminal, operation, intent, mutation)?;
        self.revision = revision;
        if let Some(generation) = displaced {
            if !self.leases.holds(generation) {
                let _ = self.media.collect_generation(generation);
            }
        }
        Ok(())
    }

    /// Marks one claim terminal, with the result the caller decided.
    fn finish_claim(
        &mut self,
        operation: OperationId,
        outcome: Result<ResultEnvelope, TerminalError>,
    ) -> Result<(), FailureCause> {
        let Some(row) = self.active(operation).copied() else { return Ok(()) };
        let result = self.new_result(operation, row.intent, row.principal, outcome);
        let mutation =
            Mutation { active: Some(Change::Remove(operation)), result: Some(result), ..Mutation::default() };
        self.commit(RecordKind::Terminal, operation, row.intent, mutation)?;
        // §6.2: only once the terminal record's gate is durable "may its WORK/payload become
        // collectible", which is exactly here.
        if let Some(writing) = self.writing.take_if(|writing| writing.operation == operation) {
            let capability = writing.capability;
            let mut writer = writing.writer;
            let _ = writer.abort::<M::Error>(capability);
            let _ = self.media.collect_generation(writing.generation);
        }
        self.clear_live(operation);
        Ok(())
    }

    /// Publishes a head under no operation identity, as a device-local producer does.
    fn commit_head(&mut self, kind: ObjectKind, id: LogicalObjectId, bytes: &[u8]) -> Result<Revision, FailureCause> {
        let generation = self.reserve_generation();
        self.media.ensure_shards(generation).map_err(|_| FailureCause::MediaIo { detail: detail::media_io::WRITE })?;
        self.media
            .open_generation(generation)
            .map_err(|_| FailureCause::MediaIo { detail: detail::media_io::WRITE })?;
        self.media.truncate_payload().map_err(|_| FailureCause::MediaIo { detail: detail::media_io::WRITE })?;
        self.media.write_payload(0, bytes).map_err(|_| FailureCause::MediaIo { detail: detail::media_io::WRITE })?;
        self.media.sync_payload().map_err(|_| FailureCause::MediaIo { detail: detail::media_io::SYNCHRONIZE })?;
        let revision = Revision::new(self.revision + 1);
        let head = CatalogHead {
            key: HeadKey { kind: kind.to_u16(), id },
            flags: 0,
            revision,
            generation,
            length: bytes.len() as u64,
            crc: obc_crc::crc32(bytes),
            envelope_len: PLACEHOLDER_ENVELOPE.len() as u16,
            envelope: envelope_reservation(),
            resolution: GenerationId::ZERO,
        };
        self.commit_local_publication(head, Some(generation))?;
        Ok(revision)
    }

    /// A device-local producer's publication: the claim that reserves its generation, then the
    /// terminal record that publishes the head.
    ///
    /// It is two records rather than one because §6.1 fixes where a reserved `GenerationId` may be
    /// carried — "a normal claim carries that value in an active entry with flag bit 4" — so a
    /// publication that creates bytes has to claim before it commits, exactly as a wire operation
    /// does. `reserve` is `None` for a publication that only restamps an existing head, which
    /// reserves nothing and therefore claims no generation.
    ///
    /// Its terminal record carries a retained result like any other. §6.1 admits a head put only in
    /// a terminal record and fixes a terminal record as "requires active remove **and** result
    /// append", so there is no legal record shape that publishes a head without one — and §8.1 says
    /// the same thing from the other side, that the window "is store-global in the strict sense".
    /// The result was omitted here until the index-backed store started reading records back and
    /// found one it had written and could not decode. The identity it claims under is derived from
    /// the generation so two local publications can never collide.
    fn commit_local_publication(
        &mut self,
        head: CatalogHead,
        reserve: Option<GenerationId>,
    ) -> Result<(), FailureCause> {
        // `head.revision` is the revision this publication stamps — both callers set it to
        // `self.revision + 1` — so it is the store's next one and has never been used before.
        let operation = local_operation_id(head.revision);
        let mut row = local_claim_row(operation);
        row.subject_kind = head.key.kind;
        row.flags = if reserve.is_some() { ActiveOperation::FLAG_GENERATION_RESERVED } else { 0 };
        row.logical_id = head.key.id.get();
        row.generation = head.generation;
        let claim = Mutation {
            active: Some(Change::Put(row)),
            generation_cursor: reserve.map(|generation| generation.get() + 1),
            ..Mutation::default()
        };
        self.commit(RecordKind::Claim, operation, LOCAL_INTENT, claim)?;

        let revision = self.revision + 1;
        let envelope = ResultEnvelope::Object(ObjectResult {
            operation_id: operation,
            store_id: self.index.store,
            kind: ObjectKind::from_u16(head.key.kind).unwrap_or(ObjectKind::Ride),
            outcome: ObjectOutcome::Committed,
            logical_object_id: head.key.id,
            revision: Revision::new(revision),
            length: head.length,
            crc32: head.crc,
        });
        let mut mutation = Mutation {
            active: Some(Change::Remove(operation)),
            head: Some(Change::Put(head)),
            repository: Some(RepositoryChange {
                kind: head.key.kind,
                revision: Some(revision),
                next_logical_id: Some(self.next_logical_id),
                flags: 0,
            }),
            result: Some(self.new_result(operation, LOCAL_INTENT, LOCAL_PRINCIPAL, Ok(envelope))),
            ..Mutation::default()
        };
        if let Some(retention) = self.retention_for_displaced(head.key, head.generation) {
            mutation.retained = Some(Change::Put(retention));
        }
        let displaced = self.displaced_generation(head.key, head.generation);
        let existed = self.index.head(head.key).is_some();
        self.commit(RecordKind::Terminal, operation, LOCAL_INTENT, mutation)?;
        self.revision = revision;
        // §4: "Device-local producers publish through exactly that path", so a locally produced head
        // wakes a consumer exactly as an uploaded one does — after the same gate.
        if let Some(kind) = ObjectKind::from_u16(head.key.kind) {
            self.hooks.committed(CommitEvent {
                store: self.index.store,
                kind,
                logical_object_id: Some(head.key.id),
                revision: head.revision,
                change: if existed { ChangeKind::Replaced } else { ChangeKind::Created },
                operation,
            });
        }
        if let Some(generation) = displaced {
            if !self.leases.holds(generation) {
                let _ = self.media.collect_generation(generation);
            }
        }
        Ok(())
    }

    /// The generation a publication over `key` displaces, when there is one and it is not the same.
    fn displaced_generation(&self, key: HeadKey, incoming: GenerationId) -> Option<GenerationId> {
        let head = self.index.head(key)?;
        (head.generation != incoming).then_some(head.generation)
    }

    /// §9's retained-previous entry, when a live lease still pins the bytes being displaced.
    fn retention_for_displaced(&self, key: HeadKey, incoming: GenerationId) -> Option<RetainedPrevious> {
        let head = *self.index.head(key)?;
        if head.generation == incoming {
            return None;
        }
        let lease_count = self.leases.count_for(head.generation);
        if lease_count == 0 {
            return None;
        }
        Some(RetainedPrevious {
            reasons: RetainedPrevious::REASON_LIVE_LEASE,
            lease_count,
            kind: head.kind,
            logical_id: head.id,
            generation: head.generation,
            length: head.length,
            crc: head.crc,
            retain_through: 0,
            object_revision: head.revision,
        })
    }

    fn new_result(
        &self,
        operation: OperationId,
        intent: [u8; 32],
        principal: [u8; 32],
        outcome: Result<ResultEnvelope, TerminalError>,
    ) -> TerminalResult {
        let mut body = [0u8; TerminalResult::BODY_CAPACITY];
        let (committed, result_type) = match outcome {
            Ok(envelope) => {
                let result_type = match envelope {
                    ResultEnvelope::Object(_) => ResultType::Object,
                    ResultEnvelope::DraftPart(_) => ResultType::DraftPart,
                    ResultEnvelope::Abort(_) => ResultType::Abort,
                };
                let len = result_type.encoded_len() as usize;
                match envelope {
                    ResultEnvelope::Object(result) => body[..len].copy_from_slice(&result.encode()),
                    ResultEnvelope::DraftPart(result) => body[..len].copy_from_slice(&result.encode()),
                    ResultEnvelope::Abort(result) => body[..len].copy_from_slice(&result.encode()),
                }
                (true, result_type)
            }
            Err(terminal) => {
                let len = ResultType::Aborted.encoded_len() as usize;
                let encoded = terminal.body();
                encoded.encode_into(&mut body[..len]).expect("a text-free body is 48 bytes");
                (false, ResultType::Aborted)
            }
        };
        TerminalResult {
            commit_sequence: self.index.terminal_counter + 1,
            operation,
            intent,
            principal,
            committed,
            result_type,
            body,
        }
    }

    // -- small helpers ----------------------------------------------------------------------------

    fn active(&self, operation: OperationId) -> Option<&ActiveOperation> {
        self.index.actives.iter().find(|row| row.operation == operation)
    }

    fn reserve_generation(&mut self) -> GenerationId {
        GenerationId::new(self.index.next_generation)
    }

    fn allocate_logical_id(&mut self) -> LogicalObjectId {
        let id = LogicalObjectId::new(self.next_logical_id);
        self.next_logical_id += 1;
        id
    }

    /// The resident half of one live claim, **by reference**: it carries the claim's whole metadata
    /// envelope, and returning it by value would put that on every caller's frame.
    fn live_for(&self, operation: OperationId) -> Option<&Live> {
        self.live.iter().flatten().find(|live| live.operation == operation)
    }

    fn live_mut(&mut self, operation: OperationId) -> Option<&mut Live> {
        self.live.iter_mut().flatten().find(|live| live.operation == operation)
    }

    fn set_phase(&mut self, operation: OperationId, phase: Phase) {
        if let Some(live) = self.live_mut(operation) {
            live.phase = phase;
        }
    }

    fn clear_live(&mut self, operation: OperationId) {
        for slot in self.live.iter_mut() {
            if slot.is_some_and(|live| live.operation == operation) {
                *slot = None;
            }
        }
    }
}

impl<M: KernelMedia, V: Validator, H: Hooks> Transaction for KernelTransaction<M, V, H> {
    fn execute<'s>(&mut self, command: Command<'_>, scratch: &'s mut [u8]) -> Outcome<'s> {
        match command {
            Command::Lookup(intent) => Outcome::Claim(self.lookup(intent)),
            Command::Claim(intent) => Outcome::Claim(self.claim(intent)),
            Command::Append { operation_id, offset, bytes } => self.append(operation_id, offset, bytes),
            Command::Checkpoint { operation_id, offset } => self.checkpoint(operation_id, offset),
            Command::Seal { operation_id, declared_length, expected_crc } => {
                self.seal(operation_id, declared_length, expected_crc)
            }
            Command::Validate { operation_id } => self.validate(operation_id),
            Command::Publish { operation_id } => self.publish(operation_id),
            Command::CancelTarget { operation_id, target, reason } => self.cancel_target(operation_id, target, reason),
            Command::Abort { operation_id, cause } => self.abort(operation_id, cause),
            Command::Resolve { kind, logical_object_id, .. } => self.resolve(kind, logical_object_id),
            Command::ReadSource { offset, length } => self.read_source(offset, length, scratch),
            Command::ReleaseLease => self.release_lease(),
            Command::DeviceControl(request) => Outcome::DeviceControl(self.device_control(request, scratch)),
            Command::QueryOperation { operation_id, principal } => match self.report_for(operation_id, principal) {
                Ok(report) => Outcome::OperationReport(report),
                Err(cause) => Outcome::Failed(cause),
            },
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Free functions
// ---------------------------------------------------------------------------------------------

/// The device-control plane's config block, as a fresh transaction carries it (§16).
///
/// The three `initial_*` functions exist so the by-value and the in-place constructor cannot drift:
/// each writes exactly these values into its own destination.
fn initial_config() -> ConfigBlock {
    ConfigBlock {
        unit_flags: 0,
        weather_refresh: obc_link::control::WeatherRefresh::Off,
        name: [0; obc_link::control::MAX_DEVICE_NAME],
        name_len: 0,
    }
}

/// The device-control plane's status, as a fresh transaction carries it (§16).
fn initial_status(store: StoreId) -> DeviceStatus {
    DeviceStatus {
        firmware_major: 0,
        firmware_minor: 1,
        firmware_patch: 0,
        hardware_revision: 1,
        device_serial: [0x0b; 16],
        boot_count: 1,
        uptime_seconds: 60,
        stack_high_water: 4_096,
        status_flags: obc_link::control::status_flags::CARD_PRESENT,
        mount_class: obc_link::control::MountClass::Mounted,
        firmware_build: 1,
        store_id: store,
    }
}

/// The device-control plane's clock, as a fresh transaction carries it (§16).
fn initial_clock() -> ClockStatus {
    ClockStatus {
        epoch_seconds: 1_700_000_000,
        source: obc_link::control::ClockSource::Companion,
        state: obc_link::control::ClockState::Trusted,
    }
}

/// The 32-byte principal digest a §5.3 row stores, from the 16-byte scope the adapter established.
///
/// §5.3 reserves 32 bytes for "the opaque stable principal-scope digest"; the wire's scope is 16.
/// Left-aligned and zero-filled, so the mapping is total, injective and stable across a mount.
fn principal_bytes(principal: PrincipalScope) -> [u8; 32] {
    let mut out = [0u8; 32];
    out[..16].copy_from_slice(principal.as_bytes());
    out
}

/// The 32-byte principal a device-local producer's records carry.
///
/// Zero-filling the reserved half leaves the all-zero digest reachable from a wire scope of all
/// zeros — and a client that happened to present one would then own every local producer's
/// operation, because §3 decides `QueryOperation`'s authorization by comparing exactly these bytes.
/// Setting the reserved half instead puts the local identity outside the image of
/// [`principal_bytes`] by construction, so no wire scope can ever alias it.
const LOCAL_PRINCIPAL: [u8; 32] = {
    let mut bytes = [0u8; 32];
    let mut index = 16;
    while index < 32 {
        bytes[index] = 0xFF;
        index += 1;
    }
    bytes
};

/// The canonical-intent digest a device-local publication claims under. It is a constant because
/// there is no request to canonicalize: §11's digest exists to compare two client intents, and a
/// local producer has none to be confused with.
const LOCAL_INTENT: [u8; 32] = [0xA7; 32];

/// The active row a device-local producer claims under (§6.1's "a claim record requires active put").
fn local_claim_row(operation: OperationId) -> ActiveOperation {
    ActiveOperation {
        operation,
        intent: LOCAL_INTENT,
        principal: LOCAL_PRINCIPAL,
        opcode: Opcode::StartUpload.to_u16(),
        subject_kind: ObjectKind::Ride.to_u16(),
        phase: OperationPhase::Sealed,
        flags: 0,
        logical_id: 0,
        expected_revision: 0,
        generation: GenerationId::ZERO,
        progress_counter: 0,
        work_sequence: 0,
        abort_reason: 0,
    }
}

/// The claim mutation that row belongs to.
fn local_claim(operation: OperationId, reserve: Option<GenerationId>) -> Mutation {
    let mut row = local_claim_row(operation);
    if let Some(generation) = reserve {
        row.flags = ActiveOperation::FLAG_GENERATION_RESERVED;
        row.generation = generation;
    }
    Mutation {
        active: Some(Change::Put(row)),
        generation_cursor: reserve.map(|generation| generation.get() + 1),
        ..Mutation::default()
    }
}

/// The identity a device-local publication claims under, derived from the Revision it stamps.
///
/// §11 requires every claim to have an OperationId, and two local publications must never share
/// one — §8.1 answers `QueryOperation` by that identity, so a repeat makes the answer ambiguous.
///
/// **Not the generation.** That was the obvious derivation and it is wrong for the one local
/// publication that creates no bytes: a restamp of an existing head — §6.3's raced publication, and
/// `SetMetadata`'s local twin — reserves nothing and republishes the generation that is already
/// there, so two publications a revision apart would mint the same identity. The store-wide
/// revision is monotone and every publication advances it, so deriving from that is unique by
/// construction for both shapes. `CatalogModel::check` refuses a duplicate outright, which is what
/// turned this from an argument into a caught collision.
fn local_operation_id(revision: Revision) -> OperationId {
    let mut bytes = [0xFFu8; 16];
    bytes[8..].copy_from_slice(&revision.get().to_be_bytes());
    OperationId::new(bytes)
}

fn envelope_reservation() -> [u8; CatalogHead::ENVELOPE_CAPACITY] {
    reserve(&CatalogProjection::RESERVATION)
}

/// A projection in the fixed 96-byte reservation §5.3 gives every head, zero-padded.
fn reserve(projection: &CatalogProjection) -> [u8; CatalogHead::ENVELOPE_CAPACITY] {
    let mut envelope = [0u8; CatalogHead::ENVELOPE_CAPACITY];
    let bytes = projection.as_bytes();
    envelope[..bytes.len()].copy_from_slice(bytes);
    envelope
}

/// Which §4 change a publication is, from the opcode and the claim's target.
///
/// The wire's `Committed` covers both halves of a Put; the claim row is what separates them, because
/// a create records no expected revision and a compare-and-swap replace always does.
fn change_kind(opcode: Opcode, row: &ActiveOperation) -> ChangeKind {
    match opcode {
        Opcode::StartUpload if row.expected_revision == 0 => ChangeKind::Created,
        Opcode::StartUpload => ChangeKind::Replaced,
        Opcode::DeleteObject => ChangeKind::Deleted,
        Opcode::SetMetadata => ChangeKind::MetadataChanged,
        Opcode::InstallUpdate => ChangeKind::InstallRequested,
        _ => ChangeKind::RideAcknowledged,
    }
}

/// The wire phase one storage phase projects to (§8.1).
const fn wire_phase(phase: OperationPhase) -> Phase {
    match phase {
        OperationPhase::Prepared => Phase::Prepared,
        OperationPhase::DraftOpen => Phase::DraftOpen,
        OperationPhase::Streaming => Phase::Streaming,
        OperationPhase::Sealed => Phase::Sealed,
        OperationPhase::Validating => Phase::Validating,
        OperationPhase::Publishing => Phase::Publishing,
        OperationPhase::ExternalHandoff => Phase::ExternalHandoff,
        OperationPhase::Aborting => Phase::Aborting,
    }
}

/// The revision a publication must find under the commit lock, or `None` when it is a create.
///
/// §6.3's compare-and-swap belongs to a replace. Only `StartUpload` has a create form, and a create
/// records no expectation — which the claim row stores as zero. That cannot be confused with a real
/// expectation: a published head's revision comes from the store's monotone counter, which starts
/// at one, so zero is not a revision any head has ever had. Every other claiming opcode names an
/// existing object by construction, so its expectation is always checked.
const fn replace_expectation(row: &ActiveOperation, opcode: Opcode) -> Option<u64> {
    match opcode {
        Opcode::StartUpload if row.expected_revision == 0 => None,
        _ => Some(row.expected_revision),
    }
}

/// The §12 cause a refused on-demand re-read reports as.
fn reread_failure<E>(error: RereadError<E>) -> FailureCause {
    match error {
        RereadError::Media(_) => FailureCause::MediaIo { detail: detail::media_io::READ },
        RereadError::Missing => FailureCause::Internal { detail: detail::internal::INVARIANT },
    }
}

/// The §12 cause a generation-writer refusal reports as.
fn write_failure<E>(error: &WriteError<E>) -> FailureCause {
    match error {
        WriteError::Media(_) => FailureCause::MediaIo { detail: detail::media_io::WRITE },
        WriteError::Capability | WriteError::NotStreaming => {
            FailureCause::Internal { detail: detail::internal::INVARIANT }
        }
        WriteError::Overrun { .. } => FailureCause::Checksum { detail: detail::checksum::WHOLE_PAYLOAD },
        WriteError::Length { .. } | WriteError::Crc { .. } => {
            FailureCause::Checksum { detail: detail::checksum::WHOLE_PAYLOAD }
        }
        WriteError::Unreachable { .. } => FailureCause::MediaIo { detail: detail::media_io::UNCERTAIN_COMMIT },
        WriteError::TooLarge { .. } => FailureCause::ResourceLimit { detail: detail::resource::OBJECT_LENGTH },
    }
}

/// The typed envelope or the terminal error one retained result carries (§5.3).
fn decode_result(result: &TerminalResult) -> Result<ResultEnvelope, TerminalError> {
    if result.committed {
        let len = result.result_type.encoded_len() as usize;
        let decoded = match result.result_type {
            ResultType::Object => ObjectResult::decode(&result.body[..len]).map(ResultEnvelope::Object),
            ResultType::Abort => AbortResult::decode(&result.body[..len]).map(ResultEnvelope::Abort),
            ResultType::DraftPart => {
                obc_link::result::DraftPartResult::decode(&result.body[..len]).map(ResultEnvelope::DraftPart)
            }
            ResultType::Aborted | ResultType::Domain => Err(obc_link::DecodeError::unknown_enum()),
        };
        return decoded.map_err(|_| TerminalError {
            category: obc_link::ErrorCategory::INTERNAL,
            namespace: 0,
            detail: detail::internal::INVARIANT,
            current_revision: None,
        });
    }
    let len = ResultType::Aborted.encoded_len() as usize;
    let body = ErrorBody::decode(&result.body[..len]).unwrap_or_else(|_| {
        ErrorBody::bare(
            obc_link::ErrorCategory::INTERNAL,
            detail::internal::INVARIANT,
            obc_link::RetryGuidance::RETRY_AFTER_DELAY,
        )
    });
    Err(TerminalError {
        category: body.category,
        namespace: body.detail_namespace,
        detail: body.detail,
        current_revision: (body.presence & obc_link::error::presence::CURRENT_REVISION != 0)
            .then_some(body.current_revision),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A media that does nothing, so the size assert below measures **this struct's** layout rather
    /// than whatever media a particular board composes it with.
    struct NoMedia;

    impl GenerationMedia for NoMedia {
        type Error = ();

        fn ensure_shards(&mut self, _generation: GenerationId) -> Result<(), ()> {
            Ok(())
        }
        fn payload_length(&mut self) -> Result<u64, ()> {
            Ok(0)
        }
        fn write_payload(&mut self, _offset: u64, _bytes: &[u8]) -> Result<(), ()> {
            Ok(())
        }
        fn sync_payload(&mut self) -> Result<(), ()> {
            Ok(())
        }
        fn truncate_payload(&mut self) -> Result<(), ()> {
            Ok(())
        }
        fn write_work(&mut self, _offset: usize, _bytes: &[u8]) -> Result<(), ()> {
            Ok(())
        }
        fn sync_work(&mut self) -> Result<(), ()> {
            Ok(())
        }
    }

    impl KernelMedia for NoMedia {
        fn read_checkpoint(&mut self, _offset: usize, _into: &mut [u8]) -> Result<(), ()> {
            Ok(())
        }
        fn read_record(&mut self, _slot: u16) -> Result<Option<JournalBody>, ()> {
            Ok(None)
        }
        fn begin_checkpoint(&mut self) -> Result<(), ()> {
            Ok(())
        }
        fn write_checkpoint_sector(&mut self, _offset: usize, _sector: &[u8; SECTOR]) -> Result<(), ()> {
            Ok(())
        }
        fn finish_checkpoint(&mut self, _epoch: u64, _through_sequence: u64, _body_crc: u32) -> Result<(), ()> {
            Ok(())
        }
        fn append_journal(
            &mut self,
            _slot: u16,
            _body: &[u8; JOURNAL_BODY_LEN],
            _gate: &[u8; GATE_LEN],
        ) -> Result<(), ()> {
            Ok(())
        }
        fn open_generation(&mut self, _generation: GenerationId) -> Result<(), ()> {
            Ok(())
        }
        fn read_generation(&mut self, _generation: GenerationId, _offset: u64, _into: &mut [u8]) -> Result<usize, ()> {
            Ok(0)
        }
        fn collect_generation(&mut self, _generation: GenerationId) -> Result<(), ()> {
            Ok(())
        }
        fn free_bytes(&mut self) -> u64 {
            u64::MAX
        }
        fn reset_store(&mut self, _store: StoreId) -> Result<(), ()> {
            Ok(())
        }
    }

    type Pinned = KernelTransaction<NoMedia, AcceptEverything, NoHooks>;

    /// The size [`KernelTransaction::mount_in_place`]'s field list was written against, over a
    /// zero-sized media so the figure is this struct's own rather than a particular board's.
    ///
    /// Anonymous and at module scope so it is evaluated eagerly — a *named* const is checked only
    /// where something reads it, which would make this look like a guard while gating nothing.
    /// Two values because the two targets have different pointer widths — the board is 32-bit
    /// thumbv8m, the host suite 64-bit. Re-pinning one is the moment the field list gets checked.
    ///
    /// It was 73,384 while the transaction held the whole projection. Swapping that for §13's
    /// bounded index took 36,344 bytes out of it, and what is left is dominated by the 16 KiB seal
    /// stride rather than by any catalog state.
    ///
    /// It then grew by **1,584 bytes** when the claimed metadata envelope and the repository's
    /// derived catalog projection joined the resident half of a live claim: 176 bytes per row across
    /// §5.1's nine, which is the price of a head whose metadata is the client's own rather than a
    /// placeholder. Both are resident rather than durable on purpose — the envelope is part of §11's
    /// canonical intent and comes back with any request that could drive the claim forward, and the
    /// projection is derived from bytes that a restart discards anyway.
    #[cfg(target_pointer_width = "64")]
    const _: () = assert!(core::mem::size_of::<Pinned>() == 38_624);

    /// **The compile-time half of `mount_in_place`'s safety argument.**
    ///
    /// The constructor writes a `MaybeUninit<Self>` field by field, so its soundness is exactly the
    /// claim that the list is complete. This destructures with no `..`: a field added to
    /// `KernelTransaction` stops this file compiling until someone looks at it, and the only reason
    /// to look is the raw-pointer list.
    #[test]
    fn every_field_is_named_by_the_in_place_constructor() {
        let mut slot = std::boxed::Box::new(core::mem::MaybeUninit::<Pinned>::uninit());
        let store = StoreId::new([0x4C; 16]);
        let placed = KernelTransaction::mount_in_place(&mut slot, NoMedia, AcceptEverything, NoHooks, store);
        let KernelTransaction {
            media: _,
            validator: _,
            hooks: _,
            index,
            sequence,
            epoch_base,
            revision,
            next_logical_id,
            leases: _,
            pinned,
            lease_connection,
            live,
            writing,
            stride,
            config,
            status,
            clock,
        } = placed;
        assert_eq!(index.store, store);
        assert_eq!((*sequence, *epoch_base, *revision, *next_logical_id), (1, 0, 0, 1));
        assert!(pinned.is_none() && writing.is_none() && live.iter().all(|row| row.is_none()));
        assert_eq!(*lease_connection, 0);
        assert!(stride.iter().all(|byte| *byte == 0));
        assert_eq!(config.name_len, initial_config().name_len);
        assert_eq!(status.store_id, store);
        assert_eq!(clock.epoch_seconds, initial_clock().epoch_seconds);
    }

    /// The board runs `mount_in_place`; every host test runs `mount`. They must produce the same
    /// transaction, or the board is exercising something the suite never sees.
    #[test]
    fn the_in_place_constructor_agrees_with_the_by_value_one() {
        let store = StoreId::new([0x4C; 16]);
        let mut slot = std::boxed::Box::new(core::mem::MaybeUninit::<Pinned>::uninit());
        let placed = KernelTransaction::mount_in_place(&mut slot, NoMedia, AcceptEverything, NoHooks, store);
        let by_value =
            std::boxed::Box::new(KernelTransaction::mount(NoMedia, AcceptEverything, NoHooks, RamIndex::new(store), 0));

        assert_eq!(placed.store_id(), by_value.store_id());
        assert_eq!(*placed.index(), *by_value.index());
        assert_eq!(placed.sequence, by_value.sequence);
        assert_eq!(placed.revision, by_value.revision);
        assert_eq!(placed.next_logical_id, by_value.next_logical_id);
        assert_eq!(placed.retained_results(), by_value.retained_results());
        assert_eq!(placed.has_lease(), by_value.has_lease());
        assert_eq!(placed.compaction_required(), by_value.compaction_required());
        assert_eq!(placed.config.name_len, by_value.config.name_len);
        assert_eq!(placed.status.store_id, by_value.status.store_id);
        assert_eq!(placed.clock.epoch_seconds, by_value.clock.epoch_seconds);
        assert_eq!(placed.stride, by_value.stride);
    }

    /// A mount over a projection that already carries state derives the cursors from it, which is
    /// what makes a remount continue the store rather than restart it.
    #[test]
    fn rebind_derives_the_cursors_from_the_projection_in_place() {
        let store = StoreId::new([0x4C; 16]);
        let mut slot = std::boxed::Box::new(core::mem::MaybeUninit::<Pinned>::uninit());
        let placed = KernelTransaction::mount_in_place(&mut slot, NoMedia, AcceptEverything, NoHooks, store);
        {
            let (_, index) = placed.media_and_index_mut();
            index.reset_to_initial(store, ObjectKind::Weather.to_u16());
            index.through_sequence = 41;
            index.repositories[0].revision = Revision::new(7);
            index.repositories[0].next_logical_id = LogicalObjectId::new(9);
        }
        // §6.3's slot origin: this checkpoint absorbed through 30, so slot zero of its epoch carries
        // 31 and the record after sequence 41 lands at slot 11.
        placed.rebind(30);
        assert_eq!(placed.sequence, 42, "the journal cursor is through_sequence + 1");
        assert_eq!(placed.next_slot(), 11, "the slot is relative to the checkpoint, not to the store");
        assert_eq!(placed.revision, 7);
        assert_eq!(placed.next_logical_id, 9);
        assert_eq!(placed.status.store_id, store);
    }
}
