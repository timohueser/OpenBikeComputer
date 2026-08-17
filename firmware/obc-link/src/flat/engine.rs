//! The transfer engine: one state machine, both links, protocol v4.
//!
//! `FLAT_Store_Protocol.md` §3 is what it speaks and §2 is what it speaks to. It owns **no**
//! transport — a record comes in and what comes back is a [`Reaction`] naming bytes to send — and it
//! owns no medium: every card access goes through [`Store`], which the board binds to the flat
//! store. That is what lets one engine serve BLE and USB and be proved without a radio or a card.
//!
//! ## The driver loop
//!
//! An adapter hands the engine whole records and then pumps it until it goes quiet. Ten lines:
//!
//! ```text
//! let mut reaction = engine.on_control(&mut store, &mut policy, record, &mut out);
//! loop {
//!     match reaction {
//!         Reaction::Send { channel, len } => link.send(channel, &out[..len]),
//!         Reaction::SendAndReboot { len } => { link.send(Channel::Control, &out[..len]); link.drain(); reboot() }
//!         Reaction::Close(channel) => { link.close(channel); break }
//!         Reaction::Idle => break,
//!     }
//!     reaction = engine.poll(&mut store, &mut out);
//! }
//! ```
//!
//! [`Engine::poll`] is where a `GET` streams and where an error owed to a transfer the engine has
//! already dropped comes out, so a driver that stops pumping stalls a download. Link teardown is one
//! call, [`Engine::on_link_lost`], and it is not optional: it is the third form of cancel (§3.8).
//!
//! ## What it refuses to have
//!
//! No resume, no checkpoint, no operation identifier, no durable result, no session. One transfer at
//! a time, and the answer to a second one is `busy`. Any break before the commit — a cancel, a cable
//! pull, a CRC failure, a validator refusal — releases the allocation and leaves the card as if
//! nothing happened, and the client restarts from zero. The catalog is the only durable record of a
//! result, and §3.4's `STATUS` is how a client reads it after a break.

use obc_crc::Crc32;

use super::ids::{DisplayName, EntryFlags, EntryMeta, ObjectId, ObjectKind, Revision};
use super::store::{Mode, Mutation, Policy, PutSource, Store, StoreError};
use super::wire::{
    decode_request, detail, encode_arm, encode_cancel, encode_error, encode_get, encode_put, encode_remove,
    encode_status, write_stream, ArmRequest, ControlError, ErrorCode, GetRequest, ListRequest, ListWriter, ObjectState,
    Opcode, PutRequest, Refusal, RemoveRequest, Request, RequestId, StatusRequest, StatusResponse, StreamFrame,
    CONTROL_FLOOR, STREAM_HEADER_LEN,
};

/// The staging buffer a transfer accumulates into before it reaches the card, in bytes.
///
/// Whole 512-byte blocks leave an allocation in one media write, so a stage that is a multiple of
/// the block size turns a burst of small link records into few large writes — which is the whole of
/// what "staging buffers for throughput" means here. A board with RAM to spare raises it.
pub const DEFAULT_STAGE: usize = 4_096;

/// Which of a binding's two record channels a record belongs to (§5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Channel {
    /// One control frame per record, in strict request/response order.
    Control,
    /// One stream frame plus its payload per record.
    Stream,
}

/// What the engine wants done after one record or one pump.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reaction {
    /// Nothing to do. A silently discarded stream frame lands here (§3.8).
    Idle,
    /// Send `out[..len]` on this channel.
    Send { channel: Channel, len: usize },
    /// §4 steps 4 and 5: send `out[..len]` on the control channel, drain the link, then reboot.
    SendAndReboot { len: usize },
    /// Close this record stream and emit nothing: §3.1's unanswerable record.
    Close(Channel),
}

/// Why the device is dropping a transfer of its own accord (§3.9's `cancelled` details).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CancelCause {
    /// A device-local decision: a ride starting, a battery too low to install, a shutdown.
    Device,
    /// The transfer's own channel died under a link that can still carry the answer — a CoC that
    /// closed while ATT stayed up. A link that went away entirely is [`Engine::on_link_lost`],
    /// which answers nobody.
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

/// The record ceilings the binding imposes (§5.1, §5.2).
///
/// A link whose control records cannot carry a header, a `LIST` prefix and one entry cannot carry
/// this protocol; §5.1 says the adapter refuses the connection rather than truncating, and
/// [`Ceilings::new`] returning `None` is that refusal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ceilings {
    control: usize,
    stream: usize,
}

impl Ceilings {
    /// The two ceilings, or `None` for a link below the protocol floor.
    pub fn new(control: usize, stream: usize) -> Option<Self> {
        (control >= CONTROL_FLOOR && stream > STREAM_HEADER_LEN).then_some(Ceilings { control, stream })
    }

    /// The largest control record this link carries.
    pub fn control(&self) -> usize {
        self.control
    }

    /// The largest stream record this link carries.
    pub fn stream(&self) -> usize {
        self.stream
    }
}

/// The live upload, if one owns the engine.
struct Upload<A> {
    request: RequestId,
    /// The object being replaced, or [`ObjectId::NONE`] for a create — whose id the commit assigns,
    /// because `next_object_id` reserves nothing and a device-local commit may take it meanwhile.
    id: ObjectId,
    revision: Revision,
    kind: ObjectKind,
    name: DisplayName,
    declared_len: u64,
    declared_crc: u32,
    retain_previous: bool,
    /// The head this replaces, if any. Re-checked immediately before the commit (§3.6).
    displaced: Option<Revision>,
    received: u64,
    staged: usize,
    crc: Crc32,
    allocation: A,
}

/// The live download, if one owns the engine.
struct Download<H> {
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

/// A response the engine still owes a transfer it has already dropped (§3.8: a cancelled `PUT` or
/// `GET` receives its own error response, and the `CANCEL` receives a different one).
#[derive(Debug, Clone, Copy)]
struct Owed {
    opcode: Opcode,
    request: RequestId,
    refusal: Refusal,
}

/// The one transfer engine.
pub struct Engine<S: Store, const STAGE: usize = DEFAULT_STAGE> {
    ceilings: Ceilings,
    live: Live<S>,
    owed: Option<Owed>,
    staging: [u8; STAGE],
}

impl<S: Store, const STAGE: usize> Engine<S, STAGE> {
    /// An idle engine on a link with these ceilings.
    pub fn new(ceilings: Ceilings) -> Self {
        const {
            assert!(STAGE >= 512 && STAGE.is_multiple_of(512), "the stage is whole 512-byte blocks");
        }
        Engine { ceilings, live: Live::Idle, owed: None, staging: [0; STAGE] }
    }

    /// The `RequestId` of the live transfer, if one owns the engine.
    pub fn live_transfer(&self) -> Option<RequestId> {
        match &self.live {
            Live::Idle => None,
            Live::Upload(upload) => Some(upload.request),
            Live::Download(download) => Some(download.request),
        }
    }

    /// True while nothing is owed and nothing is live: what an adapter checks before it sleeps.
    pub fn is_quiet(&self) -> bool {
        self.owed.is_none() && matches!(self.live, Live::Idle)
    }

    /// One whole control record arrived.
    pub fn on_control<P: Policy>(&mut self, store: &mut S, policy: &mut P, record: &[u8], out: &mut [u8]) -> Reaction {
        let (header, request) = match decode_request(record) {
            Ok(decoded) => decoded,
            // §3.1: a zero `RequestId` is unanswerable, and so is a record too short to carry one.
            Err(ControlError::Unanswerable) => return Reaction::Close(Channel::Control),
            Err(ControlError::Refused { request, refusal }) => {
                // The opcode is echoed where it is known; a frame this malformed has none to echo,
                // and `LIST` is the opcode a client sends first.
                let opcode = Opcode::decode(record.get(5).copied().unwrap_or(0)).unwrap_or(Opcode::List);
                return self.emit_error(out, opcode, request, refusal);
            }
        };
        match request {
            Request::List(list) => self.on_list(store, header.request, list, out),
            Request::Status(status) => self.on_status(store, header.request, status, out),
            Request::Get(get) => self.on_get(store, header.request, get, out),
            Request::Put(put) => self.on_put(store, header.request, put, out),
            Request::Remove(remove) => self.on_remove(store, header.request, remove, out),
            Request::Cancel(cancel) => self.on_cancel(store, header.request, cancel.transfer, out),
            Request::Arm(arm) => self.on_arm(store, policy, header.request, arm, out),
        }
    }

    /// One whole stream record arrived: §3.8's 16-byte frame followed by exactly its payload.
    pub fn on_stream<P: Policy>(&mut self, store: &mut S, policy: &mut P, record: &[u8], out: &mut [u8]) -> Reaction {
        let Some((frame, payload)) = StreamFrame::split(record) else {
            // A record that does not split names no transfer this can be sure of. §3.8 makes a
            // malformed stream record terminate the transfer, and there is exactly one to terminate.
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
        // §3.8: a frame bearing a `RequestId` that is not the live transfer's is discarded in
        // silence, and so is one bearing a live *download*'s — those bytes go the other way. One
        // match settles both, and leaves nothing later in this function to be sure about.
        let Live::Upload(upload) = &self.live else { return Reaction::Idle };
        if frame.transfer != upload.request {
            return Reaction::Idle;
        }
        let (offset, declared) = (upload.received, upload.declared_len);
        if frame.len as usize > self.ceilings.stream - STREAM_HEADER_LEN {
            return self.fail_upload(store, Refusal::new(ErrorCode::InvalidFrame, detail::invalid_frame::LENGTH), out);
        }
        // "Frames are contiguous and ascending; the offset equals the receiver's next expected
        // offset." A gap and an overlap are the same refusal.
        if frame.offset != offset || offset + payload.len() as u64 > declared {
            let refusal = Refusal::new(ErrorCode::InvalidRequest, detail::invalid_request::STREAM_OFFSET);
            return self.fail_upload(store, refusal, out);
        }
        if let Err(error) = self.absorb(store, payload) {
            return self.fail_upload(store, media_refusal(error, detail::media_io::WRITE), out);
        }
        if offset + (payload.len() as u64) < declared {
            return Reaction::Idle;
        }
        self.finish_upload(store, policy, out)
    }

    /// Pumps the engine: a live download's next record, or an error owed to a dropped transfer.
    ///
    /// A driver calls this until it answers [`Reaction::Idle`].
    pub fn poll(&mut self, store: &mut S, out: &mut [u8]) -> Reaction {
        if let Some(owed) = self.owed.take() {
            return self.emit_error(out, owed.opcode, owed.request, owed.refusal);
        }
        let Live::Download(download) = &self.live else { return Reaction::Idle };
        let (request, revision, payload_len, payload_crc, offset) =
            (download.request, download.revision, download.payload_len, download.payload_crc, download.sent);
        if offset >= payload_len {
            // Every byte has been handed to the transport, so §3.5's answer to the request goes out
            // and the hold is released.
            self.abandon(store);
            return match encode_get(out, request, revision, payload_len, payload_crc) {
                Some(len) => Reaction::Send { channel: Channel::Control, len },
                None => Reaction::Idle,
            };
        }
        // A buffer that cannot hold a frame and one payload byte would stall the download forever,
        // so it ends it instead of looping on `Idle`. An adapter that reports a ceiling it will not
        // supply is the device's own fault, not the client's.
        if out.len() <= STREAM_HEADER_LEN {
            return self.fail_download(store, Refusal::plain(ErrorCode::Internal), out);
        }
        let room = out.len().min(self.ceilings.stream) - STREAM_HEADER_LEN;
        let want = room.min((payload_len - offset) as usize);
        let read = store.read(&download.handle, offset, &mut out[STREAM_HEADER_LEN..STREAM_HEADER_LEN + want]);
        match read {
            // A short read before the end of the payload is a media failure with no other way to
            // report itself: the length is the catalog's, not the reader's.
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

    /// The link went away (§3.8's third form of cancel). The live transfer is dropped, its
    /// allocation released, its handle closed, and no record of it exists.
    ///
    /// Nothing is answered, because there is nobody left to answer: an error owed to a transfer the
    /// peer can no longer hear is dropped with it.
    pub fn on_link_lost(&mut self, store: &mut S) {
        self.abandon(store);
        self.owed = None;
    }

    /// The **device's** half of §3.8's bilateral cancel: "The device cancels by answering the
    /// outstanding `PUT` or `GET` with an error and dropping the transfer."
    ///
    /// Reports whether there was one. The allocation is released or the handle closed exactly as
    /// every other abandonment does, and the transfer's `cancelled` answer goes out on the next
    /// [`poll`](Engine::poll) — the caller is a device-local decision (a ride starting, a battery
    /// below the install threshold, a stream channel that died under a control channel that did
    /// not), not a wire request, so there is no second response to pair it with.
    pub fn cancel_live(&mut self, store: &mut S, cause: CancelCause) -> bool {
        let Some(request) = self.live_transfer() else { return false };
        let opcode = if matches!(self.live, Live::Upload(_)) { Opcode::Put } else { Opcode::Get };
        self.abandon(store);
        self.owed = Some(Owed { opcode, request, refusal: Refusal::new(ErrorCode::Cancelled, cause.detail()) });
        true
    }

    // -- the opcodes -----------------------------------------------------------------------------

    fn on_list(&mut self, store: &mut S, request: RequestId, list: ListRequest, out: &mut [u8]) -> Reaction {
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
        let ceiling = self.ceilings.control;
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

    fn on_status(&mut self, store: &mut S, request: RequestId, status: StatusRequest, out: &mut [u8]) -> Reaction {
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

    fn on_get(&mut self, store: &mut S, request: RequestId, get: GetRequest, out: &mut [u8]) -> Reaction {
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
        // §3.5: the store did not write a reserve's bytes, and a recording ride's length and CRC are
        // zero until the commit that ends it, so serving one would report success over an empty
        // payload.
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
            request,
            revision: meta.revision,
            payload_len: meta.payload_len,
            payload_crc: meta.payload_crc,
            sent: 0,
            handle,
        });
        // The first record of the payload, so that `Idle` keeps meaning "nothing to do" and a
        // driver that stops pumping on it cannot stall a download.
        self.poll(store, out)
    }

    fn on_put(&mut self, store: &mut S, request: RequestId, put: PutRequest, out: &mut [u8]) -> Reaction {
        match self.admit_put(store, request, put) {
            Ok(()) => Reaction::Idle,
            Err(refusal) => self.emit_error(out, Opcode::Put, request, refusal),
        }
    }

    /// §3.6's admission: every check that must pass before a byte is allocated for.
    fn admit_put(&mut self, store: &mut S, request: RequestId, put: PutRequest) -> Result<(), Refusal> {
        if let Some(refusal) = self.busy_refusal() {
            return Err(refusal);
        }
        if let Some(refusal) = write_refusal(store.mode()) {
            return Err(refusal);
        }
        // §5.3 of the format: an object with no bytes is a `Remove`, not a `Put`, because an entry
        // that owns extents while needing none is the slack that rule forbids.
        if put.payload_len == 0 {
            return Err(bad_combination());
        }
        // §3.6: kinds 3 and 8 are produced by the device, whether the request creates or replaces.
        if put.kind.is_device_owned() {
            return Err(bad_combination());
        }
        // "legal only for kinds whose reader needs continuity — weather, today".
        if put.retain_previous && put.kind != ObjectKind::WeatherBundle {
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
            if head.flags.is_untouchable() {
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
            // A create names no id until it commits. `next_object_id` reserves nothing, and a
            // device-local commit — a ride starting mid-upload — takes the id this would have pinned
            // and turns the publish into a `revisionConflict` naming an object the client never sent.
            // The cursor never rewinds (`FLAT_Store_Format.md` §5.2), so reading it at the commit is
            // both fresh and free.
            (ObjectId::NONE, Revision::FIRST, None)
        };
        let allocation = store.allocate(put.payload_len).map_err(|error| allocate_refusal(error, put.payload_len))?;
        self.live = Live::Upload(Upload {
            request,
            id,
            revision,
            kind: put.kind,
            name: put.name,
            declared_len: put.payload_len,
            declared_crc: put.payload_crc,
            // A create has no displaced revision to retain, so the flag has nothing to ask for.
            retain_previous: put.retain_previous && displaced.is_some(),
            displaced,
            received: 0,
            staged: 0,
            crc: Crc32::new(),
            allocation,
        });
        Ok(())
    }

    fn on_remove(&mut self, store: &mut S, request: RequestId, remove: RemoveRequest, out: &mut [u8]) -> Reaction {
        match self.apply_remove(store, remove) {
            Ok(sequence) => match encode_remove(out, request, sequence) {
                Some(len) => Reaction::Send { channel: Channel::Control, len },
                None => self.emit_error(out, Opcode::Remove, request, Refusal::plain(ErrorCode::Internal)),
            },
            Err(refusal) => self.emit_error(out, Opcode::Remove, request, refusal),
        }
    }

    fn apply_remove(&mut self, store: &mut S, remove: RemoveRequest) -> Result<u64, Refusal> {
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
        if head.flags.is_untouchable() {
            return Err(bad_combination());
        }
        // §3.7: "a retained previous revision of the same object goes with it".
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

    fn on_cancel(&mut self, store: &mut S, request: RequestId, transfer: RequestId, out: &mut [u8]) -> Reaction {
        let live = self.live_transfer();
        let cancelled = live == Some(transfer);
        if cancelled {
            let opcode = if matches!(self.live, Live::Upload(_)) { Opcode::Put } else { Opcode::Get };
            self.abandon(store);
            // §3.8: the cancelled transfer receives its own error response, and the `CANCEL`
            // receives a different one. The transfer's goes out on the next pump.
            self.owed = Some(Owed {
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
        store: &mut S,
        policy: &mut P,
        request: RequestId,
        arm: ArmRequest,
        out: &mut [u8],
    ) -> Reaction {
        match self.apply_arm(store, policy, arm) {
            Ok((reserve, sequence)) => match encode_arm(out, request, reserve, sequence) {
                // §4 steps 4 and 5: the answer must reach the transport before the reboot.
                Some(len) => Reaction::SendAndReboot { len },
                None => self.emit_error(out, Opcode::Arm, request, Refusal::plain(ErrorCode::Internal)),
            },
            Err(refusal) => self.emit_error(out, Opcode::Arm, request, refusal),
        }
    }

    fn apply_arm<P: Policy>(
        &mut self,
        store: &mut S,
        policy: &mut P,
        arm: ArmRequest,
    ) -> Result<(ObjectId, u64), Refusal> {
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
        // Step 1. Every refusal here — structure, CRC, signature, monotonicity, a recording ride, a
        // flat battery — is `rejected` with the update kind's detail, and changes nothing.
        let bytes = policy
            .validate_package(head.id, head.revision)
            .map_err(|reason| Refusal::new(ErrorCode::Rejected, reason))?;
        // Step 2: one entry of kind 8 carrying `RESERVED`, with enough extents for the running
        // image. This is the one commit `ARM` makes, and it exists because the bootloader cannot
        // allocate.
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
        // Step 3. A *cut* before this completes is survivable because the reboot that follows it runs
        // the reconciliation §4 describes: the boot page does not decode, the bootloader reads no
        // pending update, and the reserve is an ordinary object the next boot removes. A **refusal**
        // reaches no reboot and therefore no reconciliation, and a `RESERVED` entry cannot be removed
        // from the wire at all (§3.7) — so leaving it would strand extents the client can never free
        // and let every retry commit another one. The refusal takes its own commit back instead,
        // which is what makes §3.9's "an error means the mutation did not happen" true here.
        if policy.hand_off((head.id, head.revision), (reserve.id, reserve.revision)).is_err() {
            // Best effort: a card that refuses both the handoff and the way back leaves the reserve
            // for the next boot's reconciliation, and there is nothing further this can do about it.
            let _ = store.commit(&[Mutation::Remove { id: reserve.id, revision: reserve.revision }]);
            return Err(Refusal::plain(ErrorCode::Internal));
        }
        Ok((reserve.id, sequence))
    }

    // -- the upload's own machinery --------------------------------------------------------------

    /// Folds one stream payload into the running CRC and the staging buffer, writing whole stages
    /// through to the card.
    fn absorb(&mut self, store: &mut S, payload: &[u8]) -> Result<(), StoreError> {
        let Live::Upload(upload) = &mut self.live else { return Err(StoreError::Invalid) };
        upload.crc.update(payload);
        upload.received += payload.len() as u64;
        let mut input = payload;
        while !input.is_empty() {
            // A record at or above one stage goes straight to the card: the copy through staging
            // would buy nothing, and this is the path a bulk USB record takes.
            if upload.staged == 0 && input.len() >= STAGE {
                let (chunk, rest) = input.split_at(STAGE);
                store.write(&mut upload.allocation, chunk)?;
                input = rest;
                continue;
            }
            let take = (STAGE - upload.staged).min(input.len());
            self.staging[upload.staged..upload.staged + take].copy_from_slice(&input[..take]);
            upload.staged += take;
            input = &input[take..];
            if upload.staged == STAGE {
                store.write(&mut upload.allocation, &self.staging)?;
                upload.staged = 0;
            }
        }
        Ok(())
    }

    /// §3.6's last byte: verify the length and the whole-payload CRC, run the kind's validator, and
    /// commit.
    fn finish_upload<P: Policy>(&mut self, store: &mut S, policy: &mut P, out: &mut [u8]) -> Reaction {
        let Live::Upload(upload) = &self.live else { return Reaction::Idle };
        let (kind, declared_len, declared_crc) = (upload.kind, upload.declared_len, upload.declared_crc);
        let computed = upload.crc.finalize();
        if computed != declared_crc {
            let refusal = Refusal::with_context(
                ErrorCode::ChecksumFailure,
                detail::checksum_failure::PAYLOAD,
                u64::from(declared_crc),
            );
            return self.fail_upload(store, refusal, out);
        }
        if let Err(reason) = policy.accept(kind, declared_len) {
            return self.fail_upload(store, Refusal::new(ErrorCode::Rejected, reason), out);
        }
        // Everything received is on the card before the commit begins.
        if let Err(error) = self.flush(store) {
            return self.fail_upload(store, media_refusal(error, detail::media_io::WRITE), out);
        }
        let request = self.owed_request();
        match self.publish(store) {
            Ok((id, revision, len, crc)) => {
                // The commit consumed the allocation and the catalog is the result. Nothing is live
                // from here, whatever the response does.
                self.live = Live::Idle;
                match encode_put(out, request, id, revision, len, crc) {
                    Some(len) => Reaction::Send { channel: Channel::Control, len },
                    // §3.4 is what a client does with a response it never saw.
                    None => Reaction::Idle,
                }
            }
            Err(refusal) => self.fail_upload(store, refusal, out),
        }
    }

    /// Writes whatever the staging buffer still holds.
    fn flush(&mut self, store: &mut S) -> Result<(), StoreError> {
        let Live::Upload(upload) = &mut self.live else { return Ok(()) };
        if upload.staged == 0 {
            return Ok(());
        }
        let staged = upload.staged;
        store.write(&mut upload.allocation, &self.staging[..staged])?;
        upload.staged = 0;
        Ok(())
    }

    /// The `RequestId` the live transfer's answer echoes. Zero when there is none, which only a
    /// caller with nothing to answer ever sees.
    fn owed_request(&self) -> RequestId {
        self.live_transfer().unwrap_or(RequestId(0))
    }

    /// The one commit a `PUT` makes: publish the new head, and settle what it displaced.
    fn publish(&mut self, store: &mut S) -> Result<(ObjectId, Revision, u64, u32), Refusal> {
        let Live::Upload(upload) = &self.live else { return Err(Refusal::plain(ErrorCode::Internal)) };
        // A create takes its id here rather than at admission: the cursor only moves forward, so
        // reading it at the commit cannot collide with a device-local commit that ran meanwhile.
        let id = if upload.id.is_some() { upload.id } else { store.next_object_id() };
        // §3.6: the expected `Revision` is checked at admission and again immediately before the
        // commit. For a create the expectation is that nothing names this id at all.
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
        // What the displaced head becomes. A replace leaves at most what it asked for: a retaining
        // one keeps exactly the revision it displaced, and an ordinary one leaves the object with a
        // head and nothing else.
        let displace = found.head.map(|head| {
            if upload.retain_previous {
                let retained = EntryMeta { flags: head.flags.with(EntryFlags::RETAINED), ..head };
                Mutation::Put { meta: retained, source: PutSource::Amend }
            } else {
                Mutation::Remove { id: head.id, revision: head.revision }
            }
        });
        // A revision the object already kept retained goes either way: a second retaining replace
        // frees the first, and an ordinary replace clears retention altogether.
        let free = found.retained.map(|meta| Mutation::Remove { id: meta.id, revision: meta.revision });
        let sequence = match (displace, free) {
            (None, _) => store.commit(&[publish]),
            (Some(displace), None) => store.commit(&[publish, displace]),
            (Some(displace), Some(free)) => store.commit(&[publish, displace, free]),
        };
        sequence.map_err(|error| commit_refusal(error, meta.payload_len))?;
        Ok((meta.id, meta.revision, meta.payload_len, meta.payload_crc))
    }

    // -- unwinding -------------------------------------------------------------------------------

    /// Drops the live transfer and releases what it holds. The one path every abandonment takes.
    fn abandon(&mut self, store: &mut S) {
        match core::mem::replace(&mut self.live, Live::Idle) {
            Live::Idle => {}
            Live::Upload(upload) => store.cancel(upload.allocation),
            Live::Download(download) => store.close(download.handle),
        }
    }

    /// Ends the live upload with a refusal: the allocation is released, the written bytes are
    /// anonymous, and the catalog is untouched.
    fn fail_upload(&mut self, store: &mut S, refusal: Refusal, out: &mut [u8]) -> Reaction {
        let request = self.owed_request();
        self.abandon(store);
        self.emit_error(out, Opcode::Put, request, refusal)
    }

    /// The same for a download, whose only hold is the open handle.
    fn fail_download(&mut self, store: &mut S, refusal: Refusal, out: &mut [u8]) -> Reaction {
        let request = self.owed_request();
        self.abandon(store);
        self.emit_error(out, Opcode::Get, request, refusal)
    }

    fn emit_error(&mut self, out: &mut [u8], opcode: Opcode, request: RequestId, refusal: Refusal) -> Reaction {
        match encode_error(out, opcode, request, &refusal) {
            Some(len) => Reaction::Send { channel: Channel::Control, len },
            // A buffer that cannot hold a 32-byte error response is below the §5.1 floor, which the
            // adapter refused at connection time. There is nothing to say and nowhere to say it.
            None => Reaction::Idle,
        }
    }

    /// §1: the device serves exactly one `PUT` or `GET`; a second is `busy`, and the context is the
    /// live transfer's own `RequestId`.
    fn busy_refusal(&self) -> Option<Refusal> {
        self.live_transfer()
            .map(|live| Refusal::with_context(ErrorCode::Busy, detail::busy::TRANSFER, u64::from(live.0)))
    }
}

/// The entries one `ObjectId` has: a head, and at most one retained revision (§5.3).
struct Found {
    head: Option<EntryMeta>,
    retained: Option<EntryMeta>,
}

/// Resolves one `ObjectId` out of the catalog view. The caller checks
/// [`entries_ok`](Store::entries_ok) afterwards — a listing that stopped early would make an absent
/// object out of a media failure.
fn lookup<S: Store>(store: &S, id: ObjectId) -> Found {
    let mut found = Found { head: None, retained: None };
    for meta in store.entries() {
        if meta.id != id {
            // The catalog is sorted by `(ObjectId, Revision)`, so once the array is past the id
            // there is nothing left to find.
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

/// §3.9's `readOnly` for an opcode that only reads. Only the two exhausted cases still serve reads.
fn read_refusal(mode: Mode) -> Option<Refusal> {
    if mode.readable() {
        return None;
    }
    Some(Refusal::new(ErrorCode::ReadOnly, read_only_detail(mode)))
}

/// The same for an opcode that commits, which the exhausted cases refuse too.
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
        // A card that is not a flat store and a card that is not the card the superblock describes
        // are the same answer: there is no flat store here.
        Mode::Unformatted | Mode::CardTooSmall => detail::read_only::UNFORMATTED,
    }
}

/// A refusal from a `write` or a `read`. A store that answers `ReadOnly` mid-transfer says so —
/// detail `0`, because the mode that produced it was `ReadWrite` when the transfer was admitted and
/// this path has no narrower fact to offer.
fn media_refusal(error: StoreError, when: u16) -> Refusal {
    match error {
        StoreError::ReadOnly => Refusal::new(ErrorCode::ReadOnly, 0),
        _ => Refusal::new(ErrorCode::MediaIo, when),
    }
}

/// A refusal from `allocate`. `Invalid` here is a full reservation table — a transient fact about
/// the device, which §3.9 answers with `busy` and never with `invalidRequest`.
///
/// That reading is only sound because the caller has already ruled out every other way `allocate`
/// says `Invalid`: `admit_put` refuses a zero declared length and a `RECORDING`/`RESERVED` head
/// before it ever reaches here, and `apply_arm` allocates only what its policy asked for. Those
/// pre-checks are load-bearing for this mapping, not decoration — remove one and a client's own bad
/// request comes back as "the device is busy, try again", forever.
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
        StoreError::NotFound | StoreError::RevisionConflict { .. } => Refusal::plain(ErrorCode::Internal),
    }
}

/// A refusal from `open`. A full hold table is the same transient fact as a full reservation table.
fn open_refusal(error: StoreError) -> Refusal {
    match error {
        StoreError::NotFound => Refusal::new(ErrorCode::NotFound, detail::not_found::OBJECT),
        StoreError::Invalid => Refusal::plain(ErrorCode::Busy),
        StoreError::Media => Refusal::new(ErrorCode::MediaIo, detail::media_io::READ),
        StoreError::ReadOnly => Refusal::new(ErrorCode::ReadOnly, detail::read_only::CATALOG_UNREADABLE),
        _ => Refusal::plain(ErrorCode::Internal),
    }
}

/// A refusal from `commit`. A commit that returns `Err` changed nothing, so every one of these is a
/// mutation that did not happen. `bytes` is what the mutation needed, which is §3.9's context for
/// `noSpace` however the store phrased its refusal.
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
        // The engine built the batch, so a structural refusal is this crate's fault and not the
        // client's.
        StoreError::Invalid => Refusal::plain(ErrorCode::Internal),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The tables between §2's refusals and §3.9's codes, checked where they are written. Every one
    /// of these also has a behaviour test over a real card in `tests/flat_engine.rs`; these are the
    /// rows a card cannot easily be made to produce.
    #[test]
    fn a_full_table_is_busy_and_never_invalid_request() {
        assert_eq!(allocate_refusal(StoreError::Invalid, 1).code, ErrorCode::Busy);
        assert_eq!(open_refusal(StoreError::Invalid).code, ErrorCode::Busy);
        // The same variant from a commit is the engine's own batch being wrong, which is not the
        // client's fault either — but it is not transient, so it is not `busy`.
        assert_eq!(commit_refusal(StoreError::Invalid, 0).code, ErrorCode::Internal);
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
        // The two exhausted cases still serve reads, and refuse every commit.
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
        // §3.9 gives code 6 one context — the bytes required — however the store phrased its
        // refusal, so a commit's `noSpace` carries it too.
        assert_eq!(commit_refusal(StoreError::TooFragmented, 42_137).context, 42_137);
        assert_eq!(commit_refusal(StoreError::CatalogFull, 42_137).context, 42_137);
        assert_eq!(media_refusal(StoreError::Media, detail::media_io::READ).detail, detail::media_io::READ);
        // A store that refuses a write because it is read-only says so, rather than blaming media —
        // with detail `0`, because the mode was writable when the transfer was admitted and this
        // path has no narrower fact than "not any more".
        assert_eq!(media_refusal(StoreError::ReadOnly, detail::media_io::WRITE), Refusal::new(ErrorCode::ReadOnly, 0));
    }

    #[test]
    fn a_link_below_the_protocol_floor_is_refused_rather_than_truncated() {
        assert!(Ceilings::new(CONTROL_FLOOR - 1, 1_024).is_none());
        assert!(Ceilings::new(CONTROL_FLOOR, STREAM_HEADER_LEN).is_none());
        let ceilings = Ceilings::new(244, 1_024).expect("the device's preferred BLE link");
        assert_eq!((ceilings.control(), ceilings.stream()), (244, 1_024));
    }
}
