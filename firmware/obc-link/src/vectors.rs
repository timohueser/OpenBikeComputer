//! The spec-derived fixture producer for `specs/vectors/device-object-v2/`.
//!
//! `Device_Object_Vectors_v2.md` §1 is unambiguous about what this file may and may not do:
//! "A production decoder must not generate its own expected bytes. The Rust fixture producer builds
//! bytes directly from the byte tables without calling the production encoder."
//!
//! So every byte below is laid down by hand, at the offset the protocol's own table gives, through
//! the little [`raw`] helpers — never through [`crate::request`], [`crate::response`], or any
//! `encode` method on a codec type. The only thing the two sides share is the specification. The
//! tests in this module then close the loop in both directions:
//!
//! 1. [`tests::checked_in_fixtures_match_the_producer`] proves the checked-in files are exactly what
//!    this producer emits — the CI guard §7 asks for, which fails on an unreviewed fixture rewrite.
//! 2. [`tests::the_production_codec_decodes_and_re_encodes_every_positive_vector`] proves the codec
//!    agrees with those bytes, and its negative twin proves every rejection lands in the category
//!    and detail the contract names.
//!
//! Regenerate after a deliberate spec change with:
//!
//! ```text
//! cargo test -p obc-link regenerate -- --ignored
//! ```
//!
//! ## Scope
//!
//! This producer emits the **wire** inventory: control vectors, stream vectors, canonical-intent
//! goldens, rejection fixtures, and the transcripts of issue #1358. §6's storage vectors are cut
//! points and record layouts of `OBC2_Storage_Format.md`, which no wire codec can produce:
//! `obc-storage`'s own spec-derived producer writes them under `storage/`, and this manifest only
//! *indexes* them by name and digest, because §1 gives the suite one manifest. Regenerate that side
//! first, then this one.

use std::format;
use std::path::PathBuf;
use std::string::{String, ToString};
use std::vec::Vec;

use crate::frame::{Opcode, HEADER_LEN};
use crate::metadata::SchemaClass;

/// Hand-built little-endian byte assembly. Deliberately tiny and deliberately not the codec.
pub mod raw {
    use std::string::String;
    use std::vec;
    use std::vec::Vec;

    /// A zero-filled buffer of exactly `len` bytes, to be filled at stated offsets.
    pub fn zeros(len: usize) -> Vec<u8> {
        vec![0u8; len]
    }

    /// Writes a `u16` at `offset`.
    pub fn u16_at(buffer: &mut [u8], offset: usize, value: u16) {
        buffer[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
    }

    /// Writes a `u32` at `offset`.
    pub fn u32_at(buffer: &mut [u8], offset: usize, value: u32) {
        buffer[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }

    /// Writes a `u64` at `offset`.
    pub fn u64_at(buffer: &mut [u8], offset: usize, value: u64) {
        buffer[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
    }

    /// Writes an `i32` at `offset`.
    pub fn i32_at(buffer: &mut [u8], offset: usize, value: i32) {
        u32_at(buffer, offset, value as u32);
    }

    /// Writes an `i64` at `offset`.
    pub fn i64_at(buffer: &mut [u8], offset: usize, value: i64) {
        u64_at(buffer, offset, value as u64);
    }

    /// Writes raw bytes at `offset`.
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

    /// The `Device_Object_Protocol_v3.md` §1 CRC over `bytes`, computed the long way so the
    /// fixtures do not inherit a bug from the shared implementation they are meant to pin.
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

use raw::{bytes_at, crc32, hex, i32_at, i64_at, u16_at, u32_at, u64_at, zeros};

/// The `specs/vectors/device-object-v2/` directory at the repo root.
pub fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../specs/vectors/device-object-v2")
}

// ---------------------------------------------------------------------------------------------
// Deterministic fixture identities. Chosen once, never derived from anything.
// ---------------------------------------------------------------------------------------------

/// The suite's StoreId.
pub const STORE: [u8; 16] =
    [0x3c, 0x92, 0x00, 0x00, 0x99, 0x16, 0x4e, 0xba, 0xab, 0xc2, 0x34, 0x2f, 0xe0, 0x8f, 0x6b, 0x10];
/// A second StoreId, for the card-replacement and reset fixtures.
pub const STORE_B: [u8; 16] =
    [0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff, 0x00];
/// The primary OperationId.
pub const OP_A: [u8; 16] = [0xa1; 16];
/// A second OperationId.
pub const OP_B: [u8; 16] = [0xb2; 16];
/// A draft parent OperationId.
pub const OP_PARENT: [u8; 16] = [0xc3; 16];
/// A draft child OperationId.
pub const OP_CHILD: [u8; 16] = [0xd4; 16];
/// An abort-command OperationId.
pub const OP_ABORT: [u8; 16] = [0xe5; 16];
/// A sealed part's opaque reference.
pub const PART_REF: [u8; 16] =
    [0x5a, 0xa5, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0x00, 0x0f, 0xf0, 0x1e, 0xe1];
/// The durable upload checkpoint granule the suite uses.
///
/// §1's *default* granule is 262,144 bytes, and the granule is device policy an acceptance
/// advertises rather than a protocol constant. The fixtures deliberately use a small one: the
/// suite's objects are 3,000 and 65,536 bytes, so at the default granule they would have exactly
/// one durable prefix — the whole object — and every "finalized prefix CRC" in the suite would be
/// the whole-object CRC wearing a different name. At 1,024 bytes the checkpoint fixtures have
/// several genuinely different prefixes, and a codec that confused a prefix CRC with an object CRC
/// would fail against them.
pub const FIXTURE_GRANULE: u32 = 1_024;

/// The device serial the status fixtures report.
pub const SERIAL: [u8; 16] =
    [0x0b, 0xc0, 0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef, 0x10, 0x32, 0x54, 0x76, 0x98, 0xba];

/// Which manifest array a fixture belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    /// A control-plane vector, including the canonical-intent goldens.
    Control,
    /// A stream-plane vector.
    Stream,
    /// A rejection fixture.
    Negative,
    /// A semantic transcript.
    Transcript,
}

impl Category {
    /// The manifest array's key.
    pub const fn key(self) -> &'static str {
        match self {
            Category::Control => "controls",
            Category::Stream => "streams",
            Category::Negative => "negative",
            Category::Transcript => "transcripts",
        }
    }

    /// The subdirectory fixtures of this category live in.
    pub const fn directory(self) -> &'static str {
        match self {
            Category::Control => "controls",
            Category::Stream => "streams",
            Category::Negative => "negative",
            Category::Transcript => "transcripts",
        }
    }
}

/// One emitted fixture file.
#[derive(Debug, Clone)]
pub struct Fixture {
    /// The stable unique name, also the file stem.
    pub name: String,
    /// Which manifest array it belongs to.
    pub category: Category,
    /// The canonical file bytes.
    pub json: String,
}

impl Fixture {
    /// The path relative to the suite directory.
    pub fn path(&self) -> String {
        format!("{}/{}.json", self.category.directory(), self.name)
    }

    /// SHA-256 of the canonical file bytes, as the manifest records it.
    pub fn sha256(&self) -> String {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(self.json.as_bytes());
        hex(&hasher.finalize())
    }
}

// ---------------------------------------------------------------------------------------------
// A minimal deterministic JSON writer. No dependency, no key reordering, no floating point.
// ---------------------------------------------------------------------------------------------

/// Builds one JSON object with insertion-ordered keys.
#[derive(Debug, Default)]
pub struct Json {
    parts: Vec<String>,
}

impl Json {
    /// A new empty object.
    pub fn new() -> Self {
        Json { parts: Vec::new() }
    }

    fn push(&mut self, key: &str, value: String) {
        self.parts.push(format!("\"{key}\": {value}"));
    }

    /// A string value.
    pub fn str(mut self, key: &str, value: &str) -> Self {
        self.push(key, format!("\"{}\"", escape(value)));
        self
    }

    /// A number that is exactly representable — at most 32 bits, per §1 of the vectors contract.
    pub fn num(mut self, key: &str, value: i64) -> Self {
        self.push(key, format!("{value}"));
        self
    }

    /// A `u64` or `i64`, which §1 requires as a canonical decimal *string*.
    pub fn big(mut self, key: &str, value: &str) -> Self {
        self.push(key, format!("\"{value}\""));
        self
    }

    /// A boolean.
    pub fn bool(mut self, key: &str, value: bool) -> Self {
        self.push(key, if value { "true".to_string() } else { "false".to_string() });
        self
    }

    /// A nested object. An empty one renders as `{}`, which is what the five empty-payload
    /// messages' semantic bodies are.
    pub fn obj(mut self, key: &str, value: Json) -> Self {
        if value.parts.is_empty() {
            self.push(key, "{}".to_string());
            return self;
        }
        self.push(key, continuation(&value.render(2), 2));
        self
    }

    /// An array of nested objects.
    pub fn array(mut self, key: &str, values: Vec<Json>) -> Self {
        if values.is_empty() {
            self.push(key, "[]".to_string());
            return self;
        }
        let rendered: Vec<String> = values.into_iter().map(|value| indent(&value.render(2), 4)).collect();
        self.push(key, format!("[\n{}\n  ]", rendered.join(",\n")));
        self
    }

    /// An array of strings.
    pub fn strings(mut self, key: &str, values: &[String]) -> Self {
        let rendered: Vec<String> = values.iter().map(|value| format!("\"{}\"", escape(value))).collect();
        self.push(key, format!("[{}]", rendered.join(", ")));
        self
    }

    fn render(self, indent_width: usize) -> String {
        let pad = " ".repeat(indent_width);
        let inner: Vec<String> = self.parts.iter().map(|part| format!("{pad}{part}")).collect();
        format!("{{\n{}\n{}}}", inner.join(",\n"), " ".repeat(indent_width.saturating_sub(2)))
    }

    /// Renders the object as a complete file, newline-terminated.
    pub fn render_file(self) -> String {
        format!("{}\n", self.render(2))
    }
}

fn indent(text: &str, width: usize) -> String {
    let pad = " ".repeat(width);
    text.lines().map(|line| format!("{pad}{line}")).collect::<Vec<_>>().join("\n")
}

/// Shifts every line but the first, so a nested value renders under the key that introduces it.
fn continuation(text: &str, width: usize) -> String {
    let pad = " ".repeat(width);
    let mut lines = text.lines();
    let first = lines.next().unwrap_or_default().to_string();
    let rest: Vec<String> = lines.map(|line| format!("{pad}{line}")).collect();
    if rest.is_empty() {
        first
    } else {
        format!("{first}\n{}", rest.join("\n"))
    }
}

fn escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

// ---------------------------------------------------------------------------------------------
// Spec-table byte builders. Every offset below is copied from the protocol's own table.
// ---------------------------------------------------------------------------------------------

/// §2's 16-byte control header plus its payload.
pub fn control_frame(opcode: u16, flags: u16, request_id: u32, payload: &[u8]) -> Vec<u8> {
    let mut record = zeros(HEADER_LEN + payload.len());
    bytes_at(&mut record, 0, b"OBCP");
    record[4] = 3;
    record[5] = 0;
    u16_at(&mut record, 6, opcode);
    u16_at(&mut record, 8, flags);
    u16_at(&mut record, 10, payload.len() as u16);
    u32_at(&mut record, 12, request_id);
    bytes_at(&mut record, HEADER_LEN, payload);
    record
}

/// §2.2's metadata envelope: an eight-byte header then `(tag, length, value)` fields.
pub fn envelope(schema_id: u16, schema_version: u8, fields: &[(u16, Vec<u8>)]) -> Vec<u8> {
    let body_len: usize = fields.iter().map(|(_, value)| 4 + value.len()).sum();
    let mut out = zeros(8 + body_len);
    u16_at(&mut out, 0, schema_id);
    out[2] = schema_version;
    u16_at(&mut out, 4, body_len as u16);
    u16_at(&mut out, 6, fields.len() as u16);
    let mut offset = 8;
    for (tag, value) in fields {
        u16_at(&mut out, offset, *tag);
        u16_at(&mut out, offset + 2, value.len() as u16);
        bytes_at(&mut out, offset + 4, value);
        offset += 4 + value.len();
    }
    out
}

/// §5's 12-byte Hello.
pub fn hello(min_major: u8, max_major: u8, control: u16, stream: u16, page_kind: u8, page_index: u8) -> Vec<u8> {
    let mut out = zeros(12);
    out[0] = min_major;
    out[1] = max_major;
    u16_at(&mut out, 2, control);
    u16_at(&mut out, 4, stream);
    out[10] = page_kind;
    out[11] = page_index;
    out
}

/// §5.1's 56-byte ResourceLimits block, at the values the storage contract freezes.
pub fn resource_limits(available_bytes: u64) -> Vec<u8> {
    let mut out = zeros(56);
    out[0] = 1;
    out[1] = 56;
    u16_at(&mut out, 4, 256);
    out[6] = 8;
    out[7] = 4;
    out[8] = 1;
    out[9] = 32;
    out[10] = 32;
    out[11] = 11;
    out[12] = 4;
    out[13] = 8;
    u16_at(&mut out, 14, 64);
    u16_at(&mut out, 16, 256);
    u64_at(&mut out, 20, 0x0000_0000_FFFF_FFFF);
    u64_at(&mut out, 28, available_bytes);
    u16_at(&mut out, 36, 64);
    u16_at(&mut out, 38, 16);
    u16_at(&mut out, 40, 128);
    u16_at(&mut out, 42, 1);
    u16_at(&mut out, 44, 8);
    u16_at(&mut out, 46, 8);
    out[48] = 1;
    out[49] = 1;
    out[50] = 1;
    out
}

/// §5's 20-byte subject entry.
#[allow(clippy::too_many_arguments)]
pub fn subject(
    namespace: u8,
    kind_code: u16,
    operation_flags: u16,
    policy_flags: u16,
    put_schema: u8,
    patch_schema: u8,
    catalog_schema: u8,
    max_length: u64,
) -> Vec<u8> {
    let mut out = zeros(20);
    out[0] = namespace;
    u16_at(&mut out, 2, kind_code);
    u16_at(&mut out, 4, operation_flags);
    u16_at(&mut out, 6, policy_flags);
    out[8] = put_schema;
    out[9] = patch_schema;
    out[10] = catalog_schema;
    u64_at(&mut out, 12, max_length);
    out
}

/// §5's 56-byte Capabilities prefix, followed by whichever page body is supplied.
#[allow(clippy::too_many_arguments)]
pub fn capabilities(
    status_flags: u16,
    store: Option<[u8; 16]>,
    link_kind: u8,
    authenticated: bool,
    capability_revision: u32,
    command_flags: u32,
    total_subjects: u16,
    page_kind: u8,
    page_index: u8,
    returned_subjects: u8,
    total_pages: u8,
    body: &[u8],
) -> Vec<u8> {
    let mut out = zeros(56 + body.len());
    out[0] = 3;
    out[1] = 1;
    u16_at(&mut out, 2, status_flags);
    if let Some(store) = store {
        bytes_at(&mut out, 4, &store);
    }
    u16_at(&mut out, 20, 244);
    u16_at(&mut out, 22, 1024);
    u32_at(&mut out, 24, FIXTURE_GRANULE);
    u16_at(&mut out, 28, 64);
    u16_at(&mut out, 30, 128);
    u16_at(&mut out, 32, 96);
    u16_at(&mut out, 34, 192);
    u16_at(&mut out, 36, 64);
    out[38] = link_kind;
    out[39] = u8::from(authenticated);
    u32_at(&mut out, 40, capability_revision);
    u32_at(&mut out, 44, command_flags);
    u16_at(&mut out, 48, total_subjects);
    out[50] = page_kind;
    out[51] = page_index;
    out[52] = returned_subjects;
    out[53] = total_pages;
    out[54] = 1;
    out[55] = 0;
    bytes_at(&mut out, 56, body);
    out
}

/// §6.1's 48-byte StartUpload prefix plus exactly one metadata envelope.
#[allow(clippy::too_many_arguments)]
pub fn start_upload(
    operation: [u8; 16],
    kind: u16,
    mode: u8,
    resume: u8,
    logical_id: u64,
    revision: u64,
    length: u64,
    crc: u32,
    metadata: &[u8],
) -> Vec<u8> {
    let mut out = zeros(48 + metadata.len());
    bytes_at(&mut out, 0, &operation);
    u16_at(&mut out, 16, kind);
    out[18] = mode;
    out[19] = resume;
    u64_at(&mut out, 20, logical_id);
    u64_at(&mut out, 28, revision);
    u64_at(&mut out, 36, length);
    u32_at(&mut out, 44, crc);
    bytes_at(&mut out, 48, metadata);
    out
}

/// §6.1's 64-byte UploadAccepted, disposition `0`.
#[allow(clippy::too_many_arguments)]
pub fn upload_accepted(
    mode: u8,
    flags: u16,
    operation: [u8; 16],
    session: u32,
    logical_id: u64,
    admission_revision: u64,
    durable_offset: u64,
    granule: u32,
    max_payload: u16,
    prefix_crc: u32,
) -> Vec<u8> {
    let mut out = zeros(64);
    out[1] = mode;
    u16_at(&mut out, 2, flags);
    bytes_at(&mut out, 4, &operation);
    u32_at(&mut out, 20, session);
    u64_at(&mut out, 24, logical_id);
    u64_at(&mut out, 32, admission_revision);
    u64_at(&mut out, 40, durable_offset);
    u32_at(&mut out, 48, granule);
    u16_at(&mut out, 52, max_payload);
    u32_at(&mut out, 56, prefix_crc);
    out
}

/// §10's ResultEnvelope prefix around a typed body.
pub fn result_envelope(result_type: u8, body: &[u8]) -> Vec<u8> {
    let mut out = zeros(4 + body.len());
    out[0] = result_type;
    bytes_at(&mut out, 4, body);
    out
}

/// §10's 64-byte ObjectResult.
#[allow(clippy::too_many_arguments)]
pub fn object_result(
    operation: [u8; 16],
    store: [u8; 16],
    kind: u16,
    outcome: u16,
    logical_id: u64,
    revision: u64,
    length: u64,
    crc: u32,
) -> Vec<u8> {
    let mut out = zeros(64);
    bytes_at(&mut out, 0, &operation);
    bytes_at(&mut out, 16, &store);
    u16_at(&mut out, 32, kind);
    u16_at(&mut out, 34, outcome);
    u64_at(&mut out, 36, logical_id);
    u64_at(&mut out, 44, revision);
    u64_at(&mut out, 52, length);
    u32_at(&mut out, 60, crc);
    out
}

/// §10's 88-byte DraftPartResult.
#[allow(clippy::too_many_arguments)]
pub fn draft_part_result(
    child: [u8; 16],
    store: [u8; 16],
    parent: [u8; 16],
    part_ref: [u8; 16],
    part_kind: u16,
    part_key: u64,
    length: u64,
    crc: u32,
) -> Vec<u8> {
    let mut out = zeros(88);
    bytes_at(&mut out, 0, &child);
    bytes_at(&mut out, 16, &store);
    bytes_at(&mut out, 32, &parent);
    bytes_at(&mut out, 48, &part_ref);
    u16_at(&mut out, 64, part_kind);
    u64_at(&mut out, 68, part_key);
    u64_at(&mut out, 76, length);
    u32_at(&mut out, 84, crc);
    out
}

/// §10's 56-byte AbortResult.
pub fn abort_result(operation: [u8; 16], store: [u8; 16], target: [u8; 16], disposition: u8) -> Vec<u8> {
    let mut out = zeros(56);
    bytes_at(&mut out, 0, &operation);
    bytes_at(&mut out, 16, &store);
    bytes_at(&mut out, 32, &target);
    out[48] = disposition;
    out
}

/// §12's ErrorBody: a 48-byte prefix and optional text.
#[allow(clippy::too_many_arguments)]
pub fn error_body(
    category: u16,
    namespace: u16,
    detail: u16,
    guidance: u8,
    owner: u8,
    presence: u16,
    retry_after_ms: u32,
    expected_offset: u64,
    current_revision: u64,
    required_bytes: u64,
    available_bytes: u64,
    text: &[u8],
) -> Vec<u8> {
    let mut out = zeros(48 + text.len());
    u16_at(&mut out, 0, category);
    u16_at(&mut out, 2, namespace);
    u16_at(&mut out, 4, detail);
    out[6] = guidance;
    out[7] = owner;
    u16_at(&mut out, 8, presence);
    u32_at(&mut out, 10, retry_after_ms);
    u64_at(&mut out, 14, expected_offset);
    u64_at(&mut out, 22, current_revision);
    u64_at(&mut out, 30, required_bytes);
    u64_at(&mut out, 38, available_bytes);
    out[46] = text.len() as u8;
    bytes_at(&mut out, 48, text);
    out
}

/// A text-free ErrorBody with only the given presence bits — §11's retained-Aborted replay shape.
pub fn bare_error(category: u16, detail: u16, presence: u16) -> Vec<u8> {
    error_body(category, 0, detail, 0, 0, presence, 0, 0, 0, 0, 0, &[])
}

/// §8.2's 44-byte catalog page prefix plus its entries.
pub fn catalog_page(
    store: [u8; 16],
    kind: u16,
    entry_count: u16,
    revision: u64,
    next_cursor: &[u8; 16],
    entries: &[u8],
) -> Vec<u8> {
    let mut out = zeros(44 + entries.len());
    bytes_at(&mut out, 0, &store);
    u16_at(&mut out, 16, kind);
    u16_at(&mut out, 18, entry_count);
    u64_at(&mut out, 20, revision);
    bytes_at(&mut out, 28, next_cursor);
    bytes_at(&mut out, 44, entries);
    out
}

/// §8.2's 36-byte catalog entry prefix plus its projection envelope.
pub fn catalog_entry(logical_id: u64, revision: u64, length: u64, crc: u32, metadata: &[u8]) -> Vec<u8> {
    let mut out = zeros(36 + metadata.len());
    u64_at(&mut out, 0, logical_id);
    u64_at(&mut out, 8, revision);
    u64_at(&mut out, 16, length);
    u32_at(&mut out, 24, crc);
    u16_at(&mut out, 30, metadata.len() as u16);
    bytes_at(&mut out, 36, metadata);
    out
}

/// §8.2's 16-byte catalog cursor, CRC included.
pub fn catalog_cursor(store: [u8; 16], revision: u64, next_index: u16, kind: u16) -> [u8; 16] {
    let mut out = [0u8; 16];
    u64_at(&mut out, 0, revision);
    u16_at(&mut out, 8, next_index);
    u16_at(&mut out, 10, kind);
    let mut input = Vec::from(store);
    input.extend_from_slice(&out[..12]);
    u32_at(&mut out, 12, crc32(&input));
    out
}

/// §8.3's 16-byte draft cursor, whose CRC also binds the parent.
pub fn draft_cursor(store: [u8; 16], parent: [u8; 16], revision: u64, next_index: u16) -> [u8; 16] {
    let mut out = [0u8; 16];
    u64_at(&mut out, 0, revision);
    u16_at(&mut out, 8, next_index);
    let mut input = Vec::from(store);
    input.extend_from_slice(&parent);
    input.extend_from_slice(&out[..12]);
    u32_at(&mut out, 12, crc32(&input));
    out
}

/// §8.1's 24-byte progress body.
pub fn progress(namespace: u8, phase: u8, flags: u8, kind: u16, logical_id: u64, durable_offset: u64) -> Vec<u8> {
    let mut out = zeros(24);
    out[0] = namespace;
    out[1] = phase;
    out[2] = flags;
    u16_at(&mut out, 4, kind);
    u64_at(&mut out, 8, logical_id);
    u64_at(&mut out, 16, durable_offset);
    out
}

/// §8.1's QueryOperation response: a state byte, three reserved bytes, then the state's body.
pub fn operation_status(state: u8, body: &[u8]) -> Vec<u8> {
    let mut out = zeros(4 + body.len());
    out[0] = state;
    bytes_at(&mut out, 4, body);
    out
}

/// §8.3's 68-byte draft entry.
#[allow(clippy::too_many_arguments)]
pub fn draft_entry(
    child: [u8; 16],
    part_ref: [u8; 16],
    part_kind: u16,
    part_key: u64,
    state: u8,
    durable_offset: u64,
    length: u64,
    crc: u32,
) -> Vec<u8> {
    let mut out = zeros(68);
    bytes_at(&mut out, 0, &child);
    bytes_at(&mut out, 16, &part_ref);
    u16_at(&mut out, 32, part_kind);
    u64_at(&mut out, 36, part_key);
    out[44] = state;
    u64_at(&mut out, 48, durable_offset);
    u64_at(&mut out, 56, length);
    u32_at(&mut out, 64, crc);
    out
}

/// §8.4's 96-byte weather request context.
#[allow(clippy::too_many_arguments)]
pub fn weather_context(
    store: [u8; 16],
    current_request: u64,
    context_revision: u64,
    head_present: bool,
    singleton: u64,
    repository_revision: u64,
    head_request: u64,
    latitude_e7: i32,
    longitude_e7: i32,
    radius_m: u32,
    earliest_issued: i64,
    valid_until: i64,
    state: u8,
) -> Vec<u8> {
    let mut out = zeros(96);
    bytes_at(&mut out, 0, &store);
    u64_at(&mut out, 16, current_request);
    u64_at(&mut out, 24, context_revision);
    u32_at(&mut out, 32, u32::from(head_present));
    u64_at(&mut out, 36, singleton);
    u64_at(&mut out, 44, repository_revision);
    u64_at(&mut out, 52, head_request);
    i32_at(&mut out, 60, latitude_e7);
    i32_at(&mut out, 64, longitude_e7);
    u32_at(&mut out, 68, radius_m);
    i64_at(&mut out, 72, earliest_issued);
    i64_at(&mut out, 80, valid_until);
    out[88] = state;
    out
}

/// §13's 16-byte stream header plus its payload.
pub fn stream_frame(session: u32, offset: u64, direction: u8, flags: u8, payload: &[u8]) -> Vec<u8> {
    let mut out = zeros(16 + payload.len());
    u32_at(&mut out, 0, session);
    u64_at(&mut out, 4, offset);
    u16_at(&mut out, 12, payload.len() as u16);
    out[14] = direction;
    out[15] = flags;
    bytes_at(&mut out, 16, payload);
    out
}

/// §13's 24-byte fault body.
pub fn fault_body(category: u16, detail: u16, expected_offset: u64, durable_offset: u64, disposition: u8) -> Vec<u8> {
    let mut out = zeros(24);
    u16_at(&mut out, 0, category);
    u16_at(&mut out, 2, detail);
    u64_at(&mut out, 4, expected_offset);
    u64_at(&mut out, 12, durable_offset);
    out[20] = disposition;
    out
}

/// §16's 64-byte GetDeviceStatus response.
pub fn device_status(status_flags: u16, mount_class: u8, store: Option<[u8; 16]>) -> Vec<u8> {
    let mut out = zeros(64);
    u16_at(&mut out, 0, 1);
    u16_at(&mut out, 2, 4);
    u16_at(&mut out, 4, 2);
    u16_at(&mut out, 6, 3);
    bytes_at(&mut out, 8, &SERIAL);
    u32_at(&mut out, 24, 412);
    u64_at(&mut out, 28, 86_400);
    u32_at(&mut out, 36, 24_576);
    u16_at(&mut out, 40, status_flags);
    out[42] = mount_class;
    u32_at(&mut out, 44, 9911);
    if let Some(store) = store {
        bytes_at(&mut out, 48, &store);
    }
    out
}

/// §16's 56-byte configuration block.
pub fn config_block(unit_flags: u8, weather_refresh: u8, name: &[u8]) -> Vec<u8> {
    let mut out = zeros(56);
    out[0] = 1;
    out[1] = 56;
    out[4] = name.len() as u8;
    out[5] = unit_flags;
    out[6] = weather_refresh;
    bytes_at(&mut out, 8, name);
    out
}

/// §16's 16-byte SetClock request.
pub fn set_clock(epoch_seconds: i64, source: u8) -> Vec<u8> {
    let mut out = zeros(16);
    i64_at(&mut out, 0, epoch_seconds);
    out[8] = source;
    out
}

/// §16's 16-byte SetClock response.
pub fn clock_status(epoch_seconds: i64, source: u8, state: u8) -> Vec<u8> {
    let mut out = zeros(16);
    i64_at(&mut out, 0, epoch_seconds);
    out[8] = source;
    out[9] = state;
    out
}

/// §11's canonical-intent 36-byte prefix plus a suffix.
pub fn canonical_intent(store: [u8; 16], opcode: u16, suffix: &[u8]) -> Vec<u8> {
    let mut out = zeros(36 + suffix.len());
    bytes_at(&mut out, 0, b"OBC-DOS3-INTENT\0");
    bytes_at(&mut out, 16, &store);
    u16_at(&mut out, 32, opcode);
    out[34] = 1;
    bytes_at(&mut out, 36, suffix);
    out
}

/// SHA-256 as the vectors record it.
pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex(&hasher.finalize())
}

// ---------------------------------------------------------------------------------------------
// Fixture inventories.
// ---------------------------------------------------------------------------------------------

/// One control vector: a whole frame plus the semantics it freezes.
#[derive(Debug, Clone)]
pub struct ControlVector {
    /// Stable name.
    pub name: String,
    /// `"request"` or `"response"`.
    pub direction: &'static str,
    /// The opcode.
    pub opcode: Opcode,
    /// The header flags word.
    pub flags: u16,
    /// The RequestId.
    pub request_id: u32,
    /// The payload bytes.
    pub payload: Vec<u8>,
    /// A one-line note naming the rule it pins.
    pub note: String,
    /// `"minimum"`, `"maximum"`, `"ceiling"`, or empty.
    pub boundary: &'static str,
}

impl ControlVector {
    /// The complete record.
    pub fn frame(&self) -> Vec<u8> {
        control_frame(self.opcode.to_u16(), self.flags, self.request_id, &self.payload)
    }

    /// The semantic body §1 requires, read from the payload at the protocol's own offsets.
    pub fn body(&self) -> semantics::Body {
        semantics::control_body(self.direction, self.opcode, self.flags, &self.payload)
    }

    fn to_fixture(&self) -> Fixture {
        let frame = self.frame();
        let json = Json::new()
            .str("name", &self.name)
            .str("suite", "device-object-v2")
            .str("kind", "control")
            .str("direction", self.direction)
            .obj("opcode", Json::new().str("name", self.opcode.name()).num("value", i64::from(self.opcode.to_u16())))
            .obj(
                "header",
                Json::new()
                    .str("magic", "OBCP")
                    .num("major", 3)
                    .num("minor", 0)
                    .num("flags", i64::from(self.flags))
                    .num("payloadLength", self.payload.len() as i64)
                    .num("requestId", i64::from(self.request_id)),
            )
            .str("boundary", self.boundary)
            .str("note", &self.note)
            .obj("body", self.body().to_json())
            .str("payload", &hex(&self.payload))
            .str("frame", &hex(&frame))
            .render_file();
        Fixture { name: self.name.clone(), category: Category::Control, json }
    }
}

/// What a negative fixture's bytes are fed to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NegativeTarget {
    /// A whole control record.
    ControlFrame,
    /// A whole control record whose opcode-specific body is the fault.
    ControlBody(Opcode, bool),
    /// A whole stream record.
    StreamFrame,
    /// A bare metadata envelope, against the ceiling its declared class imposes.
    ///
    /// §2.2 makes that ceiling a **call-site** fact: an envelope's position in its message fixes
    /// the bound, and deriving it from the version byte instead would measure an envelope that lies
    /// about its version against the ceiling it claims rather than the one its position imposes. A
    /// raw-envelope fixture therefore declares the class it is decoded in, and a harness that
    /// hard-codes one ceiling for every such fixture is testing a rule the contract does not have.
    MetadataEnvelope(SchemaClass),
    /// A bare `ErrorBody`.
    ErrorBody,
    /// A bare Capabilities payload.
    CapabilitiesPayload,
    /// A bare subject entry.
    SubjectEntry,
    /// A bare configuration block.
    ConfigBlock,
    /// A ResetStore echo, checked against the mount class the device is reporting.
    ///
    /// §16 makes the echo an admission rule rather than a decode rule — "It MUST equal the StoreId
    /// the device currently reports" — so the fixture has to carry the context the check is made
    /// against. The `u8` is that mount class.
    ResetStoreEcho(u8),
}

impl NegativeTarget {
    fn name(self) -> String {
        match self {
            NegativeTarget::ControlFrame => "controlFrame".to_string(),
            NegativeTarget::ControlBody(opcode, response) => {
                format!("{}{}", opcode.name(), if response { "Response" } else { "Request" })
            }
            NegativeTarget::StreamFrame => "streamFrame".to_string(),
            NegativeTarget::MetadataEnvelope(_) => "metadataEnvelope".to_string(),
            NegativeTarget::ErrorBody => "errorBody".to_string(),
            NegativeTarget::CapabilitiesPayload => "capabilities".to_string(),
            NegativeTarget::SubjectEntry => "subjectEntry".to_string(),
            NegativeTarget::ConfigBlock => "configBlock".to_string(),
            NegativeTarget::ResetStoreEcho(class) => format!("resetStoreEcho(mountClass={class})"),
        }
    }
}

/// One rejection fixture.
#[derive(Debug, Clone)]
pub struct NegativeVector {
    /// Stable name.
    pub name: String,
    /// What the bytes are fed to.
    pub target: NegativeTarget,
    /// The bytes.
    pub bytes: Vec<u8>,
    /// The category every conforming codec must answer with.
    pub category: crate::ErrorCategory,
    /// The detail inside it.
    pub detail: u16,
    /// A one-line note naming the rule.
    pub note: String,
}

impl NegativeVector {
    fn to_fixture(&self) -> Fixture {
        let mut json = Json::new()
            .str("name", &self.name)
            .str("suite", "device-object-v2")
            .str("kind", "negative")
            .str("target", &self.target.name());
        if let NegativeTarget::MetadataEnvelope(class) = self.target {
            // The class is the fixture's own declaration of the position it is decoded in; the
            // length is the ceiling that follows from it, so a suite can check its own constant.
            json = json.str("class", class.name()).num("maximumEncodedLength", class.ceiling() as i64);
        }
        let json = json
            .str("note", &self.note)
            .obj(
                "expect",
                Json::new()
                    .str("category", self.category.name())
                    .num("categoryValue", i64::from(self.category.get()))
                    .str("detail", crate::error::detail_name(self.category, 0, self.detail))
                    .num("detailValue", i64::from(self.detail)),
            )
            .str("bytes", &hex(&self.bytes))
            .render_file();
        Fixture { name: self.name.clone(), category: Category::Negative, json }
    }
}

/// One stream vector.
#[derive(Debug, Clone)]
pub struct StreamVector {
    /// Stable name.
    pub name: String,
    /// The record bytes.
    pub record: Vec<u8>,
    /// A one-line note.
    pub note: String,
}

impl StreamVector {
    fn to_fixture(&self) -> Fixture {
        let json = Json::new()
            .str("name", &self.name)
            .str("suite", "device-object-v2")
            .str("kind", "stream")
            .num("sessionId", i64::from(u32::from_le_bytes(self.record[0..4].try_into().unwrap())))
            .big("offset", &u64::from_le_bytes(self.record[4..12].try_into().unwrap()).to_string())
            .num("payloadLength", i64::from(u16::from_le_bytes(self.record[12..14].try_into().unwrap())))
            .num("direction", i64::from(self.record[14]))
            .num("flags", i64::from(self.record[15]))
            .str("note", &self.note)
            .str("record", &hex(&self.record))
            .render_file();
        Fixture { name: self.name.clone(), category: Category::Stream, json }
    }
}

/// One canonical-intent golden.
#[derive(Debug, Clone)]
pub struct IntentVector {
    /// Stable name.
    pub name: String,
    /// The opcode whose suffix it carries.
    pub opcode: Opcode,
    /// The exact canonical bytes.
    pub bytes: Vec<u8>,
    /// A one-line note.
    pub note: String,
}

impl IntentVector {
    fn to_fixture(&self) -> Fixture {
        let json = Json::new()
            .str("name", &self.name)
            .str("suite", "device-object-v2")
            .str("kind", "canonicalIntent")
            .obj("opcode", Json::new().str("name", self.opcode.name()).num("value", i64::from(self.opcode.to_u16())))
            .str("storeId", &hex(&STORE))
            .num("prefixLength", 36)
            .num("suffixLength", (self.bytes.len() - 36) as i64)
            .str("note", &self.note)
            .str("bytes", &hex(&self.bytes))
            .str("sha256", &sha256_hex(&self.bytes))
            .render_file();
        Fixture { name: self.name.clone(), category: Category::Control, json }
    }
}

/// One frame-limit derivation case (§14.0, and the vectors contract's §2.2).
#[derive(Debug, Clone, Copy)]
pub struct DerivationCase {
    /// `"control"` or `"stream"`.
    pub channel: &'static str,
    /// The link fact the ceiling comes from: an ATT MTU on the control channel, a CoC SDU on the
    /// stream channel.
    pub link_value: u16,
    /// The transport ceiling that link fact yields.
    pub ceiling: u16,
    /// The client's advertised maximum.
    pub client_max: u16,
    /// The device's advertised maximum.
    pub device_max: u16,
    /// `"negotiated"`, `"belowProtocolMinimum"`, or `"undeliverable"`.
    pub outcome: &'static str,
    /// The negotiated limit when there is one, else zero.
    pub negotiated: u16,
    /// The rule this case pins.
    pub note: &'static str,
}

/// The frame-limit derivation cases, pinned as data rather than as prose.
#[derive(Debug, Clone)]
pub struct DerivationVector {
    /// Stable name.
    pub name: String,
    /// The cases.
    pub cases: Vec<DerivationCase>,
}

impl DerivationVector {
    fn to_fixture(&self) -> Fixture {
        let cases = self
            .cases
            .iter()
            .map(|case| {
                Json::new()
                    .str("channel", case.channel)
                    .num("linkValue", i64::from(case.link_value))
                    .num("transportCeiling", i64::from(case.ceiling))
                    .num("clientMaximum", i64::from(case.client_max))
                    .num("deviceMaximum", i64::from(case.device_max))
                    .str("outcome", case.outcome)
                    .num("negotiated", i64::from(case.negotiated))
                    .str("note", case.note)
            })
            .collect();
        let json = Json::new()
            .str("name", &self.name)
            .str("suite", "device-object-v2")
            .str("kind", "frameLimitDerivation")
            .num("protocolMinimumControlFrame", 192)
            .num("protocolMinimumStreamFrame", 64)
            .num("maximumControlFrame", 512)
            .num("maximumStreamFrame", 4096)
            .array("cases", cases)
            .render_file();
        Fixture { name: self.name.clone(), category: Category::Control, json }
    }
}

/// One event in a transcript.
#[derive(Debug, Clone)]
pub struct Event {
    /// `"client"` or `"device"`.
    pub actor: &'static str,
    /// The principal scope.
    pub principal: &'static str,
    /// The link kind.
    pub link: &'static str,
    /// The connection generation.
    pub generation: u32,
    /// What happens.
    pub note: String,
    /// The control or stream record, when the event carries one.
    pub record: Option<Vec<u8>>,
    /// `"control"`, `"stream"`, or `"injected"`.
    pub channel: &'static str,
}

impl Event {
    fn to_json(&self) -> Json {
        let json = Json::new()
            .str("actor", self.actor)
            .str("principal", self.principal)
            .str("link", self.link)
            .num("connectionGeneration", i64::from(self.generation))
            .str("channel", self.channel)
            .str("note", &self.note);
        match &self.record {
            Some(record) => json.str("record", &hex(record)),
            None => json.str("record", ""),
        }
    }
}

/// One semantic transcript.
#[derive(Debug, Clone)]
pub struct Transcript {
    /// Stable name.
    pub name: String,
    /// What the flow proves.
    pub description: String,
    /// The ordered events.
    pub events: Vec<Event>,
}

impl Transcript {
    fn to_fixture(&self) -> Fixture {
        let json = Json::new()
            .str("name", &self.name)
            .str("suite", "device-object-v2")
            .str("kind", "transcript")
            .str("description", &self.description)
            .num("eventCount", self.events.len() as i64)
            .array("events", self.events.iter().map(Event::to_json).collect())
            .render_file();
        Fixture { name: self.name.clone(), category: Category::Transcript, json }
    }
}

mod inventory;
pub mod semantics;

pub use inventory::{controls, derivations, intents, negatives, progress_matrix, streams, transcripts};
pub use semantics::{control_body, Body, Value};

/// Every fixture the suite contains, in manifest order.
pub fn fixtures() -> Vec<Fixture> {
    let mut all = Vec::new();
    all.extend(controls().iter().map(ControlVector::to_fixture));
    all.extend(intents().iter().map(IntentVector::to_fixture));
    all.extend(derivations().iter().map(DerivationVector::to_fixture));
    all.extend(streams().iter().map(StreamVector::to_fixture));
    all.extend(negatives().iter().map(NegativeVector::to_fixture));
    all.extend(transcripts().iter().map(Transcript::to_fixture));
    all
}

/// The `storage` array, read from the checked-in `storage/` directory.
///
/// This producer does not build those bytes — `obc-storage`'s does, because §6's storage vectors are
/// OBC2 record layouts and crash cuts that no wire codec can produce. What belongs here is the
/// *index*, since `Device_Object_Vectors_v2.md` §1 gives the suite one manifest. Listing them by
/// digest of the file on disk keeps the guard honest in both directions: editing a storage fixture
/// without regenerating fails this crate's manifest check, and regenerating them without updating
/// the manifest fails `obc-storage`'s.
fn storage_entries() -> Vec<Json> {
    let directory = dir().join("storage");
    let Ok(read) = std::fs::read_dir(&directory) else { return Vec::new() };
    let mut names: Vec<String> = read
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.ends_with(".json"))
        .collect();
    names.sort();
    names
        .into_iter()
        .filter_map(|name| {
            let bytes = std::fs::read(directory.join(&name)).ok()?;
            let stem = name.trim_end_matches(".json").to_string();
            use sha2::{Digest, Sha256};
            let mut hasher = Sha256::new();
            hasher.update(&bytes);
            Some(
                Json::new()
                    .str("name", &stem)
                    .str("file", &format!("storage/{name}"))
                    .str("sha256", &hex(&hasher.finalize())),
            )
        })
        .collect()
}

/// The suite manifest: `Device_Object_Vectors_v2.md` §1's four scalars and five arrays.
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
    Json::new()
        .str("suite", "device-object-v2")
        .num("format", 1)
        .num("wire_major", 3)
        .num("storage_format", 1)
        .str(
            "storage_note",
            "The storage array indexes files under storage/, which obc-storage's own spec-derived producer \
             writes: Device_Object_Vectors_v2.md section 6 covers OBC2 record layouts and crash cuts, which \
             no wire codec can produce. This manifest lists them by name and digest; obc-storage owns their \
             bytes and holds its own guard over them.",
        )
        .array("controls", entries(Category::Control))
        .array("streams", entries(Category::Stream))
        .array("storage", storage_entries())
        .array("negative", entries(Category::Negative))
        .array("transcripts", entries(Category::Transcript))
        .render_file()
}

/// Writes the whole suite to `specs/vectors/device-object-v2/`.
pub fn write_all() -> std::io::Result<usize> {
    let root = dir();
    for category in [Category::Control, Category::Stream, Category::Negative, Category::Transcript] {
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
