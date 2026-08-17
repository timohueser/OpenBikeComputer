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
mod transaction;

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
pub use transaction::Transaction;

use crate::download::{DownloadAccepted, StartDownload};
use crate::error::{detail, ErrorBody, ErrorCategory, Owner, RetryGuidance};
use crate::frame::{ControlFrame, Opcode, HEADER_LEN, MIN_CONTROL_FRAME};
use crate::hello::LinkKind;
use crate::ids::{LogicalObjectId, OperationId, RequestId, SessionId, StoreId};
use crate::query::OperationStatus;
use crate::registry::{subject_flags, AbortReason, ObjectKind};
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

/// What a request is trying to claim, carried from the §11 lookup through to the durable claim.
#[derive(Debug, Clone, Copy)]
enum Work {
    /// A logical Put: the claim is followed by a session and a stream.
    Upload(ClaimIntent),
    /// A direct mutation: the claim is followed by validate and publish.
    Mutation(ClaimIntent),
    /// An AbortOperation command, which also names the operation it cancels (§6.4).
    AbortCommand { intent: ClaimIntent, target: OperationId, reason: AbortReason },
}

impl Work {
    const fn intent(&self) -> ClaimIntent {
        match self {
            Work::Upload(intent) | Work::Mutation(intent) => *intent,
            Work::AbortCommand { intent, .. } => *intent,
        }
    }
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
    /// The abort was the first step of a ResetStore: destroy the store once the work is terminal.
    ThenResetStore(StoreId),
    /// Nothing to answer: the link is already gone.
    Silent,
}

/// What the engine is waiting for.
#[derive(Debug, Clone, Copy)]
enum Stage {
    /// Payload bytes are being written. Nothing answers this: the frame is the whole exchange.
    Append,
    /// §11's idempotency lookup, which creates no state.
    Lookup(Work),
    /// The durable claim, after preflight passed.
    Claiming(Work),
    /// A direct mutation, at the phase of §15's command machine it is currently running.
    Mutation(CommandPhase),
    /// An AbortOperation's `validating` step: the target it is cancelling (§6.4).
    CancelTarget(OperationId),
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

/// The one outstanding piece of work on one connection, and how to answer it.
///
/// The context is the **whole** [`LinkContext`], not just the link kind: §5.2's one-outstanding
/// rule is per link, and a command outstanding when a connection dies must never be answered into
/// whatever connection now occupies that link kind. [`Engine::resume`] is given the context it is
/// resuming for and matches it exactly.
#[derive(Debug, Clone, Copy)]
struct Pending {
    context: LinkContext,
    request_id: Option<RequestId>,
    opcode: Opcode,
    operation_id: Option<OperationId>,
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
    pending: [Option<Pending>; LINK_KINDS],
    /// Per link, the claim whose durable answer is still in flight for a connection that has gone.
    ///
    /// §11 makes a durable claim something that must reach a terminal state — "A claim cannot be
    /// forgotten before terminal state" — so the engine remembers the identifier long enough to
    /// abandon it when the answer lands. There is one slot **per link kind**, because the engine
    /// admits one outstanding command per link and two links can therefore lose two connections,
    /// each with a durable claim in flight, before either answer arrives.
    orphaned_claim: [Option<OperationId>; LINK_KINDS],
}

impl Engine {
    /// A new engine serving `profile`, with no connection open.
    pub fn new(profile: DeviceProfile) -> Self {
        Engine {
            profile,
            connections: [Connection::closed(); LINK_KINDS],
            coordinator: SessionCoordinator::new(),
            transfer: None,
            pending: [None; LINK_KINDS],
            orphaned_claim: [None; LINK_KINDS],
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
    ///
    /// A command left outstanding by the connection this replaces is dropped here: its outcome
    /// arrives with the old context, matches nothing, and is disposed of by [`Engine::resume`]'s
    /// stale path rather than being answered into this new connection.
    pub fn open_connection(&mut self, context: LinkContext, ceilings: LinkCeilings) {
        self.orphan_pending(context.link_kind);
        self.coordinator.open(context);
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
        self.orphan_pending(context.link_kind);
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
                // This teardown is already walking that claim to its terminal state, so it is not
                // also an orphan waiting for a stale outcome.
                self.orphaned_claim[Self::index(context.link_kind)] = None;
                self.set_pending(Pending {
                    context,
                    request_id: None,
                    opcode: Opcode::StartUpload,
                    operation_id: Some(upload.operation_id),
                    stage: Stage::Abort(AbortReply::Silent),
                });
                Reaction::Work(Command::Abort { operation_id: upload.operation_id, cause: AbortCause::LinkLost })
            }
            Transfer::Download(_) => {
                // "releases a matching download lease exactly once".
                self.set_pending(Pending {
                    context,
                    request_id: None,
                    opcode: Opcode::StartDownload,
                    operation_id: None,
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
        if self.pending_for(link).is_some() {
            // A command is outstanding on this link and the adapter must resume it before handing
            // the next record in. Refusing here is §5.2's one-outstanding rule, and it never
            // disturbs the work in flight.
            let body = ConnectionRefusal::Outstanding.body(link);
            return Self::encode_response(&Response::Error(body), frame.opcode, frame.request_id, out);
        }
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
        if self.pending_for(context.link_kind).is_some() {
            // The adapter owes this link a `resume` before its next record; dropping the frame is
            // safer than writing bytes the engine has not accounted for.
            debug_assert!(false, "a stream record arrived while this link owed the engine an outcome");
            return Reaction::Idle;
        }
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
        if self.pending_for(download.owner.link_kind).is_some() || !download.phase.is_streamable() {
            return Reaction::Idle;
        }
        let remaining = download.source.total_length.saturating_sub(download.next_offset);
        if remaining == 0 {
            return Reaction::Idle;
        }
        let length = remaining.min(u64::from(download.max_payload)) as u16;
        self.set_pending(Pending {
            context: download.owner,
            request_id: None,
            opcode: Opcode::StartDownload,
            operation_id: None,
            stage: Stage::ReadSource,
        });
        Reaction::Work(Command::ReadSource { offset: download.next_offset, length })
    }

    /// Takes back the outcome of the command the engine asked this connection for.
    ///
    /// `context` is the connection the command was issued for, and it is matched exactly. An
    /// outcome whose connection is gone — a link that dropped, or a generation that has been
    /// replaced — is **never re-homed onto whatever now occupies that link kind**: it is disposed
    /// of by [`Engine::dispose_stale`], which durably abandons a claim the dead connection had just
    /// made rather than leaving it occupying a slot no one can reach.
    ///
    /// Nothing an outcome carries is ever handed back out: bytes it brings — echo payloads, source
    /// bytes — are encoded into `out`, which is why the reaction it produces borrows nothing.
    pub fn resume(&mut self, context: LinkContext, outcome: Outcome<'_>, out: &mut [u8]) -> Reaction<'static> {
        let pending = match self.pending_for(context.link_kind) {
            Some(pending) if pending.context == context => pending,
            _ => return self.dispose_stale(context, outcome),
        };
        match (pending.stage, outcome) {
            (Stage::Append, Outcome::Appended) => {
                self.clear_pending(pending.context.link_kind);
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
                match (session_id, pending.operation_id) {
                    (Some(session_id), Some(operation_id)) => {
                        self.step_upload(UploadEvent::Abandon);
                        self.coordinator.revoke();
                        let (category, detail) = fault_pair(cause);
                        self.set_pending(Pending {
                            stage: Stage::Abort(AbortReply::StreamFault {
                                session_id,
                                category,
                                detail,
                                expected_offset,
                            }),
                            ..pending
                        });
                        Reaction::Work(Command::Abort { operation_id, cause: AbortCause::Failed(cause) })
                    }
                    _ => {
                        self.clear_pending(pending.context.link_kind);
                        Reaction::Idle
                    }
                }
            }
            (Stage::Lookup(work), Outcome::Claim(decision)) => self.after_lookup(pending, work, decision, out),
            (Stage::Claiming(work), Outcome::Claim(decision)) => self.after_claim(pending, work, decision, out),
            (Stage::Mutation(CommandPhase::Validating), Outcome::Validated) => {
                match (CommandPhase::Validating.apply(CommandEvent::Validated), pending.operation_id) {
                    (Ok(phase), Some(operation_id)) => {
                        self.advance(pending, Stage::Mutation(phase), Command::Publish { operation_id })
                    }
                    _ => self.reply(pending, &Response::Error(internal_body()), out),
                }
            }
            (Stage::CancelTarget(target), Outcome::TargetCancelled(_)) => {
                // §6.4 step 2 releases the target's work. If that work is the live heavy transfer,
                // its session goes with it: leaving one attached would let a later payload frame
                // reach a claim the store has already made terminal.
                self.release_transfer_of(target);
                match (CommandPhase::Validating.apply(CommandEvent::Validated), pending.operation_id) {
                    (Ok(phase), Some(operation_id)) => {
                        self.advance(pending, Stage::Mutation(phase), Command::Publish { operation_id })
                    }
                    _ => self.reply(pending, &Response::Error(internal_body()), out),
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
            (Stage::Seal, Outcome::Sealed) => match pending.operation_id {
                Some(operation_id) => {
                    self.step_upload(UploadEvent::Sealed);
                    self.advance(pending, Stage::Validate, Command::Validate { operation_id })
                }
                None => self.reply(pending, &Response::Error(internal_body()), out),
            },
            (Stage::Validate, Outcome::Validated) => match pending.operation_id {
                Some(operation_id) => {
                    self.step_upload(UploadEvent::Validated);
                    self.advance(pending, Stage::Publish, Command::Publish { operation_id })
                }
                None => self.reply(pending, &Response::Error(internal_body()), out),
            },
            (Stage::Publish, Outcome::Published(envelope)) => {
                self.step_upload(UploadEvent::Published);
                self.release_transfer();
                self.reply(pending, &Response::UploadResult(envelope), out)
            }
            (Stage::Abort(reply), Outcome::Aborted(terminal)) => self.after_abort(pending, reply, terminal, out),
            (Stage::Abort(reply), Outcome::Failed(cause)) => self.after_failed_abort(pending, reply, cause, out),
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
                        self.clear_pending(pending.context.link_kind);
                        Reaction::Idle
                    }
                }
            }
            (Stage::DeviceControl, Outcome::DeviceControl(answer)) => self.after_device_control(pending, answer, out),
            (Stage::Query, Outcome::OperationReport(report)) => self.after_query(pending, report, out),
            (Stage::Lookup(_) | Stage::Claiming(_), Outcome::Failed(cause)) => {
                // A lookup or preflight failure creates no state, so there is nothing to abort.
                self.reply(pending, &Response::Error(cause.body(ClaimStatus::None)), out)
            }
            (Stage::Query, Outcome::Failed(cause)) => {
                // A query is a read. It creates no state of its own, and the operation it *names*
                // is somebody else's — so falling through to `abandon` would abort the queried
                // operation and step the live upload's phase, on the strength of a failed read.
                // §8.1 makes `QueryOperation` an observation; a store that cannot answer says so.
                //
                // `ClaimStatus::None` is about *this request*: the query claimed nothing. Whatever
                // durable claim the queried identity has is untouched and the client may ask again.
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
                self.set_pending(Pending {
                    context,
                    request_id: Some(request_id),
                    opcode,
                    operation_id: Some(request.operation_id),
                    stage: Stage::Query,
                });
                Reaction::Work(Command::QueryOperation {
                    operation_id: request.operation_id,
                    principal: context.principal,
                })
            }
            Request::DeleteObject(request) => {
                let Some(digest) = self.intent_digest(&Request::DeleteObject(request)) else {
                    return self.reply_error(link, opcode, request_id, internal_body(), out);
                };
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
                let Some(digest) = self.intent_digest(&Request::SetMetadata(request)) else {
                    return self.reply_error(link, opcode, request_id, internal_body(), out);
                };
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
                let Some(digest) = self.intent_digest(&Request::AbortOperation(request)) else {
                    return self.reply_error(link, opcode, request_id, internal_body(), out);
                };
                // §6.4's command claims nothing of its own in the object system: it names no kind
                // and no head, and its typed result is an AbortResult rather than an ObjectResult.
                let intent = ClaimIntent {
                    operation_id: request.operation_id,
                    principal: context.principal,
                    opcode,
                    digest,
                    kind: ObjectKind::Route,
                    target: Target::Create,
                    declared_length: 0,
                    expected_crc: 0,
                    target_operation_id: Some(request.target_operation_id),
                };
                self.lookup(
                    context,
                    request_id,
                    opcode,
                    Work::AbortCommand { intent, target: request.target_operation_id, reason: request.reason },
                )
            }
            Request::InstallUpdate(request) => {
                let Some(digest) = self.intent_digest(&Request::InstallUpdate(request)) else {
                    return self.reply_error(link, opcode, request_id, internal_body(), out);
                };
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
                let Some(digest) = self.intent_digest(&Request::AcknowledgeRideImported(request)) else {
                    return self.reply_error(link, opcode, request_id, internal_body(), out);
                };
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
                self.device_control(context, request_id, opcode, DeviceControlRequest::GetDeviceStatus)
            }
            Request::GetConfig => self.device_control(context, request_id, opcode, DeviceControlRequest::GetConfig),
            Request::SetConfig(block) => {
                self.device_control(context, request_id, opcode, DeviceControlRequest::SetConfig(block))
            }
            Request::SetClock(request) => {
                self.device_control(context, request_id, opcode, DeviceControlRequest::SetClock(request))
            }
            Request::ForgetBond(request) => {
                self.device_control(context, request_id, opcode, DeviceControlRequest::ForgetBond(request))
            }
            Request::Echo(echo) => {
                self.device_control(context, request_id, opcode, DeviceControlRequest::Echo(echo.payload))
            }
            Request::ResetStore(request) => self.reset_store(context, request_id, request.echoed_store_id),
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
        if let Err(body) = self.profile.require_operation(request.kind, subject_flags::PUT) {
            return self.reply_error(link, Opcode::StartUpload, request_id, body, out);
        }
        let Some(digest) = self.intent_digest(&Request::StartUpload(request)) else {
            return self.reply_error(link, Opcode::StartUpload, request_id, internal_body(), out);
        };
        // §12's precedence: the idempotency lookup precedes owner/resources and size/space, so the
        // busy and object-length checks live in `preflight` and run only once the lookup has said
        // this identifier carries no retained result and no conflicting intent.
        self.lookup(
            context,
            request_id,
            Opcode::StartUpload,
            Work::Upload(ClaimIntent {
                operation_id: request.operation_id,
                principal: context.principal,
                opcode: Opcode::StartUpload,
                digest,
                kind: request.kind,
                target: request.target,
                declared_length: request.declared_length,
                expected_crc: request.expected_crc32,
                target_operation_id: None,
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
        self.lookup(
            context,
            request_id,
            opcode,
            Work::Mutation(ClaimIntent {
                operation_id,
                principal: context.principal,
                opcode,
                digest,
                kind,
                target,
                declared_length: 0,
                expected_crc: 0,
                target_operation_id: None,
            }),
        )
    }

    /// Asks §11's claim lock what this identifier already carries, without creating state.
    fn lookup<'a>(&mut self, context: LinkContext, request_id: RequestId, opcode: Opcode, work: Work) -> Reaction<'a> {
        let intent = work.intent();
        self.set_pending(Pending {
            context,
            request_id: Some(request_id),
            opcode,
            operation_id: Some(intent.operation_id),
            stage: Stage::Lookup(work),
        });
        Reaction::Work(Command::Lookup(intent))
    }

    /// The owner/resource and size checks §11 puts between the lookup and the durable claim.
    fn preflight(&self, work: &Work) -> Result<(), ErrorBody<'static>> {
        let intent = work.intent();
        if let Work::Upload(_) = work {
            if let Ok(entry) = self.profile.require_operation(intent.kind, subject_flags::PUT) {
                if intent.declared_length > entry.max_length {
                    return Err(
                        FailureCause::ResourceLimit { detail: detail::resource::OBJECT_LENGTH }.body(ClaimStatus::None)
                    );
                }
            }
            if let Some(transfer) = self.transfer {
                // §6.1: a same-intent StartUpload for the operation that already owns the
                // coordinator is a resume, never a refusal — the lookup above has already proved
                // the intent matches, because a different one would have been a conflict.
                let same_work =
                    matches!(transfer, Transfer::Upload(upload) if upload.operation_id == intent.operation_id);
                if !same_work {
                    return Err(busy_body(detail::busy::HEAVY_TRANSFER, transfer.owner().link_kind));
                }
            }
        }
        Ok(())
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
        // §6.2, restart-only: this device *accepts* CheckpointUpload and reports the synchronized
        // prefix rather than refusing it with `unsupportedCapability/feature`. The offset it
        // reports is a progress fact only — §13's teardown rule still durably aborts the work, and
        // no client may resume from it, which is why no acceptance ever reports a nonzero offset.
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
        self.set_pending(Pending {
            context,
            request_id: Some(request_id),
            opcode: Opcode::CheckpointUpload,
            operation_id: Some(upload.operation_id),
            stage: Stage::Checkpoint,
        });
        Reaction::Work(Command::Checkpoint { operation_id: upload.operation_id, offset: request.received_next_offset })
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
        self.set_pending(Pending {
            context,
            request_id: Some(request_id),
            opcode: Opcode::FinishUpload,
            operation_id: Some(upload.operation_id),
            stage: Stage::Seal,
        });
        Reaction::Work(Command::Seal {
            operation_id: upload.operation_id,
            declared_length: upload.declared_length,
            expected_crc: upload.expected_crc,
        })
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
                self.set_pending(Pending {
                    context,
                    request_id: Some(request_id),
                    opcode: Opcode::AbortSession,
                    operation_id: Some(upload.operation_id),
                    stage: Stage::Abort(AbortReply::SessionDetached),
                });
                Reaction::Work(Command::Abort {
                    operation_id: upload.operation_id,
                    cause: AbortCause::Cancelled { reason: request.reason },
                })
            }
            Transfer::Download(_) => {
                self.set_pending(Pending {
                    context,
                    request_id: Some(request_id),
                    opcode: Opcode::AbortSession,
                    operation_id: None,
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
        self.set_pending(Pending {
            context,
            request_id: Some(request_id),
            opcode: Opcode::StartDownload,
            operation_id: None,
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
        self.set_pending(Pending {
            context,
            request_id: Some(request_id),
            opcode: Opcode::FinishDownload,
            operation_id: None,
            stage: Stage::ReleaseLease { detach: false },
        });
        Reaction::Work(Command::ReleaseLease)
    }

    fn device_control<'a>(
        &mut self,
        context: LinkContext,
        request_id: RequestId,
        opcode: Opcode,
        request: DeviceControlRequest<'a>,
    ) -> Reaction<'a> {
        // §16: the plane "claims nothing", so an active transfer is neither consulted nor touched.
        self.set_pending(Pending {
            context,
            request_id: Some(request_id),
            opcode,
            operation_id: None,
            stage: Stage::DeviceControl,
        });
        Reaction::Work(Command::DeviceControl(request))
    }

    /// ResetStore (§16), which is destructive and ends the connection.
    ///
    /// §16 makes it destroy "every object, operation result, and lease" and §5.2 makes a StoreId
    /// change a connection-ending transition. Active work is therefore durably abandoned *before*
    /// the store is destroyed rather than being silently replaced underneath it.
    fn reset_store<'a>(&mut self, context: LinkContext, request_id: RequestId, echoed: StoreId) -> Reaction<'a> {
        if let Some(transfer) = self.transfer {
            let owner = transfer.owner();
            self.coordinator.revoke_owned_by(&owner);
            let operation_id = match transfer {
                Transfer::Upload(upload) => Some(upload.operation_id),
                Transfer::Download(_) => None,
            };
            if let Some(operation_id) = operation_id {
                self.step_upload(UploadEvent::Abandon);
                self.set_pending(Pending {
                    context,
                    request_id: Some(request_id),
                    opcode: Opcode::ResetStore,
                    operation_id: Some(operation_id),
                    stage: Stage::Abort(AbortReply::ThenResetStore(echoed)),
                });
                return Reaction::Work(Command::Abort {
                    operation_id,
                    cause: AbortCause::Cancelled { reason: AbortReason::Superseded },
                });
            }
            self.transfer = None;
        }
        self.device_control(context, request_id, Opcode::ResetStore, DeviceControlRequest::ResetStore(echoed))
    }

    // -- outcomes ------------------------------------------------------------------------------

    /// §11's lookup came back: answer it outright, or run preflight and make the durable claim.
    fn after_lookup<'a>(
        &mut self,
        pending: Pending,
        work: Work,
        decision: ClaimOutcome,
        out: &mut [u8],
    ) -> Reaction<'a> {
        match decision {
            ClaimOutcome::Unclaimed => {
                // Only now — after the idempotency lookup, as §12's precedence requires — may an
                // owner/resource or size refusal be raised, and it creates no state.
                if let Err(body) = self.preflight(&work) {
                    return self.reply(pending, &Response::Error(body), out);
                }
                self.advance(pending, Stage::Claiming(work), Command::Claim(work.intent()))
            }
            // A live claim of the same intent is a resume rather than a fresh claim, so it skips
            // the durable-claim step and goes straight to its acceptance.
            ClaimOutcome::Claimed { .. } | ClaimOutcome::Restarted { .. } => {
                if let Err(body) = self.preflight(&work) {
                    return self.reply(pending, &Response::Error(body), out);
                }
                self.after_claim(pending, work, decision, out)
            }
            _ => self.replay_terminal(pending, work, decision, out),
        }
    }

    /// The claim is durable (or the lookup resolved it): move to the work the opcode implies.
    fn after_claim<'a>(
        &mut self,
        pending: Pending,
        work: Work,
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
            _ => return self.replay_terminal(pending, work, decision, out),
        };
        let intent = work.intent();
        match work {
            Work::Mutation(_) => {
                // §15: a direct mutation is `claimed -> validating -> publishing -> terminal`.
                self.advance(
                    pending,
                    Stage::Mutation(CommandPhase::Validating),
                    Command::Validate { operation_id: intent.operation_id },
                )
            }
            Work::AbortCommand { target, reason, .. } => {
                // §6.4: the target is durably marked terminal before the abort command's own
                // AbortResult is committed, and that result is what the client receives.
                self.advance(
                    pending,
                    Stage::CancelTarget(target),
                    Command::CancelTarget { operation_id: intent.operation_id, target, reason },
                )
            }
            Work::Upload(_) => {
                let Some(context) =
                    self.connection(pending.context.link_kind).context().filter(|open| open == &pending.context)
                else {
                    // The connection that asked is gone: its acceptance would bind a session to a
                    // connection that cannot use it (§3), so nothing is issued.
                    self.clear_pending(pending.context.link_kind);
                    return Reaction::Idle;
                };
                let Some(session_id) = self.coordinator.issue(context) else {
                    return self.reply(pending, &Response::Error(internal_body()), out);
                };
                let max_stream_payload = self.max_stream_payload(context.link_kind);
                self.transfer = Some(Transfer::Upload(Upload {
                    operation_id: intent.operation_id,
                    kind: intent.kind,
                    session_id,
                    owner: context,
                    phase: UploadPhase::Prepared,
                    declared_length: intent.declared_length,
                    expected_crc: intent.expected_crc,
                    next_offset: 0,
                    logical_object_id,
                }));
                let acceptance = UploadAcceptance {
                    target_mode: intent.target.mode(),
                    // Restart-only: a device that holds no durable progress reports offset zero,
                    // with restart-at-zero exactly when work was discarded to get there (§6.1).
                    flags: if restarted { AcceptanceFlags::RESTARTED } else { AcceptanceFlags::NONE },
                    operation_id: intent.operation_id,
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
        }
    }

    /// The §11 answers that end the request without any new work.
    fn replay_terminal<'a>(
        &mut self,
        pending: Pending,
        work: Work,
        decision: ClaimOutcome,
        out: &mut [u8],
    ) -> Reaction<'a> {
        let response = match decision {
            ClaimOutcome::Committed(envelope) => match work {
                // §6.1: a retained success replays as the operation's own typed response.
                Work::Upload(_) => Response::UploadAccepted(Disposition::AlreadyTerminal(envelope)),
                _ => Response::MutationResult(envelope),
            },
            ClaimOutcome::Aborted(terminal) => Response::Error(terminal.body()),
            ClaimOutcome::Conflict => Response::Error(conflict_body()),
            ClaimOutcome::ForeignPrincipal => Response::Error(unauthorized_body()),
            ClaimOutcome::Refused(cause) => Response::Error(cause.body(ClaimStatus::None)),
            ClaimOutcome::Unclaimed | ClaimOutcome::Claimed { .. } | ClaimOutcome::Restarted { .. } => {
                Response::Error(internal_body())
            }
        };
        self.reply(pending, &response, out)
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
                self.clear_pending(pending.context.link_kind);
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
            AbortReply::ThenResetStore(echoed) => {
                // The work is terminal; now the store itself may go (§16).
                let request_id = pending.request_id;
                self.clear_pending(pending.context.link_kind);
                match request_id {
                    Some(request_id) => self.device_control(
                        pending.context,
                        request_id,
                        Opcode::ResetStore,
                        DeviceControlRequest::ResetStore(echoed),
                    ),
                    None => Reaction::Idle,
                }
            }
            AbortReply::Silent => {
                self.clear_pending(pending.context.link_kind);
                Reaction::Idle
            }
        }
    }

    /// An abort the store could not make durable.
    ///
    /// §11 makes a durable claim something that must reach a terminal state — but that is the
    /// *store's* obligation, and a medium that fails under the terminal record has not discharged
    /// it. What the engine must not do is retry: the failure comes back from `Stage::Abort`, and
    /// treating it like any other failure would issue another `Abort`, which fails the same way, for
    /// ever. So the session is released, the request is answered exactly once with the claim still
    /// **live**, and the client is sent to `QueryOperation` — which is where a mount's recovery pass
    /// makes the truth available, because the claim it left behind is still on the card.
    fn after_failed_abort<'a>(
        &mut self,
        pending: Pending,
        reply: AbortReply,
        cause: FailureCause,
        out: &mut [u8],
    ) -> Reaction<'a> {
        self.step_upload(UploadEvent::Aborted);
        self.release_transfer();
        match reply {
            AbortReply::StreamFault { session_id, .. } => {
                self.clear_pending(pending.context.link_kind);
                // §13's disposition 2, exactly as it reads: "the stream transport is closed; query
                // the operation's status". Reporting `operationDurablyAborted` here would be a lie.
                let frame = StreamFrame::Fault {
                    session_id,
                    terminal: true,
                    body: FaultBody {
                        category: ErrorCategory::MEDIA_IO,
                        detail: detail::media_io::UNCERTAIN_COMMIT,
                        expected_next_offset: 0,
                        durable_next_offset: 0,
                        disposition: FaultDisposition::StreamClosedQueryStatus,
                    },
                };
                match frame.encode_into(out) {
                    Ok(len) => Reaction::Emit { channel: LinkChannel::Stream, len },
                    Err(_) => Reaction::Close(LinkChannel::Stream),
                }
            }
            AbortReply::Silent => {
                self.clear_pending(pending.context.link_kind);
                Reaction::Idle
            }
            // Including a ResetStore whose preceding abort failed: the store is not destroyed under
            // work that is still claimed.
            _ => self.reply(pending, &Response::Error(cause.body(ClaimStatus::Live)), out),
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
        let Some(context) =
            self.connection(pending.context.link_kind).context().filter(|open| open == &pending.context)
        else {
            self.clear_pending(pending.context.link_kind);
            return Reaction::Idle;
        };
        let Some(session_id) = self.coordinator.issue(context) else {
            return self.reply(pending, &Response::Error(internal_body()), out);
        };
        let max_payload = self.max_stream_payload(context.link_kind);
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
        self.clear_pending(pending.context.link_kind);
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
                // §16: reset "closes every connection, session, and lease", and §5.2 makes a
                // StoreId change a connection-ending transition. The new identity is adopted before
                // the response goes out, because every canonical intent of §11 is computed over the
                // *current* StoreId and the old one no longer exists.
                self.profile.store_id = store_id;
                self.transfer = None;
                self.coordinator = SessionCoordinator::new();
                let response = Response::ResetStoreResult(crate::control::ResetStoreResult { new_store_id: store_id });
                let reaction = self.reply(pending, &response, out);
                for link in [LinkKind::Ble, LinkKind::Usb, LinkKind::Test] {
                    self.connection_mut(link).close();
                    self.clear_pending(link);
                }
                return reaction;
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
        let Some(operation_id) = pending.operation_id else {
            self.clear_pending(pending.context.link_kind);
            return Reaction::Idle;
        };
        self.step_upload(UploadEvent::Abandon);
        self.set_pending(Pending { stage: Stage::Abort(AbortReply::Failure(cause)), ..pending });
        Reaction::Work(Command::Abort { operation_id, cause: AbortCause::Failed(cause) })
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
        self.set_pending(Pending {
            context: upload.owner,
            request_id: None,
            opcode: Opcode::StartUpload,
            operation_id: Some(upload.operation_id),
            stage: Stage::Append,
        });
        Reaction::Work(Command::Append { operation_id: upload.operation_id, offset, bytes: payload })
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
        let Some(Transfer::Upload(upload)) = self.transfer else { return Reaction::Idle };
        self.step_upload(UploadEvent::Abandon);
        self.coordinator.revoke();
        self.set_pending(Pending {
            context: upload.owner,
            request_id: None,
            opcode: Opcode::StartUpload,
            operation_id: Some(upload.operation_id),
            stage: Stage::Abort(AbortReply::StreamFault { session_id, category, detail, expected_offset }),
        });
        Reaction::Work(Command::Abort { operation_id: upload.operation_id, cause: AbortCause::StreamFault { detail } })
    }

    fn advance<'a>(&mut self, pending: Pending, stage: Stage, command: Command<'a>) -> Reaction<'a> {
        self.set_pending(Pending { stage, ..pending });
        Reaction::Work(command)
    }

    fn reply<'a>(&mut self, pending: Pending, response: &Response<'_>, out: &mut [u8]) -> Reaction<'a> {
        self.clear_pending(pending.context.link_kind);
        // Only the connection that asked may have its outstanding slot released: a reply computed
        // for a connection that has since been replaced must not free the new one's slot.
        if self.connection(pending.context.link_kind).context() == Some(pending.context) {
            self.connection_mut(pending.context.link_kind).complete();
        }
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
        self.clear_pending(link);
        self.connection_mut(link).complete();
        Self::encode_response(&Response::Error(body), opcode, request_id, out)
    }

    /// The pending work of one link, if it has any.
    fn pending_for(&self, link_kind: LinkKind) -> Option<Pending> {
        self.pending[Self::index(link_kind)]
    }

    /// Drops a link's pending work, remembering a claim whose durable answer is still in flight.
    ///
    /// Every stage from the durable claim onwards implies a claim that must reach a terminal state,
    /// not just the claim itself: a mutation dropped between `Validate` and `Publish`, or an upload
    /// dropped mid-append, owns exactly the same durable row. Only [`Stage::Lookup`] is exempt,
    /// because §11's lookup creates no state.
    fn orphan_pending(&mut self, link_kind: LinkKind) {
        let Some(pending) = self.pending[Self::index(link_kind)].take() else { return };
        let holds_claim = match pending.stage {
            Stage::Claiming(_)
            | Stage::Mutation(_)
            | Stage::CancelTarget(_)
            | Stage::Append
            | Stage::Checkpoint
            | Stage::Seal
            | Stage::Validate
            | Stage::Publish => true,
            // An abort already *is* the walk to a terminal state, and the rest carry no claim.
            Stage::Lookup(_)
            | Stage::Abort(_)
            | Stage::Resolve(_)
            | Stage::ReadSource
            | Stage::ReleaseLease { .. }
            | Stage::DeviceControl
            | Stage::Query => false,
        };
        if holds_claim {
            self.orphaned_claim[Self::index(link_kind)] = pending.operation_id;
        }
    }

    fn set_pending(&mut self, pending: Pending) {
        self.pending[Self::index(pending.context.link_kind)] = Some(pending);
    }

    fn clear_pending(&mut self, link_kind: LinkKind) {
        self.pending[Self::index(link_kind)] = None;
    }

    /// Disposes of an outcome whose connection is gone.
    ///
    /// §13 makes teardown the moment work is abandoned, and §11 makes a durable claim something
    /// that must reach a terminal state rather than linger. An outcome that lands after its
    /// connection died is therefore never answered and never re-homed: it is dropped, and if it
    /// reports a claim that has just become durable, that claim is durably abandoned here. The
    /// abort's own outcome is stale in the same way and ends the chain.
    fn dispose_stale(&mut self, context: LinkContext, outcome: Outcome<'_>) -> Reaction<'static> {
        let _ = outcome;
        // Whatever the outcome says, the connection it belongs to is gone. If that connection left
        // a durable claim behind, this is the moment it is abandoned; the abort's own outcome is
        // stale in the same way and finds the slot empty, which ends the chain.
        match self.orphaned_claim[Self::index(context.link_kind)].take() {
            Some(operation_id) => Reaction::Work(Command::Abort { operation_id, cause: AbortCause::LinkLost }),
            None => Reaction::Idle,
        }
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

    /// Releases the heavy transfer when it belongs to `operation_id`, session included.
    fn release_transfer_of(&mut self, operation_id: OperationId) {
        if let Some(Transfer::Upload(upload)) = self.transfer {
            if upload.operation_id == operation_id {
                self.coordinator.revoke_owned_by(&upload.owner);
                self.transfer = None;
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

    /// The SHA-256 of §11's canonical intent for a claiming request.
    ///
    /// `None` means this crate's codec does not build an intent for that opcode, which for a
    /// claiming request is an internal contract error rather than a digest of zeroes: two different
    /// intents that both hashed to zero would compare equal and replay each other's results.
    fn intent_digest(&self, request: &Request<'_>) -> Option<[u8; 32]> {
        request.canonical_intent(self.profile.store_id).map(|intent| intent.digest())
    }

    fn connection_mut(&mut self, link_kind: LinkKind) -> &mut Connection {
        &mut self.connections[Self::index(link_kind)]
    }

    /// The slot one link kind owns. Total over the enum, so a new link kind is a compile error
    /// here rather than an out-of-bounds index at runtime.
    const fn index(link_kind: LinkKind) -> usize {
        match link_kind {
            LinkKind::Ble => 0,
            LinkKind::Usb => 1,
            LinkKind::Test => 2,
        }
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
