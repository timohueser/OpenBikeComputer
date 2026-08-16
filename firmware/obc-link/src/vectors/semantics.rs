//! The semantic body every positive control fixture carries.
//!
//! `Device_Object_Vectors_v2.md` §1 requires a control fixture to carry "header fields, semantic
//! body, and exact frame hex". Frame hex alone is a *byte* pin: three codecs can agree on every
//! byte and still disagree about which field a byte belongs to, and a codec that transposes two
//! adjacent same-width fields round-trips perfectly. The body closes that hole by naming the value
//! the producer wrote at each offset, so a suite can check the *meaning* a decoder assigned rather
//! than only the bytes it gave back.
//!
//! The rules of the encoding, which the three suites all rely on:
//!
//! - one flat object, keys are field paths, never nested objects — `metadata.field[0].tag`,
//!   `entries[1].revision` — so any language can build the same map without a shared schema;
//! - values are JSON numbers only for fields of at most 32 bits, and canonical decimal *strings*
//!   for every `u64`/`i64`, exactly as §1 requires of the rest of the fixture;
//! - opaque byte fields (identities, diagnostic text, metadata field values) are lower-case hex;
//! - enumerated fields carry their **wire number**, never a name, because a name is this crate's
//!   vocabulary rather than the contract's;
//! - reserved fields never appear: a decoder proves them zero and then has nothing to report.
//!
//! Like the rest of this producer, nothing here calls [`crate::request`] or [`crate::response`].
//! The offsets below are read off the protocol's byte tables a second time, independently of the
//! writers in the parent module, so a transposition in either one shows up as a mismatch instead of
//! cancelling out.

use std::format;
use std::string::{String, ToString};
use std::vec::Vec;

use super::{hex, Json};
use crate::frame::{FrameFlags, Opcode};

/// One decoded field: a JSON number for anything up to 32 bits, a string for everything else.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    /// An exactly representable field of at most 32 bits.
    Num(i64),
    /// A `u64`/`i64` as a canonical decimal string, or an opaque byte field as hex.
    Text(String),
}

/// A control fixture's semantic body: field path to decoded value, in spec-table order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Body {
    /// The fields, in the order the byte table lists them.
    pub fields: Vec<(String, Value)>,
}

impl Body {
    /// An empty body — what the four empty-payload messages carry.
    pub fn new() -> Self {
        Body { fields: Vec::new() }
    }

    /// Records a field of at most 32 bits.
    pub fn num(&mut self, key: impl Into<String>, value: impl Into<i64>) {
        self.fields.push((key.into(), Value::Num(value.into())));
    }

    /// Records a `u64` as its canonical decimal string.
    pub fn u64(&mut self, key: impl Into<String>, value: u64) {
        self.fields.push((key.into(), Value::Text(value.to_string())));
    }

    /// Records an `i64` as its canonical decimal string.
    pub fn i64(&mut self, key: impl Into<String>, value: i64) {
        self.fields.push((key.into(), Value::Text(value.to_string())));
    }

    /// Records an opaque byte field as lower-case hex.
    pub fn hex(&mut self, key: impl Into<String>, value: &[u8]) {
        self.fields.push((key.into(), Value::Text(hex(value))));
    }

    /// Records a boolean as the `0`/`1` its byte carries.
    pub fn flag(&mut self, key: impl Into<String>, value: bool) {
        self.num(key, i64::from(u8::from(value)));
    }

    /// Splices another body in under a prefix.
    fn nest(&mut self, prefix: &str, other: Body) {
        for (key, value) in other.fields {
            self.fields.push((format!("{prefix}{key}"), value));
        }
    }

    /// The body as the fixture's `body` object.
    pub fn to_json(&self) -> Json {
        let mut json = Json::new();
        for (key, value) in &self.fields {
            json = match value {
                Value::Num(number) => json.num(key, *number),
                Value::Text(text) => json.str(key, text),
            };
        }
        json
    }
}

// ---------------------------------------------------------------------------------------------
// Little readers. Deliberately not `crate::codec`: this is the producer's own second reading.
// ---------------------------------------------------------------------------------------------

fn u8_at(bytes: &[u8], offset: usize) -> i64 {
    i64::from(bytes[offset])
}

fn u16_at(bytes: &[u8], offset: usize) -> i64 {
    i64::from(u16::from_le_bytes([bytes[offset], bytes[offset + 1]]))
}

fn u32_at(bytes: &[u8], offset: usize) -> i64 {
    i64::from(u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap()))
}

fn i32_at(bytes: &[u8], offset: usize) -> i64 {
    i64::from(i32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap()))
}

fn u64_at(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap())
}

fn i64_at(bytes: &[u8], offset: usize) -> i64 {
    i64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap())
}

// ---------------------------------------------------------------------------------------------
// Shared substructures.
// ---------------------------------------------------------------------------------------------

/// §2.2's metadata envelope: the header, then one row per field.
fn metadata(bytes: &[u8]) -> Body {
    let mut body = Body::new();
    body.num("schemaId", u16_at(bytes, 0));
    body.num("schemaVersion", u8_at(bytes, 2));
    body.num("encodedFieldBytes", u16_at(bytes, 4));
    body.num("fieldCount", u16_at(bytes, 6));
    let mut offset = 8;
    let mut index = 0usize;
    while offset < bytes.len() {
        let tag = u16_at(bytes, offset);
        let length = u16_at(bytes, offset + 2) as usize;
        body.num(format!("field[{index}].tag"), tag);
        body.hex(format!("field[{index}].value"), &bytes[offset + 4..offset + 4 + length]);
        offset += 4 + length;
        index += 1;
    }
    body
}

/// §8.2's sixteen-byte cursor, which both paged queries carry.
fn cursor(bytes: &[u8]) -> Body {
    let mut body = Body::new();
    body.u64("revision", u64_at(bytes, 0));
    body.num("nextEntryIndex", u16_at(bytes, 8));
    body.num("kindCode", u16_at(bytes, 10));
    body.num("crc32", u32_at(bytes, 12));
    body
}

/// §12's ErrorBody, the payload of every `response|error` frame.
fn error_body(payload: &[u8]) -> Body {
    let mut body = Body::new();
    body.num("category", u16_at(payload, 0));
    body.num("detailNamespace", u16_at(payload, 2));
    body.num("detail", u16_at(payload, 4));
    body.num("guidance", u8_at(payload, 6));
    body.num("owner", u8_at(payload, 7));
    body.num("presence", u16_at(payload, 8));
    body.num("retryAfterMs", u32_at(payload, 10));
    body.u64("expectedOffset", u64_at(payload, 14));
    body.u64("currentRevision", u64_at(payload, 22));
    body.u64("requiredBytes", u64_at(payload, 30));
    body.u64("availableBytes", u64_at(payload, 38));
    body.num("textLength", u8_at(payload, 46));
    body.hex("text", &payload[48..]);
    body
}

/// §5.1's 56-byte ResourceLimits block.
fn resource_limits(bytes: &[u8]) -> Body {
    let mut body = Body::new();
    body.num("codecVersion", u8_at(bytes, 0));
    body.num("blockLength", u8_at(bytes, 1));
    body.num("logicalCatalogHeads", u16_at(bytes, 4));
    body.num("normalClaims", u8_at(bytes, 6));
    body.num("uploadWorkSlots", u8_at(bytes, 7));
    body.num("draftParents", u8_at(bytes, 8));
    body.num("draftParts", u8_at(bytes, 9));
    body.num("manifestChildren", u8_at(bytes, 10));
    body.num("mountedFiles", u8_at(bytes, 11));
    body.num("readerLeases", u8_at(bytes, 12));
    body.num("retainedGenerations", u8_at(bytes, 13));
    body.num("retainedResults", u16_at(bytes, 14));
    body.num("inactiveWorkHorizon", u16_at(bytes, 16));
    body.u64("maxGenerationLength", u64_at(bytes, 20));
    body.u64("availableReservationBytes", u64_at(bytes, 28));
    body.num("routeHeads", u16_at(bytes, 36));
    body.num("tripHeads", u16_at(bytes, 38));
    body.num("rideHeads", u16_at(bytes, 40));
    body.num("weatherHeads", u16_at(bytes, 42));
    body.num("volumeManifestHeads", u16_at(bytes, 44));
    body.num("updatePackageHeads", u16_at(bytes, 46));
    body.num("heavyStreamSessions", u8_at(bytes, 48));
    body.num("maintenanceClaims", u8_at(bytes, 49));
    body.num("rideSlots", u8_at(bytes, 50));
    body
}

/// §5's twenty-byte subject entry.
fn subject_entry(bytes: &[u8]) -> Body {
    let mut body = Body::new();
    body.num("namespace", u8_at(bytes, 0));
    body.num("kindCode", u16_at(bytes, 2));
    body.num("operationFlags", u16_at(bytes, 4));
    body.num("policyFlags", u16_at(bytes, 6));
    body.num("putSchemaVersion", u8_at(bytes, 8));
    body.num("patchSchemaVersion", u8_at(bytes, 9));
    body.num("catalogSchemaVersion", u8_at(bytes, 10));
    body.u64("maxLength", u64_at(bytes, 12));
    body
}

/// §10's ResultEnvelope: a type byte and the typed body it introduces.
fn result_envelope(bytes: &[u8]) -> Body {
    let mut body = Body::new();
    let result_type = u8_at(bytes, 0);
    body.num("resultType", result_type);
    let inner = &bytes[4..];
    match result_type {
        1 => {
            body.hex("operationId", &inner[0..16]);
            body.hex("storeId", &inner[16..32]);
            body.num("objectKind", u16_at(inner, 32));
            body.num("outcome", u16_at(inner, 34));
            body.u64("logicalObjectId", u64_at(inner, 36));
            body.u64("revision", u64_at(inner, 44));
            body.u64("length", u64_at(inner, 52));
            body.num("crc32", u32_at(inner, 60));
        }
        2 => {
            body.hex("childOperationId", &inner[0..16]);
            body.hex("storeId", &inner[16..32]);
            body.hex("parentOperationId", &inner[32..48]);
            body.hex("draftPartRef", &inner[48..64]);
            body.num("partKind", u16_at(inner, 64));
            body.u64("partKey", u64_at(inner, 68));
            body.u64("length", u64_at(inner, 76));
            body.num("crc32", u32_at(inner, 84));
        }
        3 => {
            body.hex("operationId", &inner[0..16]);
            body.hex("storeId", &inner[16..32]);
            body.hex("targetOperationId", &inner[32..48]);
            body.num("disposition", u8_at(inner, 48));
        }
        other => panic!("no ResultEnvelope body is registered for result type {other}"),
    }
    body
}

/// §8.1's 24-byte progress body.
fn progress(bytes: &[u8]) -> Body {
    let mut body = Body::new();
    body.num("namespace", u8_at(bytes, 0));
    body.num("phase", u8_at(bytes, 1));
    body.num("flags", u8_at(bytes, 2));
    body.num("subjectKind", u16_at(bytes, 4));
    body.u64("logicalObjectId", u64_at(bytes, 8));
    body.u64("durableOffset", u64_at(bytes, 16));
    body
}

// ---------------------------------------------------------------------------------------------
// The one entry point.
// ---------------------------------------------------------------------------------------------

/// The semantic body of one control vector, read from the payload at the protocol's own offsets.
///
/// Panics on a payload no registered layout describes, which is the producer telling its author
/// that a new message shape needs a row here before it can become a fixture.
pub fn control_body(direction: &str, opcode: Opcode, flags: u16, payload: &[u8]) -> Body {
    if flags & FrameFlags::ERROR != 0 {
        return error_body(payload);
    }
    match (direction, opcode) {
        ("request", Opcode::Hello) => hello(payload),
        ("response", Opcode::Hello) => capabilities(payload),
        ("request", Opcode::StartUpload) => start_upload(payload),
        ("response", Opcode::StartUpload) => upload_accepted(payload),
        ("request", Opcode::CheckpointUpload) => checkpoint_upload(payload),
        ("response", Opcode::CheckpointUpload) => checkpoint_accepted(payload),
        ("request", Opcode::FinishUpload) => session_only(payload),
        ("response", Opcode::FinishUpload) => result_envelope(payload),
        ("request", Opcode::StartDownload) => start_download(payload),
        ("response", Opcode::StartDownload) => download_accepted(payload),
        ("request", Opcode::FinishDownload) => finish_download(payload),
        ("response", Opcode::FinishDownload) => Body::new(),
        ("request", Opcode::AbortSession) => abort_session(payload),
        ("response", Opcode::AbortSession) => one_byte("outcome", payload),
        ("request", Opcode::AbortOperation) => abort_operation(payload),
        ("response", Opcode::AbortOperation) => result_envelope(payload),
        ("request", Opcode::BeginDraft) => begin_draft(payload),
        ("response", Opcode::BeginDraft) => begin_draft_accepted(payload),
        ("request", Opcode::StartDraftPart) => start_draft_part(payload),
        ("response", Opcode::StartDraftPart) => draft_part_accepted(payload),
        ("request", Opcode::FinalizeDraft) => operation_id_only("parentOperationId", payload),
        ("response", Opcode::FinalizeDraft) => finalize_accepted(payload),
        ("request", Opcode::QueryOperation) => operation_id_only("operationId", payload),
        ("response", Opcode::QueryOperation) => operation_status(payload),
        ("request", Opcode::QueryCatalog) => query_catalog(payload),
        ("response", Opcode::QueryCatalog) => catalog_page(payload),
        ("request", Opcode::QueryDraft) => query_draft(payload),
        ("response", Opcode::QueryDraft) => draft_page(payload),
        ("request", Opcode::QueryWeatherRequest) => Body::new(),
        ("response", Opcode::QueryWeatherRequest) => weather_context(payload),
        ("request", Opcode::DeleteObject) => mutation_target(payload),
        ("response", Opcode::DeleteObject) => result_envelope(payload),
        ("request", Opcode::SetMetadata) => set_metadata(payload),
        ("response", Opcode::SetMetadata) => result_envelope(payload),
        ("request", Opcode::InstallUpdate) | ("request", Opcode::AcknowledgeRideImported) => {
            operation_on_object(payload)
        }
        ("response", Opcode::InstallUpdate) | ("response", Opcode::AcknowledgeRideImported) => result_envelope(payload),
        ("request", Opcode::GetDeviceStatus) | ("request", Opcode::GetConfig) => Body::new(),
        ("response", Opcode::GetDeviceStatus) => device_status(payload),
        ("response", Opcode::GetConfig) | ("response", Opcode::SetConfig) | ("request", Opcode::SetConfig) => {
            config_block(payload)
        }
        ("request", Opcode::SetClock) => set_clock(payload),
        ("response", Opcode::SetClock) => clock_status(payload),
        ("request", Opcode::ForgetBond) => one_byte("scope", payload),
        ("response", Opcode::ForgetBond) => Body::new(),
        ("request", Opcode::Echo) | ("response", Opcode::Echo) => {
            let mut body = Body::new();
            body.hex("payload", payload);
            body
        }
        ("request", Opcode::ResetStore) => {
            let mut body = Body::new();
            body.hex("echoStoreId", &payload[0..16]);
            body
        }
        ("response", Opcode::ResetStore) => {
            let mut body = Body::new();
            body.hex("newStoreId", &payload[0..16]);
            body
        }
        (direction, opcode) => panic!("no semantic body is registered for {direction} {}", opcode.name()),
    }
}

// ---------------------------------------------------------------------------------------------
// Per-message layouts, offset for offset from the protocol's tables.
// ---------------------------------------------------------------------------------------------

fn hello(payload: &[u8]) -> Body {
    let mut body = Body::new();
    body.num("minimumMajor", u8_at(payload, 0));
    body.num("maximumMajor", u8_at(payload, 1));
    body.num("clientMaxControlFrame", u16_at(payload, 2));
    body.num("clientMaxStreamFrame", u16_at(payload, 4));
    body.num("clientFeatureFlags", u32_at(payload, 6));
    body.num("pageKind", u8_at(payload, 10));
    body.num("pageIndex", u8_at(payload, 11));
    body
}

fn capabilities(payload: &[u8]) -> Body {
    let mut body = Body::new();
    body.num("selectedMajor", u8_at(payload, 0));
    body.num("storageFormatVersion", u8_at(payload, 1));
    body.num("statusFlags", u16_at(payload, 2));
    body.hex("storeId", &payload[4..20]);
    body.num("negotiatedControlFrame", u16_at(payload, 20));
    body.num("negotiatedStreamFrame", u16_at(payload, 22));
    body.num("checkpointGranule", u32_at(payload, 24));
    body.num("retainedResultCapacity", u16_at(payload, 28));
    body.num("metadataEnvelopeLimit", u16_at(payload, 30));
    body.num("catalogMetadataLimit", u16_at(payload, 32));
    body.num("protocolMinimumControlFrame", u16_at(payload, 34));
    body.num("protocolMinimumStreamFrame", u16_at(payload, 36));
    body.num("linkKind", u8_at(payload, 38));
    body.num("authenticated", u8_at(payload, 39));
    body.num("capabilityRevision", u32_at(payload, 40));
    body.num("commandFlags", u32_at(payload, 44));
    body.num("totalSubjectCount", u16_at(payload, 48));
    body.num("pageKind", u8_at(payload, 50));
    body.num("pageIndex", u8_at(payload, 51));
    body.num("returnedSubjectCount", u8_at(payload, 52));
    body.num("totalPages", u8_at(payload, 53));
    body.num("deviceWireMinor", u8_at(payload, 55));
    let page = &payload[56..];
    if payload[50] == 0 {
        body.nest("resourceLimits.", resource_limits(page));
    } else {
        for index in 0..usize::from(payload[52]) {
            let entry = &page[index * 20..index * 20 + 20];
            body.nest(&format!("subjects[{index}]."), subject_entry(entry));
        }
    }
    body
}

fn start_upload(payload: &[u8]) -> Body {
    let mut body = Body::new();
    body.hex("operationId", &payload[0..16]);
    body.num("objectKind", u16_at(payload, 16));
    body.num("targetMode", u8_at(payload, 18));
    body.num("resume", u8_at(payload, 19));
    body.u64("logicalObjectId", u64_at(payload, 20));
    body.u64("expectedRevision", u64_at(payload, 28));
    body.u64("declaredLength", u64_at(payload, 36));
    body.num("expectedCrc32", u32_at(payload, 44));
    body.nest("metadata.", metadata(&payload[48..]));
    body
}

fn upload_accepted(payload: &[u8]) -> Body {
    let mut body = Body::new();
    let disposition = u8_at(payload, 0);
    body.num("disposition", disposition);
    if disposition == 1 {
        body.nest("result.", result_envelope(&payload[4..]));
        return body;
    }
    body.num("targetMode", u8_at(payload, 1));
    body.num("flags", u16_at(payload, 2));
    body.hex("operationId", &payload[4..20]);
    body.num("sessionId", u32_at(payload, 20));
    body.u64("logicalObjectId", u64_at(payload, 24));
    body.u64("admissionRevision", u64_at(payload, 32));
    body.u64("durableNextOffset", u64_at(payload, 40));
    body.num("checkpointGranule", u32_at(payload, 48));
    body.num("maxStreamPayload", u16_at(payload, 52));
    body.num("finalizedPrefixCrc32", u32_at(payload, 56));
    body
}

fn checkpoint_upload(payload: &[u8]) -> Body {
    let mut body = Body::new();
    body.num("sessionId", u32_at(payload, 0));
    body.u64("receivedNextOffset", u64_at(payload, 4));
    body
}

fn checkpoint_accepted(payload: &[u8]) -> Body {
    let mut body = Body::new();
    body.num("sessionId", u32_at(payload, 0));
    body.u64("durableNextOffset", u64_at(payload, 4));
    body.num("finalizedPrefixCrc32", u32_at(payload, 12));
    body.num("checkpointSequence", u32_at(payload, 16));
    body
}

fn session_only(payload: &[u8]) -> Body {
    let mut body = Body::new();
    body.num("sessionId", u32_at(payload, 0));
    body
}

fn start_download(payload: &[u8]) -> Body {
    let mut body = Body::new();
    body.num("objectKind", u16_at(payload, 0));
    body.num("flags", u16_at(payload, 2));
    body.u64("logicalObjectId", u64_at(payload, 4));
    body.u64("startOffset", u64_at(payload, 20));
    body
}

fn download_accepted(payload: &[u8]) -> Body {
    let mut body = Body::new();
    body.hex("storeId", &payload[0..16]);
    body.num("sessionId", u32_at(payload, 16));
    body.u64("logicalObjectId", u64_at(payload, 20));
    body.u64("pinnedRevision", u64_at(payload, 28));
    body.u64("totalLength", u64_at(payload, 36));
    body.num("wholeSourceCrc32", u32_at(payload, 44));
    body.u64("acceptedStartOffset", u64_at(payload, 48));
    body.num("maxStreamPayload", u16_at(payload, 56));
    body
}

fn finish_download(payload: &[u8]) -> Body {
    let mut body = Body::new();
    body.num("sessionId", u32_at(payload, 0));
    body.u64("receivedLength", u64_at(payload, 4));
    body.num("wholeSourceCrc32", u32_at(payload, 12));
    body
}

fn abort_session(payload: &[u8]) -> Body {
    let mut body = Body::new();
    body.num("sessionId", u32_at(payload, 0));
    body.num("reason", u8_at(payload, 4));
    body
}

fn abort_operation(payload: &[u8]) -> Body {
    let mut body = Body::new();
    body.hex("operationId", &payload[0..16]);
    body.hex("targetOperationId", &payload[16..32]);
    body.num("reason", u8_at(payload, 32));
    body
}

fn begin_draft(payload: &[u8]) -> Body {
    let mut body = Body::new();
    body.hex("parentOperationId", &payload[0..16]);
    body.num("objectKind", u16_at(payload, 16));
    body.num("targetMode", u8_at(payload, 18));
    body.u64("logicalObjectId", u64_at(payload, 20));
    body.u64("expectedRevision", u64_at(payload, 28));
    body.u64("declaredManifestLength", u64_at(payload, 36));
    body.num("declaredManifestCrc32", u32_at(payload, 44));
    body.num("expectedPartCount", u16_at(payload, 48));
    body
}

fn begin_draft_accepted(payload: &[u8]) -> Body {
    let mut body = Body::new();
    let disposition = u8_at(payload, 0);
    body.num("disposition", disposition);
    if disposition == 1 {
        body.nest("result.", result_envelope(&payload[4..]));
        return body;
    }
    body.hex("parentOperationId", &payload[4..20]);
    body.u64("draftRevision", u64_at(payload, 20));
    body.num("expectedPartCount", u16_at(payload, 28));
    body.num("state", u8_at(payload, 30));
    body
}

fn start_draft_part(payload: &[u8]) -> Body {
    let mut body = Body::new();
    body.hex("childOperationId", &payload[0..16]);
    body.hex("parentOperationId", &payload[16..32]);
    body.num("partKind", u16_at(payload, 32));
    body.u64("partKey", u64_at(payload, 36));
    body.u64("declaredLength", u64_at(payload, 44));
    body.num("expectedCrc32", u32_at(payload, 52));
    body.num("resume", u8_at(payload, 56));
    body
}

fn draft_part_accepted(payload: &[u8]) -> Body {
    let mut body = Body::new();
    let disposition = u8_at(payload, 0);
    body.num("disposition", disposition);
    if disposition == 1 {
        body.nest("result.", result_envelope(&payload[4..]));
        return body;
    }
    body.num("flags", u16_at(payload, 2));
    body.hex("childOperationId", &payload[4..20]);
    body.hex("parentOperationId", &payload[20..36]);
    body.num("sessionId", u32_at(payload, 36));
    body.num("partKind", u16_at(payload, 40));
    body.u64("partKey", u64_at(payload, 44));
    body.u64("durableNextOffset", u64_at(payload, 52));
    body.num("checkpointGranule", u32_at(payload, 60));
    body.num("maxStreamPayload", u16_at(payload, 64));
    body.num("finalizedPrefixCrc32", u32_at(payload, 68));
    body
}

fn finalize_accepted(payload: &[u8]) -> Body {
    let mut body = Body::new();
    let disposition = u8_at(payload, 0);
    body.num("disposition", disposition);
    if disposition == 1 {
        body.nest("result.", result_envelope(&payload[4..]));
        return body;
    }
    body.num("flags", u16_at(payload, 2));
    body.hex("parentOperationId", &payload[4..20]);
    body.num("sessionId", u32_at(payload, 20));
    body.u64("logicalObjectId", u64_at(payload, 24));
    body.u64("admissionRevision", u64_at(payload, 32));
    body.u64("durableManifestOffset", u64_at(payload, 40));
    body.num("checkpointGranule", u32_at(payload, 48));
    body.num("maxStreamPayload", u16_at(payload, 52));
    body.num("finalizedPrefixCrc32", u32_at(payload, 56));
    body
}

fn operation_id_only(key: &str, payload: &[u8]) -> Body {
    let mut body = Body::new();
    body.hex(key, &payload[0..16]);
    body
}

fn one_byte(key: &str, payload: &[u8]) -> Body {
    let mut body = Body::new();
    body.num(key, u8_at(payload, 0));
    body
}

fn operation_status(payload: &[u8]) -> Body {
    let mut body = Body::new();
    let state = u8_at(payload, 0);
    body.num("state", state);
    let rest = &payload[4..];
    match state {
        0 => {}
        1 => body.nest("progress.", progress(rest)),
        2 => body.nest("result.", result_envelope(rest)),
        3 => body.nest("error.", error_body(rest)),
        other => panic!("no QueryOperation body is registered for state {other}"),
    }
    body
}

fn query_catalog(payload: &[u8]) -> Body {
    let mut body = Body::new();
    body.num("objectKind", u16_at(payload, 0));
    body.num("flags", u16_at(payload, 2));
    body.u64("expectedRevision", u64_at(payload, 4));
    body.nest("cursor.", cursor(&payload[12..28]));
    body
}

fn catalog_page(payload: &[u8]) -> Body {
    let mut body = Body::new();
    body.hex("storeId", &payload[0..16]);
    body.num("objectKind", u16_at(payload, 16));
    body.num("entryCount", u16_at(payload, 18));
    body.u64("revision", u64_at(payload, 20));
    body.nest("nextCursor.", cursor(&payload[28..44]));
    let mut offset = 44;
    let mut index = 0usize;
    while offset < payload.len() {
        let metadata_len = u16_at(payload, offset + 30) as usize;
        let entry = &payload[offset..offset + 36 + metadata_len];
        let prefix = format!("entries[{index}].");
        let mut entry_body = Body::new();
        entry_body.u64("logicalObjectId", u64_at(entry, 0));
        entry_body.u64("revision", u64_at(entry, 8));
        entry_body.u64("length", u64_at(entry, 16));
        entry_body.num("crc32", u32_at(entry, 24));
        entry_body.nest("metadata.", metadata(&entry[36..]));
        body.nest(&prefix, entry_body);
        offset += 36 + metadata_len;
        index += 1;
    }
    body
}

fn query_draft(payload: &[u8]) -> Body {
    let mut body = Body::new();
    body.hex("parentOperationId", &payload[0..16]);
    body.num("flags", u16_at(payload, 16));
    body.num("requestedLimit", u8_at(payload, 18));
    body.u64("expectedRevision", u64_at(payload, 20));
    body.nest("cursor.", cursor(&payload[28..44]));
    body
}

fn draft_page(payload: &[u8]) -> Body {
    let mut body = Body::new();
    body.hex("parentOperationId", &payload[0..16]);
    body.u64("draftRevision", u64_at(payload, 16));
    body.nest("nextCursor.", cursor(&payload[24..40]));
    body.num("entryCount", u8_at(payload, 40));
    body.num("flags", u8_at(payload, 41));
    for index in 0..usize::from(payload[40]) {
        let entry = &payload[44 + index * 68..44 + index * 68 + 68];
        let mut entry_body = Body::new();
        entry_body.hex("childOperationId", &entry[0..16]);
        entry_body.hex("draftPartRef", &entry[16..32]);
        entry_body.num("partKind", u16_at(entry, 32));
        entry_body.u64("partKey", u64_at(entry, 36));
        entry_body.num("state", u8_at(entry, 44));
        entry_body.u64("durableOffset", u64_at(entry, 48));
        entry_body.u64("declaredLength", u64_at(entry, 56));
        entry_body.num("crc32", u32_at(entry, 64));
        body.nest(&format!("entries[{index}]."), entry_body);
    }
    body
}

fn weather_context(payload: &[u8]) -> Body {
    let mut body = Body::new();
    body.hex("storeId", &payload[0..16]);
    body.u64("currentWeatherRequestId", u64_at(payload, 16));
    body.u64("contextRevision", u64_at(payload, 24));
    body.num("flags", u32_at(payload, 32));
    body.u64("weatherLogicalObjectId", u64_at(payload, 36));
    body.u64("repositoryRevision", u64_at(payload, 44));
    body.u64("headWeatherRequestId", u64_at(payload, 52));
    body.num("centreLatitudeE7", i32_at(payload, 60));
    body.num("centreLongitudeE7", i32_at(payload, 64));
    body.num("radiusMetres", u32_at(payload, 68));
    body.i64("earliestIssuedUtc", i64_at(payload, 72));
    body.i64("requiredValidUntilUtc", i64_at(payload, 80));
    body.num("state", u8_at(payload, 88));
    body
}

fn mutation_target(payload: &[u8]) -> Body {
    let mut body = Body::new();
    body.hex("operationId", &payload[0..16]);
    body.num("objectKind", u16_at(payload, 16));
    body.num("flags", u16_at(payload, 18));
    body.u64("logicalObjectId", u64_at(payload, 20));
    body.u64("expectedRevision", u64_at(payload, 28));
    body
}

fn set_metadata(payload: &[u8]) -> Body {
    let mut body = mutation_target(payload);
    body.nest("patch.", metadata(&payload[36..]));
    body
}

fn operation_on_object(payload: &[u8]) -> Body {
    let mut body = Body::new();
    body.hex("operationId", &payload[0..16]);
    body.u64("logicalObjectId", u64_at(payload, 16));
    body.u64("expectedRevision", u64_at(payload, 24));
    body
}

fn device_status(payload: &[u8]) -> Body {
    let mut body = Body::new();
    body.num("firmwareMajor", u16_at(payload, 0));
    body.num("firmwareMinor", u16_at(payload, 2));
    body.num("firmwarePatch", u16_at(payload, 4));
    body.num("hardwareRevision", u16_at(payload, 6));
    body.hex("deviceSerial", &payload[8..24]);
    body.num("bootCount", u32_at(payload, 24));
    body.u64("uptimeSeconds", u64_at(payload, 28));
    body.num("stackHighWater", u32_at(payload, 36));
    body.num("statusFlags", u16_at(payload, 40));
    body.num("mountClass", u8_at(payload, 42));
    body.num("firmwareBuild", u32_at(payload, 44));
    body.hex("storeId", &payload[48..64]);
    body
}

fn config_block(payload: &[u8]) -> Body {
    let mut body = Body::new();
    body.num("codecVersion", u8_at(payload, 0));
    body.num("blockLength", u8_at(payload, 1));
    body.num("nameLength", u8_at(payload, 4));
    body.num("unitFlags", u8_at(payload, 5));
    body.num("weatherRefresh", u8_at(payload, 6));
    body.hex("name", &payload[8..8 + usize::from(payload[4])]);
    body
}

fn set_clock(payload: &[u8]) -> Body {
    let mut body = Body::new();
    body.i64("epochSeconds", i64_at(payload, 0));
    body.num("source", u8_at(payload, 8));
    body
}

fn clock_status(payload: &[u8]) -> Body {
    let mut body = Body::new();
    body.i64("epochSeconds", i64_at(payload, 0));
    body.num("source", u8_at(payload, 8));
    body.num("state", u8_at(payload, 9));
    body
}
