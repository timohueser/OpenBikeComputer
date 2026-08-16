//! The 16-byte control frame header and the opcode registry
//! (`Device_Object_Protocol_v3.md` §2 and §4).
//!
//! Every control transport record contains exactly one frame. [`ControlFrame::decode`] establishes
//! that fact or refuses it, and the refusal is already split the way §2 splits it:
//!
//! - `invalidFrame` — the record cannot be established as one complete frame: bad record length,
//!   truncation, trailing bytes, bad magic, a payload length that disagrees, or a frame outside the
//!   negotiated bounds. Nothing about the payload has been looked at.
//! - `invalidDescriptor` — the frame is complete and a header field is illegal.
//! - `incompatibleVersion` — the wire version parses and cannot be served. §2 is explicit that this
//!   is "not either malformed category".
//!
//! ## RequestId zero is not answerable
//!
//! §2: "A receiver therefore treats a zero-RequestId frame exactly like untrusted framing: it emits
//! no response and closes that control record stream. `invalidDescriptor/zeroRequestId` is the
//! recorded and logged reason for that close; it is never transmitted." The decoder returns exactly
//! that error, and [`DecodeError::is_unanswerable`] is how a caller recognises the one refusal it
//! must not put on the wire.

use crate::codec::{put_bytes, put_u16, put_u32, u16_at, u32_at};
use crate::error::{detail, DecodeError};
use crate::ids::RequestId;
use crate::{BufferTooSmall, EncodeResult, WIRE_MAJOR, WIRE_MINOR};

/// The control header, in bytes.
pub const HEADER_LEN: usize = 16;

/// ASCII `OBCP`.
pub const MAGIC: [u8; 4] = *b"OBCP";

/// The protocol minimum negotiated control frame, header included (§1).
pub const MIN_CONTROL_FRAME: usize = 192;

/// The hard maximum control frame, header included (§1).
pub const MAX_CONTROL_FRAME: usize = 512;

/// The largest control payload: [`MAX_CONTROL_FRAME`] less the header.
pub const MAX_CONTROL_PAYLOAD: usize = MAX_CONTROL_FRAME - HEADER_LEN;

/// The largest payload that fits the 192-byte floor. §1 derives the floor from this number, not the
/// other way round.
pub const MIN_FRAME_PAYLOAD: usize = MIN_CONTROL_FRAME - HEADER_LEN;

/// The protocol minimum negotiated stream frame, header included (§1).
pub const MIN_STREAM_FRAME: usize = 64;

/// The hard maximum stream frame, header included (§1).
pub const MAX_STREAM_FRAME: usize = 4096;

/// The operation registry of §4.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u16)]
pub enum Opcode {
    /// Hello / Capabilities.
    Hello = 0x0001,
    /// StartUpload / UploadAccepted.
    StartUpload = 0x0100,
    /// CheckpointUpload.
    CheckpointUpload = 0x0101,
    /// FinishUpload.
    FinishUpload = 0x0102,
    /// StartDownload / DownloadAccepted.
    StartDownload = 0x0110,
    /// FinishDownload.
    FinishDownload = 0x0111,
    /// AbortSession.
    AbortSession = 0x0120,
    /// BeginDraft.
    BeginDraft = 0x0130,
    /// StartDraftPart / DraftPartAccepted.
    StartDraftPart = 0x0131,
    /// FinalizeDraft.
    FinalizeDraft = 0x0132,
    /// QueryOperation.
    QueryOperation = 0x0200,
    /// QueryCatalog.
    QueryCatalog = 0x0201,
    /// QueryDraft.
    QueryDraft = 0x0202,
    /// QueryWeatherRequest.
    QueryWeatherRequest = 0x0203,
    /// DeleteObject.
    DeleteObject = 0x0300,
    /// SetMetadata.
    SetMetadata = 0x0301,
    /// AbortOperation.
    AbortOperation = 0x0302,
    /// InstallUpdate.
    InstallUpdate = 0x0310,
    /// AcknowledgeRideImported.
    AcknowledgeRideImported = 0x0311,
    /// GetDeviceStatus — device-control plane.
    GetDeviceStatus = 0x0400,
    /// GetConfig — device-control plane.
    GetConfig = 0x0401,
    /// SetConfig — device-control plane.
    SetConfig = 0x0402,
    /// SetClock — device-control plane.
    SetClock = 0x0403,
    /// ForgetBond — device-control plane, BLE only.
    ForgetBond = 0x0404,
    /// Echo — device-control plane.
    Echo = 0x0405,
    /// ResetStore — device-control plane, destructive.
    ResetStore = 0x0406,
}

impl Opcode {
    /// Every opcode, in registry order.
    pub const ALL: [Opcode; 26] = [
        Opcode::Hello,
        Opcode::StartUpload,
        Opcode::CheckpointUpload,
        Opcode::FinishUpload,
        Opcode::StartDownload,
        Opcode::FinishDownload,
        Opcode::AbortSession,
        Opcode::BeginDraft,
        Opcode::StartDraftPart,
        Opcode::FinalizeDraft,
        Opcode::QueryOperation,
        Opcode::QueryCatalog,
        Opcode::QueryDraft,
        Opcode::QueryWeatherRequest,
        Opcode::DeleteObject,
        Opcode::SetMetadata,
        Opcode::AbortOperation,
        Opcode::InstallUpdate,
        Opcode::AcknowledgeRideImported,
        Opcode::GetDeviceStatus,
        Opcode::GetConfig,
        Opcode::SetConfig,
        Opcode::SetClock,
        Opcode::ForgetBond,
        Opcode::Echo,
        Opcode::ResetStore,
    ];

    /// Decodes a wire `u16`. An unregistered value is `unsupportedCapability/opcode`, not a framing
    /// error: the frame parsed perfectly and names something this version does not serve.
    pub const fn from_u16(value: u16) -> Option<Self> {
        match value {
            0x0001 => Some(Opcode::Hello),
            0x0100 => Some(Opcode::StartUpload),
            0x0101 => Some(Opcode::CheckpointUpload),
            0x0102 => Some(Opcode::FinishUpload),
            0x0110 => Some(Opcode::StartDownload),
            0x0111 => Some(Opcode::FinishDownload),
            0x0120 => Some(Opcode::AbortSession),
            0x0130 => Some(Opcode::BeginDraft),
            0x0131 => Some(Opcode::StartDraftPart),
            0x0132 => Some(Opcode::FinalizeDraft),
            0x0200 => Some(Opcode::QueryOperation),
            0x0201 => Some(Opcode::QueryCatalog),
            0x0202 => Some(Opcode::QueryDraft),
            0x0203 => Some(Opcode::QueryWeatherRequest),
            0x0300 => Some(Opcode::DeleteObject),
            0x0301 => Some(Opcode::SetMetadata),
            0x0302 => Some(Opcode::AbortOperation),
            0x0310 => Some(Opcode::InstallUpdate),
            0x0311 => Some(Opcode::AcknowledgeRideImported),
            0x0400 => Some(Opcode::GetDeviceStatus),
            0x0401 => Some(Opcode::GetConfig),
            0x0402 => Some(Opcode::SetConfig),
            0x0403 => Some(Opcode::SetClock),
            0x0404 => Some(Opcode::ForgetBond),
            0x0405 => Some(Opcode::Echo),
            0x0406 => Some(Opcode::ResetStore),
            _ => None,
        }
    }

    /// The wire `u16`.
    pub const fn to_u16(self) -> u16 {
        self as u16
    }

    /// The name used in fixture JSON and diagnostics.
    pub const fn name(self) -> &'static str {
        match self {
            Opcode::Hello => "Hello",
            Opcode::StartUpload => "StartUpload",
            Opcode::CheckpointUpload => "CheckpointUpload",
            Opcode::FinishUpload => "FinishUpload",
            Opcode::StartDownload => "StartDownload",
            Opcode::FinishDownload => "FinishDownload",
            Opcode::AbortSession => "AbortSession",
            Opcode::BeginDraft => "BeginDraft",
            Opcode::StartDraftPart => "StartDraftPart",
            Opcode::FinalizeDraft => "FinalizeDraft",
            Opcode::QueryOperation => "QueryOperation",
            Opcode::QueryCatalog => "QueryCatalog",
            Opcode::QueryDraft => "QueryDraft",
            Opcode::QueryWeatherRequest => "QueryWeatherRequest",
            Opcode::DeleteObject => "DeleteObject",
            Opcode::SetMetadata => "SetMetadata",
            Opcode::AbortOperation => "AbortOperation",
            Opcode::InstallUpdate => "InstallUpdate",
            Opcode::AcknowledgeRideImported => "AcknowledgeRideImported",
            Opcode::GetDeviceStatus => "GetDeviceStatus",
            Opcode::GetConfig => "GetConfig",
            Opcode::SetConfig => "SetConfig",
            Opcode::SetClock => "SetClock",
            Opcode::ForgetBond => "ForgetBond",
            Opcode::Echo => "Echo",
            Opcode::ResetStore => "ResetStore",
        }
    }

    /// True for the `0x04xx` device-control plane of §16: no OperationId, no claim, no catalog, no
    /// retained result, and answerable with no card present.
    pub const fn is_device_control(self) -> bool {
        self.to_u16() & 0xFF00 == 0x0400
    }

    /// True for the three responses §5.2 allows to set the `more` flag.
    pub const fn is_pageable(self) -> bool {
        matches!(self, Opcode::Hello | Opcode::QueryCatalog | Opcode::QueryDraft)
    }

    /// True when the request carries an `OperationId` and therefore a durable claim (§4's
    /// mutation/claim column, minus the device-control plane).
    pub const fn claims_operation(self) -> bool {
        matches!(
            self,
            Opcode::StartUpload
                | Opcode::BeginDraft
                | Opcode::StartDraftPart
                | Opcode::DeleteObject
                | Opcode::SetMetadata
                | Opcode::AbortOperation
                | Opcode::InstallUpdate
                | Opcode::AcknowledgeRideImported
        )
    }

    /// The Capabilities command-flag bit that advertises this opcode, when it has one.
    ///
    /// §5 gates seventeen operations behind a bit; the transfer opcodes and Hello have none,
    /// because a device that cannot answer those is not speaking this protocol at all.
    pub const fn command_flag(self) -> Option<u32> {
        let bit = match self {
            Opcode::QueryOperation => 0,
            Opcode::QueryCatalog => 1,
            Opcode::QueryDraft => 2,
            Opcode::QueryWeatherRequest => 3,
            Opcode::BeginDraft => 4,
            Opcode::StartDraftPart => 5,
            Opcode::FinalizeDraft => 6,
            Opcode::AbortOperation => 7,
            Opcode::InstallUpdate => 8,
            Opcode::AcknowledgeRideImported => 9,
            Opcode::GetDeviceStatus => 10,
            Opcode::GetConfig => 11,
            Opcode::SetConfig => 12,
            Opcode::SetClock => 13,
            Opcode::ForgetBond => 14,
            Opcode::Echo => 15,
            Opcode::ResetStore => 16,
            _ => return None,
        };
        Some(1u32 << bit)
    }
}

/// The header's 16-bit flags word (§2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FrameFlags(u16);

impl FrameFlags {
    /// Bit 0 — this frame is a response.
    pub const RESPONSE: u16 = 1 << 0;
    /// Bit 1 — this response carries an [`crate::ErrorBody`].
    pub const ERROR: u16 = 1 << 1;
    /// Bit 2 — another page of this snapshot exists.
    pub const MORE: u16 = 1 << 2;
    /// Every defined bit; bits `3..15` are zero.
    pub const ALL: u16 = Self::RESPONSE | Self::ERROR | Self::MORE;

    /// A request: no flags at all.
    pub const REQUEST: FrameFlags = FrameFlags(0);
    /// A successful response.
    pub const OK: FrameFlags = FrameFlags(Self::RESPONSE);
    /// A successful response with another page to come.
    pub const OK_MORE: FrameFlags = FrameFlags(Self::RESPONSE | Self::MORE);
    /// An error response.
    pub const ERR: FrameFlags = FrameFlags(Self::RESPONSE | Self::ERROR);

    /// Wraps a raw flags word.
    pub const fn from_bits(bits: u16) -> Self {
        FrameFlags(bits)
    }

    /// The raw flags word.
    pub const fn bits(self) -> u16 {
        self.0
    }

    /// True when the response bit is set.
    pub const fn is_response(self) -> bool {
        self.0 & Self::RESPONSE != 0
    }

    /// True when the error bit is set.
    pub const fn is_error(self) -> bool {
        self.0 & Self::ERROR != 0
    }

    /// True when the more bit is set.
    pub const fn has_more(self) -> bool {
        self.0 & Self::MORE != 0
    }
}

/// One complete control frame: a validated header and its borrowed payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ControlFrame<'a> {
    /// The operation.
    pub opcode: Opcode,
    /// The direction/error/paging flags.
    pub flags: FrameFlags,
    /// The correlation identifier. A response echoes its request's.
    pub request_id: RequestId,
    /// Exactly the bytes after the header.
    pub payload: &'a [u8],
}

impl<'a> ControlFrame<'a> {
    /// Decodes one record against the protocol maximum.
    pub fn decode(record: &'a [u8]) -> crate::Result<Self> {
        Self::decode_bounded(record, MAX_CONTROL_FRAME)
    }

    /// Decodes one record against a negotiated frame limit.
    ///
    /// `negotiated_max` is the smaller of the two peers' advertised maxima (§1). A frame above it
    /// is `invalidFrame/frameBounds` **before allocation** — the length is checked against the
    /// limit before the payload is borrowed.
    pub fn decode_bounded(record: &'a [u8], negotiated_max: usize) -> crate::Result<Self> {
        if record.len() < HEADER_LEN {
            return Err(DecodeError::invalid_frame(detail::frame::RECORD_LENGTH));
        }
        if record.len() > negotiated_max.min(MAX_CONTROL_FRAME) {
            return Err(DecodeError::invalid_frame(detail::frame::FRAME_BOUNDS));
        }
        if record[0..4] != MAGIC {
            return Err(DecodeError::invalid_frame(detail::frame::MAGIC));
        }
        if record[4] != WIRE_MAJOR {
            return Err(DecodeError::incompatible_version(detail::version::UNSUPPORTED_MAJOR));
        }
        if record[5] != WIRE_MINOR {
            return Err(DecodeError::incompatible_version(detail::version::UNSUPPORTED_MINOR));
        }
        let payload_len = usize::from(u16_at(record, 10));
        if payload_len > MAX_CONTROL_PAYLOAD {
            return Err(DecodeError::invalid_frame(detail::frame::PAYLOAD_LENGTH));
        }
        if HEADER_LEN + payload_len != record.len() {
            return Err(DecodeError::invalid_frame(detail::frame::PAYLOAD_LENGTH));
        }
        let request_id = RequestId::new(u32_at(record, 12))
            .ok_or_else(|| DecodeError::invalid_descriptor(detail::descriptor::ZERO_REQUEST_ID))?;
        let opcode = Opcode::from_u16(u16_at(record, 6))
            .ok_or_else(|| DecodeError::unsupported_capability(detail::capability::OPCODE))?;
        let raw_flags = u16_at(record, 8);
        if raw_flags & !FrameFlags::ALL != 0 {
            return Err(DecodeError::unsupported_flags());
        }
        let flags = FrameFlags::from_bits(raw_flags);
        if !flags.is_response() && raw_flags != 0 {
            // §2: "Requests have no flags."
            return Err(DecodeError::unsupported_flags());
        }
        if flags.has_more() && !(flags.is_response() && !flags.is_error() && opcode.is_pageable()) {
            // §2: `more` "is valid only on a paged Capabilities, QueryCatalog, or QueryDraft
            // response".
            return Err(DecodeError::invalid_combination());
        }
        Ok(ControlFrame { opcode, flags, request_id, payload: &record[HEADER_LEN..] })
    }

    /// Encodes a frame from a header and a payload, returning the exact record length.
    pub fn encode_into(
        out: &mut [u8],
        opcode: Opcode,
        flags: FrameFlags,
        request_id: RequestId,
        payload: &[u8],
    ) -> EncodeResult {
        let needed = HEADER_LEN + payload.len();
        if out.len() < needed {
            return Err(BufferTooSmall { needed, available: out.len() });
        }
        if payload.len() > MAX_CONTROL_PAYLOAD {
            return Err(BufferTooSmall { needed: MAX_CONTROL_FRAME, available: out.len() });
        }
        let out = &mut out[..needed];
        put_bytes(out, 0, &MAGIC);
        out[4] = WIRE_MAJOR;
        out[5] = WIRE_MINOR;
        put_u16(out, 6, opcode.to_u16());
        put_u16(out, 8, flags.bits());
        put_u16(out, 10, payload.len() as u16);
        put_u32(out, 12, request_id.get());
        put_bytes(out, HEADER_LEN, payload);
        Ok(needed)
    }

    /// Re-encodes this frame, for the round-trip proof every fixture is held to.
    pub fn encode(&self, out: &mut [u8]) -> EncodeResult {
        Self::encode_into(out, self.opcode, self.flags, self.request_id, self.payload)
    }
}

impl DecodeError {
    /// True for the one refusal that must never be transmitted: `invalidDescriptor/zeroRequestId`.
    ///
    /// §2 makes a zero-RequestId frame unanswerable, because every response echoes its request and
    /// a zero echo is itself illegal. A caller that sees this closes the control record stream and
    /// logs; it does not build a response frame it has no identifier for.
    pub fn is_unanswerable(self) -> bool {
        self.category == crate::ErrorCategory::INVALID_DESCRIPTOR && self.detail == detail::descriptor::ZERO_REQUEST_ID
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ErrorCategory;
    use std::vec;
    use std::vec::Vec;

    fn frame(opcode: Opcode, flags: FrameFlags, request_id: u32, payload: &[u8]) -> Vec<u8> {
        let mut out = vec![0u8; HEADER_LEN + payload.len()];
        let len =
            ControlFrame::encode_into(&mut out, opcode, flags, RequestId::new(request_id).unwrap(), payload).unwrap();
        out.truncate(len);
        out
    }

    #[test]
    fn header_is_sixteen_bytes_at_the_stated_offsets() {
        let bytes = frame(Opcode::QueryCatalog, FrameFlags::REQUEST, 0x1234_5678, &[1, 2, 3]);
        assert_eq!(&bytes[0..4], b"OBCP");
        assert_eq!(bytes[4], 3);
        assert_eq!(bytes[5], 0);
        assert_eq!(u16_at(&bytes, 6), 0x0201);
        assert_eq!(u16_at(&bytes, 8), 0);
        assert_eq!(u16_at(&bytes, 10), 3);
        assert_eq!(u32_at(&bytes, 12), 0x1234_5678);
        assert_eq!(bytes.len(), 19);
    }

    #[test]
    fn round_trips() {
        let bytes = frame(Opcode::Hello, FrameFlags::OK_MORE, 9, &[7; 40]);
        let decoded = ControlFrame::decode(&bytes).unwrap();
        assert_eq!(decoded.opcode, Opcode::Hello);
        assert!(decoded.flags.has_more());
        assert_eq!(decoded.payload.len(), 40);
        let mut again = [0u8; 128];
        let len = decoded.encode(&mut again).unwrap();
        assert_eq!(&again[..len], &bytes[..]);
    }

    #[test]
    fn framing_failures_are_split_the_way_the_spec_splits_them() {
        let good = frame(Opcode::FinishUpload, FrameFlags::REQUEST, 1, &[0; 4]);

        let mut short = good.clone();
        short.truncate(HEADER_LEN - 1);
        assert_eq!(ControlFrame::decode(&short).unwrap_err(), DecodeError::invalid_frame(detail::frame::RECORD_LENGTH));

        let mut bad_magic = good.clone();
        bad_magic[0] = b'X';
        assert_eq!(ControlFrame::decode(&bad_magic).unwrap_err(), DecodeError::invalid_frame(detail::frame::MAGIC));

        let mut bad_major = good.clone();
        bad_major[4] = 2;
        assert_eq!(
            ControlFrame::decode(&bad_major).unwrap_err(),
            DecodeError::incompatible_version(detail::version::UNSUPPORTED_MAJOR)
        );

        let mut bad_minor = good.clone();
        bad_minor[5] = 1;
        assert_eq!(
            ControlFrame::decode(&bad_minor).unwrap_err(),
            DecodeError::incompatible_version(detail::version::UNSUPPORTED_MINOR)
        );

        let mut trailing = good.clone();
        trailing.push(0);
        assert_eq!(
            ControlFrame::decode(&trailing).unwrap_err(),
            DecodeError::invalid_frame(detail::frame::PAYLOAD_LENGTH)
        );

        let mut unknown_opcode = good.clone();
        put_u16(&mut unknown_opcode, 6, 0x0999);
        assert_eq!(
            ControlFrame::decode(&unknown_opcode).unwrap_err(),
            DecodeError::unsupported_capability(detail::capability::OPCODE)
        );
    }

    #[test]
    fn zero_request_id_is_unanswerable() {
        let mut bytes = frame(Opcode::Echo, FrameFlags::REQUEST, 1, &[]);
        put_u32(&mut bytes, 12, 0);
        let err = ControlFrame::decode(&bytes).unwrap_err();
        assert_eq!(err, DecodeError::invalid_descriptor(detail::descriptor::ZERO_REQUEST_ID));
        assert!(err.is_unanswerable());
    }

    #[test]
    fn reserved_header_flags_and_illegal_more_are_rejected() {
        let mut reserved = frame(Opcode::Hello, FrameFlags::OK, 1, &[]);
        put_u16(&mut reserved, 8, FrameFlags::RESPONSE | (1 << 3));
        assert_eq!(ControlFrame::decode(&reserved).unwrap_err(), DecodeError::unsupported_flags());

        let mut flagged_request = frame(Opcode::Hello, FrameFlags::REQUEST, 1, &[]);
        put_u16(&mut flagged_request, 8, FrameFlags::MORE);
        assert_eq!(ControlFrame::decode(&flagged_request).unwrap_err(), DecodeError::unsupported_flags());

        let mut more_on_unpageable = frame(Opcode::QueryOperation, FrameFlags::OK, 1, &[]);
        put_u16(&mut more_on_unpageable, 8, FrameFlags::RESPONSE | FrameFlags::MORE);
        assert_eq!(ControlFrame::decode(&more_on_unpageable).unwrap_err(), DecodeError::invalid_combination());

        let mut more_on_error = frame(Opcode::QueryCatalog, FrameFlags::ERR, 1, &[]);
        put_u16(&mut more_on_error, 8, FrameFlags::RESPONSE | FrameFlags::ERROR | FrameFlags::MORE);
        assert_eq!(ControlFrame::decode(&more_on_error).unwrap_err(), DecodeError::invalid_combination());
    }

    #[test]
    fn a_frame_above_the_negotiated_limit_is_frame_bounds() {
        let bytes = frame(Opcode::Echo, FrameFlags::REQUEST, 1, &[0; 240]);
        assert!(ControlFrame::decode(&bytes).is_ok());
        let err = ControlFrame::decode_bounded(&bytes, MIN_CONTROL_FRAME).unwrap_err();
        assert_eq!(err, DecodeError::invalid_frame(detail::frame::FRAME_BOUNDS));
        assert_eq!(err.category, ErrorCategory::INVALID_FRAME);
    }

    #[test]
    fn the_control_floor_is_derived_from_the_schema_ceilings() {
        // §1: 44-byte page prefix + 36-byte entry prefix + 96 metadata bytes = 176 payload bytes,
        // and 176 + the 16-byte header is exactly the 192-byte floor.
        assert_eq!(44 + 36 + 96, MIN_FRAME_PAYLOAD);
        assert_eq!(MIN_FRAME_PAYLOAD + HEADER_LEN, MIN_CONTROL_FRAME);
        // The other constituent: 48 fixed StartUpload bytes plus a 128-byte metadata envelope.
        assert_eq!(48 + 128, MIN_FRAME_PAYLOAD);
        // And the maximum text-bearing ErrorBody is 112 payload bytes, well inside it.
        assert_eq!(crate::error::MAX_ERROR_BODY_LEN, 112);
    }

    #[test]
    fn command_flag_bits_match_the_capability_word() {
        assert_eq!(Opcode::QueryOperation.command_flag(), Some(1 << 0));
        assert_eq!(Opcode::ResetStore.command_flag(), Some(1 << 16));
        assert_eq!(Opcode::Hello.command_flag(), None);
        assert_eq!(Opcode::StartUpload.command_flag(), None);
        let mut seen = 0u32;
        for opcode in Opcode::ALL {
            if let Some(bit) = opcode.command_flag() {
                assert_eq!(seen & bit, 0, "{} reuses a command flag bit", opcode.name());
                seen |= bit;
            }
        }
        assert_eq!(seen, (1u32 << 17) - 1);
    }

    #[test]
    fn the_device_control_plane_is_exactly_the_0x04xx_block() {
        let control: Vec<_> = Opcode::ALL.iter().filter(|o| o.is_device_control()).collect();
        assert_eq!(control.len(), 7);
        for opcode in control {
            assert!(!opcode.claims_operation(), "{} must claim nothing", opcode.name());
        }
    }
}
