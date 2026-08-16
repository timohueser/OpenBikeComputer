//! Downloads (`Device_Object_Protocol_v3.md` §7).
//!
//! "**A download always resolves the current committed head.** There is no way to address an older
//! revision, and the device holds no history a client could ask for." Flag bit 0 and the eight
//! bytes at offset 12 of a StartDownload "are burned rather than removed: they carried a requested
//! revision in an earlier draft of this contract, and no v3.0 peer sets either" — so this decoder
//! rejects both as `invalidDescriptor/reservedBits`, and `objectNotFound/requestedRevision` stays
//! registered, reserved, and unemitted.
//!
//! A download is also not a claimed operation: it carries no `OperationId`, `QueryOperation` never
//! reports one, and the lease its acceptance takes is a RAM capability released exactly once.

use crate::codec::{bytes16_at, put_bytes, put_u16, put_u32, put_u64, u16_at, u32_at, u64_at};
use crate::error::{reject_nonzero, DecodeError};
use crate::ids::{LogicalObjectId, Revision, SessionId, StoreId};
use crate::registry::{object_kind, ObjectKind};

/// The StartDownload request.
pub const START_DOWNLOAD_LEN: usize = 28;

/// The DownloadAccepted response.
pub const DOWNLOAD_ACCEPTED_LEN: usize = 60;

/// The FinishDownload request.
pub const FINISH_DOWNLOAD_LEN: usize = 16;

/// StartDownload flag bits (§7).
pub mod download_flags {
    /// Bit 0 — burned. It carried a requested revision in an earlier draft and no v3.0 peer sets it.
    pub const RESERVED_REVISION: u16 = 1 << 0;
    /// Bit 1 — the start-offset field is meaningful.
    pub const START_OFFSET: u16 = 1 << 1;
    /// Every bit a v3.0 peer may set.
    pub const ALLOWED: u16 = START_OFFSET;
}

/// The StartDownload request (§7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StartDownload {
    /// The kind to read.
    pub kind: ObjectKind,
    /// The logical object whose current head is wanted.
    pub logical_object_id: LogicalObjectId,
    /// Where to start. `None` encodes the flag clear and the field zero; `Some` is allowed only
    /// when the kind advertises resumable download.
    pub start_offset: Option<u64>,
}

impl StartDownload {
    /// Decodes exactly [`START_DOWNLOAD_LEN`] bytes.
    pub fn decode(payload: &[u8]) -> crate::Result<Self> {
        DecodeError::exact_len(payload, START_DOWNLOAD_LEN)?;
        let flags = u16_at(payload, 2);
        if flags & !download_flags::ALLOWED != 0 {
            // Covers the burned revision bit and every undefined bit alike.
            return Err(DecodeError::reserved_bits());
        }
        reject_nonzero(payload, 12, 8)?;
        let raw_offset = u64_at(payload, 20);
        let start_offset = if flags & download_flags::START_OFFSET != 0 {
            Some(raw_offset)
        } else {
            if raw_offset != 0 {
                return Err(DecodeError::reserved_bits());
            }
            None
        };
        Ok(StartDownload {
            kind: object_kind(u16_at(payload, 0))?,
            logical_object_id: LogicalObjectId::new(u64_at(payload, 4)),
            start_offset,
        })
    }

    /// Encodes the request.
    pub fn encode(&self) -> [u8; START_DOWNLOAD_LEN] {
        let mut out = [0u8; START_DOWNLOAD_LEN];
        put_u16(&mut out, 0, self.kind.to_u16());
        put_u16(&mut out, 2, if self.start_offset.is_some() { download_flags::START_OFFSET } else { 0 });
        put_u64(&mut out, 4, self.logical_object_id.get());
        put_u64(&mut out, 20, self.start_offset.unwrap_or(0));
        out
    }
}

/// The DownloadAccepted response (§7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DownloadAccepted {
    /// The store the bytes come from.
    pub store_id: StoreId,
    /// The stream capability.
    pub session_id: SessionId,
    /// The logical object.
    pub logical_object_id: LogicalObjectId,
    /// The revision that was pinned. A later replace or delete changes visibility, not these bytes.
    pub pinned_revision: Revision,
    /// The whole source length.
    pub total_length: u64,
    /// The whole-source CRC-32/IEEE.
    pub whole_source_crc32: u32,
    /// The accepted start offset, which always equals the requested one: "The device has no
    /// discretion to move it."
    pub accepted_start_offset: u64,
    /// The largest stream payload this session emits.
    pub max_stream_payload: u16,
}

impl DownloadAccepted {
    /// Decodes exactly [`DOWNLOAD_ACCEPTED_LEN`] bytes.
    pub fn decode(payload: &[u8]) -> crate::Result<Self> {
        DecodeError::exact_len(payload, DOWNLOAD_ACCEPTED_LEN)?;
        reject_nonzero(payload, 58, 2)?;
        Ok(DownloadAccepted {
            store_id: StoreId::new(bytes16_at(payload, 0)),
            session_id: SessionId::new(u32_at(payload, 16)).ok_or_else(DecodeError::unknown_enum)?,
            logical_object_id: LogicalObjectId::new(u64_at(payload, 20)),
            pinned_revision: Revision::new(u64_at(payload, 28)),
            total_length: u64_at(payload, 36),
            whole_source_crc32: u32_at(payload, 44),
            accepted_start_offset: u64_at(payload, 48),
            max_stream_payload: u16_at(payload, 56),
        })
    }

    /// Encodes the response.
    pub fn encode(&self) -> [u8; DOWNLOAD_ACCEPTED_LEN] {
        let mut out = [0u8; DOWNLOAD_ACCEPTED_LEN];
        put_bytes(&mut out, 0, self.store_id.as_bytes());
        put_u32(&mut out, 16, self.session_id.get());
        put_u64(&mut out, 20, self.logical_object_id.get());
        put_u64(&mut out, 28, self.pinned_revision.get());
        put_u64(&mut out, 36, self.total_length);
        put_u32(&mut out, 44, self.whole_source_crc32);
        put_u64(&mut out, 48, self.accepted_start_offset);
        put_u16(&mut out, 56, self.max_stream_payload);
        out
    }
}

/// The FinishDownload request (§7). Its success response payload is empty and releases the lease
/// exactly once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FinishDownload {
    /// The session being finished.
    pub session_id: SessionId,
    /// The whole-source length the client received, including a locally retained prefix when the
    /// start offset was nonzero.
    pub received_length: u64,
    /// The whole-source CRC over that same span.
    pub whole_source_crc32: u32,
}

impl FinishDownload {
    /// Decodes exactly [`FINISH_DOWNLOAD_LEN`] bytes.
    pub fn decode(payload: &[u8]) -> crate::Result<Self> {
        DecodeError::exact_len(payload, FINISH_DOWNLOAD_LEN)?;
        Ok(FinishDownload {
            session_id: SessionId::new(u32_at(payload, 0)).ok_or_else(DecodeError::unknown_enum)?,
            received_length: u64_at(payload, 4),
            whole_source_crc32: u32_at(payload, 12),
        })
    }

    /// Encodes the request.
    pub fn encode(&self) -> [u8; FINISH_DOWNLOAD_LEN] {
        let mut out = [0u8; FINISH_DOWNLOAD_LEN];
        put_u32(&mut out, 0, self.session_id.get());
        put_u64(&mut out, 4, self.received_length);
        put_u32(&mut out, 12, self.whole_source_crc32);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_download_round_trips_with_and_without_a_start_offset() {
        let plain =
            StartDownload { kind: ObjectKind::Ride, logical_object_id: LogicalObjectId::new(19), start_offset: None };
        let bytes = plain.encode();
        assert_eq!(bytes.len(), 28);
        assert_eq!(StartDownload::decode(&bytes).unwrap(), plain);

        let resumed = StartDownload { start_offset: Some(0xFFFF_FFFF), ..plain };
        assert_eq!(StartDownload::decode(&resumed.encode()).unwrap(), resumed);

        // A zero start offset is still expressible with the flag set: it is not the same request as
        // the flag being clear, even though both stream from byte zero.
        let explicit_zero = StartDownload { start_offset: Some(0), ..plain };
        assert_eq!(StartDownload::decode(&explicit_zero.encode()).unwrap(), explicit_zero);
    }

    #[test]
    fn the_burned_revision_flag_and_field_are_reserved_bits() {
        let request =
            StartDownload { kind: ObjectKind::Route, logical_object_id: LogicalObjectId::new(1), start_offset: None };
        let mut with_flag = request.encode();
        put_u16(&mut with_flag, 2, download_flags::RESERVED_REVISION);
        assert_eq!(StartDownload::decode(&with_flag).unwrap_err(), DecodeError::reserved_bits());

        let mut with_field = request.encode();
        put_u64(&mut with_field, 12, 7);
        assert_eq!(StartDownload::decode(&with_field).unwrap_err(), DecodeError::reserved_bits());

        let mut offset_without_flag = request.encode();
        put_u64(&mut offset_without_flag, 20, 4096);
        assert_eq!(StartDownload::decode(&offset_without_flag).unwrap_err(), DecodeError::reserved_bits());
    }

    #[test]
    fn download_accepted_and_finish_round_trip_at_their_frozen_sizes() {
        let accepted = DownloadAccepted {
            store_id: StoreId::new([0x5A; 16]),
            session_id: SessionId::new(2).unwrap(),
            logical_object_id: LogicalObjectId::new(19),
            pinned_revision: Revision::new(41),
            total_length: 0x1_0000_0000,
            whole_source_crc32: 0xCBF4_3926,
            accepted_start_offset: 0,
            max_stream_payload: 4080,
        };
        let bytes = accepted.encode();
        assert_eq!(bytes.len(), 60);
        assert_eq!(DownloadAccepted::decode(&bytes).unwrap(), accepted);
        let mut reserved = bytes;
        reserved[58] = 1;
        assert_eq!(DownloadAccepted::decode(&reserved).unwrap_err(), DecodeError::reserved_bits());

        let finish = FinishDownload {
            session_id: SessionId::new(2).unwrap(),
            received_length: 0x1_0000_0000,
            whole_source_crc32: 0xCBF4_3926,
        };
        let bytes = finish.encode();
        assert_eq!(bytes.len(), 16);
        assert_eq!(FinishDownload::decode(&bytes).unwrap(), finish);
        let mut zero_session = bytes;
        put_u32(&mut zero_session, 0, 0);
        assert_eq!(FinishDownload::decode(&zero_session).unwrap_err(), DecodeError::unknown_enum());
    }

    #[test]
    fn a_u64_field_carries_its_full_range() {
        // §1: "a codec MUST decode and encode the full unsigned 64-bit range ... A codec that
        // silently truncates to 32 bits is nonconforming."
        let accepted = DownloadAccepted {
            store_id: StoreId::ZERO,
            session_id: SessionId::new(1).unwrap(),
            logical_object_id: LogicalObjectId::new(u64::MAX),
            pinned_revision: Revision::new(u64::MAX),
            total_length: u64::MAX,
            whole_source_crc32: u32::MAX,
            accepted_start_offset: u64::MAX - 1,
            max_stream_payload: u16::MAX,
        };
        assert_eq!(DownloadAccepted::decode(&accepted.encode()).unwrap(), accepted);
    }
}
