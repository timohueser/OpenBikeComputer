//! Hello, Capabilities, ResourceLimits, subject entries, and frame-limit negotiation
//! (`Device_Object_Protocol_v3.md` §5, §5.1, §5.2 and §14.0).
//!
//! Discovery is paged with *requests*, not with extra response frames: each page is its own Hello
//! under its own RequestId, and the capability revision is the snapshot token that binds the pages
//! together. §5 makes that revision "immutable within one connection generation", which is why
//! `catalogChanged/capabilitySnapshot` is registered, reserved, and never emitted.
//!
//! Two cross-checks in this module are worth naming because they exist to stop a decoder from
//! guessing:
//!
//! - Capabilities byte 54 repeats the ResourceLimits block's own byte 0. §5: "A server MUST emit
//!   equal values; a client that observes a mismatch MUST reject that page and abandon discovery
//!   rather than decode either block, because the two disagree about how to read the second."
//! - A subject's patch schema version "takes exactly two legal values": the registered `128` when
//!   the set-metadata flag is set, and zero when it is clear. "Any other value, in either
//!   direction, is `invalidDescriptor`."

use crate::codec::{bytes16_at, put_bytes, put_u16, put_u32, put_u64, u16_at, u32_at, u64_at};
use crate::error::{reject_nonzero, DecodeError};
use crate::frame::{MAX_CONTROL_FRAME, MAX_STREAM_FRAME, MIN_CONTROL_FRAME, MIN_STREAM_FRAME};
use crate::ids::StoreId;
use crate::metadata::{MAX_CATALOG_ENVELOPE, MAX_PUT_ENVELOPE};
use crate::registry::{policy_flags, schema_version, subject_flags, DraftPartKind, ObjectKind};
use crate::{BufferTooSmall, EncodeResult, WIRE_MAJOR};

/// The Hello request, in bytes.
pub const HELLO_LEN: usize = 12;

/// The Capabilities common prefix, in bytes.
pub const CAPABILITIES_PREFIX_LEN: usize = 56;

/// The ResourceLimits block, in bytes.
pub const RESOURCE_LIMITS_LEN: usize = 56;

/// One subject entry, in bytes.
pub const SUBJECT_ENTRY_LEN: usize = 20;

/// The most subject entries one page carries (§5: `first_subject = page_index * 2`).
pub const SUBJECTS_PER_PAGE: usize = 2;

/// The most subjects a device may advertise across all pages (§1).
pub const MAX_SUBJECTS: usize = 16;

/// The retained terminal result capacity, which §1 and §5 both freeze at exactly this value.
pub const RETAINED_RESULT_CAPACITY: u16 = 64;

/// The default durable upload checkpoint granule (§1).
pub const DEFAULT_CHECKPOINT_GRANULE: u32 = 262_144;

/// Which discovery page a Hello asks for, and a Capabilities returns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum PageKind {
    /// The single ResourceLimits page. Only index zero exists.
    Resources = 0,
    /// A subject-capability page.
    Subjects = 1,
}

impl PageKind {
    /// Decodes a wire `u8`.
    pub const fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(PageKind::Resources),
            1 => Some(PageKind::Subjects),
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
            PageKind::Resources => "resources",
            PageKind::Subjects => "subjects",
        }
    }
}

/// The adapter's link kind, as Capabilities reports it (§5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum LinkKind {
    /// A bonded BLE application identity.
    Ble = 1,
    /// The USB local principal.
    Usb = 2,
    /// The test link kind.
    Test = 3,
}

impl LinkKind {
    /// Every link kind, in wire order.
    pub const ALL: [LinkKind; 3] = [LinkKind::Ble, LinkKind::Usb, LinkKind::Test];

    /// Decodes a wire `u8`.
    pub const fn from_u8(value: u8) -> Option<Self> {
        match value {
            1 => Some(LinkKind::Ble),
            2 => Some(LinkKind::Usb),
            3 => Some(LinkKind::Test),
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
            LinkKind::Ble => "ble",
            LinkKind::Usb => "usb",
            LinkKind::Test => "test",
        }
    }
}

/// Capabilities status-flag bits (§5).
pub mod status_flags {
    /// A store is available and the StoreId field is meaningful.
    pub const STORE_AVAILABLE: u16 = 1 << 0;
    /// The principal is authenticated.
    pub const AUTHENTICATED: u16 = 1 << 1;
    /// A heavy transfer is in progress.
    pub const HEAVY_TRANSFER_BUSY: u16 = 1 << 2;
    /// The device is in developer/unlocked mode.
    pub const DEVELOPER_UNLOCKED: u16 = 1 << 3;
    /// Every defined bit; the rest are zero.
    pub const ALL: u16 = STORE_AVAILABLE | AUTHENTICATED | HEAVY_TRANSFER_BUSY | DEVELOPER_UNLOCKED;
}

/// Every command-flag bit defined in v3.0: bits `0..=16` (§5).
pub const COMMAND_FLAGS_ALL: u32 = (1u32 << 17) - 1;

/// The Hello request (§5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hello {
    /// The lowest wire major the client implements.
    pub minimum_major: u8,
    /// The highest wire major the client implements.
    pub maximum_major: u8,
    /// The largest control frame the client can carry, header included.
    pub client_max_control_frame: u16,
    /// The largest stream frame the client can carry, header included.
    pub client_max_stream_frame: u16,
    /// Client feature flags; zero in v3.0.
    pub client_feature_flags: u32,
    /// Which page this Hello asks for.
    pub page_kind: PageKind,
    /// The zero-based page index.
    pub page_index: u8,
}

impl Hello {
    /// The five fields §5.2 requires to stay byte-identical across a connection's Hellos.
    ///
    /// "A Hello that changes any negotiation field is `invalidDescriptor/invalidCombination`: there
    /// is no renegotiation within a connection."
    pub fn negotiation_fields(&self) -> (u8, u8, u16, u16, u32) {
        (
            self.minimum_major,
            self.maximum_major,
            self.client_max_control_frame,
            self.client_max_stream_frame,
            self.client_feature_flags,
        )
    }

    /// True when `other` may follow this Hello on the same connection — same negotiation fields,
    /// differing only in page kind and index.
    pub fn is_same_negotiation(&self, other: &Hello) -> bool {
        self.negotiation_fields() == other.negotiation_fields()
    }

    /// Decodes exactly [`HELLO_LEN`] bytes.
    pub fn decode(payload: &[u8]) -> crate::Result<Self> {
        DecodeError::exact_len(payload, HELLO_LEN)?;
        let client_feature_flags = u32_at(payload, 6);
        if client_feature_flags != 0 {
            return Err(DecodeError::unsupported_flags());
        }
        let page_kind = PageKind::from_u8(payload[10]).ok_or_else(DecodeError::unknown_enum)?;
        let page_index = payload[11];
        if page_kind == PageKind::Resources && page_index != 0 {
            // §5: "Page kind `0` has only index zero"; a nonzero resource-page index is
            // `invalidDescriptor`.
            return Err(DecodeError::invalid_combination());
        }
        let minimum_major = payload[0];
        let maximum_major = payload[1];
        if minimum_major == 0 || minimum_major > maximum_major {
            return Err(DecodeError::invalid_combination());
        }
        Ok(Hello {
            minimum_major,
            maximum_major,
            client_max_control_frame: u16_at(payload, 2),
            client_max_stream_frame: u16_at(payload, 4),
            client_feature_flags,
            page_kind,
            page_index,
        })
    }

    /// Encodes the request.
    pub fn encode(&self) -> [u8; HELLO_LEN] {
        let mut out = [0u8; HELLO_LEN];
        out[0] = self.minimum_major;
        out[1] = self.maximum_major;
        put_u16(&mut out, 2, self.client_max_control_frame);
        put_u16(&mut out, 4, self.client_max_stream_frame);
        put_u32(&mut out, 6, self.client_feature_flags);
        out[10] = self.page_kind.to_u8();
        out[11] = self.page_index;
        out
    }
}

/// One 20-byte subject entry (§5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SubjectEntry {
    /// Which registry the kind code belongs to.
    pub subject: Subject,
    /// Which operations the device serves for it.
    pub operation_flags: u16,
    /// Transport and singleton policy hints.
    pub policy_flags: u16,
    /// The Put schema version, or zero when the device serves no Put for it.
    pub put_schema_version: u8,
    /// The patch schema version: `128` with the set-metadata flag, zero without it.
    pub patch_schema_version: u8,
    /// The catalog projection schema version, or zero.
    pub catalog_schema_version: u8,
    /// The largest object of this kind the device accepts.
    pub max_length: u64,
}

/// A subject entry's namespace and kind code as one value, so an entry cannot name a draft-part
/// kind in the logical namespace.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Subject {
    /// A logical object kind.
    Logical(ObjectKind),
    /// A draft-part kind.
    DraftPart(DraftPartKind),
}

impl Subject {
    /// The namespace byte.
    pub const fn namespace(self) -> u8 {
        match self {
            Subject::Logical(_) => 1,
            Subject::DraftPart(_) => 2,
        }
    }

    /// The kind code.
    pub const fn kind_code(self) -> u16 {
        match self {
            Subject::Logical(kind) => kind.to_u16(),
            Subject::DraftPart(kind) => kind.to_u16(),
        }
    }

    /// The `(namespace, kind_code)` pair subject pages are ordered by.
    pub const fn sort_key(self) -> (u8, u16) {
        (self.namespace(), self.kind_code())
    }

    /// The name used in fixture JSON.
    pub const fn name(self) -> &'static str {
        match self {
            Subject::Logical(kind) => kind.name(),
            Subject::DraftPart(kind) => kind.name(),
        }
    }
}

impl SubjectEntry {
    /// Decodes exactly [`SUBJECT_ENTRY_LEN`] bytes.
    pub fn decode(bytes: &[u8]) -> crate::Result<Self> {
        DecodeError::exact_len(bytes, SUBJECT_ENTRY_LEN)?;
        if bytes[1] != 0 || bytes[11] != 0 {
            return Err(DecodeError::reserved_bits());
        }
        let kind_code = u16_at(bytes, 2);
        let operation_flags = u16_at(bytes, 4);
        let policy_flags = u16_at(bytes, 6);
        if operation_flags & !subject_flags::ALL != 0 || policy_flags & !policy_flags::ALL != 0 {
            return Err(DecodeError::unsupported_flags());
        }
        let put_schema_version = bytes[8];
        let patch_schema_version = bytes[9];
        let catalog_schema_version = bytes[10];
        let subject = match bytes[0] {
            1 => {
                let kind = ObjectKind::from_u16(kind_code).ok_or_else(DecodeError::unknown_enum)?;
                if operation_flags & !kind.permitted_operation_flags() != 0 {
                    // The registry's lifecycle table is normative: "a device that advertises it is
                    // nonconforming".
                    return Err(DecodeError::invalid_combination());
                }
                Subject::Logical(kind)
            }
            2 => {
                let kind = DraftPartKind::from_u16(kind_code).ok_or_else(DecodeError::unknown_enum)?;
                let permitted = subject_flags::PUT | subject_flags::RESUMABLE_UPLOAD;
                if operation_flags & subject_flags::PUT == 0 || operation_flags & !permitted != 0 {
                    // §5: draft-part subjects "advertise put and optional resumable upload only".
                    return Err(DecodeError::invalid_combination());
                }
                if put_schema_version != 0 || patch_schema_version != 0 || catalog_schema_version != 0 {
                    // "all three schema versions are zero because StartDraftPart has no metadata
                    // envelope or catalog".
                    return Err(DecodeError::invalid_combination());
                }
                Subject::DraftPart(kind)
            }
            _ => return Err(DecodeError::unknown_enum()),
        };
        if matches!(subject, Subject::Logical(_)) {
            let expected_patch =
                if operation_flags & subject_flags::SET_METADATA != 0 { schema_version::PATCH } else { 0 };
            if patch_schema_version != expected_patch {
                return Err(DecodeError::invalid_descriptor(crate::error::detail::descriptor::INVALID_COMBINATION));
            }
            // "a device advertises the constant or advertises zero for an operation it does not
            // support" — so each of the other two bytes has exactly two legal values as well.
            if put_schema_version != 0 && put_schema_version != schema_version::PUT {
                return Err(DecodeError::invalid_combination());
            }
            if catalog_schema_version != 0 && catalog_schema_version != schema_version::CATALOG {
                return Err(DecodeError::invalid_combination());
            }
        }
        Ok(SubjectEntry {
            subject,
            operation_flags,
            policy_flags,
            put_schema_version,
            patch_schema_version,
            catalog_schema_version,
            max_length: u64_at(bytes, 12),
        })
    }

    /// Encodes the entry.
    pub fn encode(&self) -> [u8; SUBJECT_ENTRY_LEN] {
        let mut out = [0u8; SUBJECT_ENTRY_LEN];
        out[0] = self.subject.namespace();
        put_u16(&mut out, 2, self.subject.kind_code());
        put_u16(&mut out, 4, self.operation_flags);
        put_u16(&mut out, 6, self.policy_flags);
        out[8] = self.put_schema_version;
        out[9] = self.patch_schema_version;
        out[10] = self.catalog_schema_version;
        put_u64(&mut out, 12, self.max_length);
        out
    }
}

/// The 56-byte ResourceLimits block (§5.1).
///
/// Every fixed value here is a **mirror** of `OBC2_Storage_Format.md` §2, which is the single
/// authority: "Where the two documents disagree, the storage contract wins and this mirror is
/// corrected." The decoder therefore enforces the block's *structure* — codec version, block
/// length, flags, reserved runs — and reports the capacities rather than refusing a device whose
/// storage contract has legitimately re-frozen one. [`REFERENCE`](Self::REFERENCE) is the frozen
/// v3.0 mirror, and [`matches_reference`](Self::matches_reference) is how a client checks it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResourceLimits {
    /// Logical catalog heads across all kinds.
    pub logical_catalog_heads: u16,
    /// Normal active claimed operations.
    pub normal_claims: u8,
    /// Resumable upload/work slots.
    pub upload_work_slots: u8,
    /// Active draft parents.
    pub draft_parents: u8,
    /// Sealed/streaming draft parts of the one active parent.
    pub draft_parts: u8,
    /// Children referenced by one manifest.
    pub manifest_children: u8,
    /// Simultaneously mounted map data files.
    pub mounted_files: u8,
    /// Live reader leases.
    pub reader_leases: u8,
    /// Retained previous generations.
    pub retained_generations: u8,
    /// Retained terminal results.
    pub retained_results: u16,
    /// Inactive-work horizon in later terminal commits.
    pub inactive_work_horizon: u16,
    /// Maximum single-generation length.
    pub max_generation_length: u64,
    /// Currently available reservation bytes — the one dynamic field.
    pub available_reservation_bytes: u64,
    /// Route catalog heads.
    pub route_heads: u16,
    /// Trip catalog heads.
    pub trip_heads: u16,
    /// Ride catalog heads.
    pub ride_heads: u16,
    /// Weather catalog heads.
    pub weather_heads: u16,
    /// Volume-manifest catalog heads.
    pub volume_manifest_heads: u16,
    /// Update-package catalog heads.
    pub update_package_heads: u16,
    /// Simultaneously attached heavy stream sessions.
    pub heavy_stream_sessions: u8,
    /// Reserved maintenance/cancellation/recovery claims.
    pub maintenance_claims: u8,
    /// Active-or-recoverable ride slots.
    pub ride_slots: u8,
}

impl ResourceLimits {
    /// The ResourceLimits codec version this crate implements.
    pub const CODEC_VERSION: u8 = 1;

    /// The v3.0 mirror of `OBC2_Storage_Format.md` §2, with the dynamic field zeroed.
    pub const REFERENCE: ResourceLimits = ResourceLimits {
        logical_catalog_heads: 256,
        normal_claims: 8,
        upload_work_slots: 4,
        draft_parents: 1,
        draft_parts: 32,
        manifest_children: 32,
        mounted_files: 11,
        reader_leases: 4,
        retained_generations: 8,
        retained_results: 64,
        inactive_work_horizon: 256,
        max_generation_length: 0x0000_0000_FFFF_FFFF,
        available_reservation_bytes: 0,
        route_heads: 64,
        trip_heads: 16,
        ride_heads: 128,
        weather_heads: 1,
        volume_manifest_heads: 8,
        update_package_heads: 8,
        heavy_stream_sessions: 1,
        maintenance_claims: 1,
        ride_slots: 1,
    };

    /// True when every fixed capacity equals the frozen mirror. The dynamic reservation-byte
    /// snapshot is deliberately excluded.
    pub fn matches_reference(&self) -> bool {
        ResourceLimits { available_reservation_bytes: 0, ..*self } == ResourceLimits::REFERENCE
    }

    /// Decodes exactly [`RESOURCE_LIMITS_LEN`] bytes.
    pub fn decode(bytes: &[u8]) -> crate::Result<Self> {
        DecodeError::exact_len(bytes, RESOURCE_LIMITS_LEN)?;
        if bytes[0] != Self::CODEC_VERSION || bytes[1] != RESOURCE_LIMITS_LEN as u8 {
            return Err(DecodeError::invalid_combination());
        }
        reject_nonzero(bytes, 2, 2)?;
        // Byte 18 was the journal capacity; §5.1 reserves it and encodes it zero.
        reject_nonzero(bytes, 18, 2)?;
        reject_nonzero(bytes, 51, 5)?;
        Ok(ResourceLimits {
            logical_catalog_heads: u16_at(bytes, 4),
            normal_claims: bytes[6],
            upload_work_slots: bytes[7],
            draft_parents: bytes[8],
            draft_parts: bytes[9],
            manifest_children: bytes[10],
            mounted_files: bytes[11],
            reader_leases: bytes[12],
            retained_generations: bytes[13],
            retained_results: u16_at(bytes, 14),
            inactive_work_horizon: u16_at(bytes, 16),
            max_generation_length: u64_at(bytes, 20),
            available_reservation_bytes: u64_at(bytes, 28),
            route_heads: u16_at(bytes, 36),
            trip_heads: u16_at(bytes, 38),
            ride_heads: u16_at(bytes, 40),
            weather_heads: u16_at(bytes, 42),
            volume_manifest_heads: u16_at(bytes, 44),
            update_package_heads: u16_at(bytes, 46),
            heavy_stream_sessions: bytes[48],
            maintenance_claims: bytes[49],
            ride_slots: bytes[50],
        })
    }

    /// Encodes the block.
    pub fn encode(&self) -> [u8; RESOURCE_LIMITS_LEN] {
        let mut out = [0u8; RESOURCE_LIMITS_LEN];
        out[0] = Self::CODEC_VERSION;
        out[1] = RESOURCE_LIMITS_LEN as u8;
        put_u16(&mut out, 4, self.logical_catalog_heads);
        out[6] = self.normal_claims;
        out[7] = self.upload_work_slots;
        out[8] = self.draft_parents;
        out[9] = self.draft_parts;
        out[10] = self.manifest_children;
        out[11] = self.mounted_files;
        out[12] = self.reader_leases;
        out[13] = self.retained_generations;
        put_u16(&mut out, 14, self.retained_results);
        put_u16(&mut out, 16, self.inactive_work_horizon);
        put_u64(&mut out, 20, self.max_generation_length);
        put_u64(&mut out, 28, self.available_reservation_bytes);
        put_u16(&mut out, 36, self.route_heads);
        put_u16(&mut out, 38, self.trip_heads);
        put_u16(&mut out, 40, self.ride_heads);
        put_u16(&mut out, 42, self.weather_heads);
        put_u16(&mut out, 44, self.volume_manifest_heads);
        put_u16(&mut out, 46, self.update_package_heads);
        out[48] = self.heavy_stream_sessions;
        out[49] = self.maintenance_claims;
        out[50] = self.ride_slots;
        out
    }
}

/// The body a Capabilities page carries after its common prefix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapabilityPage {
    /// Page kind `0`: the ResourceLimits block.
    Resources(ResourceLimits),
    /// Page kind `1`: up to two complete subject entries.
    Subjects {
        /// The entries, in ascending `(namespace, kind_code)` order.
        entries: [SubjectEntry; SUBJECTS_PER_PAGE],
        /// How many of them are meaningful: `0`, `1`, or `2`.
        count: u8,
    },
}

impl CapabilityPage {
    /// The page kind byte.
    pub const fn kind(&self) -> PageKind {
        match self {
            CapabilityPage::Resources(_) => PageKind::Resources,
            CapabilityPage::Subjects { .. } => PageKind::Subjects,
        }
    }

    /// The meaningful entries of a subject page, or an empty slice for the resource page.
    pub fn entries(&self) -> &[SubjectEntry] {
        match self {
            CapabilityPage::Resources(_) => &[],
            CapabilityPage::Subjects { entries, count } => &entries[..usize::from(*count)],
        }
    }
}

/// A Capabilities response: the 56-byte common prefix and one page body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capabilities {
    /// The selected wire major.
    pub selected_major: u8,
    /// The OBC2 storage format version.
    pub storage_format_version: u8,
    /// Store-available / authenticated / heavy-transfer-busy / developer bits.
    pub status_flags: u16,
    /// The StoreId, zero only when store-available is clear.
    pub store_id: StoreId,
    /// The negotiated maximum control frame.
    pub negotiated_control_frame: u16,
    /// The negotiated maximum stream frame.
    pub negotiated_stream_frame: u16,
    /// The durable upload checkpoint granule.
    pub checkpoint_granule: u32,
    /// The retained result capacity — exactly [`RETAINED_RESULT_CAPACITY`].
    pub retained_result_capacity: u16,
    /// The metadata envelope limit — 128.
    pub metadata_envelope_limit: u16,
    /// The catalog metadata limit — 96.
    pub catalog_metadata_limit: u16,
    /// The protocol minimum control frame — 192.
    pub protocol_min_control_frame: u16,
    /// The protocol minimum stream frame — 64.
    pub protocol_min_stream_frame: u16,
    /// This connection's link kind.
    pub link_kind: LinkKind,
    /// Whether the principal is authenticated.
    pub authenticated: bool,
    /// The snapshot token that binds one discovery's pages together.
    pub capability_revision: u32,
    /// Which of the seventeen gated operations the device serves.
    pub command_flags: u32,
    /// How many subjects the device advertises in total.
    pub total_subject_count: u16,
    /// The page index this response answers.
    pub page_index: u8,
    /// How many pages of this kind exist.
    pub total_pages: u8,
    /// The device's wire minor within the selected major.
    pub device_wire_minor: u8,
    /// The page body.
    pub page: CapabilityPage,
}

impl Capabilities {
    /// The exact encoded payload length.
    pub fn encoded_len(&self) -> usize {
        CAPABILITIES_PREFIX_LEN
            + match &self.page {
                CapabilityPage::Resources(_) => RESOURCE_LIMITS_LEN,
                CapabilityPage::Subjects { count, .. } => usize::from(*count) * SUBJECT_ENTRY_LEN,
            }
    }

    /// Decodes a Capabilities payload.
    pub fn decode(payload: &[u8]) -> crate::Result<Self> {
        DecodeError::min_len(payload, CAPABILITIES_PREFIX_LEN)?;
        if payload[0] != WIRE_MAJOR {
            return Err(DecodeError::incompatible_version(crate::error::detail::version::UNSUPPORTED_MAJOR));
        }
        let status_flags = u16_at(payload, 2);
        if status_flags & !status_flags::ALL != 0 {
            return Err(DecodeError::unsupported_flags());
        }
        let command_flags = u32_at(payload, 44);
        if command_flags & !COMMAND_FLAGS_ALL != 0 {
            return Err(DecodeError::unsupported_flags());
        }
        let store_id = StoreId::new(bytes16_at(payload, 4));
        if status_flags & status_flags::STORE_AVAILABLE == 0 && !store_id.is_zero() {
            // §5: "StoreId, zero only when store-available is clear" — the inactive alternative is
            // encoded as zero, and §1 rejects a nonzero encoding of one.
            return Err(DecodeError::reserved_bits());
        }
        if u16_at(payload, 28) != RETAINED_RESULT_CAPACITY
            || u16_at(payload, 30) != MAX_PUT_ENVELOPE as u16
            || u16_at(payload, 32) != MAX_CATALOG_ENVELOPE as u16
            || u16_at(payload, 34) != MIN_CONTROL_FRAME as u16
            || u16_at(payload, 36) != MIN_STREAM_FRAME as u16
        {
            return Err(DecodeError::invalid_combination());
        }
        let link_kind = LinkKind::from_u8(payload[38]).ok_or_else(DecodeError::unknown_enum)?;
        let authenticated = match payload[39] {
            0 => false,
            1 => true,
            _ => return Err(DecodeError::unknown_enum()),
        };
        if authenticated != (status_flags & status_flags::AUTHENTICATED != 0) {
            return Err(DecodeError::invalid_combination());
        }
        let total_subject_count = u16_at(payload, 48);
        if usize::from(total_subject_count) > MAX_SUBJECTS {
            return Err(DecodeError::invalid_combination());
        }
        let returned_page_kind = PageKind::from_u8(payload[50]).ok_or_else(DecodeError::unknown_enum)?;
        let page_index = payload[51];
        let returned_subject_count = payload[52];
        let total_pages = payload[53];
        if payload[54] != ResourceLimits::CODEC_VERSION {
            return Err(DecodeError::invalid_combination());
        }
        let body = &payload[CAPABILITIES_PREFIX_LEN..];
        let page = match returned_page_kind {
            PageKind::Resources => {
                if page_index != 0 || returned_subject_count != 0 || total_pages != 1 {
                    return Err(DecodeError::invalid_combination());
                }
                if body.len() < RESOURCE_LIMITS_LEN {
                    return Err(DecodeError::truncated());
                }
                if body.len() > RESOURCE_LIMITS_LEN {
                    return Err(DecodeError::trailing_bytes());
                }
                if body[0] != payload[54] {
                    // §5: the two disagree about how to read the second — reject the page.
                    return Err(DecodeError::invalid_combination());
                }
                CapabilityPage::Resources(ResourceLimits::decode(body)?)
            }
            PageKind::Subjects => {
                if usize::from(returned_subject_count) > SUBJECTS_PER_PAGE {
                    return Err(DecodeError::invalid_combination());
                }
                let expected_pages = total_subject_count.div_ceil(SUBJECTS_PER_PAGE as u16) as u8;
                if total_pages != expected_pages {
                    return Err(DecodeError::invalid_combination());
                }
                let first_subject = u16::from(page_index) * SUBJECTS_PER_PAGE as u16;
                if total_subject_count == 0 {
                    if page_index != 0 || returned_subject_count != 0 {
                        return Err(DecodeError::invalid_combination());
                    }
                } else {
                    if first_subject >= total_subject_count {
                        return Err(DecodeError::invalid_combination());
                    }
                    let remaining = total_subject_count - first_subject;
                    let expected = remaining.min(SUBJECTS_PER_PAGE as u16) as u8;
                    if returned_subject_count != expected {
                        return Err(DecodeError::invalid_combination());
                    }
                }
                let needed = usize::from(returned_subject_count) * SUBJECT_ENTRY_LEN;
                if body.len() < needed {
                    return Err(DecodeError::truncated());
                }
                if body.len() > needed {
                    return Err(DecodeError::trailing_bytes());
                }
                let mut entries = [SubjectEntry {
                    subject: Subject::Logical(ObjectKind::Route),
                    operation_flags: 0,
                    policy_flags: 0,
                    put_schema_version: 0,
                    patch_schema_version: 0,
                    catalog_schema_version: 0,
                    max_length: 0,
                }; SUBJECTS_PER_PAGE];
                let mut previous: Option<(u8, u16)> = None;
                for (index, slot) in entries.iter_mut().enumerate().take(usize::from(returned_subject_count)) {
                    let start = index * SUBJECT_ENTRY_LEN;
                    let entry = SubjectEntry::decode(&body[start..start + SUBJECT_ENTRY_LEN])?;
                    let key = entry.subject.sort_key();
                    if let Some(previous) = previous {
                        if key <= previous {
                            return Err(DecodeError::invalid_combination());
                        }
                    }
                    previous = Some(key);
                    *slot = entry;
                }
                CapabilityPage::Subjects { entries, count: returned_subject_count }
            }
        };
        Ok(Capabilities {
            selected_major: payload[0],
            storage_format_version: payload[1],
            status_flags,
            store_id,
            negotiated_control_frame: u16_at(payload, 20),
            negotiated_stream_frame: u16_at(payload, 22),
            checkpoint_granule: u32_at(payload, 24),
            retained_result_capacity: u16_at(payload, 28),
            metadata_envelope_limit: u16_at(payload, 30),
            catalog_metadata_limit: u16_at(payload, 32),
            protocol_min_control_frame: u16_at(payload, 34),
            protocol_min_stream_frame: u16_at(payload, 36),
            link_kind,
            authenticated,
            capability_revision: u32_at(payload, 40),
            command_flags,
            total_subject_count,
            page_index,
            total_pages,
            device_wire_minor: payload[55],
            page,
        })
    }

    /// Encodes the payload into `out`, returning its exact length.
    pub fn encode_into(&self, out: &mut [u8]) -> EncodeResult {
        let needed = self.encoded_len();
        if out.len() < needed {
            return Err(BufferTooSmall { needed, available: out.len() });
        }
        let out = &mut out[..needed];
        out.fill(0);
        out[0] = self.selected_major;
        out[1] = self.storage_format_version;
        put_u16(out, 2, self.status_flags);
        put_bytes(out, 4, self.store_id.as_bytes());
        put_u16(out, 20, self.negotiated_control_frame);
        put_u16(out, 22, self.negotiated_stream_frame);
        put_u32(out, 24, self.checkpoint_granule);
        put_u16(out, 28, self.retained_result_capacity);
        put_u16(out, 30, self.metadata_envelope_limit);
        put_u16(out, 32, self.catalog_metadata_limit);
        put_u16(out, 34, self.protocol_min_control_frame);
        put_u16(out, 36, self.protocol_min_stream_frame);
        out[38] = self.link_kind.to_u8();
        out[39] = u8::from(self.authenticated);
        put_u32(out, 40, self.capability_revision);
        put_u32(out, 44, self.command_flags);
        put_u16(out, 48, self.total_subject_count);
        out[50] = self.page.kind().to_u8();
        out[51] = self.page_index;
        out[52] = match &self.page {
            CapabilityPage::Resources(_) => 0,
            CapabilityPage::Subjects { count, .. } => *count,
        };
        out[53] = self.total_pages;
        out[54] = ResourceLimits::CODEC_VERSION;
        out[55] = self.device_wire_minor;
        match &self.page {
            CapabilityPage::Resources(limits) => {
                put_bytes(out, CAPABILITIES_PREFIX_LEN, &limits.encode());
            }
            CapabilityPage::Subjects { entries, count } => {
                for (index, entry) in entries.iter().take(usize::from(*count)).enumerate() {
                    put_bytes(out, CAPABILITIES_PREFIX_LEN + index * SUBJECT_ENTRY_LEN, &entry.encode());
                }
            }
        }
        Ok(needed)
    }
}

/// Frame-limit derivation and negotiation (§1 and §14.0).
///
/// These are pure functions over advertised numbers, which is exactly the part of §14.0 that is
/// common policy rather than a physical link fact. What the transport ceiling *is* differs per
/// binding — `ATT_MTU - 3` on BLE, the negotiated record maximum on USB — and only the BLE
/// derivation is spelled out numerically enough to encode here.
pub mod negotiation {
    use super::*;

    /// The outcome of trying to agree a frame limit.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Limit {
        /// The negotiated limit, header included.
        Negotiated(u16),
        /// Below the protocol minimum: answer Hello with the given resource-limit detail and
        /// guidance retry-only-after-user-action, and admit nothing on this connection.
        BelowProtocolMinimum,
        /// So small that even a 64-byte refusal is undeliverable; the adapter disconnects rather
        /// than truncating an error.
        Undeliverable,
    }

    /// The BLE control-record transport ceiling for an ATT MTU: one Write Request or indication
    /// value carries at most `ATT_MTU - 3` bytes (§14.0).
    pub const fn ble_control_ceiling(att_mtu: u16) -> u16 {
        att_mtu.saturating_sub(3)
    }

    /// Negotiates a control frame limit from the two advertised maxima and the transport ceiling.
    pub fn control_frame(client_max: u16, device_max: u16, transport_ceiling: u16) -> Limit {
        let smaller = client_max.min(device_max).min(transport_ceiling).min(MAX_CONTROL_FRAME as u16);
        if usize::from(smaller) >= MIN_CONTROL_FRAME {
            Limit::Negotiated(smaller)
        } else if usize::from(transport_ceiling) >= crate::frame::HEADER_LEN + crate::error::ERROR_BODY_PREFIX_LEN {
            Limit::BelowProtocolMinimum
        } else {
            Limit::Undeliverable
        }
    }

    /// The same for the stream channel, against the 64-byte stream floor.
    ///
    /// On BLE the effective limit is `min(negotiated stream maximum, CoC SDU)`, fixed at CoC
    /// establishment and constant for the channel's lifetime — pass the SDU as the ceiling.
    pub fn stream_frame(client_max: u16, device_max: u16, transport_ceiling: u16) -> Limit {
        let smaller = client_max.min(device_max).min(transport_ceiling).min(MAX_STREAM_FRAME as u16);
        if usize::from(smaller) >= MIN_STREAM_FRAME {
            Limit::Negotiated(smaller)
        } else {
            Limit::BelowProtocolMinimum
        }
    }
}

#[cfg(test)]
mod tests {
    use super::negotiation::{ble_control_ceiling, control_frame, stream_frame, Limit};
    use super::*;
    use crate::error::detail;
    use crate::STORAGE_FORMAT_VERSION;
    use std::vec;

    fn subject(kind: ObjectKind, operation_flags: u16) -> SubjectEntry {
        SubjectEntry {
            subject: Subject::Logical(kind),
            operation_flags,
            policy_flags: 0,
            put_schema_version: if operation_flags & subject_flags::PUT != 0 { schema_version::PUT } else { 0 },
            patch_schema_version: if operation_flags & subject_flags::SET_METADATA != 0 {
                schema_version::PATCH
            } else {
                0
            },
            catalog_schema_version: schema_version::CATALOG,
            max_length: 8 * 1024 * 1024,
        }
    }

    fn capabilities(page: CapabilityPage, total_subject_count: u16, page_index: u8, total_pages: u8) -> Capabilities {
        Capabilities {
            selected_major: WIRE_MAJOR,
            storage_format_version: STORAGE_FORMAT_VERSION,
            status_flags: status_flags::STORE_AVAILABLE | status_flags::AUTHENTICATED,
            store_id: StoreId::new([0xA5; 16]),
            negotiated_control_frame: 244,
            negotiated_stream_frame: 1024,
            checkpoint_granule: DEFAULT_CHECKPOINT_GRANULE,
            retained_result_capacity: RETAINED_RESULT_CAPACITY,
            metadata_envelope_limit: MAX_PUT_ENVELOPE as u16,
            catalog_metadata_limit: MAX_CATALOG_ENVELOPE as u16,
            protocol_min_control_frame: MIN_CONTROL_FRAME as u16,
            protocol_min_stream_frame: MIN_STREAM_FRAME as u16,
            link_kind: LinkKind::Usb,
            authenticated: true,
            capability_revision: 9,
            command_flags: COMMAND_FLAGS_ALL,
            total_subject_count,
            page_index,
            total_pages,
            device_wire_minor: 0,
            page,
        }
    }

    #[test]
    fn hello_is_twelve_bytes_and_round_trips() {
        let hello = Hello {
            minimum_major: 3,
            maximum_major: 3,
            client_max_control_frame: 244,
            client_max_stream_frame: 1024,
            client_feature_flags: 0,
            page_kind: PageKind::Subjects,
            page_index: 2,
        };
        let bytes = hello.encode();
        assert_eq!(bytes.len(), HELLO_LEN);
        assert_eq!(Hello::decode(&bytes).unwrap(), hello);
    }

    #[test]
    fn hello_rejects_feature_flags_a_bad_page_and_a_nonzero_resource_index() {
        let mut bytes = Hello {
            minimum_major: 3,
            maximum_major: 3,
            client_max_control_frame: 244,
            client_max_stream_frame: 1024,
            client_feature_flags: 0,
            page_kind: PageKind::Resources,
            page_index: 0,
        }
        .encode();
        bytes[11] = 1;
        assert_eq!(Hello::decode(&bytes).unwrap_err(), DecodeError::invalid_combination());
        bytes[11] = 0;
        bytes[10] = 2;
        assert_eq!(Hello::decode(&bytes).unwrap_err(), DecodeError::unknown_enum());
        bytes[10] = 0;
        put_u32(&mut bytes, 6, 1);
        assert_eq!(Hello::decode(&bytes).unwrap_err(), DecodeError::unsupported_flags());
    }

    #[test]
    fn resource_page_is_112_payload_bytes_and_mirrors_the_storage_contract() {
        let page = capabilities(CapabilityPage::Resources(ResourceLimits::REFERENCE), 6, 0, 1);
        let mut out = [0u8; 256];
        let len = page.encode_into(&mut out).unwrap();
        assert_eq!(len, 112);
        assert!(len + crate::frame::HEADER_LEN <= MIN_CONTROL_FRAME);
        let decoded = Capabilities::decode(&out[..len]).unwrap();
        assert_eq!(decoded, page);
        let CapabilityPage::Resources(limits) = decoded.page else { panic!("wrong page") };
        assert!(limits.matches_reference());
        assert_eq!(limits.retained_results, RETAINED_RESULT_CAPACITY);
        // Byte 54 repeats the block's own byte 0.
        assert_eq!(out[54], out[CAPABILITIES_PREFIX_LEN]);
    }

    #[test]
    fn a_codec_version_mismatch_between_byte_54_and_the_block_rejects_the_page() {
        let page = capabilities(CapabilityPage::Resources(ResourceLimits::REFERENCE), 6, 0, 1);
        let mut out = [0u8; 256];
        let len = page.encode_into(&mut out).unwrap();
        out[CAPABILITIES_PREFIX_LEN] = 2;
        assert_eq!(Capabilities::decode(&out[..len]).unwrap_err(), DecodeError::invalid_combination());
    }

    #[test]
    fn subject_page_holds_two_entries_and_orders_them() {
        let entries = [
            subject(ObjectKind::Route, subject_flags::PUT | subject_flags::GET | subject_flags::SET_METADATA),
            subject(ObjectKind::Trip, subject_flags::PUT | subject_flags::GET),
        ];
        let page = capabilities(CapabilityPage::Subjects { entries, count: 2 }, 4, 0, 2);
        let mut out = [0u8; 256];
        let len = page.encode_into(&mut out).unwrap();
        assert_eq!(len, 96);
        assert_eq!(Capabilities::decode(&out[..len]).unwrap(), page);

        // Descending order is rejected.
        let swapped = [entries[1], entries[0]];
        let page = capabilities(CapabilityPage::Subjects { entries: swapped, count: 2 }, 4, 0, 2);
        let len = page.encode_into(&mut out).unwrap();
        assert_eq!(Capabilities::decode(&out[..len]).unwrap_err(), DecodeError::invalid_combination());
    }

    #[test]
    fn a_zero_subject_device_answers_page_zero_with_count_zero() {
        let entries = [subject(ObjectKind::Route, subject_flags::GET); 2];
        let page = capabilities(CapabilityPage::Subjects { entries, count: 0 }, 0, 0, 0);
        let mut out = [0u8; 256];
        let len = page.encode_into(&mut out).unwrap();
        assert_eq!(len, CAPABILITIES_PREFIX_LEN);
        let decoded = Capabilities::decode(&out[..len]).unwrap();
        assert_eq!(decoded.total_pages, 0);
        assert!(decoded.page.entries().is_empty());

        // Only an index above zero is invalid in that case.
        out[51] = 1;
        assert_eq!(Capabilities::decode(&out[..len]).unwrap_err(), DecodeError::invalid_combination());
    }

    #[test]
    fn subject_entries_enforce_the_registry_lifecycle_and_the_patch_byte() {
        // A ride advertising put is nonconforming.
        let mut bytes = subject(ObjectKind::Ride, subject_flags::GET).encode();
        put_u16(&mut bytes, 4, subject_flags::PUT | subject_flags::GET);
        assert_eq!(SubjectEntry::decode(&bytes).unwrap_err(), DecodeError::invalid_combination());

        // A nonzero patch version without the set-metadata flag.
        let mut bytes = subject(ObjectKind::Trip, subject_flags::PUT | subject_flags::GET).encode();
        bytes[9] = schema_version::PATCH;
        assert_eq!(SubjectEntry::decode(&bytes).unwrap_err(), DecodeError::invalid_combination());

        // A patch version other than 128 with the flag set.
        let mut bytes = subject(ObjectKind::Route, subject_flags::GET | subject_flags::SET_METADATA).encode();
        bytes[9] = 1;
        assert_eq!(SubjectEntry::decode(&bytes).unwrap_err(), DecodeError::invalid_combination());

        // A draft-part subject may advertise only put and resumable upload, with zero schemas.
        let draft = SubjectEntry {
            subject: Subject::DraftPart(DraftPartKind::MapShard),
            operation_flags: subject_flags::PUT | subject_flags::RESUMABLE_UPLOAD,
            policy_flags: policy_flags::USB_RECOMMENDED,
            put_schema_version: 0,
            patch_schema_version: 0,
            catalog_schema_version: 0,
            max_length: 64 * 1024 * 1024,
        };
        let bytes = draft.encode();
        assert_eq!(SubjectEntry::decode(&bytes).unwrap(), draft);
        let mut with_get = bytes;
        put_u16(&mut with_get, 4, subject_flags::PUT | subject_flags::GET);
        assert_eq!(SubjectEntry::decode(&with_get).unwrap_err(), DecodeError::invalid_combination());
        let mut with_schema = bytes;
        with_schema[10] = schema_version::CATALOG;
        assert_eq!(SubjectEntry::decode(&with_schema).unwrap_err(), DecodeError::invalid_combination());
    }

    #[test]
    fn store_id_must_be_zero_when_no_store_is_available() {
        let mut page = capabilities(CapabilityPage::Resources(ResourceLimits::REFERENCE), 0, 0, 1);
        page.status_flags = status_flags::AUTHENTICATED;
        let mut out = [0u8; 256];
        let len = page.encode_into(&mut out).unwrap();
        assert_eq!(Capabilities::decode(&out[..len]).unwrap_err(), DecodeError::reserved_bits());
        page.store_id = StoreId::ZERO;
        let len = page.encode_into(&mut out).unwrap();
        assert!(Capabilities::decode(&out[..len]).is_ok());
    }

    #[test]
    fn ble_frame_limit_derivation_matches_the_pinned_cases() {
        // §14.0 and the vectors contract pin these four.
        assert_eq!(ble_control_ceiling(247), 244);
        assert_eq!(control_frame(244, 244, ble_control_ceiling(247)), Limit::Negotiated(244));
        assert_eq!(control_frame(512, 512, ble_control_ceiling(195)), Limit::Negotiated(192));
        assert_eq!(control_frame(512, 512, ble_control_ceiling(194)), Limit::BelowProtocolMinimum);
        assert_eq!(control_frame(512, 512, ble_control_ceiling(66)), Limit::Undeliverable);
        // The stream side, against the 64-byte floor and a smaller CoC SDU.
        assert_eq!(stream_frame(1024, 4096, 512), Limit::Negotiated(512));
        assert_eq!(stream_frame(1024, 4096, 63), Limit::BelowProtocolMinimum);
        // The refusal that is itself undeliverable needs a 16-byte header plus a 48-byte body.
        assert_eq!(crate::frame::HEADER_LEN + crate::error::ERROR_BODY_PREFIX_LEN, 64);
    }

    #[test]
    fn a_capabilities_page_with_a_wrong_pinned_constant_is_rejected() {
        let page = capabilities(CapabilityPage::Resources(ResourceLimits::REFERENCE), 0, 0, 1);
        let mut out = vec![0u8; 256];
        let len = page.encode_into(&mut out).unwrap();
        for offset in [28usize, 30, 32, 34, 36] {
            let mut broken = out.clone();
            put_u16(&mut broken, offset, 7);
            assert_eq!(
                Capabilities::decode(&broken[..len]).unwrap_err(),
                DecodeError::invalid_combination(),
                "offset {offset} must be pinned"
            );
        }
        let mut broken = out.clone();
        broken[38] = 4;
        assert_eq!(Capabilities::decode(&broken[..len]).unwrap_err(), DecodeError::unknown_enum());
        let mut broken = out.clone();
        put_u32(&mut broken, 44, 1 << 17);
        assert_eq!(Capabilities::decode(&broken[..len]).unwrap_err(), DecodeError::unsupported_flags());
        let mut broken = out;
        broken[4 + 16] = 0;
        let _ = broken;
        assert_eq!(detail::capability::OPCODE, 1);
    }
}
