//! The request side of the control link: one typed message per opcode.
//!
//! [`Request::decode`] takes a validated [`ControlFrame`] and returns the message its opcode names,
//! or the contract's own refusal. It is the total, bounded entry point an adapter calls; everything
//! it dispatches to lives in the per-area modules.

use crate::control::{Echo, ForgetBond, ResetStore, SetClock};
use crate::download::{FinishDownload, StartDownload};
use crate::draft::{BeginDraft, FinalizeDraft, StartDraftPart};
use crate::error::DecodeError;
use crate::frame::{ControlFrame, FrameFlags, Opcode};
use crate::hello::Hello;
use crate::ids::{OperationId, RequestId};
use crate::intent::CanonicalIntent;
use crate::mutate::{AcknowledgeRideImported, DeleteObject, InstallUpdate, SetMetadata};
use crate::query::{QueryCatalog, QueryDraft, QueryOperation};
use crate::upload::{AbortOperation, AbortSession, CheckpointUpload, FinishUpload, StartUpload};
use crate::{BufferTooSmall, EncodeResult};

/// One decoded control request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Request<'a> {
    /// `0x0001` — negotiate, or page capability discovery.
    Hello(Hello),
    /// `0x0100` — a logical-object Put.
    StartUpload(StartUpload<'a>),
    /// `0x0101` — make a payload prefix durable.
    CheckpointUpload(CheckpointUpload),
    /// `0x0102` — seal, validate, and publish.
    FinishUpload(FinishUpload),
    /// `0x0110` — resolve and pin the current head.
    StartDownload(StartDownload),
    /// `0x0111` — verify and release the lease.
    FinishDownload(FinishDownload),
    /// `0x0120` — detach a session.
    AbortSession(AbortSession),
    /// `0x0130` — open a multipart draft.
    BeginDraft(BeginDraft),
    /// `0x0131` — claim one child part.
    StartDraftPart(StartDraftPart),
    /// `0x0132` — publish the parent manifest.
    FinalizeDraft(FinalizeDraft),
    /// `0x0200` — ask what happened to an OperationId.
    QueryOperation(QueryOperation),
    /// `0x0201` — page a kind's catalog heads.
    QueryCatalog(QueryCatalog),
    /// `0x0202` — page an open draft's children.
    QueryDraft(QueryDraft),
    /// `0x0203` — read the durable weather request context. Its payload is empty.
    QueryWeatherRequest,
    /// `0x0300` — delete a head.
    DeleteObject(DeleteObject),
    /// `0x0301` — patch catalog metadata.
    SetMetadata(SetMetadata<'a>),
    /// `0x0302` — durably cancel an operation.
    AbortOperation(AbortOperation),
    /// `0x0310` — arm an update install.
    InstallUpdate(InstallUpdate),
    /// `0x0311` — acknowledge a ride import.
    AcknowledgeRideImported(AcknowledgeRideImported),
    /// `0x0400` — device identity and diagnostics. Its payload is empty.
    GetDeviceStatus,
    /// `0x0401` — read the config block. Its payload is empty.
    GetConfig,
    /// `0x0402` — write the whole config block.
    SetConfig(crate::control::ConfigBlock),
    /// `0x0403` — offer a trusted time.
    SetClock(SetClock),
    /// `0x0404` — remove bonding material. BLE only.
    ForgetBond(ForgetBond),
    /// `0x0405` — link bring-up and throughput measurement.
    Echo(Echo<'a>),
    /// `0x0406` — destroy the store and create a new StoreId.
    ResetStore(ResetStore),
}

impl<'a> Request<'a> {
    /// The opcode this request carries.
    pub const fn opcode(&self) -> Opcode {
        match self {
            Request::Hello(_) => Opcode::Hello,
            Request::StartUpload(_) => Opcode::StartUpload,
            Request::CheckpointUpload(_) => Opcode::CheckpointUpload,
            Request::FinishUpload(_) => Opcode::FinishUpload,
            Request::StartDownload(_) => Opcode::StartDownload,
            Request::FinishDownload(_) => Opcode::FinishDownload,
            Request::AbortSession(_) => Opcode::AbortSession,
            Request::BeginDraft(_) => Opcode::BeginDraft,
            Request::StartDraftPart(_) => Opcode::StartDraftPart,
            Request::FinalizeDraft(_) => Opcode::FinalizeDraft,
            Request::QueryOperation(_) => Opcode::QueryOperation,
            Request::QueryCatalog(_) => Opcode::QueryCatalog,
            Request::QueryDraft(_) => Opcode::QueryDraft,
            Request::QueryWeatherRequest => Opcode::QueryWeatherRequest,
            Request::DeleteObject(_) => Opcode::DeleteObject,
            Request::SetMetadata(_) => Opcode::SetMetadata,
            Request::AbortOperation(_) => Opcode::AbortOperation,
            Request::InstallUpdate(_) => Opcode::InstallUpdate,
            Request::AcknowledgeRideImported(_) => Opcode::AcknowledgeRideImported,
            Request::GetDeviceStatus => Opcode::GetDeviceStatus,
            Request::GetConfig => Opcode::GetConfig,
            Request::SetConfig(_) => Opcode::SetConfig,
            Request::SetClock(_) => Opcode::SetClock,
            Request::ForgetBond(_) => Opcode::ForgetBond,
            Request::Echo(_) => Opcode::Echo,
            Request::ResetStore(_) => Opcode::ResetStore,
        }
    }

    /// The OperationId this request claims, when it claims one.
    ///
    /// FinalizeDraft is deliberately absent: §11 makes it address an existing claim by OperationId
    /// alone, so its parent identifier is a lookup key rather than a new claim.
    pub fn claimed_operation_id(&self) -> Option<OperationId> {
        Some(match self {
            Request::StartUpload(request) => request.operation_id,
            Request::BeginDraft(request) => request.parent_operation_id,
            Request::StartDraftPart(request) => request.child_operation_id,
            Request::DeleteObject(request) => request.target.operation_id,
            Request::SetMetadata(request) => request.target.operation_id,
            Request::AbortOperation(request) => request.operation_id,
            Request::InstallUpdate(request) => request.operation_id,
            Request::AcknowledgeRideImported(request) => request.operation_id,
            _ => return None,
        })
    }

    /// The canonical intent whose SHA-256 keys this request's claim (§11), when it has one.
    pub fn canonical_intent(&self, store_id: crate::ids::StoreId) -> Option<CanonicalIntent> {
        Some(match self {
            Request::StartUpload(request) => CanonicalIntent::for_start_upload(store_id, request),
            Request::BeginDraft(request) => CanonicalIntent::for_begin_draft(store_id, request),
            Request::StartDraftPart(request) => CanonicalIntent::for_start_draft_part(store_id, request),
            Request::DeleteObject(request) => CanonicalIntent::for_delete_object(store_id, request),
            Request::SetMetadata(request) => CanonicalIntent::for_set_metadata(store_id, request),
            Request::AbortOperation(request) => CanonicalIntent::for_abort_operation(store_id, request),
            Request::InstallUpdate(request) => CanonicalIntent::for_install_update(store_id, request),
            Request::AcknowledgeRideImported(request) => {
                CanonicalIntent::for_acknowledge_ride_imported(store_id, request)
            }
            _ => return None,
        })
    }

    /// Decodes the request a frame carries.
    pub fn decode(frame: &ControlFrame<'a>) -> crate::Result<Self> {
        if frame.flags.is_response() {
            return Err(DecodeError::invalid_combination());
        }
        let payload = frame.payload;
        Ok(match frame.opcode {
            Opcode::Hello => Request::Hello(Hello::decode(payload)?),
            Opcode::StartUpload => Request::StartUpload(StartUpload::decode(payload)?),
            Opcode::CheckpointUpload => Request::CheckpointUpload(CheckpointUpload::decode(payload)?),
            Opcode::FinishUpload => Request::FinishUpload(FinishUpload::decode(payload)?),
            Opcode::StartDownload => Request::StartDownload(StartDownload::decode(payload)?),
            Opcode::FinishDownload => Request::FinishDownload(FinishDownload::decode(payload)?),
            Opcode::AbortSession => Request::AbortSession(AbortSession::decode(payload)?),
            Opcode::BeginDraft => Request::BeginDraft(BeginDraft::decode(payload)?),
            Opcode::StartDraftPart => Request::StartDraftPart(StartDraftPart::decode(payload)?),
            Opcode::FinalizeDraft => Request::FinalizeDraft(FinalizeDraft::decode(payload)?),
            Opcode::QueryOperation => Request::QueryOperation(QueryOperation::decode(payload)?),
            Opcode::QueryCatalog => Request::QueryCatalog(QueryCatalog::decode(payload)?),
            Opcode::QueryDraft => Request::QueryDraft(QueryDraft::decode(payload)?),
            Opcode::QueryWeatherRequest => {
                DecodeError::exact_len(payload, 0)?;
                Request::QueryWeatherRequest
            }
            Opcode::DeleteObject => Request::DeleteObject(DeleteObject::decode(payload)?),
            Opcode::SetMetadata => Request::SetMetadata(SetMetadata::decode(payload)?),
            Opcode::AbortOperation => Request::AbortOperation(AbortOperation::decode(payload)?),
            Opcode::InstallUpdate => Request::InstallUpdate(InstallUpdate::decode(payload)?),
            Opcode::AcknowledgeRideImported => {
                Request::AcknowledgeRideImported(AcknowledgeRideImported::decode(payload)?)
            }
            Opcode::GetDeviceStatus => {
                DecodeError::exact_len(payload, 0)?;
                Request::GetDeviceStatus
            }
            Opcode::GetConfig => {
                DecodeError::exact_len(payload, 0)?;
                Request::GetConfig
            }
            Opcode::SetConfig => Request::SetConfig(crate::control::ConfigBlock::decode(payload)?),
            Opcode::SetClock => Request::SetClock(SetClock::decode(payload)?),
            Opcode::ForgetBond => Request::ForgetBond(ForgetBond::decode(payload)?),
            Opcode::Echo => Request::Echo(Echo::decode(payload)?),
            Opcode::ResetStore => Request::ResetStore(ResetStore::decode(payload)?),
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
            Request::Hello(request) => fixed(out, &request.encode()),
            Request::StartUpload(request) => request.encode_into(out),
            Request::CheckpointUpload(request) => fixed(out, &request.encode()),
            Request::FinishUpload(request) => fixed(out, &request.encode()),
            Request::StartDownload(request) => fixed(out, &request.encode()),
            Request::FinishDownload(request) => fixed(out, &request.encode()),
            Request::AbortSession(request) => fixed(out, &request.encode()),
            Request::BeginDraft(request) => fixed(out, &request.encode()),
            Request::StartDraftPart(request) => fixed(out, &request.encode()),
            Request::FinalizeDraft(request) => fixed(out, &request.encode()),
            Request::QueryOperation(request) => fixed(out, &request.encode()),
            Request::QueryCatalog(request) => fixed(out, &request.encode()),
            Request::QueryDraft(request) => fixed(out, &request.encode()),
            Request::QueryWeatherRequest | Request::GetDeviceStatus | Request::GetConfig => Ok(0),
            Request::DeleteObject(request) => fixed(out, &request.encode()),
            Request::SetMetadata(request) => request.encode_into(out),
            Request::AbortOperation(request) => fixed(out, &request.encode()),
            Request::InstallUpdate(request) => fixed(out, &request.encode()),
            Request::AcknowledgeRideImported(request) => fixed(out, &request.encode()),
            Request::SetConfig(request) => fixed(out, &request.encode()),
            Request::SetClock(request) => fixed(out, &request.encode()),
            Request::ForgetBond(request) => fixed(out, &request.encode()),
            Request::Echo(request) => request.encode_into(out),
            Request::ResetStore(request) => fixed(out, &request.encode()),
        }
    }

    /// Encodes a complete control record: header plus payload.
    pub fn encode_frame(&self, request_id: RequestId, out: &mut [u8]) -> EncodeResult {
        if out.len() < crate::frame::HEADER_LEN {
            return Err(BufferTooSmall { needed: crate::frame::HEADER_LEN, available: out.len() });
        }
        let payload_len = self.encode_payload(&mut out[crate::frame::HEADER_LEN..])?;
        let mut header = [0u8; crate::frame::HEADER_LEN];
        crate::frame::ControlFrame::encode_into(&mut header, self.opcode(), FrameFlags::REQUEST, request_id, &[])?;
        out[..crate::frame::HEADER_LEN].copy_from_slice(&header);
        crate::codec::put_u16(out, 10, payload_len as u16);
        Ok(crate::frame::HEADER_LEN + payload_len)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hello::PageKind;
    use std::vec;

    #[test]
    fn every_opcode_round_trips_through_a_whole_frame() {
        let hello = Request::Hello(Hello {
            minimum_major: 3,
            maximum_major: 3,
            client_max_control_frame: 244,
            client_max_stream_frame: 1024,
            client_feature_flags: 0,
            page_kind: PageKind::Resources,
            page_index: 0,
        });
        let mut out = vec![0u8; 512];
        let len = hello.encode_frame(RequestId::new(1).unwrap(), &mut out).unwrap();
        let frame = ControlFrame::decode(&out[..len]).unwrap();
        assert_eq!(frame.opcode, Opcode::Hello);
        assert_eq!(Request::decode(&frame).unwrap(), hello);
        assert_eq!(len, crate::frame::HEADER_LEN + 12);
    }

    #[test]
    fn the_three_empty_payload_requests_reject_a_body() {
        for opcode in [Opcode::QueryWeatherRequest, Opcode::GetDeviceStatus, Opcode::GetConfig] {
            let mut out = vec![0u8; 32];
            let len =
                ControlFrame::encode_into(&mut out, opcode, FrameFlags::REQUEST, RequestId::new(2).unwrap(), &[0u8; 1])
                    .unwrap();
            let frame = ControlFrame::decode(&out[..len]).unwrap();
            assert_eq!(Request::decode(&frame).unwrap_err(), DecodeError::trailing_bytes());
        }
    }

    #[test]
    fn a_response_frame_is_not_a_request() {
        let mut out = vec![0u8; 64];
        let len = ControlFrame::encode_into(&mut out, Opcode::Echo, FrameFlags::OK, RequestId::new(3).unwrap(), b"hi")
            .unwrap();
        let frame = ControlFrame::decode(&out[..len]).unwrap();
        assert_eq!(Request::decode(&frame).unwrap_err(), DecodeError::invalid_combination());
    }

    #[test]
    fn exactly_the_eight_claiming_opcodes_report_an_operation_id_and_an_intent() {
        let store = crate::ids::StoreId::new([1; 16]);
        let claiming: std::vec::Vec<Opcode> = Opcode::ALL.iter().copied().filter(|o| o.claims_operation()).collect();
        assert_eq!(claiming.len(), 8);

        let query = Request::QueryOperation(QueryOperation { operation_id: OperationId::new([1; 16]) });
        assert!(query.claimed_operation_id().is_none());
        assert!(query.canonical_intent(store).is_none());

        let finalize = Request::FinalizeDraft(FinalizeDraft { parent_operation_id: OperationId::new([2; 16]) });
        assert!(finalize.claimed_operation_id().is_none());
        assert!(finalize.canonical_intent(store).is_none());

        let delete = Request::DeleteObject(DeleteObject {
            target: crate::mutate::MutationTarget {
                operation_id: OperationId::new([3; 16]),
                kind: crate::registry::ObjectKind::Route,
                logical_object_id: crate::ids::LogicalObjectId::new(1),
                expected_revision: crate::ids::Revision::new(2),
            },
        });
        assert_eq!(delete.claimed_operation_id(), Some(OperationId::new([3; 16])));
        assert!(delete.canonical_intent(store).is_some());
    }
}
