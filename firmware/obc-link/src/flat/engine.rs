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
    decode_request, detail, encode_arm, encode_cancel, encode_error, encode_format, encode_get, encode_put,
    encode_remove, encode_status, write_stream, ArmRequest, ControlError, ErrorCode, FormatRequest, GetRequest,
    ListRequest, ListWriter, ObjectState, Opcode, PutRequest, Refusal, RemoveRequest, Request, RequestId,
    StatusRequest, StatusResponse, StreamFrame, CONTROL_FLOOR, STREAM_HEADER_LEN,
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
    pub const fn new(control: usize, stream: usize) -> Option<Self> {
        // `then_some` is not `const`; the `if` is the same statement and is.
        if control >= CONTROL_FLOOR && stream > STREAM_HEADER_LEN {
            Some(Ceilings { control, stream })
        } else {
            None
        }
    }

    /// §5.1's ceilings for a BLE link, from what the link negotiated and what the adapter can hold.
    ///
    /// Three facts, in one place because they are one rule and an adapter that re-derived it would
    /// be re-deriving a specification:
    ///
    /// - **The control ceiling is `ATT_MTU - 3`** (§5.1). An `att_mtu` below 3 cannot carry a
    ///   handle-value payload at all and yields `None` rather than an underflow.
    /// - **The stream ceiling is the CoC SDU**, fixed at channel establishment.
    /// - **Both are clamped to `buffer`**, the adapter's reaction buffer. That clamp is
    ///   load-bearing rather than defensive: the engine frames a `LIST` page and a stream record
    ///   against these numbers, so a link that negotiated *upward* of the adapter's buffer would
    ///   otherwise have it framing into bytes that are not there.
    ///
    /// `None` is §5.1's "a link below that floor cannot carry this protocol and the adapter refuses
    /// the connection rather than truncating" — including the case where the clamp itself is what
    /// puts the link under the floor, since a buffer too small to hold a single-entry page is the
    /// same refusal for the same reason.
    pub fn for_ble(att_mtu: usize, coc_sdu: usize, buffer: usize) -> Option<Self> {
        let control = att_mtu.checked_sub(3)?.min(buffer);
        let stream = coc_sdu.min(buffer);
        Ceilings::new(control, stream)
    }

    /// §5.2's ceilings for a USB link — the adapter's record buffer, on both channels.
    ///
    /// **There is nothing to negotiate and therefore nothing to derive.** §5.1 reads BLE's two
    /// numbers off a link that fixed them at connection time; USB fixes neither, because a bulk
    /// endpoint's max packet is a *packet* size and §5.2's records span packets by design. So the
    /// ceiling is a constant of the binding, and the constant is the buffer the adapter frames into
    /// — which is the same clamp `for_ble` applies last, arrived at directly instead of after two
    /// link facts that do not exist here.
    ///
    /// One number for both channels, because one buffer serves both: the engine frames a `LIST`
    /// page and a `GET`'s stream records into the same reaction buffer, so a control ceiling above
    /// the stream one (or the reverse) would describe bytes the adapter does not have.
    ///
    /// `None` is §5.1's refusal, unchanged: a buffer too small for a single-entry page cannot carry
    /// this protocol, and truncating it is not on the table.
    pub const fn for_usb(record: usize) -> Option<Self> {
        Ceilings::new(record, record)
    }

    /// The largest control record this link carries.
    pub const fn control(&self) -> usize {
        self.control
    }

    /// The largest stream record this link carries.
    pub const fn stream(&self) -> usize {
        self.stream
    }
}

/// **The adapter's admission latch** (§3.6, §5), and the reason it is a type here rather than a
/// `bool` in a binding.
///
/// §3.6 lets a client stream a `PUT` immediately, without waiting for an acceptance, so the first
/// stream frame of a transfer races its own control frame. A binding therefore has to know whether a
/// frame is *already admitted* — a continuation of a live transfer, deliverable at once — or
/// possibly the leading edge of one whose control frame has not arrived, which §5 says it must
/// **hold** rather than deliver (the engine would discard it in silence and the upload would die at
/// offset zero) and rather than drop.
///
/// Asking the engine per frame answers that correctly and costs a round trip on every record of a
/// multi-megabyte upload. Latching "something has been admitted" is cheap and **wrong**: it stays
/// set when the transfer it was set for finishes, so the *second* `PUT` on one channel — the
/// ordinary "upload three routes" session — has its leading frame waved through to an idle engine
/// and dies exactly as the unlatched race did. That is not hypothetical; it is the bug this type
/// replaces.
///
/// So the latch remembers **which** `RequestId` was admitted. A frame bearing that id is a
/// continuation and skips the query; any other id — including the first frame of the next transfer
/// on the same channel — is queried. The hot path stays free and the race stays closed.
///
/// One deliberate narrowing: a client that reuses a `RequestId` immediately after its transfer ended
/// is treated as a continuation. §3.8 already tells clients not to (`SHOULD NOT`, because in-flight
/// frames from the old transfer would be absorbed by the new one) and describes this same failure,
/// so the latch inherits the specification's own boundary rather than drawing a new one.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Admission {
    admitted: Option<RequestId>,
}

impl Admission {
    /// A latch that has admitted nothing — what every new channel starts with.
    pub fn new() -> Self {
        Admission { admitted: None }
    }

    /// True when the engine must be consulted before this frame may be delivered.
    pub fn needs_query(&self, frame: RequestId) -> bool {
        self.admitted != Some(frame)
    }

    /// Record what the engine reported while `frame` was waiting. Returns **true when the frame must
    /// be held** — nothing is live, or what is live is a different transfer, and either way this
    /// frame is not admitted yet.
    pub fn observed(&mut self, frame: RequestId, live: Option<RequestId>) -> bool {
        if live == Some(frame) {
            self.admitted = Some(frame);
            return false;
        }
        // Not this transfer. Forget any earlier admission too: whatever it named is not what is
        // arriving, so keeping it could only wave a later frame through on a stale identity.
        self.admitted = None;
        true
    }

    /// Forget every admission — a new channel has admitted nothing.
    pub fn reset(&mut self) {
        self.admitted = None;
    }
}

/// **What a live upload has landed so far** — the one thing a *device* needs from the engine that no
/// client ever asks for.
///
/// A map is hundreds of megabytes and lands over twenty minutes, and a rider watching a progress
/// bar is not a client of this protocol: the wire's answer to "how is it going" is the transfer's
/// one response, twenty minutes later. So the device reads the engine directly, and reads it where
/// the engine already is — no round trip, no second counter, nothing on the wire.
///
/// It is a report, not a hook. The engine calls nothing back and knows nothing about screens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UploadProgress {
    /// The transfer's identifier (§3.1), so a reader can tell one upload from the next.
    pub request: RequestId,
    /// What is being uploaded — the only input a device needs to decide whether it has a screen
    /// for this.
    pub kind: ObjectKind,
    /// Payload bytes absorbed so far.
    pub received: u64,
    /// What the `PUT` declared (§3.6).
    pub declared: u64,
}

/// **How the last upload ended**, latched once and taken once.
///
/// Latched rather than reported live for the same reason the progress above is read rather than
/// pushed: the terminal fact exists for exactly one instant — the call that commits or refuses — and
/// a device that only looks between calls would otherwise see an upload vanish with no verdict. A
/// caller that never looks costs one stale enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UploadEnd {
    /// §3.6's commit landed. `id` is the resulting head and `replaced` distinguishes a create from
    /// replace-at-same-id so a board can invalidate state derived from the displaced revision.
    Committed { id: ObjectId, replaced: bool },
    /// The transfer was refused, with the code its error response carried (§3.9). The detail is
    /// deliberately not here: a device turns this into one of a handful of screens, and every
    /// narrower fact belongs to the client that asked.
    Refused(ErrorCode),
}

/// **Which wire a call arrived on.**
///
/// The engine is one value serving two links at once — a phone in a pocket and a cable in J3 — and
/// almost nothing it does needs to know which. §1's "one engine, one owner" and §1's one-transfer
/// rule are *why* it is one value: a second `PUT` is `busy` whichever wire asked, and that falls out
/// of there being one `live` rather than out of any arbitration.
///
/// Three things do need to know, and each is a fact about a *link* rather than about the store:
///
/// - **Ceilings are per link** (§5.1 vs §5.2): 245 bytes of CoC SDU against 4,112 bytes of USB
///   record. One shared number would have each link framing against the other's.
/// - **A link coming up may not disturb the other one's transfer.** It is a new peer on one wire,
///   not a new state of the device.
/// - **A link going away releases only what that link held** (§3.8's third form of cancel answers
///   "the transfer whose link went away", not "the transfer").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Link {
    /// The radio (§5.1).
    Ble,
    /// The cable (§5.2).
    Usb,
}

impl Link {
    /// Index into the per-link tables. `usize` rather than a map because there are two links and
    /// there will not be a third — a third *transport* would be a new binding section in the spec.
    const fn index(self) -> usize {
        match self {
            Link::Ble => 0,
            Link::Usb => 1,
        }
    }
}

/// The live upload, if one owns the engine.
struct Upload<A> {
    /// The link that admitted it. Only this link may stream into it, pump it, or lose it.
    link: Link,
    /// **The ceilings this transfer was admitted under**, captured once at admission rather than
    /// read per record. A `GET` frames every one of its stream records against the link it is being
    /// served to, and that link's numbers must not be able to change under it — which is exactly
    /// what a shared `Engine::ceilings` allowed the *other* link to do.
    ceilings: Ceilings,
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
    /// Which half of a double-width adapter stage is currently being filled.
    stage_bank: usize,
    crc: Crc32,
    allocation: A,
}

/// The live download, if one owns the engine.
struct Download<H> {
    /// As [`Upload::link`].
    link: Link,
    /// As [`Upload::ceilings`].
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

/// A response the engine still owes a transfer it has already dropped (§3.8: a cancelled `PUT` or
/// `GET` receives its own error response, and the `CANCEL` receives a different one).
#[derive(Debug, Clone, Copy)]
struct Owed {
    /// The link the answer is owed *to*. A response cannot be pumped out of the other one.
    link: Link,
    opcode: Opcode,
    request: RequestId,
    refusal: Refusal,
}

/// The one transfer engine.
pub struct Engine<S: Store, const STAGE: usize = DEFAULT_STAGE> {
    /// What each link negotiated, indexed by [`Link::index`]. `None` until that link comes up, and
    /// `None` again when it goes away — a link with no ceilings cannot be served, which is the
    /// honest state of a wire nobody is on.
    ceilings: [Option<Ceilings>; 2],
    live: Live<S>,
    owed: Option<Owed>,
    /// The verdict on the last upload, for a device with a screen. See [`UploadEnd`].
    upload_end: Option<(ObjectKind, UploadEnd)>,
    staging: [u8; STAGE],
}

impl<S: Store, const STAGE: usize> Default for Engine<S, STAGE> {
    fn default() -> Self {
        Engine::new()
    }
}

impl<S: Store, const STAGE: usize> Engine<S, STAGE> {
    /// An idle engine with **no link up**. Each link announces itself with
    /// [`on_link_up`](Engine::on_link_up) and is served only while it has.
    pub fn new() -> Self {
        const {
            assert!(STAGE >= 512 && STAGE.is_multiple_of(512), "the stage is whole 512-byte blocks");
        }
        Engine { ceilings: [None, None], live: Live::Idle, owed: None, upload_end: None, staging: [0; STAGE] }
    }

    /// What `link` negotiated, or `None` while it is down.
    fn link_ceilings(&self, link: Link) -> Option<Ceilings> {
        self.ceilings[link.index()]
    }

    /// The link that owns the live transfer, if one does.
    fn live_link(&self) -> Option<Link> {
        match &self.live {
            Live::Idle => None,
            Live::Upload(upload) => Some(upload.link),
            Live::Download(download) => Some(download.link),
        }
    }

    /// The `RequestId` of the live transfer, if one owns the engine.
    pub fn live_transfer(&self) -> Option<RequestId> {
        match &self.live {
            Live::Idle => None,
            Live::Upload(upload) => Some(upload.request),
            Live::Download(download) => Some(download.request),
        }
    }

    /// What the live upload has landed so far, or `None` when none is live. See [`UploadProgress`].
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

    /// Whether this exact upload owns the engine.
    ///
    /// Adapter-specific resources must use this rather than an app-facing progress projection:
    /// progress deliberately omits the owner link, so it cannot prove that USB — rather than BLE —
    /// is entitled to claim a cable-only staging arena.
    pub fn upload_matches(&self, link: Link, request: RequestId, kind: ObjectKind) -> bool {
        matches!(
            &self.live,
            Live::Upload(upload) if upload.link == link && upload.request == request && upload.kind == kind
        )
    }

    /// **Take the verdict on the last upload**, clearing it. See [`UploadEnd`].
    ///
    /// A link that goes away leaves nothing here: §3.8's third form of cancel answers nobody, and a
    /// device whose cable was pulled needs no card explaining that back to the rider who pulled it.
    pub fn take_upload_end(&mut self) -> Option<(ObjectKind, UploadEnd)> {
        self.upload_end.take()
    }

    /// True while nothing is owed and nothing is live: what an adapter checks before it sleeps.
    pub fn is_quiet(&self) -> bool {
        self.owed.is_none() && matches!(self.live, Live::Idle)
    }

    /// One whole control record arrived.
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
            Request::List(list) => self.on_list(store, ceilings, header.request, list, out),
            Request::Status(status) => self.on_status(store, header.request, status, out),
            Request::Get(get) => self.on_get(store, link, ceilings, header.request, get, out),
            Request::Put(put) => self.on_put(store, link, ceilings, header.request, put, out),
            Request::Remove(remove) => self.on_remove(store, header.request, remove, out),
            Request::Cancel(cancel) => self.on_cancel(store, link, header.request, cancel.transfer, out),
            Request::Arm(arm) => self.on_arm(store, policy, header.request, arm, out),
            Request::Format(format) => self.on_format(store, header.request, format, out),
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
        // Once formatting starts, the old superblocks are invalidated first. Success or media
        // failure, the current in-memory store must never continue serving after this answer leaves
        // the link.
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

    /// One whole stream record arrived: §3.8's 16-byte frame followed by exactly its payload.
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

    /// The bank the cable adapter must lend to [`on_stream_staged`](Self::on_stream_staged).
    ///
    /// Exposing the index before the borrow lets an arena-backed adapter form `&mut` for only the
    /// inactive half. The opposite half may still be borrowed by deferred card DMA and must not be
    /// covered by a whole-arena mutable reference.
    pub fn upload_stage_bank(&self) -> Option<usize> {
        match &self.live {
            Live::Upload(upload) => Some(upload.stage_bank),
            _ => None,
        }
    }

    /// One whole stream record, using `stage` as the current bank of this upload's two-bank
    /// write-combining buffer.
    ///
    /// This is the cable adapter's high-throughput seam. The protocol record ceiling is deliberately
    /// independent of the card's efficient command width: a USB adapter may retain a scratch arm
    /// for the whole `PUT` and lend it here on every record, letting several records reach
    /// [`Store::write`] as one contiguous run. Radio adapters use [`on_stream`](Self::on_stream) and
    /// retain the engine's small resident stage.
    ///
    /// A buffer at the same bank index and with the same length must be supplied until it fills;
    /// then the engine advances [`upload_stage_bank`](Self::upload_stage_bank). The bank must be a
    /// non-zero multiple of 512 bytes. Supplying no stage part-way through would
    /// change the backing storage beneath `Upload::staged`; adapters must instead cancel the
    /// transfer if their scratch ownership is revoked.
    pub fn on_stream_staged<P: Policy>(
        &mut self,
        link: Link,
        store: &S,
        policy: &mut P,
        record: &[u8],
        out: &mut [u8],
        bank: usize,
        stage: &mut [u8],
    ) -> Reaction {
        if stage.len() < 512 || !stage.len().is_multiple_of(512) || self.upload_stage_bank() != Some(bank) {
            return Reaction::Close(Channel::Stream);
        }
        self.on_stream_with_stage(link, store, policy, record, out, Some((bank, stage)))
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
        // §3.8's silent discard, one step earlier: bytes on a wire that owns no transfer belong to
        // no transfer this can be sure of, and that includes bytes on the *other* link's wire.
        if self.live_link() != Some(link) {
            return Reaction::Idle;
        }
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
        if frame.len as usize > upload.ceilings.stream - STREAM_HEADER_LEN {
            return self.fail_upload(store, Refusal::new(ErrorCode::InvalidFrame, detail::invalid_frame::LENGTH), out);
        }
        // "Frames are contiguous and ascending; the offset equals the receiver's next expected
        // offset." A gap and an overlap are the same refusal.
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
    ///
    /// A driver calls this until it answers [`Reaction::Idle`].
    pub fn poll(&mut self, link: Link, store: &S, out: &mut [u8]) -> Reaction {
        if self.owed.is_some_and(|owed| owed.link == link) {
            let owed = self.owed.take().expect("just checked");
            return self.emit_error(out, owed.opcode, owed.request, owed.refusal);
        }
        let Live::Download(download) = &self.live else { return Reaction::Idle };
        // Only the link being served pumps its own download. The other one asking is not an error —
        // an adapter pumps until it is told there is nothing to do — it simply has nothing here.
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
        let room = out.len().min(stream_ceiling) - STREAM_HEADER_LEN;
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

    /// **A link came up with these ceilings** (§5.1, §5.2).
    ///
    /// It touches `link` and nothing else, and that is the fix this method exists for. It used to
    /// release the live transfer and rebuild the whole engine — which was correct while one link
    /// existed and became a bug the moment two did: a phone reconnecting destroyed a cable's
    /// twenty-minute map upload, with no answer to the client that was sending it, and re-pinned the
    /// stream ceiling to the radio's 245 bytes so the cable's next 4,112-byte record terminated
    /// over-ceiling. Neither peer had done anything wrong.
    ///
    /// So: a link coming up is a **new peer on one wire**, not a new state of the device. If this
    /// link owned the live transfer, that transfer's peer is gone and it is released (nobody is left
    /// to answer, exactly as §3.8's third form of cancel says). If the *other* link owned it, it is
    /// left completely alone, and the newcomer's own `PUT` or `GET` meets §1's one-at-a-time rule in
    /// the ordinary way: `busy`, with the live `RequestId` as context, whichever wire asked.
    pub fn on_link_up(&mut self, link: Link, store: &S, ceilings: Ceilings) {
        if self.live_link() == Some(link) {
            self.abandon(store);
        }
        if self.owed.is_some_and(|owed| owed.link == link) {
            self.owed = None;
        }
        self.ceilings[link.index()] = Some(ceilings);
    }

    /// **The link went away** (§3.8's third form of cancel), scoped to the link that went.
    ///
    /// Nothing is answered, because there is nobody left to answer: an error owed to a transfer the
    /// peer can no longer hear is dropped with it. What is *not* dropped is the other link's
    /// transfer — a cable being unplugged is not a reason to kill a phone's download, and the
    /// unscoped version of this method was how it became one.
    pub fn on_link_lost(&mut self, link: Link, store: &S) {
        if self.live_link() == Some(link) {
            self.abandon(store);
        }
        if self.owed.is_some_and(|owed| owed.link == link) {
            self.owed = None;
        }
        self.ceilings[link.index()] = None;
    }

    /// The **device's** half of §3.8's bilateral cancel: "The device cancels by answering the
    /// outstanding `PUT` or `GET` with an error and dropping the transfer."
    ///
    /// Reports whether there was one. The allocation is released or the handle closed exactly as
    /// every other abandonment does, and the transfer's `cancelled` answer goes out on the next
    /// [`poll`](Engine::poll) — the caller is a device-local decision (a ride starting, a battery
    /// below the install threshold, a stream channel that died under a control channel that did
    /// not), not a wire request, so there is no second response to pair it with.
    pub fn cancel_live(&mut self, store: &S, cause: CancelCause) -> bool {
        let Some(request) = self.live_transfer() else { return false };
        let opcode = if matches!(self.live, Live::Upload(_)) { Opcode::Put } else { Opcode::Get };
        // The owning link, read **before** the abandon that forgets it: the answer is owed to the
        // wire the transfer was on, and pumping it out of the other one would hand a client an error
        // for a `RequestId` it never sent.
        let link = self.live_link().expect("live_transfer just answered Some");
        self.abandon(store);
        self.owed = Some(Owed { link, opcode, request, refusal: Refusal::new(ErrorCode::Cancelled, cause.detail()) });
        true
    }

    // -- the opcodes -----------------------------------------------------------------------------

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
            link,
            ceilings,
            request,
            revision: meta.revision,
            payload_len: meta.payload_len,
            payload_crc: meta.payload_crc,
            sent: 0,
            handle,
        });
        // The first record of the payload, so that `Idle` keeps meaning "nothing to do" and a
        // driver that stops pumping on it cannot stall a download.
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
            Err(refusal) => self.emit_error(out, Opcode::Put, request, refusal),
        }
    }

    /// §3.6's admission: every check that must pass before a byte is allocated for.
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
            link,
            ceilings,
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
            stage_bank: 0,
            crc: Crc32::new(),
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

    fn on_cancel(
        &mut self,
        store: &S,
        link: Link,
        request: RequestId,
        transfer: RequestId,
        out: &mut [u8],
    ) -> Reaction {
        // **Both halves: the identifier *and* the wire.** `RequestId` spaces are per client — §3.1
        // makes the client choose them and nothing coordinates two clients — so a phone and a cable
        // picking the same small number is ordinary, not adversarial. Matching on the identifier
        // alone let a `CANCEL` from one link destroy the other link's transfer *and* mint the
        // cancelled error to the asking link, so the victim was killed silently and its peer was
        // never told. This was the one entry point that missed the link identity when the rest of
        // the lifecycle gained it.
        //
        // A `CANCEL` naming a transfer the asking link does not own answers `cancelled = false`,
        // which is §3.8's own honest answer: there is no such transfer *of yours*.
        let live = self.live_transfer();
        let cancelled = live == Some(transfer) && self.live_link() == Some(link);
        if cancelled {
            let opcode = if matches!(self.live, Live::Upload(_)) { Opcode::Put } else { Opcode::Get };
            self.abandon(store);
            // §3.8: the cancelled transfer receives its own error response, and the `CANCEL`
            // receives a different one. The transfer's goes out on the next pump.
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
                // §4 steps 4 and 5: the answer must reach the transport before the reboot.
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
    fn absorb(&mut self, store: &S, payload: &[u8], stage: Option<(usize, &mut [u8])>) -> Result<(), StoreError> {
        let external = stage.is_some();
        let staging: &mut [u8] = match stage {
            Some((_, stage)) => stage,
            None => &mut self.staging,
        };
        let Live::Upload(upload) = &mut self.live else { return Err(StoreError::Invalid) };
        let stage_len = staging.len();
        let base = 0;
        // An oddly-sized record could otherwise fill this bank and need the next one in the same
        // call. The board cannot safely borrow that next bank until this borrow ends because the
        // write starts deferred DMA. Production 4 KiB records divide the 64 KiB bank exactly; a
        // final short record cannot cross it.
        if external && upload.staged + payload.len() > stage_len {
            return Err(StoreError::Invalid);
        }
        upload.crc.update(payload);
        upload.received += payload.len() as u64;
        let mut input = payload;
        while !input.is_empty() {
            // A record at or above one stage goes straight to the card: the copy through staging
            // would buy nothing, and this is the path a bulk USB record takes.
            //
            // **The whole aligned prefix, in one call.** Splitting at exactly `STAGE` would hand a
            // 4,096-byte USB record to the card as eight separate 512-byte writes, and on this card
            // a write command costs about the same whether it carries one block or a hundred
            // (`FLAT_Store_Format.md` §5.5) — so the split, not the copy, was the cost. §5.2's
            // ceiling is chosen to make a full USB stream record exactly 4,096 payload bytes for
            // this reason, and the remainder below is the transfer's last, short record.
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

    /// §3.6's last byte: verify the length and the whole-payload CRC, run the kind's validator, and
    /// commit.
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
        if let Err(error) = self.flush(store, stage) {
            return self.fail_upload(store, media_refusal(error, detail::media_io::WRITE), out);
        }
        let request = self.owed_request();
        match self.publish(store) {
            Ok((id, revision, len, crc)) => {
                // The commit consumed the allocation and the catalog is the result. Nothing is live
                // from here, whatever the response does.
                self.live = Live::Idle;
                self.upload_end = Some((kind, UploadEnd::Committed { id, replaced }));
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

    /// The `RequestId` the live transfer's answer echoes. Zero when there is none, which only a
    /// caller with nothing to answer ever sees.
    fn owed_request(&self) -> RequestId {
        self.live_transfer().unwrap_or(RequestId(0))
    }

    /// The one commit a `PUT` makes: publish the new head, and settle what it displaced.
    fn publish(&mut self, store: &S) -> Result<(ObjectId, Revision, u64, u32), Refusal> {
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
    fn abandon(&mut self, store: &S) {
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

    /// The same for a download, whose only hold is the open handle.
    fn fail_download(&mut self, store: &S, refusal: Refusal, out: &mut [u8]) -> Reaction {
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
        // `allocate` takes no hold row, so this is unreachable from here — mapped rather than
        // funnelled into `Internal`, because a store that ever does say it is the same transient
        // fact the arm above reports and a client should read it the same way.
        StoreError::Busy => Refusal::new(ErrorCode::Busy, detail::busy::HOLDS),
        StoreError::NotFound | StoreError::RevisionConflict { .. } => Refusal::plain(ErrorCode::Internal),
    }
}

/// A refusal from `open`. A full hold table is the same transient fact as a full reservation table,
/// and since FS7.5-c2 it **says which** — `busy` detail `holds 2` rather than a plain `busy`, so a
/// client can tell "another transfer owns the device" from "every read slot is taken".
///
/// `Invalid` still maps to a plain `busy` here for the reservation case, and it is also what a `GET`
/// on a `RESERVED` entry produces — which is a genuine client error the store has no other way to
/// spell at this seam. That conflation is older than this slice and is not resolved by it.
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
        // A commit takes no hold row either. Same reasoning as `allocate_refusal`'s arm: mapped to
        // the transient answer rather than to `Internal`, so a store that ever reports it is read
        // as *ask again* and not as a device defect.
        StoreError::Busy => Refusal::new(ErrorCode::Busy, detail::busy::HOLDS),
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

    /// A full **hold** table says which kind of busy it is. The detail is the half a client's retry
    /// policy reads, and it is frozen in `FLAT_Store_Protocol.md` §3.9 — so it is pinned here rather
    /// than left to whoever next edits the match.
    #[test]
    fn a_full_hold_table_is_busy_with_the_holds_detail() {
        assert_eq!(open_refusal(StoreError::Busy), Refusal::new(ErrorCode::Busy, detail::busy::HOLDS));
        // The other two seams cannot produce it — neither takes a hold row — but they map it rather
        // than funnelling it into `Internal`, so a store that ever does say it reads as *ask again*.
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
    fn the_admission_latch_queries_the_first_frame_of_every_transfer_on_a_channel() {
        let (a, b) = (RequestId(0x2A01), RequestId(0x2A02));
        let mut admission = Admission::new();

        // Transfer A: the leading frame is always queried, and is held while nothing is live.
        assert!(admission.needs_query(a));
        assert!(admission.observed(a, None), "an idle engine must hold the leading frame");
        // Still unadmitted, so the next frame is queried again rather than waved through.
        assert!(admission.needs_query(a));
        assert!(!admission.observed(a, Some(a)), "the engine admitted it — deliver");
        // Now it is a continuation: no further round trip for the rest of the upload.
        assert!(!admission.needs_query(a));

        // **The second transfer on the same channel.** A has ended — which the adapter never sees,
        // because a transfer ends on a *stream* frame — so the latch must not still be answering for
        // it. This is the regression a plain "something was admitted" flag shipped.
        assert!(admission.needs_query(b), "B's leading frame must be queried, not waved through on A");
        assert!(admission.observed(b, None), "B is not admitted yet — hold, do not deliver to an idle engine");
        assert!(!admission.observed(b, Some(b)));
        assert!(!admission.needs_query(b));
        // A's identity is stale now and must not be honoured either.
        assert!(admission.needs_query(a));

        // A frame for a transfer other than the live one is held rather than delivered, and it
        // clears the latch so the *live* transfer's next frame is re-queried rather than trusted.
        assert!(admission.observed(a, Some(b)));
        assert!(admission.needs_query(b));

        // A new channel admits nothing.
        assert!(!admission.observed(b, Some(b)));
        assert!(!admission.needs_query(b));
        admission.reset();
        assert!(admission.needs_query(b));
        assert_eq!(Admission::new(), Admission::default());
    }

    #[test]
    fn the_ble_binding_reads_its_ceilings_off_the_link_and_the_adapters_buffer() {
        // §5.1's own example: the device's preferred 247-byte MTU gives 244 bytes of control.
        let ceilings = Ceilings::for_ble(247, 245, 256).expect("the device's preferred BLE link");
        assert_eq!((ceilings.control(), ceilings.stream()), (244, 245));
        // A link that negotiates upward of the adapter's buffer is clamped to it, both channels.
        let clamped = Ceilings::for_ble(517, 1_024, 256).expect("a large link, small buffer");
        assert_eq!((clamped.control(), clamped.stream()), (256, 256));
        // The floor still applies after the clamp: a buffer under a single-entry page is refused
        // rather than truncated, exactly as an under-floor MTU is.
        assert_eq!(Ceilings::for_ble(247, 245, CONTROL_FLOOR - 1), None);
        // An MTU below the floor is refused however roomy the buffer.
        assert_eq!(Ceilings::for_ble(CONTROL_FLOOR + 2, 245, 4_096), None);
        assert!(Ceilings::for_ble(CONTROL_FLOOR + 3, 245, 4_096).is_some());
        // An `ATT_MTU` too small to subtract the 3-byte header from is `None`, never an underflow.
        for att in [0, 1, 2, 3] {
            assert_eq!(Ceilings::for_ble(att, 245, 4_096), None, "att_mtu {att} underflowed");
        }
        // A stream ceiling that cannot carry a frame header plus one payload byte is refused.
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
