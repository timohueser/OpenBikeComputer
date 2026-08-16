//! The stream frame and its compact fault body (`Device_Object_Protocol_v3.md` §13).
//!
//! Every stream transport record is one 16-byte header and its payload. The legal flag combinations
//! "are exhaustive, per direction, and every other combination is `invalidFrame`" — including the
//! two that look plausible and are not: any nonzero flag on a data direction, and terminal without
//! fault. §13 says why the second is reserved: "a stream has no successful terminal frame: success
//! is FinishUpload or FinishDownload on the control link, never a stream flag."
//!
//! The fault body is deliberately narrow: "Only namespace-zero transport category/details from
//! Section 12 are valid in this compact body; semantic/domain errors use a correlated control
//! response."

use crate::codec::{put_u16, put_u32, put_u64, u16_at, u32_at, u64_at};
use crate::error::{detail, reject_nonzero, DecodeError, ErrorCategory};
use crate::frame::{MAX_STREAM_FRAME, MIN_STREAM_FRAME};
use crate::ids::SessionId;
use crate::{BufferTooSmall, EncodeResult};

/// The stream header, in bytes.
pub const STREAM_HEADER_LEN: usize = 16;

/// The fault status body, in bytes.
pub const FAULT_BODY_LEN: usize = 24;

/// The largest stream payload the hard maximum frame allows.
pub const MAX_STREAM_PAYLOAD: usize = MAX_STREAM_FRAME - STREAM_HEADER_LEN;

/// A stream frame's direction (§13).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Direction {
    /// Client to device payload bytes.
    Upload = 1,
    /// Device to client payload bytes.
    Download = 2,
    /// A status frame. Its offset is always zero.
    Status = 3,
}

impl Direction {
    /// Every direction, in wire order.
    pub const ALL: [Direction; 3] = [Direction::Upload, Direction::Download, Direction::Status];

    /// Decodes a wire `u8`.
    pub const fn from_u8(value: u8) -> Option<Self> {
        match value {
            1 => Some(Direction::Upload),
            2 => Some(Direction::Download),
            3 => Some(Direction::Status),
            _ => None,
        }
    }

    /// The wire `u8`.
    pub const fn to_u8(self) -> u8 {
        self as u8
    }

    /// True for the two directions that carry payload bytes.
    pub const fn is_data(self) -> bool {
        matches!(self, Direction::Upload | Direction::Download)
    }

    /// The name used in fixture JSON.
    pub const fn name(self) -> &'static str {
        match self {
            Direction::Upload => "upload",
            Direction::Download => "download",
            Direction::Status => "status",
        }
    }
}

/// Stream flag bits (§13).
pub mod stream_flags {
    /// Bit 0 — this status frame reports a fault.
    pub const FAULT: u8 = 1 << 0;
    /// Bit 1 — the session is released and the operation is durably aborted or must be queried.
    pub const TERMINAL: u8 = 1 << 1;
    /// Every defined bit.
    pub const ALL: u8 = FAULT | TERMINAL;
}

/// What a client should do after a fault (§13).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum FaultDisposition {
    /// Resume under a new SessionId. Nonterminal.
    ResumeWithNewSession = 0,
    /// The operation is durably aborted. Terminal.
    OperationDurablyAborted = 1,
    /// The stream transport is closed; query the operation's status. Terminal.
    StreamClosedQueryStatus = 2,
}

impl FaultDisposition {
    /// Every disposition, in wire order.
    pub const ALL: [FaultDisposition; 3] = [
        FaultDisposition::ResumeWithNewSession,
        FaultDisposition::OperationDurablyAborted,
        FaultDisposition::StreamClosedQueryStatus,
    ];

    /// Decodes a wire `u8`.
    pub const fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(FaultDisposition::ResumeWithNewSession),
            1 => Some(FaultDisposition::OperationDurablyAborted),
            2 => Some(FaultDisposition::StreamClosedQueryStatus),
            _ => None,
        }
    }

    /// The wire `u8`.
    pub const fn to_u8(self) -> u8 {
        self as u8
    }

    /// True for the two dispositions §13 allows to appear with the terminal bit.
    pub const fn may_be_terminal(self) -> bool {
        matches!(self, FaultDisposition::OperationDurablyAborted | FaultDisposition::StreamClosedQueryStatus)
    }

    /// The name used in fixture JSON.
    pub const fn name(self) -> &'static str {
        match self {
            FaultDisposition::ResumeWithNewSession => "resumeWithNewSession",
            FaultDisposition::OperationDurablyAborted => "operationDurablyAborted",
            FaultDisposition::StreamClosedQueryStatus => "streamClosedQueryStatus",
        }
    }
}

/// The 24-byte fault body (§13).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FaultBody {
    /// A namespace-zero transport category from §12.
    pub category: ErrorCategory,
    /// Its category-scoped detail.
    pub detail: u16,
    /// The offset the session expected.
    pub expected_next_offset: u64,
    /// The offset that is durable.
    pub durable_next_offset: u64,
    /// What to do next.
    pub disposition: FaultDisposition,
}

impl FaultBody {
    /// The categories §13 admits into the compact fault body.
    ///
    /// "Only namespace-zero transport category/details from Section 12 are valid in this compact
    /// body; semantic/domain errors use a correlated control response." The body has no namespace
    /// field at all, so `semanticValidation` — the one category whose detail is namespace-scoped —
    /// could not be read unambiguously even if it were allowed. The rest are the transport-layer
    /// conditions a stream receiver can actually raise while a session is attached.
    pub const TRANSPORT_CATEGORIES: [ErrorCategory; 10] = [
        ErrorCategory::INVALID_FRAME,
        ErrorCategory::INVALID_DESCRIPTOR,
        ErrorCategory::INVALID_OFFSET,
        ErrorCategory::INVALID_SESSION,
        ErrorCategory::CHECKSUM_FAILURE,
        ErrorCategory::MEDIA_UNAVAILABLE,
        ErrorCategory::MEDIA_IO,
        ErrorCategory::CANCELLED,
        ErrorCategory::LINK_LOST,
        ErrorCategory::INTERNAL,
    ];

    /// Decodes exactly [`FAULT_BODY_LEN`] bytes.
    pub fn decode(body: &[u8]) -> crate::Result<Self> {
        DecodeError::exact_len(body, FAULT_BODY_LEN)?;
        reject_nonzero(body, 21, 3)?;
        let category = ErrorCategory::from_u16(u16_at(body, 0)).ok_or_else(DecodeError::unknown_enum)?;
        if !Self::TRANSPORT_CATEGORIES.contains(&category) {
            return Err(DecodeError::unknown_enum());
        }
        Ok(FaultBody {
            category,
            detail: u16_at(body, 2),
            expected_next_offset: u64_at(body, 4),
            durable_next_offset: u64_at(body, 12),
            disposition: FaultDisposition::from_u8(body[20]).ok_or_else(DecodeError::unknown_enum)?,
        })
    }

    /// Encodes the body.
    pub fn encode(&self) -> [u8; FAULT_BODY_LEN] {
        let mut out = [0u8; FAULT_BODY_LEN];
        put_u16(&mut out, 0, self.category.get());
        put_u16(&mut out, 2, self.detail);
        put_u64(&mut out, 4, self.expected_next_offset);
        put_u64(&mut out, 12, self.durable_next_offset);
        out[20] = self.disposition.to_u8();
        out
    }
}

/// One complete stream frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamFrame<'a> {
    /// An ordinary data frame at exactly the session's next offset.
    Data {
        /// The session.
        session_id: SessionId,
        /// Upload or download.
        direction: Direction,
        /// The absolute payload offset.
        offset: u64,
        /// The payload, which is never empty.
        payload: &'a [u8],
    },
    /// A status frame carrying a fault.
    Fault {
        /// The session.
        session_id: SessionId,
        /// Whether the session is released and the operation terminal.
        terminal: bool,
        /// The fault.
        body: FaultBody,
    },
}

impl<'a> StreamFrame<'a> {
    /// The exact encoded record length.
    pub fn encoded_len(&self) -> usize {
        STREAM_HEADER_LEN
            + match self {
                StreamFrame::Data { payload, .. } => payload.len(),
                StreamFrame::Fault { .. } => FAULT_BODY_LEN,
            }
    }

    /// The session the frame names.
    pub fn session_id(&self) -> SessionId {
        match self {
            StreamFrame::Data { session_id, .. } | StreamFrame::Fault { session_id, .. } => *session_id,
        }
    }

    /// Decodes one stream record against the hard maximum.
    pub fn decode(record: &'a [u8]) -> crate::Result<Self> {
        Self::decode_bounded(record, MAX_STREAM_FRAME)
    }

    /// Decodes one stream record against a negotiated (or CoC-effective) frame limit.
    ///
    /// §14.0: on BLE "The effective stream frame limit is therefore
    /// `min(negotiated stream maximum, CoC SDU)`, fixed at CoC establishment", and "every session
    /// start validates its frames against the effective limit, not against the advertised one".
    pub fn decode_bounded(record: &'a [u8], effective_max: usize) -> crate::Result<Self> {
        if record.len() < STREAM_HEADER_LEN {
            return Err(DecodeError::invalid_frame(detail::frame::RECORD_LENGTH));
        }
        if record.len() > effective_max.min(MAX_STREAM_FRAME) {
            return Err(DecodeError::invalid_frame(detail::frame::FRAME_BOUNDS));
        }
        let payload_len = usize::from(u16_at(record, 12));
        if STREAM_HEADER_LEN + payload_len != record.len() {
            return Err(DecodeError::invalid_frame(detail::frame::PAYLOAD_LENGTH));
        }
        let session_id = SessionId::new(u32_at(record, 0)).ok_or_else(DecodeError::unknown_enum)?;
        let direction = Direction::from_u8(record[14]).ok_or_else(DecodeError::unknown_enum)?;
        let flags = record[15];
        if flags & !stream_flags::ALL != 0 {
            return Err(DecodeError::invalid_frame(detail::frame::MALFORMED_HEADER));
        }
        let offset = u64_at(record, 4);
        let payload = &record[STREAM_HEADER_LEN..];
        if direction.is_data() {
            if flags != 0 {
                // "any nonzero flag on a data direction is rejected".
                return Err(DecodeError::invalid_frame(detail::frame::MALFORMED_HEADER));
            }
            if payload.is_empty() {
                // "Data directions have nonempty payload".
                return Err(DecodeError::invalid_frame(detail::frame::PAYLOAD_LENGTH));
            }
            offset
                .checked_add(payload_len as u64)
                .ok_or_else(|| DecodeError::invalid_frame(detail::frame::PAYLOAD_LENGTH))?;
            return Ok(StreamFrame::Data { session_id, direction, offset, payload });
        }
        if offset != 0 {
            // "Status direction has offset zero."
            return Err(DecodeError::reserved_bits());
        }
        if flags & stream_flags::FAULT == 0 {
            // Status with no fault bit, and status with terminal alone, are both reserved.
            return Err(DecodeError::invalid_frame(detail::frame::MALFORMED_HEADER));
        }
        let terminal = flags & stream_flags::TERMINAL != 0;
        let body = FaultBody::decode(payload)?;
        if terminal != body.disposition.may_be_terminal() {
            // Disposition `0` is the nonterminal form; `1` and `2` are the terminal ones.
            return Err(DecodeError::invalid_combination());
        }
        Ok(StreamFrame::Fault { session_id, terminal, body })
    }

    /// Encodes the record into `out`, returning its exact length.
    pub fn encode_into(&self, out: &mut [u8]) -> EncodeResult {
        let needed = self.encoded_len();
        if out.len() < needed {
            return Err(BufferTooSmall { needed, available: out.len() });
        }
        let out = &mut out[..needed];
        out.fill(0);
        match self {
            StreamFrame::Data { session_id, direction, offset, payload } => {
                put_u32(out, 0, session_id.get());
                put_u64(out, 4, *offset);
                put_u16(out, 12, payload.len() as u16);
                out[14] = direction.to_u8();
                out[STREAM_HEADER_LEN..].copy_from_slice(payload);
            }
            StreamFrame::Fault { session_id, terminal, body } => {
                put_u32(out, 0, session_id.get());
                put_u16(out, 12, FAULT_BODY_LEN as u16);
                out[14] = Direction::Status.to_u8();
                out[15] = stream_flags::FAULT | if *terminal { stream_flags::TERMINAL } else { 0 };
                out[STREAM_HEADER_LEN..].copy_from_slice(&body.encode());
            }
        }
        Ok(needed)
    }
}

/// A 64-byte stream frame — the protocol floor — carries this much payload.
pub const MIN_STREAM_PAYLOAD_AT_FLOOR: usize = MIN_STREAM_FRAME - STREAM_HEADER_LEN;

#[cfg(test)]
mod tests {
    use super::*;
    use std::vec;
    use std::vec::Vec;

    fn data(offset: u64, payload: &[u8]) -> Vec<u8> {
        let frame =
            StreamFrame::Data { session_id: SessionId::new(3).unwrap(), direction: Direction::Upload, offset, payload };
        let mut out = vec![0u8; frame.encoded_len()];
        frame.encode_into(&mut out).unwrap();
        out
    }

    #[test]
    fn data_frames_round_trip_at_every_boundary() {
        for (offset, len) in [(0u64, 1usize), (262_144, 1008), (0xFFFF_FFFE, 1), (0xFFFF_FFFF, 4080)] {
            let payload = vec![0xA5u8; len];
            let bytes = data(offset, &payload);
            let decoded = StreamFrame::decode(&bytes).unwrap();
            let StreamFrame::Data { offset: decoded_offset, payload: decoded_payload, .. } = decoded else {
                panic!("expected data");
            };
            assert_eq!(decoded_offset, offset);
            assert_eq!(decoded_payload.len(), len);
            let mut again = vec![0u8; bytes.len()];
            decoded.encode_into(&mut again).unwrap();
            assert_eq!(again, bytes);
        }
    }

    #[test]
    fn every_forbidden_data_frame_shape_is_rejected() {
        let bytes = data(0, b"x");

        let mut zero_session = bytes.clone();
        put_u32(&mut zero_session, 0, 0);
        assert_eq!(StreamFrame::decode(&zero_session).unwrap_err(), DecodeError::unknown_enum());

        let mut flagged = bytes.clone();
        flagged[15] = stream_flags::FAULT;
        assert_eq!(
            StreamFrame::decode(&flagged).unwrap_err(),
            DecodeError::invalid_frame(detail::frame::MALFORMED_HEADER)
        );

        let mut reserved_flag = bytes.clone();
        reserved_flag[15] = 1 << 4;
        assert_eq!(
            StreamFrame::decode(&reserved_flag).unwrap_err(),
            DecodeError::invalid_frame(detail::frame::MALFORMED_HEADER)
        );

        let mut bad_direction = bytes.clone();
        bad_direction[14] = 4;
        assert_eq!(StreamFrame::decode(&bad_direction).unwrap_err(), DecodeError::unknown_enum());

        let mut zero_payload = bytes.clone();
        put_u16(&mut zero_payload, 12, 0);
        zero_payload.truncate(STREAM_HEADER_LEN);
        assert_eq!(
            StreamFrame::decode(&zero_payload).unwrap_err(),
            DecodeError::invalid_frame(detail::frame::PAYLOAD_LENGTH)
        );

        let mut truncated = bytes.clone();
        truncated.pop();
        assert_eq!(
            StreamFrame::decode(&truncated).unwrap_err(),
            DecodeError::invalid_frame(detail::frame::PAYLOAD_LENGTH)
        );

        // offset + length overflowing the u64 space.
        let mut overflow = data(u64::MAX, b"xx");
        put_u64(&mut overflow, 4, u64::MAX);
        assert_eq!(
            StreamFrame::decode(&overflow).unwrap_err(),
            DecodeError::invalid_frame(detail::frame::PAYLOAD_LENGTH)
        );

        // A frame above the effective limit.
        let big = data(0, &vec![0u8; 1024]);
        assert!(StreamFrame::decode(&big).is_ok());
        assert_eq!(
            StreamFrame::decode_bounded(&big, 512).unwrap_err(),
            DecodeError::invalid_frame(detail::frame::FRAME_BOUNDS)
        );
    }

    fn fault(disposition: FaultDisposition, terminal: bool) -> Vec<u8> {
        let frame = StreamFrame::Fault {
            session_id: SessionId::new(3).unwrap(),
            terminal,
            body: FaultBody {
                category: ErrorCategory::INVALID_OFFSET,
                detail: detail::offset::UNEXPECTED_OFFSET,
                expected_next_offset: 262_144,
                durable_next_offset: 262_144,
                disposition,
            },
        };
        let mut out = vec![0u8; frame.encoded_len()];
        frame.encode_into(&mut out).unwrap();
        out
    }

    #[test]
    fn every_fault_disposition_round_trips_in_its_legal_form() {
        for (disposition, terminal) in [
            (FaultDisposition::ResumeWithNewSession, false),
            (FaultDisposition::OperationDurablyAborted, true),
            (FaultDisposition::StreamClosedQueryStatus, true),
        ] {
            let bytes = fault(disposition, terminal);
            assert_eq!(bytes.len(), 40);
            let decoded = StreamFrame::decode(&bytes).unwrap();
            let StreamFrame::Fault { terminal: decoded_terminal, body, .. } = decoded else { panic!("expected fault") };
            assert_eq!(decoded_terminal, terminal);
            assert_eq!(body.disposition, disposition);
            let mut again = vec![0u8; bytes.len()];
            decoded.encode_into(&mut again).unwrap();
            assert_eq!(again, bytes);
        }
    }

    #[test]
    fn status_frames_reject_the_combinations_the_flag_table_forbids() {
        // Terminal without fault.
        let mut bytes = fault(FaultDisposition::OperationDurablyAborted, true);
        bytes[15] = stream_flags::TERMINAL;
        assert_eq!(
            StreamFrame::decode(&bytes).unwrap_err(),
            DecodeError::invalid_frame(detail::frame::MALFORMED_HEADER)
        );

        // Status with no flags at all.
        let mut bytes = fault(FaultDisposition::ResumeWithNewSession, false);
        bytes[15] = 0;
        assert_eq!(
            StreamFrame::decode(&bytes).unwrap_err(),
            DecodeError::invalid_frame(detail::frame::MALFORMED_HEADER)
        );

        // A nonzero offset on a status frame.
        let mut bytes = fault(FaultDisposition::ResumeWithNewSession, false);
        put_u64(&mut bytes, 4, 1);
        assert_eq!(StreamFrame::decode(&bytes).unwrap_err(), DecodeError::reserved_bits());

        // A terminal disposition without the terminal bit, and the reverse.
        let mut bytes = fault(FaultDisposition::OperationDurablyAborted, true);
        bytes[15] = stream_flags::FAULT;
        assert_eq!(StreamFrame::decode(&bytes).unwrap_err(), DecodeError::invalid_combination());
        let mut bytes = fault(FaultDisposition::ResumeWithNewSession, false);
        bytes[15] = stream_flags::FAULT | stream_flags::TERMINAL;
        assert_eq!(StreamFrame::decode(&bytes).unwrap_err(), DecodeError::invalid_combination());

        // A reserved byte inside the fault body.
        let mut bytes = fault(FaultDisposition::ResumeWithNewSession, false);
        bytes[STREAM_HEADER_LEN + 21] = 1;
        assert_eq!(StreamFrame::decode(&bytes).unwrap_err(), DecodeError::reserved_bits());

        // A semantic/domain category in the compact body, which has no namespace field to carry
        // the ObjectKind such a detail would be scoped to.
        let mut bytes = fault(FaultDisposition::ResumeWithNewSession, false);
        put_u16(&mut bytes, STREAM_HEADER_LEN, ErrorCategory::SEMANTIC_VALIDATION.get());
        assert_eq!(StreamFrame::decode(&bytes).unwrap_err(), DecodeError::unknown_enum());
        for category in [ErrorCategory::BUSY, ErrorCategory::REVISION_CONFLICT, ErrorCategory::OBJECT_NOT_FOUND] {
            let mut bytes = fault(FaultDisposition::ResumeWithNewSession, false);
            put_u16(&mut bytes, STREAM_HEADER_LEN, category.get());
            assert_eq!(StreamFrame::decode(&bytes).unwrap_err(), DecodeError::unknown_enum());
        }
    }

    #[test]
    fn the_stream_floor_carries_a_whole_fault_body() {
        assert_eq!(MIN_STREAM_PAYLOAD_AT_FLOOR, 48);
        // The 24-byte fault body fits the floor with room to spare, which is what makes a fault
        // deliverable on the smallest channel this protocol admits.
        assert_eq!(MIN_STREAM_PAYLOAD_AT_FLOOR - FAULT_BODY_LEN, 24);
    }
}
