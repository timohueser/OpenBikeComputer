//! The fixture inventory itself: every vector `Device_Object_Vectors_v2.md` requires of the wire.
//!
//! Everything here is data. The builders it calls live in the parent module and write bytes at the
//! offsets the protocol's own tables give, so nothing in this file goes through the codec.

use std::format;
use std::string::ToString;
use std::vec;
use std::vec::Vec;

use super::*;
use crate::error::{detail, presence, ErrorCategory, Owner, RetryGuidance};
use crate::frame::{FrameFlags, Opcode};

// Frame flag words, written as the numbers a foreign codec reads.
const REQUEST: u16 = 0;
const OK: u16 = FrameFlags::RESPONSE;
const OK_MORE: u16 = FrameFlags::RESPONSE | FrameFlags::MORE;
const ERR: u16 = FrameFlags::RESPONSE | FrameFlags::ERROR;

// Subject operation and policy flag words.
const PUT: u16 = 1;
const GET: u16 = 1 << 1;
const DELETE: u16 = 1 << 2;
const SET_META: u16 = 1 << 3;
const RESUMABLE_UP: u16 = 1 << 4;
const RESUMABLE_DOWN: u16 = 1 << 5;
const DRAFT_FINALIZE: u16 = 1 << 6;

/// The command-flag word a fully featured device advertises: bits `0..=16`.
const ALL_COMMANDS: u32 = (1 << 17) - 1;

fn control(
    name: &str,
    direction: &'static str,
    opcode: Opcode,
    flags: u16,
    request_id: u32,
    payload: Vec<u8>,
    note: &str,
) -> ControlVector {
    ControlVector {
        name: name.to_string(),
        direction,
        opcode,
        flags,
        request_id,
        payload,
        note: note.to_string(),
        boundary: "",
    }
}

fn bounded(vector: ControlVector, boundary: &'static str) -> ControlVector {
    ControlVector { boundary, ..vector }
}

fn request(name: &str, opcode: Opcode, request_id: u32, payload: Vec<u8>, note: &str) -> ControlVector {
    control(name, "request", opcode, REQUEST, request_id, payload, note)
}

fn response(name: &str, opcode: Opcode, request_id: u32, payload: Vec<u8>, note: &str) -> ControlVector {
    control(name, "response", opcode, OK, request_id, payload, note)
}

// ---------------------------------------------------------------------------------------------
// Registered metadata envelopes, built field by field from `Device_Object_Registries_v2.md` §4.
// ---------------------------------------------------------------------------------------------

/// Route Put v1: one required retention byte. 13 bytes.
pub fn route_put(retention: u8) -> Vec<u8> {
    envelope(1, 1, &[(0x8001, vec![retention])])
}

/// Trip Put v1: no fields. The canonical eight-byte header with both counts zero.
pub fn trip_put() -> Vec<u8> {
    envelope(2, 1, &[])
}

/// Weather Put v1: all six required facts. 68 bytes — the largest a device can produce.
pub fn weather_put(
    request_id: u64,
    latitude_e7: i32,
    longitude_e7: i32,
    radius: u32,
    issued: i64,
    until: i64,
) -> Vec<u8> {
    envelope(
        4,
        1,
        &[
            (0x8001, request_id.to_le_bytes().to_vec()),
            (0x8002, latitude_e7.to_le_bytes().to_vec()),
            (0x8003, longitude_e7.to_le_bytes().to_vec()),
            (0x8004, radius.to_le_bytes().to_vec()),
            (0x8005, issued.to_le_bytes().to_vec()),
            (0x8006, until.to_le_bytes().to_vec()),
        ],
    )
}

/// Route SetMetadata v128, with whichever of the three optional fields are supplied.
pub fn route_patch(retention: Option<u8>, selected: Option<bool>, name: Option<&str>) -> Vec<u8> {
    let mut fields: Vec<(u16, Vec<u8>)> = Vec::new();
    if let Some(retention) = retention {
        fields.push((0x8001, vec![retention]));
    }
    if let Some(selected) = selected {
        fields.push((0x8002, vec![u8::from(selected)]));
    }
    if let Some(name) = name {
        fields.push((0x8003, name.as_bytes().to_vec()));
    }
    envelope(1, 128, &fields)
}

/// Volume-manifest SetMetadata v128: the one selected flag.
pub fn volume_patch(selected: bool) -> Vec<u8> {
    envelope(6, 128, &[(0x8001, vec![u8::from(selected)])])
}

/// Route catalog projection v64. Base tags ascend `1, 2, 3, 4`; the last two are noncritical.
pub fn route_catalog(name: &str, retention: u8, selected: Option<bool>, created: Option<i64>) -> Vec<u8> {
    let mut fields: Vec<(u16, Vec<u8>)> = vec![(0x8001, name.as_bytes().to_vec()), (0x8002, vec![retention])];
    if let Some(selected) = selected {
        fields.push((0x0003, vec![u8::from(selected)]));
    }
    if let Some(created) = created {
        fields.push((0x0004, created.to_le_bytes().to_vec()));
    }
    envelope(1, 64, &fields)
}

/// Ride catalog projection v64: four required fields, 41 bytes.
pub fn ride_catalog(start_utc: i64, duration_s: u32, distance_m: u32, imported: bool) -> Vec<u8> {
    envelope(
        3,
        64,
        &[
            (0x8001, start_utc.to_le_bytes().to_vec()),
            (0x8002, duration_s.to_le_bytes().to_vec()),
            (0x8003, distance_m.to_le_bytes().to_vec()),
            (0x8004, vec![u8::from(imported)]),
        ],
    )
}

/// Trip catalog projection v64.
pub fn trip_catalog(name: &str, stages: u16) -> Vec<u8> {
    envelope(2, 64, &[(0x8001, name.as_bytes().to_vec()), (0x8002, stages.to_le_bytes().to_vec())])
}

/// Weather catalog projection v64.
pub fn weather_catalog(request_id: u64, issued: i64, until: i64) -> Vec<u8> {
    envelope(
        4,
        64,
        &[
            (0x8001, request_id.to_le_bytes().to_vec()),
            (0x8002, issued.to_le_bytes().to_vec()),
            (0x8003, until.to_le_bytes().to_vec()),
        ],
    )
}

/// Volume-manifest catalog projection v64.
pub fn volume_catalog(name: &str, selected: bool, parts: u16) -> Vec<u8> {
    envelope(
        6,
        64,
        &[
            (0x8001, name.as_bytes().to_vec()),
            (0x8002, vec![u8::from(selected)]),
            (0x8003, parts.to_le_bytes().to_vec()),
        ],
    )
}

/// Update-package catalog projection v64.
pub fn update_catalog(version: &str, state: u8, digest: [u8; 32]) -> Vec<u8> {
    envelope(7, 64, &[(0x8001, version.as_bytes().to_vec()), (0x8002, vec![state]), (0x8003, digest.to_vec())])
}

// ---------------------------------------------------------------------------------------------
// Control vectors.
// ---------------------------------------------------------------------------------------------

/// A committed route ObjectResult, reused across fixtures and transcripts.
pub fn committed_route_result(operation: [u8; 16], logical_id: u64, revision: u64, length: u64, crc: u32) -> Vec<u8> {
    result_envelope(1, &object_result(operation, STORE, 1, 0, logical_id, revision, length, crc))
}

/// The payload bytes the create transcript uploads.
pub fn route_payload() -> Vec<u8> {
    deterministic(3000, 251)
}

/// The bytes one draft part carries.
pub fn draft_part_payload() -> Vec<u8> {
    deterministic(65_536, 241)
}

/// The bytes the volume manifest carries: its 96-byte header plus three 56-byte records.
pub fn manifest_payload() -> Vec<u8> {
    deterministic(96 + 56 * 3, 239)
}

/// A deterministic byte source.
///
/// Every CRC in the suite is computed over exactly these bytes and never invented. That matters
/// most for the finalized *prefix* CRCs: a fabricated one, or one clamped to the whole object,
/// would let a codec that hashes the wrong span pass the very vector meant to catch it.
fn deterministic(len: usize, modulus: usize) -> Vec<u8> {
    (0..len).map(|index| (index % modulus) as u8).collect()
}

/// Every control vector in the suite.
pub fn controls() -> Vec<ControlVector> {
    let mut all: Vec<ControlVector> = Vec::new();
    let payload = route_payload();
    let payload_crc = crc32(&payload);
    let payload_len = payload.len() as u64;
    let part = draft_part_payload();
    let part_crc = crc32(&part);
    let manifest = manifest_payload();
    let manifest_crc = crc32(&manifest);

    // ---- Hello and Capabilities -------------------------------------------------------------
    all.push(bounded(
        request(
            "hello-resource-page",
            Opcode::Hello,
            1,
            hello(3, 3, 244, 1024, 0, 0),
            "The 12-byte Hello asking for resource page zero.",
        ),
        "minimum",
    ));
    all.push(request(
        "hello-subject-page-one",
        Opcode::Hello,
        3,
        hello(3, 3, 244, 1024, 1, 1),
        "A repeated Hello may differ only in page kind and index; every negotiation field is byte-identical.",
    ));
    all.push(request(
        "hello-at-the-192-byte-floor",
        Opcode::Hello,
        4,
        hello(3, 3, 192, 64, 0, 0),
        "A client advertising exactly the two protocol minima.",
    ));

    let subjects = [
        subject(1, 1, PUT | GET | DELETE | SET_META | RESUMABLE_UP | RESUMABLE_DOWN, 0, 1, 128, 64, 8 * 1024 * 1024),
        subject(1, 2, PUT | GET | DELETE, 0, 1, 0, 64, 1024 * 1024),
        subject(1, 3, GET | DELETE | RESUMABLE_DOWN, 0, 0, 0, 64, 64 * 1024 * 1024),
        subject(1, 4, PUT | GET | DELETE, 1 << 3, 1, 0, 64, 512 * 1024),
        subject(1, 6, GET | DELETE | SET_META | DRAFT_FINALIZE, 1, 0, 128, 64, 4096),
        subject(1, 7, PUT | GET | DELETE | RESUMABLE_UP, 1 | (1 << 1), 1, 0, 64, 2 * 1024 * 1024),
        subject(2, 1, PUT | RESUMABLE_UP, 1, 0, 0, 0, 512 * 1024 * 1024),
        subject(2, 2, PUT | RESUMABLE_UP, 1, 0, 0, 0, 512 * 1024 * 1024),
    ];
    all.push(response(
        "capabilities-resource-page",
        Opcode::Hello,
        1,
        capabilities(0b0011, Some(STORE), 2, true, 7, ALL_COMMANDS, 8, 0, 0, 0, 1, &resource_limits(3_221_225_472)),
        "The 112-byte resource page. Byte 54 equals the block's own byte 0.",
    ));
    // The resource page sets `more` when subjects exist, so its flags word carries it.
    all.last_mut().unwrap().flags = OK_MORE;

    for page_index in 0..4u8 {
        let mut body = Vec::new();
        body.extend_from_slice(&subjects[usize::from(page_index) * 2]);
        body.extend_from_slice(&subjects[usize::from(page_index) * 2 + 1]);
        let last = page_index == 3;
        let mut vector = response(
            &format!("capabilities-subject-page-{page_index}"),
            Opcode::Hello,
            2,
            capabilities(0b0011, Some(STORE), 2, true, 7, ALL_COMMANDS, 8, 1, page_index, 2, 4, &body),
            "Two 20-byte subject entries in ascending (namespace, kind) order; `more` is set on every page but the last.",
        );
        vector.flags = if last { OK } else { OK_MORE };
        all.push(vector);
    }

    all.push(response(
        "capabilities-zero-subject-page",
        Opcode::Hello,
        5,
        capabilities(0b0001, Some(STORE), 3, false, 1, 1 << 10, 0, 1, 0, 0, 0, &[]),
        "A device advertising no subject answers page zero with count zero, total pages zero, and `more` clear.",
    ));
    all.push(response(
        "capabilities-unauthenticated-test-link",
        Opcode::Hello,
        6,
        capabilities(0, None, 3, false, 1, 1 << 15, 0, 0, 0, 0, 1, &resource_limits(0)),
        "Auth state 0 is reachable only on the test link kind; with store-available clear the StoreId field is zero.",
    ));

    // ---- Upload -----------------------------------------------------------------------------
    all.push(bounded(
        request(
            "start-upload-create-trip-minimum",
            Opcode::StartUpload,
            10,
            start_upload(OP_A, 2, 0, 0, 0, 0, 4096, 0x1234_5678, &trip_put()),
            "The 56-byte minimum StartUpload: create mode encodes both identity fields as zero, over an empty Put envelope.",
        ),
        "minimum",
    ));
    all.push(request(
        "start-upload-create-route",
        Opcode::StartUpload,
        11,
        start_upload(OP_A, 1, 0, 1, 0, 0, payload_len, payload_crc, &route_put(2)),
        "A create with resume permitted and a one-field route Put envelope.",
    ));
    all.push(request(
        "start-upload-replace-route-at-revision",
        Opcode::StartUpload,
        12,
        start_upload(OP_B, 1, 1, 1, 9, 41, payload_len, payload_crc, &route_put(4)),
        "Replace carries the repository's exact identity and expected revision.",
    ));
    all.push(request(
        "start-upload-replace-zero-identity",
        Opcode::StartUpload,
        13,
        start_upload(
            OP_B,
            4,
            1,
            0,
            0,
            0,
            900,
            0xABCD,
            &weather_put(42, 480_000_000, 77_000_000, 50_000, 1_700_000_000, 1_700_086_400),
        ),
        "Replace mode with a zero LogicalObjectId and zero expected Revision: zero is a value, never a sentinel.",
    ));
    all.push(bounded(
        request(
            "start-upload-weather-maximum-producible",
            Opcode::StartUpload,
            14,
            start_upload(
                OP_A,
                4,
                1,
                1,
                0,
                88,
                40_960,
                0x5566_7788,
                &weather_put(43, -120_000_000, 1_750_000_000, 100_000, 1_700_000_000, 1_700_090_000),
            ),
            "116 payload bytes: the largest StartUpload any registered schema produces.",
        ),
        "maximum",
    ));
    all.push(request(
        "start-upload-restart-at-zero",
        Opcode::StartUpload,
        16,
        start_upload(OP_A, 1, 0, 0, 0, 0, payload_len, payload_crc, &route_put(0)),
        "Resume byte `0`: discard any durable work and stream from byte zero.",
    ));

    let accept = |name: &str, flags: u16, offset: u64, crc: u32, note: &str| {
        response(
            name,
            Opcode::StartUpload,
            11,
            upload_accepted(0, flags, OP_A, 0x0000_0011, 9, 41, offset, FIXTURE_GRANULE, 1008, crc),
            note,
        )
    };
    all.push(bounded(
        accept(
            "upload-accepted-offset-zero",
            0,
            0,
            0,
            "64 bytes, no durable work: both flags clear, offset zero, prefix CRC zero.",
        ),
        "minimum",
    ));
    all.push(accept(
        "upload-accepted-resumed",
        1,
        u64::from(FIXTURE_GRANULE),
        crc32(&payload[..FIXTURE_GRANULE as usize]),
        "Resumed work: the finalized CRC covers exactly the durable prefix this response reports, and no other \
         span of this object hashes to it.",
    ));
    all.push(accept(
        "upload-accepted-restart-at-zero",
        2,
        0,
        0,
        "Restart-at-zero forces both the durable offset and the finalized prefix CRC to zero.",
    ));
    all.push(response(
        "upload-accepted-already-terminal",
        Opcode::StartUpload,
        11,
        {
            let mut body = vec![1u8, 0, 0, 0];
            body.extend_from_slice(&committed_route_result(OP_A, 9, 42, payload_len, payload_crc));
            body
        },
        "Disposition 1 replays the retained ObjectResult of the same intent; no session is created.",
    ));

    all.push(request(
        "checkpoint-upload-request",
        Opcode::CheckpointUpload,
        20,
        {
            let mut body = zeros(12);
            u32_at(&mut body, 0, 0x0000_0011);
            u64_at(&mut body, 4, u64::from(FIXTURE_GRANULE));
            body
        },
        "The 12-byte checkpoint at exactly one granule.",
    ));
    // §6.2: the offset "is an exact multiple of the checkpoint granule, except at the declared end,
    // where it equals the declared length" — 1,024, 2,048, and the 3,000-byte end. Each carries the
    // finalized CRC of exactly its own prefix, and there is deliberately no clamp: a clamped
    // producer would report three different offsets under one identical CRC.
    for (sequence, offset) in
        [(1u32, u64::from(FIXTURE_GRANULE)), (2, u64::from(FIXTURE_GRANULE) * 2), (3, payload_len)]
    {
        let mut body = zeros(20);
        u32_at(&mut body, 0, 0x0000_0011);
        u64_at(&mut body, 4, offset);
        u32_at(&mut body, 12, crc32(&payload[..offset as usize]));
        u32_at(&mut body, 16, sequence);
        all.push(response(
            &format!("checkpoint-accepted-sequence-{sequence}"),
            Opcode::CheckpointUpload,
            20,
            body,
            "The sequence starts at 1, strictly increases, and is scoped to the work record rather than the session.",
        ));
    }

    all.push(request(
        "finish-upload-request",
        Opcode::FinishUpload,
        21,
        0x0000_0011u32.to_le_bytes().to_vec(),
        "FinishUpload is exactly a SessionId.",
    ));
    all.push(response(
        "finish-upload-object-result-committed",
        Opcode::FinishUpload,
        21,
        committed_route_result(OP_A, 9, 42, payload_len, payload_crc),
        "Publication and terminal success are one durable commit.",
    ));

    // ---- Every ObjectResult outcome ---------------------------------------------------------
    for (outcome, opcode, name, note) in [
        (0u16, Opcode::FinishUpload, "object-result-committed", "Outcome 0: a payload committed as the new head."),
        (
            1,
            Opcode::FinishUpload,
            "object-result-reserved-superseded-weather-decode-only",
            "Outcome 1 is registered, reserved, and never emitted; it exists here as a decode-only row.",
        ),
        (2, Opcode::DeleteObject, "object-result-deleted", "Outcome 2 reports the deleted old head's length and CRC."),
        (
            3,
            Opcode::SetMetadata,
            "object-result-metadata-changed",
            "Outcome 3: catalog metadata changed in one commit.",
        ),
        (
            4,
            Opcode::InstallUpdate,
            "object-result-update-install-requested",
            "Outcome 4 is emitted only after the boot handoff is durable.",
        ),
        (
            5,
            Opcode::AcknowledgeRideImported,
            "object-result-ride-imported",
            "Outcome 5 follows the client durably storing and verifying the download.",
        ),
    ] {
        all.push(response(
            name,
            opcode,
            30,
            result_envelope(1, &object_result(OP_B, STORE, 1, outcome, 9, 43, 3000, 0x1122_3344)),
            note,
        ));
    }

    all.push(response(
        "draft-part-result-sealed",
        Opcode::FinishUpload,
        31,
        result_envelope(2, &draft_part_result(OP_CHILD, STORE, OP_PARENT, PART_REF, 2, 7, part.len() as u64, part_crc)),
        "A sealed part's result: no LogicalObjectId, no GenerationId, and the opaque ref that only sealing mints.",
    ));
    for (disposition, name, note) in [
        (0u8, "abort-result-cancelled", "The target was nonterminal and is now durably Aborted."),
        (1, "abort-result-already-terminal", "The target was already terminal and is unchanged."),
        (
            2,
            "abort-result-already-absent",
            "Returned only when authorization can be established without leaking another principal's target.",
        ),
    ] {
        all.push(response(
            name,
            Opcode::AbortOperation,
            32,
            result_envelope(3, &abort_result(OP_ABORT, STORE, OP_A, disposition)),
            note,
        ));
    }

    // ---- Download ---------------------------------------------------------------------------
    let mut download_request = zeros(28);
    u16_at(&mut download_request, 0, 1);
    u64_at(&mut download_request, 4, 9);
    all.push(request(
        "start-download-request",
        Opcode::StartDownload,
        40,
        download_request.clone(),
        "A download always resolves the current committed head; the revision flag and field are burned.",
    ));
    let mut resumed_download = download_request.clone();
    u16_at(&mut resumed_download, 2, 1 << 1);
    u64_at(&mut resumed_download, 20, 1_048_576);
    all.push(request(
        "start-download-resumed-request",
        Opcode::StartDownload,
        41,
        resumed_download,
        "A nonzero start offset is allowed only when the kind advertises resumable download.",
    ));

    let mut accepted = zeros(60);
    bytes_at(&mut accepted, 0, &STORE);
    u32_at(&mut accepted, 16, 0x0000_0021);
    u64_at(&mut accepted, 20, 9);
    u64_at(&mut accepted, 28, 42);
    u64_at(&mut accepted, 36, payload_len);
    u32_at(&mut accepted, 44, payload_crc);
    u16_at(&mut accepted, 56, 1008);
    all.push(response(
        "download-accepted",
        Opcode::StartDownload,
        40,
        accepted,
        "Resolve and lease happen before this response; the accepted start offset always equals the requested one.",
    ));
    let mut finish_download = zeros(16);
    u32_at(&mut finish_download, 0, 0x0000_0021);
    u64_at(&mut finish_download, 4, payload_len);
    u32_at(&mut finish_download, 12, payload_crc);
    all.push(request(
        "finish-download-request",
        Opcode::FinishDownload,
        42,
        finish_download,
        "Length and CRC include a locally retained prefix when the start offset was nonzero.",
    ));
    all.push(response(
        "finish-download-released",
        Opcode::FinishDownload,
        42,
        vec![],
        "The empty success releases the lease exactly once.",
    ));

    // ---- Session and operation aborts --------------------------------------------------------
    for (reason, name) in [(1u8, "client-cancelled"), (2, "superseded"), (3, "user-requested")] {
        let mut body = zeros(8);
        u32_at(&mut body, 0, 0x0000_0011);
        body[4] = reason;
        all.push(request(
            &format!("abort-session-{name}"),
            Opcode::AbortSession,
            50,
            body,
            "Detaching a resumable upload preserves its durable work; a restart-only upload is durably aborted.",
        ));
    }
    all.push(response(
        "abort-session-detached",
        Opcode::AbortSession,
        50,
        vec![0],
        "Outcome 0: the session was detached.",
    ));
    all.push(response(
        "abort-session-already-terminal",
        Opcode::AbortSession,
        50,
        vec![1],
        "Outcome 1: the operation was already terminal, so there was no session to detach.",
    ));

    let mut abort_operation = zeros(40);
    bytes_at(&mut abort_operation, 0, &OP_ABORT);
    bytes_at(&mut abort_operation, 16, &OP_A);
    abort_operation[32] = 1;
    all.push(request(
        "abort-operation-request",
        Opcode::AbortOperation,
        51,
        abort_operation,
        "The abort command claims its own OperationId in the reserved cancellation/recovery slot.",
    ));

    // ---- Drafts -----------------------------------------------------------------------------
    let mut begin = zeros(52);
    bytes_at(&mut begin, 0, &OP_PARENT);
    u16_at(&mut begin, 16, 6);
    u64_at(&mut begin, 36, manifest.len() as u64);
    u32_at(&mut begin, 44, manifest_crc);
    u16_at(&mut begin, 48, 3);
    all.push(request(
        "begin-draft-create-volume-manifest",
        Opcode::BeginDraft,
        60,
        begin,
        "BeginDraft binds target, expected revision, manifest length and CRC, and the exact child count.",
    ));
    let mut begin_accepted = zeros(32);
    bytes_at(&mut begin_accepted, 4, &OP_PARENT);
    u64_at(&mut begin_accepted, 20, 1);
    u16_at(&mut begin_accepted, 28, 3);
    all.push(response(
        "begin-draft-accepted-open",
        Opcode::BeginDraft,
        60,
        begin_accepted,
        "Disposition 0 is a four-byte prefix plus 28 bytes; the parent stays InProgress and consumes no result slot.",
    ));
    all.push(response(
        "begin-draft-already-terminal",
        Opcode::BeginDraft,
        60,
        {
            let mut body = vec![1u8, 0, 0, 0];
            body.extend_from_slice(&result_envelope(
                1,
                &object_result(OP_PARENT, STORE, 6, 0, 2, 51, manifest.len() as u64, manifest_crc),
            ));
            body
        },
        "A terminal parent replays its ObjectResult behind the same four-byte prefix.",
    ));

    let mut start_part = zeros(64);
    bytes_at(&mut start_part, 0, &OP_CHILD);
    bytes_at(&mut start_part, 16, &OP_PARENT);
    u16_at(&mut start_part, 32, 2);
    u64_at(&mut start_part, 36, 7);
    u64_at(&mut start_part, 44, part.len() as u64);
    u32_at(&mut start_part, 52, part_crc);
    start_part[56] = 1;
    all.push(request(
        "start-draft-part-request",
        Opcode::StartDraftPart,
        61,
        start_part.clone(),
        "(DraftPartKind, part key) is unique within the parent; the child OperationId is distinct from it.",
    ));
    let mut restart_part = start_part;
    restart_part[56] = 0;
    all.push(request(
        "start-draft-part-restart-request",
        Opcode::StartDraftPart,
        62,
        restart_part,
        "Section 6.1's resume table governs a part unchanged, read against the DraftPartKind subject.",
    ));

    let draft_accept = |name: &str, flags: u16, offset: u64, crc: u32, note: &str| {
        let mut body = zeros(72);
        u16_at(&mut body, 2, flags);
        bytes_at(&mut body, 4, &OP_CHILD);
        bytes_at(&mut body, 20, &OP_PARENT);
        u32_at(&mut body, 36, 0x0000_0031);
        u16_at(&mut body, 40, 2);
        u64_at(&mut body, 44, 7);
        u64_at(&mut body, 52, offset);
        u32_at(&mut body, 60, FIXTURE_GRANULE);
        u16_at(&mut body, 64, 1008);
        u32_at(&mut body, 68, crc);
        response(name, Opcode::StartDraftPart, 61, body, note)
    };
    all.push(draft_accept(
        "draft-part-accepted-offset-zero",
        0,
        0,
        0,
        "72 bytes; the accepted response contains no DraftPartRef.",
    ));
    all.push(draft_accept(
        "draft-part-accepted-resumed",
        1,
        32_768,
        crc32(&part[..32_768]),
        "Resumed work reports the last durable checkpoint, and the CRC is that prefix's own.",
    ));
    all.push(draft_accept(
        "draft-part-accepted-restart-at-zero",
        2,
        0,
        0,
        "Restart-at-zero is emitted only after the durable restart record is synchronized.",
    ));

    all.push(request(
        "finalize-draft-request",
        Opcode::FinalizeDraft,
        63,
        OP_PARENT.to_vec(),
        "FinalizeDraft is exactly the parent OperationId; it computes no canonical intent and makes no second claim.",
    ));
    let finalize_accept = |name: &str, flags: u16, offset: u64, crc: u32, note: &str| {
        let mut body = zeros(64);
        u16_at(&mut body, 2, flags);
        bytes_at(&mut body, 4, &OP_PARENT);
        u32_at(&mut body, 20, 0x0000_0041);
        u64_at(&mut body, 24, 2);
        u64_at(&mut body, 32, 50);
        u64_at(&mut body, 40, offset);
        u32_at(&mut body, 48, FIXTURE_GRANULE);
        u16_at(&mut body, 52, 1008);
        u32_at(&mut body, 56, crc);
        response(name, Opcode::FinalizeDraft, 63, body, note)
    };
    all.push(finalize_accept(
        "finalize-accepted-offset-zero",
        0,
        0,
        0,
        "The manifest acceptance is 64 bytes and carries the same flag word at offset 2.",
    ));
    all.push(finalize_accept(
        "finalize-accepted-resumed",
        1,
        128,
        crc32(&manifest[..128]),
        "A resumed manifest stream reports its durable prefix and that prefix's own CRC.",
    ));
    all.push(finalize_accept("finalize-accepted-restart-at-zero", 2, 0, 0, "Restart-at-zero on the manifest stream."));

    // ---- Queries ----------------------------------------------------------------------------
    all.push(request(
        "query-operation-request",
        Opcode::QueryOperation,
        70,
        OP_A.to_vec(),
        "The request is exactly one OperationId.",
    ));
    all.push(response(
        "query-operation-unknown",
        Opcode::QueryOperation,
        70,
        operation_status(0, &[]),
        "Unknown means only that the ID is neither active nor retained; it cannot distinguish never-claimed from evicted.",
    ));
    // §8.1's progress matrix in full: every originating claim, every phase that claim may occupy,
    // and the flag/ID/offset shape the matrix fixes for it. The matrix is normative — "A phase
    // outside its row, a nonzero kind in namespace none, or a nonzero ID/offset where the matrix
    // says zero is an internal state/codec error and MUST NOT be emitted" — so a client that
    // cannot read one of these rows has an interop hole, not a cosmetic gap.
    for row in progress_matrix() {
        all.push(response(
            &row.name,
            Opcode::QueryOperation,
            70,
            operation_status(1, &progress(row.namespace, row.phase, row.flags, row.kind, row.logical_id, row.offset)),
            &row.note,
        ));
    }
    all.push(response(
        "query-operation-committed",
        Opcode::QueryOperation,
        70,
        operation_status(2, &committed_route_result(OP_A, 9, 42, payload_len, payload_crc)),
        "A lost response is unknown delivery, not a failed mutation; the query returns the exact retained result.",
    ));
    all.push(response(
        "query-operation-aborted",
        Opcode::QueryOperation,
        70,
        operation_status(3, &bare_error(14, 1, presence::DURABLE_CLAIM_EXISTS | presence::CLAIM_IS_TERMINAL)),
        "Aborted is a successful query answer carrying the retained text-free body, with both claim-status bits set.",
    ));

    let mut catalog_request = zeros(28);
    u16_at(&mut catalog_request, 0, 1);
    all.push(request(
        "query-catalog-first-page",
        Opcode::QueryCatalog,
        80,
        catalog_request.clone(),
        "With neither flag, both fields are zero and the current first page is requested.",
    ));
    let mut unchanged = catalog_request.clone();
    u16_at(&mut unchanged, 2, 1);
    u64_at(&mut unchanged, 4, 42);
    all.push(request(
        "query-catalog-unchanged-check",
        Opcode::QueryCatalog,
        81,
        unchanged,
        "Expected-revision alone is an incremental unchanged check.",
    ));
    let cursor = catalog_cursor(STORE, 42, 3, 1);
    let mut continued = catalog_request;
    u16_at(&mut continued, 2, 0b11);
    u64_at(&mut continued, 4, 42);
    bytes_at(&mut continued, 12, &cursor);
    all.push(request(
        "query-catalog-cursor-continuation",
        Opcode::QueryCatalog,
        82,
        continued,
        "A cursor requires both flags and an expected revision equal to the cursor revision.",
    ));

    let zero_cursor = [0u8; 16];
    all.push(response(
        "catalog-page-empty",
        Opcode::QueryCatalog,
        80,
        catalog_page(STORE, 1, 0, 42, &zero_cursor, &[]),
        "An empty page is the 44-byte prefix alone.",
    ));
    all.push(response(
        "catalog-page-unchanged",
        Opcode::QueryCatalog,
        81,
        catalog_page(STORE, 1, 0, 42, &zero_cursor, &[]),
        "An exact expected-revision match returns zero entries, a zero cursor, and no `more` even when the catalog is nonempty.",
    ));
    let one_entry = catalog_entry(
        9,
        42,
        payload_len,
        payload_crc,
        &route_catalog("Kaiserstuhl loop", 2, Some(true), Some(1_700_000_000)),
    );
    all.push(response(
        "catalog-page-one-entry",
        Opcode::QueryCatalog,
        80,
        catalog_page(STORE, 1, 1, 42, &zero_cursor, &one_entry),
        "One 36-byte entry prefix plus its catalog projection envelope.",
    ));
    let maximum_metadata = catalog_entry(
        9,
        42,
        payload_len,
        payload_crc,
        &route_catalog("A route display name of exactly 48 bytes long!!!", 5, Some(false), Some(1_700_000_000)),
    );
    let mut continuing = response(
        "catalog-page-maximum-metadata",
        Opcode::QueryCatalog,
        80,
        catalog_page(STORE, 1, 1, 42, &cursor, &maximum_metadata),
        "The largest entry any registered schema produces: a 162-byte payload, with `more` set and a next cursor.",
    );
    continuing.flags = OK_MORE;
    all.push(bounded(continuing, "maximum"));
    // The producible maximum-count page. Five whole ride entries is the most a 512-byte frame
    // carries: the smallest registered catalog projection is ride's 41 bytes, so one entry is 77
    // bytes and 44 + 6 * 77 = 506 already exceeds the 496-byte payload maximum.
    let mut many = Vec::new();
    for index in 0..5u64 {
        many.extend_from_slice(&catalog_entry(
            index + 1,
            50 + index,
            120_000 + index,
            0x0A0B_0C0D,
            &ride_catalog(1_700_000_000 + index as i64 * 3600, 5400, 42_000, index % 2 == 0),
        ));
    }
    let mut max_count = response(
        "catalog-page-maximum-count",
        Opcode::QueryCatalog,
        83,
        catalog_page(STORE, 3, 5, 60, &catalog_cursor(STORE, 60, 5, 3), &many),
        "Five whole ride entries: the most a 512-byte control frame carries, since one ride entry is 77 bytes.",
    );
    max_count.flags = OK_MORE;
    all.push(bounded(max_count, "maximum"));

    // One catalog projection per registered kind, so every schema appears.
    for (kind, name, metadata) in [
        (2u16, "catalog-page-trip-projection", trip_catalog("Alpine crossing", 4)),
        (4, "catalog-page-weather-projection", weather_catalog(42, 1_700_000_000, 1_700_086_400)),
        (6, "catalog-page-volume-manifest-projection", volume_catalog("Baden-Wurttemberg", false, 3)),
        (7, "catalog-page-update-package-projection", update_catalog("1.4.2", 1, [0x7Fu8; 32])),
    ] {
        let entry = catalog_entry(1, 12, 4096, 0x1234, &metadata);
        all.push(response(
            name,
            Opcode::QueryCatalog,
            84,
            catalog_page(STORE, kind, 1, 12, &zero_cursor, &entry),
            "One catalog projection per registered kind.",
        ));
    }

    let mut draft_request = zeros(44);
    bytes_at(&mut draft_request, 0, &OP_PARENT);
    draft_request[18] = 6;
    all.push(request(
        "query-draft-request",
        Opcode::QueryDraft,
        90,
        draft_request,
        "The requested limit is 1 through 6 and the snapshot token is the draft revision.",
    ));
    let mut draft_body = zeros(44);
    bytes_at(&mut draft_body, 0, &OP_PARENT);
    u64_at(&mut draft_body, 16, 5);
    draft_body[40] = 4;
    draft_body[41] = 1;
    for (index, (child, part_ref, kind, key, state, offset, length)) in [
        ([0x01u8; 16], [0u8; 16], 1u16, 1u64, 0u8, 0u64, 4096u64),
        ([0x02u8; 16], [0u8; 16], 2, 7, 1, 32_768, 65_536),
        ([0x03u8; 16], PART_REF, 2, 8, 2, 65_536, 65_536),
        ([0x04u8; 16], [0u8; 16], 3, 2, 3, 0, 1024),
    ]
    .into_iter()
    .enumerate()
    {
        let _ = index;
        draft_body.extend_from_slice(&draft_entry(child, part_ref, kind, key, state, offset, length, 0x2222_3333));
    }
    all.push(response(
        "draft-page-every-state",
        Opcode::QueryDraft,
        90,
        draft_body,
        "Prepared, streaming, sealed, and aborted entries in strict (kind, key) order; the ref is zero unless sealed.",
    ));
    let mut empty_draft = zeros(44);
    bytes_at(&mut empty_draft, 0, &OP_PARENT);
    u64_at(&mut empty_draft, 16, 1);
    all.push(response("draft-page-empty", Opcode::QueryDraft, 90, empty_draft, "A parent with no children yet."));
    let mut continuing_draft = zeros(44);
    bytes_at(&mut continuing_draft, 0, &OP_PARENT);
    u64_at(&mut continuing_draft, 16, 9);
    bytes_at(&mut continuing_draft, 24, &draft_cursor(STORE, OP_PARENT, 9, 6));
    continuing_draft[40] = 6;
    for index in 0..6u64 {
        continuing_draft.extend_from_slice(&draft_entry(
            [0x10 + index as u8; 16],
            [0u8; 16],
            2,
            index + 1,
            1,
            0,
            4096,
            0x4444,
        ));
    }
    let mut continuing_draft = response(
        "draft-page-continuing",
        Opcode::QueryDraft,
        91,
        continuing_draft,
        "The largest draft page is 452 payload bytes and carries a next cursor bound to this store and parent.",
    );
    continuing_draft.flags = OK_MORE;
    all.push(bounded(continuing_draft, "maximum"));

    all.push(request(
        "query-weather-request",
        Opcode::QueryWeatherRequest,
        100,
        vec![],
        "The request payload is empty.",
    ));
    all.push(response(
        "weather-context-pending",
        Opcode::QueryWeatherRequest,
        100,
        weather_context(
            STORE,
            42,
            3,
            false,
            0,
            88,
            0,
            480_000_000,
            77_000_000,
            50_000,
            1_700_000_000,
            1_700_086_400,
            1,
        ),
        "The singleton identity and repository revision remain authoritative with no head.",
    ));
    all.push(response(
        "weather-context-satisfied",
        Opcode::QueryWeatherRequest,
        100,
        weather_context(STORE, 42, 3, true, 0, 89, 42, -335_000_000, -1_750_000_000, 100_000, 1_700_000_000, 1_700_090_000, 2),
        "A satisfied context reports the head's request ID; the singleton identity here is zero, which is an allocated value.",
    ));

    // ---- Direct mutations --------------------------------------------------------------------
    let mut delete = zeros(36);
    bytes_at(&mut delete, 0, &OP_B);
    u16_at(&mut delete, 16, 1);
    u16_at(&mut delete, 18, 1);
    u64_at(&mut delete, 20, 9);
    u64_at(&mut delete, 28, 42);
    all.push(request(
        "delete-object-request",
        Opcode::DeleteObject,
        110,
        delete.clone(),
        "Delete is an idempotent catalog transaction with a mandatory expected revision.",
    ));
    let mut set_metadata = delete.clone();
    set_metadata.extend_from_slice(&route_patch(Some(3), Some(true), Some("Kaiserstuhl loop")));
    all.push(request(
        "set-metadata-route-request",
        Opcode::SetMetadata,
        111,
        set_metadata,
        "Every present field is applied in one catalog commit; metadata never changes through a sidecar.",
    ));
    let mut volume_set = zeros(36);
    bytes_at(&mut volume_set, 0, &OP_A);
    u16_at(&mut volume_set, 16, 6);
    u16_at(&mut volume_set, 18, 1);
    u64_at(&mut volume_set, 20, 2);
    u64_at(&mut volume_set, 28, 51);
    volume_set.extend_from_slice(&volume_patch(true));
    all.push(request(
        "set-metadata-volume-selected-request",
        Opcode::SetMetadata,
        112,
        volume_set,
        "Initial publication derives selected false; selecting the release requires a later compare-and-swap SetMetadata.",
    ));

    let mut install = zeros(32);
    bytes_at(&mut install, 0, &OP_A);
    u64_at(&mut install, 16, 3);
    u64_at(&mut install, 24, 70);
    all.push(request(
        "install-update-request",
        Opcode::InstallUpdate,
        113,
        install,
        "Install requires a VerifiedReady package and a mandatory version-monotonicity check on the device.",
    ));
    let mut acknowledge = zeros(32);
    bytes_at(&mut acknowledge, 0, &OP_B);
    u64_at(&mut acknowledge, 16, 5);
    u64_at(&mut acknowledge, 24, 61);
    all.push(request(
        "acknowledge-ride-imported-request",
        Opcode::AcknowledgeRideImported,
        114,
        acknowledge,
        "Download completion alone never changes import state.",
    ));

    // ---- Device control ----------------------------------------------------------------------
    all.push(request(
        "get-device-status-request",
        Opcode::GetDeviceStatus,
        120,
        vec![],
        "The request payload is empty.",
    ));
    for class in 0..=6u8 {
        let store = if matches!(class, 3 | 4 | 6) { Some(STORE) } else { None };
        let flags = if class == 0 { 0 } else { 1 };
        all.push(response(
            &format!("device-status-mount-class-{class}"),
            Opcode::GetDeviceStatus,
            120,
            device_status(flags, class, store),
            "The 64-byte status, pinned once per mount class; the StoreId field is zero except in classes 3, 4, and 6.",
        ));
    }
    all.push(request("get-config-request", Opcode::GetConfig, 121, vec![], "The request payload is empty."));
    for (name, bytes, note) in [
        ("config-block-full-name", &b"abcdefghijklmnopqrstuvwxyz012345"[..], "A full 32-byte name with no terminator."),
        ("config-block-short-name", &b"OBC"[..], "A short name whose padding is zero."),
        ("config-block-empty-name", &b""[..], "A zero length means the device advertises its factory default name."),
    ] {
        all.push(response(&format!("{name}-response"), Opcode::GetConfig, 121, config_block(0b101, 2, bytes), note));
        all.push(request(&format!("set-{name}-request"), Opcode::SetConfig, 122, config_block(0b101, 2, bytes), note));
    }
    all.push(response(
        "set-config-response",
        Opcode::SetConfig,
        122,
        config_block(0b101, 2, b"OBC"),
        "SetConfig persists the block before it responds, and the response is the block as it now stands.",
    ));
    for (source, name) in [(1u8, "companion"), (2, "gps")] {
        all.push(request(
            &format!("set-clock-{name}-request"),
            Opcode::SetClock,
            123,
            set_clock(1_763_000_000, source),
            "The 16-byte request: epoch seconds and the offered source.",
        ));
        all.push(response(
            &format!("clock-status-{name}-trusted"),
            Opcode::SetClock,
            123,
            clock_status(1_763_000_000, source, 1),
            "The response reports the clock after the request and the source the device now trusts.",
        ));
    }
    all.push(response(
        "clock-status-untrusted",
        Opcode::SetClock,
        124,
        clock_status(0, 2, 0),
        "No set of any source is refused while the clock is still untrusted.",
    ));
    for (scope, name) in [(1u8, "this-bond"), (2, "every-bond")] {
        let mut body = zeros(8);
        body[0] = scope;
        all.push(request(
            &format!("forget-bond-{name}-request"),
            Opcode::ForgetBond,
            125,
            body,
            "The 8-byte BLE-only request.",
        ));
    }
    all.push(response(
        "forget-bond-response",
        Opcode::ForgetBond,
        125,
        vec![],
        "The response payload is empty; the link then drops.",
    ));
    for (name, len, boundary) in
        [("echo-empty", 0usize, "minimum"), ("echo-one-byte", 1, ""), ("echo-negotiated-maximum", 228, "maximum")]
    {
        let payload: Vec<u8> = (0..len).map(|index| (index % 256) as u8).collect();
        all.push(bounded(
            request(
                &format!("{name}-request"),
                Opcode::Echo,
                126,
                payload.clone(),
                "Echo's maximum is the negotiated control frame less the 16-byte header.",
            ),
            boundary,
        ));
        all.push(bounded(
            response(
                &format!("{name}-response"),
                Opcode::Echo,
                126,
                payload,
                "The response payload is those bytes byte-identical.",
            ),
            boundary,
        ));
    }
    all.push(request(
        "reset-store-request",
        Opcode::ResetStore,
        127,
        STORE.to_vec(),
        "The echo is the confirmation, checked before anything is deleted.",
    ));
    all.push(response(
        "reset-store-result",
        Opcode::ResetStore,
        127,
        STORE_B.to_vec(),
        "The new StoreId is returned only after the first checkpoint gate of the new store is durable.",
    ));

    // ---- Error responses ---------------------------------------------------------------------
    all.extend(error_vectors());
    all
}

/// Every error-response vector: one per category, plus the presence, owner, text, and claim-status
/// rows `Device_Object_Vectors_v2.md` §4 asks for.
fn error_vectors() -> Vec<ControlVector> {
    let mut all = Vec::new();
    let mut push = |name: &str, opcode: Opcode, payload: Vec<u8>, note: &str| {
        all.push(control(name, "response", opcode, ERR, 200, payload, note));
    };

    // One vector per category, each with its permitted guidance and required presence.
    push(
        "error-incompatible-version",
        Opcode::Hello,
        error_body(
            1,
            0,
            detail::version::UNSUPPORTED_MAJOR,
            RetryGuidance::RETRY_AFTER_USER_ACTION.get(),
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            b"wire major 2 is not served",
        ),
        "A legacy peer that reaches a parseable frame gets an explicit incompatible-version result.",
    );
    push(
        "error-unsupported-capability-opcode",
        Opcode::ForgetBond,
        error_body(2, 0, detail::capability::OPCODE, RetryGuidance::REJECT_PERMANENTLY.get(), 0, 0, 0, 0, 0, 0, 0, &[]),
        "ForgetBond on a non-BLE link: the device clears command-flag bit 14 and answers unsupportedCapability/opcode.",
    );
    push(
        "error-unsupported-capability-non-cancellable",
        Opcode::AbortOperation,
        error_body(
            2,
            0,
            detail::capability::NON_CANCELLABLE_OPERATION,
            RetryGuidance::REJECT_PERMANENTLY.get(),
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            &[],
        ),
        "AbortOperation naming an InstallUpdate target, refused in preflight with both claim-status bits clear.",
    );
    push(
        "error-authentication-failed",
        Opcode::QueryCatalog,
        error_body(
            3,
            0,
            detail::authentication::MISSING_CREDENTIAL,
            RetryGuidance::RETRY_AFTER_USER_ACTION.get(),
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            &[],
        ),
        "Authentication precedes object-existence, revision, operation-status, and busy facts.",
    );
    push(
        "error-authorization-failed-operation-owner",
        Opcode::QueryOperation,
        error_body(
            4,
            0,
            detail::authorization::OPERATION_OWNER,
            RetryGuidance::RETRY_AFTER_USER_ACTION.get(),
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            &[],
        ),
        "A different principal receives authorizationFailed, not status and not operationIdConflict.",
    );
    push(
        "error-authorization-failed-device-control",
        Opcode::SetConfig,
        error_body(
            4,
            0,
            detail::authorization::DEVICE_CONTROL,
            RetryGuidance::RETRY_AFTER_USER_ACTION.get(),
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            &[],
        ),
        "A principal that may not use a supported device-control operation.",
    );
    // The owner byte at every one of its six values.
    for (owner, name, note) in [
        (Owner::NONE, "error-busy-owner-none", "Owner none, on a refusal that reports no owner class."),
        (Owner::BLE, "error-busy-owner-ble", "Owner 1 is exactly the BLE link kind."),
        (Owner::USB, "error-busy-owner-usb", "Owner 2 is exactly the USB link kind."),
        (Owner::TEST, "error-busy-owner-test", "Owner 3 is exactly the test link kind."),
        (
            Owner::LOCAL_PRODUCER,
            "error-busy-owner-local-producer",
            "A device-local producer holding the slot against a link client: owner 4 has no link-kind meaning.",
        ),
        (Owner::MAINTENANCE, "error-busy-owner-maintenance", "Owner 5 is the reserved cancellation/recovery claim."),
    ] {
        push(
            name,
            Opcode::StartUpload,
            error_body(
                5,
                0,
                detail::busy::HEAVY_TRANSFER,
                RetryGuidance::RETRY_AFTER_OWNER_RELEASE.get(),
                owner.get(),
                0,
                0,
                0,
                0,
                0,
                0,
                &[],
            ),
            note,
        );
    }
    push(
        "error-busy-retry-after-delay",
        Opcode::StartUpload,
        error_body(5, 0, detail::busy::NORMAL_OPERATION_CLAIMS, RetryGuidance::RETRY_AFTER_DELAY.get(), Owner::USB.get(), presence::RETRY_DELAY, 2500, 0, 0, 0, 0, &[]),
        "Busy with retry-delay guidance carries a delay; a second request while one is outstanding is answered this way.",
    );
    push(
        "error-busy-draft-parents",
        Opcode::BeginDraft,
        error_body(
            5,
            0,
            detail::busy::DRAFT_PARENTS,
            RetryGuidance::RETRY_AFTER_OWNER_RELEASE.get(),
            Owner::LOCAL_PRODUCER.get(),
            0,
            0,
            0,
            0,
            0,
            0,
            &[],
        ),
        "A second BeginDraft while a parent is open is an ownership refusal, not a compiled-capacity failure.",
    );
    push(
        "error-busy-ride-slot",
        Opcode::StartUpload,
        error_body(
            5,
            0,
            detail::busy::RIDE_SLOT,
            RetryGuidance::RETRY_AFTER_OWNER_RELEASE.get(),
            Owner::LOCAL_PRODUCER.get(),
            0,
            0,
            0,
            0,
            0,
            0,
            &[],
        ),
        "An occupied ride slot is an owner, not a capacity the client can plan around.",
    );
    push(
        "error-busy-maintenance-claim",
        Opcode::AbortOperation,
        error_body(5, 0, detail::busy::MAINTENANCE_CANCELLATION_RECOVERY_CLAIM, RetryGuidance::RETRY_AFTER_OWNER_RELEASE.get(), Owner::MAINTENANCE.get(), 0, 0, 0, 0, 0, 0, &[]),
        "When the reserved slot is occupied, a different new AbortOperation is refused rather than partially cancelling.",
    );
    push(
        "error-invalid-frame-truncated",
        Opcode::StartUpload,
        error_body(6, 0, detail::frame::TRUNCATED, RetryGuidance::RECONNECT_THEN_QUERY.get(), 0, 0, 0, 0, 0, 0, 0, &[]),
        "invalidFrame means the record cannot be established as one complete frame.",
    );
    push(
        "error-invalid-descriptor-reserved-bits",
        Opcode::StartDownload,
        error_body(
            7,
            0,
            detail::descriptor::RESERVED_BITS,
            RetryGuidance::REJECT_PERMANENTLY.get(),
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            &[],
        ),
        "invalidDescriptor means a complete frame carries an illegal field value or reserved bit.",
    );
    push(
        "error-invalid-descriptor-empty-metadata-patch",
        Opcode::SetMetadata,
        error_body(
            7,
            0,
            detail::descriptor::EMPTY_METADATA_PATCH,
            RetryGuidance::REJECT_PERMANENTLY.get(),
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            &[],
        ),
        "A well-formed zero-field patch envelope is refused as a request, not as a codec error.",
    );
    push(
        "error-invalid-offset-live-claim",
        Opcode::CheckpointUpload,
        error_body(
            8,
            0,
            detail::offset::UNEXPECTED_OFFSET,
            RetryGuidance::RESUME_AT_EXPECTED_OFFSET.get(),
            0,
            presence::EXPECTED_OFFSET | presence::DURABLE_CLAIM_EXISTS,
            0,
            262_144,
            0,
            0,
            0,
            &[],
        ),
        "An error against a live claimed operation sets bit 5 and clears bit 6.",
    );
    push(
        "error-invalid-session",
        Opcode::FinishUpload,
        error_body(
            9,
            0,
            detail::session::STALE_CONNECTION,
            RetryGuidance::RECONNECT_THEN_QUERY.get(),
            0,
            presence::DURABLE_CLAIM_EXISTS,
            0,
            0,
            0,
            0,
            0,
            &[],
        ),
        "A reconnect makes every earlier SessionId stale even for the same principal.",
    );
    push(
        "error-object-not-found",
        Opcode::StartDownload,
        error_body(10, 0, detail::not_found::LOGICAL_OBJECT, RetryGuidance::REFRESH.get(), 0, 0, 0, 0, 0, 0, 0, &[]),
        "No existence detail beyond the authorized target.",
    );
    push(
        "error-object-not-found-draft-parent-unknown",
        Opcode::QueryDraft,
        error_body(
            10,
            0,
            detail::not_found::DRAFT_PARENT_UNKNOWN,
            RetryGuidance::REJECT_PERMANENTLY.get(),
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            &[],
        ),
        "Emitted only after principal ownership is authorized.",
    );
    push(
        "error-object-not-found-operation-terminal",
        Opcode::QueryDraft,
        error_body(
            10,
            0,
            detail::not_found::OPERATION_TERMINAL,
            RetryGuidance::QUERY_OPERATION_NOW.get(),
            0,
            presence::DURABLE_CLAIM_EXISTS | presence::CLAIM_IS_TERMINAL,
            0,
            0,
            0,
            0,
            0,
            &[],
        ),
        "The terminal commit removes the draft-parent row; the client uses QueryOperation instead.",
    );
    push(
        "error-object-not-found-weather-request-context",
        Opcode::QueryWeatherRequest,
        error_body(
            10,
            0,
            detail::not_found::WEATHER_REQUEST_CONTEXT,
            RetryGuidance::REFRESH.get(),
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            &[],
        ),
        "An authorized query before any context exists does not synthesize a zero WeatherRequestId.",
    );
    push(
        "error-revision-conflict",
        Opcode::StartUpload,
        error_body(
            11,
            0,
            detail::revision::OBJECT,
            RetryGuidance::REFRESH.get(),
            0,
            presence::CURRENT_REVISION,
            0,
            0,
            43,
            0,
            0,
            &[],
        ),
        "A compare-and-swap failure reports the authoritative current revision.",
    );
    push(
        "error-insufficient-space",
        Opcode::StartUpload,
        error_body(
            12,
            0,
            detail::space::RESERVATION_BYTES,
            RetryGuidance::RETRY_AFTER_USER_ACTION.get(),
            0,
            presence::REQUIRED_BYTES | presence::AVAILABLE_BYTES,
            0,
            0,
            0,
            8_388_608,
            1_048_576,
            b"free space on the card",
        ),
        "insufficientSpace reports required and available bytes.",
    );
    push(
        "error-checksum-failure-whole-payload",
        Opcode::FinishUpload,
        error_body(
            13,
            0,
            detail::checksum::WHOLE_PAYLOAD,
            RetryGuidance::RETRY_SAME_REQUEST.get(),
            0,
            presence::DURABLE_CLAIM_EXISTS,
            0,
            0,
            0,
            0,
            0,
            &[],
        ),
        "The declared CRC did not match the sealed bytes.",
    );
    push(
        "error-checksum-failure-durable-prefix",
        Opcode::StartUpload,
        error_body(
            13,
            0,
            detail::checksum::DURABLE_PREFIX,
            RetryGuidance::RETRY_SAME_REQUEST.get(),
            0,
            presence::EXPECTED_OFFSET | presence::DURABLE_CLAIM_EXISTS,
            0,
            262_144,
            0,
            0,
            0,
            &[],
        ),
        "A prefix mismatch uses the expected offset; concatenation onto an unverified prefix is never allowed.",
    );
    push(
        "error-semantic-validation-weather-request-mismatch",
        Opcode::FinishUpload,
        error_body(
            14,
            4,
            5,
            RetryGuidance::REJECT_PERMANENTLY.get(),
            0,
            presence::DURABLE_CLAIM_EXISTS | presence::CLAIM_IS_TERMINAL,
            0,
            0,
            0,
            0,
            0,
            &[],
        ),
        "A bundle whose request context is no longer current aborts terminally in the weather namespace.",
    );
    push(
        "error-semantic-validation-downgrade-denied",
        Opcode::InstallUpdate,
        error_body(
            14,
            7,
            4,
            RetryGuidance::REJECT_PERMANENTLY.get(),
            0,
            presence::DURABLE_CLAIM_EXISTS | presence::CLAIM_IS_TERMINAL,
            0,
            0,
            0,
            0,
            0,
            &[],
        ),
        "Version monotonicity is a mandatory admission check, not a host courtesy.",
    );
    push(
        "error-semantic-validation-unsafe-runtime-state",
        Opcode::InstallUpdate,
        error_body(14, 7, 7, RetryGuidance::RETRY_AFTER_DELAY.get(), 0, presence::RETRY_DELAY, 60_000, 0, 0, 0, 0, &[]),
        "A retryable domain precondition does not terminally claim the OperationId: both claim-status bits are clear.",
    );
    push(
        "error-semantic-validation-draft-incomplete",
        Opcode::FinalizeDraft,
        error_body(
            14,
            6,
            7,
            RetryGuidance::RETRY_SAME_REQUEST.get(),
            0,
            presence::DURABLE_CLAIM_EXISTS,
            0,
            0,
            0,
            0,
            0,
            &[],
        ),
        "Missing or nonsealed children return a retryable domain-state error and create no session.",
    );
    push(
        "error-semantic-validation-clock-regression",
        Opcode::SetClock,
        error_body(
            14,
            0,
            detail::semantic_common::CLOCK_REGRESSION,
            RetryGuidance::REJECT_PERMANENTLY.get(),
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            &[],
        ),
        "The device-control plane owns no ObjectKind, so its one semantic refusal uses namespace zero.",
    );
    push(
        "error-media-unavailable-no-card",
        Opcode::ResetStore,
        error_body(
            15,
            0,
            detail::media::NO_CARD,
            RetryGuidance::RETRY_AFTER_USER_ACTION.get(),
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            &[],
        ),
        "ResetStore is the one device-control member that needs the medium.",
    );
    push(
        "error-media-unavailable-unmounted",
        Opcode::ResetStore,
        error_body(
            15,
            0,
            detail::media::UNMOUNTED,
            RetryGuidance::RETRY_AFTER_USER_ACTION.get(),
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            &[],
        ),
        "The device never formats a volume whose geometry or filesystem it does not accept.",
    );
    push(
        "error-media-io-uncertain-commit",
        Opcode::FinishUpload,
        error_body(
            16,
            0,
            detail::media_io::UNCERTAIN_COMMIT,
            RetryGuidance::QUERY_OPERATION_NOW.get(),
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            &[],
        ),
        "\"may have been claimed\" is not \"claimed\": both claim-status bits are clear and the guidance is query now.",
    );
    push(
        "error-cancelled",
        Opcode::FinishUpload,
        error_body(
            17,
            0,
            detail::cancelled::WORK_EXPIRED,
            RetryGuidance::REJECT_PERMANENTLY.get(),
            0,
            presence::DURABLE_CLAIM_EXISTS | presence::CLAIM_IS_TERMINAL,
            0,
            0,
            0,
            0,
            0,
            &[],
        ),
        "Every error that reports a terminal outcome sets both claim-status bits.",
    );
    push(
        "error-link-lost",
        Opcode::StartUpload,
        error_body(
            18,
            0,
            detail::link::STREAM,
            RetryGuidance::RECONNECT_THEN_QUERY.get(),
            0,
            presence::DURABLE_CLAIM_EXISTS,
            0,
            0,
            0,
            0,
            0,
            &[],
        ),
        "A lost response is unknown delivery; the client queries its OperationId after reconnecting.",
    );
    push(
        "error-operation-id-conflict",
        Opcode::StartUpload,
        error_body(
            19,
            0,
            detail::conflict::INTENT_DIGEST,
            RetryGuidance::NEW_ID_FOR_NEW_INTENT.get(),
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            &[],
        ),
        "operationIdConflict clears both claim-status bits: the request's own intent was never claimed.",
    );
    push(
        "error-resource-limit-minimum-control-frame",
        Opcode::Hello,
        error_body(
            20,
            0,
            detail::resource::MINIMUM_CONTROL_FRAME,
            RetryGuidance::RETRY_AFTER_USER_ACTION.get(),
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            &[],
        ),
        "A transport ceiling below the 192-byte minimum fails Hello and admits nothing on that connection.",
    );
    push(
        "error-resource-limit-minimum-stream-frame",
        Opcode::Hello,
        error_body(
            20,
            0,
            detail::resource::MINIMUM_STREAM_FRAME,
            RetryGuidance::RETRY_AFTER_USER_ACTION.get(),
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            &[],
        ),
        "An SDU below the 64-byte floor refuses the channel at CoC establishment.",
    );
    push(
        "error-resource-limit-object-length",
        Opcode::StartUpload,
        error_body(
            20,
            0,
            detail::resource::OBJECT_LENGTH,
            RetryGuidance::RETRY_AFTER_USER_ACTION.get(),
            0,
            presence::REQUIRED_BYTES | presence::AVAILABLE_BYTES,
            0,
            0,
            0,
            5_000_000_000,
            4_294_967_295,
            &[],
        ),
        "A kind's advertised maximum object length remains authoritative even when a transport could carry more.",
    );
    push(
        "error-catalog-changed",
        Opcode::QueryCatalog,
        error_body(
            21,
            0,
            detail::changed::CATALOG_SNAPSHOT,
            RetryGuidance::REFRESH.get(),
            0,
            presence::CURRENT_REVISION,
            0,
            0,
            61,
            0,
            0,
            &[],
        ),
        "A revision mismatch is catalogChanged with the current revision and refresh guidance.",
    );
    push(
        "error-catalog-changed-draft-snapshot",
        Opcode::QueryDraft,
        error_body(
            21,
            0,
            detail::changed::DRAFT_SNAPSHOT,
            RetryGuidance::REFRESH.get(),
            0,
            presence::CURRENT_REVISION,
            0,
            0,
            10,
            0,
            0,
            &[],
        ),
        "Pagination is snapshot-bound to the draft revision and rejects a changed one instead of mixing child sets.",
    );
    push(
        "error-internal",
        Opcode::FinishUpload,
        error_body(
            22,
            0,
            detail::internal::INVARIANT,
            RetryGuidance::RETRY_AFTER_DELAY.get(),
            0,
            presence::RETRY_DELAY,
            1000,
            0,
            0,
            0,
            0,
            &[],
        ),
        "A stable category detail is reported when known.",
    );

    // The nine reserved details, as decode-only rows no v3.0 device emits.
    for (category, detail, name) in [
        (12u16, detail::space::RETAINED_PREVIOUS, "reserved-detail-insufficient-space-retained-previous"),
        (5, detail::busy::DRAFT_PARTS, "reserved-detail-busy-draft-parts"),
        (20, detail::resource::DRAFT_PARENTS, "reserved-detail-resource-limit-draft-parents"),
        (5, detail::busy::RETAINED_PREVIOUS, "reserved-detail-busy-retained-previous"),
        (20, detail::resource::RIDE_SLOT, "reserved-detail-resource-limit-ride-slot"),
        (5, detail::busy::MAINTENANCE, "reserved-detail-busy-maintenance"),
        (21, detail::changed::CAPABILITY_SNAPSHOT, "reserved-detail-catalog-changed-capability-snapshot"),
        (10, detail::not_found::REQUESTED_REVISION, "reserved-detail-object-not-found-requested-revision"),
        (10, detail::not_found::RESUMABLE_WORK, "reserved-detail-object-not-found-resumable-work"),
    ] {
        push(
            name,
            Opcode::StartUpload,
            error_body(category, 0, detail, 0, 0, 0, 0, 0, 0, 0, 0, &[]),
            "Registered so its number stays burned; reserved and never emitted in v3.0. Decode-only.",
        );
    }

    // Retained-Aborted replays for the four categories whose live form has required presence.
    for (category, detail, name) in [
        (5u16, detail::busy::HEAVY_TRANSFER, "retained-aborted-replay-busy"),
        (8, detail::offset::UNEXPECTED_OFFSET, "retained-aborted-replay-invalid-offset"),
        (12, detail::space::RESERVATION_BYTES, "retained-aborted-replay-insufficient-space"),
        (15, detail::media::NO_CARD, "retained-aborted-replay-media-unavailable"),
    ] {
        push(
            name,
            Opcode::StartUpload,
            bare_error(category, detail, presence::DURABLE_CLAIM_EXISTS | presence::CLAIM_IS_TERMINAL),
            "A replayed terminal body is exempt from the presence matrix: owner none, guidance forced to reject-permanently, both claim-status bits set.",
        );
    }

    // Diagnostic text boundaries, including the one that inverts the naive expectation.
    push(
        "error-text-empty",
        Opcode::FinishUpload,
        error_body(22, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0, &[]),
        "Empty text is the ordinary case; text is optional and never drives behaviour.",
    );
    push(
        "error-text-exactly-64-bytes",
        Opcode::FinishUpload,
        error_body(
            22,
            0,
            2,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            "sixty-four bytes of supplementary diagnostic text, exactly!"
                .as_bytes()
                .iter()
                .copied()
                .chain(b"12345".iter().copied())
                .collect::<Vec<u8>>()
                .as_slice(),
        ),
        "The maximum text-bearing ErrorBody is 112 payload bytes.",
    );
    push(
        "error-text-invalid-utf8-must-still-decode",
        Opcode::FinishUpload,
        error_body(22, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0, &[0xff, 0xfe, 0xfd, 0x80]),
        "A receiver MUST NOT reject a frame because its diagnostic text is malformed; it renders it lossily.",
    );

    // A completeness sweep. §4 requires "at least one vector" freezing every error category and
    // every allowed category/detail/retry combination, so each registered row the hand-written
    // vectors above do not already reach gets one here, at the guidance and presence its category
    // requires.
    let covered: Vec<(u16, u16)> = all
        .iter()
        .map(|vector| {
            (
                u16::from_le_bytes([vector.payload[0], vector.payload[1]]),
                u16::from_le_bytes([vector.payload[4], vector.payload[5]]),
            )
        })
        .collect();
    let mut sweep = Vec::new();
    for row in crate::error::detail_registry() {
        if row.code == 0 || covered.contains(&(row.category.get(), row.code)) {
            continue;
        }
        if row.category == ErrorCategory::INVALID_DESCRIPTOR && row.code == detail::descriptor::ZERO_REQUEST_ID {
            // §2: `invalidDescriptor/zeroRequestId` "is the recorded and logged reason for that
            // close; it is never transmitted". Freezing it as a response frame would freeze a
            // frame no conforming device can send. The behaviour is pinned instead by
            // `negative/frame-zero-request-id`, which is where it actually lives.
            continue;
        }
        let shape = category_defaults(row.category.get());
        sweep.push(control(
            &format!("error-{}-{}", row.category.name(), row.name),
            "response",
            Opcode::StartUpload,
            ERR,
            201,
            error_body(
                row.category.get(),
                0,
                row.code,
                shape.guidance,
                shape.owner,
                shape.presence,
                shape.retry_after_ms,
                shape.expected_offset,
                shape.current_revision,
                shape.required_bytes,
                shape.available_bytes,
                &[],
            ),
            "One vector per registered category and detail, at the guidance and presence its category requires.",
        ));
    }
    all.extend(sweep);

    all
}

/// The guidance and presence §12's matrix requires of a live response in each category.
struct ErrorShape {
    guidance: u8,
    owner: u8,
    presence: u16,
    retry_after_ms: u32,
    expected_offset: u64,
    current_revision: u64,
    required_bytes: u64,
    available_bytes: u64,
}

fn category_defaults(category: u16) -> ErrorShape {
    let bare = |guidance: RetryGuidance| ErrorShape {
        guidance: guidance.get(),
        owner: 0,
        presence: 0,
        retry_after_ms: 0,
        expected_offset: 0,
        current_revision: 0,
        required_bytes: 0,
        available_bytes: 0,
    };
    match category {
        1 | 3 | 4 | 15 | 20 | 22 => bare(RetryGuidance::RETRY_AFTER_USER_ACTION),
        2 | 7 | 10 | 17 => bare(RetryGuidance::REJECT_PERMANENTLY),
        5 => ErrorShape {
            guidance: RetryGuidance::RETRY_AFTER_OWNER_RELEASE.get(),
            owner: Owner::USB.get(),
            ..bare(RetryGuidance::RETRY_AFTER_OWNER_RELEASE)
        },
        6 => bare(RetryGuidance::RECONNECT_THEN_QUERY),
        8 => ErrorShape {
            presence: presence::EXPECTED_OFFSET,
            expected_offset: 262_144,
            ..bare(RetryGuidance::RESUME_AT_EXPECTED_OFFSET)
        },
        9 | 16 | 18 => bare(RetryGuidance::RECONNECT_THEN_QUERY),
        11 | 21 => {
            ErrorShape { presence: presence::CURRENT_REVISION, current_revision: 51, ..bare(RetryGuidance::REFRESH) }
        }
        12 => ErrorShape {
            presence: presence::REQUIRED_BYTES | presence::AVAILABLE_BYTES,
            required_bytes: 1_048_576,
            available_bytes: 4096,
            ..bare(RetryGuidance::RETRY_AFTER_USER_ACTION)
        },
        13 => bare(RetryGuidance::RETRY_SAME_REQUEST),
        14 => bare(RetryGuidance::REJECT_PERMANENTLY),
        19 => bare(RetryGuidance::NEW_ID_FOR_NEW_INTENT),
        _ => bare(RetryGuidance::REJECT_PERMANENTLY),
    }
}

// ---------------------------------------------------------------------------------------------
// Canonical-intent goldens.
// ---------------------------------------------------------------------------------------------

/// One row of §8.1's progress matrix.
pub struct ProgressRow {
    /// Stable fixture name.
    pub name: String,
    /// Subject namespace: none `0`, logical `1`, draft part `2`.
    pub namespace: u8,
    /// Subject kind code, zero in namespace none.
    pub kind: u16,
    /// The phase.
    pub phase: u8,
    /// Resumable / attached / ID-present.
    pub flags: u8,
    /// The assigned LogicalObjectId, zero when ID-present is clear.
    pub logical_id: u64,
    /// The durable offset the matrix fixes for this row.
    pub offset: u64,
    /// The rule this row pins.
    pub note: String,
}

const RESUMABLE: u8 = 1;
const ATTACHED: u8 = 1 << 1;
const ID_PRESENT: u8 = 1 << 2;

/// §8.1's progress matrix, expanded to one row per `(originating claim, phase)` the matrix admits.
///
/// The eight claim families and the phases each may occupy come straight from the matrix table.
/// Two extra rows exercise the flag variants the matrix leaves to policy rather than fixing: a
/// claim whose kind advertises no resume, and resumable work whose session has been detached.
pub fn progress_matrix() -> Vec<ProgressRow> {
    let route_length = route_payload().len() as u64;
    let part_length = draft_part_payload().len() as u64;
    let manifest_length = manifest_payload().len() as u64;
    let granule = u64::from(FIXTURE_GRANULE);
    let phases = [(0u8, "prepared"), (1, "streaming"), (2, "sealed"), (3, "validating"), (4, "publishing")];
    let mut rows = Vec::new();
    let mut push =
        |name: &str, namespace: u8, kind: u16, phase: u8, flags: u8, logical_id: u64, offset: u64, note: &str| {
            rows.push(ProgressRow {
                name: format!("query-operation-progress-{name}"),
                namespace,
                kind,
                phase,
                flags,
                logical_id,
                offset,
                note: note.to_string(),
            });
        };

    // StartUpload `0x0100`: logical namespace, phases 0..4 and aborting 7; ID-present set; the
    // offset is the durable payload prefix, and the declared length in phases 2..4.
    for (phase, label) in phases {
        let offset = if phase >= 2 {
            route_length
        } else if phase == 0 {
            0
        } else {
            granule
        };
        let flags = RESUMABLE | ID_PRESENT | if phase == 1 { ATTACHED } else { 0 };
        push(
            &format!("start-upload-{label}"),
            1,
            1,
            phase,
            flags,
            9,
            offset,
            "StartUpload: ID-present set, the offset is the durable payload prefix and the declared length in \
             phases 2..4, and a session is attached only while one exists.",
        );
    }
    push("start-upload-aborting", 1, 1, 7, RESUMABLE | ID_PRESENT, 9, granule, "Aborting has no attachment.");
    push(
        "start-upload-prepared-not-resumable",
        1,
        1,
        0,
        ID_PRESENT,
        9,
        0,
        "The resumable bit reflects the claimed policy, so a kind that advertises no resumable upload clears it.",
    );

    // StartDraftPart `0x0131`: draft-part namespace, ID-present clear and the ID zero throughout.
    for (phase, label) in phases {
        let offset = if phase >= 2 {
            part_length
        } else if phase == 0 {
            0
        } else {
            32_768
        };
        let flags = RESUMABLE | if phase == 1 { ATTACHED } else { 0 };
        push(
            &format!("draft-part-{label}"),
            2,
            2,
            phase,
            flags,
            0,
            offset,
            "A draft part reports ID-present clear and its ID zero; the offset is the durable part prefix.",
        );
    }
    push("draft-part-aborting", 2, 2, 7, RESUMABLE, 0, 32_768, "Aborting has no attachment.");
    push(
        "draft-part-streaming-detached",
        2,
        1,
        1,
        RESUMABLE,
        0,
        4096,
        "Resumable work whose session was detached: attachment is advisory and grants no ownership.",
    );

    // BeginDraft / FinalizeDraft parent `0x0130`: draft-open 6, the manifest phases, aborting 7.
    push("draft-parent-draft-open", 1, 6, 6, ID_PRESENT, 2, 0, "Draft-open has offset zero and no attached session.");
    for (phase, label) in phases {
        let offset = if phase >= 2 {
            manifest_length
        } else if phase == 0 {
            0
        } else {
            128
        };
        let flags = RESUMABLE | ID_PRESENT | if phase == 1 { ATTACHED } else { 0 };
        push(
            &format!("draft-parent-manifest-{label}"),
            1,
            6,
            phase,
            flags,
            2,
            offset,
            "The parent's manifest phases use resumable/attached and the durable manifest offset.",
        );
    }
    push("draft-parent-aborting", 1, 6, 7, ID_PRESENT, 2, 0, "Aborting has offset zero and no attached session.");

    // DeleteObject `0x0300` and SetMetadata `0x0301`: phases 3, 4 and aborting 7, only ID-present
    // set, offset zero.
    for opcode_label in ["delete", "set-metadata"] {
        for (phase, label) in [(3u8, "validating"), (4, "publishing"), (7, "aborting")] {
            push(
                &format!("{opcode_label}-{label}"),
                1,
                1,
                phase,
                ID_PRESENT,
                9,
                0,
                "A direct mutation reports only ID-present, its target, and offset zero.",
            );
        }
    }

    // AbortOperation `0x0302`: namespace none, kind zero, aborting only, everything else zero.
    push(
        "abort-command-aborting",
        0,
        0,
        7,
        0,
        0,
        0,
        "An AbortOperation command has namespace none, kind zero, and flags, ID, and offset all zero.",
    );

    // InstallUpdate `0x0310`: phases 3, 4 and external-handoff 5 only — it never enters aborting.
    for (phase, label) in [(3u8, "validating"), (4, "publishing"), (5, "external-handoff")] {
        push(
            &format!("install-update-{label}"),
            1,
            7,
            phase,
            ID_PRESENT,
            3,
            0,
            "InstallUpdate occupies phases 3, 4, and 5 only and never enters aborting.",
        );
    }

    // AcknowledgeRideImported `0x0311`: phases 3, 4 and aborting 7.
    for (phase, label) in [(3u8, "validating"), (4, "publishing"), (7, "aborting")] {
        push(
            &format!("acknowledge-ride-{label}"),
            1,
            3,
            phase,
            ID_PRESENT,
            5,
            0,
            "AcknowledgeRideImported names the ride it acknowledges.",
        );
    }

    rows
}

/// Every canonical-intent golden: one per row of §11's suffix table.
pub fn intents() -> Vec<IntentVector> {
    let payload = route_payload();
    let payload_crc = crc32(&payload);
    let payload_len = payload.len() as u64;
    let mut all = Vec::new();
    let mut push = |name: &str, opcode: Opcode, suffix: Vec<u8>, note: &str| {
        all.push(IntentVector {
            name: format!("intent-{name}"),
            opcode,
            bytes: canonical_intent(STORE, opcode.to_u16(), &suffix),
            note: note.to_string(),
        });
    };

    let mut start_upload_suffix = zeros(34);
    u16_at(&mut start_upload_suffix, 0, 1);
    u64_at(&mut start_upload_suffix, 4, 0);
    u64_at(&mut start_upload_suffix, 12, 0);
    u64_at(&mut start_upload_suffix, 20, payload_len);
    u32_at(&mut start_upload_suffix, 28, payload_crc);
    let route_envelope = route_put(2);
    u16_at(&mut start_upload_suffix, 32, route_envelope.len() as u16);
    start_upload_suffix.extend_from_slice(&route_envelope);
    push(
        "start-upload-create-route",
        Opcode::StartUpload,
        start_upload_suffix,
        "Inactive target fields are included as their required zero bytes, so there is one encoding per intent.",
    );

    let mut replace_suffix = zeros(34);
    u16_at(&mut replace_suffix, 0, 1);
    replace_suffix[2] = 1;
    u64_at(&mut replace_suffix, 4, 9);
    u64_at(&mut replace_suffix, 12, 41);
    u64_at(&mut replace_suffix, 20, payload_len);
    u32_at(&mut replace_suffix, 28, payload_crc);
    let replace_envelope = route_put(4);
    u16_at(&mut replace_suffix, 32, replace_envelope.len() as u16);
    replace_suffix.extend_from_slice(&replace_envelope);
    push(
        "start-upload-replace-route",
        Opcode::StartUpload,
        replace_suffix,
        "Replace mode carries the exact compare-and-swap token.",
    );

    let mut begin_suffix = zeros(36);
    u16_at(&mut begin_suffix, 0, 6);
    u64_at(&mut begin_suffix, 20, manifest_payload().len() as u64);
    u32_at(&mut begin_suffix, 28, crc32(&manifest_payload()));
    u16_at(&mut begin_suffix, 32, 3);
    push(
        "begin-draft",
        Opcode::BeginDraft,
        begin_suffix,
        "BeginDraft's suffix ends with the exact part count and a zero u16.",
    );

    let mut part_suffix = zeros(40);
    bytes_at(&mut part_suffix, 0, &OP_PARENT);
    u16_at(&mut part_suffix, 16, 2);
    u64_at(&mut part_suffix, 20, 7);
    u64_at(&mut part_suffix, 28, draft_part_payload().len() as u64);
    u32_at(&mut part_suffix, 36, crc32(&draft_part_payload()));
    push(
        "start-draft-part",
        Opcode::StartDraftPart,
        part_suffix,
        "The child's own OperationId is the lookup key and is not repeated in the digest.",
    );

    let mut delete_suffix = zeros(18);
    u16_at(&mut delete_suffix, 0, 1);
    u64_at(&mut delete_suffix, 2, 9);
    u64_at(&mut delete_suffix, 10, 42);
    push(
        "delete-object",
        Opcode::DeleteObject,
        delete_suffix,
        "DeleteObject's suffix is kind, identity, and expected revision.",
    );

    let mut set_suffix = zeros(20);
    u16_at(&mut set_suffix, 0, 1);
    u64_at(&mut set_suffix, 2, 9);
    u64_at(&mut set_suffix, 10, 42);
    let patch = route_patch(Some(3), Some(true), Some("Kaiserstuhl loop"));
    u16_at(&mut set_suffix, 18, patch.len() as u16);
    set_suffix.extend_from_slice(&patch);
    push("set-metadata", Opcode::SetMetadata, set_suffix, "The patch envelope is part of the intent, byte for byte.");

    let mut abort_suffix = zeros(24);
    bytes_at(&mut abort_suffix, 0, &OP_A);
    abort_suffix[16] = 1;
    push(
        "abort-operation",
        Opcode::AbortOperation,
        abort_suffix,
        "The abort command's suffix is its target and reason, then seven zero bytes.",
    );

    let mut install_suffix = zeros(18);
    u16_at(&mut install_suffix, 0, 7);
    u64_at(&mut install_suffix, 2, 3);
    u64_at(&mut install_suffix, 10, 70);
    push(
        "install-update",
        Opcode::InstallUpdate,
        install_suffix,
        "The ObjectKind field is pinned at the literal value 7.",
    );

    let mut ack_suffix = zeros(18);
    u16_at(&mut ack_suffix, 0, 3);
    u64_at(&mut ack_suffix, 2, 5);
    u64_at(&mut ack_suffix, 10, 61);
    push(
        "acknowledge-ride-imported",
        Opcode::AcknowledgeRideImported,
        ack_suffix,
        "The ObjectKind field is pinned at the literal value 3.",
    );

    all
}

/// §14.0's frame-limit derivation, as the cases the vectors contract asks for.
///
/// "Frame-limit derivation is pinned as cases rather than prose: ATT MTU 247 yields a 244-byte
/// ceiling, 195 yields exactly the 192-byte minimum, 194 is refused at Hello with
/// `resourceLimit/minimumControlFrame`, and 66 produces no frame at all because even the refusal is
/// undeliverable."
pub fn derivations() -> Vec<DerivationVector> {
    let control = |att_mtu: u16, outcome: &'static str, negotiated: u16, note: &'static str| DerivationCase {
        channel: "control",
        link_value: att_mtu,
        ceiling: att_mtu.saturating_sub(3),
        client_max: 512,
        device_max: 512,
        outcome,
        negotiated,
        note,
    };
    let stream =
        |sdu: u16, client_max: u16, outcome: &'static str, negotiated: u16, note: &'static str| DerivationCase {
            channel: "stream",
            link_value: sdu,
            ceiling: sdu,
            client_max,
            device_max: 4096,
            outcome,
            negotiated,
            note,
        };
    vec![DerivationVector {
        name: "frame-limit-derivation-cases".to_string(),
        cases: vec![
            control(
                247,
                "negotiated",
                244,
                "One ATT Write Request or indication value carries at most ATT_MTU - 3 bytes, so the device's \
                 preferred 247-byte MTU yields a 244-byte ceiling.",
            ),
            control(
                195,
                "negotiated",
                192,
                "Carrying the 192-byte protocol minimum therefore requires ATT_MTU >= 195, and 195 yields exactly it.",
            ),
            control(
                194,
                "belowProtocolMinimum",
                0,
                "Below the minimum no negotiation is possible: Hello is answered resourceLimit/minimumControlFrame \
                 with retry-only-after-user-action, and nothing is admitted on that connection.",
            ),
            control(
                66,
                "undeliverable",
                0,
                "Below a 64-byte frame — the 16-byte header plus a text-free ErrorBody — the refusal itself is \
                 undeliverable, so the adapter disconnects rather than truncating an error.",
            ),
            stream(
                512,
                1024,
                "negotiated",
                512,
                "The effective stream limit is min(negotiated stream maximum, CoC SDU), fixed at CoC establishment.",
            ),
            stream(
                63,
                1024,
                "belowProtocolMinimum",
                0,
                "An SDU below the 64-byte floor refuses the channel with resourceLimit/minimumStreamFrame.",
            ),
        ],
    }]
}

// ---------------------------------------------------------------------------------------------
// Stream vectors.
// ---------------------------------------------------------------------------------------------

/// Every stream vector in the suite.
pub fn streams() -> Vec<StreamVector> {
    let payload = route_payload();
    let mut all = Vec::new();
    let mut push = |name: &str, record: Vec<u8>, note: &str| {
        all.push(StreamVector { name: name.to_string(), record, note: note.to_string() });
    };

    for (direction, label) in [(1u8, "upload"), (2, "download")] {
        push(
            &format!("{label}-first-frame"),
            stream_frame(0x0000_0011, 0, direction, 0, &payload[..1008]),
            "The first data frame of a session, at offset zero.",
        );
        push(
            &format!("{label}-middle-frame"),
            stream_frame(0x0000_0011, 1008, direction, 0, &payload[1008..2016]),
            "A middle frame at exactly the session's next offset.",
        );
        push(
            &format!("{label}-final-frame"),
            stream_frame(0x0000_0011, 2016, direction, 0, &payload[2016..]),
            "The final frame; success is FinishUpload or FinishDownload on the control link, never a stream flag.",
        );
        push(
            &format!("{label}-minimum-payload"),
            stream_frame(0x0000_0011, 4096, direction, 0, &[0x5A]),
            "The one-byte minimum: data directions have nonempty payload.",
        );
        push(
            &format!("{label}-maximum-negotiated-payload"),
            stream_frame(0x0000_0011, 8192, direction, 0, &vec![0xA5; 1008]),
            "The maximum payload at a 1024-byte negotiated stream frame.",
        );
        push(
            &format!("{label}-offset-below-u32-maximum"),
            stream_frame(0x0000_0011, 0xFFFF_FFFE, direction, 0, &[0x01]),
            "An offset just below 0xFFFF_FFFF, without allocating the preceding bytes.",
        );
        push(
            &format!("{label}-offset-above-u32-maximum"),
            stream_frame(0x0000_0011, 0x1_0000_0000, direction, 0, &[0x02]),
            "A u64 offset past the 32-bit boundary: a codec that truncates to 32 bits is nonconforming.",
        );
    }

    push(
        "upload-resumed-prefix-frame",
        stream_frame(0x0000_0012, u64::from(FIXTURE_GRANULE), 1, 0, &payload[FIXTURE_GRANULE as usize..1032]),
        "The frame a resumed client sends after comparing its retained prefix CRC against the acceptance's.",
    );

    for (disposition, terminal, name, note) in [
        (
            0u8,
            false,
            "fault-resume-with-new-session",
            "Disposition 0 is the nonterminal form: the session may still be resumed under a new SessionId.",
        ),
        (
            1,
            true,
            "fault-operation-durably-aborted",
            "Disposition 1 is terminal: the session is released and the operation durably aborted.",
        ),
        (
            2,
            true,
            "fault-stream-closed-query-status",
            "Disposition 2 is terminal: the stream transport is closed and the client queries the operation.",
        ),
    ] {
        push(
            name,
            stream_frame(
                0x0000_0011,
                0,
                3,
                1 | if terminal { 2 } else { 0 },
                &fault_body(8, 1, u64::from(FIXTURE_GRANULE), u64::from(FIXTURE_GRANULE), disposition),
            ),
            note,
        );
    }
    push(
        "fault-media-io-mid-stream",
        stream_frame(0x0000_0011, 0, 3, 1 | 2, &fault_body(16, 2, 4096, 0, 1)),
        "Only namespace-zero transport categories are valid in this compact body.",
    );

    all
}

// ---------------------------------------------------------------------------------------------
// Rejection fixtures.
// ---------------------------------------------------------------------------------------------

/// Every rejection fixture in the suite.
pub fn negatives() -> Vec<NegativeVector> {
    let mut all = Vec::new();
    let mut push = |name: &str,
                    target: NegativeTarget,
                    bytes: Vec<u8>,
                    category: ErrorCategory,
                    detail: u16,
                    note: &str| {
        all.push(NegativeVector { name: name.to_string(), target, bytes, category, detail, note: note.to_string() });
    };

    // ---- Framing -----------------------------------------------------------------------------
    let good_frame = control_frame(Opcode::FinishUpload.to_u16(), REQUEST, 1, &[0x11, 0, 0, 0]);
    let mut bad_magic = good_frame.clone();
    bad_magic[0] = b'X';
    push(
        "frame-bad-magic",
        NegativeTarget::ControlFrame,
        bad_magic,
        ErrorCategory::INVALID_FRAME,
        detail::frame::MAGIC,
        "Bad magic is invalidFrame, not a descriptor fault.",
    );
    let mut bad_major = good_frame.clone();
    bad_major[4] = 2;
    push(
        "frame-incompatible-major",
        NegativeTarget::ControlFrame,
        bad_major,
        ErrorCategory::INCOMPATIBLE_VERSION,
        detail::version::UNSUPPORTED_MAJOR,
        "An unsupported parseable wire version is incompatibleVersion, not either malformed category.",
    );
    let mut bad_minor = good_frame.clone();
    bad_minor[5] = 1;
    push(
        "frame-unsupported-minor",
        NegativeTarget::ControlFrame,
        bad_minor,
        ErrorCategory::INCOMPATIBLE_VERSION,
        detail::version::UNSUPPORTED_MINOR,
        "A frame minor above the device's is incompatibleVersion/unsupportedMinor.",
    );
    let mut zero_request = good_frame.clone();
    u32_at(&mut zero_request, 12, 0);
    push(
        "frame-zero-request-id",
        NegativeTarget::ControlFrame,
        zero_request,
        ErrorCategory::INVALID_DESCRIPTOR,
        detail::descriptor::ZERO_REQUEST_ID,
        "A zero-RequestId frame produces no response at all and closes the control record stream.",
    );
    let mut truncated = good_frame.clone();
    truncated.truncate(15);
    push(
        "frame-record-too-short",
        NegativeTarget::ControlFrame,
        truncated,
        ErrorCategory::INVALID_FRAME,
        detail::frame::RECORD_LENGTH,
        "A record shorter than the 16-byte header cannot be established as a frame.",
    );
    let mut trailing = good_frame.clone();
    trailing.push(0);
    push(
        "frame-payload-length-mismatch",
        NegativeTarget::ControlFrame,
        trailing,
        ErrorCategory::INVALID_FRAME,
        detail::frame::PAYLOAD_LENGTH,
        "A payload length that disagrees with the record is invalidFrame.",
    );
    let mut overflow = good_frame.clone();
    u16_at(&mut overflow, 10, 497);
    push(
        "frame-payload-length-overflow",
        NegativeTarget::ControlFrame,
        overflow,
        ErrorCategory::INVALID_FRAME,
        detail::frame::PAYLOAD_LENGTH,
        "A payload length above 496 overflows the hard maximum frame.",
    );
    let mut unknown_opcode = good_frame.clone();
    u16_at(&mut unknown_opcode, 6, 0x0999);
    push(
        "frame-unknown-opcode",
        NegativeTarget::ControlFrame,
        unknown_opcode,
        ErrorCategory::UNSUPPORTED_CAPABILITY,
        detail::capability::OPCODE,
        "Unknown opcodes are unsupportedCapability; the frame itself parsed perfectly.",
    );
    let mut reserved_flags = good_frame.clone();
    u16_at(&mut reserved_flags, 8, 1 << 3);
    push(
        "frame-reserved-header-flags",
        NegativeTarget::ControlFrame,
        reserved_flags,
        ErrorCategory::INVALID_DESCRIPTOR,
        detail::descriptor::UNSUPPORTED_FLAGS,
        "Flags 3..15 are zero.",
    );
    let mut flagged_request = good_frame.clone();
    u16_at(&mut flagged_request, 8, FrameFlags::MORE);
    push(
        "frame-flags-on-a-request",
        NegativeTarget::ControlFrame,
        flagged_request,
        ErrorCategory::INVALID_DESCRIPTOR,
        detail::descriptor::UNSUPPORTED_FLAGS,
        "Requests have no flags.",
    );
    let more_on_unpageable = control_frame(Opcode::QueryOperation.to_u16(), OK_MORE, 1, &operation_status(0, &[]));
    push(
        "frame-more-on-an-unpageable-response",
        NegativeTarget::ControlFrame,
        more_on_unpageable,
        ErrorCategory::INVALID_DESCRIPTOR,
        detail::descriptor::INVALID_COMBINATION,
        "`more` is valid only on a paged Capabilities, QueryCatalog, or QueryDraft response.",
    );

    // ---- Metadata envelopes --------------------------------------------------------------------
    let route = route_put(2);
    let mut duplicate = envelope(1, 128, &[(0x8001, vec![1]), (0x8001, vec![1])]);
    push(
        "metadata-duplicate-base-tag",
        NegativeTarget::MetadataEnvelope(128),
        duplicate.clone(),
        ErrorCategory::INVALID_DESCRIPTOR,
        detail::descriptor::DUPLICATE_FIELD,
        "Base tags are unique; changing only the critical bit does not create another field.",
    );
    duplicate = envelope(1, 128, &[(0x8002, vec![1]), (0x8001, vec![1])]);
    push(
        "metadata-out-of-order-base-tags",
        NegativeTarget::MetadataEnvelope(128),
        duplicate,
        ErrorCategory::INVALID_DESCRIPTOR,
        detail::descriptor::OUT_OF_ORDER_FIELD,
        "Fields are strictly increasing by base tag.",
    );
    let mut zero_tag = route.clone();
    u16_at(&mut zero_tag, 8, 0x8000);
    push(
        "metadata-zero-base-tag",
        NegativeTarget::MetadataEnvelope(128),
        zero_tag,
        ErrorCategory::INVALID_DESCRIPTOR,
        detail::descriptor::NONCANONICAL_METADATA,
        "The tag's low 15 bits are a nonzero base tag.",
    );
    let mut bad_count = route.clone();
    u16_at(&mut bad_count, 6, 2);
    push(
        "metadata-field-count-disagrees",
        NegativeTarget::MetadataEnvelope(128),
        bad_count,
        ErrorCategory::INVALID_DESCRIPTOR,
        detail::descriptor::NONCANONICAL_METADATA,
        "field_count equals the number of fields.",
    );
    let mut runs_past = route.clone();
    u16_at(&mut runs_past, 10, 9);
    push(
        "metadata-value-length-runs-past-the-body",
        NegativeTarget::MetadataEnvelope(128),
        runs_past,
        ErrorCategory::INVALID_DESCRIPTOR,
        detail::descriptor::NONCANONICAL_METADATA,
        "encoded_field_bytes equals the exact sum of every 4 + value_length.",
    );
    let mut padded = route.clone();
    u16_at(&mut padded, 4, (route.len() - 8 + 2) as u16);
    padded.extend_from_slice(&[0, 0]);
    push(
        "metadata-trailing-padding",
        NegativeTarget::MetadataEnvelope(128),
        padded,
        ErrorCategory::INVALID_DESCRIPTOR,
        detail::descriptor::NONCANONICAL_METADATA,
        "Padding is forbidden.",
    );
    let mut nonzero_flags = route.clone();
    nonzero_flags[3] = 1;
    push(
        "metadata-nonzero-header-flags",
        NegativeTarget::MetadataEnvelope(128),
        nonzero_flags,
        ErrorCategory::INVALID_DESCRIPTOR,
        detail::descriptor::RESERVED_BITS,
        "The header flags are zero.",
    );
    let mut oversized = zeros(8);
    u16_at(&mut oversized, 0, 1);
    oversized[2] = 64;
    u16_at(&mut oversized, 4, 89);
    push(
        "metadata-above-the-catalog-ceiling",
        NegativeTarget::MetadataEnvelope(96),
        oversized,
        ErrorCategory::INVALID_DESCRIPTOR,
        detail::descriptor::NESTED_LENGTH,
        "Catalog envelopes are at most 96 bytes, so their encoded fields are at most 88.",
    );

    // ---- Descriptor bodies -----------------------------------------------------------------
    let create_with_id = control_frame(
        Opcode::StartUpload.to_u16(),
        REQUEST,
        1,
        &start_upload(OP_A, 1, 0, 0, 5, 0, 10, 0, &route_put(1)),
    );
    push(
        "start-upload-create-with-a-nonzero-identity",
        NegativeTarget::ControlBody(Opcode::StartUpload, false),
        create_with_id,
        ErrorCategory::INVALID_DESCRIPTOR,
        detail::descriptor::INVALID_COMBINATION,
        "In create mode both identity fields are constrained to zero.",
    );
    let bad_resume = control_frame(
        Opcode::StartUpload.to_u16(),
        REQUEST,
        1,
        &start_upload(OP_A, 1, 0, 2, 0, 0, 10, 0, &route_put(1)),
    );
    push(
        "start-upload-resume-byte-above-one",
        NegativeTarget::ControlBody(Opcode::StartUpload, false),
        bad_resume,
        ErrorCategory::INVALID_DESCRIPTOR,
        detail::descriptor::UNKNOWN_ENUM,
        "The resume byte has exactly two legal values.",
    );
    let empty_patch = {
        let mut body = zeros(36);
        bytes_at(&mut body, 0, &OP_A);
        u16_at(&mut body, 16, 1);
        u16_at(&mut body, 18, 1);
        u64_at(&mut body, 20, 9);
        u64_at(&mut body, 28, 42);
        body.extend_from_slice(&envelope(1, 128, &[]));
        control_frame(Opcode::SetMetadata.to_u16(), REQUEST, 1, &body)
    };
    push(
        "set-metadata-empty-patch",
        NegativeTarget::ControlBody(Opcode::SetMetadata, false),
        empty_patch,
        ErrorCategory::INVALID_DESCRIPTOR,
        detail::descriptor::EMPTY_METADATA_PATCH,
        "A well-formed zero-field patch is refused as a request.",
    );
    let missing_revision_flag = {
        let mut body = zeros(36);
        bytes_at(&mut body, 0, &OP_A);
        u16_at(&mut body, 16, 1);
        u64_at(&mut body, 20, 9);
        u64_at(&mut body, 28, 42);
        control_frame(Opcode::DeleteObject.to_u16(), REQUEST, 1, &body)
    };
    push(
        "delete-object-without-the-mandatory-revision-flag",
        NegativeTarget::ControlBody(Opcode::DeleteObject, false),
        missing_revision_flag,
        ErrorCategory::INVALID_DESCRIPTOR,
        detail::descriptor::INVALID_COMBINATION,
        "Expected-revision bit 0 is mandatory.",
    );
    let download_reserved_flag = {
        let mut body = zeros(28);
        u16_at(&mut body, 0, 1);
        u16_at(&mut body, 2, 1);
        u64_at(&mut body, 4, 9);
        control_frame(Opcode::StartDownload.to_u16(), REQUEST, 1, &body)
    };
    push(
        "start-download-reserved-revision-flag",
        NegativeTarget::ControlBody(Opcode::StartDownload, false),
        download_reserved_flag,
        ErrorCategory::INVALID_DESCRIPTOR,
        detail::descriptor::RESERVED_BITS,
        "Flag bit 0 is burned and no v3.0 peer sets it.",
    );
    let download_reserved_field = {
        let mut body = zeros(28);
        u16_at(&mut body, 0, 1);
        u64_at(&mut body, 4, 9);
        u64_at(&mut body, 12, 3);
        control_frame(Opcode::StartDownload.to_u16(), REQUEST, 1, &body)
    };
    push(
        "start-download-reserved-revision-field",
        NegativeTarget::ControlBody(Opcode::StartDownload, false),
        download_reserved_field,
        ErrorCategory::INVALID_DESCRIPTOR,
        detail::descriptor::RESERVED_BITS,
        "The eight bytes at offset 12 are burned too.",
    );
    let cursor_only = {
        let mut body = zeros(28);
        u16_at(&mut body, 0, 1);
        u16_at(&mut body, 2, 2);
        control_frame(Opcode::QueryCatalog.to_u16(), REQUEST, 1, &body)
    };
    push(
        "query-catalog-cursor-flag-alone",
        NegativeTarget::ControlBody(Opcode::QueryCatalog, false),
        cursor_only,
        ErrorCategory::INVALID_DESCRIPTOR,
        detail::descriptor::INVALID_COMBINATION,
        "A cursor requires both bits.",
    );
    let cursor_revision_mismatch = {
        let mut body = zeros(28);
        u16_at(&mut body, 0, 1);
        u16_at(&mut body, 2, 3);
        u64_at(&mut body, 4, 43);
        bytes_at(&mut body, 12, &catalog_cursor(STORE, 42, 3, 1));
        control_frame(Opcode::QueryCatalog.to_u16(), REQUEST, 1, &body)
    };
    push(
        "query-catalog-cursor-revision-mismatch",
        NegativeTarget::ControlBody(Opcode::QueryCatalog, false),
        cursor_revision_mismatch,
        ErrorCategory::INVALID_DESCRIPTOR,
        detail::descriptor::INVALID_COMBINATION,
        "The expected revision must equal the cursor revision.",
    );
    let stray_expected_revision = {
        let mut body = zeros(28);
        u16_at(&mut body, 0, 1);
        u64_at(&mut body, 4, 42);
        control_frame(Opcode::QueryCatalog.to_u16(), REQUEST, 1, &body)
    };
    push(
        "query-catalog-expected-revision-without-its-flag",
        NegativeTarget::ControlBody(Opcode::QueryCatalog, false),
        stray_expected_revision,
        ErrorCategory::INVALID_DESCRIPTOR,
        detail::descriptor::RESERVED_BITS,
        "With neither flag, both fields are zero.",
    );
    let draft_limit_zero = {
        let mut body = zeros(44);
        bytes_at(&mut body, 0, &OP_PARENT);
        control_frame(Opcode::QueryDraft.to_u16(), REQUEST, 1, &body)
    };
    push(
        "query-draft-zero-limit",
        NegativeTarget::ControlBody(Opcode::QueryDraft, false),
        draft_limit_zero,
        ErrorCategory::INVALID_DESCRIPTOR,
        detail::descriptor::INVALID_COMBINATION,
        "The requested limit is 1 through 6.",
    );
    let child_equals_parent = {
        let mut body = zeros(64);
        bytes_at(&mut body, 0, &OP_PARENT);
        bytes_at(&mut body, 16, &OP_PARENT);
        u16_at(&mut body, 32, 2);
        u64_at(&mut body, 36, 1);
        u64_at(&mut body, 44, 10);
        control_frame(Opcode::StartDraftPart.to_u16(), REQUEST, 1, &body)
    };
    push(
        "start-draft-part-child-equals-parent",
        NegativeTarget::ControlBody(Opcode::StartDraftPart, false),
        child_equals_parent,
        ErrorCategory::INVALID_DESCRIPTOR,
        detail::descriptor::INVALID_COMBINATION,
        "The child OperationId must be distinct from the parent.",
    );
    let zero_part_count = {
        let mut body = zeros(52);
        bytes_at(&mut body, 0, &OP_PARENT);
        u16_at(&mut body, 16, 6);
        u64_at(&mut body, 36, 264);
        control_frame(Opcode::BeginDraft.to_u16(), REQUEST, 1, &body)
    };
    push(
        "begin-draft-zero-part-count",
        NegativeTarget::ControlBody(Opcode::BeginDraft, false),
        zero_part_count,
        ErrorCategory::INVALID_DESCRIPTOR,
        detail::descriptor::INVALID_COMBINATION,
        "The exact part count is nonzero.",
    );
    let zero_session_finish = control_frame(Opcode::FinishUpload.to_u16(), REQUEST, 1, &[0, 0, 0, 0]);
    push(
        "finish-upload-zero-session",
        NegativeTarget::ControlBody(Opcode::FinishUpload, false),
        zero_session_finish,
        ErrorCategory::INVALID_DESCRIPTOR,
        detail::descriptor::UNKNOWN_ENUM,
        "A SessionId is nonzero by construction.",
    );

    // ---- Acceptance pre-freeze twins and flag faults ----------------------------------------
    let mut short_upload = upload_accepted(0, 0, OP_A, 1, 9, 41, 0, 262_144, 1008, 0);
    short_upload.truncate(56);
    push(
        "upload-accepted-pre-freeze-56-bytes",
        NegativeTarget::ControlBody(Opcode::StartUpload, true),
        control_frame(Opcode::StartUpload.to_u16(), OK, 1, &short_upload),
        ErrorCategory::INVALID_FRAME,
        detail::frame::TRUNCATED,
        "The 56-byte pre-freeze twin MUST fail decode rather than decode short.",
    );
    let both_flags = upload_accepted(0, 3, OP_A, 1, 9, 41, 0, 262_144, 1008, 0);
    push(
        "upload-accepted-both-resume-flags",
        NegativeTarget::ControlBody(Opcode::StartUpload, true),
        control_frame(Opcode::StartUpload.to_u16(), OK, 1, &both_flags),
        ErrorCategory::INVALID_DESCRIPTOR,
        detail::descriptor::INVALID_COMBINATION,
        "Restart-at-zero and resumed-work are never both set.",
    );
    let restart_with_offset = upload_accepted(0, 2, OP_A, 1, 9, 41, 262_144, 262_144, 1008, 0);
    push(
        "upload-accepted-restart-with-a-nonzero-offset",
        NegativeTarget::ControlBody(Opcode::StartUpload, true),
        control_frame(Opcode::StartUpload.to_u16(), OK, 1, &restart_with_offset),
        ErrorCategory::INVALID_DESCRIPTOR,
        detail::descriptor::INVALID_COMBINATION,
        "Restart-at-zero forces the reported durable next offset to zero.",
    );
    let crc_over_empty_prefix = upload_accepted(0, 0, OP_A, 1, 9, 41, 0, 262_144, 1008, 7);
    push(
        "upload-accepted-crc-over-an-empty-prefix",
        NegativeTarget::ControlBody(Opcode::StartUpload, true),
        control_frame(Opcode::StartUpload.to_u16(), OK, 1, &crc_over_empty_prefix),
        ErrorCategory::INVALID_DESCRIPTOR,
        detail::descriptor::INVALID_COMBINATION,
        "The finalized prefix CRC is zero when the durable next offset is zero.",
    );
    let flag_in_target_mode = upload_accepted(2, 0, OP_A, 1, 9, 41, 0, 262_144, 1008, 0);
    push(
        "upload-accepted-flag-in-the-target-mode-byte",
        NegativeTarget::ControlBody(Opcode::StartUpload, true),
        control_frame(Opcode::StartUpload.to_u16(), OK, 1, &flag_in_target_mode),
        ErrorCategory::INVALID_DESCRIPTOR,
        detail::descriptor::UNKNOWN_ENUM,
        "Offset 1 is target mode here, and a flag written into it is rejected.",
    );

    let mut short_part = zeros(72);
    u32_at(&mut short_part, 36, 1);
    u16_at(&mut short_part, 40, 2);
    short_part.truncate(68);
    push(
        "draft-part-accepted-pre-freeze-68-bytes",
        NegativeTarget::ControlBody(Opcode::StartDraftPart, true),
        control_frame(Opcode::StartDraftPart.to_u16(), OK, 1, &short_part),
        ErrorCategory::INVALID_FRAME,
        detail::frame::TRUNCATED,
        "The 68-byte pre-freeze twin MUST fail decode.",
    );
    let mut flagged_reserved = zeros(72);
    flagged_reserved[1] = 1;
    u32_at(&mut flagged_reserved, 36, 1);
    u16_at(&mut flagged_reserved, 40, 2);
    push(
        "draft-part-accepted-flag-in-the-reserved-byte",
        NegativeTarget::ControlBody(Opcode::StartDraftPart, true),
        control_frame(Opcode::StartDraftPart.to_u16(), OK, 1, &flagged_reserved),
        ErrorCategory::INVALID_DESCRIPTOR,
        detail::descriptor::RESERVED_BITS,
        "Offset 1 is reserved in DraftPartAccepted.",
    );
    let mut short_finalize = zeros(64);
    u32_at(&mut short_finalize, 20, 1);
    short_finalize.truncate(56);
    push(
        "finalize-accepted-pre-freeze-56-bytes",
        NegativeTarget::ControlBody(Opcode::FinalizeDraft, true),
        control_frame(Opcode::FinalizeDraft.to_u16(), OK, 1, &short_finalize),
        ErrorCategory::INVALID_FRAME,
        detail::frame::TRUNCATED,
        "The 56-byte pre-freeze twin MUST fail decode.",
    );
    let mut finalize_flagged = zeros(64);
    finalize_flagged[1] = 1;
    u32_at(&mut finalize_flagged, 20, 1);
    push(
        "finalize-accepted-flag-in-the-reserved-byte",
        NegativeTarget::ControlBody(Opcode::FinalizeDraft, true),
        control_frame(Opcode::FinalizeDraft.to_u16(), OK, 1, &finalize_flagged),
        ErrorCategory::INVALID_DESCRIPTOR,
        detail::descriptor::RESERVED_BITS,
        "Offset 1 is reserved in the FinalizeDraft acceptance.",
    );

    // ---- Capabilities ------------------------------------------------------------------------
    let mut mismatched_codec =
        capabilities(0b0011, Some(STORE), 2, true, 7, ALL_COMMANDS, 0, 0, 0, 0, 1, &resource_limits(0));
    mismatched_codec[56] = 2;
    push("capabilities-codec-version-mismatch", NegativeTarget::CapabilitiesPayload, mismatched_codec, ErrorCategory::INVALID_DESCRIPTOR, detail::descriptor::INVALID_COMBINATION, "A client that observes byte 54 disagreeing with the block's byte 0 MUST reject the page without decoding either block.");
    let subject_page_beyond_the_end = capabilities(0b0011, Some(STORE), 2, true, 7, ALL_COMMANDS, 0, 1, 1, 0, 0, &[]);
    push(
        "capabilities-subject-page-index-beyond-the-end",
        NegativeTarget::CapabilitiesPayload,
        subject_page_beyond_the_end,
        ErrorCategory::INVALID_DESCRIPTOR,
        detail::descriptor::INVALID_COMBINATION,
        "For a zero-subject device only a subject page index above zero is invalid.",
    );
    let store_without_availability =
        capabilities(0b0010, Some(STORE), 2, true, 7, ALL_COMMANDS, 0, 0, 0, 0, 1, &resource_limits(0));
    push(
        "capabilities-store-id-without-store-available",
        NegativeTarget::CapabilitiesPayload,
        store_without_availability,
        ErrorCategory::INVALID_DESCRIPTOR,
        detail::descriptor::RESERVED_BITS,
        "The StoreId is zero only when store-available is clear, and an inactive field is encoded zero.",
    );
    push(
        "subject-entry-patch-version-without-the-flag",
        NegativeTarget::SubjectEntry,
        subject(1, 2, PUT | GET, 0, 1, 128, 64, 1024),
        ErrorCategory::INVALID_DESCRIPTOR,
        detail::descriptor::INVALID_COMBINATION,
        "A nonzero patch schema version while the set-metadata flag is clear.",
    );
    push(
        "subject-entry-patch-version-other-than-128",
        NegativeTarget::SubjectEntry,
        subject(1, 1, GET | SET_META, 0, 0, 1, 64, 1024),
        ErrorCategory::INVALID_DESCRIPTOR,
        detail::descriptor::INVALID_COMBINATION,
        "A patch schema version other than 128 while that flag is set.",
    );
    push(
        "subject-entry-ride-advertising-put",
        NegativeTarget::SubjectEntry,
        subject(1, 3, PUT | GET, 0, 1, 0, 64, 1024),
        ErrorCategory::INVALID_DESCRIPTOR,
        detail::descriptor::INVALID_COMBINATION,
        "A `no` in the registry's lifecycle table is normative.",
    );
    push(
        "subject-entry-draft-part-advertising-get",
        NegativeTarget::SubjectEntry,
        subject(2, 1, PUT | GET, 0, 0, 0, 0, 1024),
        ErrorCategory::INVALID_DESCRIPTOR,
        detail::descriptor::INVALID_COMBINATION,
        "A draft-part subject advertises put and optional resumable upload only.",
    );

    // ---- Error bodies ------------------------------------------------------------------------
    push(
        "error-body-category-zero",
        NegativeTarget::ErrorBody,
        error_body(0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, &[]),
        ErrorCategory::INVALID_DESCRIPTOR,
        detail::descriptor::UNKNOWN_ENUM,
        "Category 0 is reserved and invalid: a receiver treats it as a malformed body.",
    );
    push(
        "error-body-terminal-bit-without-claim-bit",
        NegativeTarget::ErrorBody,
        error_body(17, 0, 1, 0, 0, presence::CLAIM_IS_TERMINAL, 0, 0, 0, 0, 0, &[]),
        ErrorCategory::INVALID_DESCRIPTOR,
        detail::descriptor::INVALID_COMBINATION,
        "Bit 6 set with bit 5 clear MUST be rejected as a malformed body.",
    );
    push(
        "error-body-reserved-presence-bit",
        NegativeTarget::ErrorBody,
        error_body(17, 0, 1, 0, 0, 1 << 7, 0, 0, 0, 0, 0, &[]),
        ErrorCategory::INVALID_DESCRIPTOR,
        detail::descriptor::RESERVED_BITS,
        "Presence bits 7..15 are zero.",
    );
    let mut over_long_text = error_body(22, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, &[b'x'; 65]);
    over_long_text[46] = 65;
    push(
        "error-body-text-length-above-64",
        NegativeTarget::ErrorBody,
        over_long_text,
        ErrorCategory::INVALID_FRAME,
        detail::frame::PAYLOAD_LENGTH,
        "A text length above 64 is structural.",
    );
    let mut disagreeing_text = error_body(22, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, b"ab");
    disagreeing_text[46] = 3;
    push(
        "error-body-text-length-disagrees",
        NegativeTarget::ErrorBody,
        disagreeing_text,
        ErrorCategory::INVALID_FRAME,
        detail::frame::PAYLOAD_LENGTH,
        "A text length that disagrees with the payload length is structural.",
    );

    // ---- Device control ----------------------------------------------------------------------
    let mut padded_name = config_block(0, 0, b"OBC");
    padded_name[11] = b'!';
    push(
        "config-nonzero-byte-beyond-the-name-length",
        NegativeTarget::ConfigBlock,
        padded_name,
        ErrorCategory::INVALID_DESCRIPTOR,
        detail::descriptor::RESERVED_BITS,
        "A nonzero byte at or beyond the stated length is invalidDescriptor.",
    );
    let mut long_name = config_block(0, 0, b"OBC");
    long_name[4] = 33;
    push(
        "config-name-length-above-32",
        NegativeTarget::ConfigBlock,
        long_name,
        ErrorCategory::INVALID_DESCRIPTOR,
        detail::descriptor::INVALID_COMBINATION,
        "A name length above 32 is invalidDescriptor.",
    );
    push(
        "config-weather-refresh-above-4",
        NegativeTarget::ConfigBlock,
        config_block(0, 5, b"OBC"),
        ErrorCategory::INVALID_DESCRIPTOR,
        detail::descriptor::UNKNOWN_ENUM,
        "A weather-refresh value above 4 is invalidDescriptor.",
    );
    push(
        "config-reserved-unit-flag",
        NegativeTarget::ConfigBlock,
        config_block(1 << 3, 0, b"OBC"),
        ErrorCategory::INVALID_DESCRIPTOR,
        detail::descriptor::UNSUPPORTED_FLAGS,
        "A reserved unit-flag bit is invalidDescriptor.",
    );
    let mut bad_length = config_block(0, 0, b"OBC");
    bad_length[1] = 57;
    push(
        "config-block-length-other-than-56",
        NegativeTarget::ConfigBlock,
        bad_length,
        ErrorCategory::INVALID_DESCRIPTOR,
        detail::descriptor::INVALID_COMBINATION,
        "A block length other than 56 is invalidDescriptor.",
    );
    let unknown_source = control_frame(Opcode::SetClock.to_u16(), REQUEST, 1, &set_clock(1_763_000_000, 3));
    push(
        "set-clock-unknown-source",
        NegativeTarget::ControlBody(Opcode::SetClock, false),
        unknown_source,
        ErrorCategory::INVALID_DESCRIPTOR,
        detail::descriptor::UNKNOWN_ENUM,
        "An unknown source value is invalidDescriptor/unknownEnum.",
    );
    let mut zero_scope = zeros(8);
    zero_scope[0] = 0;
    push(
        "forget-bond-zero-scope",
        NegativeTarget::ControlBody(Opcode::ForgetBond, false),
        control_frame(Opcode::ForgetBond.to_u16(), REQUEST, 1, &zero_scope),
        ErrorCategory::INVALID_DESCRIPTOR,
        detail::descriptor::UNKNOWN_ENUM,
        "The scope byte is this bond `1` or every bond `2`.",
    );
    let status_with_store_in_class_zero = {
        let mut body = device_status(0, 0, None);
        bytes_at(&mut body, 48, &STORE);
        control_frame(Opcode::GetDeviceStatus.to_u16(), OK, 1, &body)
    };
    push(
        "device-status-store-id-in-a-class-that-reports-none",
        NegativeTarget::ControlBody(Opcode::GetDeviceStatus, true),
        status_with_store_in_class_zero,
        ErrorCategory::INVALID_DESCRIPTOR,
        detail::descriptor::RESERVED_BITS,
        "The StoreId is zero unless the mount class is 3, 4, or 6.",
    );

    // ---- Schema conformance, beyond canonical form (§2.2) --------------------------------------
    let put_with = |fields: Vec<u8>| {
        control_frame(Opcode::StartUpload.to_u16(), REQUEST, 1, &start_upload(OP_A, 1, 0, 0, 0, 0, 10, 0, &fields))
    };
    push(
        "metadata-unknown-critical-field",
        NegativeTarget::ControlBody(Opcode::StartUpload, false),
        put_with(envelope(1, 1, &[(0x8001, vec![2]), (0x8055, vec![9])])),
        ErrorCategory::INVALID_DESCRIPTOR,
        detail::descriptor::INVALID_COMBINATION,
        "A mutating request rejects an unknown critical field.",
    );
    push(
        "metadata-unknown-noncritical-field-in-a-mutating-request",
        NegativeTarget::ControlBody(Opcode::StartUpload, false),
        put_with(envelope(1, 1, &[(0x8001, vec![2]), (0x0055, vec![9])])),
        ErrorCategory::INVALID_DESCRIPTOR,
        detail::descriptor::INVALID_COMBINATION,
        "Mutating requests reject every unknown field, whether critical or not; only a projection may skip one.",
    );
    push(
        "metadata-schema-id-does-not-match-the-object-kind",
        NegativeTarget::ControlBody(Opcode::StartUpload, false),
        put_with(envelope(2, 1, &[])),
        ErrorCategory::INVALID_DESCRIPTOR,
        detail::descriptor::INVALID_COMBINATION,
        "The schema_id must exactly match the containing logical ObjectKind.",
    );
    push(
        "metadata-schema-version-is-not-the-registered-one",
        NegativeTarget::ControlBody(Opcode::StartUpload, false),
        put_with(envelope(1, 2, &[(0x8001, vec![2])])),
        ErrorCategory::UNSUPPORTED_CAPABILITY,
        detail::capability::SCHEMA_VERSION,
        "Put schemas are version 1, patch 128, catalog 64 — registry constants, not a negotiation.",
    );
    push(
        "metadata-missing-a-required-field",
        NegativeTarget::ControlBody(Opcode::StartUpload, false),
        put_with(envelope(1, 1, &[])),
        ErrorCategory::INVALID_DESCRIPTOR,
        detail::descriptor::INVALID_COMBINATION,
        "Every registered required field appears exactly once; route Put requires its retention byte.",
    );
    push(
        "metadata-text-field-above-its-registered-length",
        NegativeTarget::ControlBody(Opcode::SetMetadata, false),
        {
            let mut body = zeros(36);
            bytes_at(&mut body, 0, &OP_A);
            u16_at(&mut body, 16, 1);
            u16_at(&mut body, 18, 1);
            u64_at(&mut body, 20, 9);
            u64_at(&mut body, 28, 42);
            body.extend_from_slice(&envelope(1, 128, &[(0x8003, vec![b'x'; 49])]));
            control_frame(Opcode::SetMetadata.to_u16(), REQUEST, 1, &body)
        },
        ErrorCategory::INVALID_DESCRIPTOR,
        detail::descriptor::NONCANONICAL_METADATA,
        "The route display name is registered at 1-48 bytes; 49 is a schema-disallowed width.",
    );
    push(
        "set-metadata-on-a-kind-with-no-patch-schema",
        NegativeTarget::ControlBody(Opcode::SetMetadata, false),
        {
            let mut body = zeros(36);
            bytes_at(&mut body, 0, &OP_A);
            u16_at(&mut body, 16, 2);
            u16_at(&mut body, 18, 1);
            u64_at(&mut body, 20, 4);
            u64_at(&mut body, 28, 12);
            body.extend_from_slice(&envelope(2, 128, &[(0x8001, vec![1])]));
            control_frame(Opcode::SetMetadata.to_u16(), REQUEST, 1, &body)
        },
        ErrorCategory::UNSUPPORTED_CAPABILITY,
        detail::capability::LOGICAL_KIND,
        "Trip, ride, weather and update package reject SetMetadata as unsupported.",
    );
    push(
        "catalog-projection-unknown-critical-field",
        NegativeTarget::ControlBody(Opcode::QueryCatalog, true),
        {
            let entry = catalog_entry(
                1,
                2,
                3,
                4,
                &envelope(
                    3,
                    64,
                    &[
                        (0x8001, 1_700_000_000i64.to_le_bytes().to_vec()),
                        (0x8002, 5400u32.to_le_bytes().to_vec()),
                        (0x8003, 42_000u32.to_le_bytes().to_vec()),
                        (0x8004, vec![1]),
                        (0x8055, vec![7]),
                    ],
                ),
            );
            control_frame(Opcode::QueryCatalog.to_u16(), OK, 1, &catalog_page(STORE, 3, 1, 12, &[0u8; 16], &entry))
        },
        ErrorCategory::INVALID_DESCRIPTOR,
        detail::descriptor::INVALID_COMBINATION,
        "A projection rejects an unknown critical field even though it may skip a well-formed noncritical one.",
    );
    push(
        "catalog-projection-oversized-for-its-registered-schema",
        NegativeTarget::ControlBody(Opcode::QueryCatalog, true),
        {
            // Every field here is legal on its own — the unknown noncritical one is even skippable —
            // and the envelope is still 54 bytes past ride's registered 41-byte maximum.
            let entry = catalog_entry(
                1,
                2,
                3,
                4,
                &envelope(
                    3,
                    64,
                    &[
                        (0x8001, 1_700_000_000i64.to_le_bytes().to_vec()),
                        (0x8002, 5400u32.to_le_bytes().to_vec()),
                        (0x8003, 42_000u32.to_le_bytes().to_vec()),
                        (0x8004, vec![1]),
                        (0x0055, vec![0x33; 50]),
                    ],
                ),
            );
            control_frame(Opcode::QueryCatalog.to_u16(), OK, 1, &catalog_page(STORE, 3, 1, 12, &[0u8; 16], &entry))
        },
        ErrorCategory::INVALID_DESCRIPTOR,
        detail::descriptor::NESTED_LENGTH,
        "A decoder rejects a schema-specific envelope larger than the registry's per-kind maximum.",
    );

    // ---- ResetStore admission (§16) ------------------------------------------------------------
    push(
        "reset-store-echo-mismatch",
        NegativeTarget::ResetStoreEcho(3),
        STORE_B.to_vec(),
        ErrorCategory::INVALID_DESCRIPTOR,
        detail::descriptor::INVALID_COMBINATION,
        "The echo MUST equal the StoreId the device currently reports, and it is checked before anything is deleted.",
    );
    push(
        "reset-store-zero-echo-in-a-class-that-reports-a-store",
        NegativeTarget::ResetStoreEcho(3),
        [0u8; 16].to_vec(),
        ErrorCategory::INVALID_DESCRIPTOR,
        detail::descriptor::INVALID_COMBINATION,
        "The all-zero form is admitted only in the two classes that report no StoreId at all.",
    );

    // ---- Streams -----------------------------------------------------------------------------
    let mut zero_session = stream_frame(0, 0, 1, 0, &[1]);
    u32_at(&mut zero_session, 0, 0);
    push(
        "stream-zero-session",
        NegativeTarget::StreamFrame,
        zero_session,
        ErrorCategory::INVALID_DESCRIPTOR,
        detail::descriptor::UNKNOWN_ENUM,
        "Every data frame carries a nonzero SessionId.",
    );
    push(
        "stream-wrong-direction-value",
        NegativeTarget::StreamFrame,
        stream_frame(1, 0, 4, 0, &[1]),
        ErrorCategory::INVALID_DESCRIPTOR,
        detail::descriptor::UNKNOWN_ENUM,
        "Directions are upload 1, download 2, status 3.",
    );
    push(
        "stream-zero-payload-on-a-data-direction",
        NegativeTarget::StreamFrame,
        stream_frame(1, 0, 1, 0, &[]),
        ErrorCategory::INVALID_FRAME,
        detail::frame::PAYLOAD_LENGTH,
        "Data directions have nonempty payload.",
    );
    push(
        "stream-flag-on-a-data-direction",
        NegativeTarget::StreamFrame,
        stream_frame(1, 0, 1, 1, &[1]),
        ErrorCategory::INVALID_FRAME,
        detail::frame::MALFORMED_HEADER,
        "Any nonzero flag on a data direction is rejected.",
    );
    push(
        "stream-reserved-flag-bit",
        NegativeTarget::StreamFrame,
        stream_frame(1, 0, 3, 1 << 4, &fault_body(8, 1, 0, 0, 0)),
        ErrorCategory::INVALID_FRAME,
        detail::frame::MALFORMED_HEADER,
        "Flags above bit 1 are undefined.",
    );
    push(
        "stream-terminal-without-fault",
        NegativeTarget::StreamFrame,
        stream_frame(1, 0, 3, 2, &fault_body(8, 1, 0, 0, 1)),
        ErrorCategory::INVALID_FRAME,
        detail::frame::MALFORMED_HEADER,
        "Terminal without fault is reserved: a stream has no successful terminal frame.",
    );
    push(
        "stream-status-with-no-flags",
        NegativeTarget::StreamFrame,
        stream_frame(1, 0, 3, 0, &fault_body(8, 1, 0, 0, 0)),
        ErrorCategory::INVALID_FRAME,
        detail::frame::MALFORMED_HEADER,
        "Status with neither flag is reserved.",
    );
    push(
        "stream-status-with-a-nonzero-offset",
        NegativeTarget::StreamFrame,
        stream_frame(1, 8, 3, 1, &fault_body(8, 1, 0, 0, 0)),
        ErrorCategory::INVALID_DESCRIPTOR,
        detail::descriptor::RESERVED_BITS,
        "Status direction has offset zero.",
    );
    push(
        "stream-nonterminal-disposition-with-the-terminal-bit",
        NegativeTarget::StreamFrame,
        stream_frame(1, 0, 3, 3, &fault_body(8, 1, 0, 0, 0)),
        ErrorCategory::INVALID_DESCRIPTOR,
        detail::descriptor::INVALID_COMBINATION,
        "Disposition 0 is the nonterminal form.",
    );
    push(
        "stream-terminal-disposition-without-the-terminal-bit",
        NegativeTarget::StreamFrame,
        stream_frame(1, 0, 3, 1, &fault_body(8, 1, 0, 0, 1)),
        ErrorCategory::INVALID_DESCRIPTOR,
        detail::descriptor::INVALID_COMBINATION,
        "Dispositions 1 and 2 are the terminal ones.",
    );
    let mut truncated_stream = stream_frame(1, 0, 1, 0, &[1, 2, 3, 4]);
    truncated_stream.pop();
    push(
        "stream-truncated-payload",
        NegativeTarget::StreamFrame,
        truncated_stream,
        ErrorCategory::INVALID_FRAME,
        detail::frame::PAYLOAD_LENGTH,
        "A payload length that disagrees with the record is invalidFrame.",
    );
    let mut overlong_stream = stream_frame(1, 0, 1, 0, &[1, 2, 3, 4]);
    overlong_stream.push(0);
    push(
        "stream-overlong-payload",
        NegativeTarget::StreamFrame,
        overlong_stream,
        ErrorCategory::INVALID_FRAME,
        detail::frame::PAYLOAD_LENGTH,
        "Trailing bytes past the stated payload are invalidFrame.",
    );
    push(
        "stream-offset-plus-length-overflows",
        NegativeTarget::StreamFrame,
        stream_frame(1, u64::MAX, 1, 0, &[1, 2]),
        ErrorCategory::INVALID_FRAME,
        detail::frame::PAYLOAD_LENGTH,
        "offset + length must not overflow the u64 space.",
    );
    let mut bad_fault_reserved = stream_frame(1, 0, 3, 1, &fault_body(8, 1, 0, 0, 0));
    bad_fault_reserved[16 + 21] = 1;
    push(
        "stream-fault-reserved-byte",
        NegativeTarget::StreamFrame,
        bad_fault_reserved,
        ErrorCategory::INVALID_DESCRIPTOR,
        detail::descriptor::RESERVED_BITS,
        "The fault body's three trailing bytes are reserved.",
    );
    push(
        "stream-fault-semantic-category",
        NegativeTarget::StreamFrame,
        stream_frame(1, 0, 3, 1, &fault_body(14, 5, 0, 0, 0)),
        ErrorCategory::INVALID_DESCRIPTOR,
        detail::descriptor::UNKNOWN_ENUM,
        "Only namespace-zero transport categories are valid in the compact body, which has no namespace field a \
         semantic detail could be scoped to.",
    );
    push(
        "stream-fault-domain-category",
        NegativeTarget::StreamFrame,
        stream_frame(1, 0, 3, 1, &fault_body(11, 1, 0, 0, 0)),
        ErrorCategory::INVALID_DESCRIPTOR,
        detail::descriptor::UNKNOWN_ENUM,
        "A revisionConflict is a domain outcome and uses a correlated control response, not a stream fault.",
    );

    all
}

// ---------------------------------------------------------------------------------------------
// Transcripts.
// ---------------------------------------------------------------------------------------------

fn event(actor: &'static str, channel: &'static str, note: &str, record: Option<Vec<u8>>) -> Event {
    Event { actor, principal: "companion-ble", link: "ble", generation: 1, note: note.to_string(), record, channel }
}

fn event_in(generation: u32, actor: &'static str, channel: &'static str, note: &str, record: Option<Vec<u8>>) -> Event {
    Event { generation, ..event(actor, channel, note, record) }
}

/// The nine semantic transcripts issue #1358 requires.
pub fn transcripts() -> Vec<Transcript> {
    let payload = route_payload();
    let payload_crc = crc32(&payload);
    let payload_len = payload.len() as u64;
    let session = 0x0000_0011u32;
    let mut all = Vec::new();

    // 1. Create.
    all.push(Transcript {
        name: "create-upload-publish-and-download".to_string(),
        description: "Create upload, checkpoints, seal, publish, catalog query, and download.".to_string(),
        events: vec![
            event(
                "client",
                "control",
                "Hello negotiates the wire major; nothing may be admitted before it.",
                Some(control_frame(1, REQUEST, 1, &hello(3, 3, 244, 1024, 0, 0))),
            ),
            event(
                "device",
                "control",
                "Capabilities answers with the resource page and sets `more` because subjects exist.",
                Some(control_frame(
                    1,
                    OK_MORE,
                    1,
                    &capabilities(
                        0b0011,
                        Some(STORE),
                        1,
                        true,
                        7,
                        ALL_COMMANDS,
                        8,
                        0,
                        0,
                        0,
                        1,
                        &resource_limits(3_221_225_472),
                    ),
                )),
            ),
            event(
                "client",
                "control",
                "StartUpload claims the OperationId before any payload byte exists.",
                Some(control_frame(
                    0x0100,
                    REQUEST,
                    2,
                    &start_upload(OP_A, 1, 0, 1, 0, 0, payload_len, payload_crc, &route_put(2)),
                )),
            ),
            event(
                "device",
                "control",
                "UploadAccepted issues a fresh SessionId at durable offset zero.",
                Some(control_frame(0x0100, OK, 2, &upload_accepted(0, 0, OP_A, session, 9, 41, 0, 262_144, 1008, 0))),
            ),
            event(
                "client",
                "stream",
                "The first data frame, at exactly the session's next offset.",
                Some(stream_frame(session, 0, 1, 0, &payload[..1008])),
            ),
            event(
                "client",
                "stream",
                "The middle frame.",
                Some(stream_frame(session, 1008, 1, 0, &payload[1008..2016])),
            ),
            event("client", "stream", "The final frame.", Some(stream_frame(session, 2016, 1, 0, &payload[2016..]))),
            event(
                "client",
                "control",
                "A checkpoint at the declared end.",
                Some(control_frame(0x0101, REQUEST, 3, &{
                    let mut body = zeros(12);
                    u32_at(&mut body, 0, session);
                    u64_at(&mut body, 4, payload_len);
                    body
                })),
            ),
            event(
                "device",
                "control",
                "The response is emitted only after the bytes and the work record are durable.",
                Some(control_frame(0x0101, OK, 3, &{
                    let mut body = zeros(20);
                    u32_at(&mut body, 0, session);
                    u64_at(&mut body, 4, payload_len);
                    u32_at(&mut body, 12, payload_crc);
                    u32_at(&mut body, 16, 1);
                    body
                })),
            ),
            event(
                "client",
                "control",
                "FinishUpload seals, validates, and publishes.",
                Some(control_frame(0x0102, REQUEST, 4, &session.to_le_bytes())),
            ),
            event(
                "device",
                "control",
                "Publication and terminal success are one durable commit.",
                Some(control_frame(0x0102, OK, 4, &committed_route_result(OP_A, 9, 42, payload_len, payload_crc))),
            ),
            event(
                "client",
                "control",
                "The catalog now reports the new head at the revision the result named.",
                Some(control_frame(0x0201, REQUEST, 5, &{
                    let mut body = zeros(28);
                    u16_at(&mut body, 0, 1);
                    body
                })),
            ),
            event(
                "device",
                "control",
                "One entry, whose Revision is the CAS token for a later mutation.",
                Some(control_frame(
                    0x0201,
                    OK,
                    5,
                    &catalog_page(
                        STORE,
                        1,
                        1,
                        42,
                        &[0u8; 16],
                        &catalog_entry(
                            9,
                            42,
                            payload_len,
                            payload_crc,
                            &route_catalog("Kaiserstuhl loop", 2, Some(false), Some(1_700_000_000)),
                        ),
                    ),
                )),
            ),
            event(
                "client",
                "control",
                "A download resolves that same head.",
                Some(control_frame(0x0110, REQUEST, 6, &{
                    let mut body = zeros(28);
                    u16_at(&mut body, 0, 1);
                    u64_at(&mut body, 4, 9);
                    body
                })),
            ),
            event(
                "device",
                "control",
                "DownloadAccepted pins the head and reports its length and CRC.",
                Some(control_frame(0x0110, OK, 6, &{
                    let mut body = zeros(60);
                    bytes_at(&mut body, 0, &STORE);
                    u32_at(&mut body, 16, 0x21);
                    u64_at(&mut body, 20, 9);
                    u64_at(&mut body, 28, 42);
                    u64_at(&mut body, 36, payload_len);
                    u32_at(&mut body, 44, payload_crc);
                    u16_at(&mut body, 56, 1008);
                    body
                })),
            ),
        ],
    });

    // 2. Replace conflict.
    all.push(Transcript {
        name: "replace-conflict-at-the-commit-lock".to_string(),
        description: "Replace admitted, a concurrent mutation wins, and the publication CAS recheck rejects the stale replace.".to_string(),
        events: vec![
            event("client", "control", "Replace is admitted against the revision the catalog reported.", Some(control_frame(0x0100, REQUEST, 10, &start_upload(OP_B, 1, 1, 0, 9, 42, payload_len, payload_crc, &route_put(2))))),
            event("device", "control", "Admission passes; the reported repository revision is a diagnostic snapshot, not the next CAS token.", Some(control_frame(0x0100, OK, 10, &upload_accepted(1, 0, OP_B, session, 9, 42, 0, 262_144, 1008, 0)))),
            event("device", "injected", "A device-local producer publishes a competing mutation, advancing the entry to revision 43.", None),
            event("client", "stream", "The client streams its bytes, unaware.", Some(stream_frame(session, 0, 1, 0, &payload[..1008]))),
            event("client", "control", "FinishUpload rechecks the expected revision under the store commit lock.", Some(control_frame(0x0102, REQUEST, 11, &session.to_le_bytes()))),
            event("device", "control", "revisionConflict with the authoritative current revision; the old head is unchanged and the operation is durably aborted.", Some(control_frame(0x0102, ERR, 11, &error_body(11, 0, 1, RetryGuidance::REFRESH.get(), 0, presence::CURRENT_REVISION | presence::DURABLE_CLAIM_EXISTS | presence::CLAIM_IS_TERMINAL, 0, 0, 43, 0, 0, &[])))),
            event("client", "control", "QueryOperation confirms the terminal Aborted state.", Some(control_frame(0x0200, REQUEST, 12, &OP_B))),
            event("device", "control", "The retained bare body carries both claim-status bits and no text.", Some(control_frame(0x0200, OK, 12, &operation_status(3, &bare_error(11, 1, presence::DURABLE_CLAIM_EXISTS | presence::CLAIM_IS_TERMINAL))))),
        ],
    });

    // 3. Lost result.
    all.push(Transcript {
        name: "lost-result-then-query-operation".to_string(),
        description: "The committed terminal response is lost, the client reconnects, QueryOperation returns the exact retained result, and the retry creates no second commit.".to_string(),
        events: vec![
            event("client", "control", "FinishUpload on a session whose publication will succeed.", Some(control_frame(0x0102, REQUEST, 20, &session.to_le_bytes()))),
            event("device", "injected", "The publication commits durably; the response frame is lost on the link.", None),
            event("client", "injected", "The link drops with the mutation outstanding.", None),
            event_in(2, "client", "control", "After reconnecting, QueryOperation is the first operation-bearing request.", Some(control_frame(0x0200, REQUEST, 1, &OP_A))),
            event_in(2, "device", "control", "The exact retained result comes back; a lost response is unknown delivery, not a failed mutation.", Some(control_frame(0x0200, OK, 1, &operation_status(2, &committed_route_result(OP_A, 9, 42, payload_len, payload_crc))))),
            event_in(2, "client", "control", "Reissuing the same OperationId with identical intent must not write again.", Some(control_frame(0x0100, REQUEST, 2, &start_upload(OP_A, 1, 0, 1, 0, 0, payload_len, payload_crc, &route_put(2))))),
            event_in(2, "device", "control", "Disposition 1 replays the same result; no generation and no second commit.", Some(control_frame(0x0100, OK, 2, &{ let mut body = vec![1u8, 0, 0, 0]; body.extend_from_slice(&committed_route_result(OP_A, 9, 42, payload_len, payload_crc)); body }))),
            event_in(2, "client", "control", "The same OperationId with a different intent is a hard conflict.", Some(control_frame(0x0100, REQUEST, 3, &start_upload(OP_A, 1, 0, 1, 0, 0, payload_len + 1, payload_crc, &route_put(2))))),
            event_in(2, "device", "control", "operationIdConflict, with both claim-status bits clear and new-ID guidance.", Some(control_frame(0x0100, ERR, 3, &error_body(19, 0, 1, RetryGuidance::NEW_ID_FOR_NEW_INTENT.get(), 0, 0, 0, 0, 0, 0, 0, &[])))),
        ],
    });

    // 4. Resume.
    all.push(Transcript {
        name: "disconnect-reboot-and-resume".to_string(),
        description: "A disconnect after uncheckpointed bytes, a reboot, a resume from the last durable offset, and exactly one final publication.".to_string(),
        events: vec![
            event("client", "control", "A checkpoint makes the first granule durable.", Some(control_frame(0x0101, REQUEST, 30, &{ let mut body = zeros(12); u32_at(&mut body, 0, session); u64_at(&mut body, 4, 262_144); body }))),
            event("device", "control", "Sequence 1, with the finalized CRC of exactly that prefix.", Some(control_frame(0x0101, OK, 30, &{ let mut body = zeros(20); u32_at(&mut body, 0, session); u64_at(&mut body, 4, 262_144); u32_at(&mut body, 12, 0x1357_9BDF); u32_at(&mut body, 16, 1); body }))),
            event("client", "stream", "More bytes are sent and never checkpointed.", Some(stream_frame(session, 262_144, 1, 0, &[0xEE; 512]))),
            event("client", "injected", "The link drops, then the device reboots. Payload beyond the last checkpoint may be discarded.", None),
            event_in(2, "client", "control", "StartUpload repeats the same OperationId and intent with resume permitted.", Some(control_frame(0x0100, REQUEST, 1, &start_upload(OP_A, 1, 0, 1, 0, 0, payload_len, payload_crc, &route_put(2))))),
            event_in(2, "device", "control", "Resumed-work is set, the durable next offset is the last checkpoint, and the acceptance carries that prefix's CRC.", Some(control_frame(0x0100, OK, 1, &upload_accepted(0, 1, OP_A, 0x0000_0012, 9, 41, 262_144, 262_144, 1008, 0x1357_9BDF)))),
            event_in(2, "client", "stream", "The client compares its retained prefix CRC against the field and only then sends new bytes.", Some(stream_frame(0x0000_0012, 262_144, 1, 0, &[0x11, 0x22, 0x33, 0x44]))),
            event_in(2, "client", "control", "FinishUpload publishes exactly once.", Some(control_frame(0x0102, REQUEST, 2, &0x0000_0012u32.to_le_bytes()))),
            event_in(2, "device", "control", "One terminal ObjectResult for the whole resumed transfer.", Some(control_frame(0x0102, OK, 2, &committed_route_result(OP_A, 9, 42, payload_len, payload_crc)))),
        ],
    });

    // 4b. The bounded exactly-once window and its eviction boundary.
    all.push(Transcript {
        name: "result-window-eviction-boundary".to_string(),
        description: "The retained-result window fills, the operation's result survives 63 newer terminals, the 64th \
                      evicts it, and Unknown is then reconciled against the catalog rather than replayed."
            .to_string(),
        events: vec![
            event(
                "device",
                "control",
                "The create commits and its result occupies one of the 64 retained slots.",
                Some(control_frame(0x0102, OK, 90, &committed_route_result(OP_A, 9, 42, payload_len, payload_crc))),
            ),
            event(
                "device",
                "injected",
                "63 later terminal operations commit. The window is store-global, so device-local ride, weather, \
                 update-state and import results fill it exactly as a link client's do.",
                None,
            ),
            event(
                "client",
                "control",
                "QueryOperation after 63 newer terminals.",
                Some(control_frame(0x0200, REQUEST, 91, &OP_A)),
            ),
            event(
                "device",
                "control",
                "Still Committed: retention is proven at 63 rather than assumed.",
                Some(control_frame(
                    0x0200,
                    OK,
                    91,
                    &operation_status(2, &committed_route_result(OP_A, 9, 42, payload_len, payload_crc)),
                )),
            ),
            event(
                "device",
                "injected",
                "The 64th newer terminal record commits and deterministically evicts the oldest.",
                None,
            ),
            event("client", "control", "The same query again.", Some(control_frame(0x0200, REQUEST, 92, &OP_A))),
            event(
                "device",
                "control",
                "Unknown. It cannot distinguish never-claimed from evicted, and the device cannot close that hole \
                 on the client's behalf.",
                Some(control_frame(0x0200, OK, 92, &operation_status(0, &[]))),
            ),
            event(
                "client",
                "control",
                "So the client reconciles domain state instead of replaying: a create carries no prior Revision, so \
                 a blind reissue under a fresh OperationId would publish a second object.",
                Some(control_frame(0x0201, REQUEST, 93, &{
                    let mut body = zeros(28);
                    u16_at(&mut body, 0, 1);
                    body
                })),
            ),
            event(
                "device",
                "control",
                "The catalog shows the head is already there, so nothing is reissued and the OperationId is never \
                 reused.",
                Some(control_frame(
                    0x0201,
                    OK,
                    93,
                    &catalog_page(
                        STORE,
                        1,
                        1,
                        42,
                        &[0u8; 16],
                        &catalog_entry(
                            9,
                            42,
                            payload_len,
                            payload_crc,
                            &route_catalog("Kaiserstuhl loop", 2, Some(false), Some(1_700_000_000)),
                        ),
                    ),
                )),
            ),
        ],
    });

    // 4c. The draft machinery, which is the most stateful surface the wire has.
    let part = draft_part_payload();
    let part_crc = crc32(&part);
    let manifest = manifest_payload();
    let manifest_crc = crc32(&manifest);
    let part_session = 0x0000_0031u32;
    let manifest_session = 0x0000_0041u32;
    all.push(Transcript {
        name: "draft-begin-parts-finalize-and-paging".to_string(),
        description: "BeginDraft, a child part streamed and sealed, snapshot paging over the draft, a second \
                      BeginDraft refused while the parent is open, atomic finalization, and the explicit selection \
                      that follows an initially unselected release."
            .to_string(),
        events: vec![
            event(
                "client",
                "control",
                "BeginDraft binds target, expected revision, manifest length and CRC, and the exact child count.",
                Some(control_frame(0x0130, REQUEST, 100, &{
                    let mut body = zeros(52);
                    bytes_at(&mut body, 0, &OP_PARENT);
                    u16_at(&mut body, 16, 6);
                    u64_at(&mut body, 36, manifest.len() as u64);
                    u32_at(&mut body, 44, manifest_crc);
                    u16_at(&mut body, 48, 1);
                    body
                })),
            ),
            event(
                "device",
                "control",
                "The parent opens at draft revision 1 and consumes no terminal-result slot.",
                Some(control_frame(0x0130, OK, 100, &{
                    let mut body = zeros(32);
                    bytes_at(&mut body, 4, &OP_PARENT);
                    u64_at(&mut body, 20, 1);
                    u16_at(&mut body, 28, 1);
                    body
                })),
            ),
            event(
                "client",
                "control",
                "A second BeginDraft while that parent is open.",
                Some(control_frame(0x0130, REQUEST, 101, &{
                    let mut body = zeros(52);
                    bytes_at(&mut body, 0, &OP_B);
                    u16_at(&mut body, 16, 6);
                    u64_at(&mut body, 36, 264);
                    u32_at(&mut body, 44, 1);
                    u16_at(&mut body, 48, 1);
                    body
                })),
            ),
            event(
                "device",
                "control",
                "Refused busy/draftParents before any claim, reporting that parent's owner — an ownership refusal, \
                 not a compiled-capacity failure.",
                Some(control_frame(
                    0x0130,
                    ERR,
                    101,
                    &error_body(5, 0, 4, RetryGuidance::RETRY_AFTER_OWNER_RELEASE.get(), 1, 0, 0, 0, 0, 0, 0, &[]),
                )),
            ),
            event(
                "client",
                "control",
                "StartDraftPart durably claims the child; (kind, key) is unique within the parent.",
                Some(control_frame(0x0131, REQUEST, 102, &{
                    let mut body = zeros(64);
                    bytes_at(&mut body, 0, &OP_CHILD);
                    bytes_at(&mut body, 16, &OP_PARENT);
                    u16_at(&mut body, 32, 1);
                    u64_at(&mut body, 36, 1);
                    u64_at(&mut body, 44, part.len() as u64);
                    u32_at(&mut body, 52, part_crc);
                    body[56] = 1;
                    body
                })),
            ),
            event(
                "device",
                "control",
                "DraftPartAccepted returns only a session and a durable offset: the opaque ref does not exist yet.",
                Some(control_frame(0x0131, OK, 102, &{
                    let mut body = zeros(72);
                    bytes_at(&mut body, 4, &OP_CHILD);
                    bytes_at(&mut body, 20, &OP_PARENT);
                    u32_at(&mut body, 36, part_session);
                    u16_at(&mut body, 40, 1);
                    u64_at(&mut body, 44, 1);
                    u32_at(&mut body, 60, FIXTURE_GRANULE);
                    u16_at(&mut body, 64, 1008);
                    body
                })),
            ),
            event(
                "client",
                "stream",
                "The part's first bytes.",
                Some(stream_frame(part_session, 0, 1, 0, &part[..1008])),
            ),
            event(
                "client",
                "control",
                "FinishUpload seals the part.",
                Some(control_frame(0x0102, REQUEST, 103, &part_session.to_le_bytes())),
            ),
            event(
                "device",
                "control",
                "Sealing mints the DraftPartRef and returns a DraftPartResult — never a logical result.",
                Some(control_frame(
                    0x0102,
                    OK,
                    103,
                    &result_envelope(
                        2,
                        &draft_part_result(OP_CHILD, STORE, OP_PARENT, PART_REF, 1, 1, part.len() as u64, part_crc),
                    ),
                )),
            ),
            event(
                "client",
                "control",
                "QueryDraft pages the parent's children under the draft-revision snapshot.",
                Some(control_frame(0x0202, REQUEST, 104, &{
                    let mut body = zeros(44);
                    bytes_at(&mut body, 0, &OP_PARENT);
                    body[18] = 6;
                    body
                })),
            ),
            event(
                "device",
                "control",
                "Draft revision 3: it moved when the child was claimed and again when it sealed, and never for a \
                 payload checkpoint. The sealed entry is the only one carrying a ref.",
                Some(control_frame(0x0202, OK, 104, &{
                    let mut body = zeros(44);
                    bytes_at(&mut body, 0, &OP_PARENT);
                    u64_at(&mut body, 16, 3);
                    body[40] = 1;
                    body.extend_from_slice(&draft_entry(
                        OP_CHILD,
                        PART_REF,
                        1,
                        1,
                        2,
                        part.len() as u64,
                        part.len() as u64,
                        part_crc,
                    ));
                    body
                })),
            ),
            event(
                "client",
                "control",
                "FinalizeDraft addresses the existing claim by OperationId alone.",
                Some(control_frame(0x0132, REQUEST, 105, &OP_PARENT)),
            ),
            event(
                "device",
                "control",
                "The manifest acceptance issues a fresh session for the bound manifest stream.",
                Some(control_frame(0x0132, OK, 105, &{
                    let mut body = zeros(64);
                    bytes_at(&mut body, 4, &OP_PARENT);
                    u32_at(&mut body, 20, manifest_session);
                    u64_at(&mut body, 24, 2);
                    u64_at(&mut body, 32, 50);
                    u32_at(&mut body, 48, FIXTURE_GRANULE);
                    u16_at(&mut body, 52, 1008);
                    body
                })),
            ),
            event(
                "client",
                "stream",
                "The manifest bytes, which name exactly the sealed refs of this parent.",
                Some(stream_frame(manifest_session, 0, 1, 0, &manifest)),
            ),
            event(
                "client",
                "control",
                "FinishUpload verifies the manifest against BeginDraft's declared length and CRC.",
                Some(control_frame(0x0102, REQUEST, 106, &manifest_session.to_le_bytes())),
            ),
            event(
                "device",
                "control",
                "One commit publishes the manifest and every referenced part; no physical generation is exposed.",
                Some(control_frame(
                    0x0102,
                    OK,
                    106,
                    &result_envelope(
                        1,
                        &object_result(OP_PARENT, STORE, 6, 0, 2, 51, manifest.len() as u64, manifest_crc),
                    ),
                )),
            ),
            event(
                "client",
                "control",
                "Initial publication derives selected false, so selecting the release is a separate \
                 compare-and-swap SetMetadata.",
                Some(control_frame(0x0301, REQUEST, 107, &{
                    let mut body = zeros(36);
                    bytes_at(&mut body, 0, &OP_B);
                    u16_at(&mut body, 16, 6);
                    u16_at(&mut body, 18, 1);
                    u64_at(&mut body, 20, 2);
                    u64_at(&mut body, 28, 51);
                    body.extend_from_slice(&volume_patch(true));
                    body
                })),
            ),
            event(
                "device",
                "control",
                "metadataChanged at the next revision.",
                Some(control_frame(
                    0x0301,
                    OK,
                    107,
                    &result_envelope(1, &object_result(OP_B, STORE, 6, 3, 2, 52, manifest.len() as u64, manifest_crc)),
                )),
            ),
        ],
    });

    // 5. Abort.
    all.push(Transcript {
        name: "abort-session-retains-work-abort-operation-abandons-it".to_string(),
        description: "AbortSession retains resumable work; AbortOperation durably abandons it and repeating the command is idempotent.".to_string(),
        events: vec![
            event("client", "control", "AbortSession detaches the session, client-cancelled.", Some(control_frame(0x0120, REQUEST, 40, &{ let mut body = zeros(8); u32_at(&mut body, 0, session); body[4] = 1; body }))),
            event("device", "control", "Outcome 0: detached. A resumable upload keeps its durable work.", Some(control_frame(0x0120, OK, 40, &[0]))),
            event("client", "control", "QueryOperation still reports the claim as live.", Some(control_frame(0x0200, REQUEST, 41, &OP_A))),
            event("device", "control", "InProgress, resumable, with no session attached.", Some(control_frame(0x0200, OK, 41, &operation_status(1, &progress(1, 0, 0b101, 1, 9, 262_144))))),
            event("client", "control", "AbortOperation claims its own OperationId in the reserved cancellation slot.", Some(control_frame(0x0302, REQUEST, 42, &{ let mut body = zeros(40); bytes_at(&mut body, 0, &OP_ABORT); bytes_at(&mut body, 16, &OP_A); body[32] = 3; body }))),
            event("device", "control", "The AbortResult is committed only after the target is durably Aborted.", Some(control_frame(0x0302, OK, 42, &result_envelope(3, &abort_result(OP_ABORT, STORE, OP_A, 0))))),
            event("client", "control", "Repeating the abort command is idempotent by its own OperationId.", Some(control_frame(0x0302, REQUEST, 43, &{ let mut body = zeros(40); bytes_at(&mut body, 0, &OP_ABORT); bytes_at(&mut body, 16, &OP_A); body[32] = 3; body }))),
            event("device", "control", "The same AbortResult, unchanged.", Some(control_frame(0x0302, OK, 43, &result_envelope(3, &abort_result(OP_ABORT, STORE, OP_A, 0))))),
            event("client", "control", "The target itself never receives an AbortResult; its status is the retained bare body.", Some(control_frame(0x0200, REQUEST, 44, &OP_A))),
            event("device", "control", "Aborted, cancelled/userRequested, both claim-status bits set.", Some(control_frame(0x0200, OK, 44, &operation_status(3, &bare_error(17, 3, presence::DURABLE_CLAIM_EXISTS | presence::CLAIM_IS_TERMINAL))))),
        ],
    });

    // 6. Wrong-owner teardown.
    all.push(Transcript {
        name: "wrong-owner-cannot-advance-or-release-a-session".to_string(),
        description: "A stale same-link owner, a wrong-link owner, and a wrong principal all fail to advance or release the current session.".to_string(),
        events: vec![
            event("device", "control", "A session is live under connection generation 1.", Some(control_frame(0x0100, OK, 50, &upload_accepted(0, 0, OP_A, session, 9, 41, 0, 262_144, 1008, 0)))),
            event("client", "injected", "The link drops and reconnects, creating generation 2. Every earlier SessionId is now stale.", None),
            event_in(2, "client", "control", "A FinishUpload bearing the stale SessionId.", Some(control_frame(0x0102, REQUEST, 1, &session.to_le_bytes()))),
            event_in(2, "device", "control", "invalidSession/staleConnection; nothing advances and nothing is released.", Some(control_frame(0x0102, ERR, 1, &error_body(9, 0, 2, RetryGuidance::RECONNECT_THEN_QUERY.get(), 0, presence::DURABLE_CLAIM_EXISTS, 0, 0, 0, 0, 0, &[])))),
            Event { link: "usb", principal: "usb-local", ..event_in(2, "client", "control", "The same SessionId offered on the other link kind.", Some(control_frame(0x0101, REQUEST, 2, &{ let mut body = zeros(12); u32_at(&mut body, 0, session); u64_at(&mut body, 4, 262_144); body }))) },
            Event { link: "usb", principal: "usb-local", ..event_in(2, "device", "control", "invalidSession/wrongLink: a SessionId is valid only with its link kind, principal scope, and generation.", Some(control_frame(0x0101, ERR, 2, &error_body(9, 0, 4, RetryGuidance::RECONNECT_THEN_QUERY.get(), 0, 0, 0, 0, 0, 0, 0, &[])))) },
            Event { principal: "other-companion", ..event_in(2, "client", "control", "A different principal queries the operation.", Some(control_frame(0x0200, REQUEST, 3, &OP_A))) },
            Event { principal: "other-companion", ..event_in(2, "device", "control", "authorizationFailed, not status: authorization precedes operation-status facts.", Some(control_frame(0x0200, ERR, 3, &error_body(4, 0, 2, RetryGuidance::RETRY_AFTER_USER_ACTION.get(), 0, 0, 0, 0, 0, 0, 0, &[])))) },
            event_in(2, "device", "stream", "A frame bearing a session released earlier in this generation is silently discarded; one released in an earlier generation is stale by the generation check alone.", Some(stream_frame(session, 0, 1, 0, &[0x01]))),
        ],
    });

    // 7. Download pinning.
    all.push(Transcript {
        name: "download-pin-survives-replace-and-delete".to_string(),
        description: "A download's pinned bytes survive a replace and a delete, and become collectible only after the matching release.".to_string(),
        events: vec![
            event("client", "control", "StartDownload resolves the current head.", Some(control_frame(0x0110, REQUEST, 60, &{ let mut body = zeros(28); u16_at(&mut body, 0, 1); u64_at(&mut body, 4, 9); body }))),
            event("device", "control", "Resolve and lease happen before this response; the pinned revision is 42.", Some(control_frame(0x0110, OK, 60, &{ let mut body = zeros(60); bytes_at(&mut body, 0, &STORE); u32_at(&mut body, 16, 0x21); u64_at(&mut body, 20, 9); u64_at(&mut body, 28, 42); u64_at(&mut body, 36, payload_len); u32_at(&mut body, 44, payload_crc); u16_at(&mut body, 56, 1008); body }))),
            event("client", "stream", "Streaming begins from the pinned generation.", Some(stream_frame(0x21, 0, 2, 0, &payload[..1008]))),
            event("device", "injected", "A replace publishes revision 43 and a delete follows. Visibility changes; the pinned bytes do not.", None),
            event("client", "stream", "The remaining frames still come from the pinned generation.", Some(stream_frame(0x21, 1008, 2, 0, &payload[1008..2016]))),
            event("client", "control", "FinishDownload verifies the whole-source length and CRC.", Some(control_frame(0x0111, REQUEST, 61, &{ let mut body = zeros(16); u32_at(&mut body, 0, 0x21); u64_at(&mut body, 4, payload_len); u32_at(&mut body, 12, payload_crc); body }))),
            event("device", "control", "The empty success releases the lease exactly once; only now is the displaced generation collectible.", Some(control_frame(0x0111, OK, 61, &[]))),
            event("client", "control", "The catalog reports the object as gone.", Some(control_frame(0x0201, REQUEST, 62, &{ let mut body = zeros(28); u16_at(&mut body, 0, 1); body }))),
            event("device", "control", "An empty page at the revision the delete produced.", Some(control_frame(0x0201, OK, 62, &catalog_page(STORE, 1, 0, 44, &[0u8; 16], &[])))),
        ],
    });

    // 8. Delete.
    all.push(Transcript {
        name: "delete-lost-result-and-pinned-reader-continuity".to_string(),
        description: "DeleteObject's result is lost and recovered by query, while a pinned reader keeps streaming."
            .to_string(),
        events: vec![
            event(
                "client",
                "control",
                "DeleteObject with the entry's exact expected revision.",
                Some(control_frame(0x0300, REQUEST, 70, &{
                    let mut body = zeros(36);
                    bytes_at(&mut body, 0, &OP_B);
                    u16_at(&mut body, 16, 1);
                    u16_at(&mut body, 18, 1);
                    u64_at(&mut body, 20, 9);
                    u64_at(&mut body, 28, 42);
                    body
                })),
            ),
            event("device", "injected", "The catalog transaction commits; the response is lost.", None),
            event(
                "client",
                "control",
                "QueryOperation on the same OperationId.",
                Some(control_frame(0x0200, REQUEST, 71, &OP_B)),
            ),
            event(
                "device",
                "control",
                "Committed, outcome deleted, with the deleted old head's length and CRC.",
                Some(control_frame(
                    0x0200,
                    OK,
                    71,
                    &operation_status(
                        2,
                        &result_envelope(1, &object_result(OP_B, STORE, 1, 2, 9, 44, payload_len, payload_crc)),
                    ),
                )),
            ),
            event(
                "client",
                "control",
                "Reissuing the identical delete returns the same result and writes nothing.",
                Some(control_frame(0x0300, REQUEST, 72, &{
                    let mut body = zeros(36);
                    bytes_at(&mut body, 0, &OP_B);
                    u16_at(&mut body, 16, 1);
                    u16_at(&mut body, 18, 1);
                    u64_at(&mut body, 20, 9);
                    u64_at(&mut body, 28, 42);
                    body
                })),
            ),
            event(
                "device",
                "control",
                "The same ObjectResult, replayed.",
                Some(control_frame(
                    0x0300,
                    OK,
                    72,
                    &result_envelope(1, &object_result(OP_B, STORE, 1, 2, 9, 44, payload_len, payload_crc)),
                )),
            ),
            event(
                "client",
                "stream",
                "A reader pinned before the delete keeps streaming its generation.",
                Some(stream_frame(0x21, 2016, 2, 0, &payload[2016..])),
            ),
        ],
    });

    // 9. Metadata update.
    all.push(Transcript {
        name: "set-metadata-compare-and-swap-and-lost-result".to_string(),
        description:
            "SetMetadata's compare-and-swap, its lost result recovered by query, and the absence of any sidecar state."
                .to_string(),
        events: vec![
            event(
                "client",
                "control",
                "A patch carrying retention, selected, and a display name.",
                Some(control_frame(0x0301, REQUEST, 80, &{
                    let mut body = zeros(36);
                    bytes_at(&mut body, 0, &OP_A);
                    u16_at(&mut body, 16, 1);
                    u16_at(&mut body, 18, 1);
                    u64_at(&mut body, 20, 9);
                    u64_at(&mut body, 28, 42);
                    body.extend_from_slice(&route_patch(Some(3), Some(true), Some("Kaiserstuhl loop")));
                    body
                })),
            ),
            event("device", "injected", "The catalog commit succeeds; the response is lost.", None),
            event("client", "control", "QueryOperation recovers it.", Some(control_frame(0x0200, REQUEST, 81, &OP_A))),
            event(
                "device",
                "control",
                "Committed, outcome metadataChanged, at the new revision.",
                Some(control_frame(
                    0x0200,
                    OK,
                    81,
                    &operation_status(
                        2,
                        &result_envelope(1, &object_result(OP_A, STORE, 1, 3, 9, 45, payload_len, payload_crc)),
                    ),
                )),
            ),
            event(
                "client",
                "control",
                "The catalog projection carries the new facts — there is no sidecar to read.",
                Some(control_frame(0x0201, REQUEST, 82, &{
                    let mut body = zeros(28);
                    u16_at(&mut body, 0, 1);
                    body
                })),
            ),
            event(
                "device",
                "control",
                "The entry now reports the patched name, retention, and selected flag.",
                Some(control_frame(
                    0x0201,
                    OK,
                    82,
                    &catalog_page(
                        STORE,
                        1,
                        1,
                        45,
                        &[0u8; 16],
                        &catalog_entry(
                            9,
                            45,
                            payload_len,
                            payload_crc,
                            &route_catalog("Kaiserstuhl loop", 3, Some(true), Some(1_700_000_000)),
                        ),
                    ),
                )),
            ),
            event(
                "client",
                "control",
                "A second patch at the stale revision is a compare-and-swap failure.",
                Some(control_frame(0x0301, REQUEST, 83, &{
                    let mut body = zeros(36);
                    bytes_at(&mut body, 0, &OP_B);
                    u16_at(&mut body, 16, 1);
                    u16_at(&mut body, 18, 1);
                    u64_at(&mut body, 20, 9);
                    u64_at(&mut body, 28, 42);
                    body.extend_from_slice(&route_patch(None, Some(false), None));
                    body
                })),
            ),
            event(
                "device",
                "control",
                "revisionConflict reporting the authoritative current revision.",
                Some(control_frame(
                    0x0301,
                    ERR,
                    83,
                    &error_body(
                        11,
                        0,
                        1,
                        RetryGuidance::REFRESH.get(),
                        0,
                        presence::CURRENT_REVISION,
                        0,
                        0,
                        45,
                        0,
                        0,
                        &[],
                    ),
                )),
            ),
        ],
    });

    all
}
