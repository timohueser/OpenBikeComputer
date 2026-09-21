//! The transfer engine for `FLAT_Store_Protocol.md`: one state machine, both links.
//!
//! It owns no transport and no medium. A record comes in, a [`Reaction`] names the bytes to send,
//! and every card access goes through [`Store`]. An adapter hands the engine whole records and then
//! pumps [`Engine::poll`] until it answers [`Reaction::Idle`]; a driver that stops pumping stalls a
//! download. [`Engine::on_link_lost`] is not optional: it is the third form of cancel.
//!
//! One transfer at a time; a second is `busy`. Any break before the commit releases the allocation
//! and leaves the card unchanged, and the client restarts from zero. There is no resume and no
//! session.

use obc_crc::Crc32;

use super::ids::{DisplayName, EntryFlags, EntryMeta, ObjectId, ObjectKind, Revision};
use super::store::{ArchiveError, ArchiveSource, Mode, Mutation, Policy, PutSource, Store, StoreError};
use super::wire::{
    decode_request, detail, encode_archive, encode_arm, encode_cancel, encode_error, encode_format, encode_get,
    encode_put, encode_remove, encode_status, write_stream, ArmRequest, ControlError, ErrorCode, FormatRequest,
    GetRequest, ListRequest, ListWriter, ObjectState, Opcode, PutRequest, Refusal, RemoveRequest, Request, RequestId,
    StatusRequest, StatusResponse, StreamFrame, CONTROL_FLOOR, STREAM_HEADER_LEN,
};

/// The staging buffer a transfer fills before it reaches the card, in bytes. Whole 512-byte blocks
/// leave an allocation in one media write, so the stage must be a multiple of the block size.
pub const DEFAULT_STAGE: usize = 4_096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Channel {
    /// One control frame per record, in strict request/response order.
    Control,
    Stream,
}

/// What the engine wants done after one record or one pump.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reaction {
    Idle,
    /// Send `out[..len]` on this channel.
    Send {
        channel: Channel,
        len: usize,
    },
    /// Send `out[..len]` on the control channel, drain the link, then reboot.
    SendAndReboot {
        len: usize,
    },
    Close(Channel),
}

/// Why the device drops a transfer of its own accord.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CancelCause {
    /// A device-local decision, or a transfer that stopped moving ([`Engine::watch_stall`]).
    Device,
    /// The transfer's own channel died under a link that can still carry the answer. A link that
    /// went away entirely is [`Engine::on_link_lost`], which answers nobody.
    LinkLost,
}

impl CancelCause {
    fn detail(self) -> u16 {
        match self {
            CancelCause::Device => detail::cancelled::BY_DEVICE,
            CancelCause::LinkLost => detail::cancelled::LINK_LOST,
        }
    }
}

/// How long the live transfer may move no bytes before the engine gives up on it, in ms. It bounds
/// the gap between two byte-moving records, never a transfer's duration: the three terms are the
/// three waits that may legitimately sit between two records.
pub const STALL_TIMEOUT_MS: u32 = 15_000 + 4_000 + 2_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stall {
    Idle,
    Watching {
        again_in_ms: u32,
    },
    /// The transfer was abandoned for want of progress. The engine is idle and the transfer's
    /// `cancelled` answer is owed on the next [`poll`](Engine::poll).
    Abandoned(RequestId),
}

/// Where the live transfer stood the last time it moved, and when that was.
#[derive(Debug, Clone, Copy)]
struct Anchor {
    request: RequestId,
    bytes: u64,
    /// The caller's clock. Compared with `wrapping_sub`, so its ~49-day wrap is a non-event.
    at_ms: u32,
}

/// The record ceilings the binding imposes. A link whose control records cannot carry a header, a
/// `LIST` prefix and one entry cannot carry this protocol, and the adapter refuses it rather than
/// truncating.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ceilings {
    control: usize,
    stream: usize,
}

impl Ceilings {
    /// The two ceilings, or `None` for a link below the protocol floor.
    pub const fn new(control: usize, stream: usize) -> Option<Self> {
        // `then_some` is not `const`; the `if` is the same statement and is.
        if control >= CONTROL_FLOOR && stream > STREAM_HEADER_LEN {
            Some(Ceilings { control, stream })
        } else {
            None
        }
    }

    /// The control ceiling is `ATT_MTU - 3` and the stream ceiling is the CoC SDU. Both are clamped
    /// to `buffer`, the adapter's reaction buffer, because the engine frames a `LIST` page and a
    /// stream record against these numbers.
    pub fn for_ble(att_mtu: usize, coc_sdu: usize, buffer: usize) -> Option<Self> {
        let control = att_mtu.checked_sub(3)?.min(buffer);
        let stream = coc_sdu.min(buffer);
        Ceilings::new(control, stream)
    }

    /// A USB binding negotiates nothing — a bulk endpoint's max packet is a packet size, and its
    /// records span packets by design — so both ceilings are the buffer the adapter frames into.
    pub const fn for_usb(record: usize) -> Option<Self> {
        Ceilings::new(record, record)
    }

    pub const fn control(&self) -> usize {
        self.control
    }

    pub const fn stream(&self) -> usize {
        self.stream
    }
}

/// The adapter's admission latch.
///
/// A client may stream a `PUT` without waiting for an acceptance, so the first stream frame of a
/// transfer races its own control frame. The adapter must hold a frame that is not admitted yet:
/// the engine discards it in silence and the upload dies at offset zero. The latch remembers which
/// `RequestId` was admitted, so a continuation skips the query and the first frame of the next
/// transfer on the same channel is queried again.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Admission {
    admitted: Option<RequestId>,
}

impl Admission {
    pub fn new() -> Self {
        Admission { admitted: None }
    }

    /// True when the engine must be consulted before this frame may be delivered.
    pub fn needs_query(&self, frame: RequestId) -> bool {
        self.admitted != Some(frame)
    }

    /// Record what the engine reported while `frame` was waiting. Returns true when the frame must
    /// be held, because it is not admitted yet.
    pub fn observed(&mut self, frame: RequestId, live: Option<RequestId>) -> bool {
        if live == Some(frame) {
            self.admitted = Some(frame);
            return false;
        }
        // Not this transfer. Forget any earlier admission too, or a later frame goes through on a
        // stale identity.
        self.admitted = None;
        true
    }

    pub fn reset(&mut self) {
        self.admitted = None;
    }
}

/// What a live upload has landed so far. A multi-megabyte upload answers the wire once, at the end,
/// so a device that shows progress reads the engine directly instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UploadProgress {
    pub request: RequestId,
    pub kind: ObjectKind,
    /// Payload bytes absorbed so far.
    pub received: u64,
    /// The payload length the `PUT` declared.
    pub declared: u64,
}

pub struct StreamBuffers<'record, 'out> {
    record: &'record [u8],
    out: &'out mut [u8],
}

impl<'record, 'out> StreamBuffers<'record, 'out> {
    pub fn new(record: &'record [u8], out: &'out mut [u8]) -> Self {
        Self { record, out }
    }
}

pub struct UsbMapBatch<'stage> {
    request: RequestId,
    offset: u64,
    len: usize,
    bank: usize,
    stage: &'stage mut [u8],
}

impl<'stage> UsbMapBatch<'stage> {
    pub fn new(request: RequestId, offset: u64, len: usize, bank: usize, stage: &'stage mut [u8]) -> Self {
        Self { request, offset, len, bank, stage }
    }
}

/// How the last upload ended, latched once and taken once. The fact exists for one instant, the
/// call that commits or refuses, so a device that only looks between calls would otherwise see an
/// upload vanish with no verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UploadEnd {
    /// The commit landed. `id` is the resulting head, and `replaced` distinguishes a create from a
    /// replace at the same id, so a board can invalidate state derived from the displaced revision.
    Committed { id: ObjectId, replaced: bool },
    /// The transfer was refused, with the code its error response carried.
    Refused(ErrorCode),
}

/// Which wire a call arrived on. One engine serves both links and almost nothing it does needs to
/// know which. Three things do: ceilings are per link, a link coming up must not disturb the other
/// link's transfer, and a link going away releases only what that link held.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Link {
    Ble,
    Usb,
}

impl Link {
    const fn index(self) -> usize {
        match self {
            Link::Ble => 0,
            Link::Usb => 1,
        }
    }
}

struct Upload<A> {
    /// The link that admitted it. Only this link may stream into it, pump it, or lose it.
    link: Link,
    /// Captured at admission: the numbers a transfer frames against must not change under it.
    ceilings: Ceilings,
    request: RequestId,
    /// The object being replaced, or [`ObjectId::NONE`] for a create, whose id the commit assigns.
    id: ObjectId,
    revision: Revision,
    kind: ObjectKind,
    name: DisplayName,
    declared_len: u64,
    declared_crc: u32,
    /// The head this replaces, if any. Re-checked immediately before the commit.
    displaced: Option<Revision>,
    received: u64,
    staged: usize,
    /// Which half of a double-width adapter stage is currently being filled.
    stage_bank: usize,
    /// Whole-payload verification, for every link and kind but a USB map shard: the cable already
    /// protects and retries each packet in hardware, and a second pass throttles that path.
    crc: Option<Crc32>,
    allocation: A,
}

struct Download<H> {
    link: Link,
    ceilings: Ceilings,
    request: RequestId,
    revision: Revision,
    payload_len: u64,
    payload_crc: u32,
    sent: u64,
    handle: H,
}

enum Live<S: Store> {
    Idle,
    Upload(Upload<S::Allocation>),
    Download(Download<S::Handle>),
}

/// A response the engine still owes a transfer it has already dropped: a cancelled `PUT` or `GET`
/// receives its own error response, and the `CANCEL` receives a different one.
#[derive(Debug, Clone, Copy)]
struct Owed {
    /// The link the answer is owed to. It cannot be pumped out of the other one.
    link: Link,
    opcode: Opcode,
    request: RequestId,
    refusal: Refusal,
}

pub struct Engine<S: Store, const STAGE: usize = DEFAULT_STAGE> {
    /// What each link negotiated, indexed by [`Link::index`]. `None` while that link is down; a
    /// link with no ceilings cannot be served.
    ceilings: [Option<Ceilings>; 2],
    live: Live<S>,
    stall: Option<Anchor>,
    owed: Option<Owed>,
    upload_end: Option<(ObjectKind, UploadEnd)>,
    staging: [u8; STAGE],
}

impl<S: Store, const STAGE: usize> Default for Engine<S, STAGE> {
    fn default() -> Self {
        Engine::new()
    }
}

impl<S: Store, const STAGE: usize> Engine<S, STAGE> {
    /// An idle engine with no link up. Each link announces itself with
    /// [`on_link_up`](Engine::on_link_up) and is served only while it has.
    pub fn new() -> Self {
        const {
            assert!(STAGE >= 512 && STAGE.is_multiple_of(512), "the stage is whole 512-byte blocks");
        }
        Engine {
            ceilings: [None, None],
            live: Live::Idle,
            stall: None,
            owed: None,
            upload_end: None,
            staging: [0; STAGE],
        }
    }

    fn link_ceilings(&self, link: Link) -> Option<Ceilings> {
        self.ceilings[link.index()]
    }

    fn live_link(&self) -> Option<Link> {
        match &self.live {
            Live::Idle => None,
            Live::Upload(upload) => Some(upload.link),
            Live::Download(download) => Some(download.link),
        }
    }

    pub fn live_transfer(&self) -> Option<RequestId> {
        match &self.live {
            Live::Idle => None,
            Live::Upload(upload) => Some(upload.request),
            Live::Download(download) => Some(download.request),
        }
    }

    fn live_bytes(&self) -> u64 {
        match &self.live {
            Live::Idle => 0,
            Live::Upload(upload) => upload.received,
            Live::Download(download) => download.sent,
        }
    }

    /// One turn of the transfer stall watchdog. Nothing else bounds how long a transfer stays live.
    ///
    /// `now_ms` is the caller's monotonic millisecond clock; the engine has none. Call this after
    /// every engine call and again when [`Stall::Watching`]'s deadline arrives. It re-anchors
    /// whenever the transfer moved a byte, and abandons a still one as
    /// [`cancel_live`](Engine::cancel_live) does.
    pub fn watch_stall(&mut self, store: &S, now_ms: u32) -> Stall {
        let Some(request) = self.live_transfer() else { return Stall::Idle };
        let bytes = self.live_bytes();
        let anchor = match self.stall {
            Some(anchor) if anchor.request == request && anchor.bytes == bytes => anchor,
            _ => *self.stall.insert(Anchor { request, bytes, at_ms: now_ms }),
        };
        match STALL_TIMEOUT_MS.checked_sub(now_ms.wrapping_sub(anchor.at_ms)) {
            Some(0) | None => {
                self.cancel_live(store, CancelCause::Device);
                Stall::Abandoned(request)
            }
            Some(again_in_ms) => Stall::Watching { again_in_ms },
        }
    }

    pub fn live_upload(&self) -> Option<UploadProgress> {
        match &self.live {
            Live::Upload(upload) => Some(UploadProgress {
                request: upload.request,
                kind: upload.kind,
                received: upload.received,
                declared: upload.declared_len,
            }),
            _ => None,
        }
    }

    /// Whether this exact upload owns the engine. Adapter-specific resources must use this rather
    /// than [`UploadProgress`], which omits the owner link and so cannot prove that USB, not BLE,
    /// may claim a cable-only staging arena.
    pub fn upload_matches(&self, link: Link, request: RequestId, kind: ObjectKind) -> bool {
        matches!(
            &self.live,
            Live::Upload(upload) if upload.link == link && upload.request == request && upload.kind == kind
        )
    }

    /// The declared length when this exact upload owns the engine. The USB adapter reads it once,
    /// after a map `PUT` is admitted, to know when a batch holds the transfer's final byte.
    pub fn upload_declared_len(&self, link: Link, request: RequestId, kind: ObjectKind) -> Option<u64> {
        match &self.live {
            Live::Upload(upload) if upload.link == link && upload.request == request && upload.kind == kind => {
                Some(upload.declared_len)
            }
            _ => None,
        }
    }

    /// Take the verdict on the last upload, clearing it. A link that goes away leaves nothing here,
    /// because that form of cancel answers nobody.
    pub fn take_upload_end(&mut self) -> Option<(ObjectKind, UploadEnd)> {
        self.upload_end.take()
    }

    /// True while nothing is owed and nothing is live: what an adapter checks before it sleeps.
    pub fn is_quiet(&self) -> bool {
        self.owed.is_none() && matches!(self.live, Live::Idle)
    }

    pub fn on_control<P: Policy>(
        &mut self,
        link: Link,
        store: &S,
        policy: &mut P,
        record: &[u8],
        out: &mut [u8],
    ) -> Reaction {
        // A link with no ceilings is a wire nobody is on; it cannot be framed to.
        let Some(ceilings) = self.link_ceilings(link) else { return Reaction::Idle };
        let (header, request) = match decode_request(record) {
            Ok(decoded) => decoded,
            // A zero `RequestId` is unanswerable, and so is a record too short to carry one.
            Err(ControlError::Unanswerable) => return Reaction::Close(Channel::Control),
            Err(ControlError::Refused { request, refusal }) => {
                // A frame this malformed has no opcode to echo; `LIST` is what a client sends first.
                let opcode = Opcode::decode(record.get(5).copied().unwrap_or(0)).unwrap_or(Opcode::List);
                return self.emit_error(out, opcode, request, refusal);
            }
        };
        match request {
            Request::List(list) => self.on_list(store, ceilings, header.request, list, out),
            Request::Status(status) => self.on_status(store, header.request, status, out),
            Request::Get(get) => self.on_get(store, link, ceilings, header.request, get, out),
            Request::Put(put) => self.on_put(store, link, ceilings, header.request, put, out),
            Request::Remove(remove) => self.on_remove(store, header.request, remove, out),
            Request::Cancel(cancel) => self.on_cancel(store, link, header.request, cancel.transfer, out),
            Request::Arm(arm) => self.on_arm(store, policy, header.request, arm, out),
            Request::Format(format) => self.on_format(store, header.request, format, out),
            Request::ArchiveRide(source) => self.on_archive(store, header.request, source, out),
        }
    }

    fn on_archive(&mut self, store: &S, request: RequestId, source: ArchiveSource, out: &mut [u8]) -> Reaction {
        if let Some(refusal) = self.busy_refusal().or_else(|| read_refusal(store.mode())) {
            return self.emit_error(out, Opcode::ArchiveRide, request, refusal);
        }
        match store.archive_ride(source) {
            Ok(result) => match encode_archive(out, request, result) {
                Some(len) => Reaction::Send { channel: Channel::Control, len },
                None => Reaction::Close(Channel::Control),
            },
            Err(error) => {
                let refusal = match error {
                    ArchiveError::Unsupported => Refusal::plain(ErrorCode::Unsupported),
                    ArchiveError::SourceMismatch => bad_combination(),
                    ArchiveError::Store(StoreError::Media) => Refusal::plain(ErrorCode::MediaIo),
                    ArchiveError::Store(error) => {
                        write_refusal(store.mode()).unwrap_or_else(|| allocate_refusal(error, 0))
                    }
                };
                self.emit_error(out, Opcode::ArchiveRide, request, refusal)
            }
        }
    }

    fn on_format(&mut self, store: &S, request: RequestId, format: FormatRequest, out: &mut [u8]) -> Reaction {
        if let Err(refusal) = self.admit_format(store, format) {
            return self.emit_error(out, Opcode::Format, request, refusal);
        }
        let len = match store.format(format.replacement) {
            Ok(()) => encode_format(out, request, format.replacement),
            Err(error) => encode_error(out, Opcode::Format, request, &media_refusal(error, detail::media_io::WRITE)),
        };
        // Formatting invalidates the old superblocks first, so the in-memory store must not serve
        // again after this answer leaves the link, whether the format succeeded or not.
        match len {
            Some(len) => Reaction::SendAndReboot { len },
            None => Reaction::Close(Channel::Control),
        }
    }

    fn admit_format(&mut self, store: &S, format: FormatRequest) -> Result<(), Refusal> {
        if let Some(refusal) = self.busy_refusal() {
            return Err(refusal);
        }
        let expected = if store.mode().readable() { store.store_id() } else { super::ids::StoreId([0; 16]) };
        if format.expected != expected {
            return Err(bad_combination());
        }
        Ok(())
    }

    /// One whole stream record arrived: a 16-byte frame followed by exactly its payload.
    pub fn on_stream<P: Policy>(
        &mut self,
        link: Link,
        store: &S,
        policy: &mut P,
        record: &[u8],
        out: &mut [u8],
    ) -> Reaction {
        self.on_stream_with_stage(link, store, policy, record, out, None)
    }

    /// The bank the cable adapter must lend to [`on_stream_staged`](Self::on_stream_staged). The
    /// index comes before the borrow so an arena-backed adapter forms `&mut` for the inactive half
    /// alone; deferred card DMA may still hold the other half.
    pub fn upload_stage_bank(&self) -> Option<usize> {
        match &self.live {
            Live::Upload(upload) => Some(upload.stage_bank),
            _ => None,
        }
    }

    /// One whole stream record, using `stage` as the current bank of this upload's two-bank
    /// write-combining buffer, so several records reach [`Store::write`] as one contiguous run.
    /// Radio adapters use [`on_stream`](Self::on_stream) and the engine's resident stage.
    ///
    /// The bank must be a non-zero multiple of 512 bytes, and the same index and length must be
    /// supplied until it fills. An adapter that loses its scratch must cancel the transfer:
    /// stopping part-way changes the storage beneath `Upload::staged`.
    pub fn on_stream_staged<P: Policy>(
        &mut self,
        link: Link,
        store: &S,
        policy: &mut P,
        buffers: StreamBuffers<'_, '_>,
        bank: usize,
        stage: &mut [u8],
    ) -> Reaction {
        let StreamBuffers { record, out } = buffers;
        if stage.len() < 512 || !stage.len().is_multiple_of(512) || self.upload_stage_bank() != Some(bank) {
            return Reaction::Close(Channel::Stream);
        }
        self.on_stream_with_stage(link, store, policy, record, out, Some((bank, stage)))
    }

    /// Commit one adapter-assembled USB map batch already resident in the current arena bank. The
    /// adapter still validates record framing; this seam only amortises the storage-owner crossing.
    pub fn on_usb_map_batch<P: Policy>(
        &mut self,
        store: &S,
        policy: &mut P,
        batch: UsbMapBatch<'_>,
        out: &mut [u8],
    ) -> Reaction {
        let UsbMapBatch { request, offset, len, bank, stage } = batch;
        if stage.len() < 512
            || !stage.len().is_multiple_of(512)
            || self.upload_stage_bank() != Some(bank)
            || len == 0
            || len > stage.len()
        {
            return Reaction::Close(Channel::Stream);
        }
        if self.live_link() != Some(Link::Usb) {
            return Reaction::Idle;
        }
        let Live::Upload(upload) = &mut self.live else { return Reaction::Idle };
        if upload.request != request {
            return Reaction::Idle;
        }
        if upload.kind != ObjectKind::MapShard || upload.staged != 0 {
            return self.fail_upload(store, Refusal::plain(ErrorCode::Internal), out);
        }
        if offset != upload.received || offset + len as u64 > upload.declared_len {
            return self.fail_upload(
                store,
                Refusal::new(ErrorCode::InvalidRequest, detail::invalid_request::STREAM_OFFSET),
                out,
            );
        }
        if let Some(crc) = &mut upload.crc {
            crc.update(&stage[..len]);
        }
        if let Err(error) = store.write(&mut upload.allocation, &stage[..len]) {
            return self.fail_upload(store, media_refusal(error, detail::media_io::WRITE), out);
        }
        upload.received += len as u64;
        upload.stage_bank ^= 1;
        if upload.received < upload.declared_len {
            Reaction::Idle
        } else {
            self.finish_upload(store, policy, out, None)
        }
    }

    fn on_stream_with_stage<P: Policy>(
        &mut self,
        link: Link,
        store: &S,
        policy: &mut P,
        record: &[u8],
        out: &mut [u8],
        mut stage: Option<(usize, &mut [u8])>,
    ) -> Reaction {
        // The silent discard, one step earlier: bytes on a wire that owns no transfer, including
        // the other link's wire, belong to no transfer this can be sure of.
        if self.live_link() != Some(link) {
            return Reaction::Idle;
        }
        let Some((frame, payload)) = StreamFrame::split(record) else {
            // A malformed stream record terminates the transfer, and there is exactly one live.
            let Live::Upload(upload) = &self.live else { return Reaction::Idle };
            let request = upload.request;
            self.abandon(store);
            return self.emit_error(
                out,
                Opcode::Put,
                request,
                Refusal::new(ErrorCode::InvalidFrame, detail::invalid_frame::LENGTH),
            );
        };
        // A frame whose `RequestId` is not the live upload's is discarded in silence, and so is one
        // bearing a live download's: those bytes go the other way.
        let Live::Upload(upload) = &self.live else { return Reaction::Idle };
        if frame.transfer != upload.request {
            return Reaction::Idle;
        }
        let (offset, declared) = (upload.received, upload.declared_len);
        if frame.len as usize > upload.ceilings.stream - STREAM_HEADER_LEN {
            return self.fail_upload(store, Refusal::new(ErrorCode::InvalidFrame, detail::invalid_frame::LENGTH), out);
        }
        // Frames are contiguous and ascending. A gap and an overlap are the same refusal.
        if frame.offset != offset || offset + payload.len() as u64 > declared {
            let refusal = Refusal::new(ErrorCode::InvalidRequest, detail::invalid_request::STREAM_OFFSET);
            return self.fail_upload(store, refusal, out);
        }
        let staged = stage.as_mut().map(|(bank, bytes)| (*bank, &mut **bytes));
        if let Err(error) = self.absorb(store, payload, staged) {
            return self.fail_upload(store, media_refusal(error, detail::media_io::WRITE), out);
        }
        if offset + (payload.len() as u64) < declared {
            return Reaction::Idle;
        }
        self.finish_upload(store, policy, out, stage)
    }

    /// Pumps the engine: a live download's next record, or an error owed to a dropped transfer.
    /// A driver calls this until it answers [`Reaction::Idle`].
    pub fn poll(&mut self, link: Link, store: &S, out: &mut [u8]) -> Reaction {
        if self.owed.is_some_and(|owed| owed.link == link) {
            let owed = self.owed.take().expect("just checked");
            return self.emit_error(out, owed.opcode, owed.request, owed.refusal);
        }
        let Live::Download(download) = &self.live else { return Reaction::Idle };
        // Only the link being served pumps its own download. The other one asking is not an error.
        if download.link != link {
            return Reaction::Idle;
        }
        let (request, revision, payload_len, payload_crc, offset, stream_ceiling) = (
            download.request,
            download.revision,
            download.payload_len,
            download.payload_crc,
            download.sent,
            download.ceilings.stream,
        );
        if offset >= payload_len {
            // Every byte has reached the transport, so the answer goes out and the hold is released.
            self.abandon(store);
            return match encode_get(out, request, revision, payload_len, payload_crc) {
                Some(len) => Reaction::Send { channel: Channel::Control, len },
                None => Reaction::Idle,
            };
        }
        // A buffer that cannot hold a frame and one payload byte would stall the download forever.
        if out.len() <= STREAM_HEADER_LEN {
            return self.fail_download(store, Refusal::plain(ErrorCode::Internal), out);
        }
        let room = out.len().min(stream_ceiling) - STREAM_HEADER_LEN;
        let want = room.min((payload_len - offset) as usize);
        let read = store.read(&download.handle, offset, &mut out[STREAM_HEADER_LEN..STREAM_HEADER_LEN + want]);
        match read {
            // The length is the catalog's, not the reader's, so a short read is a media failure.
            Ok(0) => self.fail_download(store, media_refusal(StoreError::Media, detail::media_io::READ), out),
            Ok(read) => {
                if let Live::Download(download) = &mut self.live {
                    download.sent += read as u64;
                }
                match write_stream(out, request, offset, read) {
                    Some(len) => Reaction::Send { channel: Channel::Stream, len },
                    None => Reaction::Idle,
                }
            }
            Err(error) => self.fail_download(store, media_refusal(error, detail::media_io::READ), out),
        }
    }

    /// A link came up with these ceilings. It touches `link` and nothing else.
    ///
    /// A link coming up is a new peer on one wire, not a new state of the device. A transfer this
    /// link owned is released, because its peer is gone. A transfer the other link owns is left
    /// alone, and the newcomer's own `PUT` or `GET` meets the one-at-a-time rule as `busy`.
    pub fn on_link_up(&mut self, link: Link, store: &S, ceilings: Ceilings) {
        if self.live_link() == Some(link) {
            self.abandon(store);
        }
        if self.owed.is_some_and(|owed| owed.link == link) {
            self.owed = None;
        }
        self.ceilings[link.index()] = Some(ceilings);
    }

    /// The link went away: the third form of cancel, scoped to the link that went.
    ///
    /// Nothing is answered, because nobody is left to answer. The other link's transfer is not
    /// touched: a cable being unplugged is not a reason to kill a phone's download.
    pub fn on_link_lost(&mut self, link: Link, store: &S) {
        if self.live_link() == Some(link) {
            self.abandon(store);
        }
        if self.owed.is_some_and(|owed| owed.link == link) {
            self.owed = None;
        }
        self.ceilings[link.index()] = None;
    }

    /// The device's half of the bilateral cancel: answer the outstanding `PUT` or `GET` with an
    /// error and drop the transfer. Reports whether there was one.
    ///
    /// The transfer's `cancelled` answer goes out on the next [`poll`](Engine::poll). The caller is
    /// a device-local decision, not a wire request, so there is no second response to pair with it.
    pub fn cancel_live(&mut self, store: &S, cause: CancelCause) -> bool {
        let Some(request) = self.live_transfer() else { return false };
        let opcode = if matches!(self.live, Live::Upload(_)) { Opcode::Put } else { Opcode::Get };
        // Read the owning link before the abandon that forgets it: the answer is owed to the wire
        // the transfer was on.
        let link = self.live_link().expect("live_transfer just answered Some");
        self.abandon(store);
        self.owed = Some(Owed { link, opcode, request, refusal: Refusal::new(ErrorCode::Cancelled, cause.detail()) });
        true
    }

    fn on_list(
        &mut self,
        store: &S,
        ceilings: Ceilings,
        request: RequestId,
        list: ListRequest,
        out: &mut [u8],
    ) -> Reaction {
        if let Some(refusal) = read_refusal(store.mode()) {
            return self.emit_error(out, Opcode::List, request, refusal);
        }
        let sequence = store.commit_sequence();
        if let Some(cursor) = list.cursor {
            if cursor.sequence != sequence {
                let refusal =
                    Refusal::with_context(ErrorCode::CatalogChanged, detail::catalog_changed::LISTING, sequence);
                return self.emit_error(out, Opcode::List, request, refusal);
            }
        }
        let ceiling = ceilings.control;
        let Some(mut writer) = ListWriter::start(out, ceiling, store.store_id(), sequence) else {
            return self.emit_error(out, Opcode::List, request, Refusal::plain(ErrorCode::Internal));
        };
        let after = list.cursor.map(|cursor| (cursor.id, cursor.revision));
        let mut more = false;
        for meta in store.entries() {
            if after.is_some_and(|cursor| meta.key() <= cursor) {
                continue;
            }
            if list.kind.is_some_and(|kind| kind != meta.kind) {
                continue;
            }
            if !writer.push(out, &meta) {
                more = true;
                break;
            }
        }
        if !store.entries_ok() {
            let refusal = media_refusal(StoreError::Media, detail::media_io::READ);
            return self.emit_error(out, Opcode::List, request, refusal);
        }
        match writer.finish(out, request, more) {
            Some(len) => Reaction::Send { channel: Channel::Control, len },
            None => self.emit_error(out, Opcode::List, request, Refusal::plain(ErrorCode::Internal)),
        }
    }

    fn on_status(&mut self, store: &S, request: RequestId, status: StatusRequest, out: &mut [u8]) -> Reaction {
        if let Some(refusal) = read_refusal(store.mode()) {
            return self.emit_error(out, Opcode::Status, request, refusal);
        }
        let found = lookup(store, status.id);
        if !store.entries_ok() {
            let refusal = media_refusal(StoreError::Media, detail::media_io::READ);
            return self.emit_error(out, Opcode::Status, request, refusal);
        }
        let answer = match found.head {
            None => StatusResponse::absent(),
            Some(head) => StatusResponse {
                state: if head.revision == status.revision { ObjectState::Committed } else { ObjectState::Superseded },
                revision: head.revision,
                payload_len: head.payload_len,
                payload_crc: head.payload_crc,
            },
        };
        match encode_status(out, request, &answer) {
            Some(len) => Reaction::Send { channel: Channel::Control, len },
            None => self.emit_error(out, Opcode::Status, request, Refusal::plain(ErrorCode::Internal)),
        }
    }

    fn on_get(
        &mut self,
        store: &S,
        link: Link,
        ceilings: Ceilings,
        request: RequestId,
        get: GetRequest,
        out: &mut [u8],
    ) -> Reaction {
        if let Some(refusal) = self.busy_refusal() {
            return self.emit_error(out, Opcode::Get, request, refusal);
        }
        if let Some(refusal) = read_refusal(store.mode()) {
            return self.emit_error(out, Opcode::Get, request, refusal);
        }
        let found = lookup(store, get.id);
        if !store.entries_ok() {
            let refusal = media_refusal(StoreError::Media, detail::media_io::READ);
            return self.emit_error(out, Opcode::Get, request, refusal);
        }
        let wanted = match get.revision {
            Revision::HEAD => found.head,
            revision => [found.retained, found.head].into_iter().flatten().find(|meta| meta.revision == revision),
        };
        let Some(meta) = wanted else {
            let detail = if found.head.is_none() { detail::not_found::OBJECT } else { detail::not_found::REVISION };
            return self.emit_error(out, Opcode::Get, request, Refusal::new(ErrorCode::NotFound, detail));
        };
        // A reserve has no bytes on the card, and a recording ride's length and CRC are zero until
        // the commit that ends it, so serving either reports success over an empty payload.
        if meta.flags.is_untouchable() {
            let refusal = Refusal::new(ErrorCode::InvalidRequest, detail::invalid_request::BAD_COMBINATION);
            return self.emit_error(out, Opcode::Get, request, refusal);
        }
        let handle = match store.open(meta.id, Some(meta.revision)) {
            Ok(handle) => handle,
            Err(error) => {
                // A full hold table is `busy`, not a refusal of the request.
                let refusal = open_refusal(error);
                return self.emit_error(out, Opcode::Get, request, refusal);
            }
        };
        self.live = Live::Download(Download {
            link,
            ceilings,
            request,
            revision: meta.revision,
            payload_len: meta.payload_len,
            payload_crc: meta.payload_crc,
            sent: 0,
            handle,
        });
        // Send the first record now, so that `Idle` keeps meaning "nothing to do".
        self.poll(link, store, out)
    }

    fn on_put(
        &mut self,
        store: &S,
        link: Link,
        ceilings: Ceilings,
        request: RequestId,
        put: PutRequest,
        out: &mut [u8],
    ) -> Reaction {
        match self.admit_put(store, link, ceilings, request, put) {
            Ok(()) => Reaction::Idle,
            Err(refusal) => {
                if refusal.code == ErrorCode::NoSpace {
                    self.upload_end = Some((put.kind, UploadEnd::Refused(refusal.code)));
                }
                self.emit_error(out, Opcode::Put, request, refusal)
            }
        }
    }

    /// Every check that must pass before a byte is allocated for.
    fn admit_put(
        &mut self,
        store: &S,
        link: Link,
        ceilings: Ceilings,
        request: RequestId,
        put: PutRequest,
    ) -> Result<(), Refusal> {
        if let Some(refusal) = self.busy_refusal() {
            return Err(refusal);
        }
        if let Some(refusal) = write_refusal(store.mode()) {
            return Err(refusal);
        }
        // An object with no bytes is a `Remove`, not a `Put`: an entry may not own unneeded extents.
        if put.payload_len == 0 {
            return Err(bad_combination());
        }
        // Device-owned kinds are produced by the device, whether the request creates or replaces.
        if put.kind.is_device_owned() {
            return Err(bad_combination());
        }
        let (id, revision, displaced) = if put.id.is_some() {
            let found = lookup(store, put.id);
            if !store.entries_ok() {
                return Err(media_refusal(StoreError::Media, detail::media_io::READ));
            }
            let Some(head) = found.head else {
                return Err(Refusal::new(ErrorCode::RevisionConflict, detail::revision_conflict::HEAD_ABSENT));
            };
            if head.revision != put.expected {
                return Err(Refusal::with_context(
                    ErrorCode::RevisionConflict,
                    detail::revision_conflict::HEAD_DIFFERS,
                    head.revision.0,
                ));
            }
            if head.kind == ObjectKind::Metadata || head.flags.is_untouchable() {
                return Err(bad_combination());
            }
            if head.kind != put.kind {
                return Err(bad_combination());
            }
            let Some(next) = head.revision.next() else {
                return Err(Refusal::new(ErrorCode::ReadOnly, detail::read_only::REVISION_SPACE_EXHAUSTED));
            };
            (put.id, next, Some(head.revision))
        } else {
            // A create names no id until it commits: `next_object_id` reserves nothing, so a
            // device-local commit could take an id pinned here. The cursor never rewinds, so
            // reading it at the commit is both fresh and free.
            (ObjectId::NONE, Revision::FIRST, None)
        };
        let allocation = store.allocate(put.payload_len).map_err(|error| allocate_refusal(error, put.payload_len))?;
        self.live = Live::Upload(Upload {
            link,
            ceilings,
            request,
            id,
            revision,
            kind: put.kind,
            name: put.name,
            declared_len: put.payload_len,
            declared_crc: put.payload_crc,
            displaced,
            received: 0,
            staged: 0,
            stage_bank: 0,
            crc: (!(link == Link::Usb && put.kind == ObjectKind::MapShard)).then(Crc32::new),
            allocation,
        });
        Ok(())
    }

    fn on_remove(&mut self, store: &S, request: RequestId, remove: RemoveRequest, out: &mut [u8]) -> Reaction {
        match self.apply_remove(store, remove) {
            Ok(sequence) => match encode_remove(out, request, sequence) {
                Some(len) => Reaction::Send { channel: Channel::Control, len },
                None => self.emit_error(out, Opcode::Remove, request, Refusal::plain(ErrorCode::Internal)),
            },
            Err(refusal) => self.emit_error(out, Opcode::Remove, request, refusal),
        }
    }

    fn apply_remove(&mut self, store: &S, remove: RemoveRequest) -> Result<u64, Refusal> {
        if let Some(refusal) = write_refusal(store.mode()) {
            return Err(refusal);
        }
        let found = lookup(store, remove.id);
        if !store.entries_ok() {
            return Err(media_refusal(StoreError::Media, detail::media_io::READ));
        }
        let Some(head) = found.head else {
            return Err(Refusal::new(ErrorCode::NotFound, detail::not_found::OBJECT));
        };
        if head.revision != remove.expected {
            return Err(Refusal::with_context(
                ErrorCode::RevisionConflict,
                detail::revision_conflict::HEAD_DIFFERS,
                head.revision.0,
            ));
        }
        if head.kind == ObjectKind::Metadata || head.flags.is_untouchable() {
            return Err(bad_combination());
        }
        // A retained previous revision of the same object goes with the head.
        let head_mutation = Mutation::Remove { id: head.id, revision: head.revision };
        let sequence = match found.retained {
            None => store.commit(&[head_mutation]),
            Some(retained) => {
                store.commit(&[head_mutation, Mutation::Remove { id: retained.id, revision: retained.revision }])
            }
        };
        // A removal frees space rather than needing any, so `noSpace`'s context is zero here.
        sequence.map_err(|error| commit_refusal(error, 0))
    }

    fn on_cancel(
        &mut self,
        store: &S,
        link: Link,
        request: RequestId,
        transfer: RequestId,
        out: &mut [u8],
    ) -> Reaction {
        // The identifier and the wire both. `RequestId` spaces are per client, so two links can
        // pick the same small number. A `CANCEL` naming a transfer the asking link does not own
        // answers `cancelled = false`: there is no such transfer of yours.
        let live = self.live_transfer();
        let cancelled = live == Some(transfer) && self.live_link() == Some(link);
        if cancelled {
            let opcode = if matches!(self.live, Live::Upload(_)) { Opcode::Put } else { Opcode::Get };
            self.abandon(store);
            // The cancelled transfer receives its own error response, on the next pump.
            self.owed = Some(Owed {
                link,
                opcode,
                request: transfer,
                refusal: Refusal::new(ErrorCode::Cancelled, detail::cancelled::BY_CLIENT),
            });
        }
        match encode_cancel(out, request, cancelled) {
            Some(len) => Reaction::Send { channel: Channel::Control, len },
            None => self.emit_error(out, Opcode::Cancel, request, Refusal::plain(ErrorCode::Internal)),
        }
    }

    fn on_arm<P: Policy>(
        &mut self,
        store: &S,
        policy: &mut P,
        request: RequestId,
        arm: ArmRequest,
        out: &mut [u8],
    ) -> Reaction {
        match self.apply_arm(store, policy, arm) {
            Ok((reserve, sequence)) => match encode_arm(out, request, reserve, sequence) {
                // The answer must reach the transport before the reboot.
                Some(len) => Reaction::SendAndReboot { len },
                None => self.emit_error(out, Opcode::Arm, request, Refusal::plain(ErrorCode::Internal)),
            },
            Err(refusal) => self.emit_error(out, Opcode::Arm, request, refusal),
        }
    }

    fn apply_arm<P: Policy>(&mut self, store: &S, policy: &mut P, arm: ArmRequest) -> Result<(ObjectId, u64), Refusal> {
        if let Some(refusal) = self.busy_refusal() {
            return Err(refusal);
        }
        if let Some(refusal) = write_refusal(store.mode()) {
            return Err(refusal);
        }
        let found = lookup(store, arm.package);
        if !store.entries_ok() {
            return Err(media_refusal(StoreError::Media, detail::media_io::READ));
        }
        let Some(head) = found.head else {
            return Err(Refusal::new(ErrorCode::NotFound, detail::not_found::OBJECT));
        };
        if head.revision != arm.expected {
            return Err(Refusal::with_context(
                ErrorCode::RevisionConflict,
                detail::revision_conflict::HEAD_DIFFERS,
                head.revision.0,
            ));
        }
        if head.kind != ObjectKind::UpdatePackage {
            return Err(bad_combination());
        }
        // Every refusal here is `rejected` with the update kind's detail, and changes nothing.
        let bytes = policy
            .validate_package(head.id, head.revision)
            .map_err(|reason| Refusal::new(ErrorCode::Rejected, reason))?;
        // One `RESERVED` entry with enough extents for the running image. This is the one commit
        // `ARM` makes, and it exists because the bootloader cannot allocate.
        let allocation = store.allocate(bytes).map_err(|error| allocate_refusal(error, bytes))?;
        let reserve = EntryMeta {
            id: store.next_object_id(),
            revision: Revision::FIRST,
            kind: ObjectKind::RollbackReserve,
            flags: EntryFlags::RESERVED,
            payload_len: 0,
            payload_crc: 0,
            name: DisplayName::default(),
        };
        let sequence = match store.commit(&[Mutation::Put { meta: reserve, source: PutSource::Fresh(allocation) }]) {
            Ok(sequence) => sequence,
            Err(error) => {
                store.cancel(allocation);
                return Err(commit_refusal(error, bytes));
            }
        };
        // A cut here is survivable: the next boot reconciles and removes the reserve. A refusal
        // reaches no reboot, and a `RESERVED` entry cannot be removed from the wire, so it takes its
        // own commit back instead. That is what makes "an error changed nothing" true here.
        if policy.hand_off((head.id, head.revision), (reserve.id, reserve.revision)).is_err() {
            // Best effort: a card that refuses the way back too leaves the reserve for the next
            // boot's reconciliation.
            let _ = store.commit(&[Mutation::Remove { id: reserve.id, revision: reserve.revision }]);
            return Err(Refusal::plain(ErrorCode::Internal));
        }
        Ok((reserve.id, sequence))
    }

    /// Folds one payload into the running CRC and the stage, writing whole stages to the card.
    fn absorb(&mut self, store: &S, payload: &[u8], stage: Option<(usize, &mut [u8])>) -> Result<(), StoreError> {
        let external = stage.is_some();
        let staging: &mut [u8] = match stage {
            Some((_, stage)) => stage,
            None => &mut self.staging,
        };
        let Live::Upload(upload) = &mut self.live else { return Err(StoreError::Invalid) };
        let stage_len = staging.len();
        let base = 0;
        // A record must not fill this bank and need the next one in the same call: the board cannot
        // borrow that next bank until this borrow ends, because the write starts deferred DMA.
        if external && upload.staged + payload.len() > stage_len {
            return Err(StoreError::Invalid);
        }
        if let Some(crc) = &mut upload.crc {
            crc.update(payload);
        }
        upload.received += payload.len() as u64;
        let mut input = payload;
        while !input.is_empty() {
            // A record at or above one stage goes straight to the card, whole aligned prefix in one
            // call: a card write command costs about the same for one block or a hundred.
            if upload.staged == 0 && input.len() >= stage_len {
                let (chunk, rest) = input.split_at(input.len() - input.len() % stage_len);
                store.write(&mut upload.allocation, chunk)?;
                input = rest;
                continue;
            }
            let take = (stage_len - upload.staged).min(input.len());
            staging[base + upload.staged..base + upload.staged + take].copy_from_slice(&input[..take]);
            upload.staged += take;
            input = &input[take..];
            if upload.staged == stage_len {
                store.write(&mut upload.allocation, &staging[base..base + stage_len])?;
                upload.staged = 0;
                if external {
                    upload.stage_bank ^= 1;
                }
            }
        }
        Ok(())
    }

    /// The last byte: verify the whole-payload CRC, run the kind's validator, and commit.
    fn finish_upload<P: Policy>(
        &mut self,
        store: &S,
        policy: &mut P,
        out: &mut [u8],
        stage: Option<(usize, &mut [u8])>,
    ) -> Reaction {
        let Live::Upload(upload) = &self.live else { return Reaction::Idle };
        let (kind, declared_len, declared_crc, replaced) =
            (upload.kind, upload.declared_len, upload.declared_crc, upload.displaced.is_some());
        if let Some(crc) = upload.crc {
            let computed = crc.finalize();
            if computed != declared_crc {
                let refusal = Refusal::with_context(
                    ErrorCode::ChecksumFailure,
                    detail::checksum_failure::PAYLOAD,
                    u64::from(declared_crc),
                );
                return self.fail_upload(store, refusal, out);
            }
        }
        if let Err(reason) = policy.accept(kind, declared_len) {
            return self.fail_upload(store, Refusal::new(ErrorCode::Rejected, reason), out);
        }
        // Everything received is on the card before the commit begins.
        if let Err(error) = self.flush(store, stage) {
            return self.fail_upload(store, media_refusal(error, detail::media_io::WRITE), out);
        }
        let request = self.owed_request();
        match self.publish(store) {
            Ok((id, revision, len, crc)) => {
                // The commit consumed the allocation, so nothing is live from here.
                self.live = Live::Idle;
                self.upload_end = Some((kind, UploadEnd::Committed { id, replaced }));
                match encode_put(out, request, id, revision, len, crc) {
                    Some(len) => Reaction::Send { channel: Channel::Control, len },
                    None => Reaction::Idle,
                }
            }
            Err(refusal) => self.fail_upload(store, refusal, out),
        }
    }

    fn flush(&mut self, store: &S, stage: Option<(usize, &mut [u8])>) -> Result<(), StoreError> {
        let Live::Upload(upload) = &mut self.live else { return Ok(()) };
        if upload.staged == 0 {
            return Ok(());
        }
        let staged = upload.staged;
        let staging: &[u8] = match stage {
            Some((_, stage)) => stage,
            None => &self.staging,
        };
        let base = 0;
        store.write(&mut upload.allocation, &staging[base..base + staged])?;
        upload.staged = 0;
        Ok(())
    }

    /// The `RequestId` the live transfer's answer echoes, or zero when there is none.
    fn owed_request(&self) -> RequestId {
        self.live_transfer().unwrap_or(RequestId(0))
    }

    /// The one commit a `PUT` makes: publish the new head, and settle what it displaced.
    fn publish(&mut self, store: &S) -> Result<(ObjectId, Revision, u64, u32), Refusal> {
        let Live::Upload(upload) = &self.live else { return Err(Refusal::plain(ErrorCode::Internal)) };
        // A create takes its id here rather than at admission: the cursor only moves forward, so
        // reading it at the commit cannot collide with a device-local commit that ran meanwhile.
        let id = if upload.id.is_some() { upload.id } else { store.next_object_id() };
        // The expected `Revision` is checked at admission and again immediately before the commit.
        // For a create the expectation is that nothing names this id at all.
        let found = lookup(store, id);
        if !store.entries_ok() {
            return Err(media_refusal(StoreError::Media, detail::media_io::READ));
        }
        if found.head.map(|meta| meta.revision) != upload.displaced {
            return Err(Refusal::with_context(
                ErrorCode::RevisionConflict,
                detail::revision_conflict::HEAD_DIFFERS,
                found.head.map_or(0, |meta| meta.revision.0),
            ));
        }
        let meta = EntryMeta {
            id,
            revision: upload.revision,
            kind: upload.kind,
            flags: EntryFlags::NONE,
            payload_len: upload.declared_len,
            payload_crc: upload.declared_crc,
            name: upload.name,
        };
        let publish = Mutation::Put { meta, source: PutSource::Fresh(upload.allocation) };
        let displace = found.head.map(|head| Mutation::Remove { id: head.id, revision: head.revision });
        let free = found.retained.map(|meta| Mutation::Remove { id: meta.id, revision: meta.revision });
        let sequence = match (displace, free) {
            (None, _) => store.commit(&[publish]),
            (Some(displace), None) => store.commit(&[publish, displace]),
            (Some(displace), Some(free)) => store.commit(&[publish, displace, free]),
        };
        sequence.map_err(|error| commit_refusal(error, meta.payload_len))?;
        Ok((meta.id, meta.revision, meta.payload_len, meta.payload_crc))
    }

    /// Drops the live transfer and releases what it holds. The one path every abandonment takes.
    fn abandon(&mut self, store: &S) {
        // Every way a transfer ends passes through here, so the stall anchor dies with it and the
        // next transfer starts its deadline from its own first look.
        self.stall = None;
        match core::mem::replace(&mut self.live, Live::Idle) {
            Live::Idle => {}
            Live::Upload(upload) => store.cancel(upload.allocation),
            Live::Download(download) => store.close(download.handle),
        }
    }

    /// Ends the live upload with a refusal: the allocation is released, the written bytes are
    /// anonymous, and the catalog is untouched.
    fn fail_upload(&mut self, store: &S, refusal: Refusal, out: &mut [u8]) -> Reaction {
        let request = self.owed_request();
        // Read the kind before the abandon, which is the statement that forgets it.
        if let Live::Upload(upload) = &self.live {
            self.upload_end = Some((upload.kind, UploadEnd::Refused(refusal.code)));
        }
        self.abandon(store);
        self.emit_error(out, Opcode::Put, request, refusal)
    }

    fn fail_download(&mut self, store: &S, refusal: Refusal, out: &mut [u8]) -> Reaction {
        let request = self.owed_request();
        self.abandon(store);
        self.emit_error(out, Opcode::Get, request, refusal)
    }

    fn emit_error(&mut self, out: &mut [u8], opcode: Opcode, request: RequestId, refusal: Refusal) -> Reaction {
        match encode_error(out, opcode, request, &refusal) {
            Some(len) => Reaction::Send { channel: Channel::Control, len },
            // A buffer too small for a 32-byte error response is below the floor the adapter
            // refuses at connection time. There is nothing to say and nowhere to say it.
            None => Reaction::Idle,
        }
    }

    /// The device serves exactly one `PUT` or `GET`; a second is `busy`, with the live transfer's
    /// own `RequestId` as context.
    fn busy_refusal(&self) -> Option<Refusal> {
        self.live_transfer()
            .map(|live| Refusal::with_context(ErrorCode::Busy, detail::busy::TRANSFER, u64::from(live.0)))
    }
}

/// The entries one `ObjectId` has: a head, and at most one retained revision.
struct Found {
    head: Option<EntryMeta>,
    retained: Option<EntryMeta>,
}

/// Resolves one `ObjectId` out of the catalog view. The caller must check
/// [`entries_ok`](Store::entries_ok) afterwards: a listing that stopped early would make an absent
/// object out of a media failure.
fn lookup<S: Store>(store: &S, id: ObjectId) -> Found {
    let mut found = Found { head: None, retained: None };
    for meta in store.entries() {
        if meta.id != id {
            // The catalog is sorted by `(ObjectId, Revision)`.
            if meta.id > id {
                break;
            }
            continue;
        }
        if meta.flags.has(EntryFlags::RETAINED) {
            found.retained = Some(meta);
        } else {
            found.head = Some(meta);
        }
    }
    found
}

fn bad_combination() -> Refusal {
    Refusal::new(ErrorCode::InvalidRequest, detail::invalid_request::BAD_COMBINATION)
}

/// `readOnly` for an opcode that only reads. Only the two exhausted modes still serve reads.
fn read_refusal(mode: Mode) -> Option<Refusal> {
    if mode.readable() {
        return None;
    }
    Some(Refusal::new(ErrorCode::ReadOnly, read_only_detail(mode)))
}

/// The same for an opcode that commits, which the exhausted modes refuse too.
fn write_refusal(mode: Mode) -> Option<Refusal> {
    if mode.writable() {
        return None;
    }
    Some(Refusal::new(ErrorCode::ReadOnly, read_only_detail(mode)))
}

fn read_only_detail(mode: Mode) -> u16 {
    match mode {
        Mode::ReadWrite => 0,
        Mode::RevisionSpaceExhausted | Mode::SequenceSpaceExhausted => detail::read_only::REVISION_SPACE_EXHAUSTED,
        Mode::CatalogUnreadable => detail::read_only::CATALOG_UNREADABLE,
        // A card that is not a flat store and a card the superblock does not describe are the same
        // answer: there is no flat store here.
        Mode::Unformatted | Mode::CardTooSmall => detail::read_only::UNFORMATTED,
    }
}

/// A refusal from a `write` or a `read`. A store that turns `ReadOnly` mid-transfer says so with
/// detail `0`: it was writable when the transfer was admitted, and there is no narrower fact here.
fn media_refusal(error: StoreError, when: u16) -> Refusal {
    match error {
        StoreError::ReadOnly => Refusal::new(ErrorCode::ReadOnly, 0),
        _ => Refusal::new(ErrorCode::MediaIo, when),
    }
}

/// A refusal from `allocate`. `Invalid` here means a full reservation table, which is transient and
/// answered with `busy`. That reading holds only because the callers rule out every other way
/// `allocate` says `Invalid` first; remove one of those checks and a client's own bad request comes
/// back as "busy, try again" forever.
fn allocate_refusal(error: StoreError, bytes: u64) -> Refusal {
    match error {
        StoreError::NoSpace { required } => {
            Refusal::with_context(ErrorCode::NoSpace, detail::no_space::EXTENTS, required)
        }
        StoreError::TooFragmented => Refusal::with_context(ErrorCode::NoSpace, detail::no_space::TOO_FRAGMENTED, bytes),
        StoreError::CatalogFull => Refusal::with_context(ErrorCode::NoSpace, detail::no_space::CATALOG_FULL, bytes),
        StoreError::Invalid => Refusal::plain(ErrorCode::Busy),
        StoreError::Media => Refusal::new(ErrorCode::MediaIo, detail::media_io::WRITE),
        StoreError::ReadOnly => Refusal::new(ErrorCode::ReadOnly, detail::read_only::REVISION_SPACE_EXHAUSTED),
        // `allocate` takes no hold row, so this is unreachable. It is mapped rather than funnelled
        // into `Internal` so that a store that ever does say it reads as the same transient fact.
        StoreError::Busy => Refusal::new(ErrorCode::Busy, detail::busy::HOLDS),
        StoreError::NotFound | StoreError::RevisionConflict { .. } => Refusal::plain(ErrorCode::Internal),
    }
}

/// A refusal from `open`. A full hold table answers `busy` with the `holds` detail, so a client can
/// tell "another transfer owns the device" from "every read slot is taken". `Invalid` maps to a
/// plain `busy`, and it is also what a `GET` on a `RESERVED` entry produces.
fn open_refusal(error: StoreError) -> Refusal {
    match error {
        StoreError::NotFound => Refusal::new(ErrorCode::NotFound, detail::not_found::OBJECT),
        StoreError::Busy => Refusal::new(ErrorCode::Busy, detail::busy::HOLDS),
        StoreError::Invalid => Refusal::plain(ErrorCode::Busy),
        StoreError::Media => Refusal::new(ErrorCode::MediaIo, detail::media_io::READ),
        StoreError::ReadOnly => Refusal::new(ErrorCode::ReadOnly, detail::read_only::CATALOG_UNREADABLE),
        _ => Refusal::plain(ErrorCode::Internal),
    }
}

/// A refusal from `commit`, which changed nothing. `bytes` is what the mutation needed: the context
/// `noSpace` carries, however the store phrased its refusal.
fn commit_refusal(error: StoreError, bytes: u64) -> Refusal {
    match error {
        StoreError::NotFound => Refusal::new(ErrorCode::NotFound, detail::not_found::OBJECT),
        StoreError::RevisionConflict { current } => {
            Refusal::with_context(ErrorCode::RevisionConflict, detail::revision_conflict::HEAD_DIFFERS, current.0)
        }
        StoreError::NoSpace { required } => {
            Refusal::with_context(ErrorCode::NoSpace, detail::no_space::EXTENTS, required)
        }
        StoreError::TooFragmented => Refusal::with_context(ErrorCode::NoSpace, detail::no_space::TOO_FRAGMENTED, bytes),
        StoreError::CatalogFull => Refusal::with_context(ErrorCode::NoSpace, detail::no_space::CATALOG_FULL, bytes),
        StoreError::Media => Refusal::new(ErrorCode::MediaIo, detail::media_io::SYNC),
        StoreError::ReadOnly => Refusal::new(ErrorCode::ReadOnly, detail::read_only::REVISION_SPACE_EXHAUSTED),
        // The engine built the batch, so a structural refusal is this crate's fault.
        StoreError::Invalid => Refusal::plain(ErrorCode::Internal),
        // A commit takes no hold row either. Same reasoning as `allocate_refusal`'s arm.
        StoreError::Busy => Refusal::new(ErrorCode::Busy, detail::busy::HOLDS),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The rows a card cannot easily be made to produce; the rest are covered over a real card in
    /// `tests/flat_engine.rs`.
    #[test]
    fn a_full_table_is_busy_and_never_invalid_request() {
        assert_eq!(allocate_refusal(StoreError::Invalid, 1).code, ErrorCode::Busy);
        assert_eq!(open_refusal(StoreError::Invalid).code, ErrorCode::Busy);
        // The same variant from a commit is the engine's own batch being wrong: not transient.
        assert_eq!(commit_refusal(StoreError::Invalid, 0).code, ErrorCode::Internal);
    }

    /// The detail is what a client's retry policy reads, and `FLAT_Store_Protocol.md` freezes it.
    #[test]
    fn a_full_hold_table_is_busy_with_the_holds_detail() {
        assert_eq!(open_refusal(StoreError::Busy), Refusal::new(ErrorCode::Busy, detail::busy::HOLDS));
        assert_eq!(allocate_refusal(StoreError::Busy, 1), Refusal::new(ErrorCode::Busy, detail::busy::HOLDS));
        assert_eq!(commit_refusal(StoreError::Busy, 0), Refusal::new(ErrorCode::Busy, detail::busy::HOLDS));
        assert_ne!(detail::busy::HOLDS, detail::busy::TRANSFER, "the two reasons must stay distinguishable");
    }

    #[test]
    fn a_card_that_is_not_the_card_the_superblock_describes_is_unformatted_on_the_wire() {
        for mode in [Mode::Unformatted, Mode::CardTooSmall] {
            assert_eq!(
                read_refusal(mode),
                Some(Refusal::new(ErrorCode::ReadOnly, detail::read_only::UNFORMATTED)),
                "{mode:?} still serves reads"
            );
            assert_eq!(write_refusal(mode).map(|refusal| refusal.detail), Some(detail::read_only::UNFORMATTED));
        }
        assert_eq!(
            read_refusal(Mode::CatalogUnreadable),
            Some(Refusal::new(ErrorCode::ReadOnly, detail::read_only::CATALOG_UNREADABLE))
        );
        for mode in [Mode::RevisionSpaceExhausted, Mode::SequenceSpaceExhausted] {
            assert_eq!(read_refusal(mode), None, "{mode:?} stopped serving reads");
            assert_eq!(
                write_refusal(mode),
                Some(Refusal::new(ErrorCode::ReadOnly, detail::read_only::REVISION_SPACE_EXHAUSTED))
            );
        }
        assert_eq!(read_refusal(Mode::ReadWrite), None);
        assert_eq!(write_refusal(Mode::ReadWrite), None);
    }

    #[test]
    fn every_seam_refusal_carries_the_context_its_code_defines() {
        assert_eq!(allocate_refusal(StoreError::NoSpace { required: 42_137 }, 1).context, 42_137);
        assert_eq!(allocate_refusal(StoreError::TooFragmented, 42_137).context, 42_137);
        assert_eq!(allocate_refusal(StoreError::CatalogFull, 42_137).context, 42_137);
        assert_eq!(commit_refusal(StoreError::RevisionConflict { current: Revision(5) }, 0).context, 5);
        assert_eq!(commit_refusal(StoreError::Media, 0), Refusal::new(ErrorCode::MediaIo, detail::media_io::SYNC));
        // `noSpace` carries one context, the bytes required, however the store phrased its refusal.
        assert_eq!(commit_refusal(StoreError::TooFragmented, 42_137).context, 42_137);
        assert_eq!(commit_refusal(StoreError::CatalogFull, 42_137).context, 42_137);
        assert_eq!(media_refusal(StoreError::Media, detail::media_io::READ).detail, detail::media_io::READ);
        // A store that refuses a write because it is read-only says so rather than blaming media.
        assert_eq!(media_refusal(StoreError::ReadOnly, detail::media_io::WRITE), Refusal::new(ErrorCode::ReadOnly, 0));
    }

    #[test]
    fn the_admission_latch_queries_the_first_frame_of_every_transfer_on_a_channel() {
        let (a, b) = (RequestId(0x2A01), RequestId(0x2A02));
        let mut admission = Admission::new();

        assert!(admission.needs_query(a));
        assert!(admission.observed(a, None), "an idle engine must hold the leading frame");
        assert!(admission.needs_query(a));
        assert!(!admission.observed(a, Some(a)), "the engine admitted it — deliver");
        assert!(!admission.needs_query(a));

        // The second transfer on the same channel. The adapter never sees A end, because a transfer
        // ends on a stream frame, so the latch must not still answer for it.
        assert!(admission.needs_query(b), "B's leading frame must be queried, not waved through on A");
        assert!(admission.observed(b, None), "B is not admitted yet — hold, do not deliver to an idle engine");
        assert!(!admission.observed(b, Some(b)));
        assert!(!admission.needs_query(b));
        assert!(admission.needs_query(a));

        // A frame for another transfer is held, and it clears the latch so the live transfer's next
        // frame is re-queried.
        assert!(admission.observed(a, Some(b)));
        assert!(admission.needs_query(b));

        assert!(!admission.observed(b, Some(b)));
        assert!(!admission.needs_query(b));
        admission.reset();
        assert!(admission.needs_query(b));
        assert_eq!(Admission::new(), Admission::default());
    }

    #[test]
    fn the_ble_binding_reads_its_ceilings_off_the_link_and_the_adapters_buffer() {
        // A 247-byte MTU gives 244 bytes of control.
        let ceilings = Ceilings::for_ble(247, 245, 256).expect("the device's preferred BLE link");
        assert_eq!((ceilings.control(), ceilings.stream()), (244, 245));
        let clamped = Ceilings::for_ble(517, 1_024, 256).expect("a large link, small buffer");
        assert_eq!((clamped.control(), clamped.stream()), (256, 256));
        assert_eq!(Ceilings::for_ble(247, 245, CONTROL_FLOOR - 1), None);
        assert_eq!(Ceilings::for_ble(CONTROL_FLOOR + 2, 245, 4_096), None);
        assert!(Ceilings::for_ble(CONTROL_FLOOR + 3, 245, 4_096).is_some());
        // An `ATT_MTU` too small to subtract the 3-byte header from is `None`, never an underflow.
        for att in [0, 1, 2, 3] {
            assert_eq!(Ceilings::for_ble(att, 245, 4_096), None, "att_mtu {att} underflowed");
        }
        assert_eq!(Ceilings::for_ble(247, STREAM_HEADER_LEN, 4_096), None);
        assert!(Ceilings::for_ble(247, STREAM_HEADER_LEN + 1, 4_096).is_some());
    }

    #[test]
    fn a_link_below_the_protocol_floor_is_refused_rather_than_truncated() {
        assert!(Ceilings::new(CONTROL_FLOOR - 1, 1_024).is_none());
        assert!(Ceilings::new(CONTROL_FLOOR, STREAM_HEADER_LEN).is_none());
        let ceilings = Ceilings::new(244, 1_024).expect("the device's preferred BLE link");
        assert_eq!((ceilings.control(), ceilings.stream()), (244, 1_024));
    }
}
