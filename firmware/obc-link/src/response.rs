//! The response side of the control link.
//!
//! [`Response::decode`] needs the whole frame rather than just its payload, because two of the
//! contract's rules are only checkable with the header in hand:
//!
//! - the `error` flag selects an [`ErrorBody`] regardless of opcode, which is how a retained Aborted
//!   replay reaches a client (§11: "retained Aborted replay is always a `response|error` frame for
//!   that request opcode containing exactly the stored 48-byte ErrorBody with text length zero");
//! - the `more` flag and a page's next cursor must agree. §8.2 says the next cursor "is zero unless
//!   `more` is set", and a page that disagrees is describing a snapshot the client cannot follow.

use crate::control::{ClockStatus, ConfigBlock, DeviceStatus, Echo, ResetStoreResult};
use crate::download::DownloadAccepted;
use crate::draft::{BeginDraftAccepted, DraftPartAccepted, FinalizeDraftAccepted};
use crate::error::{DecodeError, ErrorBody};
use crate::frame::{ControlFrame, FrameFlags, Opcode};
use crate::hello::{Capabilities, CapabilityPage};
use crate::ids::RequestId;
use crate::query::{CatalogPage, DraftPage, OperationStatus, WeatherRequestContext};
use crate::result::ResultEnvelope;
use crate::upload::{AbortSessionOutcome, CheckpointAccepted, UploadAccepted};
use crate::{BufferTooSmall, EncodeResult};

/// One decoded control response.
///
/// The variants differ in size — a `DraftPage` inlines its six 68-byte entries — and that is the
/// design rather than an oversight. §8.3 bounds a draft page at six entries, so the page is a fixed
/// array with no allocation behind it; boxing the large variant, which is clippy's usual advice,
/// would need an allocator this crate deliberately does not have and would cost the type its
/// `Copy`. A caller that cares decodes the page type it asked for directly.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Response<'a> {
    /// A Capabilities page.
    Capabilities(Capabilities),
    /// An UploadAccepted, in either disposition.
    UploadAccepted(UploadAccepted),
    /// A durable checkpoint.
    CheckpointAccepted(CheckpointAccepted),
    /// A FinishUpload's typed terminal result.
    UploadResult(ResultEnvelope),
    /// A DownloadAccepted.
    DownloadAccepted(DownloadAccepted),
    /// FinishDownload's empty success, which releases the lease exactly once.
    DownloadFinished,
    /// AbortSession's one-byte outcome.
    SessionAborted(AbortSessionOutcome),
    /// A BeginDraft acceptance or retained result.
    BeginDraftAccepted(BeginDraftAccepted),
    /// A DraftPartAccepted acceptance or retained result.
    DraftPartAccepted(DraftPartAccepted),
    /// A FinalizeDraft acceptance or retained result.
    FinalizeDraftAccepted(FinalizeDraftAccepted),
    /// A QueryOperation status.
    OperationStatus(OperationStatus<'a>),
    /// A QueryCatalog page.
    CatalogPage(CatalogPage<'a>),
    /// A QueryDraft page.
    DraftPage(DraftPage),
    /// The durable weather request context.
    WeatherRequestContext(WeatherRequestContext),
    /// A direct mutation's typed terminal result.
    MutationResult(ResultEnvelope),
    /// GetDeviceStatus.
    DeviceStatus(DeviceStatus),
    /// GetConfig or SetConfig — both return the block as it stands after the request.
    Config(ConfigBlock),
    /// SetClock.
    ClockStatus(ClockStatus),
    /// ForgetBond's empty success.
    BondForgotten,
    /// Echo's byte-identical payload.
    Echo(Echo<'a>),
    /// ResetStore's new StoreId.
    ResetStoreResult(ResetStoreResult),
    /// An error body, for any opcode.
    Error(ErrorBody<'a>),
}

impl<'a> Response<'a> {
    /// The flags a frame carrying this response sets, for a given paging decision.
    pub fn flags(&self, more: bool) -> FrameFlags {
        if matches!(self, Response::Error(_)) {
            FrameFlags::ERR
        } else if more {
            FrameFlags::OK_MORE
        } else {
            FrameFlags::OK
        }
    }

    /// Decodes the response a frame carries.
    pub fn decode(frame: &ControlFrame<'a>) -> crate::Result<Self> {
        if !frame.flags.is_response() {
            return Err(DecodeError::invalid_combination());
        }
        let payload = frame.payload;
        if frame.flags.is_error() {
            return Ok(Response::Error(ErrorBody::decode(payload)?));
        }
        let more = frame.flags.has_more();
        Ok(match frame.opcode {
            Opcode::Hello => {
                let page = Capabilities::decode(payload)?;
                // §5: the resource page sets `more` when subjects exist; a subject page sets it
                // when another subject page exists.
                let expected_more = match page.page {
                    CapabilityPage::Resources(_) => page.total_subject_count > 0,
                    CapabilityPage::Subjects { .. } => u16::from(page.page_index) + 1 < u16::from(page.total_pages),
                };
                if more != expected_more {
                    return Err(DecodeError::invalid_combination());
                }
                Response::Capabilities(page)
            }
            Opcode::StartUpload => Response::UploadAccepted(UploadAccepted::decode(payload)?),
            Opcode::CheckpointUpload => Response::CheckpointAccepted(CheckpointAccepted::decode(payload)?),
            Opcode::FinishUpload => Response::UploadResult(ResultEnvelope::decode(payload)?),
            Opcode::StartDownload => Response::DownloadAccepted(DownloadAccepted::decode(payload)?),
            Opcode::FinishDownload => {
                DecodeError::exact_len(payload, 0)?;
                Response::DownloadFinished
            }
            Opcode::AbortSession => Response::SessionAborted(AbortSessionOutcome::decode(payload)?),
            Opcode::BeginDraft => Response::BeginDraftAccepted(BeginDraftAccepted::decode(payload)?),
            Opcode::StartDraftPart => Response::DraftPartAccepted(DraftPartAccepted::decode(payload)?),
            Opcode::FinalizeDraft => Response::FinalizeDraftAccepted(FinalizeDraftAccepted::decode(payload)?),
            Opcode::QueryOperation => Response::OperationStatus(OperationStatus::decode(payload)?),
            Opcode::QueryCatalog => {
                let page = CatalogPage::decode(payload)?;
                if more == page.next_cursor.is_zero() {
                    return Err(DecodeError::invalid_combination());
                }
                Response::CatalogPage(page)
            }
            Opcode::QueryDraft => {
                let page = DraftPage::decode(payload)?;
                if more == page.next_cursor.is_zero() {
                    return Err(DecodeError::invalid_combination());
                }
                Response::DraftPage(page)
            }
            Opcode::QueryWeatherRequest => Response::WeatherRequestContext(WeatherRequestContext::decode(payload)?),
            Opcode::DeleteObject | Opcode::SetMetadata | Opcode::InstallUpdate | Opcode::AcknowledgeRideImported => {
                Response::MutationResult(ResultEnvelope::decode(payload)?)
            }
            Opcode::AbortOperation => Response::MutationResult(ResultEnvelope::decode(payload)?),
            Opcode::GetDeviceStatus => Response::DeviceStatus(DeviceStatus::decode(payload)?),
            Opcode::GetConfig | Opcode::SetConfig => Response::Config(ConfigBlock::decode(payload)?),
            Opcode::SetClock => Response::ClockStatus(ClockStatus::decode(payload)?),
            Opcode::ForgetBond => {
                DecodeError::exact_len(payload, 0)?;
                Response::BondForgotten
            }
            Opcode::Echo => Response::Echo(Echo::decode(payload)?),
            Opcode::ResetStore => Response::ResetStoreResult(ResetStoreResult::decode(payload)?),
        })
    }

    /// Encodes just the payload into `out`, returning its exact length.
    pub fn encode_payload(&self, out: &mut [u8]) -> EncodeResult {
        fn fixed(out: &mut [u8], bytes: &[u8]) -> EncodeResult {
            if out.len() < bytes.len() {
                return Err(BufferTooSmall { needed: bytes.len(), available: out.len() });
            }
            out[..bytes.len()].copy_from_slice(bytes);
            Ok(bytes.len())
        }
        match self {
            Response::Capabilities(body) => body.encode_into(out),
            Response::UploadAccepted(body) => body.encode_into(out),
            Response::CheckpointAccepted(body) => fixed(out, &body.encode()),
            Response::UploadResult(body) | Response::MutationResult(body) => body.encode_into(out),
            Response::DownloadAccepted(body) => fixed(out, &body.encode()),
            Response::DownloadFinished | Response::BondForgotten => Ok(0),
            Response::SessionAborted(body) => fixed(out, &body.encode()),
            Response::BeginDraftAccepted(body) => body.encode_into(out),
            Response::DraftPartAccepted(body) => body.encode_into(out),
            Response::FinalizeDraftAccepted(body) => body.encode_into(out),
            Response::OperationStatus(body) => body.encode_into(out),
            Response::CatalogPage(body) => body.encode_into(out),
            Response::DraftPage(body) => body.encode_into(out),
            Response::WeatherRequestContext(body) => fixed(out, &body.encode()),
            Response::DeviceStatus(body) => fixed(out, &body.encode()),
            Response::Config(body) => fixed(out, &body.encode()),
            Response::ClockStatus(body) => fixed(out, &body.encode()),
            Response::Echo(body) => body.encode_into(out),
            Response::ResetStoreResult(body) => fixed(out, &body.encode()),
            Response::Error(body) => body.encode_into(out),
        }
    }

    /// Encodes a complete control record for `opcode`, echoing `request_id`.
    ///
    /// The opcode is a parameter because a response does not carry one of its own: it echoes its
    /// request's, and an `ErrorBody` is answerable for any opcode.
    pub fn encode_frame(&self, opcode: Opcode, request_id: RequestId, more: bool, out: &mut [u8]) -> EncodeResult {
        if out.len() < crate::frame::HEADER_LEN {
            return Err(BufferTooSmall { needed: crate::frame::HEADER_LEN, available: out.len() });
        }
        let payload_len = self.encode_payload(&mut out[crate::frame::HEADER_LEN..])?;
        let mut header = [0u8; crate::frame::HEADER_LEN];
        ControlFrame::encode_into(&mut header, opcode, self.flags(more), request_id, &[])?;
        out[..crate::frame::HEADER_LEN].copy_from_slice(&header);
        crate::codec::put_u16(out, 10, payload_len as u16);
        Ok(crate::frame::HEADER_LEN + payload_len)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::{presence, ErrorCategory, Owner, RetryGuidance};
    use crate::hello::{status_flags, LinkKind, ResourceLimits, COMMAND_FLAGS_ALL, DEFAULT_CHECKPOINT_GRANULE};
    use crate::ids::StoreId;
    use crate::metadata::{MAX_CATALOG_ENVELOPE, MAX_PUT_ENVELOPE};
    use crate::query::CatalogCursor;
    use crate::registry::ObjectKind;
    use std::vec;

    fn capabilities(page: CapabilityPage, total_subject_count: u16, page_index: u8, total_pages: u8) -> Capabilities {
        Capabilities {
            selected_major: crate::WIRE_MAJOR,
            storage_format_version: crate::STORAGE_FORMAT_VERSION,
            status_flags: status_flags::STORE_AVAILABLE | status_flags::AUTHENTICATED,
            store_id: StoreId::new([0xA5; 16]),
            negotiated_control_frame: 244,
            negotiated_stream_frame: 1024,
            checkpoint_granule: DEFAULT_CHECKPOINT_GRANULE,
            retained_result_capacity: 64,
            metadata_envelope_limit: MAX_PUT_ENVELOPE as u16,
            catalog_metadata_limit: MAX_CATALOG_ENVELOPE as u16,
            protocol_min_control_frame: 192,
            protocol_min_stream_frame: 64,
            link_kind: LinkKind::Usb,
            authenticated: true,
            capability_revision: 4,
            command_flags: COMMAND_FLAGS_ALL,
            total_subject_count,
            page_index,
            total_pages,
            device_wire_minor: 0,
            page,
        }
    }

    #[test]
    fn an_error_body_decodes_for_any_opcode() {
        let body = ErrorBody {
            owner: Owner::LOCAL_PRODUCER,
            presence: presence::DURABLE_CLAIM_EXISTS,
            ..ErrorBody::bare(ErrorCategory::BUSY, 1, RetryGuidance::RETRY_AFTER_OWNER_RELEASE)
        };
        let response = Response::Error(body);
        let mut out = vec![0u8; 256];
        let len = response.encode_frame(Opcode::StartUpload, RequestId::new(5).unwrap(), false, &mut out).unwrap();
        let frame = ControlFrame::decode(&out[..len]).unwrap();
        assert!(frame.flags.is_error());
        assert_eq!(Response::decode(&frame).unwrap(), response);
    }

    #[test]
    fn the_more_flag_must_agree_with_the_resource_page_and_the_subject_pages() {
        let mut out = vec![0u8; 256];

        // A resource page on a device that advertises subjects sets `more`.
        let page = capabilities(CapabilityPage::Resources(ResourceLimits::REFERENCE), 3, 0, 1);
        let response = Response::Capabilities(page);
        let len = response.encode_frame(Opcode::Hello, RequestId::new(1).unwrap(), true, &mut out).unwrap();
        assert!(Response::decode(&ControlFrame::decode(&out[..len]).unwrap()).is_ok());
        let len = response.encode_frame(Opcode::Hello, RequestId::new(1).unwrap(), false, &mut out).unwrap();
        assert_eq!(
            Response::decode(&ControlFrame::decode(&out[..len]).unwrap()).unwrap_err(),
            DecodeError::invalid_combination()
        );

        // A zero-subject device's resource page clears it.
        let page = capabilities(CapabilityPage::Resources(ResourceLimits::REFERENCE), 0, 0, 1);
        let response = Response::Capabilities(page);
        let len = response.encode_frame(Opcode::Hello, RequestId::new(1).unwrap(), false, &mut out).unwrap();
        assert!(Response::decode(&ControlFrame::decode(&out[..len]).unwrap()).is_ok());
    }

    #[test]
    fn a_catalog_page_must_carry_a_next_cursor_exactly_when_more_is_set() {
        let page = CatalogPage {
            store_id: StoreId::new([1; 16]),
            kind: ObjectKind::Route,
            revision: crate::ids::Revision::new(7),
            next_cursor: CatalogCursor::ZERO,
            entry_count: 0,
            entry_bytes: &[],
        };
        let mut out = vec![0u8; 256];
        let response = Response::CatalogPage(page);
        let len = response.encode_frame(Opcode::QueryCatalog, RequestId::new(1).unwrap(), false, &mut out).unwrap();
        assert!(Response::decode(&ControlFrame::decode(&out[..len]).unwrap()).is_ok());

        let len = response.encode_frame(Opcode::QueryCatalog, RequestId::new(1).unwrap(), true, &mut out).unwrap();
        assert_eq!(
            Response::decode(&ControlFrame::decode(&out[..len]).unwrap()).unwrap_err(),
            DecodeError::invalid_combination()
        );

        // A continuing page carries a cursor whose CRC binds it to this page's own store, and the
        // decoder verifies it: an invented CRC is `checksumFailure/cursor`, not a followable cursor.
        let mut cursor = CatalogCursor { revision: 7, next_entry_index: 3, kind_code: 1, crc32: 0x1234 };
        let continuing = CatalogPage { next_cursor: cursor, ..page };
        let response = Response::CatalogPage(continuing);
        let len = response.encode_frame(Opcode::QueryCatalog, RequestId::new(1).unwrap(), true, &mut out).unwrap();
        assert_eq!(
            Response::decode(&ControlFrame::decode(&out[..len]).unwrap()).unwrap_err(),
            DecodeError::new(crate::ErrorCategory::CHECKSUM_FAILURE, crate::error::detail::checksum::CURSOR)
        );

        cursor.crc32 = cursor.catalog_crc(page.store_id);
        let continuing = CatalogPage { next_cursor: cursor, ..page };
        let response = Response::CatalogPage(continuing);
        let len = response.encode_frame(Opcode::QueryCatalog, RequestId::new(1).unwrap(), true, &mut out).unwrap();
        assert!(Response::decode(&ControlFrame::decode(&out[..len]).unwrap()).is_ok());
    }

    #[test]
    fn a_request_frame_is_not_a_response() {
        let mut out = vec![0u8; 64];
        let len = ControlFrame::encode_into(
            &mut out,
            Opcode::FinishDownload,
            FrameFlags::REQUEST,
            RequestId::new(1).unwrap(),
            &[0u8; 16],
        )
        .unwrap();
        let frame = ControlFrame::decode(&out[..len]).unwrap();
        assert_eq!(Response::decode(&frame).unwrap_err(), DecodeError::invalid_combination());
    }
}
