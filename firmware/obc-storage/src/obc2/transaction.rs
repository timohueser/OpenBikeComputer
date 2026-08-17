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
    OperationReport, Outcome, PinnedSource, PrincipalScope, TerminalError, Transaction,
};
use obc_link::error::detail;
use obc_link::frame::Opcode;
use obc_link::ids::{DraftPartRef, GenerationId, LogicalObjectId, OperationId, Revision, SessionId, StoreId};
use obc_link::query::{progress_flags, OperationProgress};
use obc_link::registry::{AbortReason, ObjectKind, ObjectOutcome, Phase, SubjectNamespace};
use obc_link::result::{AbortDisposition, AbortResult, ObjectResult, ResultEnvelope};
use obc_link::upload::Target;
use obc_link::ErrorBody;

use super::entries::{
    ActiveOperation, CatalogHead, HeadKey, OperationPhase, ResultType, RetainedPrevious, TerminalResult,
};
use super::generation::{Capability, GenerationMedia, GenerationWriter, Intent, WriteError};
use super::journal::{Change, JournalBody, Mutation, RecordKind, RepositoryChange};
use super::leases::{LeaseHandle, LeaseTable, ReleaseEffect};
use super::limits::{
    GATE_LEN, JOURNAL_BODY_LEN, JOURNAL_SLOTS, MAX_ACTIVE_OPERATIONS, MAX_NORMAL_ACTIVE_OPERATIONS,
    MAX_TERMINAL_RESULTS, SLOT_STRIDE,
};
use super::model::CatalogModel;
use super::work::Subject;

/// The §5.1 ceiling on normal claimed operations, re-exported so a caller can size against it.
pub const ACTIVE_CLAIMS: usize = MAX_NORMAL_ACTIVE_OPERATIONS;

/// The §8.1 retained-result window.
pub const RESULT_RING: usize = MAX_TERMINAL_RESULTS;

/// The eight canonical bytes every head's envelope reservation carries until the effect seam
/// carries the wire's own metadata envelope.
///
/// §5.3 gives a head an eight-to-ninety-six byte canonical envelope and refuses a shorter one.
/// `ClaimIntent` does not carry `StartUpload`'s envelope — the engine drops it — so the kernel has
/// nothing authentic to store yet and writes the minimum well-formed reservation instead. Threading
/// the envelope through the seam is a DOS5 change to both crates, and is called out as such.
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
}

/// The typed validator §6.3 runs over sealed bytes, before publication and never before the seal.
///
/// The domain validators are DOS5's. What this seam fixes now is the shape of the refusal: a
/// `semanticValidation` detail in the kind's **own** namespace, which is what makes a rejection
/// legible to a client that has never heard of this device's repositories.
pub trait Validator {
    /// Validates one sealed generation. `Err(detail)` is the kind-scoped semantic detail.
    fn validate(&mut self, kind: ObjectKind, generation: GenerationId, length: u64, crc: u32) -> Result<(), u16>;
}

/// The validator a device without domain rules runs: every sealed generation is admissible.
#[derive(Debug, Default, Clone, Copy)]
pub struct AcceptEverything;

impl Validator for AcceptEverything {
    fn validate(&mut self, _kind: ObjectKind, _generation: GenerationId, _length: u64, _crc: u32) -> Result<(), u16> {
        Ok(())
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
    model: CatalogModel,
    /// The journal cursor: the next sequence and the slot it lands in.
    sequence: u64,
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
    /// Opens a transaction over a store whose projection is `model`.
    ///
    /// `sequence` is the journal cursor a mount established: the next record's sequence, which is
    /// the projection's `through_sequence` plus one.
    pub fn mount(media: M, validator: V, hooks: H, model: CatalogModel) -> Self {
        let sequence = model.through_sequence.saturating_add(1);
        let revision = model.repositories.iter().map(|row| row.revision.get()).max().unwrap_or(0);
        let next_logical_id = model.repositories.iter().map(|row| row.next_logical_id.get()).max().unwrap_or(0).max(1);
        let store = model.store;
        KernelTransaction {
            media,
            validator,
            hooks,
            model,
            sequence,
            revision,
            next_logical_id,
            leases: LeaseTable::new(),
            pinned: None,
            lease_connection: 0,
            live: [None; MAX_ACTIVE_OPERATIONS],
            writing: None,
            stride: [0; SLOT_STRIDE],
            config: ConfigBlock {
                unit_flags: 0,
                weather_refresh: obc_link::control::WeatherRefresh::Off,
                name: [0; obc_link::control::MAX_DEVICE_NAME],
                name_len: 0,
            },
            status: DeviceStatus {
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
            },
            clock: ClockStatus {
                epoch_seconds: 1_700_000_000,
                source: obc_link::control::ClockSource::Companion,
                state: obc_link::control::ClockState::Trusted,
            },
        }
    }

    /// The store's identity.
    pub fn store_id(&self) -> StoreId {
        self.model.store
    }

    /// The projection, for a caller that wants to compare a mount against what was committed.
    pub fn model(&self) -> &CatalogModel {
        &self.model
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

    /// The hooks, so a test can arm the next one.
    pub fn hooks_mut(&mut self) -> &mut H {
        &mut self.hooks
    }

    /// The typed validator, so a domain can be installed or a refusal armed.
    pub fn validator_mut(&mut self) -> &mut V {
        &mut self.validator
    }

    /// The current head of one logical object, as `(revision, length, crc)`.
    pub fn head(&self, kind: ObjectKind, logical_object_id: LogicalObjectId) -> Option<(Revision, u64, u32)> {
        self.model
            .head(HeadKey { kind: kind.to_u16(), id: logical_object_id })
            .map(|head| (head.revision, head.length, head.crc))
    }

    /// How many terminal results are retained. The ring never grows past [`RESULT_RING`].
    pub fn retained_results(&self) -> usize {
        self.model.results.len()
    }

    /// True when the operation's result is still inside the retained window.
    pub fn retains(&self, operation_id: OperationId) -> bool {
        self.model.result_for(operation_id).is_some()
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
        let head = *self.model.head(HeadKey { kind: kind.to_u16(), id: logical_object_id })?;
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
    pub fn retain_local_result(&mut self, operation_id: OperationId) {
        let envelope = ResultEnvelope::Object(ObjectResult {
            operation_id,
            store_id: self.model.store,
            kind: ObjectKind::Ride,
            outcome: ObjectOutcome::Committed,
            logical_object_id: LogicalObjectId::ZERO,
            revision: Revision::ZERO,
            length: 0,
            crc32: 0,
        });
        let result = self.committed_result(operation_id, [0; 32], PrincipalScope::new([0; 16]), envelope);
        let mutation = Mutation { result: Some(result), ..Mutation::default() };
        let _ = self.commit(RecordKind::Terminal, operation_id, [0x01; 32], mutation);
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
            let (principal, digest, logical_id) = (row.principal, row.intent, row.logical_id);
            if principal != principal_bytes(intent.principal) {
                return ClaimOutcome::ForeignPrincipal;
            }
            if digest != intent.digest {
                return ClaimOutcome::Conflict;
            }
            if let Err(cause) = self.restart_work(intent.operation_id) {
                return ClaimOutcome::Refused(cause);
            }
            return ClaimOutcome::Restarted {
                logical_object_id: LogicalObjectId::new(logical_id),
                repository_revision: Revision::new(self.revision),
            };
        }
        let Some(result) = self.model.result_for(intent.operation_id) else {
            return ClaimOutcome::Unclaimed;
        };
        let result = *result;
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
        if self.model.actives.len() >= ACTIVE_CLAIMS {
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
        if let Err(cause) = self.commit(RecordKind::Claim, intent.operation_id, intent.digest, mutation) {
            return ClaimOutcome::Refused(cause);
        }

        if intent.opcode == Opcode::StartUpload {
            // §12's lazy shards: the leaf's directory has to exist before the leaf does, and
            // admission is where §11 and §12 put that obligation. The writer asks again before its
            // first byte; `make_dir` on a directory that is already there "is not an error".
            if let Err(error) = self.media.ensure_shards(generation) {
                let _ = error;
                return ClaimOutcome::Refused(FailureCause::MediaIo { detail: detail::media_io::WRITE });
            }
            if let Err(error) = self.media.open_generation(generation) {
                let _ = error;
                return ClaimOutcome::Refused(FailureCause::MediaIo { detail: detail::media_io::WRITE });
            }
            let generation_intent = Intent {
                store: self.model.store,
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
            match GenerationWriter::begin::<M::Error>(generation_intent) {
                Ok((writer, capability)) => {
                    self.writing =
                        Some(Writing { operation: intent.operation_id, writer, capability, generation, sealed: false });
                }
                Err(_) => {
                    return ClaimOutcome::Refused(FailureCause::ResourceLimit {
                        detail: detail::resource::OBJECT_LENGTH,
                    })
                }
            }
        }
        let live = Live {
            operation: intent.operation_id,
            phase: wire_phase(phase),
            durable_offset: 0,
            checkpoint_sequence: 0,
            target: intent.target_operation_id,
            disposition: None,
        };
        match self.live.iter_mut().find(|slot| slot.is_none()) {
            Some(slot) => *slot = Some(live),
            // The durable claim is bounded by the same §5.1 capacity this array is, and admission
            // refused above once the projection was full, so there is always a slot.
            None => return ClaimOutcome::Refused(FailureCause::Internal { detail: detail::internal::INVARIANT }),
        }
        ClaimOutcome::Claimed { logical_object_id, repository_revision: Revision::new(self.revision) }
    }

    /// Whether an `AbortOperation` may name this target at all (§3's ownership, §9's
    /// non-cancellable `InstallUpdate`). `None` means it may.
    fn target_admissibility(&self, target: OperationId, principal: PrincipalScope) -> Option<FailureCause> {
        let active = self.active(target);
        let retained = self.model.result_for(target);
        let owner = active.map(|row| row.principal).or_else(|| retained.map(|result| result.principal));
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
    fn restart_work(&mut self, operation_id: OperationId) -> Result<(), FailureCause> {
        if let Some(writing) = self.writing.as_mut() {
            if writing.operation == operation_id {
                let capability = writing.capability;
                match writing.writer.restart(capability, &mut self.media) {
                    Ok(fresh) => writing.capability = fresh,
                    Err(error) => return Err(write_failure(&error)),
                }
                writing.sealed = false;
            }
        }
        if let Some(live) = self.live_mut(operation_id) {
            live.phase = Phase::Prepared;
            live.durable_offset = 0;
            live.checkpoint_sequence = 0;
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
        // §7 runs domain validation only after the seal, over bytes the card is known to hold, so
        // what the validator is handed is the sealed generation's own length and CRC. A direct
        // mutation has no generation of its own and is validated on its request alone.
        let (generation, length, crc) = match self.writing.as_ref().filter(|writing| writing.sealed) {
            Some(writing) => (writing.generation, writing.writer.written(), writing.writer.declared_crc()),
            None => (row.generation, 0, 0),
        };
        match self.validator.validate(kind, generation, length, crc) {
            Ok(()) => {
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
            // A device-local producer commits a competing mutation just before the commit lock.
            let existing = *self.model.head(key).expect("the raced head exists");
            let head = CatalogHead { revision: Revision::new(self.revision + 1), ..existing };
            if self.commit_local_publication(head, None).is_err() {
                return Outcome::Failed(FailureCause::MediaIo { detail: detail::media_io::WRITE });
            }
        }

        // The compare-and-swap is rechecked here, under the commit lock, exactly as §6.3 requires.
        if let Some(expected) = replace_expectation(&row, opcode) {
            let current = self.model.head(key).map_or(Revision::ZERO, |head| head.revision);
            if current.get() != expected {
                return Outcome::Failed(FailureCause::RevisionConflict { detail: detail::revision::OBJECT, current });
            }
        }

        let principal = row.principal;
        let digest = row.intent;
        let envelope = match opcode {
            Opcode::StartUpload => {
                let Some(writing) = self.writing.take() else {
                    return Outcome::Failed(FailureCause::Internal { detail: detail::internal::INVARIANT });
                };
                let length = writing.writer.written();
                let crc = writing.writer.declared_crc();
                let revision = Revision::new(self.revision + 1);
                let head = CatalogHead {
                    key,
                    flags: 0,
                    revision,
                    generation: writing.generation,
                    length,
                    crc,
                    envelope_len: PLACEHOLDER_ENVELOPE.len() as u16,
                    envelope: envelope_reservation(),
                    resolution: GenerationId::ZERO,
                };
                let result = ObjectResult {
                    operation_id,
                    store_id: self.model.store,
                    kind,
                    outcome: ObjectOutcome::Committed,
                    logical_object_id,
                    revision,
                    length,
                    crc32: crc,
                };
                let envelope = ResultEnvelope::Object(result);
                if self.commit_publication(operation_id, digest, principal, Some(head), envelope).is_err() {
                    return Outcome::Failed(FailureCause::MediaIo { detail: detail::media_io::WRITE });
                }
                envelope
            }
            Opcode::DeleteObject => {
                let previous = self.model.head(key).map(|head| (head.length, head.crc)).unwrap_or((0, 0));
                let revision = Revision::new(self.revision + 1);
                let envelope = ResultEnvelope::Object(ObjectResult {
                    operation_id,
                    store_id: self.model.store,
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
                let Some(existing) = self.model.head(key).copied() else {
                    return Outcome::Failed(FailureCause::ObjectNotFound { detail: detail::not_found::LOGICAL_OBJECT });
                };
                let revision = Revision::new(self.revision + 1);
                let head = CatalogHead { revision, ..existing };
                let envelope = ResultEnvelope::Object(ObjectResult {
                    operation_id,
                    store_id: self.model.store,
                    kind,
                    outcome: ObjectOutcome::MetadataChanged,
                    logical_object_id,
                    revision,
                    length: existing.length,
                    crc32: existing.crc,
                });
                if self.commit_publication(operation_id, digest, principal, Some(head), envelope).is_err() {
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
                let revision = Revision::new(self.revision + 1);
                let envelope = ResultEnvelope::Object(ObjectResult {
                    operation_id,
                    store_id: self.model.store,
                    kind,
                    outcome,
                    logical_object_id,
                    revision,
                    length: 0,
                    crc32: 0,
                });
                if self.commit_publication(operation_id, digest, principal, None, envelope).is_err() {
                    return Outcome::Failed(FailureCause::MediaIo { detail: detail::media_io::WRITE });
                }
                envelope
            }
            Opcode::AbortOperation => {
                let live = self.live_for(operation_id);
                let disposition = live.and_then(|live| live.disposition).unwrap_or(AbortDisposition::AlreadyAbsent);
                let envelope = ResultEnvelope::Abort(AbortResult {
                    operation_id,
                    store_id: self.model.store,
                    // §6.4 gives the abort command's own result the target it named.
                    target_operation_id: live.and_then(|live| live.target).unwrap_or(OperationId::ZERO),
                    disposition,
                });
                if self.commit_publication(operation_id, digest, principal, None, envelope).is_err() {
                    return Outcome::Failed(FailureCause::MediaIo { detail: detail::media_io::WRITE });
                }
                envelope
            }
            _ => return Outcome::Failed(FailureCause::Internal { detail: detail::internal::INVARIANT }),
        };
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
        if self.active(operation_id).is_some() {
            if let Some(cause) = self.hooks.aborting() {
                return Outcome::Failed(cause);
            }
        }
        let terminal = cause.terminal();
        if self.active(operation_id).is_some() && self.finish_claim(operation_id, Err(terminal)).is_err() {
            return Outcome::Failed(FailureCause::MediaIo { detail: detail::media_io::WRITE });
        }
        Outcome::Aborted(terminal)
    }

    // -- downloads ------------------------------------------------------------------------------

    fn resolve<'s>(&mut self, kind: ObjectKind, logical_object_id: LogicalObjectId) -> Outcome<'s> {
        let key = HeadKey { kind: kind.to_u16(), id: logical_object_id };
        let Some(head) = self.model.head(key).copied() else {
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
        if let ReleaseEffect::Retention(change) = self.leases.release(pinned.handle, &self.model.retained) {
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
                if echoed != self.model.store {
                    return DeviceControlAnswer::Refused(FailureCause::MediaUnavailable {
                        detail: detail::media::UNMOUNTED,
                    });
                }
                let store = self.hooks.mint_store_id(self.model.store);
                if self.media.reset_store(store).is_err() {
                    return DeviceControlAnswer::Refused(FailureCause::MediaIo { detail: detail::media_io::WRITE });
                }
                // §16: reset destroys "every object, operation result, and lease". The projection
                // is rebuilt as §12's first checkpoint, and nothing of the old store survives it.
                self.model.reset_to_initial(store, ObjectKind::Weather.to_u16());
                self.sequence = 1;
                self.revision = 0;
                self.next_logical_id = 1;
                self.leases.clear();
                self.pinned = None;
                self.live = [None; MAX_ACTIVE_OPERATIONS];
                self.writing = None;
                self.status.store_id = store;
                DeviceControlAnswer::ResetStore(store)
            }
        }
    }

    /// The report an operation gets when `principal` asks about it (§3: authorization precedes
    /// status, so a foreign principal never learns whether the ID exists).
    pub fn report_for(&self, operation_id: OperationId, principal: PrincipalScope) -> OperationReport {
        let owner = self
            .active(operation_id)
            .map(|row| row.principal)
            .or_else(|| self.model.result_for(operation_id).map(|result| result.principal));
        match owner {
            Some(owner) if owner != principal_bytes(principal) => OperationReport::NotAuthorized,
            _ => self.report(operation_id),
        }
    }

    fn report(&self, operation_id: OperationId) -> OperationReport {
        if let Some(row) = self.active(operation_id) {
            // §8.1's matrix fixes every field from the originating claim, and a row outside it "is
            // an internal state/codec error and MUST NOT be emitted".
            if row.opcode == Opcode::AbortOperation.to_u16() {
                return OperationReport::InProgress(OperationProgress {
                    namespace: SubjectNamespace::None,
                    subject_kind: 0,
                    phase: Phase::Aborting,
                    flags: 0,
                    logical_object_id: LogicalObjectId::ZERO,
                    durable_offset: 0,
                });
            }
            let live = self.live_for(operation_id);
            let phase = live.map_or(wire_phase(row.phase), |live| live.phase);
            let is_upload = row.opcode == Opcode::StartUpload.to_u16();
            let mut flags = progress_flags::LOGICAL_ID_PRESENT;
            if is_upload && !matches!(phase, Phase::Aborting) {
                // "attached only while that session exists; ... aborting has no attachment".
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
            return OperationReport::InProgress(OperationProgress {
                namespace: SubjectNamespace::Logical,
                subject_kind: row.subject_kind,
                phase,
                flags,
                logical_object_id: LogicalObjectId::new(row.logical_id),
                durable_offset,
            });
        }
        match self.model.result_for(operation_id) {
            Some(result) => match decode_result(result) {
                Ok(envelope) => OperationReport::Committed(envelope),
                Err(terminal) => OperationReport::Aborted(terminal),
            },
            // §8.1: "Unknown means only that the ID is neither active nor retained. It cannot
            // distinguish never claimed from evicted."
            None => OperationReport::Unknown,
        }
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
        if self.sequence == 0 || self.sequence > JOURNAL_SLOTS as u64 {
            // §6.3's compaction is what frees the journal ring, and it is a store-level background
            // pass with its own budget rather than something a client's FinishUpload should pay
            // for. Until #1359 owns that pass, the ring is a hard bound and reaching it is a
            // resource limit — never a wrapped slot that would make recovery choose a suffix that
            // spans two epochs.
            return Err(FailureCause::ResourceLimit { detail: detail::resource::NORMAL_OPERATION_CLAIMS });
        }
        let slot = (self.sequence - 1) as u16;
        let record = JournalBody {
            store: self.model.store,
            epoch: self.model.epoch,
            sequence: self.sequence,
            slot,
            kind,
            operation,
            intent,
            mutation,
        };
        let body = record.encode_body();
        let gate = record.gate_for(&body).encode();
        if self.media.append_journal(slot, &body, &gate).is_err() {
            return Err(FailureCause::MediaIo { detail: detail::media_io::WRITE });
        }
        if self.model.apply(&record).is_err() {
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
    ) -> Result<(), FailureCause> {
        let revision = self.revision + 1;
        let kind = head.map_or_else(|| self.active(operation).map_or(0, |row| row.subject_kind), |head| head.key.kind);
        let mut mutation = Mutation {
            active: Some(Change::Remove(operation)),
            repository: Some(RepositoryChange { kind, revision: Some(revision), next_logical_id: None, flags: 0 }),
            result: Some(self.result_entry(operation, intent, principal, Ok(envelope))),
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
        self.revision = revision;
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
            head: self.model.head(key).map(|_| Change::Remove(key)),
            repository: Some(RepositoryChange {
                kind: key.kind,
                revision: Some(revision),
                next_logical_id: None,
                flags: 0,
            }),
            result: Some(self.result_entry(operation, intent, principal, Ok(envelope))),
            ..Mutation::default()
        };
        let displaced = self.model.head(key).map(|head| head.generation);
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
        let result = self.result_entry(operation, row.intent, row.principal, outcome);
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
    /// The terminal record carries **no** retained result. That is the one place this path differs
    /// from a wire operation's, and it is deliberate: a local publication answers no client, so
    /// there is no typed result for §8.1's window to hold. The identity it claims under is derived
    /// from the generation so two local publications can never collide.
    fn commit_local_publication(
        &mut self,
        head: CatalogHead,
        reserve: Option<GenerationId>,
    ) -> Result<(), FailureCause> {
        let operation = local_operation_id(head.generation);
        let row = ActiveOperation {
            operation,
            intent: LOCAL_INTENT,
            principal: [0; 32],
            opcode: Opcode::StartUpload.to_u16(),
            subject_kind: head.key.kind,
            phase: OperationPhase::Sealed,
            flags: if reserve.is_some() { ActiveOperation::FLAG_GENERATION_RESERVED } else { 0 },
            logical_id: head.key.id.get(),
            expected_revision: 0,
            generation: head.generation,
            progress_counter: 0,
            work_sequence: 0,
            abort_reason: 0,
        };
        let claim = Mutation {
            active: Some(Change::Put(row)),
            generation_cursor: reserve.map(|generation| generation.get() + 1),
            ..Mutation::default()
        };
        self.commit(RecordKind::Claim, operation, LOCAL_INTENT, claim)?;

        let revision = self.revision + 1;
        let mut mutation = Mutation {
            active: Some(Change::Remove(operation)),
            head: Some(Change::Put(head)),
            repository: Some(RepositoryChange {
                kind: head.key.kind,
                revision: Some(revision),
                next_logical_id: Some(self.next_logical_id),
                flags: 0,
            }),
            ..Mutation::default()
        };
        if let Some(retention) = self.retention_for_displaced(head.key, head.generation) {
            mutation.retained = Some(Change::Put(retention));
        }
        let displaced = self.displaced_generation(head.key, head.generation);
        self.commit(RecordKind::Terminal, operation, LOCAL_INTENT, mutation)?;
        self.revision = revision;
        if let Some(generation) = displaced {
            if !self.leases.holds(generation) {
                let _ = self.media.collect_generation(generation);
            }
        }
        Ok(())
    }

    /// The generation a publication over `key` displaces, when there is one and it is not the same.
    fn displaced_generation(&self, key: HeadKey, incoming: GenerationId) -> Option<GenerationId> {
        let head = self.model.head(key)?;
        (head.generation != incoming).then_some(head.generation)
    }

    /// §9's retained-previous entry, when a live lease still pins the bytes being displaced.
    fn retention_for_displaced(&self, key: HeadKey, incoming: GenerationId) -> Option<RetainedPrevious> {
        let head = *self.model.head(key)?;
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
            kind: head.key.kind,
            logical_id: head.key.id,
            generation: head.generation,
            length: head.length,
            crc: head.crc,
            retain_through: 0,
            object_revision: head.revision,
        })
    }

    fn result_entry(
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
            commit_sequence: self.model.terminal_counter + 1,
            operation,
            intent,
            principal,
            committed,
            result_type,
            body,
        }
    }

    fn committed_result(
        &self,
        operation: OperationId,
        intent: [u8; 32],
        principal: PrincipalScope,
        envelope: ResultEnvelope,
    ) -> TerminalResult {
        self.result_entry(operation, intent, principal_bytes(principal), Ok(envelope))
    }

    // -- small helpers ----------------------------------------------------------------------------

    fn active(&self, operation: OperationId) -> Option<&ActiveOperation> {
        self.model.actives.iter().find(|row| row.operation == operation)
    }

    fn reserve_generation(&mut self) -> GenerationId {
        GenerationId::new(self.model.next_generation)
    }

    fn allocate_logical_id(&mut self) -> LogicalObjectId {
        let id = LogicalObjectId::new(self.next_logical_id);
        self.next_logical_id += 1;
        id
    }

    fn live_for(&self, operation: OperationId) -> Option<Live> {
        self.live.iter().flatten().find(|live| live.operation == operation).copied()
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
            Command::QueryOperation { operation_id, principal } => {
                Outcome::OperationReport(self.report_for(operation_id, principal))
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Free functions
// ---------------------------------------------------------------------------------------------

/// The 32-byte principal digest a §5.3 row stores, from the 16-byte scope the adapter established.
///
/// §5.3 reserves 32 bytes for "the opaque stable principal-scope digest"; the wire's scope is 16.
/// Left-aligned and zero-filled, so the mapping is total, injective and stable across a mount.
fn principal_bytes(principal: PrincipalScope) -> [u8; 32] {
    let mut out = [0u8; 32];
    out[..16].copy_from_slice(principal.as_bytes());
    out
}

/// The canonical-intent digest a device-local publication claims under. It is a constant because
/// there is no request to canonicalize: §11's digest exists to compare two client intents, and a
/// local producer has none to be confused with.
const LOCAL_INTENT: [u8; 32] = [0xA7; 32];

/// The identity a device-local publication claims under, derived from the generation it creates.
///
/// §11 requires every claim to have an OperationId, and two local publications must never share
/// one. Deriving it from the reserved generation makes that structural: generations are never
/// reused, so neither are these.
fn local_operation_id(generation: GenerationId) -> OperationId {
    let mut bytes = [0xFFu8; 16];
    bytes[8..].copy_from_slice(&generation.get().to_be_bytes());
    OperationId::new(bytes)
}

fn envelope_reservation() -> [u8; CatalogHead::ENVELOPE_CAPACITY] {
    let mut envelope = [0u8; CatalogHead::ENVELOPE_CAPACITY];
    envelope[..PLACEHOLDER_ENVELOPE.len()].copy_from_slice(&PLACEHOLDER_ENVELOPE);
    envelope
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
