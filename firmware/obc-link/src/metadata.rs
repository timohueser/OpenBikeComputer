//! The metadata envelope codec (`Device_Object_Protocol_v3.md` §2.2) and the per-kind field tables
//! it is validated against (`Device_Object_Registries_v2.md` §4).
//!
//! An envelope is the one place a domain adds a bounded declared fact without touching the common
//! wire contract, which is why it has its own codec rather than being another fixed layout. Two
//! layers of checking are deliberately kept apart:
//!
//! 1. [`MetadataEnvelope::decode`] establishes **canonical form** — the eight-byte header, fields
//!    that are strictly increasing by unique nonzero base tag, lengths that sum exactly to
//!    `encoded_field_bytes`, no padding and no trailing bytes. It knows nothing about kinds.
//! 2. [`Schema::validate`] establishes **schema conformance** — this envelope belongs to that
//!    ObjectKind and operation, every registered required field appears exactly once, every field's
//!    width and range is the registered one, and text obeys §2.2's rules.
//!
//! A decoder that only did the first would accept a route retention of `200`; one that only did the
//! second could not tell where the envelope ends. Both are required, and §2.2 says why the boundary
//! matters: "silently ignoring a requested mutation is forbidden".

use crate::codec::{put_u16, u16_at};
use crate::error::{detail, DecodeError};
use crate::registry::{schema_version, ObjectKind};
use crate::{BufferTooSmall, EncodeResult};

/// The envelope header, in bytes.
pub const ENVELOPE_HEADER_LEN: usize = 8;

/// The common ceiling on a Put or patch envelope (§1).
pub const MAX_PUT_ENVELOPE: usize = 128;

/// The common ceiling on a catalog projection envelope (§1).
pub const MAX_CATALOG_ENVELOPE: usize = 96;

/// The largest field body a Put or patch envelope may carry: 128 less the header.
pub const MAX_PUT_FIELD_BYTES: usize = MAX_PUT_ENVELOPE - ENVELOPE_HEADER_LEN;

/// The largest field body a catalog envelope may carry: 96 less the header.
pub const MAX_CATALOG_FIELD_BYTES: usize = MAX_CATALOG_ENVELOPE - ENVELOPE_HEADER_LEN;

/// The critical bit of a field tag. Its low 15 bits are the base tag.
pub const CRITICAL_BIT: u16 = 0x8000;

/// Which of the three registered schema families an envelope belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchemaClass {
    /// The `StartUpload` Put envelope, version `1`.
    Put,
    /// The `SetMetadata` patch envelope, version `128`.
    Patch,
    /// The catalog projection envelope, version `64`.
    Catalog,
}

impl SchemaClass {
    /// The registry's version constant for this family.
    pub const fn version(self) -> u8 {
        match self {
            SchemaClass::Put => schema_version::PUT,
            SchemaClass::Patch => schema_version::PATCH,
            SchemaClass::Catalog => schema_version::CATALOG,
        }
    }

    /// The common envelope ceiling for this family.
    pub const fn ceiling(self) -> usize {
        match self {
            SchemaClass::Put | SchemaClass::Patch => MAX_PUT_ENVELOPE,
            SchemaClass::Catalog => MAX_CATALOG_ENVELOPE,
        }
    }

    /// True for a family carried by a mutating request, where §2.2 rejects *every* unknown field.
    pub const fn is_mutating(self) -> bool {
        matches!(self, SchemaClass::Put | SchemaClass::Patch)
    }

    /// The name used in fixture JSON.
    pub const fn name(self) -> &'static str {
        match self {
            SchemaClass::Put => "put",
            SchemaClass::Patch => "patch",
            SchemaClass::Catalog => "catalog",
        }
    }
}

/// A registered field's wire type, width, and range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldType {
    /// One byte, any value.
    U8,
    /// One byte, inclusive range.
    U8Range(u8, u8),
    /// One byte, exactly `0` or `1` (§2.2's boolean).
    Bool,
    /// Two little-endian bytes.
    U16,
    /// Four little-endian bytes.
    U32,
    /// Eight little-endian bytes.
    U64,
    /// Four little-endian two's-complement bytes.
    I32,
    /// Eight little-endian two's-complement bytes.
    I64,
    /// UTF-8 text, inclusive encoded-byte bounds. Empty is legal only when the minimum is zero.
    Text(u16, u16),
    /// A byte string of exactly this length, copied verbatim.
    Bytes(u16),
}

impl FieldType {
    /// True when `len` is a legal encoded length for this type.
    pub const fn accepts_len(self, len: u16) -> bool {
        match self {
            FieldType::U8 | FieldType::U8Range(_, _) | FieldType::Bool => len == 1,
            FieldType::U16 => len == 2,
            FieldType::U32 | FieldType::I32 => len == 4,
            FieldType::U64 | FieldType::I64 => len == 8,
            FieldType::Text(min, max) => len >= min && len <= max,
            FieldType::Bytes(exact) => len == exact,
        }
    }

    /// The largest encoded value length this type can carry — the input to the registry's maximum
    /// envelope lengths.
    pub const fn max_len(self) -> u16 {
        match self {
            FieldType::U8 | FieldType::U8Range(_, _) | FieldType::Bool => 1,
            FieldType::U16 => 2,
            FieldType::U32 | FieldType::I32 => 4,
            FieldType::U64 | FieldType::I64 => 8,
            FieldType::Text(_, max) => max,
            FieldType::Bytes(exact) => exact,
        }
    }

    /// Checks a value's bytes against the type's range and text rules. Length is already checked.
    fn accepts_value(self, value: &[u8]) -> bool {
        match self {
            FieldType::Bool => value[0] <= 1,
            FieldType::U8Range(min, max) => value[0] >= min && value[0] <= max,
            FieldType::Text(_, _) => text_is_clean(value),
            _ => true,
        }
    }
}

/// One registered metadata field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FieldSpec {
    /// The full tag including its critical bit.
    pub tag: u16,
    /// The wire type.
    pub ty: FieldType,
    /// Whether every projection/request of this schema must carry it.
    pub required: bool,
    /// The registry's name, for fixtures and diagnostics.
    pub name: &'static str,
}

impl FieldSpec {
    const fn new(tag: u16, ty: FieldType, required: bool, name: &'static str) -> Self {
        FieldSpec { tag, ty, required, name }
    }

    /// The tag with its critical bit removed.
    pub const fn base_tag(&self) -> u16 {
        self.tag & !CRITICAL_BIT
    }

    /// True when the registry marks the field critical.
    pub const fn is_critical(&self) -> bool {
        self.tag & CRITICAL_BIT != 0
    }
}

/// One registered schema: a kind, a family, its version, and its field table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Schema {
    /// The owning kind. Also the envelope's `schema_id`.
    pub kind: ObjectKind,
    /// Which family.
    pub class: SchemaClass,
    /// The registry's version constant.
    pub version: u8,
    /// The registry's maximum encoded envelope length, header included (§4's table).
    pub max_encoded_len: usize,
    /// The field table, in ascending base-tag order.
    pub fields: &'static [FieldSpec],
}

const ROUTE_PUT: [FieldSpec; 1] = [FieldSpec::new(0x8001, FieldType::U8Range(0, 5), true, "retention")];

const WEATHER_PUT: [FieldSpec; 6] = [
    FieldSpec::new(0x8001, FieldType::U64, true, "weatherRequestId"),
    FieldSpec::new(0x8002, FieldType::I32, true, "coverageCentreLatitude"),
    FieldSpec::new(0x8003, FieldType::I32, true, "coverageCentreLongitude"),
    FieldSpec::new(0x8004, FieldType::U32, true, "coverageRadiusMetres"),
    FieldSpec::new(0x8005, FieldType::I64, true, "issuedUtc"),
    FieldSpec::new(0x8006, FieldType::I64, true, "validUntilUtc"),
];

const EMPTY_FIELDS: [FieldSpec; 0] = [];

const ROUTE_PATCH: [FieldSpec; 3] = [
    FieldSpec::new(0x8001, FieldType::U8Range(0, 5), false, "retention"),
    FieldSpec::new(0x8002, FieldType::Bool, false, "selected"),
    FieldSpec::new(0x8003, FieldType::Text(1, 48), false, "displayName"),
];

const VOLUME_PATCH: [FieldSpec; 1] = [FieldSpec::new(0x8001, FieldType::Bool, false, "selected")];

// Ordered by *base* tag, which is the order the wire requires: §2.2 says "changing only the
// critical bit does not create another field", so `0x0003` sorts after `0x8002`, not before it.
// `0x0003` and `0x0004` are the registry's two noncritical rows: a reader that does not know them
// may skip them, which is exactly why the device may omit them when it lacks the fact.
const ROUTE_CATALOG: [FieldSpec; 4] = [
    FieldSpec::new(0x8001, FieldType::Text(1, 48), true, "displayName"),
    FieldSpec::new(0x8002, FieldType::U8Range(0, 5), true, "retention"),
    FieldSpec::new(0x0003, FieldType::Bool, false, "selected"),
    FieldSpec::new(0x0004, FieldType::I64, false, "trustedCreationUtc"),
];

const TRIP_CATALOG: [FieldSpec; 2] = [
    FieldSpec::new(0x8001, FieldType::Text(1, 48), true, "displayName"),
    FieldSpec::new(0x8002, FieldType::U16, true, "stageCount"),
];

const RIDE_CATALOG: [FieldSpec; 4] = [
    FieldSpec::new(0x8001, FieldType::I64, true, "startUtc"),
    FieldSpec::new(0x8002, FieldType::U32, true, "durationSeconds"),
    FieldSpec::new(0x8003, FieldType::U32, true, "distanceMetres"),
    FieldSpec::new(0x8004, FieldType::Bool, true, "imported"),
];

const WEATHER_CATALOG: [FieldSpec; 3] = [
    FieldSpec::new(0x8001, FieldType::U64, true, "weatherRequestId"),
    FieldSpec::new(0x8002, FieldType::I64, true, "issuedUtc"),
    FieldSpec::new(0x8003, FieldType::I64, true, "validUntilUtc"),
];

const VOLUME_CATALOG: [FieldSpec; 3] = [
    FieldSpec::new(0x8001, FieldType::Text(1, 32), true, "displayName"),
    FieldSpec::new(0x8002, FieldType::Bool, true, "selected"),
    FieldSpec::new(0x8003, FieldType::U16, true, "partCount"),
];

const UPDATE_CATALOG: [FieldSpec; 3] = [
    FieldSpec::new(0x8001, FieldType::Text(1, 24), true, "semanticVersion"),
    FieldSpec::new(0x8002, FieldType::U8Range(1, 6), true, "state"),
    FieldSpec::new(0x8003, FieldType::Bytes(32), true, "imageDigest"),
];

const fn schema_of(
    kind: ObjectKind,
    class: SchemaClass,
    max_encoded_len: usize,
    fields: &'static [FieldSpec],
) -> Schema {
    Schema { kind, class, version: class.version(), max_encoded_len, fields }
}

impl Schema {
    /// Looks up the registered schema for a kind and family, if one exists.
    ///
    /// `None` means the registry does not define that combination at all — `SetMetadata` on trip,
    /// ride, weather, or update package — and a request naming it is `unsupportedCapability`.
    pub const fn lookup(kind: ObjectKind, class: SchemaClass) -> Option<Schema> {
        Some(match (kind, class) {
            (ObjectKind::Route, SchemaClass::Put) => schema_of(kind, class, 13, &ROUTE_PUT),
            (ObjectKind::Trip, SchemaClass::Put) => schema_of(kind, class, 8, &EMPTY_FIELDS),
            (ObjectKind::Ride, SchemaClass::Put) => schema_of(kind, class, 8, &EMPTY_FIELDS),
            (ObjectKind::Weather, SchemaClass::Put) => schema_of(kind, class, 68, &WEATHER_PUT),
            (ObjectKind::VolumeManifest, SchemaClass::Put) => schema_of(kind, class, 8, &EMPTY_FIELDS),
            (ObjectKind::UpdatePackage, SchemaClass::Put) => schema_of(kind, class, 8, &EMPTY_FIELDS),
            (ObjectKind::Route, SchemaClass::Patch) => schema_of(kind, class, 70, &ROUTE_PATCH),
            (ObjectKind::VolumeManifest, SchemaClass::Patch) => schema_of(kind, class, 13, &VOLUME_PATCH),
            (ObjectKind::Route, SchemaClass::Catalog) => schema_of(kind, class, 82, &ROUTE_CATALOG),
            (ObjectKind::Trip, SchemaClass::Catalog) => schema_of(kind, class, 66, &TRIP_CATALOG),
            (ObjectKind::Ride, SchemaClass::Catalog) => schema_of(kind, class, 41, &RIDE_CATALOG),
            (ObjectKind::Weather, SchemaClass::Catalog) => schema_of(kind, class, 44, &WEATHER_CATALOG),
            (ObjectKind::VolumeManifest, SchemaClass::Catalog) => schema_of(kind, class, 55, &VOLUME_CATALOG),
            (ObjectKind::UpdatePackage, SchemaClass::Catalog) => schema_of(kind, class, 77, &UPDATE_CATALOG),
            _ => return None,
        })
    }

    /// Finds the registered field with this base tag.
    pub fn field(&self, base_tag: u16) -> Option<&'static FieldSpec> {
        self.fields.iter().find(|spec| spec.base_tag() == base_tag)
    }

    /// Validates a canonically decoded envelope against this schema.
    ///
    /// §2.2: schema ID matches the containing kind, the version is the one advertised for the
    /// operation, "every registered required field appears exactly once", mutating requests reject
    /// every unknown field and response decoders reject unknown *critical* fields while skipping
    /// well-formed unknown noncritical ones.
    pub fn validate(&self, envelope: &MetadataEnvelope<'_>) -> crate::Result<()> {
        if envelope.schema_id != self.kind.to_u16() {
            return Err(DecodeError::invalid_combination());
        }
        if envelope.schema_version != self.version {
            return Err(DecodeError::unsupported_capability(detail::capability::SCHEMA_VERSION));
        }
        let mut seen: u32 = 0;
        for field in envelope.fields() {
            match self.fields.iter().position(|spec| spec.base_tag() == field.base_tag) {
                Some(index) => {
                    let spec = &self.fields[index];
                    if spec.is_critical() != field.critical {
                        // The critical bit is registry-assigned, so a flipped one names a field
                        // this schema does not have rather than the same one under another rule.
                        return Err(DecodeError::invalid_combination());
                    }
                    let len = field.value.len() as u16;
                    if !spec.ty.accepts_len(len) || !spec.ty.accepts_value(field.value) {
                        return Err(DecodeError::invalid_descriptor(detail::descriptor::NONCANONICAL_METADATA));
                    }
                    seen |= 1 << index;
                }
                None => {
                    if self.class.is_mutating() || field.critical {
                        return Err(DecodeError::invalid_combination());
                    }
                    // A well-formed unknown noncritical field in a projection is skipped: §2.2
                    // permits exactly that, and only for a response decoder.
                }
            }
        }
        for (index, spec) in self.fields.iter().enumerate() {
            if spec.required && seen & (1 << index) == 0 {
                return Err(DecodeError::invalid_combination());
            }
        }
        // Last, because an envelope that is over the registered maximum is almost always over it
        // *because of* a field the loop above has a narrower answer for. `nestedLength` is what
        // remains when every field is registered, in order, and correctly sized, and the whole is
        // still longer than `Device_Object_Registries_v2.md` §4's table allows.
        if envelope.encoded_len() > self.max_encoded_len {
            return Err(DecodeError::invalid_descriptor(detail::descriptor::NESTED_LENGTH));
        }
        Ok(())
    }

    /// The largest envelope this table can produce, recomputed from the field types.
    ///
    /// §1 requires implementations to "recompute these maxima in shared codec tests when any
    /// constituent limit or prefix changes"; this is the recomputation, and a unit test holds it
    /// against [`max_encoded_len`](Self::max_encoded_len).
    pub fn computed_max_encoded_len(&self) -> usize {
        ENVELOPE_HEADER_LEN + self.fields.iter().map(|spec| 4 + usize::from(spec.ty.max_len())).sum::<usize>()
    }
}

/// One decoded field of an envelope.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MetadataField<'a> {
    /// The nonzero base tag: the tag with its critical bit removed.
    pub base_tag: u16,
    /// True when the critical bit was set.
    pub critical: bool,
    /// The value bytes, borrowed.
    pub value: &'a [u8],
}

impl<'a> MetadataField<'a> {
    /// The full tag as encoded.
    pub const fn tag(&self) -> u16 {
        if self.critical {
            self.base_tag | CRITICAL_BIT
        } else {
            self.base_tag
        }
    }

    /// The value as a `u8`, when the field is one byte wide.
    pub fn as_u8(&self) -> Option<u8> {
        match self.value {
            [b] => Some(*b),
            _ => None,
        }
    }

    /// The value as a little-endian `u16`.
    pub fn as_u16(&self) -> Option<u16> {
        self.value.try_into().ok().map(u16::from_le_bytes)
    }

    /// The value as a little-endian `u32`.
    pub fn as_u32(&self) -> Option<u32> {
        self.value.try_into().ok().map(u32::from_le_bytes)
    }

    /// The value as a little-endian `u64`.
    pub fn as_u64(&self) -> Option<u64> {
        self.value.try_into().ok().map(u64::from_le_bytes)
    }

    /// The value as a little-endian two's-complement `i32`.
    pub fn as_i32(&self) -> Option<i32> {
        self.as_u32().map(|raw| raw as i32)
    }

    /// The value as a little-endian two's-complement `i64`.
    pub fn as_i64(&self) -> Option<i64> {
        self.as_u64().map(|raw| raw as i64)
    }

    /// The value as text, when it is valid UTF-8. Schema validation has already applied §2.2's
    /// rules, so a schema-validated text field always yields `Some`.
    pub fn as_str(&self) -> Option<&'a str> {
        core::str::from_utf8(self.value).ok()
    }
}

/// A canonically decoded metadata envelope.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MetadataEnvelope<'a> {
    /// The numeric `ObjectKind` this envelope belongs to.
    pub schema_id: u16,
    /// The schema version.
    pub schema_version: u8,
    /// Exactly `encoded_field_bytes` bytes of field body.
    pub field_bytes: &'a [u8],
    /// The number of fields in that body.
    pub field_count: u16,
}

impl<'a> MetadataEnvelope<'a> {
    /// The exact encoded length: `8 + encoded_field_bytes` (§2.2).
    pub fn encoded_len(&self) -> usize {
        ENVELOPE_HEADER_LEN + self.field_bytes.len()
    }

    /// Reads the envelope at the start of `bytes`, returning it and the bytes it consumed.
    ///
    /// This is the form Put and patch bodies need, where the envelope is the tail of a larger
    /// message: its own header gives the decoder "an unambiguous end-of-envelope boundary".
    pub fn decode_prefix(bytes: &'a [u8], ceiling: usize) -> crate::Result<(Self, usize)> {
        DecodeError::min_len(bytes, ENVELOPE_HEADER_LEN)?;
        let schema_id = u16_at(bytes, 0);
        let schema_version = bytes[2];
        if bytes[3] != 0 {
            // §2.2: "The header flags are zero."
            return Err(DecodeError::reserved_bits());
        }
        let encoded_field_bytes = usize::from(u16_at(bytes, 4));
        let field_count = u16_at(bytes, 6);
        let total = ENVELOPE_HEADER_LEN + encoded_field_bytes;
        if total > ceiling {
            return Err(DecodeError::invalid_descriptor(detail::descriptor::NESTED_LENGTH));
        }
        if bytes.len() < total {
            return Err(DecodeError::truncated());
        }
        let envelope = MetadataEnvelope {
            schema_id,
            schema_version,
            field_bytes: &bytes[ENVELOPE_HEADER_LEN..total],
            field_count,
        };
        envelope.check_canonical()?;
        Ok((envelope, total))
    }

    /// Decodes an envelope that is exactly `bytes`, rejecting any trailing byte.
    pub fn decode(bytes: &'a [u8], ceiling: usize) -> crate::Result<Self> {
        let (envelope, used) = Self::decode_prefix(bytes, ceiling)?;
        if used != bytes.len() {
            return Err(DecodeError::trailing_bytes());
        }
        Ok(envelope)
    }

    /// §2.2's canonical-form rules, independent of any schema.
    fn check_canonical(&self) -> crate::Result<()> {
        let mut offset = 0usize;
        let mut count = 0u32;
        let mut previous: Option<u16> = None;
        while offset < self.field_bytes.len() {
            if self.field_bytes.len() - offset < 4 {
                // A partial field header is padding or truncation, both forbidden.
                return Err(DecodeError::invalid_descriptor(detail::descriptor::NONCANONICAL_METADATA));
            }
            let tag = u16_at(self.field_bytes, offset);
            let value_len = usize::from(u16_at(self.field_bytes, offset + 2));
            let base_tag = tag & !CRITICAL_BIT;
            if base_tag == 0 {
                return Err(DecodeError::invalid_descriptor(detail::descriptor::NONCANONICAL_METADATA));
            }
            // §2.2 groups duplicate and out-of-order base tags under `noncanonicalMetadata`, while
            // §12 registers `duplicateField` and `outOfOrderField` as their own details and does
            // *not* list either among the nine reserved-never-emitted rows. The narrower detail is
            // emitted here so both registered codes have an emitter; the category is
            // `invalidDescriptor` under either reading, so no peer can disagree about the outcome.
            match previous {
                Some(previous) if previous == base_tag => {
                    return Err(DecodeError::invalid_descriptor(detail::descriptor::DUPLICATE_FIELD))
                }
                Some(previous) if previous > base_tag => {
                    return Err(DecodeError::invalid_descriptor(detail::descriptor::OUT_OF_ORDER_FIELD))
                }
                _ => {}
            }
            previous = Some(base_tag);
            let end = offset
                .checked_add(4)
                .and_then(|start| start.checked_add(value_len))
                .ok_or_else(|| DecodeError::invalid_descriptor(detail::descriptor::NONCANONICAL_METADATA))?;
            if end > self.field_bytes.len() {
                return Err(DecodeError::invalid_descriptor(detail::descriptor::NONCANONICAL_METADATA));
            }
            offset = end;
            count += 1;
        }
        if offset != self.field_bytes.len() || u32::from(self.field_count) != count {
            return Err(DecodeError::invalid_descriptor(detail::descriptor::NONCANONICAL_METADATA));
        }
        Ok(())
    }

    /// Iterates the fields in wire order. Canonical form is already established, so this cannot
    /// fail and never yields a partial field.
    pub fn fields(&self) -> FieldIter<'a> {
        FieldIter { bytes: self.field_bytes, offset: 0 }
    }

    /// Finds a field by base tag.
    pub fn field(&self, base_tag: u16) -> Option<MetadataField<'a>> {
        self.fields().find(|field| field.base_tag == base_tag)
    }

    /// Encodes the envelope into `out`, returning its exact length.
    pub fn encode_into(&self, out: &mut [u8]) -> EncodeResult {
        let needed = self.encoded_len();
        if out.len() < needed {
            return Err(BufferTooSmall { needed, available: out.len() });
        }
        let out = &mut out[..needed];
        put_u16(out, 0, self.schema_id);
        out[2] = self.schema_version;
        out[3] = 0;
        put_u16(out, 4, self.field_bytes.len() as u16);
        put_u16(out, 6, self.field_count);
        out[ENVELOPE_HEADER_LEN..].copy_from_slice(self.field_bytes);
        Ok(needed)
    }

    /// The canonical empty envelope for a schema: "Even a schema with no fields uses its canonical
    /// eight-byte header with both counts zero."
    pub const fn empty(kind: ObjectKind, class: SchemaClass) -> Self {
        MetadataEnvelope { schema_id: kind.to_u16(), schema_version: class.version(), field_bytes: &[], field_count: 0 }
    }
}

/// Iterator over a canonical envelope's fields.
#[derive(Debug, Clone, Copy)]
pub struct FieldIter<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Iterator for FieldIter<'a> {
    type Item = MetadataField<'a>;

    /// Yields the next field, or `None` at the end of the body **or** at the first byte the body
    /// cannot describe.
    ///
    /// `check_canonical` has already proved that a *decoded* envelope never takes the second
    /// branch. The bounds checks are here for the other way an envelope can exist: this type's
    /// fields are public, so a caller may hand-build one whose `field_bytes` does not agree with
    /// its own field headers. This crate is destined for the device image, so that must end the
    /// iteration rather than panic — which is also what [`CatalogEntryIter`](crate::query) does
    /// with its own re-parse.
    fn next(&mut self) -> Option<Self::Item> {
        let remaining = self.bytes.len().checked_sub(self.offset)?;
        if remaining == 0 {
            return None;
        }
        if remaining < 4 {
            return None;
        }
        let tag = u16_at(self.bytes, self.offset);
        let value_len = usize::from(u16_at(self.bytes, self.offset + 2));
        let start = self.offset + 4;
        let end = start.checked_add(value_len)?;
        if end > self.bytes.len() {
            return None;
        }
        self.offset = end;
        Some(MetadataField {
            base_tag: tag & !CRITICAL_BIT,
            critical: tag & CRITICAL_BIT != 0,
            value: &self.bytes[start..end],
        })
    }
}

/// Builds a canonical envelope into a caller-owned buffer, without allocating.
///
/// Fields must be pushed in ascending base-tag order; pushing out of order or twice is refused
/// rather than producing a noncanonical envelope that the peer would reject.
#[derive(Debug)]
pub struct MetadataWriter<'a> {
    buffer: &'a mut [u8],
    used: usize,
    count: u16,
    previous: Option<u16>,
}

/// Why a metadata envelope could not be built.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteError {
    /// The buffer cannot hold another field.
    OutOfRoom,
    /// The tag is zero, or not strictly above the previous one.
    OutOfOrder,
    /// The value is longer than a `u16` length can describe, or than the ceiling allows.
    ValueTooLong,
}

impl<'a> MetadataWriter<'a> {
    /// Starts an envelope in `buffer`, reserving its eight-byte header.
    pub fn new(buffer: &'a mut [u8]) -> core::result::Result<Self, WriteError> {
        if buffer.len() < ENVELOPE_HEADER_LEN {
            return Err(WriteError::OutOfRoom);
        }
        Ok(MetadataWriter { buffer, used: ENVELOPE_HEADER_LEN, count: 0, previous: None })
    }

    /// Appends one field.
    pub fn push(&mut self, tag: u16, value: &[u8]) -> core::result::Result<(), WriteError> {
        let base_tag = tag & !CRITICAL_BIT;
        if base_tag == 0 {
            return Err(WriteError::OutOfOrder);
        }
        if let Some(previous) = self.previous {
            if base_tag <= previous {
                return Err(WriteError::OutOfOrder);
            }
        }
        if value.len() > usize::from(u16::MAX) {
            return Err(WriteError::ValueTooLong);
        }
        let needed = self.used + 4 + value.len();
        if needed > self.buffer.len() {
            return Err(WriteError::OutOfRoom);
        }
        put_u16(self.buffer, self.used, tag);
        put_u16(self.buffer, self.used + 2, value.len() as u16);
        self.buffer[self.used + 4..needed].copy_from_slice(value);
        self.used = needed;
        self.count += 1;
        self.previous = Some(base_tag);
        Ok(())
    }

    /// Writes the header and returns the complete envelope bytes.
    pub fn finish(self, kind: ObjectKind, class: SchemaClass) -> &'a [u8] {
        let field_bytes = self.used - ENVELOPE_HEADER_LEN;
        put_u16(self.buffer, 0, kind.to_u16());
        self.buffer[2] = class.version();
        self.buffer[3] = 0;
        put_u16(self.buffer, 4, field_bytes as u16);
        put_u16(self.buffer, 6, self.count);
        &self.buffer[..self.used]
    }
}

/// §2.2's text rule: shortest-form valid UTF-8 with no NUL, C0/C1 control, surrogate, or
/// noncharacter scalar.
///
/// Rust's own UTF-8 validation already rejects overlong encodings and surrogates, which is exactly
/// "shortest-form" and the surrogate half of the rule; the rest is a scalar scan. Accepted bytes
/// are canonical as-is — this function never normalizes, trims, or case-folds, because §2.2
/// forbids a codec from rewriting them.
pub fn text_is_clean(bytes: &[u8]) -> bool {
    let Ok(text) = core::str::from_utf8(bytes) else {
        return false;
    };
    text.chars().all(|scalar| {
        let code = u32::from(scalar);
        let control = code < 0x20 || (0x7F..=0x9F).contains(&code);
        let noncharacter = (0xFDD0..=0xFDEF).contains(&code) || matches!(code & 0xFFFF, 0xFFFE | 0xFFFF);
        !control && !noncharacter
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::vec;
    use std::vec::Vec;

    fn envelope(kind: ObjectKind, class: SchemaClass, fields: &[(u16, Vec<u8>)]) -> Vec<u8> {
        let mut buffer = [0u8; 256];
        let mut writer = MetadataWriter::new(&mut buffer).unwrap();
        for (tag, value) in fields {
            writer.push(*tag, value).unwrap();
        }
        writer.finish(kind, class).to_vec()
    }

    #[test]
    fn every_registered_schema_maximum_matches_its_field_table() {
        // `Device_Object_Registries_v2.md` §4's table, recomputed from the field types.
        for kind in ObjectKind::ALL {
            for class in [SchemaClass::Put, SchemaClass::Patch, SchemaClass::Catalog] {
                let Some(schema) = Schema::lookup(kind, class) else { continue };
                assert_eq!(
                    schema.computed_max_encoded_len(),
                    schema.max_encoded_len,
                    "{} {} maximum drifted",
                    kind.name(),
                    class.name()
                );
                assert!(schema.max_encoded_len <= class.ceiling());
            }
        }
        // The two figures §1 and §2.2 quote as the producible per-kind maxima.
        assert_eq!(Schema::lookup(ObjectKind::Weather, SchemaClass::Put).unwrap().max_encoded_len, 68);
        assert_eq!(Schema::lookup(ObjectKind::Route, SchemaClass::Catalog).unwrap().max_encoded_len, 82);
    }

    #[test]
    fn set_metadata_exists_only_where_the_registry_says() {
        for kind in ObjectKind::ALL {
            assert_eq!(
                Schema::lookup(kind, SchemaClass::Patch).is_some(),
                kind.supports_set_metadata(),
                "{} patch schema disagrees with the lifecycle table",
                kind.name()
            );
        }
    }

    #[test]
    fn empty_envelope_is_eight_canonical_bytes() {
        let bytes = envelope(ObjectKind::Trip, SchemaClass::Put, &[]);
        assert_eq!(bytes, vec![0x02, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00]);
        let decoded = MetadataEnvelope::decode(&bytes, MAX_PUT_ENVELOPE).unwrap();
        assert_eq!(decoded.field_count, 0);
        assert_eq!(decoded, MetadataEnvelope::empty(ObjectKind::Trip, SchemaClass::Put));
        Schema::lookup(ObjectKind::Trip, SchemaClass::Put).unwrap().validate(&decoded).unwrap();
    }

    #[test]
    fn weather_put_round_trips_at_its_registered_maximum() {
        let bytes = envelope(
            ObjectKind::Weather,
            SchemaClass::Put,
            &[
                (0x8001, 42u64.to_le_bytes().to_vec()),
                (0x8002, 480_000_000i32.to_le_bytes().to_vec()),
                (0x8003, 77_000_000i32.to_le_bytes().to_vec()),
                (0x8004, 50_000u32.to_le_bytes().to_vec()),
                (0x8005, 1_700_000_000i64.to_le_bytes().to_vec()),
                (0x8006, 1_700_050_000i64.to_le_bytes().to_vec()),
            ],
        );
        assert_eq!(bytes.len(), 68);
        let decoded = MetadataEnvelope::decode(&bytes, MAX_PUT_ENVELOPE).unwrap();
        Schema::lookup(ObjectKind::Weather, SchemaClass::Put).unwrap().validate(&decoded).unwrap();
        assert_eq!(decoded.field(1).unwrap().as_u64(), Some(42));
        assert_eq!(decoded.field(2).unwrap().as_i32(), Some(480_000_000));
        let mut out = [0u8; 128];
        let len = decoded.encode_into(&mut out).unwrap();
        assert_eq!(&out[..len], &bytes[..]);
    }

    #[test]
    fn duplicate_and_out_of_order_tags_have_their_own_details() {
        let mut bytes = envelope(ObjectKind::Route, SchemaClass::Patch, &[(0x8001, vec![1]), (0x8002, vec![1])]);
        // Rewrite the second tag to duplicate the first.
        put_u16(&mut bytes, ENVELOPE_HEADER_LEN + 5, 0x8001);
        assert_eq!(
            MetadataEnvelope::decode(&bytes, MAX_PUT_ENVELOPE).unwrap_err(),
            DecodeError::invalid_descriptor(detail::descriptor::DUPLICATE_FIELD)
        );

        let mut bytes = envelope(ObjectKind::Route, SchemaClass::Patch, &[(0x8001, vec![1]), (0x8002, vec![1])]);
        put_u16(&mut bytes, ENVELOPE_HEADER_LEN, 0x8003);
        assert_eq!(
            MetadataEnvelope::decode(&bytes, MAX_PUT_ENVELOPE).unwrap_err(),
            DecodeError::invalid_descriptor(detail::descriptor::OUT_OF_ORDER_FIELD)
        );
    }

    #[test]
    fn structural_faults_are_noncanonical_metadata() {
        // A field count that disagrees with the body.
        let mut bytes = envelope(ObjectKind::Route, SchemaClass::Put, &[(0x8001, vec![2])]);
        put_u16(&mut bytes, 6, 2);
        assert_eq!(
            MetadataEnvelope::decode(&bytes, MAX_PUT_ENVELOPE).unwrap_err(),
            DecodeError::invalid_descriptor(detail::descriptor::NONCANONICAL_METADATA)
        );

        // A value length that runs past the body.
        let mut bytes = envelope(ObjectKind::Route, SchemaClass::Put, &[(0x8001, vec![2])]);
        put_u16(&mut bytes, ENVELOPE_HEADER_LEN + 2, 9);
        assert_eq!(
            MetadataEnvelope::decode(&bytes, MAX_PUT_ENVELOPE).unwrap_err(),
            DecodeError::invalid_descriptor(detail::descriptor::NONCANONICAL_METADATA)
        );

        // A zero base tag.
        let mut bytes = envelope(ObjectKind::Route, SchemaClass::Put, &[(0x8001, vec![2])]);
        put_u16(&mut bytes, ENVELOPE_HEADER_LEN, 0x8000);
        assert_eq!(
            MetadataEnvelope::decode(&bytes, MAX_PUT_ENVELOPE).unwrap_err(),
            DecodeError::invalid_descriptor(detail::descriptor::NONCANONICAL_METADATA)
        );

        // Nonzero header flags.
        let mut bytes = envelope(ObjectKind::Route, SchemaClass::Put, &[(0x8001, vec![2])]);
        bytes[3] = 1;
        assert_eq!(MetadataEnvelope::decode(&bytes, MAX_PUT_ENVELOPE).unwrap_err(), DecodeError::reserved_bits());
    }

    #[test]
    fn schema_validation_enforces_required_widths_ranges_and_unknown_fields() {
        let schema = Schema::lookup(ObjectKind::Route, SchemaClass::Put).unwrap();

        // Missing required field.
        let bytes = envelope(ObjectKind::Route, SchemaClass::Put, &[]);
        let decoded = MetadataEnvelope::decode(&bytes, MAX_PUT_ENVELOPE).unwrap();
        assert_eq!(schema.validate(&decoded).unwrap_err(), DecodeError::invalid_combination());

        // Out-of-range retention.
        let bytes = envelope(ObjectKind::Route, SchemaClass::Put, &[(0x8001, vec![6])]);
        let decoded = MetadataEnvelope::decode(&bytes, MAX_PUT_ENVELOPE).unwrap();
        assert_eq!(
            schema.validate(&decoded).unwrap_err(),
            DecodeError::invalid_descriptor(detail::descriptor::NONCANONICAL_METADATA)
        );

        // Wrong width.
        let bytes = envelope(ObjectKind::Route, SchemaClass::Put, &[(0x8001, vec![1, 0])]);
        let decoded = MetadataEnvelope::decode(&bytes, MAX_PUT_ENVELOPE).unwrap();
        assert!(schema.validate(&decoded).is_err());

        // An unknown noncritical field is still refused in a mutating request.
        let bytes = envelope(ObjectKind::Route, SchemaClass::Put, &[(0x8001, vec![1]), (0x0077, vec![0])]);
        let decoded = MetadataEnvelope::decode(&bytes, MAX_PUT_ENVELOPE).unwrap();
        assert_eq!(schema.validate(&decoded).unwrap_err(), DecodeError::invalid_combination());

        // Wrong kind and wrong version.
        let bytes = envelope(ObjectKind::Trip, SchemaClass::Put, &[]);
        let decoded = MetadataEnvelope::decode(&bytes, MAX_PUT_ENVELOPE).unwrap();
        assert_eq!(schema.validate(&decoded).unwrap_err(), DecodeError::invalid_combination());

        let mut bytes = envelope(ObjectKind::Route, SchemaClass::Put, &[(0x8001, vec![1])]);
        bytes[2] = 2;
        let decoded = MetadataEnvelope::decode(&bytes, MAX_PUT_ENVELOPE).unwrap();
        assert_eq!(
            schema.validate(&decoded).unwrap_err(),
            DecodeError::unsupported_capability(detail::capability::SCHEMA_VERSION)
        );
    }

    #[test]
    fn a_catalog_projection_skips_unknown_noncritical_and_rejects_unknown_critical() {
        let schema = Schema::lookup(ObjectKind::Trip, SchemaClass::Catalog).unwrap();
        let base: [(u16, Vec<u8>); 2] = [(0x8001, b"Alpine loop".to_vec()), (0x8002, 3u16.to_le_bytes().to_vec())];

        let mut with_unknown = base.to_vec();
        with_unknown.push((0x0055, vec![9, 9]));
        let bytes = envelope(ObjectKind::Trip, SchemaClass::Catalog, &with_unknown);
        let decoded = MetadataEnvelope::decode(&bytes, MAX_CATALOG_ENVELOPE).unwrap();
        schema.validate(&decoded).expect("a well-formed unknown noncritical field is skipped");

        let mut with_critical = base.to_vec();
        with_critical.push((0x8055, vec![9, 9]));
        let bytes = envelope(ObjectKind::Trip, SchemaClass::Catalog, &with_critical);
        let decoded = MetadataEnvelope::decode(&bytes, MAX_CATALOG_ENVELOPE).unwrap();
        assert_eq!(schema.validate(&decoded).unwrap_err(), DecodeError::invalid_combination());
    }

    #[test]
    fn text_rules_reject_control_surrogate_and_noncharacter_scalars() {
        assert!(text_is_clean("Schwarzwald – Süd".as_bytes()));
        assert!(text_is_clean(&[]));
        assert!(!text_is_clean(b"with\0nul"));
        assert!(!text_is_clean(b"tab\there"));
        assert!(!text_is_clean(&[0xC2, 0x85])); // U+0085, a C1 control
        assert!(!text_is_clean(&[0xEF, 0xB7, 0x90])); // U+FDD0
        assert!(!text_is_clean(&[0xEF, 0xBF, 0xBE])); // U+FFFE
        assert!(!text_is_clean(&[0xF4, 0x8F, 0xBF, 0xBF])); // U+10FFFF
        assert!(!text_is_clean(&[0xED, 0xA0, 0x80])); // a surrogate, rejected as invalid UTF-8
        assert!(!text_is_clean(&[0xC0, 0xAF])); // overlong, not shortest form
    }

    #[test]
    fn the_ceiling_is_enforced_before_the_body_is_read() {
        let mut bytes = vec![0u8; ENVELOPE_HEADER_LEN];
        put_u16(&mut bytes, 0, ObjectKind::Route.to_u16());
        bytes[2] = schema_version::CATALOG;
        put_u16(&mut bytes, 4, (MAX_CATALOG_FIELD_BYTES + 1) as u16);
        assert_eq!(
            MetadataEnvelope::decode_prefix(&bytes, MAX_CATALOG_ENVELOPE).unwrap_err(),
            DecodeError::invalid_descriptor(detail::descriptor::NESTED_LENGTH)
        );
    }

    #[test]
    fn the_writer_refuses_to_produce_a_noncanonical_envelope() {
        let mut buffer = [0u8; 64];
        let mut writer = MetadataWriter::new(&mut buffer).unwrap();
        writer.push(0x8002, &[1]).unwrap();
        assert_eq!(writer.push(0x8001, &[1]), Err(WriteError::OutOfOrder));
        assert_eq!(writer.push(0x8002, &[1]), Err(WriteError::OutOfOrder));
        assert_eq!(writer.push(0x0000, &[1]), Err(WriteError::OutOfOrder));
    }
}
