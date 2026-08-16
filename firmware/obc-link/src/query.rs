//! The four queries (`Device_Object_Protocol_v3.md` §8).
//!
//! ## Paging is done with requests
//!
//! §5.2: "a `more` flag means 'issue the next request', each page is its own request under its own
//! RequestId, and the snapshot token — capability revision, catalog cursor, or draft revision — is
//! what binds the pages together." The `more` flag lives in the control header, so the cross-check
//! that a next cursor is nonzero exactly when `more` is set belongs to [`crate::response`], which
//! can see both; this module owns the payloads.
//!
//! ## Cursors are opaque to application code and normative to a codec
//!
//! §8.2 gives the catalog cursor a byte layout — revision, next entry index, ObjectKind, and a
//! CRC-32 over the current StoreId followed by those first twelve bytes — and then says the bytes
//! "are opaque to application code despite their normative codec". [`CatalogCursor`] is therefore a
//! codec, not an accessor a client is meant to reason with; [`CatalogCursor::verify`] needs the
//! StoreId because the CRC binds the cursor to one store.

use crate::codec::{
    bytes16_at, i32_at, i64_at, put_bytes, put_i32, put_i64, put_u16, put_u32, put_u64, u16_at, u32_at, u64_at,
};
use crate::error::{reject_nonzero, DecodeError, ErrorBody, ERROR_BODY_PREFIX_LEN};
use crate::ids::{LogicalObjectId, OperationId, Revision, StoreId, WeatherRequestId};
use crate::metadata::{MetadataEnvelope, Schema, SchemaClass, MAX_CATALOG_ENVELOPE};
use crate::registry::{draft_part_kind, object_kind, DraftPartKind, ObjectKind, Phase, SubjectNamespace};
use crate::result::ResultEnvelope;
use crate::{BufferTooSmall, EncodeResult};

/// The QueryOperation request: exactly one OperationId.
pub const QUERY_OPERATION_LEN: usize = 16;

/// The QueryOperation response prefix: state byte plus three reserved bytes.
pub const OPERATION_STATE_PREFIX_LEN: usize = 4;

/// The InProgress progress body.
pub const PROGRESS_LEN: usize = 24;

/// The QueryCatalog request.
pub const QUERY_CATALOG_LEN: usize = 28;

/// A cursor, in either query.
pub const CURSOR_LEN: usize = 16;

/// The QueryCatalog response prefix.
pub const CATALOG_PAGE_PREFIX_LEN: usize = 44;

/// The fixed prefix of one catalog entry, before its metadata envelope.
pub const CATALOG_ENTRY_PREFIX_LEN: usize = 36;

/// The most whole entries one catalog page returns (§8.2). A device may return fewer.
pub const MAX_CATALOG_ENTRIES: usize = 10;

/// The QueryDraft request.
pub const QUERY_DRAFT_LEN: usize = 44;

/// The QueryDraft response prefix.
pub const DRAFT_PAGE_PREFIX_LEN: usize = 44;

/// One QueryDraft entry.
pub const DRAFT_ENTRY_LEN: usize = 68;

/// The most entries one draft page returns.
pub const MAX_DRAFT_ENTRIES: usize = 6;

/// The QueryWeatherRequest response.
pub const WEATHER_REQUEST_LEN: usize = 96;

/// The QueryOperation request (§8.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QueryOperation {
    /// The operation to ask about.
    pub operation_id: OperationId,
}

impl QueryOperation {
    /// Decodes exactly [`QUERY_OPERATION_LEN`] bytes.
    pub fn decode(payload: &[u8]) -> crate::Result<Self> {
        DecodeError::exact_len(payload, QUERY_OPERATION_LEN)?;
        Ok(QueryOperation { operation_id: OperationId::new(bytes16_at(payload, 0)) })
    }

    /// Encodes the request.
    pub fn encode(&self) -> [u8; QUERY_OPERATION_LEN] {
        self.operation_id.to_bytes()
    }
}

/// Progress-body flag bits (§8.1).
pub mod progress_flags {
    /// Bit 0 — the claim's policy permits a resume.
    pub const RESUMABLE: u8 = 1 << 0;
    /// Bit 1 — a session is currently attached. Advisory: "Attachment ... grants no ownership."
    pub const SESSION_ATTACHED: u8 = 1 << 1;
    /// Bit 2 — the LogicalObjectId field is meaningful.
    pub const LOGICAL_ID_PRESENT: u8 = 1 << 2;
    /// Every defined bit; bits `3..7` are zero.
    pub const ALL: u8 = RESUMABLE | SESSION_ATTACHED | LOGICAL_ID_PRESENT;
}

/// The 24-byte InProgress body (§8.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OperationProgress {
    /// Which registry the subject kind belongs to, or `None`.
    pub namespace: SubjectNamespace,
    /// The subject kind code. Zero exactly when the namespace is `None`.
    pub subject_kind: u16,
    /// The phase.
    pub phase: Phase,
    /// Resumable / attached / ID-present.
    pub flags: u8,
    /// The assigned logical identity. Zero when the ID-present bit is clear; with the bit set,
    /// "zero remains a valid opaque LogicalObjectId".
    pub logical_object_id: LogicalObjectId,
    /// The durable payload prefix, manifest prefix, or zero, depending on the originating claim.
    pub durable_offset: u64,
}

impl OperationProgress {
    /// True when the LogicalObjectId field is meaningful.
    pub const fn logical_id_present(&self) -> bool {
        self.flags & progress_flags::LOGICAL_ID_PRESENT != 0
    }

    /// The subject kind as a logical `ObjectKind`, when the namespace says so.
    pub fn logical_kind(&self) -> Option<ObjectKind> {
        match self.namespace {
            SubjectNamespace::Logical => ObjectKind::from_u16(self.subject_kind),
            _ => None,
        }
    }

    /// The subject kind as a `DraftPartKind`, when the namespace says so.
    pub fn draft_part_kind(&self) -> Option<DraftPartKind> {
        match self.namespace {
            SubjectNamespace::DraftPart => DraftPartKind::from_u16(self.subject_kind),
            _ => None,
        }
    }

    /// Decodes exactly [`PROGRESS_LEN`] bytes.
    pub fn decode(body: &[u8]) -> crate::Result<Self> {
        DecodeError::exact_len(body, PROGRESS_LEN)?;
        if body[3] != 0 {
            return Err(DecodeError::reserved_bits());
        }
        reject_nonzero(body, 6, 2)?;
        let flags = body[2];
        if flags & !progress_flags::ALL != 0 {
            return Err(DecodeError::unsupported_flags());
        }
        let namespace = SubjectNamespace::from_u8(body[0]).ok_or_else(DecodeError::unknown_enum)?;
        let subject_kind = u16_at(body, 4);
        match namespace {
            SubjectNamespace::None => {
                if subject_kind != 0 {
                    // §8.1: "a nonzero kind in namespace none ... MUST NOT be emitted".
                    return Err(DecodeError::invalid_combination());
                }
            }
            SubjectNamespace::Logical => {
                object_kind(subject_kind)?;
            }
            SubjectNamespace::DraftPart => {
                draft_part_kind(subject_kind)?;
            }
        }
        let logical_object_id = u64_at(body, 8);
        if flags & progress_flags::LOGICAL_ID_PRESENT == 0 && logical_object_id != 0 {
            // "An ID field with ID-present clear is zero."
            return Err(DecodeError::reserved_bits());
        }
        Ok(OperationProgress {
            namespace,
            subject_kind,
            phase: Phase::from_u8(body[1]).ok_or_else(DecodeError::unknown_enum)?,
            flags,
            logical_object_id: LogicalObjectId::new(logical_object_id),
            durable_offset: u64_at(body, 16),
        })
    }

    /// Encodes the body.
    pub fn encode(&self) -> [u8; PROGRESS_LEN] {
        let mut out = [0u8; PROGRESS_LEN];
        out[0] = self.namespace.to_u8();
        out[1] = self.phase.to_u8();
        out[2] = self.flags;
        put_u16(&mut out, 4, self.subject_kind);
        put_u64(&mut out, 8, self.logical_object_id.get());
        put_u64(&mut out, 16, self.durable_offset);
        out
    }
}

/// The QueryOperation response (§8.1).
///
/// `Aborted` carries a bare, text-free [`ErrorBody`] — §11: "QueryOperation is intentionally
/// different: its successful state `Aborted` is followed by the same bare ErrorBody so status can
/// be inspected without turning the query itself into a failed request."
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperationStatus<'a> {
    /// Neither active nor retained. It "cannot distinguish never claimed from evicted".
    Unknown,
    /// Claimed and live.
    InProgress(OperationProgress),
    /// Terminal success, with its typed result.
    Committed(ResultEnvelope),
    /// Terminal failure, with its retained text-free body.
    Aborted(ErrorBody<'a>),
}

impl<'a> OperationStatus<'a> {
    /// The state byte.
    pub const fn state(&self) -> u8 {
        match self {
            OperationStatus::Unknown => 0,
            OperationStatus::InProgress(_) => 1,
            OperationStatus::Committed(_) => 2,
            OperationStatus::Aborted(_) => 3,
        }
    }

    /// The name used in fixture JSON.
    pub const fn name(&self) -> &'static str {
        match self {
            OperationStatus::Unknown => "unknown",
            OperationStatus::InProgress(_) => "inProgress",
            OperationStatus::Committed(_) => "committed",
            OperationStatus::Aborted(_) => "aborted",
        }
    }

    /// The exact encoded payload length.
    pub fn encoded_len(&self) -> usize {
        OPERATION_STATE_PREFIX_LEN
            + match self {
                OperationStatus::Unknown => 0,
                OperationStatus::InProgress(_) => PROGRESS_LEN,
                OperationStatus::Committed(envelope) => envelope.encoded_len(),
                OperationStatus::Aborted(body) => body.encoded_len(),
            }
    }

    /// Decodes a QueryOperation response payload.
    pub fn decode(payload: &'a [u8]) -> crate::Result<Self> {
        DecodeError::min_len(payload, OPERATION_STATE_PREFIX_LEN)?;
        reject_nonzero(payload, 1, 3)?;
        let body = &payload[OPERATION_STATE_PREFIX_LEN..];
        match payload[0] {
            0 => {
                if !body.is_empty() {
                    return Err(DecodeError::trailing_bytes());
                }
                Ok(OperationStatus::Unknown)
            }
            1 => Ok(OperationStatus::InProgress(OperationProgress::decode(body)?)),
            2 => Ok(OperationStatus::Committed(ResultEnvelope::decode(body)?)),
            3 => {
                let error = ErrorBody::decode(body)?;
                if !error.text.is_empty() {
                    // §8.1: "Aborted ... ErrorBody without diagnostic text".
                    return Err(DecodeError::invalid_combination());
                }
                Ok(OperationStatus::Aborted(error))
            }
            _ => Err(DecodeError::unknown_enum()),
        }
    }

    /// Encodes the payload into `out`, returning its exact length.
    pub fn encode_into(&self, out: &mut [u8]) -> EncodeResult {
        let needed = self.encoded_len();
        if out.len() < needed {
            return Err(BufferTooSmall { needed, available: out.len() });
        }
        out[0] = self.state();
        out[1..OPERATION_STATE_PREFIX_LEN].fill(0);
        let body = &mut out[OPERATION_STATE_PREFIX_LEN..needed];
        match self {
            OperationStatus::Unknown => {}
            OperationStatus::InProgress(progress) => body.copy_from_slice(&progress.encode()),
            OperationStatus::Committed(envelope) => {
                envelope.encode_into(body)?;
            }
            OperationStatus::Aborted(error) => {
                error.encode_into(body)?;
            }
        }
        Ok(needed)
    }
}

/// A catalog or draft cursor: sixteen bytes with a normative codec and no application meaning.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CatalogCursor {
    /// The snapshot revision this cursor belongs to.
    pub revision: u64,
    /// The next entry index within that snapshot.
    pub next_entry_index: u16,
    /// The ObjectKind for a catalog cursor; zero for a draft cursor.
    pub kind_code: u16,
    /// CRC-32/IEEE over the binding prefix and the first twelve cursor bytes.
    pub crc32: u32,
}

impl CatalogCursor {
    /// The all-zero cursor, which is what a page carries when `more` is clear.
    pub const ZERO: CatalogCursor = CatalogCursor { revision: 0, next_entry_index: 0, kind_code: 0, crc32: 0 };

    /// Decodes sixteen bytes.
    pub fn decode(bytes: &[u8]) -> crate::Result<Self> {
        DecodeError::exact_len(bytes, CURSOR_LEN)?;
        Ok(CatalogCursor {
            revision: u64_at(bytes, 0),
            next_entry_index: u16_at(bytes, 8),
            kind_code: u16_at(bytes, 10),
            crc32: u32_at(bytes, 12),
        })
    }

    /// Encodes sixteen bytes.
    pub fn encode(&self) -> [u8; CURSOR_LEN] {
        let mut out = [0u8; CURSOR_LEN];
        put_u64(&mut out, 0, self.revision);
        put_u16(&mut out, 8, self.next_entry_index);
        put_u16(&mut out, 10, self.kind_code);
        put_u32(&mut out, 12, self.crc32);
        out
    }

    /// True when every byte is zero.
    pub fn is_zero(&self) -> bool {
        *self == CatalogCursor::ZERO
    }

    /// The catalog cursor's CRC: over the current StoreId followed by the first twelve cursor bytes
    /// (§8.2).
    pub fn catalog_crc(&self, store_id: StoreId) -> u32 {
        let mut hasher = obc_crc::Crc32::new();
        hasher.update(store_id.as_bytes());
        hasher.update(&self.encode()[..12]);
        hasher.finalize()
    }

    /// The draft cursor's CRC: over the current StoreId, the parent OperationId, then those same
    /// twelve bytes (§8.3). "This binds a cursor to one store and parent."
    pub fn draft_crc(&self, store_id: StoreId, parent: OperationId) -> u32 {
        let mut hasher = obc_crc::Crc32::new();
        hasher.update(store_id.as_bytes());
        hasher.update(parent.as_bytes());
        hasher.update(&self.encode()[..12]);
        hasher.finalize()
    }

    /// Verifies a catalog cursor against the store that issued it.
    pub fn verify_catalog(&self, store_id: StoreId) -> crate::Result<()> {
        if self.crc32 == self.catalog_crc(store_id) {
            Ok(())
        } else {
            Err(DecodeError::new(crate::ErrorCategory::CHECKSUM_FAILURE, crate::error::detail::checksum::CURSOR))
        }
    }

    /// Verifies a draft cursor against the store and parent that issued it.
    pub fn verify_draft(&self, store_id: StoreId, parent: OperationId) -> crate::Result<()> {
        if self.crc32 == self.draft_crc(store_id, parent) {
            Ok(())
        } else {
            Err(DecodeError::new(crate::ErrorCategory::CHECKSUM_FAILURE, crate::error::detail::checksum::CURSOR))
        }
    }
}

/// Query flag bits shared by QueryCatalog and QueryDraft.
pub mod query_flags {
    /// Bit 0 — the expected-revision field is meaningful.
    pub const EXPECTED_REVISION: u16 = 1 << 0;
    /// Bit 1 — the cursor field is meaningful.
    pub const CURSOR: u16 = 1 << 1;
    /// Every defined bit.
    pub const ALL: u16 = EXPECTED_REVISION | CURSOR;
}

/// What a paged query asks for. The three legal flag combinations, and only those (§8.2, §8.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageRequest {
    /// Neither flag: the current first page.
    CurrentFirstPage,
    /// Expected-revision alone: an incremental unchanged check, which returns an empty page on an
    /// exact match and `catalogChanged` on a mismatch.
    UnchangedCheck(u64),
    /// Both flags: continue a snapshot. The expected revision must equal the cursor's.
    Continue {
        /// The snapshot revision, equal to the cursor's own.
        expected_revision: u64,
        /// The cursor the previous page returned.
        cursor: CatalogCursor,
    },
}

impl PageRequest {
    /// The wire flags word.
    pub const fn flags(&self) -> u16 {
        match self {
            PageRequest::CurrentFirstPage => 0,
            PageRequest::UnchangedCheck(_) => query_flags::EXPECTED_REVISION,
            PageRequest::Continue { .. } => query_flags::ALL,
        }
    }

    /// The wire expected-revision field.
    pub const fn expected_revision(&self) -> u64 {
        match self {
            PageRequest::CurrentFirstPage => 0,
            PageRequest::UnchangedCheck(revision) => *revision,
            PageRequest::Continue { expected_revision, .. } => *expected_revision,
        }
    }

    /// The wire cursor field.
    pub const fn cursor(&self) -> CatalogCursor {
        match self {
            PageRequest::Continue { cursor, .. } => *cursor,
            _ => CatalogCursor::ZERO,
        }
    }

    /// The name used in fixture JSON.
    pub const fn name(&self) -> &'static str {
        match self {
            PageRequest::CurrentFirstPage => "currentFirstPage",
            PageRequest::UnchangedCheck(_) => "unchangedCheck",
            PageRequest::Continue { .. } => "continue",
        }
    }

    fn decode(flags: u16, expected_revision: u64, cursor_bytes: &[u8]) -> crate::Result<Self> {
        if flags & !query_flags::ALL != 0 {
            return Err(DecodeError::unsupported_flags());
        }
        let cursor_is_zero = cursor_bytes.iter().all(|&b| b == 0);
        match flags {
            0 => {
                if expected_revision != 0 || !cursor_is_zero {
                    // "With neither flag, both fields are zero".
                    return Err(DecodeError::reserved_bits());
                }
                Ok(PageRequest::CurrentFirstPage)
            }
            query_flags::EXPECTED_REVISION => {
                if !cursor_is_zero {
                    return Err(DecodeError::reserved_bits());
                }
                Ok(PageRequest::UnchangedCheck(expected_revision))
            }
            query_flags::CURSOR => {
                // "Cursor requires both bits"; the cursor bit alone is not a combination.
                Err(DecodeError::invalid_combination())
            }
            _ => {
                let cursor = CatalogCursor::decode(cursor_bytes)?;
                if cursor.revision != expected_revision {
                    // "...and an expected revision equal to the cursor revision."
                    return Err(DecodeError::invalid_combination());
                }
                Ok(PageRequest::Continue { expected_revision, cursor })
            }
        }
    }
}

/// The QueryCatalog request (§8.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QueryCatalog {
    /// Which kind's heads to list.
    pub kind: ObjectKind,
    /// Which page.
    pub page: PageRequest,
}

impl QueryCatalog {
    /// Decodes exactly [`QUERY_CATALOG_LEN`] bytes.
    pub fn decode(payload: &[u8]) -> crate::Result<Self> {
        DecodeError::exact_len(payload, QUERY_CATALOG_LEN)?;
        let page = PageRequest::decode(u16_at(payload, 2), u64_at(payload, 4), &payload[12..28])?;
        let kind = object_kind(u16_at(payload, 0))?;
        if let PageRequest::Continue { cursor, .. } = page {
            if cursor.kind_code != kind.to_u16() {
                return Err(DecodeError::invalid_combination());
            }
        }
        Ok(QueryCatalog { kind, page })
    }

    /// Encodes the request.
    pub fn encode(&self) -> [u8; QUERY_CATALOG_LEN] {
        let mut out = [0u8; QUERY_CATALOG_LEN];
        put_u16(&mut out, 0, self.kind.to_u16());
        put_u16(&mut out, 2, self.page.flags());
        put_u64(&mut out, 4, self.page.expected_revision());
        put_bytes(&mut out, 12, &self.page.cursor().encode());
        out
    }
}

/// One catalog entry: a 36-byte prefix and its projection envelope.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CatalogEntry<'a> {
    /// The logical identity.
    pub logical_object_id: LogicalObjectId,
    /// The entry Revision — the compare-and-swap token for a later mutation of this entry.
    pub revision: Revision,
    /// The head's length.
    pub length: u64,
    /// The head's CRC-32/IEEE.
    pub crc32: u32,
    /// The catalog projection envelope.
    pub metadata: MetadataEnvelope<'a>,
}

impl<'a> CatalogEntry<'a> {
    /// The exact encoded length.
    pub fn encoded_len(&self) -> usize {
        CATALOG_ENTRY_PREFIX_LEN + self.metadata.encoded_len()
    }

    /// Decodes one entry at the start of `bytes`, returning it and the bytes it consumed.
    pub fn decode_prefix(bytes: &'a [u8], kind: ObjectKind) -> crate::Result<(Self, usize)> {
        DecodeError::min_len(bytes, CATALOG_ENTRY_PREFIX_LEN)?;
        if u16_at(bytes, 28) != 0 {
            // §8.2: "Entry flags are zero in v3.0 and nonzero values are rejected."
            return Err(DecodeError::reserved_bits());
        }
        reject_nonzero(bytes, 32, 4)?;
        let metadata_len = usize::from(u16_at(bytes, 30));
        if metadata_len > MAX_CATALOG_ENVELOPE {
            return Err(DecodeError::invalid_descriptor(crate::error::detail::descriptor::NESTED_LENGTH));
        }
        let total = CATALOG_ENTRY_PREFIX_LEN + metadata_len;
        if bytes.len() < total {
            return Err(DecodeError::truncated());
        }
        let metadata = MetadataEnvelope::decode(&bytes[CATALOG_ENTRY_PREFIX_LEN..total], MAX_CATALOG_ENVELOPE)?;
        if metadata.encoded_len() != metadata_len {
            return Err(DecodeError::invalid_descriptor(crate::error::detail::descriptor::NESTED_LENGTH));
        }
        // The projection belongs to the page's kind, so the schema is knowable here and is checked
        // here. §2.2's response rule applies: unknown *critical* fields are rejected and a
        // well-formed unknown noncritical one may be skipped.
        Schema::lookup(kind, SchemaClass::Catalog)
            .ok_or_else(|| DecodeError::unsupported_capability(crate::error::detail::capability::LOGICAL_KIND))?
            .validate(&metadata)?;
        Ok((
            CatalogEntry {
                logical_object_id: LogicalObjectId::new(u64_at(bytes, 0)),
                revision: Revision::new(u64_at(bytes, 8)),
                length: u64_at(bytes, 16),
                crc32: u32_at(bytes, 24),
                metadata,
            },
            total,
        ))
    }

    /// Encodes the entry into `out`, returning its exact length.
    pub fn encode_into(&self, out: &mut [u8]) -> EncodeResult {
        let needed = self.encoded_len();
        if out.len() < needed {
            return Err(BufferTooSmall { needed, available: out.len() });
        }
        let out = &mut out[..needed];
        out.fill(0);
        put_u64(out, 0, self.logical_object_id.get());
        put_u64(out, 8, self.revision.get());
        put_u64(out, 16, self.length);
        put_u32(out, 24, self.crc32);
        put_u16(out, 30, self.metadata.encoded_len() as u16);
        self.metadata.encode_into(&mut out[CATALOG_ENTRY_PREFIX_LEN..])?;
        Ok(needed)
    }
}

/// A decoded QueryCatalog page. Its entries stay borrowed as bytes, so re-encoding is byte-exact.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CatalogPage<'a> {
    /// The store.
    pub store_id: StoreId,
    /// The kind.
    pub kind: ObjectKind,
    /// The repository revision this snapshot belongs to.
    pub revision: Revision,
    /// The cursor for the next page, zero unless the frame's `more` flag is set.
    pub next_cursor: CatalogCursor,
    /// How many entries follow.
    pub entry_count: u16,
    /// Exactly those entries' bytes.
    pub entry_bytes: &'a [u8],
}

impl<'a> CatalogPage<'a> {
    /// The exact encoded payload length.
    pub fn encoded_len(&self) -> usize {
        CATALOG_PAGE_PREFIX_LEN + self.entry_bytes.len()
    }

    /// Iterates the entries.
    pub fn entries(&self) -> CatalogEntryIter<'a> {
        CatalogEntryIter { bytes: self.entry_bytes, offset: 0, kind: self.kind }
    }

    /// Decodes a page payload.
    pub fn decode(payload: &'a [u8]) -> crate::Result<Self> {
        DecodeError::min_len(payload, CATALOG_PAGE_PREFIX_LEN)?;
        let entry_count = u16_at(payload, 18);
        if usize::from(entry_count) > MAX_CATALOG_ENTRIES {
            return Err(DecodeError::invalid_combination());
        }
        let page = CatalogPage {
            store_id: StoreId::new(bytes16_at(payload, 0)),
            kind: object_kind(u16_at(payload, 16))?,
            revision: Revision::new(u64_at(payload, 20)),
            next_cursor: CatalogCursor::decode(&payload[28..44])?,
            entry_count,
            entry_bytes: &payload[CATALOG_PAGE_PREFIX_LEN..],
        };
        let mut seen = 0u16;
        let mut offset = 0usize;
        let mut previous: Option<LogicalObjectId> = None;
        while offset < page.entry_bytes.len() {
            let (entry, used) = CatalogEntry::decode_prefix(&page.entry_bytes[offset..], page.kind)?;
            if let Some(previous) = previous {
                if entry.logical_object_id <= previous {
                    // "Entries are ordered by LogicalObjectId" — and a head appears once.
                    return Err(DecodeError::invalid_combination());
                }
            }
            previous = Some(entry.logical_object_id);
            offset += used;
            seen += 1;
            if seen > MAX_CATALOG_ENTRIES as u16 {
                return Err(DecodeError::invalid_combination());
            }
        }
        if seen != entry_count {
            return Err(DecodeError::invalid_descriptor(crate::error::detail::descriptor::NESTED_LENGTH));
        }
        Ok(page)
    }

    /// Encodes the page into `out`, returning its exact length.
    pub fn encode_into(&self, out: &mut [u8]) -> EncodeResult {
        let needed = self.encoded_len();
        if out.len() < needed {
            return Err(BufferTooSmall { needed, available: out.len() });
        }
        let out = &mut out[..needed];
        out.fill(0);
        put_bytes(out, 0, self.store_id.as_bytes());
        put_u16(out, 16, self.kind.to_u16());
        put_u16(out, 18, self.entry_count);
        put_u64(out, 20, self.revision.get());
        put_bytes(out, 28, &self.next_cursor.encode());
        put_bytes(out, CATALOG_PAGE_PREFIX_LEN, self.entry_bytes);
        Ok(needed)
    }
}

/// Iterator over a validated page's entries.
#[derive(Debug, Clone, Copy)]
pub struct CatalogEntryIter<'a> {
    bytes: &'a [u8],
    offset: usize,
    kind: ObjectKind,
}

impl<'a> Iterator for CatalogEntryIter<'a> {
    type Item = CatalogEntry<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.offset >= self.bytes.len() {
            return None;
        }
        let (entry, used) = CatalogEntry::decode_prefix(&self.bytes[self.offset..], self.kind).ok()?;
        self.offset += used;
        Some(entry)
    }
}

/// The QueryDraft request (§8.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QueryDraft {
    /// The parent whose children to page.
    pub parent_operation_id: OperationId,
    /// How many entries the client wants, `1` through `6`.
    pub requested_limit: u8,
    /// Which page.
    pub page: PageRequest,
}

impl QueryDraft {
    /// Decodes exactly [`QUERY_DRAFT_LEN`] bytes.
    pub fn decode(payload: &[u8]) -> crate::Result<Self> {
        DecodeError::exact_len(payload, QUERY_DRAFT_LEN)?;
        if payload[19] != 0 {
            return Err(DecodeError::reserved_bits());
        }
        let requested_limit = payload[18];
        if requested_limit == 0 || usize::from(requested_limit) > MAX_DRAFT_ENTRIES {
            return Err(DecodeError::invalid_combination());
        }
        let page = PageRequest::decode(u16_at(payload, 16), u64_at(payload, 20), &payload[28..44])?;
        if let PageRequest::Continue { cursor, .. } = page {
            if cursor.kind_code != 0 {
                // §8.3: the draft cursor's third field is "zero `u16`".
                return Err(DecodeError::reserved_bits());
            }
        }
        Ok(QueryDraft { parent_operation_id: OperationId::new(bytes16_at(payload, 0)), requested_limit, page })
    }

    /// Encodes the request.
    pub fn encode(&self) -> [u8; QUERY_DRAFT_LEN] {
        let mut out = [0u8; QUERY_DRAFT_LEN];
        put_bytes(&mut out, 0, self.parent_operation_id.as_bytes());
        put_u16(&mut out, 16, self.page.flags());
        out[18] = self.requested_limit;
        put_u64(&mut out, 20, self.page.expected_revision());
        put_bytes(&mut out, 28, &self.page.cursor().encode());
        out
    }
}

/// A draft part's state, as QueryDraft projects it (§8.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum DraftPartState {
    /// Claimed, no accepted byte yet.
    Prepared = 0,
    /// Accepting bytes.
    Streaming = 1,
    /// Sealed; this is the only state that carries a `DraftPartRef`.
    Sealed = 2,
    /// Durably aborted.
    Aborted = 3,
}

impl DraftPartState {
    /// Every state, in wire order.
    pub const ALL: [DraftPartState; 4] =
        [DraftPartState::Prepared, DraftPartState::Streaming, DraftPartState::Sealed, DraftPartState::Aborted];

    /// Decodes a wire `u8`.
    pub const fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(DraftPartState::Prepared),
            1 => Some(DraftPartState::Streaming),
            2 => Some(DraftPartState::Sealed),
            3 => Some(DraftPartState::Aborted),
            _ => None,
        }
    }

    /// The wire `u8`.
    pub const fn to_u8(self) -> u8 {
        self as u8
    }

    /// The name used in fixture JSON.
    pub const fn name(self) -> &'static str {
        match self {
            DraftPartState::Prepared => "prepared",
            DraftPartState::Streaming => "streaming",
            DraftPartState::Sealed => "sealed",
            DraftPartState::Aborted => "aborted",
        }
    }
}

/// Draft page flag bits (§8.3).
pub mod draft_page_flags {
    /// Bit 0 — the parent's manifest is streaming.
    pub const MANIFEST_STREAMING: u8 = 1 << 0;
    /// Bit 1 — the parent is aborting.
    pub const ABORTING: u8 = 1 << 1;
    /// Every defined bit.
    pub const ALL: u8 = MANIFEST_STREAMING | ABORTING;
}

/// One 68-byte QueryDraft entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DraftEntry {
    /// The child operation.
    pub child_operation_id: OperationId,
    /// The opaque reference, zero unless the state is sealed.
    pub draft_part_ref: crate::ids::DraftPartRef,
    /// The part kind.
    pub part_kind: DraftPartKind,
    /// The part key.
    pub part_key: u64,
    /// The part state.
    pub state: DraftPartState,
    /// The durable payload prefix.
    pub durable_offset: u64,
    /// The declared length.
    pub declared_length: u64,
    /// The declared CRC-32/IEEE.
    pub crc32: u32,
}

impl DraftEntry {
    /// The `(kind, key)` pair entries are strictly ordered by.
    pub fn sort_key(&self) -> (u16, u64) {
        (self.part_kind.to_u16(), self.part_key)
    }

    /// Decodes exactly [`DRAFT_ENTRY_LEN`] bytes.
    pub fn decode(bytes: &[u8]) -> crate::Result<Self> {
        DecodeError::exact_len(bytes, DRAFT_ENTRY_LEN)?;
        reject_nonzero(bytes, 34, 2)?;
        reject_nonzero(bytes, 46, 2)?;
        if bytes[45] != 0 {
            // §8.3 gives this entry a `flags u8` and defines no bit for it in v3.0, so §1's rule
            // for an inactive field applies: encoded zero, rejected when nonzero.
            return Err(DecodeError::reserved_bits());
        }
        let state = DraftPartState::from_u8(bytes[44]).ok_or_else(DecodeError::unknown_enum)?;
        let draft_part_ref = crate::ids::DraftPartRef::new(bytes16_at(bytes, 16));
        if state != DraftPartState::Sealed && !draft_part_ref.is_zero() {
            // "DraftPartRef is zero unless state sealed `2`".
            return Err(DecodeError::reserved_bits());
        }
        Ok(DraftEntry {
            child_operation_id: OperationId::new(bytes16_at(bytes, 0)),
            draft_part_ref,
            part_kind: draft_part_kind(u16_at(bytes, 32))?,
            part_key: u64_at(bytes, 36),
            state,
            durable_offset: u64_at(bytes, 48),
            declared_length: u64_at(bytes, 56),
            crc32: u32_at(bytes, 64),
        })
    }

    /// Encodes the entry.
    pub fn encode(&self) -> [u8; DRAFT_ENTRY_LEN] {
        let mut out = [0u8; DRAFT_ENTRY_LEN];
        put_bytes(&mut out, 0, self.child_operation_id.as_bytes());
        put_bytes(&mut out, 16, self.draft_part_ref.as_bytes());
        put_u16(&mut out, 32, self.part_kind.to_u16());
        put_u64(&mut out, 36, self.part_key);
        out[44] = self.state.to_u8();
        put_u64(&mut out, 48, self.durable_offset);
        put_u64(&mut out, 56, self.declared_length);
        put_u32(&mut out, 64, self.crc32);
        out
    }
}

/// A QueryDraft page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DraftPage {
    /// The parent.
    pub parent_operation_id: OperationId,
    /// The snapshot revision.
    pub draft_revision: u64,
    /// The cursor for the next page, zero unless `more` is set.
    pub next_cursor: CatalogCursor,
    /// Manifest-streaming / aborting.
    pub flags: u8,
    /// How many entries are meaningful.
    pub count: u8,
    /// The entries, in strictly ascending `(DraftPartKind, part_key)` order.
    pub entries: [DraftEntry; MAX_DRAFT_ENTRIES],
}

impl DraftPage {
    /// The meaningful entries.
    pub fn entries(&self) -> &[DraftEntry] {
        &self.entries[..usize::from(self.count)]
    }

    /// The exact encoded payload length.
    pub fn encoded_len(&self) -> usize {
        DRAFT_PAGE_PREFIX_LEN + usize::from(self.count) * DRAFT_ENTRY_LEN
    }

    /// Decodes a page payload.
    pub fn decode(payload: &[u8]) -> crate::Result<Self> {
        DecodeError::min_len(payload, DRAFT_PAGE_PREFIX_LEN)?;
        let count = payload[40];
        if usize::from(count) > MAX_DRAFT_ENTRIES {
            return Err(DecodeError::invalid_combination());
        }
        let flags = payload[41];
        if flags & !draft_page_flags::ALL != 0 {
            return Err(DecodeError::unsupported_flags());
        }
        reject_nonzero(payload, 42, 2)?;
        let body = &payload[DRAFT_PAGE_PREFIX_LEN..];
        let needed = usize::from(count) * DRAFT_ENTRY_LEN;
        if body.len() < needed {
            return Err(DecodeError::truncated());
        }
        if body.len() > needed {
            return Err(DecodeError::trailing_bytes());
        }
        let mut entries = [DraftEntry {
            child_operation_id: OperationId::ZERO,
            draft_part_ref: crate::ids::DraftPartRef::ZERO,
            part_kind: DraftPartKind::StandaloneMapBlob,
            part_key: 0,
            state: DraftPartState::Prepared,
            durable_offset: 0,
            declared_length: 0,
            crc32: 0,
        }; MAX_DRAFT_ENTRIES];
        let mut previous: Option<(u16, u64)> = None;
        for (index, slot) in entries.iter_mut().enumerate().take(usize::from(count)) {
            let start = index * DRAFT_ENTRY_LEN;
            let entry = DraftEntry::decode(&body[start..start + DRAFT_ENTRY_LEN])?;
            if let Some(previous) = previous {
                if entry.sort_key() <= previous {
                    return Err(DecodeError::invalid_combination());
                }
            }
            previous = Some(entry.sort_key());
            *slot = entry;
        }
        Ok(DraftPage {
            parent_operation_id: OperationId::new(bytes16_at(payload, 0)),
            draft_revision: u64_at(payload, 16),
            next_cursor: CatalogCursor::decode(&payload[24..40])?,
            flags,
            count,
            entries,
        })
    }

    /// Encodes the page into `out`, returning its exact length.
    pub fn encode_into(&self, out: &mut [u8]) -> EncodeResult {
        let needed = self.encoded_len();
        if out.len() < needed {
            return Err(BufferTooSmall { needed, available: out.len() });
        }
        let out = &mut out[..needed];
        out.fill(0);
        put_bytes(out, 0, self.parent_operation_id.as_bytes());
        put_u64(out, 16, self.draft_revision);
        put_bytes(out, 24, &self.next_cursor.encode());
        out[40] = self.count;
        out[41] = self.flags;
        for (index, entry) in self.entries().iter().enumerate() {
            put_bytes(out, DRAFT_PAGE_PREFIX_LEN + index * DRAFT_ENTRY_LEN, &entry.encode());
        }
        Ok(needed)
    }
}

/// The durable weather request context's state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum WeatherContextState {
    /// No bundle answers the current request yet.
    Pending = 1,
    /// A bundle has published against it.
    Satisfied = 2,
}

impl WeatherContextState {
    /// Decodes a wire `u8`. Zero and other values are invalid.
    pub const fn from_u8(value: u8) -> Option<Self> {
        match value {
            1 => Some(WeatherContextState::Pending),
            2 => Some(WeatherContextState::Satisfied),
            _ => None,
        }
    }

    /// The wire `u8`.
    pub const fn to_u8(self) -> u8 {
        self as u8
    }

    /// The name used in fixture JSON.
    pub const fn name(self) -> &'static str {
        match self {
            WeatherContextState::Pending => "pending",
            WeatherContextState::Satisfied => "satisfied",
        }
    }
}

/// The QueryWeatherRequest response (§8.4). Its request payload is empty.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WeatherRequestContext {
    /// The store.
    pub store_id: StoreId,
    /// The current request identity.
    pub current_request_id: WeatherRequestId,
    /// The request-context revision, which moves independently of the object revision.
    pub context_revision: u64,
    /// The reserved weather singleton identity — "an ordinary `u64` value, not a sentinel".
    pub weather_logical_object_id: LogicalObjectId,
    /// The weather repository revision — the CAS token to use for a reply.
    pub repository_revision: Revision,
    /// The request the published head answered, when one exists.
    pub head_request_id: Option<WeatherRequestId>,
    /// Required centre latitude, signed degrees times 10,000,000.
    pub centre_latitude_e7: i32,
    /// Required centre longitude, signed degrees times 10,000,000.
    pub centre_longitude_e7: i32,
    /// Required radius in metres.
    pub radius_metres: u32,
    /// Earliest acceptable issued time, signed Unix seconds.
    pub earliest_issued_utc: i64,
    /// Required valid-until time, signed Unix seconds.
    pub required_valid_until_utc: i64,
    /// Pending or satisfied.
    pub state: WeatherContextState,
}

/// Head-present flag bit (§8.4).
pub const WEATHER_HEAD_PRESENT: u32 = 1 << 0;

impl WeatherRequestContext {
    /// Decodes exactly [`WEATHER_REQUEST_LEN`] bytes.
    pub fn decode(payload: &[u8]) -> crate::Result<Self> {
        DecodeError::exact_len(payload, WEATHER_REQUEST_LEN)?;
        reject_nonzero(payload, 89, 7)?;
        let flags = u32_at(payload, 32);
        if flags & !WEATHER_HEAD_PRESENT != 0 {
            return Err(DecodeError::unsupported_flags());
        }
        let raw_head = u64_at(payload, 52);
        let head_request_id = if flags & WEATHER_HEAD_PRESENT != 0 {
            Some(WeatherRequestId::new(raw_head))
        } else {
            if raw_head != 0 {
                // "head WeatherRequestId; inactive zero when head-present is clear".
                return Err(DecodeError::reserved_bits());
            }
            None
        };
        let centre_latitude_e7 = i32_at(payload, 60);
        let centre_longitude_e7 = i32_at(payload, 64);
        let radius_metres = u32_at(payload, 68);
        let earliest_issued_utc = i64_at(payload, 72);
        let required_valid_until_utc = i64_at(payload, 80);
        // The registry's §3 ranges are part of the context's definition, so a page outside them is
        // not a context this contract can describe.
        if !(-900_000_000..=900_000_000).contains(&centre_latitude_e7)
            || !(-1_800_000_000..=1_800_000_000).contains(&centre_longitude_e7)
            || radius_metres == 0
            || radius_metres > 100_000
            || required_valid_until_utc <= earliest_issued_utc
        {
            return Err(DecodeError::invalid_combination());
        }
        Ok(WeatherRequestContext {
            store_id: StoreId::new(bytes16_at(payload, 0)),
            current_request_id: WeatherRequestId::new(u64_at(payload, 16)),
            context_revision: u64_at(payload, 24),
            weather_logical_object_id: LogicalObjectId::new(u64_at(payload, 36)),
            repository_revision: Revision::new(u64_at(payload, 44)),
            head_request_id,
            centre_latitude_e7,
            centre_longitude_e7,
            radius_metres,
            earliest_issued_utc,
            required_valid_until_utc,
            state: WeatherContextState::from_u8(payload[88]).ok_or_else(DecodeError::unknown_enum)?,
        })
    }

    /// Encodes the response.
    pub fn encode(&self) -> [u8; WEATHER_REQUEST_LEN] {
        let mut out = [0u8; WEATHER_REQUEST_LEN];
        put_bytes(&mut out, 0, self.store_id.as_bytes());
        put_u64(&mut out, 16, self.current_request_id.get());
        put_u64(&mut out, 24, self.context_revision);
        put_u32(&mut out, 32, if self.head_request_id.is_some() { WEATHER_HEAD_PRESENT } else { 0 });
        put_u64(&mut out, 36, self.weather_logical_object_id.get());
        put_u64(&mut out, 44, self.repository_revision.get());
        put_u64(&mut out, 52, self.head_request_id.map_or(0, |id| id.get()));
        put_i32(&mut out, 60, self.centre_latitude_e7);
        put_i32(&mut out, 64, self.centre_longitude_e7);
        put_u32(&mut out, 68, self.radius_metres);
        put_i64(&mut out, 72, self.earliest_issued_utc);
        put_i64(&mut out, 80, self.required_valid_until_utc);
        out[88] = self.state.to_u8();
        out
    }
}

/// The size of an `ErrorBody` with no text, which is what a retained Aborted status carries.
pub const BARE_ERROR_BODY_LEN: usize = ERROR_BODY_PREFIX_LEN;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::{presence, ErrorCategory, RetryGuidance};
    use crate::metadata::{MetadataWriter, SchemaClass};
    use crate::registry::ObjectOutcome;
    use crate::result::ObjectResult;
    use std::vec;
    use std::vec::Vec;

    #[test]
    fn query_operation_states_round_trip() {
        let request = QueryOperation { operation_id: OperationId::new([0x77; 16]) };
        assert_eq!(QueryOperation::decode(&request.encode()).unwrap(), request);

        let mut out = [0u8; 128];
        for status in [
            OperationStatus::Unknown,
            OperationStatus::InProgress(OperationProgress {
                namespace: SubjectNamespace::Logical,
                subject_kind: ObjectKind::Route.to_u16(),
                phase: Phase::Streaming,
                flags: progress_flags::RESUMABLE
                    | progress_flags::SESSION_ATTACHED
                    | progress_flags::LOGICAL_ID_PRESENT,
                logical_object_id: LogicalObjectId::new(3),
                durable_offset: 262_144,
            }),
            OperationStatus::Committed(ResultEnvelope::Object(ObjectResult {
                operation_id: OperationId::new([1; 16]),
                store_id: StoreId::new([2; 16]),
                kind: ObjectKind::Route,
                outcome: ObjectOutcome::Committed,
                logical_object_id: LogicalObjectId::new(3),
                revision: Revision::new(4),
                length: 5,
                crc32: 6,
            })),
            OperationStatus::Aborted(ErrorBody {
                presence: presence::DURABLE_CLAIM_EXISTS | presence::CLAIM_IS_TERMINAL,
                ..ErrorBody::bare(ErrorCategory::CANCELLED, 1, RetryGuidance::REJECT_PERMANENTLY)
            }),
        ] {
            let len = status.encode_into(&mut out).unwrap();
            assert_eq!(OperationStatus::decode(&out[..len]).unwrap(), status);
        }
        assert_eq!(OperationStatus::Unknown.encoded_len(), 4);
        assert_eq!(OperationStatus::decode(&[0, 0, 0, 0, 9]).unwrap_err(), DecodeError::trailing_bytes());
    }

    #[test]
    fn a_progress_body_may_not_name_a_kind_in_namespace_none() {
        let progress = OperationProgress {
            namespace: SubjectNamespace::None,
            subject_kind: 1,
            phase: Phase::Aborting,
            flags: 0,
            logical_object_id: LogicalObjectId::ZERO,
            durable_offset: 0,
        };
        assert_eq!(OperationProgress::decode(&progress.encode()).unwrap_err(), DecodeError::invalid_combination());

        let with_id = OperationProgress {
            namespace: SubjectNamespace::Logical,
            subject_kind: ObjectKind::Ride.to_u16(),
            phase: Phase::Publishing,
            flags: 0,
            logical_object_id: LogicalObjectId::new(4),
            durable_offset: 0,
        };
        assert_eq!(OperationProgress::decode(&with_id.encode()).unwrap_err(), DecodeError::reserved_bits());
    }

    fn route_catalog_metadata<'a>(buffer: &'a mut [u8], name: &[u8]) -> MetadataEnvelope<'a> {
        let mut writer = MetadataWriter::new(buffer).unwrap();
        writer.push(0x8001, name).unwrap();
        writer.push(0x8002, &[2]).unwrap();
        writer.push(0x0003, &[1]).unwrap();
        let bytes = writer.finish(ObjectKind::Route, SchemaClass::Catalog);
        MetadataEnvelope::decode(bytes, MAX_CATALOG_ENVELOPE).unwrap()
    }

    #[test]
    fn a_catalog_page_round_trips_and_holds_its_ordering() {
        let mut first_buffer = [0u8; 96];
        let mut second_buffer = [0u8; 96];
        let entries = [
            CatalogEntry {
                logical_object_id: LogicalObjectId::new(1),
                revision: Revision::new(4),
                length: 900,
                crc32: 0x1111_1111,
                metadata: route_catalog_metadata(&mut first_buffer, b"Kaiserstuhl"),
            },
            CatalogEntry {
                logical_object_id: LogicalObjectId::new(9),
                revision: Revision::new(6),
                length: 1200,
                crc32: 0x2222_2222,
                metadata: route_catalog_metadata(&mut second_buffer, b"Feldberg"),
            },
        ];
        let mut entry_bytes = vec![0u8; 256];
        let mut used = 0;
        for entry in &entries {
            used += entry.encode_into(&mut entry_bytes[used..]).unwrap();
        }
        entry_bytes.truncate(used);

        let page = CatalogPage {
            store_id: StoreId::new([0x11; 16]),
            kind: ObjectKind::Route,
            revision: Revision::new(6),
            next_cursor: CatalogCursor::ZERO,
            entry_count: 2,
            entry_bytes: &entry_bytes,
        };
        let mut out = vec![0u8; 496];
        let len = page.encode_into(&mut out).unwrap();
        let decoded = CatalogPage::decode(&out[..len]).unwrap();
        assert_eq!(decoded.entries().count(), 2);
        let names: Vec<_> = decoded.entries().map(|entry| entry.metadata.field(1).unwrap().as_str().unwrap()).collect();
        assert_eq!(names, vec!["Kaiserstuhl", "Feldberg"]);
        let mut again = vec![0u8; 496];
        let again_len = decoded.encode_into(&mut again).unwrap();
        assert_eq!(&again[..again_len], &out[..len]);

        // A count that disagrees with the body is a nested-length fault.
        let mut broken = out[..len].to_vec();
        put_u16(&mut broken, 18, 1);
        assert_eq!(
            CatalogPage::decode(&broken).unwrap_err(),
            DecodeError::invalid_descriptor(crate::error::detail::descriptor::NESTED_LENGTH)
        );
    }

    #[test]
    fn the_largest_producible_catalog_page_fits_the_192_byte_floor() {
        // §8.2: one ceiling-sized entry makes a 176-byte payload; route's real maximum is 162.
        assert_eq!(CATALOG_PAGE_PREFIX_LEN + CATALOG_ENTRY_PREFIX_LEN + MAX_CATALOG_ENVELOPE, 176);
        assert_eq!(CATALOG_PAGE_PREFIX_LEN + CATALOG_ENTRY_PREFIX_LEN + 82, 162);
    }

    #[test]
    fn page_request_combinations_are_exactly_the_three_legal_ones() {
        let base = QueryCatalog { kind: ObjectKind::Route, page: PageRequest::CurrentFirstPage };
        assert_eq!(QueryCatalog::decode(&base.encode()).unwrap(), base);

        let unchanged = QueryCatalog { kind: ObjectKind::Route, page: PageRequest::UnchangedCheck(12) };
        assert_eq!(QueryCatalog::decode(&unchanged.encode()).unwrap(), unchanged);

        let store = StoreId::new([9; 16]);
        let mut cursor = CatalogCursor { revision: 12, next_entry_index: 2, kind_code: 1, crc32: 0 };
        cursor.crc32 = cursor.catalog_crc(store);
        let continued =
            QueryCatalog { kind: ObjectKind::Route, page: PageRequest::Continue { expected_revision: 12, cursor } };
        let bytes = continued.encode();
        assert_eq!(QueryCatalog::decode(&bytes).unwrap(), continued);
        cursor.verify_catalog(store).unwrap();
        assert_eq!(cursor.verify_catalog(StoreId::new([8; 16])).unwrap_err().category, ErrorCategory::CHECKSUM_FAILURE);

        // Cursor bit alone.
        let mut cursor_only = bytes;
        put_u16(&mut cursor_only, 2, query_flags::CURSOR);
        assert_eq!(QueryCatalog::decode(&cursor_only).unwrap_err(), DecodeError::invalid_combination());

        // Expected revision that disagrees with the cursor's.
        let mut mismatched = bytes;
        put_u64(&mut mismatched, 4, 13);
        assert_eq!(QueryCatalog::decode(&mismatched).unwrap_err(), DecodeError::invalid_combination());

        // A nonzero field with its flag clear.
        let mut stray = base.encode();
        put_u64(&mut stray, 4, 1);
        assert_eq!(QueryCatalog::decode(&stray).unwrap_err(), DecodeError::reserved_bits());
    }

    #[test]
    fn a_draft_page_orders_its_entries_and_pins_the_sealed_ref_rule() {
        let sealed = DraftEntry {
            child_operation_id: OperationId::new([1; 16]),
            draft_part_ref: crate::ids::DraftPartRef::new([0xAB; 16]),
            part_kind: DraftPartKind::StandaloneMapBlob,
            part_key: 1,
            state: DraftPartState::Sealed,
            durable_offset: 4096,
            declared_length: 4096,
            crc32: 0x3333,
        };
        let streaming = DraftEntry {
            child_operation_id: OperationId::new([2; 16]),
            draft_part_ref: crate::ids::DraftPartRef::ZERO,
            part_kind: DraftPartKind::MapShard,
            part_key: 7,
            state: DraftPartState::Streaming,
            durable_offset: 1024,
            declared_length: 8192,
            crc32: 0x4444,
        };
        let mut entries = [sealed; MAX_DRAFT_ENTRIES];
        entries[1] = streaming;
        let page = DraftPage {
            parent_operation_id: OperationId::new([0x31; 16]),
            draft_revision: 5,
            next_cursor: CatalogCursor::ZERO,
            flags: draft_page_flags::MANIFEST_STREAMING,
            count: 2,
            entries,
        };
        let mut out = [0u8; 496];
        let len = page.encode_into(&mut out).unwrap();
        assert_eq!(len, 44 + 2 * 68);
        let decoded = DraftPage::decode(&out[..len]).unwrap();
        assert_eq!(decoded.entries().len(), 2);
        assert_eq!(decoded.entries()[0], sealed);

        // A nonsealed entry carrying a ref.
        let mut bad = streaming;
        bad.draft_part_ref = crate::ids::DraftPartRef::new([1; 16]);
        assert_eq!(DraftEntry::decode(&bad.encode()).unwrap_err(), DecodeError::reserved_bits());

        // The largest response is 452 payload bytes, below 496.
        assert_eq!(DRAFT_PAGE_PREFIX_LEN + MAX_DRAFT_ENTRIES * DRAFT_ENTRY_LEN, 452);
    }

    #[test]
    fn query_draft_limits_are_one_through_six() {
        let request = QueryDraft {
            parent_operation_id: OperationId::new([3; 16]),
            requested_limit: 6,
            page: PageRequest::CurrentFirstPage,
        };
        assert_eq!(QueryDraft::decode(&request.encode()).unwrap(), request);
        let mut zero = request.encode();
        zero[18] = 0;
        assert_eq!(QueryDraft::decode(&zero).unwrap_err(), DecodeError::invalid_combination());
        let mut seven = request.encode();
        seven[18] = 7;
        assert_eq!(QueryDraft::decode(&seven).unwrap_err(), DecodeError::invalid_combination());
    }

    #[test]
    fn the_weather_context_round_trips_and_enforces_the_registry_ranges() {
        let context = WeatherRequestContext {
            store_id: StoreId::new([0x21; 16]),
            current_request_id: WeatherRequestId::new(12),
            context_revision: 3,
            weather_logical_object_id: LogicalObjectId::ZERO,
            repository_revision: Revision::new(88),
            head_request_id: Some(WeatherRequestId::new(11)),
            centre_latitude_e7: 480_000_000,
            centre_longitude_e7: -1_200_000_000,
            radius_metres: 50_000,
            earliest_issued_utc: 1_700_000_000,
            required_valid_until_utc: 1_700_086_400,
            state: WeatherContextState::Satisfied,
        };
        let bytes = context.encode();
        assert_eq!(bytes.len(), 96);
        assert_eq!(WeatherRequestContext::decode(&bytes).unwrap(), context);

        let pending = WeatherRequestContext { head_request_id: None, state: WeatherContextState::Pending, ..context };
        assert_eq!(WeatherRequestContext::decode(&pending.encode()).unwrap(), pending);

        let mut stray_head = pending.encode();
        put_u64(&mut stray_head, 52, 5);
        assert_eq!(WeatherRequestContext::decode(&stray_head).unwrap_err(), DecodeError::reserved_bits());

        let mut bad_radius = context.encode();
        put_u32(&mut bad_radius, 68, 100_001);
        assert_eq!(WeatherRequestContext::decode(&bad_radius).unwrap_err(), DecodeError::invalid_combination());

        let mut bad_state = context.encode();
        bad_state[88] = 0;
        assert_eq!(WeatherRequestContext::decode(&bad_state).unwrap_err(), DecodeError::unknown_enum());
    }
}
