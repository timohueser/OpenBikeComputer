//! Protocol v4's bytes: the control frame, the seven request bodies, the response bodies, the
//! stream frame and the error body.
//!
//! `FLAT_Store_Protocol.md` §3 is the sole authority and every offset below is transcribed from its
//! tables. Decoding is **total**: an input is either a typed message or a typed [`Refusal`] carrying
//! the contract's own code and detail, and nothing here panics on hostile bytes. Encoding writes
//! exact bytes into a caller-provided slice and reports the length — this crate allocates nothing,
//! on the device or on the host.
//!
//! The codec holds no state and knows no policy. It does not decide whether a `PUT` may replace a
//! ride, whether a listing is stale, or what a kind's validator thinks; those are
//! [`super::engine`]'s, and the split is what lets the same bytes be produced by a fixture producer
//! that never calls this code.

use super::ids::{DisplayName, EntryMeta, ObjectId, ObjectKind, Revision, StoreId, NAME_CAPACITY};

/// The wire major this module implements. It is a transport fact (§4), never negotiated.
pub const WIRE_MAJOR: u8 = 4;

/// The four bytes every control frame opens with.
pub const MAGIC: [u8; 4] = *b"OBC4";

/// The control frame header, §3.1.
pub const HEADER_LEN: usize = 16;

/// The stream frame, §3.8. A stream record is this followed by exactly `payload length` bytes.
pub const STREAM_HEADER_LEN: usize = 16;

/// An error response payload, §3.9. Exactly this, never more and never less.
pub const ERROR_BODY_LEN: usize = 16;

/// `StoreId` plus commit sequence, ahead of a `LIST` page's entries (§3.3).
pub const LIST_PREFIX_LEN: usize = 24;

/// One `LIST` entry (§3.3).
pub const LIST_ENTRY_LEN: usize = 88;

/// The smallest control record that can carry this protocol: a header plus a single-entry `LIST`
/// page (§5.1). A link below this floor is refused rather than truncated.
pub const CONTROL_FLOOR: usize = HEADER_LEN + LIST_PREFIX_LEN + LIST_ENTRY_LEN;

/// The largest fixed request, which is `PUT` (§5.1).
pub const MAX_REQUEST_FRAME: usize = HEADER_LEN + PUT_BODY_LEN;

const LIST_BODY_LEN: usize = 32;
const STATUS_BODY_LEN: usize = 16;
const GET_BODY_LEN: usize = 16;
const PUT_BODY_LEN: usize = 84;
const REMOVE_BODY_LEN: usize = 16;
const CANCEL_BODY_LEN: usize = 4;
const ARM_BODY_LEN: usize = 16;

/// A client-chosen transfer identifier (§3.1). Nonzero: a zero one is unanswerable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct RequestId(pub u32);

/// §3.2's opcode table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Opcode {
    List = 0x01,
    Status = 0x02,
    Get = 0x03,
    Put = 0x04,
    Remove = 0x05,
    Cancel = 0x06,
    Arm = 0x07,
}

impl Opcode {
    /// Decodes §3.2's byte. Anything else is `unsupported`.
    pub fn decode(value: u8) -> Option<Self> {
        Some(match value {
            0x01 => Opcode::List,
            0x02 => Opcode::Status,
            0x03 => Opcode::Get,
            0x04 => Opcode::Put,
            0x05 => Opcode::Remove,
            0x06 => Opcode::Cancel,
            0x07 => Opcode::Arm,
            _ => return None,
        })
    }

    /// The byte §3.2 registers.
    pub fn value(self) -> u8 {
        self as u8
    }

    /// The exact payload length this opcode's request carries.
    fn request_body_len(self) -> usize {
        match self {
            Opcode::List => LIST_BODY_LEN,
            Opcode::Status => STATUS_BODY_LEN,
            Opcode::Get => GET_BODY_LEN,
            Opcode::Put => PUT_BODY_LEN,
            Opcode::Remove => REMOVE_BODY_LEN,
            Opcode::Cancel => CANCEL_BODY_LEN,
            Opcode::Arm => ARM_BODY_LEN,
        }
    }
}

/// §3.1's flag bits.
pub mod flags {
    /// A successful response.
    pub const RESPONSE: u16 = 1 << 0;
    /// An error response; its payload is exactly one 16-byte error body.
    pub const ERROR: u16 = 1 << 1;
    /// A further `LIST` page exists.
    pub const MORE: u16 = 1 << 2;
}

/// §3.9's code table. Code `0` is invalid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum ErrorCode {
    Unsupported = 1,
    InvalidFrame = 2,
    InvalidRequest = 3,
    NotFound = 4,
    RevisionConflict = 5,
    NoSpace = 6,
    ChecksumFailure = 7,
    MediaIo = 8,
    Busy = 9,
    Cancelled = 10,
    Rejected = 11,
    Internal = 12,
    CatalogChanged = 13,
    ReadOnly = 14,
}

impl ErrorCode {
    /// The `u16` §3.9 registers.
    pub fn value(self) -> u16 {
        self as u16
    }

    /// Decodes §3.9's code, for a client and for the fixtures.
    pub fn decode(value: u16) -> Option<Self> {
        Some(match value {
            1 => ErrorCode::Unsupported,
            2 => ErrorCode::InvalidFrame,
            3 => ErrorCode::InvalidRequest,
            4 => ErrorCode::NotFound,
            5 => ErrorCode::RevisionConflict,
            6 => ErrorCode::NoSpace,
            7 => ErrorCode::ChecksumFailure,
            8 => ErrorCode::MediaIo,
            9 => ErrorCode::Busy,
            10 => ErrorCode::Cancelled,
            11 => ErrorCode::Rejected,
            12 => ErrorCode::Internal,
            13 => ErrorCode::CatalogChanged,
            14 => ErrorCode::ReadOnly,
            _ => return None,
        })
    }
}

/// §3.9's code-scoped details. `0` means no narrower fact.
pub mod detail {
    /// `unsupported`.
    pub mod unsupported {
        pub const OPCODE: u16 = 1;
        pub const KIND: u16 = 2;
        pub const WIRE_MAJOR: u16 = 3;
    }
    /// `invalidFrame`.
    pub mod invalid_frame {
        pub const MAGIC: u16 = 1;
        pub const LENGTH: u16 = 2;
        pub const TRUNCATED: u16 = 3;
        pub const TRAILING: u16 = 4;
    }
    /// `invalidRequest`.
    pub mod invalid_request {
        pub const RESERVED_BITS: u16 = 1;
        pub const UNKNOWN_ENUM: u16 = 2;
        pub const BAD_COMBINATION: u16 = 3;
        pub const STREAM_OFFSET: u16 = 4;
    }
    /// `notFound`.
    pub mod not_found {
        pub const OBJECT: u16 = 1;
        pub const REVISION: u16 = 2;
    }
    /// `revisionConflict`; context is the current head `Revision`.
    pub mod revision_conflict {
        pub const HEAD_DIFFERS: u16 = 1;
        pub const HEAD_ABSENT: u16 = 2;
    }
    /// `noSpace`; context is the bytes required.
    pub mod no_space {
        pub const EXTENTS: u16 = 1;
        pub const CATALOG_FULL: u16 = 2;
        pub const TOO_FRAGMENTED: u16 = 3;
    }
    /// `checksumFailure`; context is the declared payload CRC.
    pub mod checksum_failure {
        pub const PAYLOAD: u16 = 1;
    }
    /// `mediaIo`.
    pub mod media_io {
        pub const READ: u16 = 1;
        pub const WRITE: u16 = 2;
        pub const SYNC: u16 = 3;
    }
    /// `busy`; context is the `RequestId` of the live transfer.
    pub mod busy {
        pub const TRANSFER: u16 = 1;
    }
    /// `cancelled`.
    pub mod cancelled {
        pub const BY_CLIENT: u16 = 1;
        pub const BY_DEVICE: u16 = 2;
        pub const LINK_LOST: u16 = 3;
    }
    /// `catalogChanged`; context is the current commit sequence.
    pub mod catalog_changed {
        pub const LISTING: u16 = 1;
    }
    /// `readOnly`.
    pub mod read_only {
        pub const CATALOG_UNREADABLE: u16 = 1;
        pub const REVISION_SPACE_EXHAUSTED: u16 = 2;
        pub const UNFORMATTED: u16 = 3;
    }
}

/// One refusal, exactly as §3.9's body carries it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Refusal {
    pub code: ErrorCode,
    pub detail: u16,
    pub context: u64,
}

impl Refusal {
    /// A refusal with no narrower fact and no context.
    pub const fn plain(code: ErrorCode) -> Self {
        Refusal { code, detail: 0, context: 0 }
    }

    /// A refusal with a detail and no context.
    pub const fn new(code: ErrorCode, detail: u16) -> Self {
        Refusal { code, detail, context: 0 }
    }

    /// A refusal with a detail and the code's context.
    pub const fn with_context(code: ErrorCode, detail: u16, context: u64) -> Self {
        Refusal { code, detail, context }
    }

    /// The 16 bytes of §3.9's body.
    pub fn encode(&self) -> [u8; ERROR_BODY_LEN] {
        let mut body = [0u8; ERROR_BODY_LEN];
        body[0..2].copy_from_slice(&self.code.value().to_le_bytes());
        body[2..4].copy_from_slice(&self.detail.to_le_bytes());
        body[4..12].copy_from_slice(&self.context.to_le_bytes());
        body
    }

    /// Decodes §3.9's body. Code `0`, an unknown code or a nonzero tail is a malformed body.
    pub fn decode(body: &[u8]) -> Option<Self> {
        if body.len() != ERROR_BODY_LEN || body[12..] != [0; 4] {
            return None;
        }
        Some(Refusal { code: ErrorCode::decode(u16_at(body, 0))?, detail: u16_at(body, 2), context: u64_at(body, 4) })
    }
}

const fn reserved_bits() -> Refusal {
    Refusal::new(ErrorCode::InvalidRequest, detail::invalid_request::RESERVED_BITS)
}

const fn bad_combination() -> Refusal {
    Refusal::new(ErrorCode::InvalidRequest, detail::invalid_request::BAD_COMBINATION)
}

fn u16_at(bytes: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([bytes[at], bytes[at + 1]])
}

fn u32_at(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}

fn u64_at(bytes: &[u8], at: usize) -> u64 {
    let mut value = [0u8; 8];
    value.copy_from_slice(&bytes[at..at + 8]);
    u64::from_le_bytes(value)
}

fn is_zero(bytes: &[u8]) -> bool {
    bytes.iter().all(|&byte| byte == 0)
}

/// Why a control record produced no message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlError {
    /// There is no `RequestId` to echo — the record is shorter than a header, or its `RequestId` is
    /// zero (§3.1). A receiver emits nothing and closes that record stream.
    Unanswerable,
    /// The request is refused, and this is the body of the error response it gets.
    Refused { request: RequestId, refusal: Refusal },
}

/// §3.1's header, as a decoded request carries it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    pub opcode: Opcode,
    pub request: RequestId,
}

/// One decoded request. There are seven and there is no generic forwarding path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Request {
    List(ListRequest),
    Status(StatusRequest),
    Get(GetRequest),
    Put(PutRequest),
    Remove(RemoveRequest),
    Cancel(CancelRequest),
    Arm(ArmRequest),
}

/// §3.3's cursor: the **pair**, plus the commit sequence the page was told.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListCursor {
    pub id: ObjectId,
    pub revision: Revision,
    pub sequence: u64,
}

/// §3.3.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListRequest {
    /// `None` lists every kind.
    pub kind: Option<ObjectKind>,
    /// `None` on a first page, which declares no expectation.
    pub cursor: Option<ListCursor>,
}

/// §3.4.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StatusRequest {
    pub id: ObjectId,
    pub revision: Revision,
}

/// §3.5. `Revision::HEAD` takes the current head.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GetRequest {
    pub id: ObjectId,
    pub revision: Revision,
}

/// §3.6.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PutRequest {
    /// `ObjectId::NONE` creates a new object.
    pub id: ObjectId,
    /// Zero when creating.
    pub expected: Revision,
    pub payload_len: u64,
    pub payload_crc: u32,
    pub kind: ObjectKind,
    /// Leave the displaced revision `RETAINED`.
    pub retain_previous: bool,
    pub name: DisplayName,
}

/// §3.7.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemoveRequest {
    pub id: ObjectId,
    pub expected: Revision,
}

/// §3.8.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CancelRequest {
    pub transfer: RequestId,
}

/// §4.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArmRequest {
    pub package: ObjectId,
    pub expected: Revision,
}

/// Decodes one whole control record: §3.1's header and the opcode's body.
///
/// Total. The two failures are §3.1's unanswerable record and a typed refusal to be sent back under
/// the request's own `RequestId`.
pub fn decode_request(record: &[u8]) -> Result<(Header, Request), ControlError> {
    if record.len() < HEADER_LEN {
        return Err(ControlError::Unanswerable);
    }
    let request = RequestId(u32_at(record, 12));
    if request.0 == 0 {
        return Err(ControlError::Unanswerable);
    }
    let refuse = |refusal| Err(ControlError::Refused { request, refusal });

    if record[0..4] != MAGIC {
        return refuse(Refusal::new(ErrorCode::InvalidFrame, detail::invalid_frame::MAGIC));
    }
    if record[4] != WIRE_MAJOR {
        return refuse(Refusal::new(ErrorCode::Unsupported, detail::unsupported::WIRE_MAJOR));
    }
    let Some(opcode) = Opcode::decode(record[5]) else {
        return refuse(Refusal::new(ErrorCode::Unsupported, detail::unsupported::OPCODE));
    };
    // "Requests carry no flags", and the reserved half-word is zero.
    if u16_at(record, 6) != 0 || u16_at(record, 10) != 0 {
        return refuse(reserved_bits());
    }
    let declared = u16_at(record, 8) as usize;
    if declared != opcode.request_body_len() {
        return refuse(Refusal::new(ErrorCode::InvalidFrame, detail::invalid_frame::LENGTH));
    }
    let carried = record.len() - HEADER_LEN;
    if carried < declared {
        return refuse(Refusal::new(ErrorCode::InvalidFrame, detail::invalid_frame::TRUNCATED));
    }
    if carried > declared {
        return refuse(Refusal::new(ErrorCode::InvalidFrame, detail::invalid_frame::TRAILING));
    }

    let body = &record[HEADER_LEN..];
    let decoded = match opcode {
        Opcode::List => decode_list(body).map(Request::List),
        Opcode::Status => decode_status(body).map(Request::Status),
        Opcode::Get => decode_get(body).map(Request::Get),
        Opcode::Put => decode_put(body).map(Request::Put),
        Opcode::Remove => decode_remove(body).map(Request::Remove),
        Opcode::Cancel => decode_cancel(body).map(Request::Cancel),
        Opcode::Arm => decode_arm(body).map(Request::Arm),
    };
    match decoded {
        Ok(message) => Ok((Header { opcode, request }, message)),
        Err(refusal) => refuse(refusal),
    }
}

fn decode_list(body: &[u8]) -> Result<ListRequest, Refusal> {
    let filter = u16_at(body, 0);
    let kind = match filter {
        0 => None,
        value => match ObjectKind::decode(value) {
            Some(kind) => Some(kind),
            // A filter naming a kind this major does not register is `unsupported`, exactly as an
            // unknown opcode is: the client asked for something the device has no table for.
            None => return Err(Refusal::new(ErrorCode::Unsupported, detail::unsupported::KIND)),
        },
    };
    let flags = u16_at(body, 2);
    if flags & !1 != 0 || !is_zero(&body[4..8]) {
        return Err(reserved_bits());
    }
    let cursor =
        ListCursor { id: ObjectId(u64_at(body, 8)), revision: Revision(u64_at(body, 16)), sequence: u64_at(body, 24) };
    if flags & 1 == 0 {
        // "zero unless the cursor bit is set" — three fields, one rule.
        if (cursor.id.0, cursor.revision.0, cursor.sequence) != (0, 0, 0) {
            return Err(bad_combination());
        }
        return Ok(ListRequest { kind, cursor: None });
    }
    Ok(ListRequest { kind, cursor: Some(cursor) })
}

fn decode_status(body: &[u8]) -> Result<StatusRequest, Refusal> {
    let id = ObjectId(u64_at(body, 0));
    // §3.4: "A STATUS naming ObjectId zero is invalidRequest; the identity of the store comes from
    // LIST."
    if !id.is_some() {
        return Err(bad_combination());
    }
    Ok(StatusRequest { id, revision: Revision(u64_at(body, 8)) })
}

fn decode_get(body: &[u8]) -> Result<GetRequest, Refusal> {
    Ok(GetRequest { id: ObjectId(u64_at(body, 0)), revision: Revision(u64_at(body, 8)) })
}

fn decode_put(body: &[u8]) -> Result<PutRequest, Refusal> {
    let id = ObjectId(u64_at(body, 0));
    let expected = Revision(u64_at(body, 8));
    // §3.6: "Zero is not a wildcard in either field."
    if id.is_some() != (expected.0 != 0) {
        return Err(bad_combination());
    }
    let Some(kind) = ObjectKind::decode(u16_at(body, 28)) else {
        return Err(Refusal::new(ErrorCode::Unsupported, detail::unsupported::KIND));
    };
    let flags = u16_at(body, 30);
    if flags & !1 != 0 || !is_zero(&body[33..36]) {
        return Err(reserved_bits());
    }
    let name = decode_name(body[32], &body[36..36 + NAME_CAPACITY])?;
    Ok(PutRequest {
        id,
        expected,
        payload_len: u64_at(body, 16),
        payload_crc: u32_at(body, 24),
        kind,
        retain_previous: flags & 1 != 0,
        name,
    })
}

/// §3.3 and §3.6 carry the same 49-byte name field: a length byte, then 48 bytes whose unused tail
/// is zero. The store keeps whatever bytes it is given, so the one rule this enforces beyond the
/// spec's table is that the name is the UTF-8 the field says it is — a menu has nothing else to do
/// with bytes that are not.
fn decode_name(len: u8, field: &[u8]) -> Result<DisplayName, Refusal> {
    let len = len as usize;
    if len > NAME_CAPACITY {
        return Err(bad_combination());
    }
    if !is_zero(&field[len..NAME_CAPACITY]) {
        return Err(reserved_bits());
    }
    if core::str::from_utf8(&field[..len]).is_err() {
        return Err(bad_combination());
    }
    DisplayName::from_bytes(&field[..len]).ok_or_else(bad_combination)
}

fn decode_remove(body: &[u8]) -> Result<RemoveRequest, Refusal> {
    Ok(RemoveRequest { id: ObjectId(u64_at(body, 0)), expected: Revision(u64_at(body, 8)) })
}

fn decode_cancel(body: &[u8]) -> Result<CancelRequest, Refusal> {
    Ok(CancelRequest { transfer: RequestId(u32_at(body, 0)) })
}

fn decode_arm(body: &[u8]) -> Result<ArmRequest, Refusal> {
    Ok(ArmRequest { package: ObjectId(u64_at(body, 0)), expected: Revision(u64_at(body, 8)) })
}

/// Writes §3.1's header into `out` and returns the whole record's length, or `None` when the
/// caller's buffer cannot hold the frame.
fn write_header(out: &mut [u8], opcode: Opcode, flags: u16, payload: usize, request: RequestId) -> Option<usize> {
    let total = HEADER_LEN + payload;
    if out.len() < total || payload > u16::MAX as usize {
        return None;
    }
    out[0..4].copy_from_slice(&MAGIC);
    out[4] = WIRE_MAJOR;
    out[5] = opcode.value();
    out[6..8].copy_from_slice(&flags.to_le_bytes());
    out[8..10].copy_from_slice(&(payload as u16).to_le_bytes());
    out[10..12].copy_from_slice(&0u16.to_le_bytes());
    out[12..16].copy_from_slice(&request.0.to_le_bytes());
    Some(total)
}

/// An error response: §3.1's header with `response|error`, and exactly one §3.9 body.
pub fn encode_error(out: &mut [u8], opcode: Opcode, request: RequestId, refusal: &Refusal) -> Option<usize> {
    let total = write_header(out, opcode, flags::RESPONSE | flags::ERROR, ERROR_BODY_LEN, request)?;
    out[HEADER_LEN..total].copy_from_slice(&refusal.encode());
    Some(total)
}

/// §3.3's page: the 24-byte prefix, then the entries the caller pushes.
///
/// The ceiling is remembered rather than taken from the caller's buffer, so a driver hands the same
/// buffer to every channel and the page still stops where §5.1's control ceiling does.
pub struct ListWriter {
    ceiling: usize,
    entries: usize,
    filled: usize,
}

impl ListWriter {
    /// Starts a page bounded by `ceiling` bytes of `out`. Fails when that cannot hold a header, the
    /// prefix and one entry — the §5.1 floor.
    pub fn start(out: &mut [u8], ceiling: usize, store: StoreId, sequence: u64) -> Option<Self> {
        let ceiling = ceiling.min(out.len());
        if ceiling < CONTROL_FLOOR {
            return None;
        }
        let body = &mut out[HEADER_LEN..];
        body[0..16].copy_from_slice(&store.0);
        body[16..24].copy_from_slice(&sequence.to_le_bytes());
        Some(ListWriter { ceiling, entries: 0, filled: LIST_PREFIX_LEN })
    }

    /// How many entries still fit under the ceiling.
    pub fn room(&self) -> usize {
        (self.ceiling - HEADER_LEN - self.filled) / LIST_ENTRY_LEN
    }

    /// Appends one entry, or reports that the page is full.
    pub fn push(&mut self, out: &mut [u8], meta: &EntryMeta) -> bool {
        if self.room() == 0 {
            return false;
        }
        let at = HEADER_LEN + self.filled;
        let entry = &mut out[at..at + LIST_ENTRY_LEN];
        entry.fill(0);
        entry[0..8].copy_from_slice(&meta.id.0.to_le_bytes());
        entry[8..16].copy_from_slice(&meta.revision.0.to_le_bytes());
        entry[16..24].copy_from_slice(&meta.payload_len.to_le_bytes());
        entry[24..28].copy_from_slice(&meta.payload_crc.to_le_bytes());
        entry[28..30].copy_from_slice(&meta.kind.value().to_le_bytes());
        entry[30..32].copy_from_slice(&meta.flags.bits().to_le_bytes());
        entry[32] = meta.name.len() as u8;
        entry[36..36 + NAME_CAPACITY].copy_from_slice(meta.name.padded());
        self.filled += LIST_ENTRY_LEN;
        self.entries += 1;
        true
    }

    /// Entries on the page.
    pub fn len(&self) -> usize {
        self.entries
    }

    /// True while the page carries the prefix and nothing else.
    pub fn is_empty(&self) -> bool {
        self.entries == 0
    }

    /// Seals the page. `more` sets §3.1's bit, which is valid on nothing else.
    pub fn finish(self, out: &mut [u8], request: RequestId, more: bool) -> Option<usize> {
        let flags = flags::RESPONSE | if more { flags::MORE } else { 0 };
        write_header(out, Opcode::List, flags, self.filled, request)
    }
}

/// §3.4's three states.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ObjectState {
    Absent = 0,
    Committed = 1,
    Superseded = 2,
}

/// §3.4's response.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StatusResponse {
    pub state: ObjectState,
    /// The current head, or zeros when absent.
    pub revision: Revision,
    pub payload_len: u64,
    pub payload_crc: u32,
}

impl StatusResponse {
    /// The answer for an `ObjectId` no entry names.
    pub fn absent() -> Self {
        StatusResponse { state: ObjectState::Absent, revision: Revision(0), payload_len: 0, payload_crc: 0 }
    }
}

/// Writes §3.4's 24-byte response.
pub fn encode_status(out: &mut [u8], request: RequestId, answer: &StatusResponse) -> Option<usize> {
    let total = write_header(out, Opcode::Status, flags::RESPONSE, 24, request)?;
    let body = &mut out[HEADER_LEN..total];
    body.fill(0);
    body[0] = answer.state as u8;
    body[4..12].copy_from_slice(&answer.revision.0.to_le_bytes());
    body[12..20].copy_from_slice(&answer.payload_len.to_le_bytes());
    body[20..24].copy_from_slice(&answer.payload_crc.to_le_bytes());
    Some(total)
}

/// Writes §3.5's 24-byte response, sent once the last payload byte is on the transport.
pub fn encode_get(out: &mut [u8], request: RequestId, served: Revision, payload_len: u64, crc: u32) -> Option<usize> {
    let total = write_header(out, Opcode::Get, flags::RESPONSE, 24, request)?;
    let body = &mut out[HEADER_LEN..total];
    body.fill(0);
    body[0..8].copy_from_slice(&served.0.to_le_bytes());
    body[8..16].copy_from_slice(&payload_len.to_le_bytes());
    body[16..20].copy_from_slice(&crc.to_le_bytes());
    Some(total)
}

/// Writes §3.6's 32-byte response.
pub fn encode_put(
    out: &mut [u8],
    request: RequestId,
    id: ObjectId,
    revision: Revision,
    payload_len: u64,
    crc: u32,
) -> Option<usize> {
    let total = write_header(out, Opcode::Put, flags::RESPONSE, 32, request)?;
    let body = &mut out[HEADER_LEN..total];
    body.fill(0);
    body[0..8].copy_from_slice(&id.0.to_le_bytes());
    body[8..16].copy_from_slice(&revision.0.to_le_bytes());
    body[16..24].copy_from_slice(&payload_len.to_le_bytes());
    body[24..28].copy_from_slice(&crc.to_le_bytes());
    Some(total)
}

/// Writes §3.7's 8-byte response: the new catalog commit sequence.
pub fn encode_remove(out: &mut [u8], request: RequestId, sequence: u64) -> Option<usize> {
    let total = write_header(out, Opcode::Remove, flags::RESPONSE, 8, request)?;
    out[HEADER_LEN..total].copy_from_slice(&sequence.to_le_bytes());
    Some(total)
}

/// Writes §3.8's 1-byte response: `0` cancelled, `1` no such transfer.
pub fn encode_cancel(out: &mut [u8], request: RequestId, cancelled: bool) -> Option<usize> {
    let total = write_header(out, Opcode::Cancel, flags::RESPONSE, 1, request)?;
    out[HEADER_LEN] = u8::from(!cancelled);
    Some(total)
}

/// Writes §4's 16-byte response: the rollback reserve's `ObjectId` and the new commit sequence.
pub fn encode_arm(out: &mut [u8], request: RequestId, reserve: ObjectId, sequence: u64) -> Option<usize> {
    let total = write_header(out, Opcode::Arm, flags::RESPONSE, 16, request)?;
    let body = &mut out[HEADER_LEN..total];
    body[0..8].copy_from_slice(&reserve.0.to_le_bytes());
    body[8..16].copy_from_slice(&sequence.to_le_bytes());
    Some(total)
}

/// §3.8's stream frame. A stream record is this immediately followed by exactly `len` payload bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamFrame {
    pub transfer: RequestId,
    pub offset: u64,
    pub len: u16,
}

impl StreamFrame {
    /// The 16 header bytes.
    pub fn encode(&self) -> [u8; STREAM_HEADER_LEN] {
        let mut frame = [0u8; STREAM_HEADER_LEN];
        frame[0..4].copy_from_slice(&self.transfer.0.to_le_bytes());
        frame[4..12].copy_from_slice(&self.offset.to_le_bytes());
        frame[12..14].copy_from_slice(&self.len.to_le_bytes());
        frame
    }

    /// Splits one stream record into its frame and its payload.
    ///
    /// `None` is §3.8's "a zero length, a length disagreeing with the record" and a nonzero reserved
    /// field: a record this cannot split names no offset, so the caller has nothing to answer with
    /// beyond terminating the transfer it claims to belong to.
    pub fn split(record: &[u8]) -> Option<(StreamFrame, &[u8])> {
        if record.len() < STREAM_HEADER_LEN || u16_at(record, 14) != 0 {
            return None;
        }
        let len = u16_at(record, 12);
        if len == 0 || record.len() != STREAM_HEADER_LEN + len as usize {
            return None;
        }
        let frame = StreamFrame { transfer: RequestId(u32_at(record, 0)), offset: u64_at(record, 4), len };
        Some((frame, &record[STREAM_HEADER_LEN..]))
    }
}

/// Writes a stream record's header for `len` payload bytes the caller has already placed at
/// `out[16..16 + len]`, and reports the record length.
pub fn write_stream(out: &mut [u8], transfer: RequestId, offset: u64, len: usize) -> Option<usize> {
    let total = STREAM_HEADER_LEN + len;
    if out.len() < total || len == 0 || len > u16::MAX as usize {
        return None;
    }
    out[..STREAM_HEADER_LEN].copy_from_slice(&StreamFrame { transfer, offset, len: len as u16 }.encode());
    Some(total)
}

#[cfg(test)]
mod tests {
    use super::super::ids::EntryFlags;
    use super::*;

    /// §3.10's `PUT` creating the route, byte for byte.
    const PUT_VECTOR: [u8; 100] = {
        let mut frame = [0u8; 100];
        frame[0] = 0x4F;
        frame[1] = 0x42;
        frame[2] = 0x43;
        frame[3] = 0x34;
        frame[4] = 0x04;
        frame[5] = 0x04;
        frame[8] = 0x54;
        frame[12] = 0x01;
        frame[13] = 0x2A;
        // Declared length 42,137 at body offset 16.
        frame[32] = 0x99;
        frame[33] = 0xA4;
        // CRC 0x9C4A7E21 at body offset 24.
        frame[40] = 0x21;
        frame[41] = 0x7E;
        frame[42] = 0x4A;
        frame[43] = 0x9C;
        // Kind 1 at body offset 28, name length 12 at body offset 32.
        frame[44] = 0x01;
        frame[48] = 0x0C;
        let name = *b"Grimsel Loop";
        let mut index = 0;
        while index < name.len() {
            frame[52 + index] = name[index];
            index += 1;
        }
        frame
    };

    #[test]
    fn the_specs_put_vector_decodes_to_its_fields() {
        let (header, request) = decode_request(&PUT_VECTOR).unwrap();
        assert_eq!(header, Header { opcode: Opcode::Put, request: RequestId(0x0000_2A01) });
        let Request::Put(put) = request else { panic!("not a PUT") };
        assert_eq!(put.id, ObjectId::NONE);
        assert_eq!(put.expected, Revision(0));
        assert_eq!(put.payload_len, 42_137);
        assert_eq!(put.payload_crc, 0x9C4A_7E21);
        assert_eq!(put.kind, ObjectKind::Route);
        assert!(!put.retain_previous);
        assert_eq!(put.name.as_bytes(), b"Grimsel Loop");
    }

    #[test]
    fn the_specs_error_vector_is_what_a_revision_conflict_encodes() {
        let mut out = [0u8; 32];
        let refusal = Refusal::with_context(ErrorCode::RevisionConflict, detail::revision_conflict::HEAD_DIFFERS, 5);
        let len = encode_error(&mut out, Opcode::Put, RequestId(0x0000_2A01), &refusal).unwrap();
        assert_eq!(len, 32);
        assert_eq!(
            out,
            [
                0x4F, 0x42, 0x43, 0x34, 0x04, 0x04, 0x03, 0x00, 0x10, 0x00, 0x00, 0x00, 0x01, 0x2A, 0x00, 0x00, 0x05,
                0x00, 0x01, 0x00, 0x05, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            ]
        );
        assert_eq!(Refusal::decode(&out[HEADER_LEN..]), Some(refusal));
    }

    #[test]
    fn the_specs_stream_frame_is_offset_40960_and_1024_bytes() {
        let mut record = [0u8; STREAM_HEADER_LEN + 1024];
        let len = write_stream(&mut record, RequestId(0x0000_2A01), 40_960, 1024).unwrap();
        assert_eq!(len, STREAM_HEADER_LEN + 1024);
        assert_eq!(
            record[..STREAM_HEADER_LEN],
            [0x01, 0x2A, 0x00, 0x00, 0x00, 0xA0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x04, 0x00, 0x00]
        );
        let (frame, payload) = StreamFrame::split(&record).unwrap();
        assert_eq!(frame, StreamFrame { transfer: RequestId(0x0000_2A01), offset: 40_960, len: 1024 });
        assert_eq!(payload.len(), 1024);
    }

    #[test]
    fn a_record_with_no_answerable_request_id_is_unanswerable() {
        assert_eq!(decode_request(&[]), Err(ControlError::Unanswerable));
        assert_eq!(decode_request(&[0; HEADER_LEN - 1]), Err(ControlError::Unanswerable));
        let mut zero = PUT_VECTOR;
        zero[12..16].copy_from_slice(&0u32.to_le_bytes());
        assert_eq!(decode_request(&zero), Err(ControlError::Unanswerable));
    }

    fn refusal_of(record: &[u8]) -> Refusal {
        match decode_request(record) {
            Err(ControlError::Refused { refusal, .. }) => refusal,
            other => panic!("expected a refusal, got {other:?}"),
        }
    }

    #[test]
    fn every_framing_rule_of_section_3_1_refuses_with_its_own_detail() {
        let mut wrong_magic = PUT_VECTOR;
        wrong_magic[3] = 0x35;
        assert_eq!(refusal_of(&wrong_magic), Refusal::new(ErrorCode::InvalidFrame, detail::invalid_frame::MAGIC));

        let mut wrong_major = PUT_VECTOR;
        wrong_major[4] = 3;
        assert_eq!(refusal_of(&wrong_major), Refusal::new(ErrorCode::Unsupported, detail::unsupported::WIRE_MAJOR));

        let mut unknown_opcode = PUT_VECTOR;
        unknown_opcode[5] = 0x09;
        assert_eq!(refusal_of(&unknown_opcode), Refusal::new(ErrorCode::Unsupported, detail::unsupported::OPCODE));

        let mut flagged = PUT_VECTOR;
        flagged[6] = 0x01;
        assert_eq!(refusal_of(&flagged), reserved_bits());

        let mut reserved = PUT_VECTOR;
        reserved[10] = 0x01;
        assert_eq!(refusal_of(&reserved), reserved_bits());

        let mut wrong_length = PUT_VECTOR;
        wrong_length[8] = 0x53;
        assert_eq!(refusal_of(&wrong_length), Refusal::new(ErrorCode::InvalidFrame, detail::invalid_frame::LENGTH));

        assert_eq!(
            refusal_of(&PUT_VECTOR[..99]),
            Refusal::new(ErrorCode::InvalidFrame, detail::invalid_frame::TRUNCATED)
        );

        let mut trailing = [0u8; 101];
        trailing[..100].copy_from_slice(&PUT_VECTOR);
        assert_eq!(refusal_of(&trailing), Refusal::new(ErrorCode::InvalidFrame, detail::invalid_frame::TRAILING));
    }

    #[test]
    fn a_put_body_refuses_every_field_rule_of_section_3_6() {
        let mut wildcard = PUT_VECTOR;
        wildcard[16..24].copy_from_slice(&7u64.to_le_bytes());
        assert_eq!(refusal_of(&wildcard), bad_combination(), "a create naming an ObjectId");

        let mut replace = PUT_VECTOR;
        replace[16..24].copy_from_slice(&7u64.to_le_bytes());
        replace[24..32].copy_from_slice(&2u64.to_le_bytes());
        assert!(decode_request(&replace).is_ok(), "a replace naming both");

        let mut no_revision = PUT_VECTOR;
        no_revision[24..32].copy_from_slice(&2u64.to_le_bytes());
        assert_eq!(refusal_of(&no_revision), bad_combination(), "a create with an expected revision");

        let mut unknown_kind = PUT_VECTOR;
        unknown_kind[44] = 0x09;
        assert_eq!(refusal_of(&unknown_kind), Refusal::new(ErrorCode::Unsupported, detail::unsupported::KIND));

        let mut flagged = PUT_VECTOR;
        flagged[46] = 0x02;
        assert_eq!(refusal_of(&flagged), reserved_bits(), "an undefined request flag");

        let mut retaining = PUT_VECTOR;
        retaining[46] = 0x01;
        let Ok((_, Request::Put(put))) = decode_request(&retaining) else { panic!("not a PUT") };
        assert!(put.retain_previous);

        let mut long_name = PUT_VECTOR;
        long_name[48] = 49;
        assert_eq!(refusal_of(&long_name), bad_combination());

        let mut dirty_pad = PUT_VECTOR;
        dirty_pad[99] = 1;
        assert_eq!(refusal_of(&dirty_pad), reserved_bits(), "a nonzero name pad");

        let mut dirty_reserved = PUT_VECTOR;
        dirty_reserved[49] = 1;
        assert_eq!(refusal_of(&dirty_reserved), reserved_bits(), "a nonzero reserved run");

        let mut not_utf8 = PUT_VECTOR;
        not_utf8[52] = 0xFF;
        assert_eq!(refusal_of(&not_utf8), bad_combination(), "a name that is not UTF-8");
    }

    fn request_frame(opcode: Opcode, body: &[u8]) -> [u8; 64] {
        let mut record = [0u8; 64];
        write_header(&mut record, opcode, 0, body.len(), RequestId(9)).unwrap();
        record[HEADER_LEN..HEADER_LEN + body.len()].copy_from_slice(body);
        record
    }

    #[test]
    fn a_list_cursor_is_all_three_fields_or_none_of_them() {
        let mut body = [0u8; LIST_BODY_LEN];
        let record = request_frame(Opcode::List, &body);
        let Ok((_, Request::List(list))) = decode_request(&record[..HEADER_LEN + LIST_BODY_LEN]) else {
            panic!("not a LIST")
        };
        assert_eq!(list, ListRequest { kind: None, cursor: None });

        body[8..16].copy_from_slice(&4u64.to_le_bytes());
        let record = request_frame(Opcode::List, &body);
        assert_eq!(refusal_of(&record[..HEADER_LEN + LIST_BODY_LEN]), bad_combination());

        body[2] = 1;
        body[16..24].copy_from_slice(&2u64.to_le_bytes());
        body[24..32].copy_from_slice(&11u64.to_le_bytes());
        let record = request_frame(Opcode::List, &body);
        let Ok((_, Request::List(list))) = decode_request(&record[..HEADER_LEN + LIST_BODY_LEN]) else {
            panic!("not a LIST")
        };
        assert_eq!(list.cursor, Some(ListCursor { id: ObjectId(4), revision: Revision(2), sequence: 11 }));

        body[2] = 2;
        let record = request_frame(Opcode::List, &body);
        assert_eq!(refusal_of(&record[..HEADER_LEN + LIST_BODY_LEN]), reserved_bits());

        let mut filtered = [0u8; LIST_BODY_LEN];
        filtered[0] = 9;
        let record = request_frame(Opcode::List, &filtered);
        assert_eq!(
            refusal_of(&record[..HEADER_LEN + LIST_BODY_LEN]),
            Refusal::new(ErrorCode::Unsupported, detail::unsupported::KIND)
        );
    }

    #[test]
    fn a_status_naming_object_zero_is_refused() {
        let body = [0u8; STATUS_BODY_LEN];
        let record = request_frame(Opcode::Status, &body);
        assert_eq!(refusal_of(&record[..HEADER_LEN + STATUS_BODY_LEN]), bad_combination());
    }

    #[test]
    fn a_stream_record_must_agree_with_its_own_length() {
        let mut record = [0u8; STREAM_HEADER_LEN + 4];
        record[0..4].copy_from_slice(&7u32.to_le_bytes());
        record[12..14].copy_from_slice(&4u16.to_le_bytes());
        assert!(StreamFrame::split(&record).is_some());
        record[12..14].copy_from_slice(&5u16.to_le_bytes());
        assert!(StreamFrame::split(&record).is_none(), "a length above the record");
        record[12..14].copy_from_slice(&0u16.to_le_bytes());
        assert!(StreamFrame::split(&record).is_none(), "a zero length");
        record[12..14].copy_from_slice(&4u16.to_le_bytes());
        record[14] = 1;
        assert!(StreamFrame::split(&record).is_none(), "a nonzero reserved field");
        assert!(StreamFrame::split(&record[..8]).is_none(), "a record below the frame");
    }

    #[test]
    fn a_list_page_carries_the_prefix_and_as_many_entries_as_the_ceiling_allows() {
        let meta = EntryMeta {
            id: ObjectId(1),
            revision: Revision(3),
            kind: ObjectKind::Route,
            flags: EntryFlags::NONE,
            payload_len: 42_137,
            payload_crc: 0x9C4A_7E21,
            name: DisplayName::new("Grimsel Loop").unwrap(),
        };
        let mut out = [0u8; 244];
        let mut page = ListWriter::start(&mut out, 244, StoreId([0xAB; 16]), 7).unwrap();
        assert_eq!(page.room(), 2, "a 244-byte BLE ceiling carries two entries");
        assert!(page.push(&mut out, &meta));
        assert!(page.push(&mut out, &meta));
        assert!(!page.push(&mut out, &meta));
        assert_eq!(page.len(), 2);
        let len = page.finish(&mut out, RequestId(1), true).unwrap();
        assert_eq!(len, HEADER_LEN + LIST_PREFIX_LEN + 2 * LIST_ENTRY_LEN);
        assert_eq!(u16_at(&out, 6), flags::RESPONSE | flags::MORE);
        assert_eq!(u16_at(&out, 8) as usize, LIST_PREFIX_LEN + 2 * LIST_ENTRY_LEN);

        // A ceiling one byte under the §5.1 floor cannot carry a page at all, and neither can a
        // buffer that short.
        assert!(ListWriter::start(&mut out, CONTROL_FLOOR - 1, StoreId([0; 16]), 0).is_none());
        let mut small = [0u8; CONTROL_FLOOR - 1];
        assert!(ListWriter::start(&mut small, 244, StoreId([0; 16]), 0).is_none());
    }

    #[test]
    fn an_encoder_refuses_a_buffer_it_would_overrun() {
        let mut out = [0u8; HEADER_LEN + 8];
        assert!(encode_error(&mut out, Opcode::Get, RequestId(1), &Refusal::plain(ErrorCode::Internal)).is_none());
        assert!(encode_remove(&mut out, RequestId(1), 12).is_some());
        assert!(encode_status(&mut out, RequestId(1), &StatusResponse::absent()).is_none());
        assert!(write_stream(&mut out, RequestId(1), 0, 0).is_none(), "a zero-length stream record");
    }
}
