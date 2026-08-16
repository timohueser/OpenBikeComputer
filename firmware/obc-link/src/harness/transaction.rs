//! An in-memory transaction with the DOS2 lifecycle's shape.
//!
//! It is the thing slice 5 replaces with the real kernel, so it is deliberately built to the seam
//! rather than to the test: claim → append → checkpoint → seal → validate → publish/abort, with
//! exactly-once results in a 64-entry ring and a compare-and-swap on every publication. Where it
//! overlaps `obc-storage`'s `obc2` model it mirrors that model's semantics — ring append
//! "overwrites `result_start` and advances that index by one", and a publication rechecks the
//! expected Revision under the commit lock — because the engine must not have to change shape when
//! the real one arrives.
//!
//! What it deliberately is not: durable. It holds no work across a link teardown, which is exactly
//! the restart-only profile §6.1 freezes for the first device.

use std::vec;
use std::vec::Vec;

use crate::control::{ClockStatus, ConfigBlock, DeviceStatus};
use crate::engine::{
    AbortCause, ClaimIntent, ClaimOutcome, Command, DeviceControlAnswer, DeviceControlRequest, FailureCause,
    OperationReport, Outcome, PinnedSource, PrincipalScope, TerminalError,
};
use crate::error::detail;
use crate::frame::Opcode;
use crate::ids::{GenerationId, LogicalObjectId, OperationId, Revision, StoreId};
use crate::query::{progress_flags, OperationProgress};
use crate::registry::{ObjectKind, ObjectOutcome, Phase, SubjectNamespace};
use crate::result::{ObjectResult, ResultEnvelope};
use crate::upload::Target;

/// §5.1's retained terminal results, and §2 of the storage contract's ring.
pub const RESULT_RING: usize = 64;

/// §5.1's normal active claimed operations.
pub const ACTIVE_CLAIMS: usize = 8;

/// One published head.
#[derive(Debug, Clone)]
struct Head {
    kind: ObjectKind,
    logical_object_id: LogicalObjectId,
    revision: Revision,
    generation: GenerationId,
    length: u64,
    crc32: u32,
}

/// One immutable physical generation. A pinned reader keeps reading it after a replace or delete.
#[derive(Debug, Clone)]
struct Generation {
    id: GenerationId,
    bytes: Vec<u8>,
}

/// One live claim and, for an upload, its work record.
#[derive(Debug, Clone)]
struct Claim {
    intent: ClaimIntent,
    logical_object_id: LogicalObjectId,
    buffer: Vec<u8>,
    durable_offset: u64,
    checkpoint_sequence: u32,
    sealed: bool,
    phase: Phase,
}

/// One retained terminal result (§8.1's 64-result window).
#[derive(Debug, Clone)]
struct Retained {
    operation_id: OperationId,
    principal: PrincipalScope,
    digest: [u8; 32],
    outcome: Result<ResultEnvelope, TerminalError>,
}

/// The RAM lease a download holds over one generation (§7).
#[derive(Debug, Clone, Copy)]
struct Lease {
    generation: GenerationId,
}

/// Failures a test asks the transaction to produce, so the engine's unwind paths are exercised.
#[derive(Debug, Clone, Copy, Default)]
pub struct Faults {
    /// Fail the append that would cross this offset.
    pub fail_append_at: Option<u64>,
    /// Fail the seal, as a whole-payload checksum failure.
    pub fail_seal: bool,
    /// Fail the typed validator with this kind-scoped detail.
    pub fail_validation: Option<u16>,
    /// Fail the publication as a media write.
    pub fail_publication: bool,
    /// Refuse the next claim in preflight, before any state exists.
    pub refuse_claim: Option<FailureCause>,
    /// Publish a competing revision just before the commit lock, as a device-local producer would.
    pub race_publication: bool,
}

/// The in-memory transaction.
#[derive(Debug)]
pub struct FakeTransaction {
    store_id: StoreId,
    heads: Vec<Head>,
    generations: Vec<Generation>,
    claims: Vec<Claim>,
    results: Vec<Retained>,
    result_start: usize,
    lease: Option<Lease>,
    next_logical_id: u64,
    next_generation: u64,
    repository_revision: u64,
    /// Injected failures.
    pub faults: Faults,
    /// The device-control plane's config block.
    pub config: ConfigBlock,
    /// The device-control plane's status.
    pub status: DeviceStatus,
    /// The device-control plane's clock.
    pub clock: ClockStatus,
}

impl FakeTransaction {
    /// An empty store.
    pub fn new(store_id: StoreId) -> Self {
        FakeTransaction {
            store_id,
            heads: Vec::new(),
            generations: Vec::new(),
            claims: Vec::new(),
            results: Vec::new(),
            result_start: 0,
            lease: None,
            next_logical_id: 1,
            next_generation: 1,
            repository_revision: 0,
            faults: Faults::default(),
            config: ConfigBlock {
                unit_flags: 0,
                weather_refresh: crate::control::WeatherRefresh::Off,
                name: [0; crate::control::MAX_DEVICE_NAME],
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
                status_flags: crate::control::status_flags::CARD_PRESENT,
                mount_class: crate::control::MountClass::Mounted,
                firmware_build: 1,
                store_id,
            },
            clock: ClockStatus {
                epoch_seconds: 1_700_000_000,
                source: crate::control::ClockSource::Companion,
                state: crate::control::ClockState::Trusted,
            },
        }
    }

    /// The store's identity.
    pub fn store_id(&self) -> StoreId {
        self.store_id
    }

    /// The current head of one logical object, as `(revision, length, crc)`.
    pub fn head(&self, kind: ObjectKind, logical_object_id: LogicalObjectId) -> Option<(Revision, u64, u32)> {
        self.heads
            .iter()
            .find(|head| head.kind == kind && head.logical_object_id == logical_object_id)
            .map(|head| (head.revision, head.length, head.crc32))
    }

    /// The bytes of a published head, for a test that wants to compare what it uploaded.
    pub fn payload(&self, kind: ObjectKind, logical_object_id: LogicalObjectId) -> Option<&[u8]> {
        let head = self.heads.iter().find(|head| head.kind == kind && head.logical_object_id == logical_object_id)?;
        self.generation_bytes(head.generation)
    }

    /// How many terminal results are retained. The ring never grows past [`RESULT_RING`].
    pub fn retained_results(&self) -> usize {
        self.results.len()
    }

    /// True when the operation's result is still inside the retained window.
    pub fn retains(&self, operation_id: OperationId) -> bool {
        self.results.iter().any(|retained| retained.operation_id == operation_id)
    }

    /// True while a reader lease is held.
    pub fn has_lease(&self) -> bool {
        self.lease.is_some()
    }

    /// Publishes a head directly, as a device-local producer does. Returns its new Revision.
    pub fn publish_local(&mut self, kind: ObjectKind, bytes: &[u8]) -> (LogicalObjectId, Revision) {
        let logical_object_id = LogicalObjectId::new(self.next_logical_id);
        self.next_logical_id += 1;
        let revision = self.commit_head(kind, logical_object_id, bytes);
        (logical_object_id, revision)
    }

    /// Retains one terminal result under a synthetic identity, as any device-local producer's
    /// terminal commit does. §8.1: the window "is store-global in the strict sense".
    pub fn retain_local_result(&mut self, operation_id: OperationId) {
        let envelope = ResultEnvelope::Object(ObjectResult {
            operation_id,
            store_id: self.store_id,
            kind: ObjectKind::Ride,
            outcome: ObjectOutcome::Committed,
            logical_object_id: LogicalObjectId::ZERO,
            revision: Revision::ZERO,
            length: 0,
            crc32: 0,
        });
        self.retain(Retained {
            operation_id,
            principal: PrincipalScope::new([0; 16]),
            digest: [0; 32],
            outcome: Ok(envelope),
        });
    }

    /// Executes one engine command. `scratch` carries any bytes the outcome hands back.
    pub fn execute<'s>(&mut self, command: Command<'_>, scratch: &'s mut [u8]) -> Outcome<'s> {
        match command {
            Command::Claim(intent) => Outcome::Claim(self.claim(intent)),
            Command::Append { offset, bytes } => self.append(offset, bytes),
            Command::Checkpoint { offset } => self.checkpoint(offset),
            Command::Seal { declared_length, expected_crc } => self.seal(declared_length, expected_crc),
            Command::Validate => self.validate(),
            Command::Publish => self.publish(),
            Command::Abort(cause) => self.abort(cause),
            Command::Resolve { kind, logical_object_id, .. } => self.resolve(kind, logical_object_id),
            Command::ReadSource { offset, length } => self.read_source(offset, length, scratch),
            Command::ReleaseLease => {
                self.lease = None;
                Outcome::LeaseReleased
            }
            Command::DeviceControl(request) => Outcome::DeviceControl(self.device_control(request, scratch)),
            Command::QueryOperation(operation_id) => Outcome::OperationReport(self.report(operation_id)),
        }
    }

    // -- the claim lock ---------------------------------------------------------------------------

    fn claim(&mut self, intent: ClaimIntent) -> ClaimOutcome {
        if let Some(index) = self.claims.iter().position(|claim| claim.intent.operation_id == intent.operation_id) {
            let claim = &self.claims[index];
            // §11 actions 2 and 4 precede action 3: a foreign principal never learns the intent.
            if claim.intent.principal != intent.principal {
                return ClaimOutcome::ForeignPrincipal;
            }
            if claim.intent.digest != intent.digest {
                return ClaimOutcome::Conflict;
            }
            let logical_object_id = claim.logical_object_id;
            // Restart-only: the work this claim already holds is discarded and started again.
            self.claims[index].buffer.clear();
            self.claims[index].durable_offset = 0;
            self.claims[index].sealed = false;
            self.claims[index].phase = Phase::Prepared;
            return ClaimOutcome::Restarted {
                logical_object_id,
                repository_revision: Revision::new(self.repository_revision),
            };
        }
        if let Some(retained) = self.results.iter().find(|retained| retained.operation_id == intent.operation_id) {
            if retained.principal != intent.principal {
                return ClaimOutcome::ForeignPrincipal;
            }
            if retained.digest != intent.digest {
                return ClaimOutcome::Conflict;
            }
            return match retained.outcome {
                Ok(envelope) => ClaimOutcome::Committed(envelope),
                Err(terminal) => ClaimOutcome::Aborted(terminal),
            };
        }
        if let Some(cause) = self.faults.refuse_claim.take() {
            // Preflight: "may fail without creating state".
            return ClaimOutcome::Refused(cause);
        }
        if self.claims.len() == ACTIVE_CLAIMS {
            return ClaimOutcome::Refused(FailureCause::ResourceLimit {
                detail: detail::resource::NORMAL_OPERATION_CLAIMS,
            });
        }
        let logical_object_id = match intent.target {
            Target::Create => {
                let id = LogicalObjectId::new(self.next_logical_id);
                self.next_logical_id += 1;
                id
            }
            Target::Replace { logical_object_id, .. } => logical_object_id,
        };
        self.claims.push(Claim {
            intent,
            logical_object_id,
            buffer: Vec::new(),
            durable_offset: 0,
            checkpoint_sequence: 0,
            sealed: false,
            phase: if intent.opcode == Opcode::StartUpload { Phase::Prepared } else { Phase::Validating },
        });
        ClaimOutcome::Claimed { logical_object_id, repository_revision: Revision::new(self.repository_revision) }
    }

    // -- the work record --------------------------------------------------------------------------

    fn append<'s>(&mut self, offset: u64, bytes: &[u8]) -> Outcome<'s> {
        if let Some(at) = self.faults.fail_append_at {
            if offset + bytes.len() as u64 > at {
                return Outcome::Failed(FailureCause::MediaIo { detail: detail::media_io::WRITE });
            }
        }
        let Some(claim) = self.active_upload_mut() else {
            return Outcome::Failed(FailureCause::Internal { detail: detail::internal::INVARIANT });
        };
        if offset != claim.buffer.len() as u64 {
            return Outcome::Failed(FailureCause::Internal { detail: detail::internal::INVARIANT });
        }
        claim.buffer.extend_from_slice(bytes);
        claim.phase = Phase::Streaming;
        Outcome::Appended
    }

    fn checkpoint<'s>(&mut self, offset: u64) -> Outcome<'s> {
        let Some(claim) = self.active_upload_mut() else {
            return Outcome::Failed(FailureCause::Internal { detail: detail::internal::INVARIANT });
        };
        if offset > claim.buffer.len() as u64 {
            return Outcome::Failed(FailureCause::Internal { detail: detail::internal::INVARIANT });
        }
        claim.durable_offset = offset;
        claim.checkpoint_sequence += 1;
        let prefix_crc = obc_crc::crc32(&claim.buffer[..offset as usize]);
        Outcome::Checkpointed { durable_offset: offset, prefix_crc, sequence: claim.checkpoint_sequence }
    }

    fn seal<'s>(&mut self, declared_length: u64, expected_crc: u32) -> Outcome<'s> {
        if self.faults.fail_seal {
            return Outcome::Failed(FailureCause::Checksum { detail: detail::checksum::WHOLE_PAYLOAD });
        }
        let Some(claim) = self.active_upload_mut() else {
            return Outcome::Failed(FailureCause::Internal { detail: detail::internal::INVARIANT });
        };
        if claim.buffer.len() as u64 != declared_length || obc_crc::crc32(&claim.buffer) != expected_crc {
            return Outcome::Failed(FailureCause::Checksum { detail: detail::checksum::WHOLE_PAYLOAD });
        }
        claim.sealed = true;
        claim.phase = Phase::Sealed;
        Outcome::Sealed
    }

    fn validate<'s>(&mut self) -> Outcome<'s> {
        let kind = self.active_claim().map(|claim| claim.intent.kind).unwrap_or(ObjectKind::Route);
        if let Some(detail) = self.faults.fail_validation {
            return Outcome::Failed(FailureCause::SemanticValidation { kind, detail });
        }
        if let Some(claim) = self.active_claim_mut() {
            claim.phase = Phase::Validating;
        }
        Outcome::Validated
    }

    fn publish<'s>(&mut self) -> Outcome<'s> {
        if self.faults.fail_publication {
            return Outcome::Failed(FailureCause::MediaIo { detail: detail::media_io::WRITE });
        }
        let Some(claim) = self.active_claim().cloned() else {
            return Outcome::Failed(FailureCause::Internal { detail: detail::internal::INVARIANT });
        };
        if self.faults.race_publication {
            // A device-local producer commits a competing mutation just before the commit lock.
            self.faults.race_publication = false;
            let bytes =
                self.payload(claim.intent.kind, claim.logical_object_id).map(<[u8]>::to_vec).unwrap_or_default();
            self.commit_head(claim.intent.kind, claim.logical_object_id, &bytes);
        }
        // The compare-and-swap is rechecked here, under the commit lock, exactly as §6.3 requires.
        if let Target::Replace { logical_object_id, expected_revision } = claim.intent.target {
            let current = self.head(claim.intent.kind, logical_object_id).map(|(revision, _, _)| revision);
            let current = current.unwrap_or(Revision::ZERO);
            if current != expected_revision {
                return Outcome::Failed(FailureCause::RevisionConflict { detail: detail::revision::OBJECT, current });
            }
        }
        let envelope = match claim.intent.opcode {
            Opcode::StartUpload => {
                let revision = self.commit_head(claim.intent.kind, claim.logical_object_id, &claim.buffer);
                self.object_result(
                    &claim,
                    ObjectOutcome::Committed,
                    revision,
                    claim.buffer.len() as u64,
                    obc_crc::crc32(&claim.buffer),
                )
            }
            Opcode::DeleteObject => {
                let previous = self.head(claim.intent.kind, claim.logical_object_id);
                self.remove_head(claim.intent.kind, claim.logical_object_id);
                let (length, crc32) = previous.map(|(_, length, crc)| (length, crc)).unwrap_or((0, 0));
                let revision = self.bump_repository();
                self.object_result(&claim, ObjectOutcome::Deleted, revision, length, crc32)
            }
            Opcode::SetMetadata => {
                let previous = self.head(claim.intent.kind, claim.logical_object_id);
                let revision = self.bump_head_revision(claim.intent.kind, claim.logical_object_id);
                let (length, crc32) = previous.map(|(_, length, crc)| (length, crc)).unwrap_or((0, 0));
                self.object_result(&claim, ObjectOutcome::MetadataChanged, revision, length, crc32)
            }
            Opcode::InstallUpdate => {
                let revision = self.bump_repository();
                self.object_result(&claim, ObjectOutcome::UpdateInstallRequested, revision, 0, 0)
            }
            Opcode::AcknowledgeRideImported => {
                let revision = self.bump_repository();
                self.object_result(&claim, ObjectOutcome::RideImported, revision, 0, 0)
            }
            Opcode::AbortOperation => {
                let target = claim.intent.target;
                let _ = target;
                let revision = self.bump_repository();
                self.object_result(&claim, ObjectOutcome::Committed, revision, 0, 0)
            }
            _ => return Outcome::Failed(FailureCause::Internal { detail: detail::internal::INVARIANT }),
        };
        self.finish_claim(claim.intent.operation_id, Ok(envelope));
        Outcome::Published(envelope)
    }

    fn abort<'s>(&mut self, cause: AbortCause) -> Outcome<'s> {
        let terminal = cause.terminal();
        if let Some(claim) = self.active_claim().cloned() {
            self.finish_claim(claim.intent.operation_id, Err(terminal));
        }
        Outcome::Aborted(terminal)
    }

    // -- downloads ---------------------------------------------------------------------------------

    fn resolve<'s>(&mut self, kind: ObjectKind, logical_object_id: LogicalObjectId) -> Outcome<'s> {
        let Some(head) =
            self.heads.iter().find(|head| head.kind == kind && head.logical_object_id == logical_object_id)
        else {
            return Outcome::Failed(FailureCause::ObjectNotFound { detail: detail::not_found::LOGICAL_OBJECT });
        };
        // The lease is a RAM capability over the head generation this response reports (§7).
        self.lease = Some(Lease { generation: head.generation });
        Outcome::Resolved(PinnedSource {
            logical_object_id,
            revision: head.revision,
            total_length: head.length,
            crc32: head.crc32,
        })
    }

    fn read_source<'s>(&mut self, offset: u64, length: u16, scratch: &'s mut [u8]) -> Outcome<'s> {
        let Some(lease) = self.lease else {
            return Outcome::Failed(FailureCause::Internal { detail: detail::internal::INVARIANT });
        };
        let Some(bytes) = self.generation_bytes(lease.generation) else {
            return Outcome::Failed(FailureCause::MediaIo { detail: detail::media_io::READ });
        };
        let start = offset as usize;
        let end = (start + usize::from(length)).min(bytes.len());
        let taken = end.saturating_sub(start);
        scratch[..taken].copy_from_slice(&bytes[start..end]);
        Outcome::SourceBytes { offset, bytes: &scratch[..taken] }
    }

    // -- device control and queries -----------------------------------------------------------------

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
                scratch[..payload.len()].copy_from_slice(payload);
                DeviceControlAnswer::Echo(&scratch[..payload.len()])
            }
            DeviceControlRequest::ResetStore(echoed) => {
                if echoed != self.store_id {
                    return DeviceControlAnswer::Refused(FailureCause::MediaUnavailable {
                        detail: detail::media::UNMOUNTED,
                    });
                }
                let new_store = StoreId::new([0x5A; 16]);
                *self = FakeTransaction::new(new_store);
                DeviceControlAnswer::ResetStore(new_store)
            }
        }
    }

    fn report(&self, operation_id: OperationId) -> OperationReport {
        if let Some(claim) = self.claims.iter().find(|claim| claim.intent.operation_id == operation_id) {
            let mut flags = progress_flags::LOGICAL_ID_PRESENT;
            if claim.intent.opcode == Opcode::StartUpload {
                flags |= progress_flags::SESSION_ATTACHED;
            }
            return OperationReport::InProgress(OperationProgress {
                namespace: SubjectNamespace::Logical,
                subject_kind: claim.intent.kind.to_u16(),
                phase: claim.phase,
                flags,
                logical_object_id: claim.logical_object_id,
                durable_offset: if claim.intent.opcode == Opcode::StartUpload { claim.durable_offset } else { 0 },
            });
        }
        match self.results.iter().find(|retained| retained.operation_id == operation_id) {
            Some(retained) => match retained.outcome {
                Ok(envelope) => OperationReport::Committed(envelope),
                Err(terminal) => OperationReport::Aborted(terminal),
            },
            // §8.1: "Unknown means only that the ID is neither active nor retained. It cannot
            // distinguish never claimed from evicted."
            None => OperationReport::Unknown,
        }
    }

    /// The report an operation gets when `principal` asks about it (§3: authorization precedes
    /// status, so a foreign principal never learns whether the ID exists).
    pub fn report_for(&self, operation_id: OperationId, principal: PrincipalScope) -> OperationReport {
        let owner = self
            .claims
            .iter()
            .find(|claim| claim.intent.operation_id == operation_id)
            .map(|claim| claim.intent.principal)
            .or_else(|| {
                self.results.iter().find(|retained| retained.operation_id == operation_id).map(|r| r.principal)
            });
        match owner {
            Some(owner) if owner != principal => OperationReport::NotAuthorized,
            _ => self.report(operation_id),
        }
    }

    // -- helpers -----------------------------------------------------------------------------------

    fn active_claim(&self) -> Option<&Claim> {
        self.claims.last()
    }

    fn active_claim_mut(&mut self) -> Option<&mut Claim> {
        self.claims.last_mut()
    }

    fn active_upload_mut(&mut self) -> Option<&mut Claim> {
        self.claims.iter_mut().rev().find(|claim| claim.intent.opcode == Opcode::StartUpload)
    }

    fn generation_bytes(&self, generation: GenerationId) -> Option<&[u8]> {
        self.generations.iter().find(|entry| entry.id == generation).map(|entry| entry.bytes.as_slice())
    }

    fn commit_head(&mut self, kind: ObjectKind, logical_object_id: LogicalObjectId, bytes: &[u8]) -> Revision {
        let generation = GenerationId::new(self.next_generation);
        self.next_generation += 1;
        self.generations.push(Generation { id: generation, bytes: bytes.to_vec() });
        let revision = self.bump_repository();
        let head = Head {
            kind,
            logical_object_id,
            revision,
            generation,
            length: bytes.len() as u64,
            crc32: obc_crc::crc32(bytes),
        };
        match self.heads.iter_mut().find(|head| head.kind == kind && head.logical_object_id == logical_object_id) {
            Some(existing) => *existing = head,
            None => self.heads.push(head),
        }
        revision
    }

    fn bump_head_revision(&mut self, kind: ObjectKind, logical_object_id: LogicalObjectId) -> Revision {
        let revision = self.bump_repository();
        if let Some(head) =
            self.heads.iter_mut().find(|head| head.kind == kind && head.logical_object_id == logical_object_id)
        {
            head.revision = revision;
        }
        revision
    }

    fn remove_head(&mut self, kind: ObjectKind, logical_object_id: LogicalObjectId) {
        self.heads.retain(|head| !(head.kind == kind && head.logical_object_id == logical_object_id));
    }

    fn bump_repository(&mut self) -> Revision {
        self.repository_revision += 1;
        Revision::new(self.repository_revision)
    }

    fn object_result(
        &self,
        claim: &Claim,
        outcome: ObjectOutcome,
        revision: Revision,
        length: u64,
        crc32: u32,
    ) -> ResultEnvelope {
        ResultEnvelope::Object(ObjectResult {
            operation_id: claim.intent.operation_id,
            store_id: self.store_id,
            kind: claim.intent.kind,
            outcome,
            logical_object_id: claim.logical_object_id,
            revision,
            length,
            crc32,
        })
    }

    fn finish_claim(&mut self, operation_id: OperationId, outcome: Result<ResultEnvelope, TerminalError>) {
        let claim = self
            .claims
            .iter()
            .position(|claim| claim.intent.operation_id == operation_id)
            .map(|index| self.claims.remove(index));
        let (principal, digest) = claim
            .map(|claim| (claim.intent.principal, claim.intent.digest))
            .unwrap_or((PrincipalScope::new([0; 16]), [0; 32]));
        self.retain(Retained { operation_id, principal, digest, outcome });
    }

    fn retain(&mut self, retained: Retained) {
        // The same ring `obc-storage`'s model keeps: a full ring overwrites its start and advances
        // it by one, which is the only place a result is forgotten.
        if self.results.len() == RESULT_RING {
            self.results.remove(self.result_start.min(self.results.len() - 1));
            self.result_start = 0;
        }
        self.results.push(retained);
    }
}

/// A payload of `len` deterministic bytes, for tests that need a real object.
pub fn payload(len: usize) -> Vec<u8> {
    (0..len).map(|index| (index % 251) as u8).collect::<vec::Vec<u8>>()
}
