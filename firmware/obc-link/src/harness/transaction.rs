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
use crate::registry::{AbortReason, ObjectKind, ObjectOutcome, Phase, SubjectNamespace};
use crate::result::{AbortDisposition, AbortResult, ObjectResult, ResultEnvelope};
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
    /// What an AbortOperation command found when it reached its target (§6.4).
    disposition: Option<AbortDisposition>,
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

    /// Claims an InstallUpdate directly, so a test can name a target §9 makes non-cancellable.
    pub fn claim_install_update(&mut self, operation_id: OperationId, principal: PrincipalScope) {
        self.claims.push(Claim {
            intent: ClaimIntent {
                operation_id,
                principal,
                opcode: Opcode::InstallUpdate,
                digest: [0x11; 32],
                kind: ObjectKind::UpdatePackage,
                target: Target::Create,
                declared_length: 0,
                expected_crc: 0,
                target_operation_id: None,
            },
            logical_object_id: LogicalObjectId::ZERO,
            buffer: Vec::new(),
            durable_offset: 0,
            checkpoint_sequence: 0,
            sealed: false,
            phase: Phase::ExternalHandoff,
            disposition: None,
        });
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
            Command::ReleaseLease => {
                self.lease = None;
                Outcome::LeaseReleased
            }
            Command::DeviceControl(request) => Outcome::DeviceControl(self.device_control(request, scratch)),
            Command::QueryOperation { operation_id, principal } => {
                Outcome::OperationReport(self.report_for(operation_id, principal))
            }
        }
    }

    // -- the claim lock ---------------------------------------------------------------------------

    /// §11's lookup: it answers every action but the first and **creates no state**.
    fn lookup(&mut self, intent: ClaimIntent) -> ClaimOutcome {
        if let Some(claim) = self.claims.iter().find(|claim| claim.intent.operation_id == intent.operation_id) {
            if claim.intent.principal != intent.principal {
                return ClaimOutcome::ForeignPrincipal;
            }
            if claim.intent.digest != intent.digest {
                return ClaimOutcome::Conflict;
            }
            // Live work of the same intent. §6.1's restart-only row: the work is discarded and
            // restarted, and the durable restart record is synchronized before the acceptance.
            let logical_object_id = claim.logical_object_id;
            self.restart_work(intent.operation_id);
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
        ClaimOutcome::Unclaimed
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
        if let Some(cause) = self.faults.refuse_claim.take() {
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
            disposition: None,
        });
        ClaimOutcome::Claimed { logical_object_id, repository_revision: Revision::new(self.repository_revision) }
    }

    /// Whether an AbortOperation may name this target at all (§3's ownership, §9's non-cancellable
    /// InstallUpdate). `None` means it may.
    fn target_admissibility(&self, target: OperationId, principal: PrincipalScope) -> Option<FailureCause> {
        let claim = self.claims.iter().find(|claim| claim.intent.operation_id == target);
        let retained = self.results.iter().find(|retained| retained.operation_id == target);
        let owner = claim.map(|claim| claim.intent.principal).or(retained.map(|retained| retained.principal));
        if let Some(owner) = owner {
            if owner != principal {
                // §6.4 "requires the target's owning principal", and §3 puts authorization ahead of
                // every existence fact.
                return Some(FailureCause::Authorization { detail: detail::authorization::OPERATION_OWNER });
            }
        }
        if claim.is_some_and(|claim| claim.intent.opcode == Opcode::InstallUpdate) {
            // §9: "An AbortOperation naming an InstallUpdate target is refused with
            // unsupportedCapability/nonCancellableOperation ... it creates no state and burns no
            // identifier."
            return Some(FailureCause::UnsupportedCapability { detail: detail::capability::NON_CANCELLABLE_OPERATION });
        }
        None
    }

    /// Discards a live work record and re-synchronizes it at offset zero (§6.1's restart row).
    fn restart_work(&mut self, operation_id: OperationId) {
        if let Some(claim) = self.claims.iter_mut().find(|claim| claim.intent.operation_id == operation_id) {
            claim.buffer.clear();
            claim.durable_offset = 0;
            claim.checkpoint_sequence = 0;
            claim.sealed = false;
            claim.phase = Phase::Prepared;
        }
    }

    // -- the work record --------------------------------------------------------------------------

    fn append<'s>(&mut self, operation_id: OperationId, offset: u64, bytes: &[u8]) -> Outcome<'s> {
        if let Some(at) = self.faults.fail_append_at {
            if offset + bytes.len() as u64 > at {
                return Outcome::Failed(FailureCause::MediaIo { detail: detail::media_io::WRITE });
            }
        }
        let Some(claim) = self.claim_mut(operation_id) else {
            return Outcome::Failed(FailureCause::Internal { detail: detail::internal::INVARIANT });
        };
        if offset != claim.buffer.len() as u64 {
            return Outcome::Failed(FailureCause::Internal { detail: detail::internal::INVARIANT });
        }
        claim.buffer.extend_from_slice(bytes);
        claim.phase = Phase::Streaming;
        Outcome::Appended
    }

    fn checkpoint<'s>(&mut self, operation_id: OperationId, offset: u64) -> Outcome<'s> {
        let Some(claim) = self.claim_mut(operation_id) else {
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

    fn seal<'s>(&mut self, operation_id: OperationId, declared_length: u64, expected_crc: u32) -> Outcome<'s> {
        if self.faults.fail_seal {
            return Outcome::Failed(FailureCause::Checksum { detail: detail::checksum::WHOLE_PAYLOAD });
        }
        let Some(claim) = self.claim_mut(operation_id) else {
            return Outcome::Failed(FailureCause::Internal { detail: detail::internal::INVARIANT });
        };
        if claim.buffer.len() as u64 != declared_length || obc_crc::crc32(&claim.buffer) != expected_crc {
            return Outcome::Failed(FailureCause::Checksum { detail: detail::checksum::WHOLE_PAYLOAD });
        }
        claim.sealed = true;
        claim.phase = Phase::Sealed;
        Outcome::Sealed
    }

    fn validate<'s>(&mut self, operation_id: OperationId) -> Outcome<'s> {
        let Some(kind) = self.claim_for(operation_id).map(|claim| claim.intent.kind) else {
            return Outcome::Failed(FailureCause::Internal { detail: detail::internal::INVARIANT });
        };
        if let Some(detail) = self.faults.fail_validation {
            return Outcome::Failed(FailureCause::SemanticValidation { kind, detail });
        }
        if let Some(claim) = self.claim_mut(operation_id) {
            claim.phase = Phase::Validating;
        }
        Outcome::Validated
    }

    /// §6.4's second durable step: the target is marked terminal Aborted before the abort command's
    /// own result is committed, and the disposition it produced is kept for that result.
    fn cancel_target<'s>(
        &mut self,
        operation_id: OperationId,
        target: OperationId,
        reason: AbortReason,
    ) -> Outcome<'s> {
        let disposition = if self.claims.iter().any(|claim| claim.intent.operation_id == target) {
            let terminal = AbortCause::Cancelled { reason }.terminal();
            self.finish_claim(target, Err(terminal));
            AbortDisposition::Cancelled
        } else if self.results.iter().any(|retained| retained.operation_id == target) {
            // "If the target was already terminal, it is unchanged and the abort result says
            // `already terminal`."
            AbortDisposition::AlreadyTerminal
        } else {
            AbortDisposition::AlreadyAbsent
        };
        if let Some(claim) = self.claim_mut(operation_id) {
            claim.disposition = Some(disposition);
            claim.phase = Phase::Aborting;
        }
        Outcome::TargetCancelled(disposition)
    }

    fn publish<'s>(&mut self, operation_id: OperationId) -> Outcome<'s> {
        if self.faults.fail_publication {
            return Outcome::Failed(FailureCause::MediaIo { detail: detail::media_io::WRITE });
        }
        let Some(claim) = self.claim_for(operation_id).cloned() else {
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
                // §10: the abort command's typed success is an AbortResult, and §6.4 gives that
                // result to the command rather than to the target it cancelled.
                ResultEnvelope::Abort(AbortResult {
                    operation_id: claim.intent.operation_id,
                    store_id: self.store_id,
                    target_operation_id: claim.intent.target_operation_id.unwrap_or(OperationId::ZERO),
                    disposition: claim.disposition.unwrap_or(AbortDisposition::AlreadyAbsent),
                })
            }
            _ => return Outcome::Failed(FailureCause::Internal { detail: detail::internal::INVARIANT }),
        };
        self.finish_claim(claim.intent.operation_id, Ok(envelope));
        Outcome::Published(envelope)
    }

    fn abort<'s>(&mut self, operation_id: OperationId, cause: AbortCause) -> Outcome<'s> {
        let terminal = cause.terminal();
        if self.claim_for(operation_id).is_some() {
            self.finish_claim(operation_id, Err(terminal));
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
        // Every bound is checked rather than assumed: an offset past the end, a length past the
        // end, and a length past the caller's scratch all clamp instead of panicking.
        let start = (offset as usize).min(bytes.len());
        let end = start.saturating_add(usize::from(length)).min(bytes.len());
        let taken = end.saturating_sub(start).min(scratch.len());
        scratch[..taken].copy_from_slice(&bytes[start..start + taken]);
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
        if let Some(claim) = self.claim_for(operation_id) {
            // §8.1's matrix fixes every field from the originating claim, and a row outside it
            // "is an internal state/codec error and MUST NOT be emitted".
            if claim.intent.opcode == Opcode::AbortOperation {
                return OperationReport::InProgress(OperationProgress {
                    namespace: SubjectNamespace::None,
                    subject_kind: 0,
                    phase: Phase::Aborting,
                    flags: 0,
                    logical_object_id: LogicalObjectId::ZERO,
                    durable_offset: 0,
                });
            }
            let is_upload = claim.intent.opcode == Opcode::StartUpload;
            let mut flags = progress_flags::LOGICAL_ID_PRESENT;
            if is_upload && !matches!(claim.phase, Phase::Aborting) {
                // "attached only while that session exists; ... aborting has no attachment".
                flags |= progress_flags::SESSION_ATTACHED;
            }
            let durable_offset = match (is_upload, claim.phase) {
                // "offset is durable payload prefix, declared length in phases 2..4".
                (true, Phase::Sealed | Phase::Validating | Phase::Publishing) => claim.intent.declared_length,
                (true, _) => claim.durable_offset,
                (false, _) => 0,
            };
            return OperationReport::InProgress(OperationProgress {
                namespace: SubjectNamespace::Logical,
                subject_kind: claim.intent.kind.to_u16(),
                phase: claim.phase,
                flags,
                logical_object_id: claim.logical_object_id,
                durable_offset,
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

    fn claim_for(&self, operation_id: OperationId) -> Option<&Claim> {
        self.claims.iter().find(|claim| claim.intent.operation_id == operation_id)
    }

    fn claim_mut(&mut self, operation_id: OperationId) -> Option<&mut Claim> {
        self.claims.iter_mut().find(|claim| claim.intent.operation_id == operation_id)
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
        // The same ring `obc-storage`'s model keeps: "Ring append writes `(result_start +
        // result_count) mod 64`; when already full it overwrites `result_start` and advances that
        // index by one. This is the only place a result is forgotten."
        if self.results.len() < RESULT_RING {
            let index = (self.result_start + self.results.len()) % RESULT_RING;
            self.results.insert(index, retained);
            return;
        }
        self.results[self.result_start] = retained;
        self.result_start = (self.result_start + 1) % RESULT_RING;
    }
}

/// A payload of `len` deterministic bytes, for tests that need a real object.
pub fn payload(len: usize) -> Vec<u8> {
    (0..len).map(|index| (index % 251) as u8).collect::<vec::Vec<u8>>()
}
