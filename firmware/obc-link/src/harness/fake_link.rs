//! Two fake links that differ only in the physical facts §14 gives their bindings.
//!
//! [`FakeBleLink`] carries one control frame per `objectControl` value and one stream frame per CoC
//! SDU, with the ATT-MTU-derived ceiling (`ATT_MTU - 3`) and a modellable indication timeout.
//! [`FakeUsbLink`] carries `record_length u16` prefixed records on two independent ordered byte
//! streams, with a drain that completes accepted IN records in order.
//!
//! Everything above the framing is identical, and that is the point: the same engine, driven by the
//! same records, must produce the same DOS frames on both. Each link therefore reports what it
//! *sent* as whole DOS frames — [`FakeBleLink::sent`] and [`FakeUsbLink::sent`] — so a test can
//! compare the two runs byte for byte without comparing their framing.

use std::collections::VecDeque;
use std::vec::Vec;

use crate::engine::{ByteLink, LinkCeilings, LinkChannel, LinkContext, LinkError};

/// A fake link, as the harness drives it: a [`ByteLink`] plus the three things a test needs to
/// push records in and read whole DOS records back out.
///
/// The records this trait exposes are always *whole DOS frames*, never the binding's framing, which
/// is what lets one scenario assert byte-identical engine output across two very different
/// bindings.
pub trait FakeLink: ByteLink {
    /// Queues one whole DOS record as if the peer had sent it.
    fn deliver(&mut self, channel: LinkChannel, record: &[u8]);

    /// Every whole DOS record this link has sent on a channel, in order.
    fn sent(&self, channel: LinkChannel) -> &[Vec<u8>];

    /// True once a channel has been closed for untrusted framing.
    fn is_closed(&self, channel: LinkChannel) -> bool;

    /// Moves the link to a new connection generation, as a reconnect does (§3).
    fn set_generation(&mut self, generation: u32);

    /// Replaces the whole connection identity, for a reconnect that authenticates a different
    /// principal on the same link.
    fn set_context(&mut self, context: LinkContext);
}

/// The three bytes of ATT overhead one Write Request or indication value pays (§14.1).
pub const ATT_OVERHEAD: u16 = 3;

/// The USB record length prefix, in bytes (§14.2).
pub const USB_LENGTH_PREFIX: usize = 2;

fn channel_index(channel: LinkChannel) -> usize {
    match channel {
        LinkChannel::Control => 0,
        LinkChannel::Stream => 1,
    }
}

/// The BLE binding: GATT indications for control, an L2CAP CoC for the stream.
#[derive(Debug)]
pub struct FakeBleLink {
    context: LinkContext,
    att_mtu: u16,
    coc_sdu: u16,
    inbound: [VecDeque<Vec<u8>>; 2],
    outbound: [Vec<Vec<u8>>; 2],
    closed: [bool; 2],
    /// When set, the next control indication is never confirmed: the response is lost on the link.
    pub indication_times_out: bool,
    /// Indications that were sent but never confirmed.
    pub unconfirmed: usize,
    /// Records completed by [`ByteLink::drain`], in completion order.
    pub drained: Vec<Vec<u8>>,
    /// Records accepted since the last drain, in acceptance order.
    queued: Vec<Vec<u8>>,
}

impl FakeBleLink {
    /// A link with the device's preferred 247-byte ATT MTU and a 1,024-byte CoC SDU.
    pub fn new(context: LinkContext) -> Self {
        Self::with_limits(context, 247, 1_024)
    }

    /// A link with an explicit ATT MTU and CoC SDU, for the frame-limit derivations of §14.0.
    pub fn with_limits(context: LinkContext, att_mtu: u16, coc_sdu: u16) -> Self {
        FakeBleLink {
            context,
            att_mtu,
            coc_sdu,
            inbound: [VecDeque::new(), VecDeque::new()],
            outbound: [Vec::new(), Vec::new()],
            closed: [false, false],
            indication_times_out: false,
            unconfirmed: 0,
            drained: Vec::new(),
            queued: Vec::new(),
        }
    }

    /// The confirmation for a previously unconfirmed indication finally arrives.
    ///
    /// §14.1's drain waits for confirmations, so a link that has caught up must be able to say so;
    /// otherwise one lost response would fail every later drain for the connection's whole life.
    pub fn confirm(&mut self) {
        self.unconfirmed = 0;
    }

    /// How many records are accepted but not yet completed at the link layer.
    pub fn in_flight(&self) -> usize {
        self.queued.len()
    }

    /// Queues one whole DOS record as if the client had written or sent it.
    pub fn deliver(&mut self, channel: LinkChannel, record: &[u8]) {
        self.inbound[channel_index(channel)].push_back(record.to_vec());
    }

    /// Every whole DOS record this link has sent on a channel, in order.
    pub fn sent(&self, channel: LinkChannel) -> &[Vec<u8>] {
        &self.outbound[channel_index(channel)]
    }

    /// True once a channel has been closed for untrusted framing.
    pub fn is_closed(&self, channel: LinkChannel) -> bool {
        self.closed[channel_index(channel)]
    }
}

impl FakeLink for FakeBleLink {
    fn set_generation(&mut self, generation: u32) {
        self.context.generation = generation;
        self.closed = [false, false];
    }

    fn set_context(&mut self, context: LinkContext) {
        self.context = context;
        self.closed = [false, false];
    }

    fn deliver(&mut self, channel: LinkChannel, record: &[u8]) {
        FakeBleLink::deliver(self, channel, record);
    }

    fn sent(&self, channel: LinkChannel) -> &[Vec<u8>] {
        FakeBleLink::sent(self, channel)
    }

    fn is_closed(&self, channel: LinkChannel) -> bool {
        FakeBleLink::is_closed(self, channel)
    }
}

impl ByteLink for FakeBleLink {
    fn context(&self) -> LinkContext {
        self.context
    }

    fn ceilings(&self) -> LinkCeilings {
        // §14.1: "One ATT Write Request value carries at most `ATT_MTU - 3` bytes, and so does one
        // indication value". The stream ceiling is the CoC's SDU limit.
        LinkCeilings { control_frame: self.att_mtu.saturating_sub(ATT_OVERHEAD), stream_frame: self.coc_sdu }
    }

    fn receive(&mut self, channel: LinkChannel, buffer: &mut [u8]) -> Result<Option<usize>, LinkError> {
        let index = channel_index(channel);
        if self.closed[index] {
            return Ok(None);
        }
        let Some(record) = self.inbound[index].pop_front() else { return Ok(None) };
        // §14.1: one Write Request value is one control frame, and one CoC SDU is one stream frame.
        // A peer cannot hand over more than its channel's ceiling in one record.
        let ceiling = match channel {
            LinkChannel::Control => self.ceilings().control_frame,
            LinkChannel::Stream => self.ceilings().stream_frame,
        };
        if record.len() > usize::from(ceiling) || record.len() > buffer.len() {
            return Err(LinkError::RecordTooLarge);
        }
        buffer[..record.len()].copy_from_slice(&record);
        Ok(Some(record.len()))
    }

    fn send(&mut self, channel: LinkChannel, record: &[u8]) -> Result<(), LinkError> {
        let index = channel_index(channel);
        if self.closed[index] {
            return Err(LinkError::TransportFault);
        }
        let ceiling = match channel {
            LinkChannel::Control => self.ceilings().control_frame,
            LinkChannel::Stream => self.ceilings().stream_frame,
        };
        if record.len() > usize::from(ceiling) {
            return Err(LinkError::RecordTooLarge);
        }
        if channel == LinkChannel::Control && self.indication_times_out {
            // The frame went on air and the confirmation never came: the client sees nothing, and
            // §13's "a lost response is unknown delivery, not a failed mutation" is what follows.
            self.unconfirmed += 1;
            return Err(LinkError::Timeout);
        }
        self.outbound[index].push(record.to_vec());
        self.queued.push(record.to_vec());
        Ok(())
    }

    fn drain(&mut self) -> Result<(), LinkError> {
        // §14.1: the terminal indication "must receive its confirmation ... or the adapter's
        // bounded drain timeout must expire". Records that were confirmed complete in the order
        // they were accepted.
        for record in self.queued.drain(..) {
            self.drained.push(record);
        }
        if self.unconfirmed > 0 {
            return Err(LinkError::Timeout);
        }
        Ok(())
    }

    fn close(&mut self, channel: LinkChannel) {
        let index = channel_index(channel);
        self.closed[index] = true;
        self.inbound[index].clear();
    }
}

/// The USB binding: two independent ordered byte streams with `record_length u16` framing.
#[derive(Debug)]
pub struct FakeUsbLink {
    context: LinkContext,
    max_record: u16,
    inbound: [Vec<u8>; 2],
    outbound: [Vec<Vec<u8>>; 2],
    /// Records handed to the controller but not yet completed at the bus layer.
    queued: [Vec<Vec<u8>>; 2],
    closed: [bool; 2],
    /// Records completed by [`ByteLink::drain`], in completion order.
    pub drained: Vec<Vec<u8>>,
}

impl FakeUsbLink {
    /// A link whose negotiated record maximum is the protocol maximum control frame.
    pub fn new(context: LinkContext) -> Self {
        Self::with_max_record(context, crate::frame::MAX_CONTROL_FRAME as u16)
    }

    /// A link with an explicit record maximum.
    pub fn with_max_record(context: LinkContext, max_record: u16) -> Self {
        FakeUsbLink {
            context,
            max_record,
            inbound: [Vec::new(), Vec::new()],
            outbound: [Vec::new(), Vec::new()],
            queued: [Vec::new(), Vec::new()],
            closed: [false, false],
            drained: Vec::new(),
        }
    }

    /// Queues one whole DOS record, framing it as §14.2 requires.
    pub fn deliver(&mut self, channel: LinkChannel, record: &[u8]) {
        let index = channel_index(channel);
        self.inbound[index].extend_from_slice(&(record.len() as u16).to_le_bytes());
        self.inbound[index].extend_from_slice(record);
    }

    /// Writes raw bytes into the inbound stream, for the malformed-length cases of §14.2.
    pub fn deliver_raw(&mut self, channel: LinkChannel, bytes: &[u8]) {
        self.inbound[channel_index(channel)].extend_from_slice(bytes);
    }

    /// Every whole DOS record this link has accepted for sending, in order.
    pub fn sent(&self, channel: LinkChannel) -> &[Vec<u8>] {
        &self.outbound[channel_index(channel)]
    }

    /// True once a record stream has been reset or closed.
    pub fn is_closed(&self, channel: LinkChannel) -> bool {
        self.closed[channel_index(channel)]
    }

    /// How many records are accepted but not yet completed at the bus layer.
    pub fn in_flight(&self) -> usize {
        self.queued.iter().map(Vec::len).sum()
    }
}

impl FakeLink for FakeUsbLink {
    fn set_generation(&mut self, generation: u32) {
        self.context.generation = generation;
        self.closed = [false, false];
    }

    fn set_context(&mut self, context: LinkContext) {
        self.context = context;
        self.closed = [false, false];
    }

    fn deliver(&mut self, channel: LinkChannel, record: &[u8]) {
        FakeUsbLink::deliver(self, channel, record);
    }

    fn sent(&self, channel: LinkChannel) -> &[Vec<u8>] {
        FakeUsbLink::sent(self, channel)
    }

    fn is_closed(&self, channel: LinkChannel) -> bool {
        FakeUsbLink::is_closed(self, channel)
    }
}

impl ByteLink for FakeUsbLink {
    fn context(&self) -> LinkContext {
        self.context
    }

    fn ceilings(&self) -> LinkCeilings {
        // §14.2: the ceiling is the negotiated record maximum, "not bounded by the endpoint packet
        // size, since a record may span packets".
        LinkCeilings { control_frame: self.max_record, stream_frame: crate::frame::MAX_STREAM_FRAME as u16 }
    }

    fn receive(&mut self, channel: LinkChannel, buffer: &mut [u8]) -> Result<Option<usize>, LinkError> {
        let index = channel_index(channel);
        if self.closed[index] || self.inbound[index].len() < USB_LENGTH_PREFIX {
            return Ok(None);
        }
        let length = usize::from(u16::from_le_bytes([self.inbound[index][0], self.inbound[index][1]]));
        if length == 0 || length > usize::from(self.max_record.max(crate::frame::MAX_STREAM_FRAME as u16)) {
            // §14.2: "A zero, out-of-range, prematurely terminated, or overrun record length is
            // `invalidFrame` and resets only the affected USB record stream".
            self.inbound[index].clear();
            self.closed[index] = true;
            return Err(LinkError::TransportFault);
        }
        if self.inbound[index].len() < USB_LENGTH_PREFIX + length {
            // The record has not arrived whole yet; packet boundaries have no protocol meaning.
            return Ok(None);
        }
        if length > buffer.len() {
            return Err(LinkError::RecordTooLarge);
        }
        buffer[..length].copy_from_slice(&self.inbound[index][USB_LENGTH_PREFIX..USB_LENGTH_PREFIX + length]);
        self.inbound[index].drain(..USB_LENGTH_PREFIX + length);
        Ok(Some(length))
    }

    fn send(&mut self, channel: LinkChannel, record: &[u8]) -> Result<(), LinkError> {
        let index = channel_index(channel);
        if self.closed[index] {
            return Err(LinkError::TransportFault);
        }
        let ceiling = match channel {
            LinkChannel::Control => self.ceilings().control_frame,
            LinkChannel::Stream => self.ceilings().stream_frame,
        };
        if record.len() > usize::from(ceiling) {
            return Err(LinkError::RecordTooLarge);
        }
        self.outbound[index].push(record.to_vec());
        self.queued[index].push(record.to_vec());
        Ok(())
    }

    fn drain(&mut self) -> Result<(), LinkError> {
        // §14.2: "the terminal response record and all earlier IN records must complete at the USB
        // device-controller/bus layer" — in the order they were accepted.
        for index in 0..2 {
            for record in self.queued[index].drain(..) {
                self.drained.push(record);
            }
        }
        Ok(())
    }

    fn close(&mut self, channel: LinkChannel) {
        let index = channel_index(channel);
        self.closed[index] = true;
        self.inbound[index].clear();
    }
}
