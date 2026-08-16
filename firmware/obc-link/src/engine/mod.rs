//! The transfer engine: one session model and one set of state machines for both links.
//!
//! The engine is the device side of `Device_Object_Protocol_v3.md` above the codec and below the
//! board. It owns the connection state machine of §5.2, the SessionId coordinator of §3, and the
//! upload/download/command machines of §15. It owns **no** transport and does **no** storage I/O:
//! a record comes in, and what comes back is either bytes to send or a typed [`Command`] for the
//! board glue to execute against the DOS2 transaction seam.
//!
//! ## The driver loop
//!
//! Every entry point returns one [`Reaction`]. A `Work` reaction means "execute this command and
//! call [`Engine::resume`] with its [`Outcome`]", which may in turn produce another command — the
//! seal/validate/publish chain is exactly that. A driver is therefore ten lines:
//!
//! ```text
//! let mut reaction = engine.on_control(context, record, &mut out);
//! loop {
//!     match reaction {
//!         Reaction::Work(command) => reaction = engine.resume(transaction.execute(command), &mut out),
//!         Reaction::Emit { channel, len } => { link.send(channel, &out[..len]); break }
//!         Reaction::Close(channel) => { link.close(channel); break }
//!         Reaction::Idle => break,
//!     }
//! }
//! ```
//!
//! ## Restart-only
//!
//! This engine implements the **restart-only upload profile** §6.1 defines and the owner froze for
//! the first device: no kind advertises resumable upload, no durable next offset above zero is ever
//! reported, and work that cannot be resumed is never left occupying a slot — a stream fault,
//! AbortSession, or link teardown durably aborts it. The wire contract already covers both
//! profiles, so enabling resume later is an advertising and implementation change here, not a
//! change to anything this module encodes.
//!
//! ## What is deliberately not here yet
//!
//! Drafts (`BeginDraft`, `StartDraftPart`, `FinalizeDraft`, `QueryDraft`), catalog paging, and the
//! weather request context are later slices of the DOS3 issue. The engine refuses them with
//! `unsupportedCapability/opcode`, which is the same answer a conforming device gives for a
//! command-flag bit it does not set, and the profiles this crate ships clear those bits.

mod connection;
mod effect;
mod link;
mod phase;
mod profile;
mod session;

pub use connection::{Connection, ConnectionRefusal, LinkCeilings, Negotiated};
pub use effect::{
    AbortCause, ClaimIntent, ClaimOutcome, ClaimStatus, Command, DeviceControlAnswer, DeviceControlRequest,
    FailureCause, OperationReport, Outcome, PinnedSource, TerminalError,
};
pub use link::{ByteLink, LinkChannel, LinkError};
pub use phase::{
    CommandEvent, CommandPhase, DownloadEvent, DownloadPhase, IllegalTransition, UploadEvent, UploadPhase,
};
pub use profile::{DeviceProfile, SubjectTable};
pub use session::{LinkContext, PrincipalScope, SessionCoordinator, SessionRejection, StreamAdmission};

use crate::download::{DownloadAccepted, StartDownload};
use crate::error::{detail, ErrorBody, ErrorCategory, Owner, RetryGuidance};
use crate::frame::{ControlFrame, Opcode, HEADER_LEN, MIN_CONTROL_FRAME};
use crate::hello::LinkKind;
use crate::ids::{LogicalObjectId, OperationId, RequestId, SessionId};
use crate::query::OperationStatus;
use crate::registry::{subject_flags, ObjectKind};
use crate::stream::{Direction, FaultBody, FaultDisposition, StreamFrame, STREAM_HEADER_LEN};
use crate::upload::{
    AbortSessionOutcome, AcceptanceFlags, CheckpointAccepted, Disposition, StartUpload, Target, UploadAcceptance,
};
use crate::{DecodeError, Request, Response};

/// How many link kinds a device serves at once: BLE, USB, and the test link (§5).
const LINK_KINDS: usize = 3;

/// The opcodes a later DOS3 slice adds. They are refused exactly as an unadvertised opcode is.
const LATER_SLICES: [Opcode; 6] = [
    Opcode::BeginDraft,
    Opcode::StartDraftPart,
    Opcode::FinalizeDraft,
    Opcode::QueryDraft,
    Opcode::QueryCatalog,
    Opcode::QueryWeatherRequest,
];

/// What the engine wants done after one record or one outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reaction<'a> {
    /// Nothing to do. A silently discarded tombstone frame lands here, as §13 requires.
    Idle,
    /// Send `out[..len]` on this channel.
    Emit {
        /// Which record channel the bytes belong to.
        channel: LinkChannel,
        /// How many bytes of the caller's buffer to send.
        len: usize,
    },
    /// Execute this command, then call [`Engine::resume`] with its outcome.
    Work(Command<'a>),
    /// Close this record channel: untrusted framing, or a zero RequestId (§2, §13).
    Close(LinkChannel),
}

/// The live upload, if one owns the coordinator.
#[derive(Debug, Clone, Copy)]
struct Upload {
    operation_id: OperationId,
    kind: ObjectKind,
    session_id: SessionId,
    owner: LinkContext,
    phase: UploadPhase,
    declared_length: u64,
    expected_crc: u32,
    next_offset: u64,
    logical_object_id: LogicalObjectId,
}

/// The live download, if one owns the coordinator.
#[derive(Debug, Clone, Copy)]
struct Download {
    session_id: SessionId,
    owner: LinkContext,
    phase: DownloadPhase,
    source: PinnedSource,
    next_offset: u64,
    max_payload: u16,
}

/// The one heavy transfer §6 allows.
#[derive(Debug, Clone, Copy)]
enum Transfer {
    Upload(Upload),
    Download(Download),
}

impl Transfer {
    fn owner(&self) -> LinkContext {
        match self {
            Transfer::Upload(upload) => upload.owner,
            Transfer::Download(download) => download.owner,
        }
    }

    fn session_id(&self) -> SessionId {
        match self {
            Transfer::Upload(upload) => upload.session_id,
            Transfer::Download(download) => download.session_id,
        }
    }
}

/// What a StartUpload is waiting for its claim lock to say.
#[derive(Debug, Clone, Copy)]
struct UploadClaim {
    operation_id: OperationId,
    kind: ObjectKind,
    target: Target,
    declared_length: u64,
    expected_crc: u32,
}

/// What to answer once an abort is durable.
#[derive(Debug, Clone, Copy)]
enum AbortReply {
    /// Answer the pending request with this live body, its claim bits set to terminal.
    Failure(FailureCause),
    /// Answer AbortSession's one-byte outcome.
    SessionDetached,
    /// Emit the terminal stream fault §13 sends before releasing the session.
    StreamFault { session_id: SessionId, category: ErrorCategory, detail: u16, expected_offset: u64 },
    /// Nothing to answer: the link is already gone.
    Silent,
}

/// What the engine is waiting for.
#[derive(Debug, Clone, Copy)]
enum Stage {
    /// Payload bytes are being written. Nothing answers this: the frame is the whole exchange.
    Append,
    UploadClaim(UploadClaim),
    MutationClaim,
    /// A direct mutation, at the phase of §15's command machine it is currently running.
    Mutation(CommandPhase),
    Checkpoint,
    Seal,
    Validate,
    Publish,
    Abort(AbortReply),
    Resolve(StartDownload),
    ReadSource,
    ReleaseLease {
        detach: bool,
    },
    DeviceControl,
    Query,
}

/// The one outstanding piece of work, and how to answer it.
#[derive(Debug, Clone, Copy)]
struct Pending {
    link: LinkKind,
    request_id: Option<RequestId>,
    opcode: Opcode,
    stage: Stage,
}

/// What the live upload looks like from outside the engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UploadSnapshot {
    /// The claim this transfer belongs to.
    pub operation_id: OperationId,
    /// The kind being written.
    pub kind: ObjectKind,
    /// The live stream capability.
    pub session_id: SessionId,
    /// Where the upload machine of §15 stands.
    pub phase: UploadPhase,
    /// The identity the repository assigned or confirmed at admission.
    pub logical_object_id: LogicalObjectId,
    /// The length StartUpload declared.
    pub declared_length: u64,
    /// The offset the next payload frame must carry.
    pub next_offset: u64,
}

/// What the live download looks like from outside the engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DownloadSnapshot {
    /// The live stream capability.
    pub session_id: SessionId,
    /// Where the download machine of §7 stands.
    pub phase: DownloadPhase,
    /// The pinned head.
    pub source: PinnedSource,
    /// The offset the next frame will carry.
    pub next_offset: u64,
    /// The largest payload a frame of this session carries.
    pub max_payload: u16,
}

/// The device-side transfer engine.
#[derive(Debug)]
pub struct Engine {
    profile: DeviceProfile,
    connections: [Connection; LINK_KINDS],
    coordinator: SessionCoordinator,
    transfer: Option<Transfer>,
    pending: Option<Pending>,
}

impl Engine {
    /// A new engine serving `profile`, with no connection open.
    pub fn new(profile: DeviceProfile) -> Self {
        Engine {
            profile,
            connections: [Connection::closed(); LINK_KINDS],
            coordinator: SessionCoordinator::new(),
            transfer: None,
            pending: None,
        }
    }

    /// The compiled facts this engine advertises.
    pub fn profile(&self) -> &DeviceProfile {
        &self.profile
    }

    /// The compiled facts, mutably — a board changes them only between connections (§5).
    pub fn profile_mut(&mut self) -> &mut DeviceProfile {
        &mut self.profile
    }

    /// The live SessionId, if a heavy transfer owns the coordinator.
    pub fn live_session(&self) -> Option<SessionId> {
        self.coordinator.live()
    }

    /// A snapshot of the live upload, for progress publication and §8.1's InProgress projection.
    pub fn active_upload(&self) -> Option<UploadSnapshot> {
        match self.transfer {
            Some(Transfer::Upload(upload)) => Some(UploadSnapshot {
                operation_id: upload.operation_id,
                kind: upload.kind,
                session_id: upload.session_id,
                phase: upload.phase,
                logical_object_id: upload.logical_object_id,
                declared_length: upload.declared_length,
                next_offset: upload.next_offset,
            }),
            _ => None,
        }
    }

    /// A snapshot of the live download. It is not a claimed operation and has no phase byte (§7).
    pub fn active_download(&self) -> Option<DownloadSnapshot> {
        match self.transfer {
            Some(Transfer::Download(download)) => Some(DownloadSnapshot {
                session_id: download.session_id,
                phase: download.phase,
                source: download.source,
                next_offset: download.next_offset,
                max_payload: download.max_payload,
            }),
            _ => None,
        }
    }

    /// True when a heavy transfer owns the coordinator (§5's status bit 2).
    pub fn is_busy(&self) -> bool {
        self.transfer.is_some()
    }

    /// Opens a connection generation on one link. Everything the old one negotiated is discarded.
    pub fn open_connection(&mut self, context: LinkContext, ceilings: LinkCeilings) {
        self.connection_mut(context.link_kind).open(context, ceilings);
    }

    /// The connection state of one link, for tests and diagnostics.
    pub fn connection(&self, link_kind: LinkKind) -> &Connection {
        &self.connections[Self::index(link_kind)]
    }

    /// Reports link teardown exactly once, with the exact context §13 requires.
    ///
    /// A stale teardown — one naming a connection that is not the current one — is a no-op, and a
    /// teardown that finds no work of its own returns [`Reaction::Idle`].
    pub fn close_connection(&mut self, context: LinkContext) -> Reaction<'static> {
        let open = self.connection(context.link_kind).context();
        if open != Some(context) {
            return Reaction::Idle;
        }
        self.connection_mut(context.link_kind).close();
        let Some(transfer) = self.transfer else { return Reaction::Idle };
        if !transfer.owner().is_same_connection(&context) {
            return Reaction::Idle;
        }
        self.coordinator.on_link_lost(&context);
        match transfer {
            Transfer::Upload(upload) => {
                // §13: teardown "durably aborts active restart-only upload work to a terminal
                // Aborted state with a retained text-free ErrorBody".
                self.transfer = Some(Transfer::Upload(Upload { phase: UploadPhase::Aborting, ..upload }));
                self.pending = Some(Pending {
                    link: context.link_kind,
                    request_id: None,
                    opcode: Opcode::StartUpload,
                    stage: Stage::Abort(AbortReply::Silent),
                });
                Reaction::Work(Command::Abort(AbortCause::LinkLost))
            }
            Transfer::Download(_) => {
                // "releases a matching download lease exactly once".
                self.pending = Some(Pending {
                    link: context.link_kind,
                    request_id: None,
                    opcode: Opcode::StartDownload,
                    stage: Stage::ReleaseLease { detach: false },
                });
                Reaction::Work(Command::ReleaseLease)
            }
        }
    }

    /// Handles one control record.
    pub fn on_control<'a>(&mut self, context: LinkContext, record: &'a [u8], out: &mut [u8]) -> Reaction<'a> {
        if self.connection(context.link_kind).context() != Some(context) {
            // A record from a connection this engine was never told about, or from an older
            // generation the adapter has already replaced.
            return Reaction::Idle;
        }
        let link = context.link_kind;
        let bound = self
            .connection(link)
            .negotiated()
            .map_or(MIN_CONTROL_FRAME, |negotiated| usize::from(negotiated.control_frame));
        let frame = match ControlFrame::decode_bounded(record, bound) {
            Ok(frame) => frame,
            Err(error) => return self.refuse_unframed(link, record, error, out),
        };
        if let Err(refusal) = self.connection_mut(link).admit(frame.opcode, frame.request_id) {
            // A refusal here never disturbs the request in flight, and never clears its slot.
            return Self::encode_response(&Response::Error(refusal.body(link)), frame.opcode, frame.request_id, out);
        }
        if !self.profile.serves(frame.opcode) || LATER_SLICES.contains(&frame.opcode) {
            return self.reply_error(link, frame.opcode, frame.request_id, unsupported_opcode(), out);
        }
        let request = match Request::decode(&frame) {
            Ok(request) => request,
            Err(error) => return self.reply_error(link, frame.opcode, frame.request_id, body_of(error), out),
        };
        self.dispatch(context, frame.opcode, frame.request_id, request, out)
    }

    /// Handles one stream record.
    pub fn on_stream<'a>(&mut self, context: LinkContext, record: &'a [u8], out: &mut [u8]) -> Reaction<'a> {
        if self.connection(context.link_kind).context() != Some(context) {
            return Reaction::Idle;
        }
        let effective = self
            .connection(context.link_kind)
            .negotiated()
            .map_or(crate::frame::MIN_STREAM_FRAME, |negotiated| usize::from(negotiated.stream_frame));
        let frame = match StreamFrame::decode_bounded(record, effective) {
            // §13: "A structurally unframeable record ... closes the stream transport."
            Err(_) => return Reaction::Close(LinkChannel::Stream),
            Ok(frame) => frame,
        };
        let session_id = frame.session_id();
        match self.coordinator.admit_stream(session_id, &context) {
            StreamAdmission::Tombstoned => return Reaction::Idle,
            StreamAdmission::Untrusted => return Reaction::Close(LinkChannel::Stream),
            StreamAdmission::Owned => {}
        }
        let StreamFrame::Data { direction, offset, payload, .. } = frame else {
            // A device receives no fault frames: a client reports failure by aborting.
            return Reaction::Close(LinkChannel::Stream);
        };
        let Some(Transfer::Upload(upload)) = self.transfer else {
            return self.fault(session_id, ErrorCategory::INVALID_SESSION, detail::session::WRONG_DIRECTION, 0);
        };
        if direction != Direction::Upload || !upload.phase.accepts_bytes() {
            return self.fault(
                session_id,
                ErrorCategory::INVALID_SESSION,
                detail::session::WRONG_DIRECTION,
                upload.next_offset,
            );
        }
        if offset != upload.next_offset {
            // §13: an owned, parseable frame at the wrong offset earns a fault status before the
            // session is released.
            return self.fault(
                session_id,
                ErrorCategory::INVALID_OFFSET,
                detail::offset::UNEXPECTED_OFFSET,
                upload.next_offset,
            );
        }
        if offset.saturating_add(payload.len() as u64) > upload.declared_length {
            return self.fault(
                session_id,
                ErrorCategory::INVALID_OFFSET,
                detail::offset::UNEXPECTED_OFFSET,
                upload.next_offset,
            );
        }
        let _ = out;
        // A payload frame answers no request, so nothing is pending: the append is the whole
        // reaction and the next frame is admitted against the offset it advances.
        self.append(payload, offset)
    }

    /// Asks the engine for the next download frame, when one is due.
    ///
    /// The download pump is a poll rather than a reaction to a record, because §7's stream has no
    /// client frame to react to: the device sends until the declared length is reached and the
    /// client finishes on the control link.
    pub fn poll_download(&mut self) -> Reaction<'static> {
        let Some(Transfer::Download(download)) = self.transfer else { return Reaction::Idle };
        if self.pending.is_some() || !download.phase.is_streamable() {
            return Reaction::Idle;
        }
        let remaining = download.source.total_length.saturating_sub(download.next_offset);
        if remaining == 0 {
            return Reaction::Idle;
        }
        let length = remaining.min(u64::from(download.max_payload)) as u16;
        self.pending = Some(Pending {
            link: download.owner.link_kind,
            request_id: None,
            opcode: Opcode::StartDownload,
            stage: Stage::ReadSource,
        });
        Reaction::Work(Command::ReadSource { offset: download.next_offset, length })
    }

    /// Takes back the outcome of the command the engine last asked for.
    ///
    /// Nothing an outcome carries is ever handed back out: bytes it brings — echo payloads, source
    /// bytes — are encoded into `out`, which is why the reaction it produces borrows nothing.
    pub fn resume(&mut self, outcome: Outcome<'_>, out: &mut [u8]) -> Reaction<'static> {
        let Some(pending) = self.pending else {
            debug_assert!(false, "an outcome arrived with no command outstanding");
            return Reaction::Idle;
        };
        match (pending.stage, outcome) {
            (Stage::Append, Outcome::Appended) => {
                self.pending = None;
                Reaction::Idle
            }
            (Stage::Append, Outcome::Failed(cause)) => {
                // A write that fails mid-stream is a transport fault: the client is told on the
                // stream channel and, restart-only, the work is durably abandoned with it (§13).
                let session_id = self.transfer.map(|transfer| transfer.session_id());
                let expected_offset = match self.transfer {
                    Some(Transfer::Upload(upload)) => upload.next_offset,
                    _ => 0,
                };
                match session_id {
                    Some(session_id) => {
                        self.step_upload(UploadEvent::Abandon);
                        self.coordinator.revoke();
                        let (category, detail) = fault_pair(cause);
                        self.pending = Some(Pending {
                            stage: Stage::Abort(AbortReply::StreamFault {
                                session_id,
                                category,
                                detail,
                                expected_offset,
                            }),
                            ..pending
                        });
                        Reaction::Work(Command::Abort(AbortCause::Failed(cause)))
                    }
                    None => {
                        self.pending = None;
                        Reaction::Idle
                    }
                }
            }
            (Stage::UploadClaim(claim), Outcome::Claim(decision)) => {
                self.after_upload_claim(pending, claim, decision, out)
            }
            (Stage::MutationClaim, Outcome::Claim(decision)) => self.after_mutation_claim(pending, decision, out),
            (Stage::Mutation(CommandPhase::Validating), Outcome::Validated) => {
                match CommandPhase::Validating.apply(CommandEvent::Validated) {
                    Ok(phase) => self.advance(pending, Stage::Mutation(phase), Command::Publish),
                    Err(_) => self.reply(pending, &Response::Error(internal_body()), out),
                }
            }
            (Stage::Mutation(CommandPhase::Publishing), Outcome::Published(envelope)) => {
                match CommandPhase::Publishing.apply(CommandEvent::Published) {
                    Ok(CommandPhase::Terminal) => self.reply(pending, &Response::MutationResult(envelope), out),
                    _ => self.reply(pending, &Response::Error(internal_body()), out),
                }
            }
            (Stage::Checkpoint, Outcome::Checkpointed { durable_offset, prefix_crc, sequence }) => {
                let session_id = self.transfer.map(|transfer| transfer.session_id());
                match session_id {
                    Some(session_id) => {
                        let response = Response::CheckpointAccepted(CheckpointAccepted {
                            session_id,
                            durable_next_offset: durable_offset,
                            finalized_prefix_crc32: prefix_crc,
                            checkpoint_sequence: sequence,
                        });
                        self.reply(pending, &response, out)
                    }
                    None => self.reply(pending, &Response::Error(internal_body()), out),
                }
            }
            (Stage::Seal, Outcome::Sealed) => {
                self.step_upload(UploadEvent::Sealed);
                self.advance(pending, Stage::Validate, Command::Validate)
            }
            (Stage::Validate, Outcome::Validated) => {
                self.step_upload(UploadEvent::Validated);
                self.advance(pending, Stage::Publish, Command::Publish)
            }
            (Stage::Publish, Outcome::Published(envelope)) => {
                self.step_upload(UploadEvent::Published);
                self.release_transfer();
                self.reply(pending, &Response::UploadResult(envelope), out)
            }
            (Stage::Abort(reply), Outcome::Aborted(terminal)) => self.after_abort(pending, reply, terminal, out),
            (Stage::Resolve(request), Outcome::Resolved(source)) => self.after_resolve(pending, request, source, out),
            (Stage::ReadSource, Outcome::SourceBytes { offset, bytes }) => {
                self.emit_download(pending, offset, bytes, out)
            }
            (Stage::ReleaseLease { detach }, Outcome::LeaseReleased) => {
                self.release_transfer();
                let response = if detach {
                    Response::SessionAborted(AbortSessionOutcome::Detached)
                } else {
                    Response::DownloadFinished
                };
                match pending.request_id {
                    Some(_) => self.reply(pending, &response, out),
                    None => {
                        self.pending = None;
                        Reaction::Idle
                    }
                }
            }
            (Stage::DeviceControl, Outcome::DeviceControl(answer)) => self.after_device_control(pending, answer, out),
            (Stage::Query, Outcome::OperationReport(report)) => self.after_query(pending, report, out),
            (Stage::UploadClaim(_) | Stage::MutationClaim, Outcome::Failed(cause)) => {
                // A preflight failure creates no state, so there is nothing to abort.
                self.reply(pending, &Response::Error(cause.body(ClaimStatus::None)), out)
            }
            (_, Outcome::Failed(cause)) => self.abandon(pending, cause),
            _ => {
                debug_assert!(false, "an outcome arrived that the pending stage cannot use");
                self.reply(pending, &Response::Error(internal_body()), out)
            }
        }
    }

    // -- dispatch ------------------------------------------------------------------------------

    fn dispatch<'a>(
        &mut self,
        context: LinkContext,
        opcode: Opcode,
        request_id: RequestId,
        request: Request<'a>,
        out: &mut [u8],
    ) -> Reaction<'a> {
        let link = context.link_kind;
        match request {
            Request::Hello(hello) => {
                let (device_control, device_stream) =
                    (self.profile.device_max_control_frame, self.profile.device_max_stream_frame);
                let negotiated = match self.connection_mut(link).negotiate(&hello, device_control, device_stream) {
                    Ok(negotiated) => negotiated,
                    Err(refusal) => return self.reply_error(link, opcode, request_id, refusal.body(link), out),
                };
                let authenticated = link != LinkKind::Test;
                match self.profile.capabilities(&hello, &negotiated, link, authenticated, self.is_busy()) {
                    Ok((capabilities, more)) => {
                        self.connection_mut(link).complete();
                        Self::encode_page(&Response::Capabilities(capabilities), opcode, request_id, more, out)
                    }
                    Err(body) => self.reply_error(link, opcode, request_id, body, out),
                }
            }
            Request::StartUpload(request) => self.start_upload(context, request_id, request, out),
            Request::CheckpointUpload(request) => self.checkpoint(context, request_id, request, out),
            Request::FinishUpload(request) => self.finish_upload(context, request_id, request.session_id, out),
            Request::AbortSession(request) => self.abort_session(context, request_id, request, out),
            Request::StartDownload(request) => self.start_download(context, request_id, request, out),
            Request::FinishDownload(request) => self.finish_download(context, request_id, request, out),
            Request::QueryOperation(request) => {
                self.pending = Some(Pending { link, request_id: Some(request_id), opcode, stage: Stage::Query });
                Reaction::Work(Command::QueryOperation(request.operation_id))
            }
            Request::DeleteObject(request) => {
                let digest = self.intent_digest(&Request::DeleteObject(request));
                let target = Target::Replace {
                    logical_object_id: request.target.logical_object_id,
                    expected_revision: request.target.expected_revision,
                };
                self.mutate(
                    context,
                    request_id,
                    opcode,
                    request.target.operation_id,
                    request.target.kind,
                    subject_flags::DELETE,
                    target,
                    digest,
                    out,
                )
            }
            Request::SetMetadata(request) => {
                let digest = self.intent_digest(&Request::SetMetadata(request));
                let target = Target::Replace {
                    logical_object_id: request.target.logical_object_id,
                    expected_revision: request.target.expected_revision,
                };
                self.mutate(
                    context,
                    request_id,
                    opcode,
                    request.target.operation_id,
                    request.target.kind,
                    subject_flags::SET_METADATA,
                    target,
                    digest,
                    out,
                )
            }
            Request::AbortOperation(request) => {
                let digest = self.intent_digest(&Request::AbortOperation(request));
                self.claim(
                    context,
                    request_id,
                    opcode,
                    ClaimIntent {
                        operation_id: request.operation_id,
                        principal: context.principal,
                        opcode,
                        digest,
                        kind: ObjectKind::Route,
                        target: Target::Create,
                        declared_length: 0,
                        expected_crc: 0,
                    },
                    Stage::MutationClaim,
                )
            }
            Request::InstallUpdate(request) => {
                let digest = self.intent_digest(&Request::InstallUpdate(request));
                let target = Target::Replace {
                    logical_object_id: request.logical_object_id,
                    expected_revision: request.expected_revision,
                };
                self.mutate(
                    context,
                    request_id,
                    opcode,
                    request.operation_id,
                    ObjectKind::UpdatePackage,
                    subject_flags::GET,
                    target,
                    digest,
                    out,
                )
            }
            Request::AcknowledgeRideImported(request) => {
                let digest = self.intent_digest(&Request::AcknowledgeRideImported(request));
                let target = Target::Replace {
                    logical_object_id: request.logical_object_id,
                    expected_revision: request.expected_revision,
                };
                self.mutate(
                    context,
                    request_id,
                    opcode,
                    request.operation_id,
                    ObjectKind::Ride,
                    subject_flags::GET,
                    target,
                    digest,
                    out,
                )
            }
            Request::GetDeviceStatus => {
                self.device_control(link, request_id, opcode, DeviceControlRequest::GetDeviceStatus)
            }
            Request::GetConfig => self.device_control(link, request_id, opcode, DeviceControlRequest::GetConfig),
            Request::SetConfig(block) => {
                self.device_control(link, request_id, opcode, DeviceControlRequest::SetConfig(block))
            }
            Request::SetClock(request) => {
                self.device_control(link, request_id, opcode, DeviceControlRequest::SetClock(request))
            }
            Request::ForgetBond(request) => {
                self.device_control(link, request_id, opcode, DeviceControlRequest::ForgetBond(request))
            }
            Request::Echo(echo) => {
                self.device_control(link, request_id, opcode, DeviceControlRequest::Echo(echo.payload))
            }
            Request::ResetStore(request) => {
                self.device_control(link, request_id, opcode, DeviceControlRequest::ResetStore(request.echoed_store_id))
            }
            // Every remaining opcode belongs to a later slice and was refused above.
            _ => self.reply_error(link, opcode, request_id, unsupported_opcode(), out),
        }
    }

    fn start_upload<'a>(
        &mut self,
        context: LinkContext,
        request_id: RequestId,
        request: StartUpload<'_>,
        out: &mut [u8],
    ) -> Reaction<'a> {
        let link = context.link_kind;
        let entry = match self.profile.require_operation(request.kind, subject_flags::PUT) {
            Ok(entry) => entry,
            Err(body) => return self.reply_error(link, Opcode::StartUpload, request_id, body, out),
        };
        if request.declared_length > entry.max_length {
            let body = FailureCause::ResourceLimit { detail: detail::resource::OBJECT_LENGTH }.body(ClaimStatus::None);
            return self.reply_error(link, Opcode::StartUpload, request_id, body, out);
        }
        if let Some(transfer) = self.transfer {
            let same_work = matches!(transfer, Transfer::Upload(upload) if upload.operation_id == request.operation_id);
            if !same_work {
                let body = busy_body(detail::busy::HEAVY_TRANSFER, transfer.owner().link_kind);
                return self.reply_error(link, Opcode::StartUpload, request_id, body, out);
            }
        }
        let digest = crate::intent::CanonicalIntent::for_start_upload(self.profile.store_id, &request).digest();
        self.claim(
            context,
            request_id,
            Opcode::StartUpload,
            ClaimIntent {
                operation_id: request.operation_id,
                principal: context.principal,
                opcode: Opcode::StartUpload,
                digest,
                kind: request.kind,
                target: request.target,
                declared_length: request.declared_length,
                expected_crc: request.expected_crc32,
            },
            Stage::UploadClaim(UploadClaim {
                operation_id: request.operation_id,
                kind: request.kind,
                target: request.target,
                declared_length: request.declared_length,
                expected_crc: request.expected_crc32,
            }),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn mutate<'a>(
        &mut self,
        context: LinkContext,
        request_id: RequestId,
        opcode: Opcode,
        operation_id: OperationId,
        kind: ObjectKind,
        operation: u16,
        target: Target,
        digest: [u8; 32],
        out: &mut [u8],
    ) -> Reaction<'a> {
        if let Err(body) = self.profile.require_operation(kind, operation) {
            return self.reply_error(context.link_kind, opcode, request_id, body, out);
        }
        self.claim(
            context,
            request_id,
            opcode,
            ClaimIntent {
                operation_id,
                principal: context.principal,
                opcode,
                digest,
                kind,
                target,
                declared_length: 0,
                expected_crc: 0,
            },
            Stage::MutationClaim,
        )
    }

    fn claim<'a>(
        &mut self,
        context: LinkContext,
        request_id: RequestId,
        opcode: Opcode,
        intent: ClaimIntent,
        stage: Stage,
    ) -> Reaction<'a> {
        self.pending = Some(Pending { link: context.link_kind, request_id: Some(request_id), opcode, stage });
        Reaction::Work(Command::Claim(intent))
    }

    fn checkpoint<'a>(
        &mut self,
        context: LinkContext,
        request_id: RequestId,
        request: crate::upload::CheckpointUpload,
        out: &mut [u8],
    ) -> Reaction<'a> {
        let upload = match self.owned_upload(context, request.session_id) {
            Ok(upload) => upload,
            Err(body) => return self.reply_error(context.link_kind, Opcode::CheckpointUpload, request_id, body, out),
        };
        if request.received_next_offset != upload.next_offset {
            let mut body = ErrorBody::bare(
                ErrorCategory::INVALID_OFFSET,
                detail::offset::UNEXPECTED_OFFSET,
                RetryGuidance::RESUME_AT_EXPECTED_OFFSET,
            );
            body.presence = crate::error::presence::EXPECTED_OFFSET | ClaimStatus::Live.presence();
            body.expected_offset = upload.next_offset;
            return self.reply_error(context.link_kind, Opcode::CheckpointUpload, request_id, body, out);
        }
        if !request.is_on_boundary(self.profile.checkpoint_granule, upload.declared_length) {
            let mut body = ErrorBody::bare(
                ErrorCategory::INVALID_OFFSET,
                detail::offset::CHECKPOINT_BOUNDARY,
                RetryGuidance::RESUME_AT_EXPECTED_OFFSET,
            );
            body.presence = crate::error::presence::EXPECTED_OFFSET | ClaimStatus::Live.presence();
            body.expected_offset = upload.next_offset;
            return self.reply_error(context.link_kind, Opcode::CheckpointUpload, request_id, body, out);
        }
        self.pending = Some(Pending {
            link: context.link_kind,
            request_id: Some(request_id),
            opcode: Opcode::CheckpointUpload,
            stage: Stage::Checkpoint,
        });
        Reaction::Work(Command::Checkpoint { offset: request.received_next_offset })
    }

    fn finish_upload<'a>(
        &mut self,
        context: LinkContext,
        request_id: RequestId,
        session_id: SessionId,
        out: &mut [u8],
    ) -> Reaction<'a> {
        let upload = match self.owned_upload(context, session_id) {
            Ok(upload) => upload,
            Err(body) => return self.reply_error(context.link_kind, Opcode::FinishUpload, request_id, body, out),
        };
        self.step_upload(UploadEvent::Finish);
        self.pending = Some(Pending {
            link: context.link_kind,
            request_id: Some(request_id),
            opcode: Opcode::FinishUpload,
            stage: Stage::Seal,
        });
        Reaction::Work(Command::Seal { declared_length: upload.declared_length, expected_crc: upload.expected_crc })
    }

    fn abort_session<'a>(
        &mut self,
        context: LinkContext,
        request_id: RequestId,
        request: crate::upload::AbortSession,
        out: &mut [u8],
    ) -> Reaction<'a> {
        if let Err(rejection) = self.coordinator.check(request.session_id, &context) {
            return self.reply_error(
                context.link_kind,
                Opcode::AbortSession,
                request_id,
                invalid_session_body(rejection),
                out,
            );
        }
        let Some(transfer) = self.transfer else {
            return self.reply_error(
                context.link_kind,
                Opcode::AbortSession,
                request_id,
                invalid_session_body(SessionRejection::Unknown),
                out,
            );
        };
        self.coordinator.revoke_owned_by(&context);
        match transfer {
            Transfer::Upload(upload) => {
                // §6.4: "Detaching a restart-only upload durably aborts it."
                self.transfer = Some(Transfer::Upload(Upload { phase: UploadPhase::Aborting, ..upload }));
                self.pending = Some(Pending {
                    link: context.link_kind,
                    request_id: Some(request_id),
                    opcode: Opcode::AbortSession,
                    stage: Stage::Abort(AbortReply::SessionDetached),
                });
                Reaction::Work(Command::Abort(AbortCause::Cancelled { reason: request.reason }))
            }
            Transfer::Download(_) => {
                self.pending = Some(Pending {
                    link: context.link_kind,
                    request_id: Some(request_id),
                    opcode: Opcode::AbortSession,
                    stage: Stage::ReleaseLease { detach: true },
                });
                Reaction::Work(Command::ReleaseLease)
            }
        }
    }

    fn start_download<'a>(
        &mut self,
        context: LinkContext,
        request_id: RequestId,
        request: StartDownload,
        out: &mut [u8],
    ) -> Reaction<'a> {
        let link = context.link_kind;
        if let Err(body) = self.profile.require_operation(request.kind, subject_flags::GET) {
            return self.reply_error(link, Opcode::StartDownload, request_id, body, out);
        }
        if request.start_offset.is_some() && !self.profile.advertises_resumable_download(request.kind) {
            let body = ErrorBody::bare(
                ErrorCategory::UNSUPPORTED_CAPABILITY,
                detail::capability::FEATURE,
                RetryGuidance::REJECT_PERMANENTLY,
            );
            return self.reply_error(link, Opcode::StartDownload, request_id, body, out);
        }
        if let Some(transfer) = self.transfer {
            let body = busy_body(detail::busy::HEAVY_TRANSFER, transfer.owner().link_kind);
            return self.reply_error(link, Opcode::StartDownload, request_id, body, out);
        }
        self.pending = Some(Pending {
            link,
            request_id: Some(request_id),
            opcode: Opcode::StartDownload,
            stage: Stage::Resolve(request),
        });
        Reaction::Work(Command::Resolve {
            kind: request.kind,
            logical_object_id: request.logical_object_id,
            start_offset: request.start_offset.unwrap_or(0),
        })
    }

    fn finish_download<'a>(
        &mut self,
        context: LinkContext,
        request_id: RequestId,
        request: crate::download::FinishDownload,
        out: &mut [u8],
    ) -> Reaction<'a> {
        if let Err(rejection) = self.coordinator.check(request.session_id, &context) {
            return self.reply_error(
                context.link_kind,
                Opcode::FinishDownload,
                request_id,
                invalid_session_body(rejection),
                out,
            );
        }
        let Some(Transfer::Download(download)) = self.transfer else {
            return self.reply_error(
                context.link_kind,
                Opcode::FinishDownload,
                request_id,
                invalid_session_body(SessionRejection::Unknown),
                out,
            );
        };
        if request.received_length != download.source.total_length
            || request.whole_source_crc32 != download.source.crc32
        {
            // §7: "A malformed finish retains the session until matching abort or disconnect so it
            // cannot release another reader's lease."
            let body = FailureCause::Checksum { detail: detail::checksum::WHOLE_PAYLOAD }.body(ClaimStatus::None);
            return self.reply_error(context.link_kind, Opcode::FinishDownload, request_id, body, out);
        }
        let Ok(phase) = download.phase.apply(DownloadEvent::Finished) else {
            return self.reply_error(context.link_kind, Opcode::FinishDownload, request_id, internal_body(), out);
        };
        self.transfer = Some(Transfer::Download(Download { phase, ..download }));
        self.coordinator.revoke_owned_by(&context);
        self.pending = Some(Pending {
            link: context.link_kind,
            request_id: Some(request_id),
            opcode: Opcode::FinishDownload,
            stage: Stage::ReleaseLease { detach: false },
        });
        Reaction::Work(Command::ReleaseLease)
    }

    fn device_control<'a>(
        &mut self,
        link: LinkKind,
        request_id: RequestId,
        opcode: Opcode,
        request: DeviceControlRequest<'a>,
    ) -> Reaction<'a> {
        // §16: the plane "claims nothing", so an active transfer is neither consulted nor touched.
        self.pending = Some(Pending { link, request_id: Some(request_id), opcode, stage: Stage::DeviceControl });
        Reaction::Work(Command::DeviceControl(request))
    }

    // -- outcomes ------------------------------------------------------------------------------

    fn after_upload_claim<'a>(
        &mut self,
        pending: Pending,
        claim: UploadClaim,
        decision: ClaimOutcome,
        out: &mut [u8],
    ) -> Reaction<'a> {
        let (logical_object_id, admission_revision, restarted) = match decision {
            ClaimOutcome::Claimed { logical_object_id, repository_revision } => {
                (logical_object_id, repository_revision, false)
            }
            ClaimOutcome::Restarted { logical_object_id, repository_revision } => {
                (logical_object_id, repository_revision, true)
            }
            ClaimOutcome::Committed(envelope) => {
                return self.reply(pending, &Response::UploadAccepted(Disposition::AlreadyTerminal(envelope)), out)
            }
            ClaimOutcome::Aborted(terminal) => {
                return self.reply(pending, &Response::Error(terminal.body()), out);
            }
            ClaimOutcome::Conflict => return self.reply(pending, &Response::Error(conflict_body()), out),
            ClaimOutcome::ForeignPrincipal => return self.reply(pending, &Response::Error(unauthorized_body()), out),
            ClaimOutcome::Refused(cause) => {
                return self.reply(pending, &Response::Error(cause.body(ClaimStatus::None)), out)
            }
        };
        let Some(context) = self.connection(pending.link).context() else {
            self.pending = None;
            return Reaction::Idle;
        };
        let Some(session_id) = self.coordinator.issue(context) else {
            return self.reply(pending, &Response::Error(internal_body()), out);
        };
        let max_stream_payload = self.max_stream_payload(pending.link);
        self.transfer = Some(Transfer::Upload(Upload {
            operation_id: claim.operation_id,
            kind: claim.kind,
            session_id,
            owner: context,
            phase: UploadPhase::Prepared,
            declared_length: claim.declared_length,
            expected_crc: claim.expected_crc,
            next_offset: 0,
            logical_object_id,
        }));
        let acceptance = UploadAcceptance {
            target_mode: claim.target.mode(),
            // Restart-only: a device that holds no durable progress reports offset zero, with
            // restart-at-zero exactly when work was discarded to get there (§6.1's table).
            flags: if restarted { AcceptanceFlags::RESTARTED } else { AcceptanceFlags::NONE },
            operation_id: claim.operation_id,
            session_id,
            logical_object_id,
            admission_revision,
            durable_next_offset: 0,
            checkpoint_granule: self.profile.checkpoint_granule,
            max_stream_payload,
            finalized_prefix_crc32: 0,
        };
        self.reply(pending, &Response::UploadAccepted(Disposition::Accepted(acceptance)), out)
    }

    fn after_mutation_claim<'a>(&mut self, pending: Pending, decision: ClaimOutcome, out: &mut [u8]) -> Reaction<'a> {
        match decision {
            ClaimOutcome::Claimed { .. } | ClaimOutcome::Restarted { .. } => {
                // §15: a direct mutation is `claimed -> validating -> publishing -> terminal`.
                self.advance(pending, Stage::Mutation(CommandPhase::Validating), Command::Validate)
            }
            ClaimOutcome::Committed(envelope) => self.reply(pending, &Response::MutationResult(envelope), out),
            ClaimOutcome::Aborted(terminal) => self.reply(pending, &Response::Error(terminal.body()), out),
            ClaimOutcome::Conflict => self.reply(pending, &Response::Error(conflict_body()), out),
            ClaimOutcome::ForeignPrincipal => self.reply(pending, &Response::Error(unauthorized_body()), out),
            ClaimOutcome::Refused(cause) => self.reply(pending, &Response::Error(cause.body(ClaimStatus::None)), out),
        }
    }

    fn after_abort<'a>(
        &mut self,
        pending: Pending,
        reply: AbortReply,
        terminal: TerminalError,
        out: &mut [u8],
    ) -> Reaction<'a> {
        self.step_upload(UploadEvent::Aborted);
        self.release_transfer();
        match reply {
            AbortReply::Failure(cause) => self.reply(pending, &Response::Error(cause.body(ClaimStatus::Terminal)), out),
            AbortReply::SessionDetached => {
                self.reply(pending, &Response::SessionAborted(AbortSessionOutcome::Detached), out)
            }
            AbortReply::StreamFault { session_id, category, detail, expected_offset } => {
                let _ = terminal;
                self.pending = None;
                let frame = StreamFrame::Fault {
                    session_id,
                    terminal: true,
                    body: FaultBody {
                        category,
                        detail,
                        expected_next_offset: expected_offset,
                        durable_next_offset: 0,
                        disposition: FaultDisposition::OperationDurablyAborted,
                    },
                };
                match frame.encode_into(out) {
                    Ok(len) => Reaction::Emit { channel: LinkChannel::Stream, len },
                    Err(_) => Reaction::Close(LinkChannel::Stream),
                }
            }
            AbortReply::Silent => {
                self.pending = None;
                Reaction::Idle
            }
        }
    }

    fn after_resolve<'a>(
        &mut self,
        pending: Pending,
        request: StartDownload,
        source: PinnedSource,
        out: &mut [u8],
    ) -> Reaction<'a> {
        let start_offset = request.start_offset.unwrap_or(0);
        if start_offset > source.total_length {
            let mut body = ErrorBody::bare(
                ErrorCategory::INVALID_OFFSET,
                detail::offset::UNEXPECTED_OFFSET,
                RetryGuidance::RESUME_AT_EXPECTED_OFFSET,
            );
            body.presence = crate::error::presence::EXPECTED_OFFSET;
            body.expected_offset = source.total_length;
            return self.reply(pending, &Response::Error(body), out);
        }
        let Some(context) = self.connection(pending.link).context() else {
            self.pending = None;
            return Reaction::Idle;
        };
        let Some(session_id) = self.coordinator.issue(context) else {
            return self.reply(pending, &Response::Error(internal_body()), out);
        };
        let max_payload = self.max_stream_payload(pending.link);
        self.transfer = Some(Transfer::Download(Download {
            session_id,
            owner: context,
            phase: DownloadPhase::Pinned,
            source,
            next_offset: start_offset,
            max_payload,
        }));
        let response = Response::DownloadAccepted(DownloadAccepted {
            store_id: self.profile.store_id,
            session_id,
            logical_object_id: source.logical_object_id,
            pinned_revision: source.revision,
            total_length: source.total_length,
            whole_source_crc32: source.crc32,
            accepted_start_offset: start_offset,
            max_stream_payload: max_payload,
        });
        self.reply(pending, &response, out)
    }

    fn emit_download<'a>(&mut self, pending: Pending, offset: u64, bytes: &[u8], out: &mut [u8]) -> Reaction<'a> {
        self.pending = None;
        let Some(Transfer::Download(download)) = self.transfer else { return Reaction::Idle };
        let _ = pending;
        let frame = StreamFrame::Data {
            session_id: download.session_id,
            direction: Direction::Download,
            offset,
            payload: bytes,
        };
        match frame.encode_into(out) {
            Ok(len) => {
                let phase = download.phase.apply(DownloadEvent::FrameSent).unwrap_or(download.phase);
                self.transfer = Some(Transfer::Download(Download {
                    phase,
                    next_offset: offset.saturating_add(bytes.len() as u64),
                    ..download
                }));
                Reaction::Emit { channel: LinkChannel::Stream, len }
            }
            Err(_) => Reaction::Close(LinkChannel::Stream),
        }
    }

    fn after_device_control(
        &mut self,
        pending: Pending,
        answer: DeviceControlAnswer<'_>,
        out: &mut [u8],
    ) -> Reaction<'static> {
        let response = match answer {
            DeviceControlAnswer::DeviceStatus(status) => Response::DeviceStatus(status),
            DeviceControlAnswer::Config(block) => Response::Config(block),
            DeviceControlAnswer::ClockStatus(status) => Response::ClockStatus(status),
            DeviceControlAnswer::BondForgotten => Response::BondForgotten,
            DeviceControlAnswer::Echo(payload) => Response::Echo(crate::control::Echo { payload }),
            DeviceControlAnswer::ResetStore(store_id) => {
                Response::ResetStoreResult(crate::control::ResetStoreResult { new_store_id: store_id })
            }
            DeviceControlAnswer::Refused(cause) => Response::Error(cause.body(ClaimStatus::None)),
        };
        self.reply(pending, &response, out)
    }

    fn after_query<'a>(&mut self, pending: Pending, report: OperationReport, out: &mut [u8]) -> Reaction<'a> {
        let response = match report {
            OperationReport::Unknown => Response::OperationStatus(OperationStatus::Unknown),
            OperationReport::InProgress(progress) => Response::OperationStatus(OperationStatus::InProgress(progress)),
            OperationReport::Committed(envelope) => Response::OperationStatus(OperationStatus::Committed(envelope)),
            OperationReport::Aborted(terminal) => {
                // §11: the query's Aborted state is followed by the same bare body, so status can be
                // inspected without turning the query itself into a failed request.
                let body = terminal.body();
                return self.reply_status_aborted(pending, body, out);
            }
            OperationReport::NotAuthorized => Response::Error(unauthorized_body()),
        };
        self.reply(pending, &response, out)
    }

    fn reply_status_aborted<'a>(&mut self, pending: Pending, body: ErrorBody<'_>, out: &mut [u8]) -> Reaction<'a> {
        self.reply(pending, &Response::OperationStatus(OperationStatus::Aborted(body)), out)
    }

    fn abandon<'a>(&mut self, pending: Pending, cause: FailureCause) -> Reaction<'a> {
        self.step_upload(UploadEvent::Abandon);
        self.pending = Some(Pending { stage: Stage::Abort(AbortReply::Failure(cause)), ..pending });
        Reaction::Work(Command::Abort(AbortCause::Failed(cause)))
    }

    // -- plumbing ------------------------------------------------------------------------------

    fn append<'a>(&mut self, payload: &'a [u8], offset: u64) -> Reaction<'a> {
        let Some(Transfer::Upload(upload)) = self.transfer else { return Reaction::Idle };
        let phase = upload.phase.apply(UploadEvent::BytesAccepted).unwrap_or(upload.phase);
        self.transfer = Some(Transfer::Upload(Upload {
            phase,
            next_offset: offset.saturating_add(payload.len() as u64),
            ..upload
        }));
        self.pending = Some(Pending {
            link: upload.owner.link_kind,
            request_id: None,
            opcode: Opcode::StartUpload,
            stage: Stage::Append,
        });
        Reaction::Work(Command::Append { offset, bytes: payload })
    }

    fn fault<'a>(
        &mut self,
        session_id: SessionId,
        category: ErrorCategory,
        detail: u16,
        expected_offset: u64,
    ) -> Reaction<'a> {
        // §13: a restart-only upload is durably aborted, and the fault the client sees says so —
        // which is why the abort is made durable before the frame goes out.
        let link = self.transfer.map(|transfer| transfer.owner().link_kind).unwrap_or(LinkKind::Test);
        self.step_upload(UploadEvent::Abandon);
        self.coordinator.revoke();
        self.pending = Some(Pending {
            link,
            request_id: None,
            opcode: Opcode::StartUpload,
            stage: Stage::Abort(AbortReply::StreamFault { session_id, category, detail, expected_offset }),
        });
        Reaction::Work(Command::Abort(AbortCause::StreamFault { detail }))
    }

    fn advance<'a>(&mut self, pending: Pending, stage: Stage, command: Command<'a>) -> Reaction<'a> {
        self.pending = Some(Pending { stage, ..pending });
        Reaction::Work(command)
    }

    fn reply<'a>(&mut self, pending: Pending, response: &Response<'_>, out: &mut [u8]) -> Reaction<'a> {
        self.pending = None;
        self.connection_mut(pending.link).complete();
        match pending.request_id {
            Some(request_id) => Self::encode_response(response, pending.opcode, request_id, out),
            None => Reaction::Idle,
        }
    }

    fn reply_error<'a>(
        &mut self,
        link: LinkKind,
        opcode: Opcode,
        request_id: RequestId,
        body: ErrorBody<'_>,
        out: &mut [u8],
    ) -> Reaction<'a> {
        self.pending = None;
        self.connection_mut(link).complete();
        Self::encode_response(&Response::Error(body), opcode, request_id, out)
    }

    fn refuse_unframed<'a>(
        &mut self,
        link: LinkKind,
        record: &[u8],
        error: DecodeError,
        out: &mut [u8],
    ) -> Reaction<'a> {
        if error.is_unanswerable() {
            // §2: a zero-RequestId frame "is never transmitted"; the record stream is closed.
            return Reaction::Close(LinkChannel::Control);
        }
        // §2: "If enough control header is trustworthy, the adapter returns an error; otherwise it
        // closes that record stream."
        match answerable_header(record) {
            Some((opcode, request_id)) => {
                let _ = link;
                Self::encode_response(&Response::Error(body_of(error)), opcode, request_id, out)
            }
            None => Reaction::Close(LinkChannel::Control),
        }
    }

    fn encode_response<'a>(
        response: &Response<'_>,
        opcode: Opcode,
        request_id: RequestId,
        out: &mut [u8],
    ) -> Reaction<'a> {
        Self::encode_page(response, opcode, request_id, false, out)
    }

    fn encode_page<'a>(
        response: &Response<'_>,
        opcode: Opcode,
        request_id: RequestId,
        more: bool,
        out: &mut [u8],
    ) -> Reaction<'a> {
        match response.encode_frame(opcode, request_id, more, out) {
            Ok(len) => Reaction::Emit { channel: LinkChannel::Control, len },
            // A response that does not fit the buffer the board owns is an internal fault; the
            // control record stream is the only thing left to close.
            Err(_) => Reaction::Close(LinkChannel::Control),
        }
    }

    fn owned_upload(&self, context: LinkContext, session_id: SessionId) -> Result<Upload, ErrorBody<'static>> {
        if let Err(rejection) = self.coordinator.check(session_id, &context) {
            return Err(invalid_session_body(rejection));
        }
        match self.transfer {
            Some(Transfer::Upload(upload)) if upload.session_id == session_id => Ok(upload),
            _ => Err(invalid_session_body(SessionRejection::Unknown)),
        }
    }

    fn step_upload(&mut self, event: UploadEvent) {
        if let Some(Transfer::Upload(upload)) = self.transfer {
            if let Ok(phase) = upload.phase.apply(event) {
                self.transfer = Some(Transfer::Upload(Upload { phase, ..upload }));
            }
        }
    }

    fn release_transfer(&mut self) {
        if let Some(transfer) = self.transfer.take() {
            let owner = transfer.owner();
            self.coordinator.revoke_owned_by(&owner);
        }
    }

    fn max_stream_payload(&self, link: LinkKind) -> u16 {
        let frame = self
            .connection(link)
            .negotiated()
            .map_or(crate::frame::MIN_STREAM_FRAME as u16, |negotiated| negotiated.stream_frame);
        frame.saturating_sub(STREAM_HEADER_LEN as u16)
    }

    fn intent_digest(&self, request: &Request<'_>) -> [u8; 32] {
        request.canonical_intent(self.profile.store_id).map(|intent| intent.digest()).unwrap_or([0; 32])
    }

    fn connection_mut(&mut self, link_kind: LinkKind) -> &mut Connection {
        &mut self.connections[Self::index(link_kind)]
    }

    const fn index(link_kind: LinkKind) -> usize {
        (link_kind.to_u8() as usize) - 1
    }
}

/// The `unsupportedCapability/opcode` refusal §5 requires for a cleared command-flag bit.
fn unsupported_opcode() -> ErrorBody<'static> {
    ErrorBody::bare(
        ErrorCategory::UNSUPPORTED_CAPABILITY,
        detail::capability::OPCODE,
        RetryGuidance::REJECT_PERMANENTLY,
    )
}

fn conflict_body() -> ErrorBody<'static> {
    // §12: `operationIdConflict` "clears both [claim bits], because the conflicting claim belongs to
    // a different intent and the request's own intent was never claimed".
    ErrorBody::bare(
        ErrorCategory::OPERATION_ID_CONFLICT,
        detail::conflict::INTENT_DIGEST,
        RetryGuidance::NEW_ID_FOR_NEW_INTENT,
    )
}

fn unauthorized_body() -> ErrorBody<'static> {
    ErrorBody::bare(
        ErrorCategory::AUTHORIZATION_FAILED,
        detail::authorization::OPERATION_OWNER,
        RetryGuidance::RETRY_AFTER_USER_ACTION,
    )
}

fn internal_body() -> ErrorBody<'static> {
    ErrorBody::bare(ErrorCategory::INTERNAL, detail::internal::INVARIANT, RetryGuidance::RETRY_AFTER_DELAY)
}

fn busy_body(detail: u16, owner: LinkKind) -> ErrorBody<'static> {
    let mut body = ErrorBody::bare(ErrorCategory::BUSY, detail, RetryGuidance::RETRY_AFTER_OWNER_RELEASE);
    body.owner = Owner::from_u8(owner.to_u8());
    body
}

/// The `(category, detail)` a failure takes inside §13's compact fault body, which admits only the
/// ten namespace-zero transport categories.
fn fault_pair(cause: FailureCause) -> (ErrorCategory, u16) {
    let category = cause.category();
    if crate::stream::FaultBody::TRANSPORT_CATEGORIES.contains(&category) {
        let detail = match cause {
            FailureCause::MediaIo { detail }
            | FailureCause::MediaUnavailable { detail }
            | FailureCause::Checksum { detail }
            | FailureCause::Internal { detail } => detail,
            _ => 0,
        };
        (category, detail)
    } else {
        (ErrorCategory::INTERNAL, detail::internal::INVARIANT)
    }
}

fn invalid_session_body(rejection: SessionRejection) -> ErrorBody<'static> {
    // §12: `invalidSession` carries "no owner token or protected state".
    ErrorBody::bare(ErrorCategory::INVALID_SESSION, rejection.detail(), RetryGuidance::RECONNECT_THEN_QUERY)
}

/// A decoder refusal, as the body a device answers with.
fn body_of(error: DecodeError) -> ErrorBody<'static> {
    // Every decoder refusal is permanent: the same bytes decode the same way for ever, so §12's
    // "never" guidance column is the only honest answer for all of them.
    ErrorBody::bare(error.category, error.detail, RetryGuidance::REJECT_PERMANENTLY)
}

/// The opcode and RequestId of a record whose header is trustworthy enough to answer (§2).
fn answerable_header(record: &[u8]) -> Option<(Opcode, RequestId)> {
    if record.len() < HEADER_LEN || record[0..4] != crate::frame::MAGIC {
        return None;
    }
    let opcode = Opcode::from_u16(u16::from_le_bytes([record[6], record[7]]))?;
    let request_id = RequestId::new(u32::from_le_bytes([record[12], record[13], record[14], record[15]]))?;
    Some((opcode, request_id))
}
