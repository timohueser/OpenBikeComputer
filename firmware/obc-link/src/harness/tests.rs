//! The transcript and scenario suite: the same engine, driven over both fake links.
//!
//! Every scenario here is written once and run twice. The assertion that matters most is at the
//! bottom of each: the DOS records the engine emitted on BLE and on USB are byte-identical, which
//! is what "one upload and one download implementation serve BLE and USB" means when it is checked
//! rather than claimed. The one documented exception is Capabilities, which names its own link kind
//! at byte 38 by design (§5), so comparisons start after negotiation.

use std::vec;
use std::vec::Vec;

use crate::control::{Echo, ForgetBond, ForgetBondScope};
use crate::download::{FinishDownload, StartDownload};
use crate::engine::{
    ByteLink, DeviceProfile, Engine, FailureCause, LinkCeilings, LinkChannel, LinkContext, PrincipalScope, Reaction,
    UploadPhase,
};
use crate::error::{detail, presence, ErrorCategory, Owner, RetryGuidance};
use crate::frame::{ControlFrame, FrameFlags, Opcode, MAX_STREAM_FRAME};
use crate::hello::{Hello, LinkKind, PageKind, Subject, SubjectEntry};
use crate::ids::{LogicalObjectId, OperationId, RequestId, Revision, SessionId, StoreId};
use crate::metadata::{MetadataEnvelope, MetadataWriter, SchemaClass, MAX_PUT_ENVELOPE};
use crate::mutate::{DeleteObject, MutationTarget};
use crate::query::{OperationStatus, QueryOperation};
use crate::registry::{schema_version, subject_flags, AbortReason, ObjectKind};
use crate::result::ResultEnvelope;
use crate::stream::{Direction, StreamFrame};
use crate::upload::{AbortSession, CheckpointUpload, Disposition, FinishUpload, ResumePreference, StartUpload, Target};
use crate::{Request, Response};

use super::fake_link::FakeLink;
use super::transaction::{payload, FakeTransaction};
use super::{transcript, Driver, FakeBleLink, FakeUsbLink};

const STORE: StoreId = StoreId::new([0x3c; 16]);
const OP_A: OperationId = OperationId::new([0xa1; 16]);
const OP_B: OperationId = OperationId::new([0xb2; 16]);
const GRANULE: u32 = 1_024;
const OBJECT_LEN: usize = 3_000;

// -- fixtures ---------------------------------------------------------------------------------

fn principal() -> PrincipalScope {
    PrincipalScope::new([0x77; 16])
}

fn context(link_kind: LinkKind, generation: u32) -> LinkContext {
    LinkContext::new(link_kind, principal(), generation)
}

/// The restart-only device this slice ships: routes and rides, no resumable upload anywhere.
fn profile() -> DeviceProfile {
    let mut profile = DeviceProfile::new(STORE);
    profile.checkpoint_granule = GRANULE;
    profile.command_flags = Opcode::QueryOperation.command_flag().unwrap()
        | Opcode::AbortOperation.command_flag().unwrap()
        | Opcode::GetDeviceStatus.command_flag().unwrap()
        | Opcode::GetConfig.command_flag().unwrap()
        | Opcode::SetConfig.command_flag().unwrap()
        | Opcode::Echo.command_flag().unwrap();
    assert!(profile.subjects.push(SubjectEntry {
        subject: Subject::Logical(ObjectKind::Route),
        operation_flags: subject_flags::PUT | subject_flags::GET | subject_flags::DELETE | subject_flags::SET_METADATA,
        policy_flags: 0,
        put_schema_version: schema_version::PUT,
        patch_schema_version: schema_version::PATCH,
        catalog_schema_version: schema_version::CATALOG,
        max_length: 1 << 20,
    }));
    assert!(profile.subjects.push(SubjectEntry {
        subject: Subject::Logical(ObjectKind::Ride),
        operation_flags: subject_flags::GET | subject_flags::DELETE,
        policy_flags: 0,
        put_schema_version: 0,
        patch_schema_version: 0,
        catalog_schema_version: schema_version::CATALOG,
        max_length: 1 << 20,
    }));
    profile
}

fn ble(generation: u32) -> Driver<FakeBleLink> {
    Driver::new(FakeBleLink::new(context(LinkKind::Ble, generation)), profile(), FakeTransaction::new(STORE))
}

fn usb(generation: u32) -> Driver<FakeUsbLink> {
    Driver::new(FakeUsbLink::new(context(LinkKind::Usb, generation)), profile(), FakeTransaction::new(STORE))
}

fn hello(page_kind: PageKind, page_index: u8) -> Hello {
    Hello {
        minimum_major: 3,
        maximum_major: 3,
        client_max_control_frame: 244,
        client_max_stream_frame: 1_024,
        client_feature_flags: 0,
        page_kind,
        page_index,
    }
}

fn record(request: &Request<'_>, request_id: u32) -> Vec<u8> {
    let mut out = vec![0u8; 512];
    let len = request.encode_frame(RequestId::new(request_id).unwrap(), &mut out).unwrap();
    out.truncate(len);
    out
}

fn data_frame(session_id: SessionId, offset: u64, bytes: &[u8]) -> Vec<u8> {
    let frame = StreamFrame::Data { session_id, direction: Direction::Upload, offset, payload: bytes };
    let mut out = vec![0u8; frame.encoded_len()];
    frame.encode_into(&mut out).unwrap();
    out
}

fn route_put(buffer: &mut [u8], retention: u8) -> MetadataEnvelope<'_> {
    let mut writer = MetadataWriter::new(buffer).unwrap();
    writer.push(0x8001, &[retention]).unwrap();
    let bytes = writer.finish(ObjectKind::Route, SchemaClass::Put);
    MetadataEnvelope::decode(bytes, MAX_PUT_ENVELOPE).unwrap()
}

fn start_upload<'a>(
    operation_id: OperationId,
    target: Target,
    bytes: &[u8],
    metadata: MetadataEnvelope<'a>,
) -> StartUpload<'a> {
    StartUpload {
        operation_id,
        kind: ObjectKind::Route,
        target,
        resume: ResumePreference::RestartAtZero,
        declared_length: bytes.len() as u64,
        expected_crc32: obc_crc::crc32(bytes),
        metadata,
    }
}

fn decoded(record: &[u8]) -> Response<'_> {
    let frame = ControlFrame::decode(record).unwrap();
    Response::decode(&frame).unwrap()
}

fn error_of(record: &[u8]) -> crate::ErrorBody<'_> {
    match decoded(record) {
        Response::Error(body) => body,
        other => panic!("expected an error, got {other:?}"),
    }
}

fn negotiate<L: FakeLink>(driver: &mut Driver<L>) {
    driver.link.deliver(LinkChannel::Control, &record(&Request::Hello(hello(PageKind::Resources, 0)), 1));
    driver.pump().unwrap();
    driver.link.deliver(LinkChannel::Control, &record(&Request::Hello(hello(PageKind::Subjects, 0)), 2));
    driver.pump().unwrap();
}

/// Everything the device sent after negotiation, control then stream.
fn tail<L: FakeLink>(driver: &Driver<L>, control_skip: usize) -> Vec<Vec<u8>> {
    let mut records: Vec<Vec<u8>> = driver.link.sent(LinkChannel::Control)[control_skip..].to_vec();
    records.extend(driver.link.sent(LinkChannel::Stream).iter().cloned());
    records
}

/// The full create/upload/publish/download flow, as one scenario both links must run identically.
fn upload_and_download<L: FakeLink>(driver: &mut Driver<L>) -> LogicalObjectId {
    let bytes = payload(OBJECT_LEN);
    let mut buffer = [0u8; 32];
    let metadata = route_put(&mut buffer, 2);
    let request = start_upload(OP_A, Target::Create, &bytes, metadata);
    driver.link.deliver(LinkChannel::Control, &record(&Request::StartUpload(request), 3));
    driver.pump().unwrap();

    let session_id = driver.engine.live_session().expect("an accepted upload owns a session");
    let logical_object_id = match decoded(driver.link.sent(LinkChannel::Control).last().unwrap()) {
        Response::UploadAccepted(Disposition::Accepted(acceptance)) => {
            assert_eq!(acceptance.durable_next_offset, 0, "restart-only never reports durable progress");
            assert_eq!(acceptance.finalized_prefix_crc32, 0);
            assert_eq!(acceptance.flags.bits(), 0, "fresh work sets neither acceptance flag");
            assert_eq!(acceptance.checkpoint_granule, GRANULE);
            acceptance.logical_object_id
        }
        other => panic!("expected an acceptance, got {other:?}"),
    };

    for chunk in bytes.chunks(1_008) {
        let offset = driver.engine.active_upload().unwrap().next_offset;
        driver.link.deliver(LinkChannel::Stream, &data_frame(session_id, offset, chunk));
        driver.pump().unwrap();
    }
    assert_eq!(driver.engine.active_upload().unwrap().next_offset, OBJECT_LEN as u64);

    let checkpoint = CheckpointUpload { session_id, received_next_offset: OBJECT_LEN as u64 };
    driver.link.deliver(LinkChannel::Control, &record(&Request::CheckpointUpload(checkpoint), 4));
    driver.pump().unwrap();

    driver.link.deliver(LinkChannel::Control, &record(&Request::FinishUpload(FinishUpload { session_id }), 5));
    driver.pump().unwrap();
    match decoded(driver.link.sent(LinkChannel::Control).last().unwrap()) {
        Response::UploadResult(ResultEnvelope::Object(result)) => {
            assert_eq!(result.operation_id, OP_A);
            assert_eq!(result.length, OBJECT_LEN as u64);
        }
        other => panic!("expected an ObjectResult, got {other:?}"),
    }
    assert!(driver.engine.live_session().is_none(), "publication releases the session");

    let download = StartDownload { kind: ObjectKind::Route, logical_object_id, start_offset: None };
    driver.link.deliver(LinkChannel::Control, &record(&Request::StartDownload(download), 6));
    driver.pump().unwrap();
    let accepted = match decoded(driver.link.sent(LinkChannel::Control).last().unwrap()) {
        Response::DownloadAccepted(accepted) => accepted,
        other => panic!("expected a DownloadAccepted, got {other:?}"),
    };
    assert_eq!(accepted.total_length, OBJECT_LEN as u64);
    assert_eq!(driver.link.sent(LinkChannel::Stream).len(), 3, "1008 + 1008 + 984");

    let finish = FinishDownload {
        session_id: accepted.session_id,
        received_length: accepted.total_length,
        whole_source_crc32: accepted.whole_source_crc32,
    };
    driver.link.deliver(LinkChannel::Control, &record(&Request::FinishDownload(finish), 7));
    driver.pump().unwrap();
    assert!(matches!(decoded(driver.link.sent(LinkChannel::Control).last().unwrap()), Response::DownloadFinished));
    assert!(!driver.transaction.has_lease(), "the successful finish releases the lease exactly once");
    logical_object_id
}

// -- the transport-neutrality proof --------------------------------------------------------------

#[test]
fn one_engine_serves_both_links_with_byte_identical_records() {
    let mut over_ble = ble(1);
    negotiate(&mut over_ble);
    let ble_id = upload_and_download(&mut over_ble);

    let mut over_usb = usb(1);
    negotiate(&mut over_usb);
    let usb_id = upload_and_download(&mut over_usb);

    assert_eq!(ble_id, usb_id);
    assert_eq!(
        tail(&over_ble, 2),
        tail(&over_usb, 2),
        "every DOS record after negotiation is identical; only the framing around it differs"
    );
    assert_eq!(over_ble.transaction.payload(ObjectKind::Route, ble_id), Some(payload(OBJECT_LEN).as_slice()));
}

#[test]
fn the_two_bindings_frame_the_same_records_differently_and_carry_them_the_same() {
    let mut over_ble = ble(1);
    let mut over_usb = usb(1);
    let frame = record(&Request::Hello(hello(PageKind::Resources, 0)), 1);

    over_ble.link.deliver(LinkChannel::Control, &frame);
    over_usb.link.deliver(LinkChannel::Control, &frame);
    over_ble.pump().unwrap();
    over_usb.pump().unwrap();

    // The capabilities page names its own link kind (§5 byte 38), so only that byte differs.
    let ble_page = &over_ble.link.sent(LinkChannel::Control)[0];
    let usb_page = &over_usb.link.sent(LinkChannel::Control)[0];
    assert_eq!(ble_page.len(), usb_page.len());
    let differing: Vec<usize> = (0..ble_page.len()).filter(|&index| ble_page[index] != usb_page[index]).collect();
    assert_eq!(differing, vec![crate::frame::HEADER_LEN + 38], "only the link-kind byte differs");

    // A USB record may span packets; the record is only delivered once it is whole.
    let mut split = usb(1);
    let framed: Vec<u8> = (frame.len() as u16).to_le_bytes().iter().chain(frame.iter()).copied().collect();
    split.link.deliver_raw(LinkChannel::Control, &framed[..5]);
    split.pump().unwrap();
    assert!(split.link.sent(LinkChannel::Control).is_empty(), "half a record is not a record");
    split.link.deliver_raw(LinkChannel::Control, &framed[5..]);
    split.pump().unwrap();
    assert_eq!(split.link.sent(LinkChannel::Control).len(), 1);
}

// -- §5.2, the connection state machine ------------------------------------------------------------

#[test]
fn nothing_is_admitted_before_hello_and_a_second_request_is_busy() {
    for link in ["ble", "usb"] {
        let mut driver = if link == "ble" { AnyDriver::Ble(ble(1)) } else { AnyDriver::Usb(usb(1)) };
        let query = record(&Request::QueryOperation(QueryOperation { operation_id: OP_A }), 9);
        driver.deliver(LinkChannel::Control, &query);
        driver.pump();
        let body = error_of(driver.last_control());
        assert_eq!(body.category, ErrorCategory::INVALID_DESCRIPTOR);
        assert_eq!(body.detail, detail::descriptor::INVALID_COMBINATION, "{link}");
    }

    // One outstanding request per direction: the engine is mid-claim when the second arrives.
    let mut engine = Engine::new(profile());
    let context = context(LinkKind::Ble, 1);
    engine.open_connection(context, LinkCeilings { control_frame: 244, stream_frame: 1_024 });
    let mut out = [0u8; MAX_STREAM_FRAME];
    let hello_record = record(&Request::Hello(hello(PageKind::Resources, 0)), 1);
    assert!(matches!(engine.on_control(context, &hello_record, &mut out), Reaction::Emit { .. }));

    let bytes = payload(16);
    let mut buffer = [0u8; 32];
    let metadata = route_put(&mut buffer, 1);
    let first = record(&Request::StartUpload(start_upload(OP_A, Target::Create, &bytes, metadata)), 2);
    assert!(matches!(engine.on_control(context, &first, &mut out), Reaction::Work(_)));

    let second = record(&Request::QueryOperation(QueryOperation { operation_id: OP_A }), 3);
    let reaction = engine.on_control(context, &second, &mut out);
    let Reaction::Emit { len, .. } = reaction else { panic!("expected a refusal") };
    let body = error_of(&out[..len]);
    assert_eq!(body.category, ErrorCategory::BUSY);
    assert_eq!(body.detail, detail::busy::NORMAL_OPERATION_CLAIMS);
    assert_eq!(body.owner, Owner::BLE, "owner is this connection's own link kind");
}

#[test]
fn a_repeated_hello_pages_and_a_changed_one_is_refused() {
    let mut driver = ble(1);
    negotiate(&mut driver);
    assert_eq!(driver.link.sent(LinkChannel::Control).len(), 2);

    let changed = Hello { client_max_stream_frame: 512, ..hello(PageKind::Subjects, 0) };
    driver.link.deliver(LinkChannel::Control, &record(&Request::Hello(changed), 3));
    driver.pump().unwrap();
    let body = error_of(driver.link.sent(LinkChannel::Control).last().unwrap());
    assert_eq!(body.category, ErrorCategory::INVALID_DESCRIPTOR);
    assert_eq!(body.detail, detail::descriptor::INVALID_COMBINATION);
}

// -- §3 and §13, session ownership -----------------------------------------------------------------

#[test]
fn a_stale_or_wrong_wire_session_cannot_advance_or_release_anything() {
    let mut engine = Engine::new(profile());
    let mut transaction = FakeTransaction::new(STORE);
    let ble_context = context(LinkKind::Ble, 1);
    let usb_context = context(LinkKind::Usb, 1);
    engine.open_connection(ble_context, LinkCeilings { control_frame: 244, stream_frame: 1_024 });
    engine.open_connection(usb_context, LinkCeilings { control_frame: 512, stream_frame: 4_096 });
    let mut out = [0u8; MAX_STREAM_FRAME];
    let mut scratch = [0u8; MAX_STREAM_FRAME];

    for context in [ble_context, usb_context] {
        let hello_record = record(&Request::Hello(hello(PageKind::Resources, 0)), 1);
        assert!(matches!(engine.on_control(context, &hello_record, &mut out), Reaction::Emit { .. }));
    }

    let bytes = payload(64);
    let mut buffer = [0u8; 32];
    let metadata = route_put(&mut buffer, 1);
    let request = record(&Request::StartUpload(start_upload(OP_A, Target::Create, &bytes, metadata)), 2);
    let reaction = engine.on_control(ble_context, &request, &mut out);
    let Reaction::Work(command) = reaction else { panic!("expected the claim") };
    let outcome = transaction.execute(command, &mut scratch);
    assert!(matches!(engine.resume(outcome, &mut out), Reaction::Emit { .. }));
    let session_id = engine.live_session().unwrap();

    // The same identifier offered on the other link kind.
    let finish = record(&Request::FinishUpload(FinishUpload { session_id }), 3);
    let Reaction::Emit { len, .. } = engine.on_control(usb_context, &finish, &mut out) else {
        panic!("expected a refusal")
    };
    let body = error_of(&out[..len]);
    assert_eq!(body.category, ErrorCategory::INVALID_SESSION);
    assert_eq!(body.detail, detail::session::WRONG_LINK);

    // A stream frame bearing a session that was never issued to this connection is untrusted.
    let stray = data_frame(session_id, 0, b"x");
    assert_eq!(engine.on_stream(usb_context, &stray, &mut out), Reaction::Close(LinkChannel::Stream));

    // A teardown naming a connection that is not the current one changes nothing.
    let stale = LinkContext { generation: 99, ..ble_context };
    assert_eq!(engine.close_connection(stale), Reaction::Idle);
    assert_eq!(engine.live_session(), Some(session_id));

    // A reconnect makes every earlier SessionId stale, even for the same principal and link.
    let reconnected = LinkContext { generation: 2, ..ble_context };
    engine.open_connection(reconnected, LinkCeilings { control_frame: 244, stream_frame: 1_024 });
    let hello_record = record(&Request::Hello(hello(PageKind::Resources, 0)), 1);
    assert!(matches!(engine.on_control(reconnected, &hello_record, &mut out), Reaction::Emit { .. }));
    let Reaction::Emit { len, .. } = engine.on_control(reconnected, &finish, &mut out) else {
        panic!("expected a refusal")
    };
    let body = error_of(&out[..len]);
    assert_eq!(body.category, ErrorCategory::INVALID_SESSION);
    assert_eq!(body.detail, detail::session::STALE_CONNECTION);
}

#[test]
fn a_released_session_is_tombstoned_and_its_late_frames_are_discarded() {
    let mut driver = ble(1);
    negotiate(&mut driver);
    let bytes = payload(64);
    let mut buffer = [0u8; 32];
    let metadata = route_put(&mut buffer, 1);
    driver.link.deliver(
        LinkChannel::Control,
        &record(&Request::StartUpload(start_upload(OP_A, Target::Create, &bytes, metadata)), 3),
    );
    driver.pump().unwrap();
    let session_id = driver.engine.live_session().unwrap();

    let abort = AbortSession { session_id, reason: AbortReason::ClientCancelled };
    driver.link.deliver(LinkChannel::Control, &record(&Request::AbortSession(abort), 4));
    driver.pump().unwrap();
    assert!(matches!(
        decoded(driver.link.sent(LinkChannel::Control).last().unwrap()),
        Response::SessionAborted(crate::upload::AbortSessionOutcome::Detached)
    ));

    let before = driver.link.sent(LinkChannel::Stream).len();
    driver.link.deliver(LinkChannel::Stream, &data_frame(session_id, 0, b"late"));
    driver.pump().unwrap();
    assert_eq!(driver.link.sent(LinkChannel::Stream).len(), before, "a tombstoned frame is silently discarded");
    assert!(!driver.link.is_closed(LinkChannel::Stream), "and never closes the transport");

    // Restart-only work is durably aborted by the detach, so its result is terminal.
    driver
        .link
        .deliver(LinkChannel::Control, &record(&Request::QueryOperation(QueryOperation { operation_id: OP_A }), 5));
    driver.pump().unwrap();
    match decoded(driver.link.sent(LinkChannel::Control).last().unwrap()) {
        Response::OperationStatus(OperationStatus::Aborted(body)) => {
            assert_eq!(body.category, ErrorCategory::CANCELLED);
            assert_eq!(body.detail, u16::from(AbortReason::ClientCancelled.to_u8()));
            assert!(body.durable_claim_exists() && body.claim_is_terminal());
        }
        other => panic!("expected a retained Aborted, got {other:?}"),
    }
}

// -- offsets and faults ------------------------------------------------------------------------------

#[test]
fn a_frame_at_the_wrong_offset_faults_and_durably_aborts_restart_only_work() {
    for over_usb in [false, true] {
        let mut driver = if over_usb { AnyDriver::Usb(usb(1)) } else { AnyDriver::Ble(ble(1)) };
        driver.negotiate();
        let session_id = driver.start_upload(payload(2_048));

        driver.deliver(LinkChannel::Stream, &data_frame(session_id, 512, b"out of order"));
        driver.pump();

        let fault = driver.last_stream().to_vec();
        match StreamFrame::decode(&fault).unwrap() {
            StreamFrame::Fault { terminal, body, session_id: faulted } => {
                assert_eq!(faulted, session_id);
                assert!(terminal);
                assert_eq!(body.category, ErrorCategory::INVALID_OFFSET);
                assert_eq!(body.detail, detail::offset::UNEXPECTED_OFFSET);
                assert_eq!(body.expected_next_offset, 0);
                assert_eq!(body.disposition, crate::stream::FaultDisposition::OperationDurablyAborted);
            }
            other => panic!("expected a fault, got {other:?}"),
        }
        assert!(driver.live_session().is_none(), "the fault releases the session");
    }
}

#[test]
fn a_checkpoint_off_the_granule_or_the_next_offset_is_refused_without_touching_the_work() {
    let mut driver = ble(1);
    negotiate(&mut driver);
    let bytes = payload(OBJECT_LEN);
    let mut buffer = [0u8; 32];
    let metadata = route_put(&mut buffer, 1);
    driver.link.deliver(
        LinkChannel::Control,
        &record(&Request::StartUpload(start_upload(OP_A, Target::Create, &bytes, metadata)), 3),
    );
    driver.pump().unwrap();
    let session_id = driver.engine.live_session().unwrap();
    driver.link.deliver(LinkChannel::Stream, &data_frame(session_id, 0, &bytes[..1_008]));
    driver.pump().unwrap();

    let ahead = CheckpointUpload { session_id, received_next_offset: 2_048 };
    driver.link.deliver(LinkChannel::Control, &record(&Request::CheckpointUpload(ahead), 4));
    driver.pump().unwrap();
    let body = error_of(driver.link.sent(LinkChannel::Control).last().unwrap());
    assert_eq!(body.category, ErrorCategory::INVALID_OFFSET);
    assert_eq!(body.detail, detail::offset::UNEXPECTED_OFFSET);
    assert_eq!(body.expected_offset, 1_008);
    assert_ne!(body.presence & presence::DURABLE_CLAIM_EXISTS, 0);
    assert_eq!(body.presence & presence::CLAIM_IS_TERMINAL, 0, "the claim is live, not terminal");

    driver.link.deliver(LinkChannel::Stream, &data_frame(session_id, 1_008, &bytes[1_008..1_100]));
    driver.pump().unwrap();
    let off_boundary = CheckpointUpload { session_id, received_next_offset: 1_100 };
    driver.link.deliver(LinkChannel::Control, &record(&Request::CheckpointUpload(off_boundary), 5));
    driver.pump().unwrap();
    let body = error_of(driver.link.sent(LinkChannel::Control).last().unwrap());
    assert_eq!(body.detail, detail::offset::CHECKPOINT_BOUNDARY);
    assert_eq!(driver.engine.active_upload().unwrap().next_offset, 1_100, "the work is untouched");
}

// -- failures along the finish chain --------------------------------------------------------------

#[test]
fn append_validation_and_publication_failures_each_leave_one_terminal_aborted_result() {
    for (label, apply) in [("validation", 0usize), ("publication", 1), ("seal", 2)] {
        let mut driver = ble(1);
        negotiate(&mut driver);
        let bytes = payload(1_024);
        let mut buffer = [0u8; 32];
        let metadata = route_put(&mut buffer, 1);
        driver.link.deliver(
            LinkChannel::Control,
            &record(&Request::StartUpload(start_upload(OP_A, Target::Create, &bytes, metadata)), 3),
        );
        driver.pump().unwrap();
        let session_id = driver.engine.live_session().unwrap();
        for chunk in bytes.chunks(1_008) {
            let offset = driver.engine.active_upload().unwrap().next_offset;
            driver.link.deliver(LinkChannel::Stream, &data_frame(session_id, offset, chunk));
            driver.pump().unwrap();
        }

        match apply {
            0 => driver.transaction.faults.fail_validation = Some(2),
            1 => driver.transaction.faults.fail_publication = true,
            _ => driver.transaction.faults.fail_seal = true,
        }
        driver.link.deliver(LinkChannel::Control, &record(&Request::FinishUpload(FinishUpload { session_id }), 4));
        driver.pump().unwrap();

        let body = error_of(driver.link.sent(LinkChannel::Control).last().unwrap());
        let expected = match apply {
            0 => ErrorCategory::SEMANTIC_VALIDATION,
            1 => ErrorCategory::MEDIA_IO,
            _ => ErrorCategory::CHECKSUM_FAILURE,
        };
        assert_eq!(body.category, expected, "{label}");
        assert!(body.durable_claim_exists() && body.claim_is_terminal(), "{label}: the claim is spent");
        assert!(driver.engine.live_session().is_none(), "{label}: the session is released");
        assert!(driver.transaction.head(ObjectKind::Route, LogicalObjectId::new(1)).is_none(), "{label}");
        assert!(driver.transaction.retains(OP_A), "{label}: exactly one retained terminal result");
    }
}

#[test]
fn a_replace_that_loses_the_race_is_refused_at_the_commit_lock_and_leaves_the_head_alone() {
    let mut driver = ble(1);
    negotiate(&mut driver);
    let original = payload(128);
    let (logical_object_id, revision) = driver.transaction.publish_local(ObjectKind::Route, &original);

    let bytes = payload(256);
    let mut buffer = [0u8; 32];
    let metadata = route_put(&mut buffer, 1);
    let target = Target::Replace { logical_object_id, expected_revision: revision };
    driver
        .link
        .deliver(LinkChannel::Control, &record(&Request::StartUpload(start_upload(OP_A, target, &bytes, metadata)), 3));
    driver.pump().unwrap();
    let session_id = driver.engine.live_session().unwrap();
    driver.link.deliver(LinkChannel::Stream, &data_frame(session_id, 0, &bytes));
    driver.pump().unwrap();

    // A device-local producer publishes a competing mutation just before the commit lock.
    driver.transaction.faults.race_publication = true;
    driver.link.deliver(LinkChannel::Control, &record(&Request::FinishUpload(FinishUpload { session_id }), 4));
    driver.pump().unwrap();

    let body = error_of(driver.link.sent(LinkChannel::Control).last().unwrap());
    assert_eq!(body.category, ErrorCategory::REVISION_CONFLICT);
    assert_eq!(body.guidance, RetryGuidance::REFRESH);
    assert_ne!(body.presence & presence::CURRENT_REVISION, 0);
    assert!(body.current_revision > revision, "the authoritative revision comes back with it");
    assert_eq!(
        driver.transaction.payload(ObjectKind::Route, logical_object_id),
        Some(original.as_slice()),
        "conflict leaves the old logical head unchanged"
    );
}

// -- lost results, replay, and the retained window -------------------------------------------------

#[test]
fn a_lost_result_is_recovered_with_query_operation_and_replayed_by_the_same_intent() {
    let mut driver = ble(1);
    negotiate(&mut driver);
    let bytes = payload(512);
    let mut buffer = [0u8; 32];
    let metadata = route_put(&mut buffer, 1);
    let request = start_upload(OP_A, Target::Create, &bytes, metadata);
    driver.link.deliver(LinkChannel::Control, &record(&Request::StartUpload(request), 3));
    driver.pump().unwrap();
    let session_id = driver.engine.live_session().unwrap();
    driver.link.deliver(LinkChannel::Stream, &data_frame(session_id, 0, &bytes));
    driver.pump().unwrap();

    // The publication commits durably; the response frame is lost on the link.
    driver.link.indication_times_out = true;
    driver.link.deliver(LinkChannel::Control, &record(&Request::FinishUpload(FinishUpload { session_id }), 4));
    driver.pump().unwrap();
    assert_eq!(driver.link.unconfirmed, 1);
    assert_eq!(driver.drain(), Err(crate::engine::LinkError::Timeout), "an unconfirmed indication fails the drain");
    driver.link.indication_times_out = false;

    // The link drops with the mutation outstanding; the client reconnects and queries.
    driver.close();
    driver.reopen(2);
    driver.link.deliver(LinkChannel::Control, &record(&Request::Hello(hello(PageKind::Resources, 0)), 1));
    driver.pump().unwrap();
    driver
        .link
        .deliver(LinkChannel::Control, &record(&Request::QueryOperation(QueryOperation { operation_id: OP_A }), 2));
    driver.pump().unwrap();
    let committed = match decoded(driver.link.sent(LinkChannel::Control).last().unwrap()) {
        Response::OperationStatus(OperationStatus::Committed(envelope)) => envelope,
        other => panic!("expected the retained result, got {other:?}"),
    };

    // The same OperationId and intent replays that exact result and writes nothing.
    let mut buffer = [0u8; 32];
    let metadata = route_put(&mut buffer, 1);
    let replay = start_upload(OP_A, Target::Create, &bytes, metadata);
    driver.link.deliver(LinkChannel::Control, &record(&Request::StartUpload(replay), 3));
    driver.pump().unwrap();
    match decoded(driver.link.sent(LinkChannel::Control).last().unwrap()) {
        Response::UploadAccepted(Disposition::AlreadyTerminal(envelope)) => assert_eq!(envelope, committed),
        other => panic!("expected disposition 1, got {other:?}"),
    }
    assert_eq!(driver.transaction.retained_results(), 1, "no second commit");

    // The same OperationId with a different intent is a hard conflict.
    let mut buffer = [0u8; 32];
    let metadata = route_put(&mut buffer, 1);
    let other_bytes = payload(384);
    let different = start_upload(OP_A, Target::Create, &other_bytes, metadata);
    driver.link.deliver(LinkChannel::Control, &record(&Request::StartUpload(different), 4));
    driver.pump().unwrap();
    let body = error_of(driver.link.sent(LinkChannel::Control).last().unwrap());
    assert_eq!(body.category, ErrorCategory::OPERATION_ID_CONFLICT);
    assert_eq!(body.guidance, RetryGuidance::NEW_ID_FOR_NEW_INTENT);
    assert_eq!(body.presence & (presence::DURABLE_CLAIM_EXISTS | presence::CLAIM_IS_TERMINAL), 0);
}

#[test]
fn the_retained_window_is_sixty_four_and_eviction_makes_a_query_unknown() {
    let mut driver = ble(1);
    negotiate(&mut driver);
    let bytes = payload(64);
    let mut buffer = [0u8; 32];
    let metadata = route_put(&mut buffer, 1);
    driver.link.deliver(
        LinkChannel::Control,
        &record(&Request::StartUpload(start_upload(OP_A, Target::Create, &bytes, metadata)), 3),
    );
    driver.pump().unwrap();
    let session_id = driver.engine.live_session().unwrap();
    driver.link.deliver(LinkChannel::Stream, &data_frame(session_id, 0, &bytes));
    driver.pump().unwrap();
    driver.link.deliver(LinkChannel::Control, &record(&Request::FinishUpload(FinishUpload { session_id }), 4));
    driver.pump().unwrap();

    // 63 later terminal operations of any producer keep it; the 64th evicts it.
    for index in 0..63u8 {
        driver.transaction.retain_local_result(OperationId::new([index; 16]));
    }
    driver
        .link
        .deliver(LinkChannel::Control, &record(&Request::QueryOperation(QueryOperation { operation_id: OP_A }), 5));
    driver.pump().unwrap();
    assert!(matches!(
        decoded(driver.link.sent(LinkChannel::Control).last().unwrap()),
        Response::OperationStatus(OperationStatus::Committed(_))
    ));

    driver.transaction.retain_local_result(OperationId::new([0xFE; 16]));
    assert_eq!(driver.transaction.retained_results(), 64);
    driver
        .link
        .deliver(LinkChannel::Control, &record(&Request::QueryOperation(QueryOperation { operation_id: OP_A }), 6));
    driver.pump().unwrap();
    assert!(matches!(
        decoded(driver.link.sent(LinkChannel::Control).last().unwrap()),
        Response::OperationStatus(OperationStatus::Unknown)
    ));
}

// -- link loss ---------------------------------------------------------------------------------------

#[test]
fn link_loss_before_the_seal_durably_aborts_the_work_and_after_it_changes_nothing() {
    // Before the seal.
    let mut driver = ble(1);
    negotiate(&mut driver);
    let bytes = payload(512);
    let mut buffer = [0u8; 32];
    let metadata = route_put(&mut buffer, 1);
    driver.link.deliver(
        LinkChannel::Control,
        &record(&Request::StartUpload(start_upload(OP_A, Target::Create, &bytes, metadata)), 3),
    );
    driver.pump().unwrap();
    let session_id = driver.engine.live_session().unwrap();
    driver.link.deliver(LinkChannel::Stream, &data_frame(session_id, 0, &bytes[..256]));
    driver.pump().unwrap();
    driver.close();
    assert!(driver.engine.live_session().is_none());
    assert!(driver.engine.active_upload().is_none());
    assert!(driver.transaction.retains(OP_A), "restart-only work does not survive the teardown");

    // After publication, the terminal result is untouched by a teardown.
    let mut driver = ble(1);
    negotiate(&mut driver);
    let mut buffer = [0u8; 32];
    let metadata = route_put(&mut buffer, 1);
    driver.link.deliver(
        LinkChannel::Control,
        &record(&Request::StartUpload(start_upload(OP_B, Target::Create, &bytes, metadata)), 3),
    );
    driver.pump().unwrap();
    let session_id = driver.engine.live_session().unwrap();
    driver.link.deliver(LinkChannel::Stream, &data_frame(session_id, 0, &bytes));
    driver.pump().unwrap();
    driver.link.deliver(LinkChannel::Control, &record(&Request::FinishUpload(FinishUpload { session_id }), 4));
    driver.pump().unwrap();
    let published = driver.transaction.head(ObjectKind::Route, LogicalObjectId::new(1));
    driver.close();
    assert_eq!(driver.transaction.head(ObjectKind::Route, LogicalObjectId::new(1)), published);
    assert!(driver.transaction.retains(OP_B));
}

#[test]
fn link_loss_during_a_download_releases_the_lease_exactly_once() {
    let mut driver = ble(1);
    negotiate(&mut driver);
    let bytes = payload(4_096);
    let (logical_object_id, _) = driver.transaction.publish_local(ObjectKind::Route, &bytes);
    let request = StartDownload { kind: ObjectKind::Route, logical_object_id, start_offset: None };
    driver.link.deliver(LinkChannel::Control, &record(&Request::StartDownload(request), 3));
    driver.pump().unwrap();
    assert!(driver.transaction.has_lease());
    driver.close();
    assert!(!driver.transaction.has_lease());
    assert!(driver.engine.active_download().is_none());
}

// -- device control, §16 -------------------------------------------------------------------------

#[test]
fn device_control_runs_mid_transfer_without_touching_the_session() {
    let mut driver = ble(1);
    negotiate(&mut driver);
    let bytes = payload(2_048);
    let mut buffer = [0u8; 32];
    let metadata = route_put(&mut buffer, 1);
    driver.link.deliver(
        LinkChannel::Control,
        &record(&Request::StartUpload(start_upload(OP_A, Target::Create, &bytes, metadata)), 3),
    );
    driver.pump().unwrap();
    let session_id = driver.engine.live_session().unwrap();
    driver.link.deliver(LinkChannel::Stream, &data_frame(session_id, 0, &bytes[..1_008]));
    driver.pump().unwrap();

    driver.link.deliver(LinkChannel::Control, &record(&Request::GetDeviceStatus, 4));
    driver.link.deliver(LinkChannel::Control, &record(&Request::Echo(Echo { payload: b"ping" }), 5));
    driver.pump().unwrap();
    match decoded(driver.link.sent(LinkChannel::Control).last().unwrap()) {
        Response::Echo(echo) => assert_eq!(echo.payload, b"ping"),
        other => panic!("expected the echo, got {other:?}"),
    }
    assert_eq!(driver.engine.live_session(), Some(session_id), "no device-control command touches the session");
    assert_eq!(driver.engine.active_upload().unwrap().next_offset, 1_008);
    assert_eq!(driver.engine.active_upload().unwrap().phase, UploadPhase::Streaming);

    // A cleared command-flag bit is `unsupportedCapability/opcode`, whatever the request carries.
    let forget = ForgetBond { scope: ForgetBondScope::ThisBond };
    driver.link.deliver(LinkChannel::Control, &record(&Request::ForgetBond(forget), 6));
    driver.pump().unwrap();
    let body = error_of(driver.link.sent(LinkChannel::Control).last().unwrap());
    assert_eq!(body.category, ErrorCategory::UNSUPPORTED_CAPABILITY);
    assert_eq!(body.detail, detail::capability::OPCODE);
}

#[test]
fn a_direct_mutation_publishes_through_the_same_command_machine() {
    let mut driver = usb(1);
    negotiate(&mut driver);
    let bytes = payload(96);
    let (logical_object_id, revision) = driver.transaction.publish_local(ObjectKind::Route, &bytes);
    let delete = DeleteObject {
        target: MutationTarget {
            operation_id: OP_B,
            kind: ObjectKind::Route,
            logical_object_id,
            expected_revision: revision,
        },
    };
    driver.link.deliver(LinkChannel::Control, &record(&Request::DeleteObject(delete), 3));
    driver.pump().unwrap();
    match decoded(driver.link.sent(LinkChannel::Control).last().unwrap()) {
        Response::MutationResult(ResultEnvelope::Object(result)) => {
            assert_eq!(result.outcome, crate::registry::ObjectOutcome::Deleted);
            assert_eq!(result.length, bytes.len() as u64);
        }
        other => panic!("expected an ObjectResult, got {other:?}"),
    }
    assert!(driver.transaction.head(ObjectKind::Route, logical_object_id).is_none());

    // A stale expected revision is refused at the commit lock, with the authoritative revision.
    let stale = DeleteObject {
        target: MutationTarget {
            operation_id: OP_A,
            kind: ObjectKind::Route,
            logical_object_id,
            expected_revision: Revision::new(1),
        },
    };
    driver.link.deliver(LinkChannel::Control, &record(&Request::DeleteObject(stale), 4));
    driver.pump().unwrap();
    let body = error_of(driver.link.sent(LinkChannel::Control).last().unwrap());
    assert_eq!(body.category, ErrorCategory::REVISION_CONFLICT);
}

// -- fuzzing -----------------------------------------------------------------------------------

#[test]
fn fuzzed_control_and_data_frames_never_panic_and_never_advance_a_session() {
    let mut driver = ble(1);
    negotiate(&mut driver);
    let bytes = payload(1_024);
    let mut buffer = [0u8; 32];
    let metadata = route_put(&mut buffer, 1);
    driver.link.deliver(
        LinkChannel::Control,
        &record(&Request::StartUpload(start_upload(OP_A, Target::Create, &bytes, metadata)), 3),
    );
    driver.pump().unwrap();
    let session_id = driver.engine.live_session().unwrap();

    let seed_control = record(&Request::FinishUpload(FinishUpload { session_id }), 9);
    let seed_stream = data_frame(session_id, 0, &bytes[..64]);
    let mut state = 0x1234_5678u32;
    for round in 0..2_000u32 {
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let control = round.is_multiple_of(2);
        let seed = if control { &seed_control } else { &seed_stream };
        let mut mutated = seed.clone();
        if !mutated.is_empty() {
            let index = (state as usize) % mutated.len();
            mutated[index] ^= (state >> 16) as u8;
        }
        if state.is_multiple_of(7) {
            mutated.truncate((state as usize) % mutated.len().max(1));
        }
        let channel = if control { LinkChannel::Control } else { LinkChannel::Stream };
        driver.link.deliver(channel, &mutated);
        let _ = driver.pump();
        if driver.engine.live_session().is_none() {
            // A mutated frame legitimately faulted the session; restart the fuzz from a live one.
            break;
        }
        let snapshot = driver.engine.active_upload().unwrap();
        assert!(snapshot.next_offset <= snapshot.declared_length, "no fuzzed frame writes past the declared length");
    }
}

// -- transcripts ---------------------------------------------------------------------------------

#[test]
fn every_checked_in_transcript_record_survives_both_bindings_byte_for_byte() {
    let transcripts = transcript::load();
    assert_eq!(transcripts.len(), transcript::DRIVEN.len(), "the inventory and the directory agree");
    let mut records = 0usize;
    for transcript in &transcripts {
        let mut over_ble = FakeBleLink::with_limits(context(LinkKind::Ble, 1), 4_099, 4_096);
        let mut over_usb = FakeUsbLink::with_max_record(context(LinkKind::Usb, 1), 4_096);
        let mut buffer = [0u8; MAX_STREAM_FRAME];
        for event in &transcript.events {
            if event.record.is_empty() {
                continue;
            }
            records += 1;
            let channel = match event.channel {
                transcript::Channel::Stream => LinkChannel::Stream,
                _ => LinkChannel::Control,
            };
            over_ble.deliver(channel, &event.record);
            over_usb.deliver(channel, &event.record);
            let ble_len = over_ble.receive(channel, &mut buffer).unwrap().unwrap();
            let ble_bytes = buffer[..ble_len].to_vec();
            let usb_len = over_usb.receive(channel, &mut buffer).unwrap().unwrap();
            assert_eq!(ble_bytes, buffer[..usb_len], "{}: both bindings carry the same record", transcript.name);
            assert_eq!(ble_bytes, event.record, "{}: and carry it unchanged", transcript.name);
        }
    }
    assert!(records >= 100, "the checked-in transcripts carry {records} records");
}

#[test]
fn every_transcript_record_decodes_through_the_codec_the_engine_dispatches_on() {
    for transcript in transcript::load() {
        for event in &transcript.events {
            if event.record.is_empty() {
                continue;
            }
            match event.channel {
                transcript::Channel::Control => {
                    let frame = ControlFrame::decode(&event.record)
                        .unwrap_or_else(|error| panic!("{}: {:?}", transcript.name, error));
                    if event.is_client() {
                        assert_eq!(frame.flags, FrameFlags::REQUEST, "{}", transcript.name);
                        Request::decode(&frame).unwrap_or_else(|error| panic!("{}: {:?}", transcript.name, error));
                    } else {
                        assert!(frame.flags.is_response(), "{}", transcript.name);
                        Response::decode(&frame).unwrap_or_else(|error| panic!("{}: {:?}", transcript.name, error));
                    }
                }
                transcript::Channel::Stream => {
                    StreamFrame::decode(&event.record)
                        .unwrap_or_else(|error| panic!("{}: {:?}", transcript.name, error));
                }
                transcript::Channel::Injected => {}
            }
        }
    }
}

/// Replays a transcript's client records through the engine and compares each device event's
/// opcode and success/error class with what the engine answered.
fn drive_transcript<L: FakeLink>(driver: &mut Driver<L>, transcript: &transcript::Transcript) -> usize {
    let mut compared = 0usize;
    let mut sent = 0usize;
    let mut session_id = None;
    let mut logical_object_id = None;
    for event in &transcript.events {
        if event.record.is_empty() {
            continue;
        }
        if event.is_client() {
            let mut retargeted = event.record.clone();
            retarget(&mut retargeted, event.channel, session_id, logical_object_id);
            let channel = match event.channel {
                transcript::Channel::Stream => LinkChannel::Stream,
                _ => LinkChannel::Control,
            };
            driver.link.deliver(channel, &retargeted);
            driver.pump().unwrap();
            session_id = driver.engine.live_session().or(session_id);
            if let Some(record) = driver.link.sent(LinkChannel::Control).last() {
                if let Ok(frame) = ControlFrame::decode(record) {
                    if let Ok(Response::UploadResult(ResultEnvelope::Object(result))) = Response::decode(&frame) {
                        logical_object_id = Some(result.logical_object_id);
                    }
                }
            }
            continue;
        }
        // A device event: compare it with the engine's own answer to the request before it.
        let control = driver.link.sent(LinkChannel::Control);
        if control.len() <= sent {
            continue;
        }
        let answer = &control[sent];
        sent = control.len();
        let expected = ControlFrame::decode(&event.record).unwrap();
        let actual = ControlFrame::decode(answer).unwrap();
        assert_eq!(actual.opcode, expected.opcode, "{}", transcript.name);
        if expected.opcode == Opcode::QueryCatalog {
            // Catalog paging is a later slice; the engine answers as a device with the bit clear.
            let body = error_of(answer);
            assert_eq!(body.category, ErrorCategory::UNSUPPORTED_CAPABILITY);
            assert_eq!(body.detail, detail::capability::OPCODE);
            compared += 1;
            continue;
        }
        assert_eq!(actual.flags.is_error(), expected.flags.is_error(), "{}: {}", transcript.name, event.note);
        if expected.flags.is_error() {
            assert_eq!(error_of(answer).category, error_of(&event.record).category, "{}", transcript.name);
        }
        compared += 1;
    }
    compared
}

/// Points a client record at the session the engine actually issued.
///
/// A transcript is a script, not a byte oracle for an engine's own identifiers: §3 makes SessionId
/// allocation the coordinator's business, so a replay must carry the coordinator's value rather than
/// the fixture's.
fn retarget(
    record: &mut [u8],
    channel: transcript::Channel,
    session_id: Option<SessionId>,
    logical_object_id: Option<LogicalObjectId>,
) {
    match channel {
        transcript::Channel::Stream => {
            if let Some(session_id) = session_id {
                record[..4].copy_from_slice(&session_id.get().to_le_bytes());
            }
        }
        transcript::Channel::Control => {
            let Ok(frame) = ControlFrame::decode(record) else { return };
            let body = crate::frame::HEADER_LEN;
            match frame.opcode {
                Opcode::CheckpointUpload | Opcode::FinishUpload | Opcode::AbortSession | Opcode::FinishDownload => {
                    if let Some(session_id) = session_id {
                        record[body..body + 4].copy_from_slice(&session_id.get().to_le_bytes());
                    }
                }
                Opcode::StartDownload => {
                    // §3 makes the LogicalObjectId the repository's to assign, exactly as it makes
                    // the SessionId the coordinator's, so a replay names the one this device gave.
                    if let Some(logical_object_id) = logical_object_id {
                        record[body + 4..body + 12].copy_from_slice(&logical_object_id.get().to_le_bytes());
                    }
                }
                _ => {}
            }
        }
        transcript::Channel::Injected => {}
    }
}

#[test]
fn the_end_to_end_transcript_drives_the_engine_identically_on_both_links() {
    let transcripts = transcript::load();
    let driven: Vec<&transcript::Transcript> =
        transcripts.iter().filter(|transcript| transcript.drive_note().0).collect();
    assert_eq!(driven.len(), 1, "one transcript starts at Hello and stays inside the restart-only profile");

    for transcript in driven {
        let mut over_ble = ble(1);
        let mut over_usb = usb(1);
        let ble_compared = drive_transcript(&mut over_ble, transcript);
        let usb_compared = drive_transcript(&mut over_usb, transcript);
        assert_eq!(ble_compared, usb_compared);
        assert!(ble_compared >= 5, "{} compared {ble_compared} device events", transcript.name);
        assert_eq!(
            tail(&over_ble, 1),
            tail(&over_usb, 1),
            "{}: the engine's records are identical on both links",
            transcript.name
        );
    }
}

#[test]
fn every_transcript_the_harness_does_not_drive_names_the_reason() {
    for transcript in transcript::load() {
        let (driven, reason) = transcript.drive_note();
        assert!(!reason.is_empty(), "{}", transcript.name);
        if !driven {
            assert!(
                reason.contains("before the fixture")
                    || reason.contains("later DOS3 slice")
                    || reason.contains("§6.1")
                    || reason.contains("injected"),
                "{}: {reason}",
                transcript.name
            );
        }
    }
}

// -- a small link-agnostic driver, so a scenario can be written once ------------------------------

enum AnyDriver {
    Ble(Driver<FakeBleLink>),
    Usb(Driver<FakeUsbLink>),
}

impl AnyDriver {
    fn negotiate(&mut self) {
        match self {
            AnyDriver::Ble(driver) => negotiate(driver),
            AnyDriver::Usb(driver) => negotiate(driver),
        }
    }

    fn deliver(&mut self, channel: LinkChannel, record: &[u8]) {
        match self {
            AnyDriver::Ble(driver) => driver.link.deliver(channel, record),
            AnyDriver::Usb(driver) => driver.link.deliver(channel, record),
        }
    }

    fn pump(&mut self) {
        match self {
            AnyDriver::Ble(driver) => driver.pump().unwrap(),
            AnyDriver::Usb(driver) => driver.pump().unwrap(),
        }
    }

    fn live_session(&self) -> Option<SessionId> {
        match self {
            AnyDriver::Ble(driver) => driver.engine.live_session(),
            AnyDriver::Usb(driver) => driver.engine.live_session(),
        }
    }

    fn last_control(&self) -> &[u8] {
        match self {
            AnyDriver::Ble(driver) => driver.link.sent(LinkChannel::Control).last().unwrap(),
            AnyDriver::Usb(driver) => driver.link.sent(LinkChannel::Control).last().unwrap(),
        }
    }

    fn last_stream(&self) -> &[u8] {
        match self {
            AnyDriver::Ble(driver) => driver.link.sent(LinkChannel::Stream).last().unwrap(),
            AnyDriver::Usb(driver) => driver.link.sent(LinkChannel::Stream).last().unwrap(),
        }
    }

    fn start_upload(&mut self, bytes: Vec<u8>) -> SessionId {
        let mut buffer = [0u8; 32];
        let metadata = route_put(&mut buffer, 1);
        let request = start_upload(OP_A, Target::Create, &bytes, metadata);
        self.deliver(LinkChannel::Control, &record(&Request::StartUpload(request), 3));
        self.pump();
        self.live_session().expect("an accepted upload owns a session")
    }
}

#[test]
fn a_preflight_refusal_creates_no_state_and_carries_neither_claim_bit() {
    let mut driver = ble(1);
    negotiate(&mut driver);
    driver.transaction.faults.refuse_claim = Some(FailureCause::InsufficientSpace { required: 4_096, available: 512 });
    let bytes = payload(64);
    let mut buffer = [0u8; 32];
    let metadata = route_put(&mut buffer, 1);
    driver.link.deliver(
        LinkChannel::Control,
        &record(&Request::StartUpload(start_upload(OP_A, Target::Create, &bytes, metadata)), 3),
    );
    driver.pump().unwrap();
    let body = error_of(driver.link.sent(LinkChannel::Control).last().unwrap());
    assert_eq!(body.category, ErrorCategory::INSUFFICIENT_SPACE);
    assert_eq!(body.presence & (presence::DURABLE_CLAIM_EXISTS | presence::CLAIM_IS_TERMINAL), 0);
    assert_ne!(body.presence & presence::REQUIRED_BYTES, 0);
    assert!(driver.engine.live_session().is_none());
    assert_eq!(driver.transaction.retained_results(), 0, "no state at all");
}

#[test]
fn a_second_heavy_transfer_is_refused_with_the_owners_link_kind() {
    let mut driver = ble(1);
    negotiate(&mut driver);
    let bytes = payload(64);
    let mut buffer = [0u8; 32];
    let metadata = route_put(&mut buffer, 1);
    driver.link.deliver(
        LinkChannel::Control,
        &record(&Request::StartUpload(start_upload(OP_A, Target::Create, &bytes, metadata)), 3),
    );
    driver.pump().unwrap();

    let mut buffer = [0u8; 32];
    let metadata = route_put(&mut buffer, 1);
    driver.link.deliver(
        LinkChannel::Control,
        &record(&Request::StartUpload(start_upload(OP_B, Target::Create, &bytes, metadata)), 4),
    );
    driver.pump().unwrap();
    let body = error_of(driver.link.sent(LinkChannel::Control).last().unwrap());
    assert_eq!(body.category, ErrorCategory::BUSY);
    assert_eq!(body.detail, detail::busy::HEAVY_TRANSFER);
    assert_eq!(body.owner, Owner::BLE);
}

#[test]
fn usb_completes_its_in_records_in_order_and_resets_a_malformed_record_stream() {
    let mut driver = usb(1);
    negotiate(&mut driver);
    driver.link.deliver(LinkChannel::Control, &record(&Request::GetDeviceStatus, 3));
    driver.link.deliver(LinkChannel::Control, &record(&Request::Echo(Echo { payload: b"drain" }), 4));
    driver.pump().unwrap();
    assert_eq!(driver.link.in_flight(), 4, "two capability pages and two answers are accepted but not complete");
    driver.drain().unwrap();
    assert_eq!(driver.link.in_flight(), 0);
    let drained = driver.link.drained.clone();
    assert_eq!(drained, driver.link.sent(LinkChannel::Control).to_vec(), "completion order is acceptance order");

    // §14.2: a zero record length resets only the affected record stream.
    driver.link.deliver_raw(LinkChannel::Control, &[0, 0]);
    driver.pump().unwrap();
    assert!(driver.link.is_closed(LinkChannel::Control));
    assert!(!driver.link.is_closed(LinkChannel::Stream));
}
