//! The spec-derived fixture producer for `specs/vectors/flat-store-v4/`.
//!
//! Every byte below is laid down **by hand**, at the offset `FLAT_Store_Protocol.md` §3 states,
//! through the little [`raw`] helpers — never through [`super::wire`]'s encoders and never through
//! its decoders. A golden vector a codec produced proves only that the codec agrees with itself; the
//! only thing the two sides here share is the specification, and the tests then close the loop in
//! both directions:
//!
//! 1. [`tests::checked_in_fixtures_match_the_producer`] proves the files on disk are exactly what
//!    this emits, so an unreviewed fixture rewrite fails CI.
//! 2. [`tests::the_codec_encodes_every_response_vector_byte_for_byte`] and its request twin prove the
//!    production codec agrees with these bytes, and
//!    [`tests::every_negative_vector_is_refused_with_its_stated_code_and_detail`] proves every
//!    refusal lands where §3.9 says.
//! 3. [`tests::section_3_11s_own_frames_are_in_the_suite_verbatim`] pins the four frames the spec
//!    prints, against the spec's own hex.
//!
//! Regenerate after a deliberate spec change with:
//!
//! ```text
//! cargo test -p obc-link flat_regenerate -- --ignored
//! ```

use std::format;
use std::path::PathBuf;
use std::string::{String, ToString};
use std::vec::Vec;

use super::ids::NAME_CAPACITY;

/// Hand-built little-endian byte assembly. Deliberately tiny and deliberately not the codec.
pub mod raw {
    use std::string::String;
    use std::vec;
    use std::vec::Vec;

    /// A zero-filled buffer of exactly `len` bytes, to be filled at stated offsets.
    pub fn zeros(len: usize) -> Vec<u8> {
        vec![0u8; len]
    }

    pub fn u16_at(buffer: &mut [u8], offset: usize, value: u16) {
        buffer[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
    }

    pub fn u32_at(buffer: &mut [u8], offset: usize, value: u32) {
        buffer[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }

    pub fn u64_at(buffer: &mut [u8], offset: usize, value: u64) {
        buffer[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
    }

    pub fn bytes_at(buffer: &mut [u8], offset: usize, value: &[u8]) {
        buffer[offset..offset + value.len()].copy_from_slice(value);
    }

    /// Lower-case hex of a byte slice.
    pub fn hex(bytes: &[u8]) -> String {
        use core::fmt::Write;
        let mut out = String::with_capacity(bytes.len() * 2);
        for byte in bytes {
            let _ = write!(out, "{byte:02x}");
        }
        out
    }

    /// The contract's §1 CRC, computed the long way so a fixture does not inherit a bug from the
    /// shared implementation it exists to pin.
    pub fn crc32(bytes: &[u8]) -> u32 {
        let mut crc = 0xFFFF_FFFFu32;
        for &byte in bytes {
            crc ^= u32::from(byte);
            for _ in 0..8 {
                crc = if crc & 1 != 0 { 0xEDB8_8320 ^ (crc >> 1) } else { crc >> 1 };
            }
        }
        crc ^ 0xFFFF_FFFF
    }
}

use raw::{bytes_at, hex, u16_at, u32_at, u64_at, zeros};

/// A minimal JSON object writer: two-space indent, newline-terminated, no dependency of its own.
#[derive(Default)]
pub struct Json {
    parts: Vec<String>,
}

impl Json {
    pub fn new() -> Self {
        Json { parts: Vec::new() }
    }

    fn push(&mut self, key: &str, value: String) {
        self.parts.push(format!("\"{key}\": {value}"));
    }

    /// A string value.
    pub fn str(mut self, key: &str, value: &str) -> Self {
        self.push(key, format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\"")));
        self
    }

    /// A number small enough to be exact in JSON.
    pub fn num(mut self, key: &str, value: i64) -> Self {
        self.push(key, format!("{value}"));
        self
    }

    /// A `u64`, as a canonical decimal string — the suite never puts one in a JSON number.
    pub fn big(mut self, key: &str, value: u64) -> Self {
        self.push(key, format!("\"{value}\""));
        self
    }

    pub fn bool(mut self, key: &str, value: bool) -> Self {
        self.push(key, if value { "true".to_string() } else { "false".to_string() });
        self
    }

    /// A nested object.
    pub fn obj(mut self, key: &str, value: Json) -> Self {
        if value.parts.is_empty() {
            self.push(key, "{}".to_string());
            return self;
        }
        self.push(key, value.render(4));
        self
    }

    /// An array of nested objects.
    pub fn array(mut self, key: &str, values: Vec<Json>) -> Self {
        if values.is_empty() {
            self.push(key, "[]".to_string());
            return self;
        }
        let rendered: Vec<String> = values.into_iter().map(|value| format!("    {}", value.render(6))).collect();
        self.push(key, format!("[\n{}\n  ]", rendered.join(",\n")));
        self
    }

    fn render(&self, indent: usize) -> String {
        let pad = " ".repeat(indent);
        let inner: Vec<String> = self.parts.iter().map(|part| format!("{pad}{part}")).collect();
        format!("{{\n{}\n{}}}", inner.join(",\n"), " ".repeat(indent.saturating_sub(2)))
    }

    /// The object as a complete file.
    pub fn render_file(&self) -> String {
        format!("{}\n", self.render(2))
    }
}

/// Which directory a fixture lives in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    /// A control request or a successful control response.
    Control,
    /// A stream record.
    Stream,
    /// An error response.
    Error,
    /// An input the codec must refuse.
    Negative,
}

impl Category {
    fn directory(self) -> &'static str {
        match self {
            Category::Control => "controls",
            Category::Stream => "streams",
            Category::Error => "errors",
            Category::Negative => "negative",
        }
    }

    fn key(self) -> &'static str {
        match self {
            Category::Control => "controls",
            Category::Stream => "streams",
            Category::Error => "errors",
            Category::Negative => "negative",
        }
    }
}

/// One fixture: its name, where it lives, the bytes it pins, and the file that carries them.
#[derive(Debug, Clone)]
pub struct Fixture {
    pub name: String,
    pub category: Category,
    pub bytes: Vec<u8>,
    pub json: String,
}

impl Fixture {
    /// The path under the suite root.
    pub fn path(&self) -> String {
        format!("{}/{}.json", self.category.directory(), self.name)
    }

    /// The digest the manifest carries.
    pub fn sha256(&self) -> String {
        sha256_hex(self.json.as_bytes())
    }
}

/// SHA-256 of the fixture file, for the manifest.
pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest;
    let mut hasher = sha2::Sha256::new();
    hasher.update(bytes);
    hex(&hasher.finalize())
}

/// The `specs/vectors/flat-store-v4/` directory at the repo root.
pub fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../specs/vectors/flat-store-v4")
}

// ------------------------------------------------------------------------------------------------
// The identities every fixture is built from. `FLAT_Store_Format.md` §4.1 and §5.7, and
// `FLAT_Store_Protocol.md` §3.11, which carry the same two objects.
// ------------------------------------------------------------------------------------------------

/// §4.1's `StoreId`.
pub const STORE: [u8; 16] =
    [0x8F, 0x2C, 0x41, 0xD9, 0x6B, 0x07, 0x4E, 0xA3, 0xB1, 0x55, 0x9C, 0x20, 0x7D, 0xE8, 0x34, 0x66];
/// §3.10's new era after the destructive FORMAT fixture.
pub const REPLACEMENT_STORE: [u8; 16] =
    [0x2A, 0x7B, 0x16, 0xC4, 0x90, 0x31, 0x45, 0xD8, 0xA6, 0xE2, 0x73, 0x0F, 0xB9, 0x4C, 0x58, 0x11];
/// §5.7's commit sequence.
const SEQUENCE: u64 = 7;
/// §5.7's route: `ObjectId 1` at `Revision 3`, 42,137 bytes, CRC `0x9C4A7E21`, "Grimsel Loop".
const ROUTE_ID: u64 = 1;
const ROUTE_REVISION: u64 = 3;
const ROUTE_LEN: u64 = 42_137;
const ROUTE_CRC: u32 = 0x9C4A_7E21;
const ROUTE_NAME: &[u8] = b"Grimsel Loop";
/// §5.7's ride: `ObjectId 2` at `Revision 1`, `RECORDING`, no name.
const RIDE_ID: u64 = 2;
const RIDE_REVISION: u64 = 1;
/// §3.11's `RequestId` for the upload, and the one its `LIST` uses.
const UPLOAD_REQUEST: u32 = 0x0000_2A01;
const LIST_REQUEST: u32 = 0x0000_2A02;

const HEADER_LEN: usize = 16;

fn header(opcode: u8, flags: u16, payload: usize, request: u32) -> Vec<u8> {
    let mut frame = zeros(HEADER_LEN + payload);
    bytes_at(&mut frame, 0, b"OBC4");
    frame[4] = 4;
    frame[5] = opcode;
    u16_at(&mut frame, 6, flags);
    u16_at(&mut frame, 8, payload as u16);
    u32_at(&mut frame, 12, request);
    frame
}

/// One row of a `LIST` page, as §3.3's entry table lays it out.
struct Row {
    id: u64,
    revision: u64,
    len: u64,
    crc: u32,
    kind: u16,
    flags: u16,
    name: &'static [u8],
}

/// §3.3's 88-byte entry, written at `at`.
fn list_entry(frame: &mut [u8], at: usize, row: &Row) {
    u64_at(frame, at, row.id);
    u64_at(frame, at + 8, row.revision);
    u64_at(frame, at + 16, row.len);
    u32_at(frame, at + 24, row.crc);
    u16_at(frame, at + 28, row.kind);
    u16_at(frame, at + 30, row.flags);
    frame[at + 32] = row.name.len() as u8;
    bytes_at(frame, at + 36, row.name);
}

fn control(name: &str, note: &str, direction: &str, opcode: (&str, u8), bytes: Vec<u8>, body: Json) -> Fixture {
    let json = Json::new()
        .str("name", name)
        .str("suite", "flat-store-v4")
        .str("kind", "control")
        .str("direction", direction)
        .obj("opcode", Json::new().str("name", opcode.0).num("value", i64::from(opcode.1)))
        .obj(
            "header",
            Json::new()
                .str("magic", "OBC4")
                .num("major", 4)
                .num("flags", i64::from(u16::from_le_bytes([bytes[6], bytes[7]])))
                .num("payloadLength", i64::from(u16::from_le_bytes([bytes[8], bytes[9]])))
                .num("requestId", i64::from(u32::from_le_bytes([bytes[12], bytes[13], bytes[14], bytes[15]]))),
        )
        .str("note", note)
        .obj("body", body)
        .str("frame", &hex(&bytes))
        .render_file();
    Fixture { name: name.to_string(), category: Category::Control, bytes, json }
}

fn error_fixture(
    name: &str,
    note: &str,
    opcode: (&str, u8),
    request: u32,
    code: (&str, u16),
    detail: (&str, u16),
    context: u64,
) -> Fixture {
    let mut frame = header(opcode.1, 0b11, 16, request);
    u16_at(&mut frame, HEADER_LEN, code.1);
    u16_at(&mut frame, HEADER_LEN + 2, detail.1);
    u64_at(&mut frame, HEADER_LEN + 4, context);
    let json = Json::new()
        .str("name", name)
        .str("suite", "flat-store-v4")
        .str("kind", "error")
        .obj("opcode", Json::new().str("name", opcode.0).num("value", i64::from(opcode.1)))
        .num("requestId", i64::from(request))
        .obj(
            "body",
            Json::new()
                .str("code", code.0)
                .num("codeValue", i64::from(code.1))
                .str("detail", detail.0)
                .num("detailValue", i64::from(detail.1))
                .big("context", context),
        )
        .str("note", note)
        .str("frame", &hex(&frame))
        .render_file();
    Fixture { name: name.to_string(), category: Category::Error, bytes: frame, json }
}

fn stream_fixture(name: &str, note: &str, request: u32, offset: u64, payload: &[u8]) -> Fixture {
    let mut record = zeros(16 + payload.len());
    u32_at(&mut record, 0, request);
    u64_at(&mut record, 4, offset);
    u16_at(&mut record, 12, payload.len() as u16);
    bytes_at(&mut record, 16, payload);
    let json = Json::new()
        .str("name", name)
        .str("suite", "flat-store-v4")
        .str("kind", "stream")
        .num("requestId", i64::from(request))
        .big("offset", offset)
        .num("payloadLength", payload.len() as i64)
        .str("note", note)
        .str("record", &hex(&record))
        .render_file();
    Fixture { name: name.to_string(), category: Category::Stream, bytes: record, json }
}

/// What a negative vector is fed to, and what it must produce.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// A whole control record, refused with a code and a detail.
    ControlRecord { code: (&'static str, u16), detail: (&'static str, u16) },
    /// A control record with no answerable `RequestId`: the receiver emits nothing and closes.
    Unanswerable,
    /// A stream record that does not split into a frame and its payload.
    StreamRecord,
}

fn negative(name: &str, note: &str, target: Target, bytes: Vec<u8>) -> Fixture {
    let expect = match target {
        Target::ControlRecord { code, detail } => Json::new()
            .str("disposition", "errorResponse")
            .str("code", code.0)
            .num("codeValue", i64::from(code.1))
            .str("detail", detail.0)
            .num("detailValue", i64::from(detail.1)),
        Target::Unanswerable => Json::new().str("disposition", "closeRecordStream"),
        Target::StreamRecord => Json::new().str("disposition", "terminateTransfer"),
    };
    let json = Json::new()
        .str("name", name)
        .str("suite", "flat-store-v4")
        .str("kind", "negative")
        .str("target", if matches!(target, Target::StreamRecord) { "streamRecord" } else { "controlRecord" })
        .str("note", note)
        .obj("expect", expect)
        .str("bytes", &hex(&bytes))
        .render_file();
    Fixture { name: name.to_string(), category: Category::Negative, bytes, json }
}

/// The `PUT` of §3.11, verbatim: creating the route, 100 bytes on the wire.
fn put_create_request() -> Vec<u8> {
    let mut frame = header(0x04, 0, 84, UPLOAD_REQUEST);
    u64_at(&mut frame, HEADER_LEN + 16, ROUTE_LEN);
    u32_at(&mut frame, HEADER_LEN + 24, ROUTE_CRC);
    u16_at(&mut frame, HEADER_LEN + 28, 1);
    frame[HEADER_LEN + 32] = ROUTE_NAME.len() as u8;
    bytes_at(&mut frame, HEADER_LEN + 36, ROUTE_NAME);
    frame
}

/// The `LIST` response of §3.11, verbatim: both entries, no further page, 216 bytes.
fn list_response_two_entries() -> Vec<u8> {
    let mut frame = header(0x01, 0b1, 24 + 2 * 88, LIST_REQUEST);
    bytes_at(&mut frame, HEADER_LEN, &STORE);
    u64_at(&mut frame, HEADER_LEN + 16, SEQUENCE);
    let route = Row {
        id: ROUTE_ID,
        revision: ROUTE_REVISION,
        len: ROUTE_LEN,
        crc: ROUTE_CRC,
        kind: 1,
        flags: 0,
        name: ROUTE_NAME,
    };
    let ride = Row { id: RIDE_ID, revision: RIDE_REVISION, len: 0, crc: 0, kind: 3, flags: 1, name: b"" };
    list_entry(&mut frame, HEADER_LEN + 24, &route);
    list_entry(&mut frame, HEADER_LEN + 24 + 88, &ride);
    frame
}

/// Every fixture in the suite, in the order the manifest lists them.
pub fn fixtures() -> Vec<Fixture> {
    let mut all = Vec::new();

    // -- controls ---------------------------------------------------------------------------------
    let mut first_page = header(0x01, 0, 32, LIST_REQUEST);
    all.push(control(
        "list-first-page-request",
        "Every kind, no cursor: the request a client issues before it does anything else.",
        "request",
        ("LIST", 0x01),
        first_page.clone(),
        Json::new().num("kindFilter", 0).bool("cursor", false),
    ));

    u16_at(&mut first_page, HEADER_LEN, 3);
    all.push(control(
        "list-kind-filtered-request",
        "A listing of one kind: rides, so a client can see whether RECORDING has cleared.",
        "request",
        ("LIST", 0x01),
        first_page,
        Json::new().num("kindFilter", 3).bool("cursor", false),
    ));

    let mut cursor_page = header(0x01, 0, 32, LIST_REQUEST);
    u16_at(&mut cursor_page, HEADER_LEN + 2, 1);
    u64_at(&mut cursor_page, HEADER_LEN + 8, ROUTE_ID);
    u64_at(&mut cursor_page, HEADER_LEN + 16, ROUTE_REVISION);
    u64_at(&mut cursor_page, HEADER_LEN + 24, SEQUENCE);
    all.push(control(
        "list-cursor-page-request",
        "The cursor is the (ObjectId, Revision) pair, and the page resumes strictly after it.",
        "request",
        ("LIST", 0x01),
        cursor_page,
        Json::new()
            .num("kindFilter", 0)
            .bool("cursor", true)
            .big("cursorObjectId", ROUTE_ID)
            .big("cursorRevision", ROUTE_REVISION)
            .big("expectedCommitSequence", SEQUENCE),
    ));

    all.push(control(
        "list-response-two-entries",
        "Section 3.10's complete LIST response, both entries, no further page.",
        "response",
        ("LIST", 0x01),
        list_response_two_entries(),
        Json::new().str("storeId", &hex(&STORE)).big("commitSequence", SEQUENCE).num("entries", 2).bool("more", false),
    ));

    let mut empty_page = header(0x01, 0b1, 24, LIST_REQUEST);
    bytes_at(&mut empty_page, HEADER_LEN, &STORE);
    u64_at(&mut empty_page, HEADER_LEN + 16, 1);
    all.push(control(
        "list-response-empty-catalog",
        "A freshly initialized card: the page carries the identity a client keys its cache on and no entries.",
        "response",
        ("LIST", 0x01),
        empty_page,
        Json::new().str("storeId", &hex(&STORE)).big("commitSequence", 1).num("entries", 0).bool("more", false),
    ));

    let mut paged = list_response_two_entries();
    u16_at(&mut paged, 6, 0b101);
    u16_at(&mut paged, 8, (24 + 88) as u16);
    paged.truncate(HEADER_LEN + 24 + 88);
    all.push(control(
        "list-response-with-a-further-page",
        "One entry and the more bit: the client repeats the request with that entry's pair.",
        "response",
        ("LIST", 0x01),
        paged,
        Json::new().str("storeId", &hex(&STORE)).big("commitSequence", SEQUENCE).num("entries", 1).bool("more", true),
    ));

    let mut status_request = header(0x02, 0, 16, 0x0000_2A03);
    u64_at(&mut status_request, HEADER_LEN, ROUTE_ID);
    u64_at(&mut status_request, HEADER_LEN + 8, ROUTE_REVISION);
    all.push(control(
        "status-request",
        "The reconcile path after a broken link: is this object at this revision committed?",
        "request",
        ("STATUS", 0x02),
        status_request,
        Json::new().big("objectId", ROUTE_ID).big("revision", ROUTE_REVISION),
    ));

    for (name, note, state, revision, len, crc) in [
        (
            "status-response-committed",
            "The catalog holds exactly the revision asked about as the head: the upload landed.",
            1u8,
            ROUTE_REVISION,
            ROUTE_LEN,
            ROUTE_CRC,
        ),
        (
            "status-response-superseded",
            "The object exists at a different revision, and the answer says which.",
            2,
            ROUTE_REVISION + 1,
            ROUTE_LEN,
            ROUTE_CRC,
        ),
        ("status-response-absent", "No entry names that ObjectId, so every head field is zero.", 0, 0, 0, 0),
    ] {
        let mut frame = header(0x02, 0b1, 24, 0x0000_2A03);
        frame[HEADER_LEN] = state;
        u64_at(&mut frame, HEADER_LEN + 4, revision);
        u64_at(&mut frame, HEADER_LEN + 12, len);
        u32_at(&mut frame, HEADER_LEN + 20, crc);
        all.push(control(
            name,
            note,
            "response",
            ("STATUS", 0x02),
            frame,
            Json::new()
                .num("state", i64::from(state))
                .big("headRevision", revision)
                .big("headPayloadLength", len)
                .num("headPayloadCrc32", i64::from(crc)),
        ));
    }

    let mut get_head = header(0x03, 0, 16, 0x0000_2A04);
    u64_at(&mut get_head, HEADER_LEN, ROUTE_ID);
    all.push(control(
        "get-request-head",
        "Revision zero takes the current head.",
        "request",
        ("GET", 0x03),
        get_head,
        Json::new().big("objectId", ROUTE_ID).big("revision", 0),
    ));

    let mut get_pinned = header(0x03, 0, 16, 0x0000_2A04);
    u64_at(&mut get_pinned, HEADER_LEN, ROUTE_ID);
    u64_at(&mut get_pinned, HEADER_LEN + 8, ROUTE_REVISION);
    all.push(control(
        "get-request-pinned-revision",
        "A named revision, which is how a retained previous revision is reached.",
        "request",
        ("GET", 0x03),
        get_pinned,
        Json::new().big("objectId", ROUTE_ID).big("revision", ROUTE_REVISION),
    ));

    let mut get_response = header(0x03, 0b1, 24, 0x0000_2A04);
    u64_at(&mut get_response, HEADER_LEN, ROUTE_REVISION);
    u64_at(&mut get_response, HEADER_LEN + 8, ROUTE_LEN);
    u32_at(&mut get_response, HEADER_LEN + 16, ROUTE_CRC);
    all.push(control(
        "get-response",
        "Sent once the last payload byte has been handed to the transport; the client verifies both.",
        "response",
        ("GET", 0x03),
        get_response,
        Json::new()
            .big("revisionServed", ROUTE_REVISION)
            .big("payloadLength", ROUTE_LEN)
            .num("payloadCrc32", i64::from(ROUTE_CRC)),
    ));

    all.push(control(
        "put-create-request",
        "Section 3.10's PUT creating the route: ObjectId zero means create and both identity fields are zero.",
        "request",
        ("PUT", 0x04),
        put_create_request(),
        Json::new()
            .big("objectId", 0)
            .big("expectedRevision", 0)
            .big("payloadLength", ROUTE_LEN)
            .num("payloadCrc32", i64::from(ROUTE_CRC))
            .num("kind", 1)
            .bool("retainPrevious", false)
            .str("displayName", "Grimsel Loop"),
    ));

    let mut put_retaining = header(0x04, 0, 84, UPLOAD_REQUEST);
    u64_at(&mut put_retaining, HEADER_LEN, 4);
    u64_at(&mut put_retaining, HEADER_LEN + 8, 2);
    u64_at(&mut put_retaining, HEADER_LEN + 16, 8_192);
    u32_at(&mut put_retaining, HEADER_LEN + 24, 0x1234_5678);
    u16_at(&mut put_retaining, HEADER_LEN + 28, 4);
    u16_at(&mut put_retaining, HEADER_LEN + 30, 1);
    put_retaining[HEADER_LEN + 32] = 7;
    bytes_at(&mut put_retaining, HEADER_LEN + 36, b"weather");
    all.push(control(
        "put-replace-retaining-request",
        "A weather bundle replacing revision 2 and asking the same commit to leave it RETAINED.",
        "request",
        ("PUT", 0x04),
        put_retaining,
        Json::new()
            .big("objectId", 4)
            .big("expectedRevision", 2)
            .big("payloadLength", 8_192)
            .num("payloadCrc32", 0x1234_5678)
            .num("kind", 4)
            .bool("retainPrevious", true)
            .str("displayName", "weather"),
    ));

    let mut put_response = header(0x04, 0b1, 32, UPLOAD_REQUEST);
    u64_at(&mut put_response, HEADER_LEN, ROUTE_ID);
    u64_at(&mut put_response, HEADER_LEN + 8, 1);
    u64_at(&mut put_response, HEADER_LEN + 16, ROUTE_LEN);
    u32_at(&mut put_response, HEADER_LEN + 24, ROUTE_CRC);
    all.push(control(
        "put-response",
        "The commit happened: the assigned ObjectId, the new Revision, and what the catalog now holds.",
        "response",
        ("PUT", 0x04),
        put_response,
        Json::new()
            .big("objectId", ROUTE_ID)
            .big("revision", 1)
            .big("payloadLength", ROUTE_LEN)
            .num("payloadCrc32", i64::from(ROUTE_CRC)),
    ));

    let mut remove_request = header(0x05, 0, 16, 0x0000_2A05);
    u64_at(&mut remove_request, HEADER_LEN, ROUTE_ID);
    u64_at(&mut remove_request, HEADER_LEN + 8, ROUTE_REVISION);
    all.push(control(
        "remove-request",
        "One commit removes the entry and frees its extents; a retained previous revision goes with it.",
        "request",
        ("REMOVE", 0x05),
        remove_request,
        Json::new().big("objectId", ROUTE_ID).big("expectedRevision", ROUTE_REVISION),
    ));

    let mut remove_response = header(0x05, 0b1, 8, 0x0000_2A05);
    u64_at(&mut remove_response, HEADER_LEN, SEQUENCE + 1);
    all.push(control(
        "remove-response",
        "The new catalog commit sequence, and nothing else.",
        "response",
        ("REMOVE", 0x05),
        remove_response,
        Json::new().big("commitSequence", SEQUENCE + 1),
    ));

    let mut cancel_request = header(0x06, 0, 4, 0x0000_2A06);
    u32_at(&mut cancel_request, HEADER_LEN, UPLOAD_REQUEST);
    all.push(control(
        "cancel-request",
        "Cancel is bilateral: this names the transfer, which also receives its own cancelled response.",
        "request",
        ("CANCEL", 0x06),
        cancel_request,
        Json::new().num("transferRequestId", i64::from(UPLOAD_REQUEST)),
    ));

    for (name, note, value) in [
        ("cancel-response-cancelled", "0: the live transfer was dropped and its allocation released.", 0u8),
        ("cancel-response-no-such-transfer", "1: nothing by that RequestId is live.", 1),
    ] {
        let mut frame = header(0x06, 0b1, 1, 0x0000_2A06);
        frame[HEADER_LEN] = value;
        all.push(control(
            name,
            note,
            "response",
            ("CANCEL", 0x06),
            frame,
            Json::new().num("outcome", i64::from(value)),
        ));
    }

    let mut arm_request = header(0x07, 0, 16, 0x0000_2A07);
    u64_at(&mut arm_request, HEADER_LEN, 5);
    u64_at(&mut arm_request, HEADER_LEN + 8, 1);
    all.push(control(
        "arm-request",
        "Uploading never installs: ARM is the separate step that makes an installed image the next boot.",
        "request",
        ("ARM", 0x07),
        arm_request,
        Json::new().big("packageObjectId", 5).big("expectedRevision", 1),
    ));

    let mut arm_response = header(0x07, 0b1, 16, 0x0000_2A07);
    u64_at(&mut arm_response, HEADER_LEN, 6);
    u64_at(&mut arm_response, HEADER_LEN + 8, SEQUENCE + 1);
    all.push(control(
        "arm-response",
        "The rollback reserve's ObjectId and the sequence of the one commit ARM makes.",
        "response",
        ("ARM", 0x07),
        arm_response,
        Json::new().big("rollbackObjectId", 6).big("commitSequence", SEQUENCE + 1),
    ));

    let mut format_request = header(0x08, 0, 32, 0x0000_2A08);
    bytes_at(&mut format_request, HEADER_LEN, &STORE);
    bytes_at(&mut format_request, HEADER_LEN + 16, &REPLACEMENT_STORE);
    all.push(control(
        "format-request",
        "Destructive compare-and-swap: erase only the store the client confirmed, then begin a new identity era.",
        "request",
        ("FORMAT", 0x08),
        format_request,
        Json::new().str("expectedStoreId", &hex(&STORE)).str("replacementStoreId", &hex(&REPLACEMENT_STORE)),
    ));

    let mut format_response = header(0x08, 0b1, 16, 0x0000_2A08);
    bytes_at(&mut format_response, HEADER_LEN, &REPLACEMENT_STORE);
    all.push(control(
        "format-response",
        "The new store identity is durable; after this response leaves the link the device reboots.",
        "response",
        ("FORMAT", 0x08),
        format_response,
        Json::new().str("storeId", &hex(&REPLACEMENT_STORE)),
    ));

    // -- streams ----------------------------------------------------------------------------------
    let kilobyte: Vec<u8> = (0..1_024).map(|index| (index % 251) as u8).collect();
    all.push(stream_fixture(
        "stream-frame-of-section-3-11",
        "Section 3.11's stream frame: offset 40,960 and 1,024 payload bytes.",
        UPLOAD_REQUEST,
        40_960,
        &kilobyte,
    ));
    all.push(stream_fixture(
        "stream-first-frame",
        "An upload begins at offset zero and may begin before any acceptance.",
        UPLOAD_REQUEST,
        0,
        &kilobyte[..1_008],
    ));
    all.push(stream_fixture(
        "stream-final-partial-frame",
        "The last record of a payload, ending exactly at the declared length.",
        UPLOAD_REQUEST,
        41_984,
        &kilobyte[..153],
    ));
    all.push(stream_fixture(
        "stream-minimum-payload",
        "One byte: the smallest legal record, because a zero length terminates the transfer.",
        UPLOAD_REQUEST,
        42_136,
        &kilobyte[..1],
    ));

    // -- errors -----------------------------------------------------------------------------------
    all.push(error_fixture(
        "unsupported-opcode",
        "An unknown opcode. There is no generic forwarding path.",
        ("LIST", 0x01),
        0x0000_2A08,
        ("unsupported", 1),
        ("opcode", 1),
        0,
    ));
    all.push(error_fixture(
        "invalid-frame-truncated",
        "The record carries fewer bytes than its stated payload length.",
        ("PUT", 0x04),
        UPLOAD_REQUEST,
        ("invalidFrame", 2),
        ("truncated", 3),
        0,
    ));
    all.push(error_fixture(
        "invalid-request-stream-offset",
        "A gap, an overlap or a zero length on the stream channel terminates the transfer.",
        ("PUT", 0x04),
        UPLOAD_REQUEST,
        ("invalidRequest", 3),
        ("streamOffset", 4),
        0,
    ));
    all.push(error_fixture(
        "not-found-object",
        "No entry names that ObjectId.",
        ("GET", 0x03),
        0x0000_2A04,
        ("notFound", 4),
        ("object", 1),
        0,
    ));
    all.push(error_fixture(
        "revision-conflict-head-differs",
        "Section 3.10's error response: the route already exists at another revision, and the context is the head.",
        ("PUT", 0x04),
        UPLOAD_REQUEST,
        ("revisionConflict", 5),
        ("headDiffers", 1),
        5,
    ));
    all.push(error_fixture(
        "no-space-extents",
        "The context of noSpace is the bytes required.",
        ("PUT", 0x04),
        UPLOAD_REQUEST,
        ("noSpace", 6),
        ("extents", 1),
        ROUTE_LEN,
    ));
    all.push(error_fixture(
        "checksum-failure-payload",
        "The whole-payload CRC did not match, and the context is the CRC the request declared.",
        ("PUT", 0x04),
        UPLOAD_REQUEST,
        ("checksumFailure", 7),
        ("payload", 1),
        u64::from(ROUTE_CRC),
    ));
    all.push(error_fixture(
        "media-io-write",
        "The card refused a write. The mutation did not happen.",
        ("PUT", 0x04),
        UPLOAD_REQUEST,
        ("mediaIo", 8),
        ("write", 2),
        0,
    ));
    all.push(error_fixture(
        "busy-transfer",
        "One transfer at a time, and the context names the live one.",
        ("GET", 0x03),
        0x0000_2A09,
        ("busy", 9),
        ("transfer", 1),
        u64::from(UPLOAD_REQUEST),
    ));
    all.push(error_fixture(
        "cancelled-by-client",
        "The answer the cancelled transfer gets, beside the CANCEL's own one-byte response.",
        ("PUT", 0x04),
        UPLOAD_REQUEST,
        ("cancelled", 10),
        ("byClient", 1),
        0,
    ));
    all.push(error_fixture(
        "rejected-by-the-kinds-validator",
        "The kind's validator owns this detail space; 3 is an example of one it may define.",
        ("PUT", 0x04),
        UPLOAD_REQUEST,
        ("rejected", 11),
        ("kindDefined", 3),
        0,
    ));
    all.push(error_fixture(
        "internal",
        "A failure the device cannot classify. It carries no detail.",
        ("ARM", 0x07),
        0x0000_2A07,
        ("internal", 12),
        ("none", 0),
        0,
    ));
    all.push(error_fixture(
        "catalog-changed-listing",
        "A paged listing whose expected commit sequence no longer matches; the context is the current one.",
        ("LIST", 0x01),
        LIST_REQUEST,
        ("catalogChanged", 13),
        ("listing", 1),
        SEQUENCE + 1,
    ));
    all.push(error_fixture(
        "read-only-unformatted",
        "A card that is not a flat store. Every opcode answers this, including the reads.",
        ("LIST", 0x01),
        LIST_REQUEST,
        ("readOnly", 14),
        ("unformatted", 3),
        0,
    ));

    // -- negative ---------------------------------------------------------------------------------
    let framing = |name: &str, note: &str, detail: (&'static str, u16), mutate: fn(&mut Vec<u8>)| {
        let mut bytes = put_create_request();
        mutate(&mut bytes);
        negative(name, note, Target::ControlRecord { code: ("invalidFrame", 2), detail }, bytes)
    };
    all.push(framing("bad-magic", "The four bytes every control frame opens with.", ("magic", 1), |bytes| {
        bytes[3] = b'5';
    }));
    all.push(framing(
        "declared-length-is-not-the-opcodes",
        "Every message is a fixed layout; a payload length that is not this opcode's is a framing error.",
        ("length", 2),
        |bytes| u16_at(bytes, 8, 83),
    ));
    all.push(framing("truncated-record", "A record short of its stated payload.", ("truncated", 3), |bytes| {
        bytes.truncate(99);
    }));
    all.push(framing(
        "trailing-byte",
        "A frame carrying a byte past the end of its stated layout is a framing error, exactly as a short one is.",
        ("trailing", 4),
        |bytes| bytes.push(0),
    ));

    let mut wrong_major = put_create_request();
    wrong_major[4] = 3;
    all.push(negative(
        "wrong-wire-major",
        "The major is a transport fact and is never negotiated in a frame.",
        Target::ControlRecord { code: ("unsupported", 1), detail: ("wireMajor", 3) },
        wrong_major,
    ));

    let mut unknown_opcode = put_create_request();
    unknown_opcode[5] = 0x09;
    all.push(negative(
        "unknown-opcode",
        "Seven opcodes, and no generic forwarding path.",
        Target::ControlRecord { code: ("unsupported", 1), detail: ("opcode", 1) },
        unknown_opcode,
    ));

    let mut flagged = put_create_request();
    u16_at(&mut flagged, 6, 1);
    all.push(negative(
        "request-carrying-a-response-flag",
        "Requests carry no flags.",
        Target::ControlRecord { code: ("invalidRequest", 3), detail: ("reservedBits", 1) },
        flagged,
    ));

    let mut reserved_header = put_create_request();
    u16_at(&mut reserved_header, 10, 1);
    all.push(negative(
        "nonzero-reserved-header-field",
        "Reserved bytes are zero and rejected when nonzero.",
        Target::ControlRecord { code: ("invalidRequest", 3), detail: ("reservedBits", 1) },
        reserved_header,
    ));

    let bad_put = |name: &str, note: &str, detail: (&'static str, u16), mutate: fn(&mut Vec<u8>)| {
        let mut bytes = put_create_request();
        mutate(&mut bytes);
        negative(name, note, Target::ControlRecord { code: ("invalidRequest", 3), detail }, bytes)
    };
    all.push(bad_put(
        "put-create-naming-an-object",
        "Zero is not a wildcard in either field: a create sends zero in both.",
        ("badCombination", 3),
        |bytes| u64_at(bytes, HEADER_LEN, 7),
    ));
    all.push(bad_put(
        "put-replace-without-a-revision",
        "A nonzero ObjectId means replace, and the expected Revision must be the one last reported.",
        ("badCombination", 3),
        |bytes| u64_at(bytes, HEADER_LEN + 8, 2),
    ));
    all.push(bad_put(
        "put-name-length-above-48",
        "The display name is at most 48 bytes.",
        ("badCombination", 3),
        |bytes| bytes[HEADER_LEN + 32] = 49,
    ));
    all.push(bad_put(
        "put-name-that-is-not-utf8",
        "The field is UTF-8, and a menu has nothing to do with bytes that are not.",
        ("badCombination", 3),
        |bytes| bytes[HEADER_LEN + 36] = 0xFF,
    ));
    all.push(bad_put("put-nonzero-name-pad", "Unused name bytes are zero.", ("reservedBits", 1), |bytes| {
        bytes[HEADER_LEN + 36 + NAME_CAPACITY - 1] = 1
    }));
    all.push(bad_put(
        "put-nonzero-reserved-run",
        "The three bytes after the name length are zero.",
        ("reservedBits", 1),
        |bytes| bytes[HEADER_LEN + 33] = 1,
    ));
    all.push(bad_put(
        "put-undefined-request-flag",
        "Bit 0 is retain-previous and the other fifteen are zero.",
        ("reservedBits", 1),
        |bytes| u16_at(bytes, HEADER_LEN + 30, 2),
    ));

    let mut unknown_kind = put_create_request();
    u16_at(&mut unknown_kind, HEADER_LEN + 28, 9);
    all.push(negative(
        "put-unknown-kind",
        "Section 3.1 of the format contract is the sole authority for kind values.",
        Target::ControlRecord { code: ("unsupported", 1), detail: ("kind", 2) },
        unknown_kind,
    ));

    let mut list_cursorless = header(0x01, 0, 32, LIST_REQUEST);
    u64_at(&mut list_cursorless, HEADER_LEN + 8, ROUTE_ID);
    all.push(negative(
        "list-cursor-fields-without-the-cursor-bit",
        "The three cursor fields are zero unless the cursor bit is set.",
        Target::ControlRecord { code: ("invalidRequest", 3), detail: ("badCombination", 3) },
        list_cursorless,
    ));

    let mut list_flagged = header(0x01, 0, 32, LIST_REQUEST);
    u16_at(&mut list_flagged, HEADER_LEN + 2, 2);
    all.push(negative(
        "list-undefined-flag-bit",
        "Bit 0 is the cursor and the other bits are zero.",
        Target::ControlRecord { code: ("invalidRequest", 3), detail: ("reservedBits", 1) },
        list_flagged,
    ));

    let mut list_kind = header(0x01, 0, 32, LIST_REQUEST);
    u16_at(&mut list_kind, HEADER_LEN, 9);
    all.push(negative(
        "list-unknown-kind-filter",
        "A filter naming a kind this major does not register.",
        Target::ControlRecord { code: ("unsupported", 1), detail: ("kind", 2) },
        list_kind,
    ));

    let status_zero = header(0x02, 0, 16, 0x0000_2A03);
    all.push(negative(
        "status-naming-object-zero",
        "The identity of the store comes from LIST, not from a STATUS of ObjectId zero.",
        Target::ControlRecord { code: ("invalidRequest", 3), detail: ("badCombination", 3) },
        status_zero,
    ));

    let mut zero_request = put_create_request();
    u32_at(&mut zero_request, 12, 0);
    all.push(negative(
        "zero-request-id",
        "A zero RequestId is unanswerable — a response would have to echo it — so the receiver emits nothing and \
         closes that record stream.",
        Target::Unanswerable,
        zero_request,
    ));
    all.push(negative(
        "record-shorter-than-a-header",
        "There is no RequestId to echo.",
        Target::Unanswerable,
        zeros(HEADER_LEN - 1),
    ));

    let mut zero_length = zeros(16 + 4);
    u32_at(&mut zero_length, 0, UPLOAD_REQUEST);
    all.push(negative(
        "stream-zero-payload-length",
        "A zero length terminates the transfer with an error response on the control channel.",
        Target::StreamRecord,
        zero_length,
    ));

    let mut disagreeing = zeros(16 + 4);
    u32_at(&mut disagreeing, 0, UPLOAD_REQUEST);
    u16_at(&mut disagreeing, 12, 5);
    all.push(negative(
        "stream-length-disagreeing-with-the-record",
        "The record is the frame followed by exactly payload length bytes.",
        Target::StreamRecord,
        disagreeing,
    ));

    let mut stream_reserved = zeros(16 + 4);
    u32_at(&mut stream_reserved, 0, UPLOAD_REQUEST);
    u16_at(&mut stream_reserved, 12, 4);
    stream_reserved[14] = 1;
    all.push(negative(
        "stream-nonzero-reserved-field",
        "The two bytes after the length are zero.",
        Target::StreamRecord,
        stream_reserved,
    ));

    all
}

/// The suite manifest: the scalars that identify it, and one array per category.
pub fn manifest(fixtures: &[Fixture]) -> String {
    let entries = |category: Category| -> Vec<Json> {
        fixtures
            .iter()
            .filter(|fixture| fixture.category == category)
            .map(|fixture| {
                Json::new().str("name", &fixture.name).str("file", &fixture.path()).str("sha256", &fixture.sha256())
            })
            .collect()
    };
    let mut json =
        Json::new().str("suite", "flat-store-v4").num("format", 1).num("wire_major", 4).num("storage_format", 1);
    for category in [Category::Control, Category::Stream, Category::Error, Category::Negative] {
        json = json.array(category.key(), entries(category));
    }
    json.render_file()
}

/// Writes the whole suite to `specs/vectors/flat-store-v4/`.
pub fn write_all() -> std::io::Result<usize> {
    let root = dir();
    for category in [Category::Control, Category::Stream, Category::Error, Category::Negative] {
        std::fs::create_dir_all(root.join(category.directory()))?;
    }
    let all = fixtures();
    for fixture in &all {
        std::fs::write(root.join(fixture.path()), fixture.json.as_bytes())?;
    }
    std::fs::write(root.join("manifest.json"), manifest(&all).as_bytes())?;
    Ok(all.len() + 1)
}

#[cfg(test)]
mod tests;
